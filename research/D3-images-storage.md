# D3 — OCI images, registries & layered filesystems

Research reference. Claim tags: `[F]` fact required by an OCI spec or by Linux (primary source
cited), `[C]` convention (what Docker/containerd/a registry happens to do, cited), `[R]`
recommendation (ours, with tradeoff — never presented as fact), `[O]` empirically verified (host
+ kernel/tool version + exact command). Confidence `HIGH`/`MED`/`LOW`. Unsourced ⇒ LOW.

Companion files: `D1-kernel-isolation.md` (namespaces, `pivot_root`, mount syscall mechanics),
`D2-runtime-architecture.md` (runtime lifecycle, async decision), `D4-networking.md`,
D5 (cgroups/caps/seccomp/userns privilege model). **This file owns *what to mount and where the
bytes come from*; D1 owns *how mounting works inside a namespace*.**

## 0. Method and empirical baseline

| Tag | Host | Kernel / versions | Role here |
|---|---|---|---|
| **HOST-A** | `host-a` = `HOST-A`, Ubuntu 24.10 | `6.11.0-29-generic` x86_64; Docker `28.3.1`; containerd `1.7.27`; overlay2 on extfs | Live overlay mounts, real `/var/lib/docker` layout, modern-kernel overlay behaviour. **Production — read-only; my own experiments confined to a private mount namespace with everything under `/tmp`, unmounted and deleted after.** |
| **LOCAL** | this workstation (macOS), `curl 8.x` | — | Anonymous registry HTTP against `registry-1.docker.io` / `auth.docker.io`. Nothing credentialed. |
| **ROCKY** | `old-host` = `old-host.example.edu`, Rocky 8.8, kernel `4.18` | — | **UNREACHABLE for this session**: `ssh: Could not resolve hostname old-host.example.edu`, repeated over the whole session. Every rootless-overlay claim below is therefore documentary, not `[O]`. See §15.1. |

Registry probes were run from LOCAL rather than HOST-A on purpose: identical results,
zero risk of perturbing the production host's registry rate-limit bucket.

`[O]` LOCAL, `2026-09-05`. All registry captures in §2 are one continuous anonymous session
against Docker Hub. Note the dates in the payloads — `alpine:latest` at capture time was Alpine
`3.24.1`, image index digest
`sha256:6c1a91f698e41bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b`. That digest will move;
the *shape* of everything around it is what matters.

---

## 1. The three OCI specs — exact boundaries and vocabulary

Three separate specifications, three separate GitHub repos, three separate version numbers. They
are joined at exactly two seams. Getting the seams right is most of the conceptual work.

### 1.1 The division of labour

| Spec | Repo | Governs | Ends at |
|---|---|---|---|
| **Image Spec** | `opencontainers/image-spec` | The *artifact*: descriptors, manifest, index, layer changeset format, image **configuration**, the on-disk `image-layout`. | It defines an image and how to convert one into a bundle. It says nothing about how to run anything. |
| **Distribution Spec** | `opencontainers/distribution-spec` | The *transport*: an HTTP API (`/v2/...`) for pulling, pushing, discovering and deleting content addressed by digest, plus tags. | It moves opaque blobs and manifests. It does not care what is inside them. |
| **Runtime Spec** | `opencontainers/runtime-spec` | The *execution*: the **filesystem bundle** (`config.json` + a rootfs directory) and the container lifecycle/operations performed on it. | It starts from a bundle already on local disk. It has no concept of layers, registries, digests or pulling. |

`[F]` HIGH — Image Spec `spec.md`: "This specification defines an OCI Image, consisting of an
image manifest, an image index (optional), a set of filesystem layers, and a configuration."
And: "Once built the OCI Image can then be discovered by name, downloaded, verified by hash,
trusted through a signature, and **unpacked into an OCI Runtime Bundle**."
<https://github.com/opencontainers/image-spec/blob/main/spec.md>

**The seam is the word "unpacked".** Image Spec ends the moment you have (a) an extracted rootfs
directory and (b) an image config you have translated into a runtime `config.json`. Runtime Spec
begins there. The translation itself is specified — `image-spec/conversion.md` — and is
normatively *one-way and lossy*: image config → runtime config.

`[F]` HIGH — Runtime Spec `bundle.md`: "A Standard Container bundle contains all the information
needed to load and run a container… 1. `config.json`: contains configuration data. This REQUIRED
file MUST reside in the root of the bundle directory and MUST be named `config.json`. 2.
container's root filesystem: the directory referenced by `root.path`". Also: "while these
artifacts MUST all be present in a single directory on the local filesystem, that directory
itself is not part of the bundle."
<https://github.com/opencontainers/runtime-spec/blob/main/bundle.md>

The second seam is between Image and Distribution: Distribution moves *descriptor-addressed
content*. It requires manifests to be JSON with a `mediaType`, and it understands `subject` for
the referrers API, but otherwise it treats an image manifest exactly like any other OCI artifact.

### 1.2 The vocabulary, defined precisely

**digest** — `[F]` HIGH. `<algorithm>:<encoded>`, e.g. `sha256:6c3d…`. The Image Spec's
`descriptor.md` gives the grammar and states `sha256` is REQUIRED-to-implement, `sha512`
OPTIONAL. Crucially: "the digest is a *cryptographic* checksum of the content **as it is
transported**" — for a compressed layer that is the digest of the *gzip bytes*, not the tar
bytes. A digest is a content address: the same bytes have the same digest everywhere, which is
the entire basis of dedup, cache reuse, and verification.
<https://github.com/opencontainers/image-spec/blob/main/descriptor.md>

**descriptor** — `[F]` HIGH. The universal pointer: `{mediaType, digest, size}` REQUIRED, plus
OPTIONAL `urls`, `annotations`, `data`, `artifactType`, and (in an index) `platform`. Every
reference from one OCI object to another is a descriptor. `size` is not decoration: it is the
first defence against a decompression/length bomb — you refuse to read more bytes than `size`.

**blob** — `[F]` MED. Distribution-Spec term for an opaque, digest-addressed byte string stored
in a repository, reachable at `/v2/<name>/blobs/<digest>`. Layers and image configs are blobs.
Manifests are *not* blobs in the API — they live under `/v2/<name>/manifests/<ref>` and have
their own verbs (this asymmetry matters: a manifest is addressable by tag *or* digest, a blob
only by digest).

**layer** — `[F]` HIGH. A *changeset*, not a filesystem. Image Spec `layer.md`: layers are
"changesets" applied in order; the format is a tar archive of the *differences* from the previous
layer, with deletions expressed by whiteout entries. A layer is meaningless in isolation; only a
*stack* of layers denotes a filesystem.
<https://github.com/opencontainers/image-spec/blob/main/layer.md>

**rootfs** — overloaded, and the overloading causes bugs. Three distinct meanings:
1. `rootfs` object **inside the image config**: `{"type":"layers","diff_ids":[…]}` — an ordered
   list of *uncompressed* layer digests. `[F]` HIGH, `image-spec/config.md`.
2. `root.path` **in the runtime `config.json`**: a path to a directory on local disk, the thing
   the runtime will `pivot_root` into. `[F]` HIGH, `runtime-spec/config.md`.
3. Informally, "the merged directory a container sees". That is an OverlayFS mountpoint, not
   any spec object.

**manifest** — `[F]` HIGH. `application/vnd.oci.image.manifest.v1+json`. A single-platform
image: `schemaVersion: 2`, `mediaType`, one `config` descriptor, an ordered `layers` array of
descriptors, optional `subject` and `annotations`. It is itself content-addressed; its digest is
what people mean by "the image digest" / `image@sha256:…`.

**index / manifest list** — `[F]` HIGH. `application/vnd.oci.image.index.v1+json`. An array of
manifest descriptors each optionally carrying `platform: {architecture, os, os.version,
os.features, variant, features}`. Docker's older equivalent is
`application/vnd.docker.distribution.manifest.list.v2+json`; structurally near-identical.
"Manifest list" and "index" are the same idea in two vocabularies. Also called a "fat manifest".

**image config** — `[F]` HIGH. `application/vnd.oci.image.config.v1+json`. Contains
`architecture`, `os`, `rootfs.diff_ids`, `history[]`, and a `config` object holding
`Env`/`Cmd`/`Entrypoint`/`User`/`WorkingDir`/`ExposedPorts`/`Volumes`/`Labels`/`StopSignal`.

**runtime `config.json`** — `[F]` HIGH. A *completely different document* in a different spec:
`ociVersion`, `root`, `process`, `mounts[]`, `linux.namespaces`, `linux.resources`,
`linux.seccomp`, `hooks`, … Nothing in it is content-addressed; it lives as a plain file in the
bundle directory.

> **The single most confusing naming collision in the whole ecosystem:** the *image config* is a
> content-addressed blob in the registry describing how an image was built and what it defaults
> to; the *runtime `config.json`* is a plain local file describing exactly how this one container
> instance will be executed. They share zero fields by name except the coincidence that the image
> config has a sub-object also called `config`. Our code must never call both of them `Config`.
> `[R]` HIGH — name the Rust types `ImageConfig` and `RuntimeSpec` (or `Spec`, matching
> `oci-spec`'s naming), never `Config`.

**registry** — `[F]` MED. Distribution Spec: a service implementing the `/v2/` HTTP API. The spec
defines conformance in four independently-claimable categories: **pull**, **push**, **content
discovery**, **content management**.
<https://github.com/opencontainers/distribution-spec/blob/main/spec.md>

**runtime** — `[F]` HIGH. Runtime Spec: the thing that consumes a bundle and implements
`create`/`start`/`kill`/`delete`/`state`. `runc`, `crun`, and (eventually) us. D2 owns this.

### 1.3 What each spec does *not* define — the gaps we must fill ourselves

`[F]` HIGH, by omission from all three specs:

- **How to resolve a name to a registry.** `alpine` → `docker.io/library/alpine:latest` appears
  in no OCI spec. §2.1.
- **A local content store layout for a running engine.** Image Spec's `image-layout` defines an
  *interchange* directory (a tarball you `skopeo copy` around); it is explicitly not designed as
  a live engine's store, and neither Docker nor containerd uses it as one. §7.
- **Snapshots, storage drivers, copy-on-write.** Entirely out of scope for OCI. OverlayFS is our
  choice, not a requirement. The specs only require that the layer stack, when applied in order
  with whiteout semantics, yields a particular directory tree.
- **Anything about signing, SBOMs, provenance** beyond the `subject`/referrers hook.
- **How layers map to overlay lowerdirs, or in what order.** §8.

---

## 2. The complete `docker pull alpine` trace

Every step below was executed. Commands are given verbatim so they can be re-run.

### 2.1 Name resolution — almost entirely convention, not spec

Input `alpine`. Output `docker.io/library/alpine:latest`, fetched from host
`registry-1.docker.io`, path `/v2/library/alpine/manifests/latest`.

`[C]` HIGH — every one of these expansions is Docker convention, implemented in
`github.com/distribution/reference` and reused by containerd, podman, skopeo and BuildKit:

| Rule | Applied | Source |
|---|---|---|
| No registry component ⇒ default registry `docker.io` | `alpine` → `docker.io/alpine` | `distribution/reference` `normalize.go`, `ParseNormalizedNamed` |
| Registry `docker.io` and single-component path ⇒ prefix `library/` | → `docker.io/library/alpine` | same |
| No tag and no digest ⇒ tag `latest` | → `…:latest` | same |
| `docker.io` ⇒ contact `registry-1.docker.io` | | Docker's `RegistryHost` mapping — `index.docker.io` and `docker.io` are the *index*; `registry-1.docker.io` is the *registry* endpoint |
| A path component containing `.` or `:` or equal to `localhost` ⇒ treat as a registry hostname | `myhost:5000/foo` splits; `foo/bar` does not | `splitDockerDomain` |

`[F]` HIGH — the Distribution Spec only constrains the *repository name grammar*:
`[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*`, and tags:
`[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`. It says nothing about defaults, `library/`, or `latest`.

`[R]` HIGH — implement the four expansion rules as a small, well-tested pure function with a
table of cases, and make the default registry configurable. Reason: it is ~80 lines, it is where
every "why did it try to pull `library/myhost:5000/foo`" bug lives, and depending on a crate for
it buys nothing. Tradeoff: `docker.io` special-casing is ugly, but omitting it means `alpine`
does not work, which fails the interoperability bar in §12.

`[R]` MED — the "single-component ⇒ `library/`" rule must apply **only** when the registry is
`docker.io`. Applying it to `ghcr.io/foo` or a local registry is a real bug class.

### 2.2 Step 1 — the unauthenticated probe and the Bearer challenge

```
$ curl -si https://registry-1.docker.io/v2/
HTTP/2 401
content-type: application/json
docker-distribution-api-version: registry/2.0
www-authenticate: Bearer realm="https://auth.docker.io/token",service="registry.docker.io"

{"errors":[{"code":"UNAUTHORIZED","message":"authentication required","detail":null}]}
```

`[O]` LOCAL HIGH.

`[F]` HIGH — Distribution Spec: "the registry MUST implement `GET /v2/`… A `200 OK` response
SHOULD be returned… A `401 Unauthorized` response SHOULD be returned to indicate that the client
must authenticate." So the 401 here is *the specified handshake*, not an error.

`[F]` HIGH — the challenge itself is plain RFC 7235 `WWW-Authenticate`, scheme `Bearer`, with
RFC 6750 auth-param syntax. The `realm`/`service`/`scope` params are **not** in either RFC —
they are the Docker token-auth protocol.

Two important empirical details:

`[O]` LOCAL HIGH — **the challenge on `/v2/` carries no `scope`**; the challenge on an actual
resource does:

```
$ curl -sI https://registry-1.docker.io/v2/library/alpine/manifests/latest | grep -i www-auth
www-authenticate: Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/alpine:pull"
```

`[O]` LOCAL HIGH — a **nonexistent** private repository returns exactly the same 401 with a
correctly-formed scope, not a 404:

```
$ curl -sI https://registry-1.docker.io/v2/mansurpro/definitelynotarealrepo/manifests/latest
HTTP/2 401
www-authenticate: Bearer realm=...,scope="repository:mansurpro/definitelynotarealrepo:pull"
```

`[C]` HIGH — this is deliberate: registries hide the existence of private repos behind 401. A
client must therefore treat "401 after a successful token exchange" as *possibly* "does not
exist", and must not report "authentication failed" when the user is anonymous and the repo is
simply private-or-absent.

`[R]` HIGH — **do not pre-flight `GET /v2/`.** Go straight at the resource you want, and drive
authentication off the 401 you get from it — that challenge carries the scope you actually need,
so it costs one round trip instead of two and never guesses a scope wrong. Tradeoff: you lose an
early "is this even a v2 registry" signal; in practice the resource 401/404 tells you that too.

### 2.3 Step 2 — token exchange

```
$ curl -s 'https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/alpine:pull'
{"token":"eyJ…","access_token":"eyJ…","expires_in":300,"issued_at":"2026-09-05T12:49:10.768679153Z"}
```

`[O]` LOCAL HIGH. Note **`expires_in: 300`** — five minutes. Anonymous Docker Hub tokens are
short-lived; a large multi-layer pull can outlive one.

`[C]` HIGH — `token` and `access_token` are the same value. `access_token` is the OAuth2-shaped
field; `token` is the original Docker field. A client should prefer `token` and fall back to
`access_token` — some registries (notably Azure ACR historically) only emit one of the two.

`[O]` LOCAL HIGH — decoding the JWT payload (base64url of the second dot-segment) exposes the
rate limit *before you ever hit it*:

```json
{"access":[{"actions":["pull"],"name":"library/alpine",
  "parameters":{"pull_limit":"100","pull_limit_interval":"21600","registry_region":"us-east-1"},
  "type":"repository"}],
 "aud":"registry.docker.io","exp":1788612850,"iat":1788612550,"iss":"auth.docker.io",
 "jti":"dckr_jti_…","nbf":1788612250,"sub":""}
```

`[O]` LOCAL HIGH — anonymous Docker Hub as of 2026-09-05: **`pull_limit=100` per
`pull_limit_interval=21600` seconds (6 hours), keyed by source IP** (`sub` is empty ⇒ anonymous).
`[C]` MED — Docker's published policy has changed repeatedly (100/6h anon → 100/6h by IP →
various "unauthenticated 10/hour" announcements); the JWT is the authoritative live value for
*your* token, and reading it is cheaper than reading the pricing page.

`[O]` LOCAL HIGH — separately, response headers on manifest requests report a *different*
window:

```
x-ratelimit-limit: 100;w=3600
x-ratelimit-remaining: 99;w=3600
docker-ratelimit-source: 198.51.100.20
```

`w=3600` here versus `21600` in the token. `[R]` MED — treat both as advisory, and drive backoff
off `429` + `Retry-After` rather than off either counter. Reason: the two disagree, the format
is RFC-9331-draft-ish (`limit;w=window`), and no registry other than Hub is obliged to send it.

`[F]` MED — Distribution Spec defines `TOOMANYREQUESTS` as an error code; `429` handling is the
portable path.

**Scope grammar** `[C]` HIGH: `repository:<name>:<action>[,<action>]`, actions
`pull`/`push`/`delete`. Cross-repository blob mount needs *two* scopes in one token request
(repeat the `scope=` parameter). `registry:catalog:*` is the scope for `/v2/_catalog`.

`[R]` HIGH — implement the auth layer as: issue request → on 401, parse `WWW-Authenticate` →
fetch token from `realm` with `service` + `scope` (+ HTTP Basic of the user's credentials if any,
per `[C]` Docker's flow: `Authorization: Basic base64(user:pass)` to the *token* endpoint) →
cache the token keyed by `(realm, service, scope)` with its `expires_in` → retry once. Retry
**once only**; a second 401 is a real failure. Tradeoff: this is a small state machine that must
sit *underneath* every request including redirect-following, so build it as a middleware layer,
not as a call at the top of `pull()`.

`[C]` HIGH — some registries answer the challenge with `WWW-Authenticate: Basic` instead of
`Bearer` (plain `docker run registry:2` with htpasswd). Handle both.

### 2.4 Step 3 — HEAD vs GET the manifest, and `Docker-Content-Digest`

```
$ curl -sI -H "Authorization: Bearer $TOK" \
  -H 'Accept: application/vnd.oci.image.index.v1+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.list.v2+json,application/vnd.docker.distribution.manifest.v2+json' \
  https://registry-1.docker.io/v2/library/alpine/manifests/latest
HTTP/2 200
content-type: application/vnd.oci.image.index.v1+json
content-length: 9218
docker-content-digest: sha256:6c1a91f698e41bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
etag: "sha256:6c1a91f698e41bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b"
```

`[O]` LOCAL HIGH.

`[F]` HIGH — Distribution Spec: a registry "MUST" support `HEAD /v2/<name>/manifests/<reference>`
and the response "SHOULD" include `Docker-Content-Digest`. `[O]` LOCAL HIGH — Hub sends it, and
also sends `ETag` with the identical value.

**This is the cheap tag-freshness check.** `[R]` HIGH — `HEAD` the tag, compare
`Docker-Content-Digest` to what we already have; if equal, the pull is a no-op and we spent one
request. Docker's `pull` does exactly this (`Image is up to date for …`). Tradeoff: `HEAD`
requests count against the rate limit on Hub the same as `GET` (`[C]` MED — Docker's own docs
said HEAD stopped being free in 2024; the `x-ratelimit-remaining` header decremented on our GET
but we did not isolate HEAD; see §15).

`[R]` HIGH — **never trust `Docker-Content-Digest` for verification.** Use it only as a hint
(cache key, freshness). Verification means: hash the body you actually received and compare it to
the digest *you asked for*. `[O]` LOCAL HIGH — verified for real:

```
$ curl -s … /v2/library/alpine/manifests/latest > alpine-index.json
$ shasum -a 256 alpine-index.json
6c1a91f698e41bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
```

Byte-identical to the header. `[F]` HIGH — this only works because manifests are verified as
*exact bytes*: the Image Spec's canonicalization note requires that a manifest be stored and
served byte-for-byte as pushed. **Never re-serialize a manifest and re-hash it.** Any
`serde_json` round-trip changes key order/whitespace and destroys the digest.

`[R]` HIGH — our `Manifest` type must retain the raw bytes alongside the parsed struct
(`struct FetchedManifest { raw: Bytes, digest: Digest, parsed: Manifest }`). This single decision
prevents a whole class of "digest mismatch on re-push" bugs.

### 2.5 Step 4 — the `Accept` header, and the surprise that Docker Hub ignores it

The header list a modern client should send, in one line:

```
Accept: application/vnd.oci.image.index.v1+json,
        application/vnd.oci.image.manifest.v1+json,
        application/vnd.docker.distribution.manifest.list.v2+json,
        application/vnd.docker.distribution.manifest.v2+json
```

`[C]` HIGH — this is (modulo ordering and the extra `manifest.v1+prettyjws` some clients still
append) what containerd sends.

`[O]` LOCAL HIGH — **Docker Hub does not content-negotiate for these repositories. It returns
whatever media type the artifact was pushed as, regardless of `Accept`.** Tested four ways
against `library/alpine:latest`:

| `Accept:` sent | `content-type` returned |
|---|---|
| Docker list + Docker manifest only (no OCI) | `application/vnd.oci.image.index.v1+json` |
| OCI index + OCI manifest only | `application/vnd.oci.image.index.v1+json` |
| `*/*` | `application/vnd.oci.image.index.v1+json` |
| header entirely absent | `application/vnd.oci.image.index.v1+json` |

Same result for `library/busybox`, `library/ubuntu`, `library/hello-world` — all four now serve
an **OCI image index** at `:latest`.

`[O]` LOCAL HIGH — but *older pushes keep their original type*, again regardless of `Accept`:

```
library/alpine:3.9  -> content-type: application/vnd.docker.distribution.manifest.list.v2+json
library/alpine:2.6  -> content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
```

`alpine:2.6` returns **Schema 1** even when the `Accept` header explicitly excludes it
(`Accept: …oci.image.index…,…oci.image.manifest…,…docker…manifest.v2+json`).

Two hard consequences:

1. `[R]` HIGH — **`Accept` is a request, not a contract. Always branch on the response
   `Content-Type` (and, if absent, on the body's own `mediaType` field).** A client that assumes
   it got what it asked for will mis-parse. Tradeoff: none; this is strictly correct.
2. `[C]` HIGH — the once-universal advice "Docker Hub down-converts to whatever you accept" is
   **false today**. The registry stores and serves bytes; conversion (if any) happened at push
   time.

`[F]` MED — Distribution Spec says a client "SHOULD" set `Accept` and a registry that cannot
satisfy it SHOULD return `406 Not Acceptable`. Hub returns 200 with a type you did not ask for.
Real-world registries deviate; parse defensively.

### 2.6 Step 5 — the index, and platform selection

Body (abridged, 9218 bytes, 20 entries):

```json
{"manifests":[
 {"digest":"sha256:79ff19e9084a00eece421b2523fb93e22d730e2c0e525905de047e848e56d95f",
  "mediaType":"application/vnd.oci.image.manifest.v1+json",
  "platform":{"architecture":"amd64","os":"linux"},"size":1022,
  "annotations":{"org.opencontainers.image.version":"3.24.1", …}},
 {"digest":"sha256:f21951a6120df0f5f9329311202f7869e7b70120f4748632d58753cff662b126",
  "mediaType":"application/vnd.oci.image.manifest.v1+json",
  "platform":{"architecture":"unknown","os":"unknown"},"size":838,
  "annotations":{"vnd.docker.reference.type":"attestation-manifest",
                 "vnd.docker.reference.digest":"sha256:79ff19e…"}},
 …]}
```

`[O]` LOCAL HIGH — **half the entries in a modern official-image index are not images.** For every
real platform manifest there is a paired **attestation manifest** with
`platform: {"architecture":"unknown","os":"unknown"}` and the BuildKit annotations
`vnd.docker.reference.type=attestation-manifest` / `vnd.docker.reference.digest=<the manifest it
attests>`.

`[R]` HIGH — **platform selection must be a positive match, never "the first entry" and never
"skip until something matches".** The concrete rule:

```
candidates = index.manifests
  .filter(m => m.platform.os == "linux" && m.platform.architecture == "amd64")
  .filter(m => m.platform.variant is None || m.platform.variant == wanted_variant)
  .filter(m => m.mediaType is a manifest or index type)
```

and then error loudly if zero candidates. Tradeoff: an explicit filter is a few more lines than
`find_first`, but `unknown/unknown` entries are now the norm, and a naive matcher that falls
through to "take entry 0" will happily try to run an in-toto attestation as a rootfs.

`[F]` HIGH — `image-index.md`: `platform.architecture` and `platform.os` are REQUIRED when
`platform` is present, and their values "SHOULD" be `GOARCH`/`GOOS` values.
<https://github.com/opencontainers/image-spec/blob/main/image-index.md>

`[C]` HIGH — the `variant` matching table everyone reimplements (from containerd
`platforms/database.go` + `platforms/defaults_unix.go`):

| arch | variants, most→least preferred |
|---|---|
| `amd64` | `""` (also `v1`), and containerd 1.7+ additionally matches `v2`,`v3`,`v4` micro-arch levels |
| `arm64` | `v8` ≡ `""` |
| `arm` | `v8`,`v7`,`v6`,`v5` — an `armv7` host can run `v7`,`v6`,`v5` |

`[R]` HIGH for us: x86_64 first ⇒ hard-match `linux/amd64` with `variant` ignored, and leave a
`Platform` struct with `os`/`arch`/`variant` so adding `arm64` later is a match-table change and
not a refactor. Tradeoff: we cannot run `arm64` images; we were not going to anyway.

`[F]` HIGH — an index MAY contain nested indexes (`mediaType` of an entry may itself be an index
type). `[R]` MED — support one level of nesting with a recursion depth cap of 4 and a visited-digest
set. Reason: a self-referential or mutually-referential index chain is a trivial DoS otherwise.

### 2.7 Step 6 — the platform manifest

```
$ curl -sD- -H "Authorization: Bearer $TOK" \
  -H 'Accept: application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json' \
  https://registry-1.docker.io/v2/library/alpine/manifests/sha256:79ff19e9084a00eece421b2523fb93e22d730e2c0e525905de047e848e56d95f
```

```json
{"schemaVersion":2,
 "mediaType":"application/vnd.oci.image.manifest.v1+json",
 "config":{"mediaType":"application/vnd.oci.image.config.v1+json",
           "digest":"sha256:1a8c3406406e97ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b",
           "size":611},
 "layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar+gzip",
            "digest":"sha256:94a16efeeef22bb5e5045f32dafee56272ffd89860bac26f6c32123439af26a4",
            "size":3846391}],
 "annotations":{…}}
```

`[O]` LOCAL HIGH — `content-length: 1022` exactly matches the `size` the index advertised, and
`shasum -a 256` of the body equals the digest we requested. Both checks are free; do both.

`[R]` HIGH — enforce `size` as a **hard read limit**, not a post-hoc comparison: cap the body
reader at `size` bytes and fail if the stream has more. Manifest bodies should additionally get an
absolute ceiling (`[R]` MED: 4 MiB — containerd uses a similar guard) because `size` itself comes
from the untrusted index.

### 2.8 Step 7 — blobs, the CDN redirect, and range support

```
$ curl -sI -H "Authorization: Bearer $TOK" \
  https://registry-1.docker.io/v2/library/alpine/blobs/sha256:55afa1ec…
HTTP/2 200
content-length: 3846391
accept-ranges: bytes
cache-control: max-age=31536000
docker-content-digest: sha256:55afa1ec…
etag: "sha256:55afa1ec…"
```

```
$ curl -s -o /dev/null -w '%{http_code} %{redirect_url}\n' -H "Authorization: Bearer $TOK" \
  https://registry-1.docker.io/v2/library/alpine/blobs/sha256:55afa1ec…
307 https://production.cloudfront.docker.com/registry-v2/docker/registry/v2/blobs/sha256/55/55afa1ec…/data?Expires=…&Signature=…&Key-Pair-Id=…
```

`[O]` LOCAL HIGH — **HEAD is answered by the registry directly with `200`; GET is answered with
`307` to a CloudFront presigned URL.** Do not assume symmetry between HEAD and GET on blobs.

`[F]` HIGH — Distribution Spec explicitly allows this: the registry "MAY" respond to a blob GET
with a `307 Temporary Redirect` and the client "MUST" follow it. `[C]` HIGH — Docker Hub, GHCR,
ECR, GCR, and Quay all do this in production.

Three properties of the redirect target, all measured `[O]` LOCAL HIGH:

- **Sending our `Authorization: Bearer` header to CloudFront works (200) and omitting it also
  works (200).** The presigned URL carries its own auth in the query string.
- `[R]` **HIGH — strip `Authorization` when the redirect crosses to a different host anyway.**
  Reason: it happens to be harmless against CloudFront, but a registry that redirects to an
  attacker-controlled or third-party host would otherwise be handed our registry bearer token.
  This is the exact shape of CVE-2020-15157 ("containerd credential leak via
  `foreign layer` URLs" — a malicious manifest with `urls` pointing at an attacker host caused
  containerd to send registry credentials there). `reqwest` does strip sensitive headers on
  cross-origin redirect by default; `curl` does not unless `--location-trusted` is omitted…
  verify whichever client we pick rather than assuming.
- **`Range: bytes=0-99` on the CDN URL returns `206` with 100 bytes.** Resumable and parallel
  chunked layer downloads are available (RFC 7233 / RFC 9110 §14).

`[R]` MED — do **not** build range-resume for the MVP. Reason: layers are a few MB to a few
hundred MB, a failed download is cheap to redo, and resume-correctness interacts badly with
streaming digest verification (you must keep a partial hash state or re-hash from zero). Add it
when someone actually pulls a 2 GB CUDA image over a bad link. Tradeoff: accepted, and the
`accept-ranges` support is recorded here so we know it is a pure add-on later.

`[R]` HIGH — the presigned URL has an `Expires` in the query string (~1 hour observed). If a
download stalls past it, re-issue the *registry* GET to get a fresh redirect; do not retry the
stale CDN URL.

### 2.9 Step 8 — the config blob and digest verification

```
$ curl -sL -H "Authorization: Bearer $TOK" \
  https://registry-1.docker.io/v2/library/alpine/blobs/sha256:d529dd0c…
{"architecture":"amd64",
 "config":{"Env":["PATH=/usr/local/sbin:…:/bin"],"Cmd":["/bin/sh"],"WorkingDir":"/"},
 "created":"2026-06-16T00:01:29.967161902Z",
 "history":[
   {"created":"…","created_by":"ADD alpine-minirootfs-3.24.1-x86_64.tar.gz / # buildkit",
    "comment":"buildkit.dockerfile.v0"},
   {"created":"…","created_by":"CMD [\"/bin/sh\"]","comment":"buildkit.dockerfile.v0",
    "empty_layer":true}],
 "os":"linux",
 "rootfs":{"type":"layers",
           "diff_ids":["sha256:21ee5abca34e63fce933ed7c39c0e045631af0ed86d5cc0dfbdf9fdca426ce3c"]}}
```

`[O]` LOCAL HIGH — `shasum -a 256` = `d529dd0c…`, matching the manifest's `config.digest`.

Note `history` has **two entries but `diff_ids` has one**: the `CMD` step carries
`"empty_layer": true`. §4.4.

### 2.10 The trace as an algorithm

`[R]` HIGH — the pull state machine, in the order we should implement it:

```
1. normalize(ref)                        -> (registry, repository, tag|digest)
2. GET  /v2/<repo>/manifests/<ref>       -> 401 -> token(realm,service,scope) -> retry
3. branch on response Content-Type:
     index/manifest-list -> select platform -> goto 2 with the selected digest
     manifest            -> continue
     schema1             -> refuse (see §3.4)
4. verify: sha256(raw manifest bytes) == requested digest (skip only when ref was a tag,
   in which case adopt Docker-Content-Digest as the resolved digest AFTER hashing the body
   yourself and using YOUR hash as the identity)
5. GET  /v2/<repo>/blobs/<config.digest>  (follow 307, strip Authorization cross-host)
   verify sha256 == config.digest ; parse ImageConfig
6. cross-check: manifest.layers.len() == config.rootfs.diff_ids.len()   <-- MUST, see §4.5
7. for each layer descriptor, in order:
     if store already has diff_ids[i] -> skip the download entirely (dedup)
     else GET blob, stream through: [ sha256 of compressed ] -> [ decompress ]
                                    -> [ sha256 of uncompressed ] -> [ tar extractor ]
     verify compressed hash == descriptor.digest
     verify uncompressed hash == config.rootfs.diff_ids[i]
8. commit each layer atomically; compute chain IDs; record tag -> manifest digest -> config
```

`[R]` HIGH on step 7's shape: **one pass, two hashes, decompression and extraction all
streaming.** Reason: writing the compressed blob to disk then decompressing then extracting is
three passes over hundreds of MB and doubles peak disk. Tradeoff: streaming means we cannot
verify the compressed digest *before* extracting — we discover corruption after having written
files. The fix is to extract into a temporary directory and only `rename` it into the store after
both digests verify (§7.6). Never extract straight into the final location.

`[R]` MED — download layers concurrently (bounded, ~3–4 at a time) but **commit them in order**.
Reason: chain IDs are ordered, and out-of-order commits complicate crash recovery for no gain.

---
## 3. Media types and manifest versions

### 3.1 The complete table a modern client meets in the wild

`[F]` HIGH for OCI rows (`image-spec/media-types.md`); `[C]` HIGH for Docker rows
(`distribution/distribution` `docs/spec/manifest-v2-2.md`).

| Media type | Kind | Must we read? | Must we write? |
|---|---|---|---|
| `application/vnd.oci.image.index.v1+json` | index | **yes** — official images serve this today | later |
| `application/vnd.docker.distribution.manifest.list.v2+json` | index | **yes** — older tags, and most non-Hub registries | no |
| `application/vnd.oci.image.manifest.v1+json` | manifest | **yes** | later |
| `application/vnd.docker.distribution.manifest.v2+json` | manifest | **yes** | no |
| `application/vnd.oci.image.config.v1+json` | config | **yes** | later |
| `application/vnd.docker.container.image.v1+json` | config | **yes** | no |
| `application/vnd.oci.image.layer.v1.tar+gzip` | layer | **yes** | later |
| `application/vnd.docker.image.rootfs.diff.tar.gzip` | layer | **yes** | no |
| `application/vnd.oci.image.layer.v1.tar` | layer | yes (trivial) | no |
| `application/vnd.oci.image.layer.v1.tar+zstd` | layer | eventually (§3.5) | no |
| `application/vnd.docker.distribution.manifest.v1+json` / `…v1+prettyjws` | Schema 1 | see §3.4 | **never** |
| `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` | foreign/non-distributable layer | reject or handle carefully (§6.13) | never |
| `application/vnd.oci.image.layer.nondistributable.v1.tar*` | ditto, OCI, **deprecated** | ditto | never |
| `application/vnd.oci.empty.v1+json` | empty descriptor placeholder (`{}`, 2 bytes, digest `sha256:44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a`) | yes, if you touch artifacts | no |

### 3.2 OCI vs Docker Schema 2: the differences that matter

`[F]` HIGH — `image-spec/media-types.md` "Compatibility Matrix" enumerates the *entire*
difference set:

- **index vs manifest-list:** OCI adds `.annotations`, `.manifests[].annotations`,
  `.manifests[].urls`. Nothing else differs.
- **manifest:** OCI adds `.annotations`, `.config.annotations`, `.config.urls`,
  `.layers[].annotations`. Nothing else differs.
- **layer:** `application/vnd.oci.image.layer.v1.tar+gzip` and
  `application/vnd.docker.image.rootfs.diff.tar.gzip` are declared "**Interchangeable and fully
  compatible mime-types**" — the *bytes* are the same format.
- **config:** Docker's has `.config.Memory`, `.config.MemorySwap`, `.config.CpuShares`,
  `.config.Healthcheck` which OCI reserves-but-does-not-define, plus the Windows-only
  `.config.ArgsEscaped` (deprecated in OCI, kept for compatibility).

Also `[C]` HIGH: OCI 1.0 manifests were *not required* to carry a top-level `mediaType` field;
OCI 1.1 made it RECOMMENDED-to-required in practice, and the Distribution Spec requires
manifests pushed to carry it. Real-world OCI 1.0 manifests exist with no `mediaType`.

`[R]` HIGH — **model one internal type and parse both wire formats into it.** Concretely:

```rust
// One struct. serde ignores unknown fields by default; the Docker/OCI delta
// is entirely "OCI has extra optional fields", so the OCI struct is a superset.
struct Manifest { schema_version: u32, media_type: Option<String>,
                  config: Descriptor, layers: Vec<Descriptor>,
                  subject: Option<Descriptor>, annotations: Option<Map> }
```

Dispatch on the *response* `Content-Type`, falling back to the body's `mediaType`, falling back
to structural sniffing (`manifests` key present ⇒ index; `fsLayers` present ⇒ Schema 1).
Tradeoff: structural sniffing is a mild code smell but it is what containerd effectively does,
and it saves us from registries that send `application/json` or nothing.

`[R]` HIGH — normalize layer media types to an internal enum
`{Tar, TarGzip, TarZstd, Foreign(..)}` at parse time, so extraction code never sees a string.

### 3.3 What breaks if we support only OCI media types

`[O]` LOCAL HIGH, measured (§2.5): `alpine`, `busybox`, `ubuntu`, `hello-world` at `:latest`
all serve `application/vnd.oci.image.index.v1+json`. **An OCI-only client works for these today.**

`[O]` LOCAL HIGH: `alpine:3.9` serves `application/vnd.docker.distribution.manifest.list.v2+json`.
An OCI-only client **fails** on it, and on essentially every image pushed before ~2022 and every
image pushed by an older `docker push` since.

`[C]` HIGH — non-Hub registries: GHCR/Quay/ECR store what you push; a `docker push` from Docker
Engine < 23 pushes Docker Schema 2. Enormous amounts of that exist.

`[R]` HIGH — **read both Docker Schema 2 and OCI from day one.** The incremental cost is two more
strings in a match arm because the structs are compatible (§3.2). The cost of *not* doing it is
that half the registry world is unreachable. This is the highest ratio of interoperability to
effort in the entire pull path.

### 3.4 Is Schema 1 dead?

`[C]` HIGH — Docker Engine **removed Schema 1 pull support in v26.0** (2024); containerd removed
schema1 conversion in 2.0. `distribution/distribution` v3 removed schema1 storage. Every modern
toolchain refuses to *push* it.

`[O]` LOCAL HIGH — **but Docker Hub still serves it.**

```
$ curl -sI -H "Authorization: Bearer $TOK" \
  -H 'Accept: application/vnd.oci.image.index.v1+json,application/vnd.oci.image.manifest.v1+json,application/vnd.docker.distribution.manifest.v2+json' \
  https://registry-1.docker.io/v2/library/alpine/manifests/2.6
HTTP/2 200
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
```

Body starts:

```json
{"schemaVersion":1,"name":"library/alpine","tag":"2.6","architecture":"amd64",
 "fsLayers":[{"blobSum":"sha256:2a3ebcb7fbcc29bf40c4f62863008bb573acdea963454834d9483b3e5300c45d"}],
 "history":[{"v1Compatibility":"{\"id\":\"7a747d3b0848…\",\"created\":\"2016-01-18T…\",…}"}], …}
```

Note it was served **even though the `Accept` header excluded Schema 1**, confirming §2.5.

Schema 1's design problems, which is why it is dead `[C]` HIGH:
- `fsLayers` is in **reverse order** relative to Schema 2's `layers` (topmost first).
- No image config blob; the config is a JSON *string* embedded in each `history[].v1Compatibility`
  entry, and the "real" config is the first one.
- Layer identity is the `blobSum` (compressed digest); there are no `diff_ids`, so **chain IDs
  cannot be computed without downloading and decompressing every layer**.
- The manifest is JWS-signed, and the signature covers a *canonicalized* form, so the manifest's
  own digest is unstable across registries. This is exactly what CVE-2014-8178/CVE-2014-8179 and
  the later "manifest malleability" issues were about.

`[R]` HIGH — **do not implement Schema 1.** Detect `schemaVersion == 1` and return a clear
error: `"manifest schema 1 is not supported (image was pushed before 2017); ask the publisher to
re-push, or use a mirror"`. Reason: implementing it means implementing JWS canonicalization,
reverse layer order, and a second config model, for images that are a decade old. Tradeoff:
`alpine:2.6` and friends will not pull. That is the same behaviour as Docker 26+, so we are not
worse than the incumbent.

### 3.5 gzip vs zstd

`[F]` HIGH — both are first-class OCI layer media types (`media-types.md`), gzip per RFC 1952,
zstd per RFC 8478.

`[C]` HIGH — **gzip is what you will actually receive.** `[O]` LOCAL HIGH: `alpine:latest`'s
single layer is `application/vnd.oci.image.layer.v1.tar+gzip`. zstd layers exist (BuildKit
`--output type=image,compression=zstd`, and `nerdctl`/containerd support them) but are rare on
public registries because a zstd layer is unpullable by Docker < 23 and by any older client.

`[R]` HIGH — MVP: gzip only, and error clearly on `+zstd`. Add zstd when we build a pusher or hit
a real image. Reason: `zstd` the crate is a C-binding (`zstd-sys`) and `ruzstd` (pure Rust) is
decode-only and less battle-tested; adding either now buys nothing measurable. Tradeoff: a `+zstd`
image fails; that is a one-line change to fix later since the decompressor sits behind one trait.

`[R]` MED — design the decompression stage as `Box<dyn Read>` selected by media type, so
gzip/zstd/none is a match arm and not an architecture.

### 3.6 The `mediaType` mismatch rule (a real footgun)

`[F]` HIGH — `media-types.md` "Media Type Conflicts": if the returned media type does not match
the one your descriptor expected, you SHOULD **respect the expected media type if the digest
matches**, MAY warn, and MUST error if the digest does not match or you have no expected digest.

`[R]` HIGH — implement exactly that: the descriptor's `mediaType` wins over the HTTP
`Content-Type` **for blobs** (where we have an expected digest); the HTTP `Content-Type` wins for
the *first* manifest fetch (where we have no expected descriptor because we asked by tag).
`[O]` LOCAL HIGH — Hub sends `content-type: application/octet-stream` on blob GETs, so obeying
the HTTP header for blobs would break everything. Measured on the alpine config blob.

---

## 4. Layer format: tar, digests, `diff_id`, and chain IDs

### 4.1 A layer is a tar changeset, not a filesystem image

`[F]` HIGH — `image-spec/layer.md`:
- "Layer Changesets … MUST be packaged in tar archive."
- "Layer Changesets … MUST NOT include duplicate entries for file paths."
- File types covered: regular files, directories, sockets, symbolic links, block devices,
  character devices, FIFOs.
- Attributes that MUST be preserved: `mtime`, `uid`, `gid`, `mode`, **xattrs**, symlink
  `linkname`, hardlink `linkname`.
- "Sparse files SHOULD NOT be used because they lack consistent support across tar
  implementations."
- "Layer Changesets of media type `application/vnd.oci.image.layer.v1.tar` are **_applied_,
  rather than simply extracted** as tar archives." — the verb matters; see whiteouts, §5.

`[O]` LOCAL HIGH — what an actual alpine layer looks like:

```
$ gzip -dc alpine-layer.tgz | tar tvf - | head -6
drwxr-xr-x  0 0 0      0 Jun 13 12:38 bin/
lrwxrwxrwx  0 0 0      0 Jun 13 12:38 bin/arch -> /bin/busybox
lrwxrwxrwx  0 0 0      0 Jun 13 12:38 bin/ash -> /bin/busybox
-rwxr-xr-x  0 0 0 804616 Jan 10  2026 bin/busybox
$ gzip -dc alpine-layer.tgz | dd bs=512 count=1 2>/dev/null | xxd -s 257 -l 8
00000101: 7573 7461 7200 3030   ustar.00
```

Magic `ustar\0` + version `00` ⇒ **POSIX.1-1988 ustar**, not GNU (`ustar  \0`) and not PAX-only.
Paths are relative and **without** a leading `./` here — but the spec's own examples use `./etc/…`,
so both forms occur. `[R]` HIGH — normalize by stripping any leading `./` and `/` before doing
anything else.

`[O]` LOCAL HIGH — the gzip header is `1f 8b 08 00 00000000 00 ff`: `MTIME=0`, `XFL=0`,
`OS=0xff` (unknown). BuildKit zeroes these for reproducibility. `[R]` MED — if we ever *produce*
layers, do the same, or identical content will yield different compressed digests.

### 4.2 Two digests per layer, and why both must exist

For alpine's single layer, measured `[O]` LOCAL HIGH:

```
$ shasum -a 256 alpine-layer.tgz
94a16efeeef22bb5e5045f32dafee56272ffd89860bac26f6c32123439af26a4   # == manifest.layers[0].digest
$ gzip -dc alpine-layer.tgz | shasum -a 256
21ee5abca34e63fce933ed7c39c0e045631af0ed86d5cc0dfbdf9fdca426ce3c   # == config.rootfs.diff_ids[0]
compressed 3,846,391 bytes -> uncompressed 8,697,856 bytes  (2.26x)
```

| Name | What it hashes | Where it lives | What it identifies |
|---|---|---|---|
| **blob digest** (a.k.a. compressed digest) | the transported bytes — the *gzip stream* | `manifest.layers[i].digest`; the URL `/v2/<repo>/blobs/<digest>` | a **transport artifact**. Recompressing the same tar with a different gzip level changes it. |
| **`diff_id`** | the *uncompressed tar stream* | `config.rootfs.diff_ids[i]` | the **content** of the changeset. Invariant across compression choices and across registries. |

`[F]` HIGH — `image-spec/config.md` defines `rootfs.diff_ids` as "an array of layer content
hashes (`DiffIDs`), in order from first to last", and `descriptor.md` defines the descriptor
digest as the digest of the content as transported. Two different things by construction.

**Why both must exist** — the operational answer, which is the point of the whole design:
1. The registry can only verify what it stores, which is the compressed blob. So the *transport*
   identity must be the compressed digest.
2. Local dedup and cache reuse must be compression-invariant: the same `alpine` base layer
   arriving as a gzip blob from Hub and as a differently-gzipped blob from a mirror must be
   recognised as *the same layer* and stored once. Only `diff_id` gives you that.
3. Chain IDs (§4.5) are built from `diff_id`s, so the snapshot/overlay cache is
   compression-invariant too.

`[R]` HIGH — **a puller must hash both, in one streaming pass.** The pipeline:

```
HTTP body ──► Sha256Reader(compressed) ──► GzDecoder ──► Sha256Reader(uncompressed) ──► tar::Archive
                     │                                          │
                 must equal                                 must equal
            manifest.layers[i].digest                config.rootfs.diff_ids[i]
```

Both hashes are only known at EOF, i.e. after we have already written files. Hence: extract to a
staging dir, verify, then `rename` (§7.6). Tradeoff: one extra `rename` per layer versus the
alternative of buffering the whole blob in memory or on disk first. `rename` wins.

`[R]` HIGH — **failing either check must be fatal and must delete the staged output.** A layer
whose `diff_id` does not match is a substituted rootfs. Do not "warn and continue".

### 4.3 Ordering

`[F]` HIGH — `config.md`: `diff_ids` are ordered "from first to last", i.e. `[0]` is the **base**
layer and `[n-1]` is the topmost. `manifest.layers` uses the same order. `[C]` HIGH — Docker
Schema 2 also uses base-first. Only Schema 1's `fsLayers` is reversed.

**This is the array whose direction is the opposite of `lowerdir=`'s.** See §8.3. Getting one of
the two backwards yields an image that mounts successfully and is silently wrong — old files
shadowing new ones.

### 4.4 `history` and `empty_layer`

`[O]` LOCAL HIGH — alpine has `history.len() == 2` but `diff_ids.len() == 1`:

```json
"history":[
 {"created":"…","created_by":"ADD alpine-minirootfs-3.24.1-x86_64.tar.gz / # buildkit"},
 {"created":"…","created_by":"CMD [\"/bin/sh\"]","empty_layer":true}]
```

`[F]` HIGH — `config.md`: `history[].empty_layer` is "This field is used to mark if the history
item created a filesystem diff. It is set to true if this history item doesn't correspond to an
actual layer in the rootfs section."

`[R]` HIGH — the correspondence is: **walk `history` in order, skip entries with
`empty_layer: true`, and the i-th remaining entry corresponds to `diff_ids[i]`.** Anything else
(zipping the arrays directly, or assuming equal lengths) is wrong. `history` is purely
informational — `docker history` output and nothing more — so `[R]` MED: parse it, store it, do
not depend on it for correctness. It is also entirely attacker-controlled text; never render it
without escaping.

### 4.5 Chain IDs — how a *stack* is identified

`[F]` HIGH — `image-spec/config.md` §"Layer ChainID":

```
ChainID(L0)      = DiffID(L0)
ChainID(L0…Ln)   = SHA256( ChainID(L0…Ln-1) + " " + DiffID(Ln) )
```

The inputs are the full `sha256:<hex>` strings, joined by a single ASCII space, and the output is
written as a bare hex digest (Docker) or `sha256:<hex>` (containerd).

`[O]` HOST-A HIGH — verified against a real Docker install. Layer record
`/var/lib/docker/image/overlay2/layerdb/sha256/04bd4da11784f6b4c9255b68318a58d531f4025b5245147919ab614b51c7d1cb/`
contains:

```
parent: sha256:d3b8c36020492e5d44c2f18788ffa5ffe4a4161bfd90a07321affc1226e6e933
diff:   sha256:7ac624c6dffc0404b738fed2de9bd0595379da220b172b73ea0a4a76d500f21f
```

and

```python
>>> hashlib.sha256(b"sha256:d3b8c360…e6e933 sha256:7ac624c6…00f21f").hexdigest()
'04bd4da11784f6b4c9255b68318a58d531f4025b5245147919ab614b51c7d1cb'   # == the directory name
```

Exact match. `[C]` HIGH — **Docker's `layerdb` is keyed by chain ID**, and each record's `parent`
is the parent's chain ID.

**Why chain IDs matter — the cache-reuse argument.** A `diff_id` identifies a changeset. A
changeset is only meaningful applied *to a specific stack*. Two images may share `diff_ids[0..k]`
and diverge after; the *extracted, merged* filesystem after k layers is identical for both, and
that merged state is what a snapshot/overlay lowerdir set represents. Chain ID is the content
address **of the applied result of a prefix of the layer list**. Therefore:

- Two images with the same chain ID at depth k can share every lowerdir up to k. That is the
  dedup that makes "20 images all based on `debian:bookworm`" cost one copy of debian.
- A cache lookup for "do I already have this stack?" is `store.has_chain(chain_id)` — an O(1)
  lookup, no per-layer set comparison.
- `diff_id` alone is *not* sufficient as a snapshot key: the same `diff_id` applied over different
  parents produces different results.

`[R]` HIGH — **key the layer/snapshot store by chain ID, and key the blob store by digest.** Two
maps, two purposes:

```
blobs:      digest(compressed) -> file bytes            (transport cache, optional)
layers:     diff_id            -> extracted diff dir    (content dedup)
snapshots:  chain_id           -> {parent_chain_id, diff_id, lowerdir list}  (stack dedup)
```

Tradeoff: three indices instead of one is more metadata, but each answers a different question
and collapsing them forces a linear scan somewhere. This is exactly the split Docker uses
(`overlay2/<cache-id>/diff`, `layerdb/sha256/<chainID>`) and containerd uses (content store keyed
by digest, snapshotter keyed by chain ID).

`[R]` MED — store chain IDs as `sha256:<hex>` strings, matching containerd, not bare hex like
Docker. Reason: unambiguous and future-proof if `sha512` ever appears.

---

## 5. Whiteouts: tar convention → OverlayFS representation

This is where the Image Spec's abstract "removal" meets a concrete kernel filesystem, and the two
use completely different encodings. The translation happens **at extraction time**, once, and it
is the reason `docker pull` needs privileges that a plain `tar -x` does not.

### 5.1 What the Image Spec says

`[F]` HIGH — `image-spec/layer.md` §Whiteouts, verbatim points:

- "A whiteout file is an empty file with a special filename that signifies a path should be
  deleted."
- "A whiteout filename consists of the prefix `.wh.` plus the basename of the path to be deleted."
- "A `.wh.` file, without a basename to delete, is invalid and implementations SHOULD return an
  error when encountering such an entry."
- "As files prefixed with `.wh.` are special whiteout markers, it is not possible to create a
  filesystem which has a file or directory with a name beginning with `.wh.`."
- "Once a whiteout is applied, the whiteout itself MUST also be hidden."
- "**Whiteout files MUST only apply to resources in lower/parent layers.**"
- "Files that are present in the same layer as a whiteout file can only be hidden by whiteout
  files in subsequent layers."
- "Note that regardless of the path being deleted, the whiteout file is a regular file in the
  archive."
- "Implementations SHOULD generate layers such that the whiteout files appear before sibling
  directory entries."

**Opaque whiteout** `[F]` HIGH:
- "An opaque whiteout entry is a file with the name `.wh..wh..opq` indicating that all siblings
  are hidden in the lower layer."
- "An opaque whiteout file hides *all* children of the `bin/` including sub-directories and all
  descendants."
- "**`a/.wh..wh..opq` is applied first, before creating the new version of `a/b`, regardless of
  the ordering in which the whiteout file was encountered.**"
- "Implementations SHOULD generate layers using *explicit whiteout* files, but MUST accept both."

That last ordering rule is a genuine trap: a tar entry that appears *after* other entries in the
same directory must be honoured as if it came *first*. `[R]` HIGH — extraction must therefore be
**two-pass per directory**, or equivalently: scan for `.wh..wh..opq` entries and apply their
effect before processing any other entry under that directory. Single-pass streaming extraction
that applies entries in stream order is **wrong** for opaque whiteouts. Tradeoff: this argues for
buffering directory-level metadata during a streaming extract, which is cheap (paths only), not
for buffering the whole tar.

`[F]` HIGH — §"Changeset over existing files": if the entry and existing path are both
directories, replace attributes only; **in all other cases MUST do the semantic equivalent of
`unlink` then recreate**. Not "overwrite in place". This matters for symlinks: extracting a
regular file over an existing symlink must remove the symlink first, or you write *through* it.
That is the core of the symlink-escape defence in §6.3.

### 5.2 What OverlayFS uses instead

`[F]` HIGH — `Documentation/filesystems/overlayfs.rst`:
- **Whiteout:** "A whiteout is created as a character device with 0/0 device number or as a
  zero-size regular file with the xattr `trusted.overlay.whiteout`." When found in the upper
  layer it hides the corresponding name in the lower layer(s) and "is itself hidden" from the
  merged view.
- **Opaque directory:** "A directory is made opaque by setting the xattr
  `trusted.overlay.opaque` to `"y"`." Its lower counterpart is then ignored entirely.
- With `-o userxattr`, those xattr names become `user.overlay.whiteout` / `user.overlay.opaque`.

### 5.3 The translation table

| Image Spec (tar) | OverlayFS (upper/diff dir) | Privilege needed |
|---|---|---|
| `dir/.wh.name` | `mknod dir/name c 0 0`, mode 0000 | **`CAP_MKNOD`** in the mounting user ns |
| `dir/.wh..wh..opq` | `setxattr(dir, "trusted.overlay.opaque", "y")` | **`CAP_SYS_ADMIN`** (the `trusted.*` namespace) |
| — same, rootless | `setxattr(dir, "user.overlay.opaque", "y")` + mount `-o userxattr` | none |
| — alt whiteout, kernel ≥ 5.8ish | zero-size regular file + `trusted.overlay.whiteout` xattr | `CAP_SYS_ADMIN` |

`[F]` HIGH on the privilege claims: `mknod(2)` of a character device requires `CAP_MKNOD`
(`mknod(2)` man page: "creating a device special file requires `CAP_MKNOD`"), and
`xattr(7)`/`setxattr(2)`: "the `trusted.*` namespace … access … is limited to processes with the
`CAP_SYS_ADMIN` capability."

`[O]` HOST-A HIGH — the same char-device convention is what Docker actually writes. Confirmed by
the mount options on a live container showing `nouserxattr` (§8.6), i.e. this daemon uses the
`trusted.*` namespace, which it can because it runs as real root.

### 5.4 The cost of the translation, and the rootless problem

**Cost, running as root:** essentially zero. One `mknod` per deleted path, one `setxattr` per
opaque dir. Whiteouts are a tiny fraction of entries in real images.

**Cost, running rootless:** this is the whole rootless-storage problem in one place.

`[C]` HIGH — a process in a user namespace *does* hold `CAP_MKNOD` within that namespace, but
`mknod(2)` of a device node still fails: the kernel refuses device creation by a process whose
capabilities are only namespaced, because a device node is a global object. The concrete errno
observed by every rootless container project is **`EPERM`** from `mknod()` inside a userns, and
on many setups the mount is additionally `nodev` so even a created node is inert.

`[C]` HIGH — the two production answers:

1. **`fuse-overlayfs`** — a FUSE implementation that stores whiteouts as `user.*` xattrs it
   defines itself and does not need `mknod`. This is what Podman used by default for years on
   kernels without rootless overlay.
2. **kernel overlay + `-o userxattr`** (Linux ≥ 5.11, `[F]` HIGH per `overlayfs.rst`: "the
   `userxattr` mount option … the overlay implementation will use the `user.overlay.` xattr
   namespace instead of `trusted.overlay.`"), combined with **unprivileged overlayfs mounts in a
   userns** (Linux ≥ 5.11, Miklos Szeredi's series). Whiteouts still need `mknod`, so
   `containers/storage` and `fuse-overlayfs` both additionally accept a **zero-size regular file
   with the whiteout xattr** as an alternative encoding.

`[R]` HIGH — for our root-first MVP, write `mknod c 0 0` + `trusted.overlay.opaque` and be done.
`[R]` HIGH — but **isolate the whiteout writer behind a two-method trait now**
(`fn whiteout(&self, dir, name)`, `fn set_opaque(&self, dir)`), because the rootless variant is a
different pair of syscalls, not a different architecture. Tradeoff: one trait with one impl today
violates "no interface with one implementation" — accepted here specifically because the second
implementation is a known, dated requirement (D5's rootless track), not speculation.

`[R]` MED — **`user.*` xattrs are not preserved by `tar` on many filesystems and are lost across
some copy paths.** If we ever store a rootless layer and then re-mount it as root, the opaque
markers must be re-read from the same namespace they were written to. Do not mix namespaces in
one store; record which namespace a layer directory was written with, in the layer metadata.

### 5.5 The other direction (diff/commit) — not MVP

Producing a layer *from* an overlay upperdir is the inverse translation: walk the upperdir,
emit `.wh.<name>` for every char-dev-0/0, emit `.wh..wh..opq` for every dir carrying the opaque
xattr, emit everything else normally. `[C]` HIGH — this is what `docker commit` / `docker diff`
do via `graphdriver/overlay2`'s `Diff()` and containerd's `archive.Diff`. `[R]` HIGH — out of
scope until we have a build/commit story; note it here so the whiteout trait is designed
symmetric (`to_whiteout` / `from_whiteout`) rather than retrofitted.

`[C]` MED — Docker also stores **`tar-split.json.gz`** per layer. `[O]` HOST-A HIGH — present in
every `layerdb/sha256/<chainID>/` directory, e.g. 372 bytes for a small layer. It is the metadata
needed by `vbatts/tar-split` to reassemble the **byte-exact original tar stream** from the
extracted files, so that `docker push` of a pulled image reproduces the original *compressed
digest* rather than a new one. `[R]` MED — we do not need it until we push. Record the decision:
without tar-split, a re-push of a pulled image will produce different blob digests (harmless but
wasteful, and breaks cross-registry blob mounting). Cost of adding later: it must be captured
*during extraction*, so retrofitting means re-extracting every layer. That is a genuine "decide
early" item — see §11.5.

---
## 6. Extraction hazards — the most dangerous code we will write

A layer tar is **attacker-controlled input from the internet** that we parse *as root* and turn
into files, permissions, device nodes and capabilities on the host. There is no other place in
the engine where a parser bug is this directly a root compromise. This section enumerates every
hazard with the concrete mitigation.

The single structural decision that kills most of the list at once:

`[R]` **HIGH — every filesystem operation during extraction goes through a `SafeRoot` handle that
opens paths with `openat2(2)` and `RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS`, relative to a
directory fd for the extraction root.** Never construct an absolute path string and hand it to
`std::fs`.

`[O]` HOST-A 6.11.0-29 HIGH — `openat2` is present (syscall 437) and clamps as documented:

```python
# dfd = open("/tmp", O_RDONLY|O_DIRECTORY); how = {flags=O_RDONLY, mode=0, resolve=RESOLVE_IN_ROOT}
syscall(437, dfd, b"..", how, 24)  ->  fd 4
readlink("/proc/self/fd/4")        ->  "/tmp"      # ".." clamped to the root, not "/"
```

`[F]` HIGH — `openat2(2)`: `RESOLVE_IN_ROOT` — "Treat the directory referred to by `dirfd` as the
root directory while resolving `pathname`. Absolute symlinks and `..` path components will be
interpreted as if `dirfd` were the root." Added in **Linux 5.6**. `RESOLVE_NO_MAGICLINKS` blocks
`/proc/*/fd/*` style jumps. This is a *kernel-enforced* chroot for path resolution, with no
TOCTOU window, unlike every userspace "resolve then act" scheme.

`[C]` HIGH — this is exactly the fix applied to `boxlite` for **RUSTSEC-2026-0148 / CVE-2026-46703**
(CVSS 10.0, "OCI layer symlink escape → arbitrary host write"): "The fix in v0.9.0 routes every
destructive filesystem operation through a `SafeRoot` handle (`openat2(RESOLVE_IN_ROOT)` on Linux,
lexical fallback elsewhere) so that no tar entry can resolve outside the extraction root, even
with adversarial symlinks placed by earlier entries in the same layer."
<https://rustsec.org/advisories/RUSTSEC-2026-0148.html>

`[R]` MED — since we are Linux-only with a modern-kernel dev target, we can require `openat2` and
hard-fail on kernels < 5.6 rather than maintaining a lexical fallback. Tradeoff: excludes RHEL 8
(kernel 4.18) as a *host*. Given "primary dev target is a modern-kernel VM" that is acceptable;
if RHEL 8 support ever matters, the fallback is containerd's `fs.RootPath` algorithm (§6.3).

### 6.1 Path traversal via `..`

**Attack:** entry named `../../etc/shadow`, or `foo/../../..//etc/cron.d/x`.

`[C]` HIGH — containerd normalizes first: `hdr.Name = filepath.Clean(hdr.Name)` then splits and
resolves the parent through `fs.RootPath(root, ppath)`
(`containerd/archive/tar.go`, `applyNaive`).

`[R]` HIGH — reject rather than sanitize. Concretely, before touching the filesystem:
- Reject any component equal to `..`.
- Reject an empty name, `.`, or a name that `Clean`s to `.` or `/`.
- Strip exactly one leading `./`, then reject any remaining leading `/` (§6.2).
- Reject a NUL byte anywhere in the name (Rust `CString::new` will error; do it explicitly so the
  error message is useful).

Reason for reject-over-sanitize: a sanitizer that silently rewrites `../x` to `x` changes the
image's contents, and any disagreement between our sanitizer and the builder's is a
correctness bug. A malicious layer is a hard error. Tradeoff: a handful of real-world images with
sloppy tars will fail; that has not been a problem for containerd, which errors too.

### 6.2 Absolute paths

**Attack:** entry named `/etc/passwd`, or a symlink `linkname: /etc`.

`[F]` MED — the Image Spec's examples use `./`-prefixed relative names throughout; nothing
normatively forbids a leading `/`, so a defensive extractor must handle it. GNU tar strips leading
`/` and warns.

`[R]` HIGH — with `openat2(RESOLVE_IN_ROOT)` an absolute path is *automatically* reinterpreted
relative to the root, so this hazard is structurally eliminated for the entry name. Do it anyway
belt-and-braces: strip leading `/` from `hdr.name`.

**Symlink targets are the separate, harder case** — see §6.3.

### 6.3 Symlink-target escape, including the TOCTOU variant

This is the number-one real-world container-image extraction vulnerability. Three named instances:

- `[C]` HIGH **CVE-2019-1002101** (`kubectl cp`): "an attacker could craft a malicious tar with a
  header containing a symbolic link to virtually any path, and a subsequent header to a file
  inside a directory with the same name as the symlink" — extraction then writes through the
  symlink.
  <https://unit42.paloaltonetworks.com/disclosing-directory-traversal-vulnerability-kubernetes-copy-cve-2019-1002101/>
- `[C]` HIGH **CVE-2018-15664** (`docker cp`): a *TOCTOU* variant — the container replaces a path
  component with a symlink between the daemon's `resolve` and its `open`, so a valid-at-check-time
  path becomes an escape at use-time.
  <https://medium.com/@cpuguy83/docker-symlink-traversal-cve-2018-15664-explained-3f75cd4259ff>
- `[C]` HIGH **RUSTSEC-2026-0148 / CVE-2026-46703** (`boxlite`, a Rust OCI runtime): "A crafted
  layer containing a symlink whose target is an absolute on-host path (e.g. `escape -> /tmp`)
  followed by a file entry that resolves through that symlink (e.g. `escape/<path>/pwned.txt`)
  caused the extractor to write the payload to the host filesystem outside the intended rootfs
  directory." CVSS 10.0.

The *two-entry* structure is the essential shape and the reason naive per-entry validation fails:
**entry 1 is legal in isolation** (creating a symlink named `escape` pointing at `/tmp` is a
perfectly valid thing for a layer to do — real images contain symlinks to absolute paths, e.g.
`/bin/sh -> /bin/busybox`). **Entry 2 is legal in isolation** (`escape/pwned.txt` contains no
`..`). Only the *composition* escapes. So you cannot validate names; you must constrain
*resolution*.

`[C]` HIGH — how the mature implementations handle it:

- **containerd**: userspace resolution. `fs.RootPath(root, ppath)` in
  `containerd/continuity/fs/path.go` walks each component, `Lstat`s it, and if it is a symlink,
  re-joins the target *inside root*, with `linksWalked > 255 ⇒ errTooManyLinks`. The file is then
  created at `filepath.Join(ppath, base)` — note only the **parent** is resolved, and the basename
  is joined without following. Hardlinks get their own bounded resolver, `hardlinkRootPath`.
- **`containers/image` / `containers/storage`**: historically the `chrootarchive` approach —
  fork a helper that `chroot`s into the destination and untars from inside, so the kernel enforces
  containment. Heavier (a fork+exec per layer) but structurally sound.
- **`tar-rs` (Rust)**: `[C]` HIGH — **got this wrong as recently as 2026.** RUSTSEC-2026-0067 /
  CVE-2026-33056, affecting `tar <= 0.4.44`, fixed in 0.4.45: "when unpacking a tar archive, the
  tar crate's `unpack_dir` function uses `fs::metadata()` to check whether a path that already
  exists is a directory. Because `fs::metadata()` follows symbolic links, a crafted tarball
  containing a symlink entry followed by a directory entry with the same name causes the crate to
  treat the symlink target as a valid existing directory — and subsequently apply `chmod` to it."
  <https://rustsec.org/advisories/RUSTSEC-2026-0067.html>
- `[C]` HIGH — `async-tar` (CVE-2025-62518, "TARmageddon") and `astral-tokio-tar`
  (CVE-2025-59825) both had path-traversal-on-unpack in 2025; the latter's
  `allow_external_symlinks` control "could be bypassed via a pair of symlinks that individually
  point within the destination but combine to point outside of it."
  <https://github.com/advisories/GHSA-3wgq-wrwc-vqmv>

`[R]` HIGH — **`RESOLVE_IN_ROOT` makes both the escape and the TOCTOU variant impossible**,
because resolution happens once, atomically, in the kernel, with the clamp applied. There is no
window between check and use because there is no check.

`[R]` HIGH — additionally, **create symlinks with `symlinkat` into the parent dirfd, and store the
target string verbatim** (do not resolve or rewrite it). A symlink pointing at `/etc` is
*correct* inside the container's rootfs; it only becomes dangerous if *we* follow it during
extraction. Rewriting targets would corrupt real images.

`[R]` MED — cap symlink resolution depth explicitly even with `RESOLVE_IN_ROOT` (the kernel
returns `ELOOP` on its own at 40, which is fine) and log the offending entry name.

### 6.4 Hardlink escape

**Attack:** `typeflag == '1'` (`TypeLink`) with `linkname: /etc/shadow`, or `linkname:
../../../etc/shadow`. `link(2)` on the host then creates a *second name* for the host's shadow
file inside the rootfs — full read, and full write if the container can chmod it.

`[F]` HIGH — `image-spec/layer.md`: hardlinks are stored "with type of a `1` char"; "The
corresponding files that share the link with the >1 linkcount may be outside the directory that
the changeset is being produced from, in which case the `linkname` is not recorded"; "Extracting a
layer with hardlink references to files outside of the layer may fail."

`[C]` HIGH — containerd resolves the target through the dedicated `hardlinkRootPath(extractDir,
hdr.Linkname)`, which splits the linkname, runs `fs.RootPath` on the parent, and joins the base —
the same bounded resolution as for entry names.

`[R]` HIGH — resolve `linkname` with `openat2(RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS)` from the
extraction root and use `linkat(2)` with `AT_EMPTY_PATH` on the resulting fd (requires
`CAP_DAC_READ_SEARCH`) or `linkat(dirfd, name, ...)` with the resolved parent fd. Never pass a
raw string to `link`. `[R]` MED — additionally require that the resolved target is a regular file
that this same layer or a lower layer already created; a hardlink to a path that does not exist
should be an error, not a silent skip. Reason: a "skip" here turns a detectable attack into a
subtly broken image.

`[R]` MED — `sysctl fs.protected_hardlinks=1` (default on most distros) prevents an *unprivileged*
user from hardlinking to a file they do not own. It does **not** protect us: we run as root during
extraction, and root bypasses the check. Do not rely on it.

### 6.5 Device nodes

**Attack:** a layer contains `dev/sda` as a block device 8:0, or `dev/mem` as char 1:1. If
extracted for real and the container's rootfs is not mounted `nodev`, the container reads raw
disk.

`[F]` HIGH — `image-spec/layer.md` lists block devices and character devices as legal entry types,
so we cannot simply reject them without breaking spec compliance.

`[C]` HIGH — containerd's `skipFile(hdr)` returns true for `TypeBlock`/`TypeChar` **only when
running in a user namespace** (`userns.RunningInUserNS()`), because `mknod` would fail there
anyway; as real root it creates them via `handleTarTypeBlockCharFifo`.

`[R]` HIGH — create them faithfully (we must, to distinguish a real device node from an overlay
whiteout — §5.3 — and to make `/dev/null` in a `scratch`-ish image work), and **rely on the
mount-time defence instead**: the container's rootfs and every writable mount inside it get
`MS_NODEV`. That is D1/D5 territory; flag it in the handoff. Reason: extraction-time rejection
breaks legitimate images; mount-time `nodev` is a blanket kernel-enforced guarantee that also
covers device nodes the *container itself* creates at runtime.

`[R]` HIGH — **one exception that is not optional:** a `char 0:0` node is indistinguishable from a
whiteout marker. `[F]` HIGH per `overlayfs.rst`, overlay treats char 0/0 in the upper layer as a
whiteout. So a layer legitimately containing a char 0/0 device would be swallowed. Accept this —
it is a spec-level ambiguity the whole ecosystem lives with — but **do reject a `.wh.`-prefixed
name that is not a regular file**, per the spec's "regardless of the path being deleted, the
whiteout file is a regular file in the archive."

### 6.6 setuid / setgid bits

**Attack:** the layer ships `/usr/bin/whatever` mode `04755` owned by uid 0. Inside the container
this is normal (that is how `su`/`ping` work). It becomes a host problem if (a) the host and
container share a uid namespace and (b) any host user can reach the extracted path — which they
can, because `/var/lib/<engine>/…/diff/` is a normal host directory.

`[F]` HIGH — `chown(2)`: "When the owner or group of an executable file is changed by an
unprivileged user, the `S_ISUID` and `S_ISGID` mode bits are cleared." As root the bits are
*not* cleared by `chown`, but the kernel's behaviour differs by path, which is why order matters.

`[C]` HIGH — containerd's `createTarFile` does `Lchown` **first**, then `lchmod` last, with the
comment "call lchmod after lchown since lchown can modify the file mode". `[R]` HIGH — copy that
ordering exactly. Getting it backwards silently drops setuid from every binary in every image, and
`su` stops working in a way that takes a day to debug.

`[R]` HIGH — **`chmod` the extracted tree's root to `0700` and own it by root** so no host user
can traverse into a layer diff and execute a setuid binary out of it. `[O]` HOST-A HIGH — this is
what Docker does: `/var/lib/docker` is `drwx--x---  root root` and `/var/lib/docker/overlay2` is
`drwx--x---`. The `--x` on group is what lets the daemon's unprivileged helpers traverse without
listing.

`[R]` MED — offer a `--no-setuid` extraction mode that masks `S_ISUID|S_ISGID|S_ISVTX` for
untrusted images. Not MVP, but cheap and worth a flag later.

### 6.7 xattrs, and `security.capability` specifically

`[F]` HIGH — `image-spec/layer.md` lists "Extended Attributes (`xattrs`)" among attributes that
MUST be preserved.

`[C]` HIGH — xattrs travel in tar as **PAX extended headers with the Schily prefix**:
`SCHILY.xattr.<name>`. containerd writes them with
`hdr.PAXRecords[paxSchilyXattr+"security.capability"] = string(capability)` in the diff writer and
reads them back in `createTarFile` by iterating `hdr.PAXRecords` for the `SCHILY.xattr.` prefix.
`[C]` MED — GNU tar uses `RHT.security.selinux` and libarchive uses `LIBARCHIVE.xattr.<urlencoded>`
in some modes; a robust reader should recognise `SCHILY.xattr.` at minimum.

**The dangerous one is `security.capability`.** `[F]` HIGH — `capabilities(7)`: a file's
`security.capability` xattr grants file capabilities (`CAP_NET_RAW` on `ping`, and in the worst
case `cap_setuid=ep`, which is equivalent to setuid-root). Writing it requires `CAP_SETFCAP`.
A layer that carries `security.capability = cap_sys_admin+ep` on a binary is a privilege
escalation for anything that executes it.

`[R]` HIGH — **allowlist xattr namespaces at extraction.** Concretely:
- Accept `user.*` unconditionally (harmless, and needed for rootless whiteouts).
- Accept `security.capability` **only** when the image is trusted, or when a user namespace with
  a non-identity mapping will confine it (`[F]` HIGH — since Linux 4.14 file capabilities are
  namespaced via `security.capability` v3 with a rootid; a v2 capability written by host-root
  applies in *all* namespaces).
- **Reject `security.selinux`, `security.ima`, `security.evm`, and everything in `trusted.*`** —
  `trusted.*` requires `CAP_SYS_ADMIN` and is exactly the namespace overlay uses for its own
  whiteout/opaque markers, so a layer that carries `trusted.overlay.opaque` is trying to forge
  overlay metadata.
- `[C]` HIGH — containerd tolerates `EPERM`/`ENOTSUP` on `setxattr` and merely warns; that is
  *availability*-friendly but not the security posture we want for `trusted.*`.

`[R]` HIGH — this is a **must-have for MVP**, not a later hardening item. It is a ~15-line match
on the xattr name and it closes a full-privilege hole.

### 6.8 uid/gid ownership: root vs unprivileged

`[F]` HIGH — layers record numeric `uid`/`gid`. Running as root we `lchown` faithfully, so a file
owned by uid 0 in the image is owned by *host* uid 0 on disk. With no user namespace (D1: Docker's
default is 6 namespaces, **no userns**), container root **is** host root, so this is honest.

`[C]` HIGH — containerd's `noSameOwner` option skips `Lchown` entirely; rootless podman instead
runs the extraction inside a userns so `chown` to uid 0 lands on the *mapped* host uid.

`[F]` HIGH — the rootless failure mode has a specific errno: `chown` to a uid outside the userns
map returns **`EINVAL`**. containerd even special-cases the message:
`errors.Is(err, syscall.EINVAL) && userns.RunningInUserNS()` ⇒ "Hint: try increasing the number of
subordinate IDs in `/etc/subuid` and `/etc/subgid`". `[R]` MED — copy that hint verbatim; it is
the single most common rootless-storage support question.

`[R]` HIGH for us (root-first): `lchown` faithfully, and **`lchown` before `lchmod`** (§6.6).
`[R]` MED for later: the two rootless strategies are (a) extract inside a userns (chown lands on
mapped ids, layers are then only usable by that map — **so the store must be keyed by
`chainID + idmap`, not chainID alone**), or (b) extract once as-is and use **idmapped mounts**
(`mount_setattr` with `MOUNT_ATTR_IDMAP`, Linux 5.12+) to shift at mount time, which keeps a
single shared store. (b) is strictly better and is a D5/D1 joint item — flag in handoff.

### 6.9 Decompression amplification ("zip bomb")

`[O]` LOCAL MED — alpine's real ratio is 8,697,856 / 3,846,391 = **2.26x**. gzip's theoretical
maximum is ~1032:1; a hand-built 1 MB gzip can expand to ~1 GB, and a nested/streamed one much
worse. zstd's max ratio is far higher still.

`[R]` HIGH — three independent limits, all cheap:
1. **Cap the compressed read at `descriptor.size`.** The size is in the manifest, and the manifest
   is digest-verified before we fetch layers. Reading one byte more is a protocol violation.
2. **Cap the uncompressed output** at a configurable absolute maximum (default `[R]` MED: 10 GiB)
   *and* at a ratio ceiling (default `[R]` MED: 200x of `descriptor.size`). Abort with a specific
   error, not an OOM.
3. **Cap the entry count** (default `[R]` LOW: 5,000,000) — a tar of ten million zero-byte entries
   costs no decompressed bytes but exhausts inodes and dirents.

`[R]` HIGH — implement 1–3 as a `LimitedReader`-style wrapper composed into the pipeline in §4.2,
so the limits are enforced regardless of which decompressor is selected. Tradeoff: a legitimate
huge image (ML images do exceed 10 GiB uncompressed) hits the cap; make it configurable and log
the actual numbers in the error.

`[R]` MED — the `descriptor.size` cap alone is not sufficient protection because `size` is
attacker-chosen: a malicious registry can claim 5 GiB. But the manifest digest is pinned by the
index which is pinned by the tag the *user* asked for, so an attacker who can change `size` can
change everything. The absolute cap is the real backstop.

### 6.10 Long paths, and path-component limits

`[F]` HIGH — Linux `PATH_MAX` is 4096 including NUL, `NAME_MAX` is 255 bytes per component
(`limits.h`, and `open(2)` returns `ENAMETOOLONG`). ustar's fixed header allows 100 bytes for
`name` + 155 for `prefix`; **PAX and GNU longname extensions lift that to arbitrary length**, so a
tar can absolutely contain a 100 KB path.

`[R]` HIGH — enforce `NAME_MAX` per component and a total-path cap *before* syscalls, and return a
clean error. Reason: without it, the failure surfaces as `ENAMETOOLONG` from deep inside the
extractor, half a layer written. Also `[R]` MED — cap directory nesting depth (default 1024);
`RemoveAll`-style recursion on a 100,000-deep tree will blow the stack.

`[R]` MED — since we resolve with `openat2` from a dirfd, we are *not* subject to `PATH_MAX` for
the total path (each `openat2` sees only a relative fragment). That is a bonus, not a licence:
still cap, because the *container* will hit `PATH_MAX` at runtime.

### 6.11 Unicode / normalization / case

`[C]` MED — on Linux, filenames are opaque byte strings, not text. `/etc/passwd` and a name using
a homoglyph are different files, and no normalization occurs. So this is **not** a security hazard
on Linux the way it is on macOS (HFS+ NFD normalization) or Windows (case-insensitivity, `CON`,
trailing dots).

`[R]` HIGH — **treat tar entry names as `&[u8]`/`OsStr`, never `String`.** Do not
`from_utf8_lossy` them: lossy conversion maps invalid sequences to `U+FFFD`, which makes two
distinct files collide and could let entry 2 overwrite entry 1's target under a different real
name. Rust's `std::os::unix::ffi::OsStrExt::from_bytes` is the right tool.
`[R]` MED — reject entry names containing a NUL or `/` inside a component; log names with
`escape_ascii` when reporting errors so a terminal-escape-sequence filename cannot rewrite our
log output.

### 6.12 Tar variants: PAX, GNU, sparse

`[F]` HIGH — `image-spec/layer.md` links both the GNU Basic Tar Format and libarchive's `tar(5)`
POSIX ustar description as normative references for the on-the-wire format, and says "Sparse files
SHOULD NOT be used because they lack consistent support across tar implementations."

`[O]` LOCAL HIGH — the real alpine layer is plain `ustar\000` (POSIX.1-1988), §4.1.

What a reader must nevertheless handle `[C]` HIGH:

| Variant | Marker | Handling |
|---|---|---|
| POSIX ustar | magic `ustar\0`, version `00` | baseline |
| PAX (POSIX.1-2001) | typeflag `x` (per-entry) / `g` (global) | **required** — this is how xattrs, long names, sub-second mtimes and >8 GiB sizes arrive |
| GNU longname / longlink | typeflag `L` / `K` | **required** — older builders still emit these |
| GNU sparse 0.0/0.1/1.0 | typeflag `S`, or PAX `GNU.sparse.*` | `[R]` MED — **reject with a clear error.** The spec says SHOULD NOT; the parsers are historically buggy; a sparse-map parser is a nice place to hide a heap overflow. |
| `TypeXGlobalHeader` (`g`) | | `[C]` containerd logs and **ignores** it. `[R]` HIGH — do the same; never let a global header mutate defaults for later entries (that mechanism is exactly what "TARmageddon"-class bugs exploit). |
| `TypeRegA` (`\0`, pre-POSIX regular file) | | accept as regular file; containerd still does |

`[R]` HIGH — **enumerate the typeflags we accept and hard-error on anything else.** containerd's
`default: return fmt.Errorf("unhandled tar header type %d", hdr.Typeflag)` is the right posture.
A `match` with an exhaustive `_ => Err(...)` arm is free in Rust.

`[R]` MED — the Image Spec says a layer "MUST NOT include duplicate entries for file paths". Track
seen paths per layer and error on a duplicate. Reason: duplicate entries are how you build an
extractor-differential attack (our extractor keeps the last, a scanner keeps the first).
Tradeoff: a `HashSet<Vec<u8>>` of every path costs memory proportional to entry count; cap it with
the entry-count limit from §6.9.

### 6.13 Foreign / non-distributable layers and the `urls` field

`[F]` HIGH — a descriptor MAY carry `urls`, and non-distributable layers "MAY include `urls` for
downloading these layers directly". `image-spec/media-types.md` marks the whole
non-distributable family **deprecated**.

`[C]` HIGH — **CVE-2020-15157 ("ContainerDrip")**: containerd would follow a manifest's `urls` to
an arbitrary host **and send the registry credentials** with the request. A malicious image on any
registry could therefore steal the puller's credentials for that registry.

`[R]` HIGH — **ignore `urls` entirely and fetch every blob from the repository we resolved.** If a
blob genuinely is not in the repository, fail. Reason: `urls` exists to serve Windows base-layer
licensing, which we do not care about; supporting it means implementing an allowlist and a
credential-scoping rule for a feature that is deprecated. Tradeoff: Windows base images will not
pull. We are Linux-only.

### 6.14 Consolidated extraction checklist

`[R]` HIGH — the extractor, in order, per entry:

```
 1. name = bytes; strip one leading "./"; strip leading "/"
 2. reject: empty, ".", any ".." component, NUL byte, component > NAME_MAX,
            depth > MAX_DEPTH, duplicate-in-this-layer
 3. reject typeflag not in {reg, dir, symlink, hardlink, char, block, fifo, regA}
    (x/g/L/K are consumed by the tar reader, never reach here; S => hard error)
 4. if basename == ".wh..wh..opq" -> record opaque for parent; DO NOT create a file
    if basename starts with ".wh." -> require typeflag==reg && size==0;
       basename[4..] must be non-empty; emit whiteout for it; DO NOT create a file
 5. resolve parent dir via openat2(root_fd, parent, RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS|O_DIRECTORY)
    creating missing components with mkdirat(0755) as we go
 6. if target name already exists and (entry is dir && existing is dir) -> chmod/chown only
    else -> unlinkat (AT_REMOVEDIR if dir) then create        [spec MUST, §5.1]
 7. create by type: openat+write / mkdirat / symlinkat / linkat(resolved) / mknodat
 8. fchownat(AT_SYMLINK_NOFOLLOW)     <-- BEFORE chmod
 9. xattrs, allowlisted (§6.7)
10. fchmodat  (symlinks: skip, Linux has no lchmod)
11. utimensat(AT_SYMLINK_NOFOLLOW)
```

and, after the stream ends: apply directory mtimes in a second pass (containerd does this:
directory mtimes are clobbered by writing children), then apply the opaque markers recorded in
step 4 for directories that received them out of order (§5.1).

`[R]` HIGH — write a **negative test corpus** as part of the extractor: a handful of hand-built
malicious tars (`../escape`, `/abs`, `link->/tmp` + `link/x`, hardlink to `/etc/shadow`,
`trusted.overlay.opaque` xattr, 1 GB-from-1 MB gzip bomb, 300-char component, duplicate path,
`.wh.` on a directory entry, sparse entry). Each must produce a *specific* error. This is the one
place where a test suite is not optional laziness-debt: the CVEs above are all "someone did not
have this test".

---
## 7. Content-addressable storage: the OCI layout, and our store design

### 7.1 The OCI image-layout spec — what it is and is not

`[F]` HIGH — `image-spec/image-layout.md`. A directory containing exactly:

```
./oci-layout                    # {"imageLayoutVersion":"1.0.0"}  -- MUST exist
./index.json                    # an image index                  -- MUST exist
./blobs/<alg>/<encoded>         # content-addressed blobs         -- dir MUST exist, MAY be empty
```

Normative points worth having memorised:
- "The content of `blobs/<alg>/<encoded>` **MUST** match the digest `<alg>:<encoded>`."
- "`index.json` … **MUST** be an image index JSON object."
- "The blobs directory **MAY** contain blobs which are not referenced by any of the refs."
- "The blobs directory **MAY** be missing referenced blobs, in which case the missing blobs SHOULD
  be fulfilled by an external blob store."
- "Implementations should not error when encountering unknown files" — hence `docker save`'s
  `manifest.json` sitting alongside is legal.
- `index.json`'s entries carry `annotations`, and by convention
  `org.opencontainers.image.ref.name` holds the tag. `[C]` HIGH — that is how `skopeo copy
  docker://alpine oci:./out:latest` records the name.

`[F]` HIGH — the layout is defined as an *interchange* format: "This layout MAY be used in a
variety of different transport mechanisms: archive formats (e.g. tar, zip), shared filesystem
environments (e.g. nfs), or networked file fetching."

`[R]` HIGH — **support OCI image-layout as an import/export format, but do not use it as the
engine's live store.** Reasons: (a) it has no place for *extracted* layers, snapshots, chain IDs,
mount state, or reference counts — all the things a running engine needs; (b) `index.json` is a
single JSON file, so every tag mutation rewrites it, which is a concurrency and crash-safety
problem; (c) neither Docker nor containerd uses it live. Tradeoff: two formats to maintain. The
import/export path is ~200 lines and buys `skopeo copy` interop for free, which is enormously
useful for offline testing without hitting Docker Hub.

### 7.2 What containerd actually lays out, and why

`[O]` HOST-A HIGH — `/var/lib/containerd/`:

```
io.containerd.content.v1.content/
    blobs/sha256/<hex>           # 12 blobs here; permissions -r--r--r--
    ingest/                      # in-progress writes (empty on this idle host)
io.containerd.metadata.v1.bolt/meta.db          # 512 KiB boltdb
io.containerd.snapshotter.v1.overlayfs/snapshots/
io.containerd.snapshotter.v1.native/ , .btrfs/ , .blockfile/
io.containerd.runtime.v2.task/
```

`[O]` HOST-A HIGH — a blob is its own name:
`sha256sum blobs/sha256/6e9ade9a77ab… -> 6e9ade9a77ab…`. Files are mode **`0444`**.

`[C]` HIGH — the design containerd chose, and the reasoning:

1. **The content store is the OCI blob layout, verbatim** (`blobs/sha256/<hex>`). Free
   interoperability with `oci:` layouts, and one file per digest is the simplest possible dedup.
2. **`ingest/` is a separate directory for partial writes.** A write in progress gets a `ref`
   (an arbitrary string like `manifest-sha256:…`), a `data` file, a `ref` file and a `startedat`
   marker. On commit, the data file is `fsync`'d, hashed, and **`rename(2)`d** into
   `blobs/sha256/<hex>`. `rename` within a filesystem is atomic, so a blob in `blobs/` is by
   construction complete and verified. `[F]` HIGH — `rename(2)`: "If `newpath` already exists it
   will be atomically replaced… there will be no point at which another process attempting to
   access `newpath` will find it missing."
3. **Metadata lives in boltdb, not in the filesystem layout.** Leases, references, snapshot
   parentage, image name→digest — all transactional.
4. **The snapshotter is a separate plugin with its own directory**, keyed independently. The
   content store knows nothing about overlay; the snapshotter knows nothing about registries.

`[R]` HIGH — **copy points 1, 2 and 4 exactly.** They are the right answers and they are cheap.
Point 3 is the one to think about; see §7.5.

### 7.3 What Docker lays out, and the two ideas worth stealing

`[O]` HOST-A HIGH — `/var/lib/docker/image/overlay2/`:

```
repositories.json                       # name:tag -> IMAGE ID (= config blob digest)
imagedb/content/sha256/<id>             # the image config blob, verbatim
imagedb/metadata/sha256/<id>            # local-only extras (parent, lastUpdated)
layerdb/sha256/<chainID>/               # one dir per chain ID
    cache-id                            # -> the random overlay2 dir name
    diff                                # sha256:<diff_id>
    parent                              # sha256:<parent chainID>
    size                                # bytes
    tar-split.json.gz                   # to rebuild the byte-exact original tar
layerdb/mounts/<container-id>/
    mount-id, init-id, parent
distribution/diffid-by-digest/sha256/<blobdigest>   -> sha256:<diff_id>
distribution/v2metadata-by-diffid/sha256/<diff_id>  -> [{"Digest":…,"SourceRepository":…,"HMAC":…}]
```

`[O]` HOST-A HIGH — `repositories.json` maps **to the config digest**, not the manifest digest:

```json
{"Repositories":{"alpine":{
  "alpine:latest":"sha256:1a8c3406406e97ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b",
  "alpine@sha256:6c1a91f698e41bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b":
      "sha256:1a8c3406406e97ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b"}, …}}
```

Note both the tag **and** the `name@<manifest digest>` form resolve to the same *config* digest.
`[C]` HIGH — **Docker's "image ID" is the image config blob's digest.** Cross-verified:
`docker image inspect alpine --format '{{.Id}}'` → `sha256:d529dd0c…`, which is byte-identical to
the config blob we pulled anonymously in §2.9. And `{{json .RootFS}}` →
`{"Type":"layers","Layers":["sha256:21ee5abca34e…"]}` — the same `diff_id` we computed from the
gzip stream in §4.2. The registry, the local Docker store and our own hashing all agree.

Two ideas worth stealing:

- `[R]` HIGH — **`distribution/diffid-by-digest`**: a persistent map from *compressed blob digest*
  → `diff_id`. This is what lets a second pull of the same image **skip downloading a layer
  entirely** after seeing only the manifest: you know the blob digest from the manifest, you look
  up the `diff_id` you cached, you find you already have that layer. Without it you must download
  and decompress before you can tell. This is the single highest-value cache in the whole puller
  and it is one small on-disk map.
- `[C]` MED — `v2metadata-by-diffid` records which repository a layer came from, enabling
  **cross-repository blob mounting** on push (`POST /v2/<name>/blobs/uploads/?mount=<digest>&from=<repo>`).
  Only relevant when we push.

`[R]` MED — do **not** copy `repositories.json` as a single rewritten JSON file. It is a
whole-file rewrite per tag change; Docker gets away with it because the daemon serialises all
access. One file per tag is simpler and crash-safer.

### 7.4 Our store design

`[R]` HIGH — proposed layout, root configurable, default `/var/lib/<engine>`:

```
<root>/
  content/                              # immutable, digest-addressed, mode 0444
    blobs/sha256/<hex>                  #   OCI-layout compatible on purpose
    ingest/<uuid>/{data,ref,startedat}  #   partial writes; never referenced
  layers/                               # extracted layer diffs, keyed by diff_id
    sha256/<hex>/
       diff/                            #   the extracted changeset (the overlay lowerdir)
       info.json                        #   {diff_id, blob_digest, size, entries, xattr_ns}
  snapshots/                            # keyed by chain ID
    sha256/<hex>/
       info.json                        #   {parent_chain, diff_id, kind: committed}
  containers/<id>/
       upper/  work/  merged/           #   the writable layer + overlay scratch
       info.json
  refs/                                 # tag -> manifest digest
    <registry>/<repo>/tags/<tag>        #   file containing "sha256:<manifest digest>"
  manifests/sha256/<hex>                # raw manifest bytes (also in content/, but named)
  cache/
    diffid-by-blob/sha256/<hex>         # blob digest -> diff_id   (the §7.3 trick)
  tmp/                                  # staging for atomic renames; same filesystem as targets
```

Design decisions and their reasons:

- `[R]` HIGH — **`layers/` keyed by `diff_id`, `snapshots/` keyed by chain ID.** §4.5. Two images
  sharing a base share the `layers/` entry *and* the `snapshots/` entry up to the divergence
  point. Keying only by chain ID would re-extract identical layers under different parents;
  keying only by `diff_id` would force a linear parent-chain comparison at mount time.
- `[R]` HIGH — **`content/`, `layers/`, `snapshots/`, `containers/` and `tmp/` must all be on the
  same filesystem**, because every commit is a `rename(2)` and `rename` across filesystems fails
  with `EXDEV`. Also `[F]` HIGH per `overlayfs.rst`: "The `workdir` needs to be a directory on the
  same filesystem as `upperdir`."  Enforce at startup with one `statfs` comparison and a clear
  error; do not discover it at first pull.
- `[R]` MED — do not store the compressed blob at all for layers we successfully extract. It is
  dead weight (3.8 MB per alpine, gigabytes across an image set) unless we plan to push. Keep
  manifests and configs (tiny, needed for `inspect`). Tradeoff: a re-push regenerates blobs from
  the extracted tree and gets different digests (§5.5). Record the decision; revisit when push
  lands.

### 7.5 Metadata: files vs an embedded database

`[C]` HIGH — containerd uses boltdb; Docker uses a directory of tiny files; podman/`containers/storage`
uses JSON files plus a lock file.

`[R]` HIGH for MVP — **plain files + a directory lock, no database.** Reasons: (a) every value we
store is small and keyed by a digest, which is exactly what a filesystem is; (b) a `.db` file is
an opaque blob that cannot be inspected or repaired with `cat`, which matters enormously while
learning; (c) it removes a dependency (`redb`/`sled`/`rusqlite`) and its failure modes. Tradeoff:
no transactions across multiple keys — so a crash can leave a snapshot record pointing at a layer
record that was not written. Mitigate by **ordering writes so that dangling forward references are
impossible**: write the layer record, `fsync`, *then* the snapshot record, *then* the tag. A
dangling *backward* reference (a layer nobody points at) is just garbage, and GC handles it.

`[R]` MED — revisit if we need multi-writer concurrency. The moment two engine processes may run
at once, a real transactional store earns its keep. Until then a single `flock`ed daemon is
enough. Mark it: `// ponytail: flat files + flock; move to redb if we ever want multi-process`.

### 7.6 Surviving interrupted pulls

The three failure points, and the mitigation for each `[R]` HIGH:

| Failure | Symptom without care | Mitigation |
|---|---|---|
| Killed mid-blob-download | A truncated file in `blobs/` that hashes wrong forever, or worse, is *trusted* because the name is right | **Never write into `blobs/` directly.** Write to `content/ingest/<uuid>/data`, `fsync`, verify the digest, then `rename` into `blobs/sha256/<hex>`. A file in `blobs/` is complete by construction. |
| Killed mid-extraction | A half-extracted `layers/sha256/<hex>/diff/` that looks present | Extract into `tmp/<uuid>/diff`, verify **both** digests (§4.2), `fsync` the directory, then `rename tmp/<uuid> layers/sha256/<hex>`. `[F]` HIGH — directory rename is atomic within a filesystem. |
| Killed between layers | A snapshot whose parent chain is incomplete | Commit snapshots strictly in chain order, and make the *last* write the tag. An orphan snapshot is invisible because nothing references it. |

`[R]` HIGH — **`fsync` discipline**: after writing a blob's data, `fsync` the *file*; after the
`rename`, `fsync` the *parent directory*. `[F]` HIGH — POSIX/`rename(2)`: the rename is atomic but
its durability is not guaranteed until the containing directory is synced. Skipping the directory
fsync is the classic "the file is there but the name is not, after a power cut" bug.

`[R]` MED — **garbage-collect `ingest/` and `tmp/` on startup**: anything older than N hours with
no live writer is abandoned. That is the entire crash-recovery story. Do not attempt to resume a
partial download across restarts; the complexity is not worth it (§2.8).

### 7.7 Reference counting and GC

`[R]` HIGH — **do not implement refcounts. Implement mark-and-sweep.** Reasoning: a refcount is a
number that must be incremented and decremented in perfect correspondence with events, across
crashes, forever. Every container engine that used refcounts has had leak bugs. Mark-and-sweep is
stateless: the roots are enumerable at any moment, so the answer is always recomputable.

The algorithm:

```
roots = { manifest digests named by any tag in refs/ }
      ∪ { chain IDs of any live container in containers/ }
mark:  for each root manifest -> its config blob, its layer diff_ids, the chain IDs 0..n
sweep: anything in content/blobs, layers/, snapshots/ not marked, and older than a grace period,
       is deleted
```

`[R]` HIGH — the **grace period** (say 1 hour) is what makes this safe against the race "a pull
just wrote a layer but has not yet written the tag". `[C]` HIGH — containerd solves the same race
with **leases**: a lease is a time-limited root that a client holds during an operation. `[R]` MED
— leases are the more correct design and are not much more code (a directory of
`leases/<id>` files each listing digests, with an expiry); adopt them if the grace period ever
causes a real problem. Start with the grace period.

`[R]` HIGH — GC must **refuse to delete anything currently mounted**. Check `/proc/self/mountinfo`
for any mount whose `lowerdir`/`upperdir` is under a candidate path, before unlinking. `[F]` HIGH
— on Linux, unlinking a directory that is a live overlay `lowerdir` does not fail; overlay holds
the dentry, and the result is an image that looks fine until the container restarts.

### 7.8 Dedup across images — what it actually buys

`[O]` HOST-A HIGH — this host has **107 directories in `/var/lib/docker/overlay2/`** and
**106 symlinks in `overlay2/l/`**, serving (from `repositories.json`) at least 6 distinct
repositories and many tags. Layer sharing is not a theoretical benefit; it is why a machine with a
dozen images built on the same base does not have a dozen copies of that base.

`[R]` HIGH — dedup falls out of §7.4 for free: the store is keyed by content, so the second image
that needs `diff_id X` finds it present and skips download *and* extraction. There is nothing
extra to build. The one thing that would break it is keying layers by
`(image, index)` instead of by `diff_id` — do not do that even "temporarily", because migrating
the on-disk layout later is exactly the expensive change flagged in §11.5.

---

## 8. OverlayFS mechanics — from the kernel documentation

Source throughout: `Documentation/filesystems/overlayfs.rst`, Linux `master`.
<https://www.kernel.org/doc/html/latest/filesystems/overlayfs.html>

### 8.1 The three directories

`[F]` HIGH:

```
mount -t overlay overlay -o lowerdir=/lower,upperdir=/upper,workdir=/work /merged
```

- **`lowerdir`** — one or more read-only layers, colon-separated. May be shared freely between
  mounts: "Lower layers may be shared among several overlay mounts and that is indeed a very
  common practice."
- **`upperdir`** — the single writable layer. All modifications land here via copy-up.
- **`workdir`** — "The `workdir` needs to be a directory on the **same filesystem as upperdir**"
  and must be empty. It is scratch space for atomic operations: "In order to support `rm` and
  `rmdir` without changing the lower filesystem," overlay needs a place to construct results
  before making them visible.
- **`upperdir` and `workdir` may not be shared**: "Using an upper layer path and/or a workdir path
  that are already used by another overlay mount is not allowed and may fail with `EBUSY`. Using
  partially overlapping paths is not allowed and may fail with `EBUSY`."

`[F]` HIGH — omitting `upperdir=` and `workdir=` gives a **read-only** overlay: "As the example
shows, `upperdir=` and `workdir=` may be omitted. In that case the overlay will be read-only."

### 8.2 Copy-up

`[F]` MED — writing to a file that exists only in a lower layer triggers **copy-up**: the whole
file is copied into `upperdir` first, then modified. Consequences to design around:
- Appending one byte to a 2 GB file in a lower layer costs 2 GB of writes and 2 GB of upper space.
- `chmod`/`chown` on a lower file also triggers a full copy-up **unless `metacopy` is on** (§8.7).
- This is why "put your database on a volume, not the container filesystem" is universal advice.

### 8.3 lowerdir ordering — the classic error, settled empirically

`[F]` HIGH — the kernel doc, verbatim:

> "The specified lower directories will be stacked beginning from the **rightmost** one and going
> left. In the above example **lower1 will be the top**, lower2 the middle and lower3 the bottom
> layer."

So `lowerdir=A:B:C` means **A is topmost (highest priority), C is bottom**. Leftmost wins.

`[O]` HOST-A 6.11.0-29 HIGH — verified directly, in a private mount namespace under `/tmp`:

```
lowA/f.txt = "A-version"   lowB/f.txt = "B-version"
mount -t overlay -o lowerdir=lowA:lowB,...   ->  cat merged/f.txt  =>  A-version
mount -t overlay -o lowerdir=lowB:lowA,...   ->  cat merged/f.txt  =>  B-version
```

**The critical mapping, stated so it cannot be got wrong:**

```
image config:   rootfs.diff_ids = [ L0(base), L1, L2, ..., Ln(top) ]
overlay mount:  lowerdir = Ln : Ln-1 : ... : L1 : L0
                            ^^ topmost first             ^^ base last
```

**`lowerdir` is the REVERSE of `diff_ids`.**

`[O]` HOST-A 6.11.0-29, Docker 28.3.1 HIGH — **verified end-to-end against a live production
container** (a 9-layer `ghcr.io/example-org/backend:v2.25.1`). I took the image config's
`diff_ids`, computed all nine chain IDs with the §4.5 formula, read each
`layerdb/sha256/<chainID>/cache-id` to get the overlay2 directory name, then resolved the running
container's `lowerdir` entries through the `l/` symlinks and compared:

```
 config diff_ids (base-first)      chainID        cache-id        live mount lowerdir (top-first)
 diff[0]=4f35986d820c          4f35986d820c    42534ff23a4c    a5f79ae06f34   <- the "-init" layer
 diff[1]=7d8db1f43e1e          5828c9b15438    b2f31d872d08    1ab184527cb3   == diff[8]  (TOP)
 diff[2]=f79dfa5c52ce          7e3a0e5ee76d    c56b584a7442    547cda7a9ff9   == diff[7]
 diff[3]=da5f201eea36          809b468f82a9    b45d72d839b4    5bab11c42954   == diff[6]
 diff[4]=33c259529c78          ac995c361ecd    b99b39b26a44    c88211795b80   == diff[5]
 diff[5]=e35940c83e1d          1007ac3ce997    c88211795b80    b99b39b26a44   == diff[4]
 diff[6]=698e321728dd          72937144df87    5bab11c42954    b45d72d839b4   == diff[3]
 diff[7]=53393a7fef7e          72293fb3b69d    547cda7a9ff9    c56b584a7442   == diff[2]
 diff[8]=803a8e1dc20f          daae7f3b80ff    1ab184527cb3    b2f31d872d08   == diff[1]
                                                               42534ff23a4c   == diff[0]  (BASE)
```

Three things proved at once: (a) **`lowerdir` is exactly `reverse(diff_ids)`**; (b) the §4.5
chain-ID formula reproduces all nine of Docker's `layerdb` directory names from the registry's
`diff_ids` alone; (c) Docker inserts its per-container **`-init` layer above the image's topmost
layer** (§9.3).

`[R]` HIGH — write the reversal in exactly one function, name it unmistakably
(`fn lowerdir_arg(diff_ids: &[DiffId]) -> String` with a doc comment quoting the kernel doc), and
put an assertion test in the negative corpus: build two layers where the top one overwrites a file
from the bottom, mount, and assert the *top* version is visible. Reason: this bug mounts
successfully and produces a subtly wrong filesystem — the worst failure mode there is.

`[F]` HIGH — **colons in directory names must be escaped with a backslash**
(`lowerdir=/a\:lower\:\:dir`). `[R]` HIGH — since our layer directory names are hex digests we
control, they can never contain a colon. Assert that invariant rather than implementing escaping.

### 8.4 The real mount options Docker uses in production

`[O]` HOST-A 6.11.0-29, Docker 28.3.1 HIGH — from `/proc/mounts` on the live host (one of five
running containers, wrapped for readability):

```
overlay /var/lib/docker/overlay2/f322f67c…/merged overlay
  rw,relatime,
  lowerdir=/var/lib/docker/overlay2/l/RD25BPX7BGEHNXMQMCIIUNSLU2:
           /var/lib/docker/overlay2/l/WMKL3YTZWQDU6Q7XT7ZGWYE3VD:
           … (12 entries total) …
           /var/lib/docker/overlay2/l/7DKG67SG5LQXOOPH6AW46BU4KF,
  upperdir=/var/lib/docker/overlay2/f322f67c…/diff,
  workdir=/var/lib/docker/overlay2/f322f67c…/work,
  nouserxattr 0 0
```

Observations:
- **Absolute** paths through the `l/` symlink farm, not the real `<64-hex>/diff` paths (§9.2).
- Layer counts observed on this host: 8 to 12 lowerdirs per container.
- `nouserxattr` — this daemon runs as real root and uses the `trusted.overlay.*` namespace.
- **No `index=`, no `metacopy=`, no `redirect_dir=`, no `xino=` in the option string.**
- `[O]` HOST-A HIGH — `docker info`: `Backing Filesystem: extfs`, `Supports d_type: true`,
  `Using metacopy: false`, `Native Overlay Diff: true`, `userxattr: false`.
- `[O]` HOST-A HIGH — `/sys/module/overlay/parameters/`:
  `index=N`, `metacopy=N`, `redirect_dir=N`, `xino_auto=Y`, `redirect_always_follow=Y`,
  `redirect_max=256`, `nfs_export=N`, `check_copy_up=N`.

`[R]` HIGH — **mirror this option set for MVP: `lowerdir`/`upperdir`/`workdir` and nothing else.**
Every optional feature below is off in the world's most-deployed configuration. Adding one is a
decision to justify, not a default.

### 8.5 The mount-option length limit — measured, and how to escape it

This is the constraint that shapes every overlay-based storage driver's on-disk layout.

`[F]` HIGH — `mount(2)`'s `data` argument is copied by the kernel with `copy_mount_options()`,
which copies **at most one page**. On x86_64 that is 4096 bytes for the *entire* option string.

`[O]` HOST-A 6.11.0-29 HIGH — measured by direct `mount(2)` via `ctypes`, 400 prepared lower
directories, varying only the option-string length:

| N lowerdirs | option string bytes | result |
|---|---|---|
| 400 (short relative names, after `chdir`) | 1,669 | **OK**, 400 layers visible |
| 150 (long absolute paths) | 3,969 | **OK**, 150 layers visible |
| 160 (long absolute paths) | 4,229 | **FAIL — `ELOOP` (errno 40, "Too many levels of symbolic links")** |
| 200 | 5,269 | FAIL, `ELOOP` |
| 300 | 7,869 | FAIL, `ELOOP` |

Identical results whether the string is `lowerdir=…,upperdir=…,workdir=…` or
`upperdir=…,workdir=…,lowerdir=…`, and identical through `util-linux 2.40.2` `mount(8)`.

Three things to take from this:

1. **The limit is the option string length, not the layer count.** 400 layers mount fine at 1,669
   bytes; 160 layers fail at 4,229 bytes. Any design that thinks in "max layers" is thinking about
   the wrong variable.
2. **`ELOOP` is a spectacularly misleading errno** for "your option string is too long". Anyone
   debugging this from the errno alone will spend an afternoon looking at symlinks. `[R]` HIGH —
   we check the length ourselves *before* calling mount and emit our own error.
3. `[O]` LOW — in one earlier run, `mount(8)` with an 8.6 KB option string appeared to succeed with
   only ~141 of 300 layers visible (silent truncation). A careful re-run at the same lengths failed
   cleanly instead, so I could not reproduce it. Flagged in §15 rather than claimed.

**The escape hatch — and it is a good one.** `[F]` HIGH — since **Linux 6.8**, the new mount API
accepts lower layers one at a time:

```c
fsconfig(fs_fd, FSCONFIG_SET_STRING, "lowerdir+", "/l1", 0);
fsconfig(fs_fd, FSCONFIG_SET_STRING, "lowerdir+", "/l2", 0);
```

and `[F]` HIGH also as file descriptors, `FSCONFIG_SET_FD` with `"lowerdir+"`. The kernel doc adds
that this form also removes the colon-escaping problem.

`[O]` HOST-A 6.11.0-29 HIGH — **verified: `fsopen`/`fsconfig`/`fsmount`/`move_mount` mounts 500
lower layers totalling 12,000 bytes of paths, far past the 4,096-byte `mount(2)` ceiling.**
Raw syscalls via `ctypes`, no `util-linux`:

```
fsopen("overlay", FSOPEN_CLOEXEC)
  -> 500 x fsconfig(fd, FSCONFIG_SET_STRING, "lowerdir+", "<abs path>", 0)
  -> fsconfig(fd, FSCONFIG_SET_STRING, "upperdir"/"workdir", …)
  -> fsconfig(fd, FSCONFIG_CMD_CREATE) -> fsmount() -> move_mount()
N=150 total_path_bytes= 3600 -> OK, visible=150
N=300 total_path_bytes= 7200 -> OK, visible=300
N=500 total_path_bytes=12000 -> OK, visible=500
```

`[R]` **HIGH — use the new mount API (`fsopen`/`fsconfig`/`fsmount`/`move_mount`) for the overlay
mount, and skip Docker's short-symlink indirection entirely.** This is one of the most valuable
findings in this file: it deletes a whole subsystem (§9.2) that exists only to work around a
limitation we do not have to inherit. Tradeoff: requires Linux ≥ 6.8 for `lowerdir+`, which our
stated dev target satisfies. `[R]` MED — keep a `mount(2)` fallback that checks the string length
and errors clearly if it would exceed 4,096; that fallback is ~20 lines and covers older kernels
without any `l/` farm (it just caps usable layer depth).

Note this also lands squarely in D1's territory (`move_mount` into a mount namespace). Handoff §14.

### 8.6 `trusted.overlay.*` xattrs and the whiteout representations

`[F]` HIGH — from the kernel doc:

| xattr / node | Meaning |
|---|---|
| char device **0/0** in upper | whiteout: the name is deleted; the node itself is hidden from the merged view |
| zero-size regular file + `trusted.overlay.whiteout` | alternative whiteout encoding (userspace-generated layers) |
| `trusted.overlay.opaque = "y"` on an upper dir | opaque: the lower counterpart is ignored entirely |
| `trusted.overlay.opaque = "x"` on a dir | marks a dir holding *alternative* whiteouts, for nesting |
| `trusted.overlay.redirect` | this dir's lower counterpart lives at a different path (`redirect_dir`) |
| `trusted.overlay.metacopy` | only metadata was copied up; data is still in the lower layer |
| `trusted.overlay.origin` | file handle + UUID of the lower origin (`index` feature) |
| `trusted.overlay.nlink` | link count bookkeeping for indexed hardlinks |
| `overlay.overlay.` prefix | escape for nesting overlays: one prefix is stripped per overlay level |

`[O]` HOST-A 6.11.0-29 HIGH — the whiteout representation, produced by the kernel itself in a
private mount namespace:

```
# lower/keep.txt exists; then, in the merged mount:  rm merged/keep.txt
$ ls -la upper
c--------- 2 root root 0, 0 Sep  5 13:00 keep.txt
$ stat -c '%n type=%F mode=%a rdev=%t:%T' upper/keep.txt
upper/keep.txt type=character special file mode=0 rdev=0:0
```

Mode **0000**, char device, **rdev 0:0**. That is precisely what our extractor must emit for a
`.wh.<name>` tar entry (§5.3).

`[F]` HIGH — with `-o userxattr` those become `user.overlay.whiteout` / `user.overlay.opaque` /
etc.: "The `-o userxattr` mount option forces overlayfs to use the `user.overlay.` xattr namespace
instead of `trusted.overlay.`. This is useful for unprivileged mounting of overlayfs."

`[R]` HIGH — a layer extracted with `trusted.*` markers **cannot** be used under a `userxattr`
mount and vice versa. Record the namespace in `layers/…/info.json` (§7.4) and refuse to mix.
Getting this wrong produces an overlay where deletions silently reappear.

### 8.7 The optional features, and whether to touch them

| Feature | What it does | `[R]` for us |
|---|---|---|
| **`metacopy=on`** | chmod/chown copies up metadata only, not data | **No.** `[O]` HOST-A: off in production (`Using metacopy: false`, module param `N`). It also makes an upper layer only valid against the exact lower set it was created with — the kernel doc warns "if same upper is mounted with different set of lower, mount probably will succeed but **expect the unexpected** later on. So don't do it." Not worth the class of bug. |
| **`redirect_dir=on`** | a renamed directory records `trusted.overlay.redirect` instead of copying up recursively | **No** for MVP. `[O]` HOST-A: module param `N`. Without it, renaming a large lower directory copies it up entirely — slow but correct. |
| **`index=on`** | enables hardlink correctness across copy-up and `nfs_export` | **No.** `[O]` HOST-A: `N`, and Docker passes `index=off` explicitly in its option string builder. The doc: with `index`, the upper's `trusted.overlay.origin` is compared to the lower root's NFS file handle + UUID at every mount, and mismatch is `ESTALE` — so **layers become non-copyable between machines/directories**. That directly conflicts with a content-addressed store you might rsync. |
| **`xino=on/auto`** | makes `st_ino` unique and persistent across layers on different filesystems by stuffing the layer index into high inode bits | `[R]` MED — leave at the kernel default (`xino_auto=Y` on HOST-A). Only matters when layers span filesystems, which our same-filesystem rule (§7.4) forbids anyway. Worth knowing because without it, `st_ino` collisions between layers break `find -samefile`, `tar`'s hardlink detection, and some package managers. |
| **`userxattr`** | `user.overlay.*` instead of `trusted.overlay.*` | **Later, for rootless.** §8.8. |
| **`volatile`** | skips `sync` on the upper layer; a marker in `workdir/work/incompat/volatile` forces a fresh upper on next mount | `[R]` LOW — genuinely useful for throwaway build steps, meaningless for us now. |
| **`nfs_export`** | file handles survive across mounts | **No.** Requires `index=on`. |
| **data-only lower layers (`::` / `datadir+`)** | lower layers used only for file data, with metadata elsewhere — the composefs/erofs mechanism | Not now; §10.6. |

### 8.8 Valid upper filesystems, and why NFS is not one

`[F]` HIGH — the kernel doc's requirements on the upper filesystem: it must support **`d_type`**
in `readdir` (otherwise overlay cannot tell a whiteout char-dev from a file reliably), it must
support **extended attributes** in the namespace overlay is using, and `workdir` must be on the
same filesystem.

`[O]` HOST-A HIGH — `docker info` prints `Supports d_type: true` as a first-class field precisely
because ext4 formatted with `ftype=0`, and older XFS, do not — and overlay2 refuses to run there.

What works as `upperdir` `[C]` HIGH: **ext4, xfs (with `ftype=1`), btrfs, f2fs**.
What does not: **NFS, tmpfs (no `trusted.*` xattr support — but tmpfs *does* work with
`userxattr` on modern kernels), overlayfs itself (needs the `overlay.overlay.` escaping dance),
FUSE filesystems generally, and anything without `d_type`.**

**NFS specifically** `[F]` MED — the reasons it cannot be an `upperdir`: (a) NFS does not support
the `trusted.*` xattr namespace at all, so whiteouts and opaque markers have nowhere to live;
(b) overlay requires stable, local inode semantics and `d_type` that NFS's caching does not
guarantee; (c) `workdir` operations rely on local atomic rename/link semantics.

`[O]` **NOT VERIFIED** — the task asked me to reproduce the exact NFS `upperdir` error on
`old-host`, whose home is NFS. **That host was unreachable for the entire session** (`ssh: Could
not resolve hostname old-host.example.edu`, NXDOMAIN from an independent `nslookup`). The expected errno
per multiple reports is `EINVAL` at mount time with `overlayfs: filesystem on '<upper>' not
supported as upperdir` in `dmesg`, but I did not observe it and am not claiming it. §15.1.

`[R]` HIGH — check the upper filesystem at store-init time: `statfs` for the magic number, a probe
`setxattr(trusted.overlay.test)` (or `user.overlay.test` under userxattr), and a `readdir` `d_type`
probe. Fail loudly at init rather than obscurely at first container start.

### 8.9 Rootless overlay — what actually enables it

`[F]` HIGH — three separate kernel capabilities had to land, and they are often confused:

1. **Unprivileged overlayfs mount inside a user namespace** — Linux **5.11**
   (`FS_USERNS_MOUNT` on overlayfs). Before that, no amount of option-fiddling helped.
2. **`-o userxattr`** — Linux **5.11**, so the mount can write markers to `user.overlay.*` which an
   unprivileged user is allowed to set.
3. **Whiteouts still need `mknod`**, which a userns-confined process cannot do. Hence the
   alternative whiteout form (zero-size regular file + whiteout xattr) that `overlayfs.rst`
   documents as "never created by overlayfs, but can be used by userspace tools (like containers)
   that generate lower layers."

`[C]` HIGH — **Ubuntu and Rocky/RHEL both backported unprivileged overlay long before 5.11**, by
different means: Ubuntu carries a long-standing out-of-tree patch permitting overlay in a userns;
RHEL 8 (kernel 4.18.x) carries Red Hat's backport of the userns-mountable overlay work. That is
why "overlay mount inside a userns succeeds on a 4.18 RHEL box" is not a contradiction — it is a
vendor patch, not upstream 4.18 behaviour. `[O]` **NOT VERIFIED** — I could not confirm this on
`old-host`; the specific mechanism (which patch, and whether `userxattr` is present at 4.18) is
listed as an open question in §15.1.

`[R]` MED for us: rootless storage is D5's track and later than MVP. What we must do *now* to not
paint ourselves into a corner: (a) the whiteout-writer trait from §5.4; (b) record the xattr
namespace per layer (§8.6); (c) do not bake `trusted.` into any string constant outside that trait.

---

## 9. overlay2 and the snapshotter model

### 9.1 What Docker's overlay2 driver puts on disk

`[O]` HOST-A HIGH — `/var/lib/docker/overlay2/` contains **107 entries** plus `l/` holding
**106 symlinks**. A committed image layer:

```
overlay2/8f9956999fdf…c7c/
    committed              # 0-byte marker: this layer is immutable
    diff/                  # the extracted changeset -- this is the lowerdir contribution
    link                   # "BT5CHWJ6MLUN2TCAECHIFOZRUS"  (26 chars, base32-ish)
    lower                  # "l/F6PW4EUSGFIOLKQNNE632V4XPB:l/MKXBKG5XYL4WW5YJH7XPZIR6BH:l/7CA7…"
    work/                  # overlay scratch
```

A base layer has **no `lower` file** (nothing beneath it) and no `work/` until mounted.
A container layer additionally has `merged/` (the mountpoint) and no `committed`.

`[O]` HOST-A HIGH — `overlay2/l/` entries are relative symlinks into the layer's `diff`:

```
l/22JWWP3A26FDPUBZEBYGLZBT4Y -> ../ba89fa619f5ca3abbdd7d2288cbeb44711dc03f0a891b0b836ffc4d947150c73/diff
```

### 9.2 Why the `l/` short-link indirection exists — the arithmetic

`[C]` HIGH — `moby/daemon/graphdriver/overlay2/overlay.go`, verbatim:

```go
const (
    linkDir  = "l"
    maxDepth = 128
    // idLength represents the number of random characters
    // which can be used to create the unique link identifier
    // for every layer. If this value is too long then the
    // page size limit for the mount command may be exceeded.
    // The idLength should be selected such that following equation
    // is true (512 is a buffer for label metadata).
    // ((idLength + len(linkDir) + 1) * maxDepth) <= (pageSize - 512)
    idLength = 26
)
```

Do the arithmetic: `(26 + 1 + 1) * 128 = 3584`, and `4096 - 512 = 3584`. **Exactly equal.** The
128-layer limit and the 26-character link name are not arbitrary — they are solved for, from the
one-page `mount(2)` limit measured in §8.5.

Compare the alternative. A full layer path is
`/var/lib/docker/overlay2/<64 hex>/diff` = 94 characters, +1 for the colon = 95 bytes per layer.
`4096 / 95 ≈ 43` layers. The indirection buys **128 layers instead of ~43**, at the cost of a
second directory that must be kept in sync with the first.

`[C]` HIGH — and Docker still needs a *second* fallback. From the same file:

```go
if len(mountData) > pageSize-1 {
    // rebuild opts with paths relative to d.home
    ...
    mount = func(...) error { return mountFrom(d.home, source, target, mType, flags, label) }
```

`mountFrom` forks a helper that `chdir`s into `/var/lib/docker/overlay2` and mounts with *relative*
paths — 28 bytes per layer instead of 51. The comment: "Use relative paths and `mountFrom` when
the mount data has exceeded the page size … relative links make the mount data much smaller at the
expense of requiring a fork exec to chroot."

`[R]` **HIGH — do not build any of this.** §8.5 showed `fsconfig`'s `lowerdir+` mounts 500 layers
with 12 KB of paths. We can name layer directories by their chain ID or `diff_id` in full, use
absolute paths, keep no symlink farm, keep no `link`/`lower` files, and impose no 128-layer cap.
Tradeoff: Linux ≥ 6.8 for the good path. That deletes: the `l/` directory, the `link` file, the
`lower` file, the random-ID generator, the fork-exec `mountFrom` helper, and the class of bugs
where `l/` and the layer dirs disagree. This is the largest single simplification available to us
versus Docker's design.

`[C]` HIGH — one more overlay2 idea worth noting. For a **read-only** (committed) layer Docker
mounts with *no upperdir at all*, prepending the layer's own `diff` to the lowerdir list:
`lowerdir=<this diff>:<parent lowers>`. That is how `docker run --read-only`-ish internal mounts
and image inspection avoid needing a workdir. `[R]` MED — useful trick; adopt it for any
read-only mount we need.

### 9.3 `layerdb` and the container's writable layer

`[O]` HOST-A HIGH — `/var/lib/docker/image/overlay2/layerdb/mounts/<container id>/`:

```
mount-id: f3b8fd2296a2458698854b94b5eb4790688b54c3c121c0b19205f0b5a71ebc83
init-id:  f3b8fd2296a2458698854b94b5eb4790688b54c3c121c0b19205f0b5a71ebc83-init
parent:   sha256:baf694a87a40432d69679036c1578194a5e879889b2d1fe2b9d0ace9561e8898
```

`[O]` HOST-A HIGH — and the `-init` layer's position is visible in the mount: in the §8.3 table,
the live container's **first** (topmost) `lowerdir` entry `a5f79ae06f34` corresponds to no
`diff_id` at all — it is the init layer, sitting above the image's topmost layer `1ab184527cb3`
and below the container's `upperdir`.

`[C]` HIGH — Docker inserts an **`-init` layer** between the image's topmost layer and the
container's writable layer. It holds the handful of files the daemon must own per container —
`/etc/hosts`, `/etc/hostname`, `/etc/resolv.conf`, `/dev` placeholders, `/.dockerenv` — so that
they are *not* part of the image and *not* part of the user's writable diff (so `docker diff` does
not report them and `docker commit` does not bake them in).

`[R]` MED — **skip the init layer for MVP; put those files in as bind mounts instead.** D1's §6
already establishes the rootfs mount set and D4 owns `/etc/resolv.conf` content. A bind mount over
a file in the merged view achieves the same isolation without a whole extra layer. Tradeoff: bind
mounts show up in `/proc/mounts` inside the container where Docker's approach does not; nobody
will care at MVP. Revisit if `commit` ever lands.

### 9.4 containerd's snapshotter model — and whether to copy it

`[C]` HIGH — containerd replaced Docker's `graphdriver` with the **snapshotter** interface. The
conceptual difference is worth understanding even if we do not adopt the API:

| | Docker `graphdriver` | containerd `snapshotter` |
|---|---|---|
| Unit | a *layer* (`Create(id, parent)`, `ApplyDiff`, `Get`→mounted path) | a *snapshot*, keyed by a caller-chosen key, with a parent |
| States | implicit; `committed` is a marker file | explicit: **Active** (writable, has a workdir) vs **Committed** (immutable, usable as a parent) |
| Who mounts | the driver mounts and returns a **path** | the snapshotter returns a **`[]Mount` description**; *the caller* performs the mount |
| Lifecycle | `Create` → `Get` → `Put` → `Remove` | `Prepare(key, parent)` → *write into it* → `Commit(name, key)`; or `View(key, parent)` for read-only |

Three genuinely good ideas in there `[R]` HIGH to adopt, regardless of whether we copy the API:

1. **Active vs Committed as an explicit state, not a marker file.** An active snapshot has a
   `work/` and an `upper/` and may be mounted rw; a committed one has neither and may only appear
   in a `lowerdir`. Making this a Rust enum means the type system prevents mounting a committed
   snapshot rw or using an active one as a lower layer.
2. **Return a mount *description*, not a mounted path.** `struct Mount { source, target, fstype,
   options }` computed by the storage layer and executed by the runtime layer. This is exactly the
   D3/D1 boundary: we decide *what* to mount, D1 executes it inside the right namespace at the
   right point in the sequence. Returning a path forces the storage layer to mount in the host
   namespace, which is wrong for a container that should see it only after `pivot_root`.
3. **`Prepare` / `Commit` with a temporary key.** `Prepare("pull-abc", parent)` gives a writable
   snapshot you extract into; `Commit(chain_id, "pull-abc")` makes it immutable under its real
   name. That *is* the atomic-rename discipline from §7.6, expressed as an API.

`[R]` MED — do **not** copy the full snapshotter *plugin* abstraction (a trait with pluggable
btrfs/zfs/devmapper/native/stargz implementations). We have exactly one implementation, OverlayFS,
plus possibly a `native` (plain copy) fallback for filesystems where overlay does not work. One
trait with two implementations is defensible; one with one is speculative generality. Concretely:
write `struct OverlayStore` with `prepare`/`commit`/`mounts`/`remove` as inherent methods, and
extract a trait only when the second implementation actually exists.

---
## 10. Alternatives to OverlayFS — one paragraph each, and whether they matter to us

**10.1 Plain extracted directory (no union at all).** Apply every layer in order into one
directory; whiteouts become real `unlink`s. `[C]` HIGH — this is containerd's `native` snapshotter
and `umoci`'s default. Cost: O(total image size) disk and time **per container**, no sharing at
all. `[R]` **HIGH — build this first anyway.** It is ~50 lines on top of the extractor, it makes
`run` work end-to-end before overlay exists, it is the fallback for filesystems where overlay is
unavailable, and it is the reference implementation you diff against when overlay produces the
wrong tree. Keep it permanently as `--storage=native`.

**10.2 Bind mounts.** Not a layering mechanism — a bind mount gives you the *same* filesystem at a
second path, with no copy-on-write. `[R]` HIGH — irrelevant for layers, essential for everything
else: `/etc/resolv.conf`, `/etc/hosts`, volumes, and the "make the rootfs a mount point for
`pivot_root`" requirement D1 §10.1 flagged. Owned by D1 mechanically; we supply the list.

**10.3 `fuse-overlayfs`.** Userspace reimplementation of overlay semantics over FUSE, from the
Podman ecosystem. `[C]` HIGH — its reason to exist was rootless on kernels < 5.11 and on
filesystems where kernel overlay refuses. It encodes whiteouts as its own `user.*` xattrs, so no
`mknod` is needed. Costs: every read/write crosses to userspace (a real slowdown, order 2–5x on
metadata-heavy workloads), and it is another daemon per container. `[R]` MED — worth knowing as
the escape hatch for rootless-on-old-kernel; not worth implementing against, and we should never
*ship* it as a dependency when `userxattr` + kernel overlay works on our target.

**10.4 btrfs / zfs drivers.** Use the filesystem's native subvolume+snapshot primitives:
`Prepare` is `btrfs subvolume snapshot`, `Commit` is making it read-only. `[C]` HIGH — genuinely
elegant (real CoW at the block level, no copy-up penalty on large-file writes, cheap `diff`), and
containerd ships both. Costs: requires the whole store to live on that filesystem; btrfs's
snapshot count degrades performance at scale; zfs is out-of-tree. `[R]` LOW — irrelevant to us.
Note only that it is the strongest argument for the snapshotter abstraction in §9.4 — and we
already decided (`[R]` MED there) not to pay for that abstraction until a second backend exists.

**10.5 devicemapper — dead?** `[C]` HIGH — **yes, for our purposes.** `devicemapper` was
*deprecated in Docker 18.09* and **removed in Docker Engine v25**; it was the RHEL 7 answer to
"ext4 has no `d_type`" and its loopback-file default mode was notoriously fragile. The
block-level idea survives in containerd's `devmapper` snapshotter (thin-pool, still niche) and in
Kata/confidential-computing setups where a block device, not a directory, is what you must hand a
VM. `[R]` HIGH — ignore.

**10.6 erofs / composefs.** `[C]` HIGH — the current direction of travel. **EROFS** is a read-only,
compressible, mountable image format; **composefs** builds an EROFS metadata image whose file
*content* is redirected into a shared content-addressed store via overlay's **data-only lower
layers** (`::` / `datadir+`, §8.7), giving one immutable, fs-verity-verifiable metadata blob plus
byte-level dedup across images. `[R]` MED — the right thing to be aware of and the wrong thing to
build now. One concrete near-term note: **the `tar` crate is now maintained under
`github.com/composefs/tar-rs`**, so this ecosystem is where Rust container-storage work is
happening.

**10.7 Lazy pulling: eStargz / SOCI / Nydus.** `[C]` HIGH — the observation is that a container
typically reads <10% of its image, so instead of pulling and extracting whole layers, make the
layer *seekable* and fetch chunks on demand over HTTP ranges (§2.8 showed Docker Hub supports
them). **eStargz** is a gzip variant with a TOC that stays a valid `tar+gzip`, so it is backward
compatible; **Nydus** uses its own RAFS format and a userspace daemon; **SOCI** (AWS) keeps the
layer unmodified and ships a separate index artifact. All require a snapshotter that can serve a
filesystem before the data exists — i.e. FUSE or `erofs over fscache`. `[R]` HIGH — **out of
scope, permanently, for this project.** The reason to know it: it is the strongest argument for
§9.4's "return a mount *description*, not a path", because a lazy snapshotter's mount is a FUSE
mount rather than an overlay one, and only a description-based interface can express that.

---

## 11. Recommended progression — validating the project spec

The proposed sequence was: `directory rootfs → OCI image extraction → immutable layers →
OverlayFS → writable container layer`.

`[R]` HIGH — **the sequence is right and I would keep it.** Two corrections and one insertion:

### 11.1 Correction 1: split "OCI image extraction" — the registry client comes before the extractor

Building "pull an image" as one step means debugging HTTP auth, media-type dispatch, digest
verification, decompression and tar safety simultaneously. Split it:

- **1a. Registry client that fetches and verifies bytes.** Resolve the name, do the auth
  handshake, walk index → manifest → config, verify every digest, write blobs into `content/`.
  **Output: `mydocker pull alpine` prints the resolved config JSON and leaves verified blobs on
  disk. No extraction at all.** This is independently testable against Docker Hub *and* against
  a local `registry:2` container, and it is the whole of §2 and §3.
- **1b. The layer extractor**, driven from a blob already on disk. Now the entire §6 hazard list
  can be tested from a directory of hand-built malicious tars with **no network involved**.

Reason: these two have completely different failure modes and completely different test harnesses.
Fusing them means every extractor test needs a network.

### 11.2 Correction 2: "directory rootfs" first is right, but make it `native` storage, not a demo

`[R]` HIGH — implement step 0 as `--storage=native`: apply all layers into one directory
(§10.1), and keep it. It gives a working `run` on day one, and it stays as the differential
oracle for the overlay implementation forever. The spec's "directory rootfs" reads like a throwaway
scaffold; make it a permanent, supported, boring backend instead. That is strictly less total code
than writing a throwaway and later deleting it.

### 11.3 Insertion: OCI image-layout import/export, before OverlayFS

`[R]` MED — add `mydocker load ./alpine-oci/` reading an OCI image layout (§7.1). It is ~200 lines
because the store is already blob-shaped, and it means every later step can be developed offline
against `skopeo copy docker://alpine:latest oci:./alpine-oci:latest`, with zero Docker Hub rate
limit (`[O]` LOCAL: 100 pulls per 6 hours anonymously, §2.3) and deterministic bytes for tests.
This pays for itself within a week.

### 11.4 The corrected progression

| # | Step | Done when | Why here |
|---|---|---|---|
| 0 | **Digest + content store** (`content/blobs`, `ingest/`, atomic rename, fsync discipline) | `store put <file>` → blob is at its own digest, mode 0444; kill -9 mid-write leaves nothing in `blobs/` | Everything else writes through it |
| 1 | **Registry client** (name resolution, bearer auth, index→manifest→config, digest verification) | `pull alpine` verifies and stores 3 blobs; works against Hub *and* local `registry:2` | §2, §3 |
| 2 | **Layer extractor + safety layer** (openat2 SafeRoot, whiteouts, xattr allowlist, limits) | negative-test corpus (§6.14) all produce specific errors; alpine's rootfs matches `docker export` byte-for-byte modulo mtimes | §5, §6 — the dangerous code, isolated |
| 3 | **`native` storage + `run`** — apply all layers into one dir, hand it to D1 | `run alpine /bin/sh` gives a shell | first end-to-end vertical slice |
| 4 | **OCI image-layout import/export** | `skopeo copy` round-trips | unblocks offline dev/testing |
| 5 | **Immutable layers keyed by `diff_id`, snapshots keyed by chain ID**, dedup, `diffid-by-blob` cache | pulling two images sharing a base downloads the shared layers once | §4.5, §7.4 |
| 6 | **OverlayFS read-only mount** of a committed stack (`lowerdir` only, no upper) | `lowerdir` reversal test passes (§8.3); mount via `fsconfig`/`lowerdir+` | §8 |
| 7 | **Writable container layer** (`upper` + `work` per container, active/committed states) | writes in the container do not touch the image layers; `rm` of a lower file leaves a char 0/0 in upper | §9.4 idea 1 |
| 8 | **GC** (mark-and-sweep with grace period, mount-liveness check) | `rmi` + `gc` frees exactly the unreferenced blobs and layers, never a mounted one | §7.7 |

### 11.5 Which step locks in an expensive decision

`[R]` HIGH — **step 0 and step 5 together are the storage-layout lock-in.** Specifically:

1. **The keying scheme** — `layers/` by `diff_id`, `snapshots/` by chain ID. Changing this later
   means renaming or re-extracting every layer on every existing installation, i.e. writing a
   migration. Docker has carried `layerdb` + `cache-id` indirection since 2016 for exactly this
   reason: it *cannot* rename the overlay2 directories, so it keeps a permanent map from chain ID
   to a random directory name. **We can avoid inheriting that indirection only by getting the key
   right the first time.**
2. **"Everything on one filesystem"** — assumed by every atomic `rename` and by overlay's
   workdir rule (§7.4). Relaxing it later means introducing copy-instead-of-rename paths
   everywhere.
3. **`tar-split` capture (§5.5)** — the one decision that is *cheap now and expensive later*,
   because the data can only be captured **during extraction**. If we ever want `push` to
   reproduce original blob digests, retrofitting means re-extracting every layer in the store.
   `[R]` MED — I still say skip it (we have no push story and it adds a nontrivial dependency's
   worth of logic), but this is the one item to consciously accept rather than not notice.
   Write it down: `// ponytail: no tar-split; re-push will produce new blob digests`.

`[R]` HIGH — everything else on the list is cheap to change. The extractor's internals, the mount
API used, the media types accepted, the GC policy — all replaceable without touching disk layout.

---

## 12. OCI conformance target by stage

The useful framing is not "how much of the spec" but **"what is the cheapest thing that lets us
run a real `alpine` from Docker Hub"**, since that is interoperability, and everything else is
paper correctness.

### 12.1 The interoperability minimum — the short list

`[R]` HIGH — the smallest set that pulls and runs `docker.io/library/alpine:latest` today:

1. Name normalization: `docker.io`, `library/`, `latest`, `registry-1.docker.io` (§2.1).
2. Bearer token flow: 401 → parse challenge → token → retry (§2.3).
3. `Accept` with all four modern media types, **and dispatch on the response `Content-Type`** (§2.5).
4. Index → platform match `linux/amd64`, skipping `unknown/unknown` attestations (§2.6).
5. Manifest + config parse; verify every digest (§2.7, §2.9).
6. Blob GET, **follow 307 to the CDN, strip `Authorization` cross-host** (§2.8).
7. gzip decompress; verify compressed digest and `diff_id` (§4.2).
8. tar apply with `.wh.`/`.wh..wh..opq` (§5) and the §6 safety layer.
9. Overlay (or `native`) mount with `lowerdir` reversed (§8.3).
10. Translate image config → runtime `config.json`: `Env`, `Cmd`, `Entrypoint`, `WorkingDir`,
    `User` (§1.2).

That is the whole list. Note what is **not** on it: push, Schema 1, zstd, `urls`, referrers,
`_catalog`, tag listing, deletion, cross-repo mount, signatures, annotations.

### 12.2 By stage

| | **MVP** | **Intermediate** | **Mature** |
|---|---|---|---|
| **Distribution Spec** | The **pull** category only, and only the parts §12.1 lists. Anonymous + Basic + Bearer. | Full **pull** conformance (incl. `HEAD` manifest/blob, `Range` on blobs, correct error codes/`OCI-*` headers). **Content discovery**: `GET /v2/<name>/tags/list` with pagination. Credential store, mirrors, `insecure` registries. | **Push** (chunked/monolithic uploads, `POST…PATCH…PUT`, cross-repo `mount=`), **content management** (`DELETE`), referrers API, run the official `distribution-spec/conformance` suite in CI. |
| **Image Spec** | Read OCI + Docker Schema 2 manifest/index/config; gzip and uncompressed layers; whiteouts; `diff_ids`; chain IDs. | `image-layout` import/export; `+zstd`; full config translation (`ExposedPorts`, `Volumes`, `Labels`, `StopSignal`, `User` as name not just uid); `history`/`empty_layer` for `history` output; annotations surfaced in `inspect`. | Produce images (build/commit): layer diff generation, tar-split, reproducible gzip, index generation, `subject`/referrers for signatures/SBOMs. |
| **Runtime Spec** | D2 owns. Our contribution: a correct `root.path` and the `mounts[]` list. | | |
| **What we can honestly claim** | "pulls and runs OCI images from OCI-compliant registries" | "OCI Distribution pull-conformant" (testable) | "OCI Distribution pull+push+discovery conformant; OCI Image producer" |

`[R]` HIGH — **run the official conformance suite as soon as we claim pull conformance**:
`github.com/opencontainers/distribution-spec/conformance` is a Ginkgo binary driven by env vars
against a live registry; it is the only credible evidence for the claim, and it also finds our
bugs against a local `registry:2`.

`[R]` MED — the *cheapest high-value* item beyond §12.1 is **`HEAD` on the tag to skip a no-op
pull** (§2.4). One request, saves a whole pull, and it is what makes repeated `run` feel fast.

---

## 13. Rust crates in this area

**The binding constraint, from D2:** `[C]` HIGH — D2 §8.3 rules that the process which calls
`clone()` must be single-threaded and must never link an async runtime, and resolves it
structurally: **the pull path runs as a separate process invocation.** Quoting D2: "put the pull
path in a separate process, not just a separate module … It may be `#[tokio::main]`, use
`reqwest`, spawn 32 threads, whatever. It writes layers to disk and exits. The supervisor process
never links any of it."

`[R]` HIGH — **that resolves the sync-vs-async question for D3 completely: we are free to pick
whichever HTTP client is best, because none of it touches the supervisor.** Do not let this become
a reason to introduce async into the storage *mount* path, which does run in the supervisor.

Crate data below from crates.io on 2026-09-05.

### 13.1 HTTP client

| Crate | Version / health | What it abstracts | Verdict |
|---|---|---|---|
| **`ureq` 3.4.0** | updated 2026-08-08, 56.5M recent dl, `algesten/ureq` | **Blocking** HTTP/1.1 with TLS, redirects, `http` crate types. No runtime, small tree. | `[R]` **HIGH — use this.** Reasons: a pull is a handful of sequential requests plus N layer downloads that a thread pool handles fine; blocking I/O composes *directly* with `Read`-based decompression and tar (§4.2) with no `AsyncRead` bridging; and it keeps the option open of one day linking the puller into the supervisor. Tradeoff: no HTTP/2, so no request multiplexing — irrelevant at our request counts. **Check its redirect policy strips `Authorization` cross-host (§2.8 / CVE-2020-15157) and set it explicitly if not.** |
| **`reqwest` 0.13.4** | updated 2026-05-25, 176M recent dl | Async (tokio) HTTP/1+2, or a `blocking` feature that spawns a background runtime. | `[R]` MED — the safe default if we want HTTP/2 or connection pooling across many concurrent layer fetches. Its `blocking` mode still starts a tokio runtime thread, which is exactly the thing D2 forbids in the supervisor — acceptable only because pulls are a separate process. `[C]` HIGH — reqwest strips sensitive headers on cross-origin redirect by default; still verify. |
| **`hyper` 1.11.1** | 197M recent dl | The protocol layer, not a client. No redirects, no TLS wiring, no proxy handling. | `[R]` LOW — using hyper directly means reimplementing redirect-following and TLS setup. Educational, but the education is in HTTP plumbing, not containers. Skip. |

`[R]` HIGH — whichever we pick, **write the registry client ourselves** (auth state machine,
media-type dispatch, digest-verifying reader). That is where the container-specific learning is,
it is a few hundred lines, and it is the part we must understand to debug real registries.

### 13.2 OCI crates

| Crate | Health | Verdict |
|---|---|---|
| **`oci-spec` 0.10.0** | updated 2026-05-27, 4.4M recent dl, containers-org | `[R]` **HIGH — use it, for the types only.** It is `serde` structs + builders for image manifest/index/config **and** the runtime `config.json`, tracking spec versions. Writing these by hand is ~800 lines of `#[derive(Serialize, Deserialize)]` that teaches nothing and drifts from the spec. It hides no mechanics: the wire format is still JSON we see. **One caveat: it does not preserve raw bytes**, so keep our own `FetchedManifest { raw, digest, parsed }` wrapper (§2.4). |
| **`oci-client` 0.17.0** | updated 2026-05-19, 2.9M recent dl, `oras-project/rust-oci-client` | `[R]` **MED — read it, do not depend on it.** It is a competent async registry client (auth, manifests, blobs, push) and is the successor to `oci-distribution`. Depending on it means the auth handshake, media-type negotiation, digest verification and CDN-redirect handling — **the entire content of §2, which is the point of the exercise** — happen inside someone else's crate. That is the one place in this file where "write it ourselves" is clearly more educational than costly. Use it as a reference implementation and as a cross-check oracle. |
| **`oci-distribution` 0.11.0** | **last updated 2024-03-27**, declining downloads | `[R]` HIGH — **dead; superseded by `oci-client`.** Do not start here. |

### 13.3 tar — and the honest answer on safety

**`tar` 0.4.46** — updated 2026-05-18, 51M recent dl, now maintained at
**`github.com/composefs/tar-rs`**.

**Does it handle the §6 hazards? Mostly no — and it has had CVEs for exactly this.**

`[C]` HIGH — what it *does* give us: a correct, streaming, no-`unsafe`-in-the-parser reader for
ustar + PAX + GNU longname, header field decoding (including PAX `SCHILY.xattr.*` records exposed
via `Entry::pax_extensions()`), and `Entry::header()` giving raw `uid`/`gid`/`mode`/`mtime`/
`entry_type`/`link_name`. That is genuinely the tedious part and it is well tested.

`[C]` HIGH — what it does **not** safely give us is `unpack`/`unpack_in`:
- **RUSTSEC-2026-0067 / CVE-2026-33056** (`tar <= 0.4.44`, fixed 0.4.45): `unpack_dir` used
  `fs::metadata()`, which follows symlinks, so "a crafted tarball containing a symlink entry
  followed by a directory entry with the same name causes the crate to treat the symlink target as
  a valid existing directory — and subsequently apply `chmod` to it", changing permissions on
  arbitrary directories outside the extraction root.
- The same *shape* of bug has hit `async-tar` (CVE-2025-62518) and `astral-tokio-tar`
  (CVE-2025-59825) within the last year.
- It has no concept of OCI whiteouts, no xattr allowlist, no decompression limits, no `openat2`.

`[R]` **HIGH — use `tar` as a header/stream *parser* only. Never call `unpack`, `unpack_in`, or
`Entry::unpack`. Write our own applier on top of `Entry::header()` + `Entry` as a `Read`,
implementing §6.14 against `openat2` dirfds.** Reasoning: the parsing is boring, well-tested, and
not where the danger lives; the *applying* is where every CVE in this area lives, is where our
security posture is decided, and is ~400 lines we should own and test. This is the clearest
draw-the-line case in the whole file. Tradeoff: we do not get `unpack`'s convenience — which is
the entire point.

`[R]` MED — pin `tar >= 0.4.45` and put `cargo-deny`/`cargo-audit` in CI on day one. Given that
this crate had a path-traversal advisory in 2026, "we don't call the vulnerable function" is a
claim CI should keep honest.

### 13.4 Compression

| Crate | Health | Verdict |
|---|---|---|
| **`flate2` 1.1.10** | updated 2026-08-28, 150M recent dl, `rust-lang/flate2-rs` | `[R]` **HIGH — use it, with the `rust_backend` (miniz_oxide) feature, not `zlib-ng`/`zlib`.** `GzDecoder` is a `Read` adaptor that drops straight into the §4.2 pipeline. Pure-Rust backend means **no C in the code path that parses attacker-controlled compressed data** — which is precisely where memory-safety bugs historically live. Tradeoff: `zlib-ng` is measurably faster; take the safety. Re-measure only if decompression shows up in a profile. |
| **`zstd` 0.14.0** | updated 2026-09-04, 77M recent dl | `[R]` MED — bindings to the C `libzstd` (`zstd-sys`), so it *is* C parsing untrusted input. Not needed for MVP (§3.5). When needed, prefer it over `ruzstd` anyway (decode-only, far less exercised) but treat it as a trust-boundary dependency. |

`[R]` HIGH — **write our own `Read` wrapper enforcing the §6.9 limits** (input cap, output cap,
ratio cap) and compose it around whichever decoder. Neither crate will do this for us, and it is
15 lines.

### 13.5 Hashing

| Crate | Health | Verdict |
|---|---|---|
| **`sha2` 0.11.0** | updated 2026-03-25, 242M recent dl, RustCrypto | `[R]` **HIGH — use it.** Pure Rust with runtime-detected SHA-NI/AVX2 acceleration via `cpufeatures`. Implements `Digest` + `std::io::Write`, so a hashing pass-through reader is trivial. No C, no build-script toolchain requirement. |
| **`ring` 0.17.14** | **last updated 2025-03-11**, 153M recent dl | `[R]` MED — excellent crypto but includes assembly + C, needs a C toolchain, and has been slow-moving. Overkill: we need one hash function, not a crypto suite. |
| **`openssl`** | | `[R]` HIGH — **no.** A system C library dependency, dynamic-linking pain, and a huge attack surface for `sha256`. |

`[R]` HIGH — wrap it in a `DigestReader<R: Read>` that hashes as it passes bytes through, and a
`Digest` newtype that parses/formats `sha256:<64 hex>` and **compares in constant time** (use
`subtle` or a manual byte-XOR fold). Constant-time comparison for a *public* digest is not
strictly necessary, but it costs nothing and removes an argument.

### 13.6 JSON, and the digest-stability trap

**`serde_json` 1.0.151** — 300M recent dl. `[R]` HIGH — use it, obviously.

`[R]` **HIGH — but never re-serialize a manifest and hash the result** (§2.4). `serde_json` does
not preserve key order or whitespace, and the Image Spec's canonicalization rules mean the digest
is over the *exact received bytes*. Enforce this at the type level: `FetchedManifest` owns
`raw: Bytes` and the parsed view is derived from it, never the other way round.

`[R]` MED — use `serde_json::from_slice` with `#[serde(deny_unknown_fields)]` **off** (OCI
explicitly requires forward-compatible extensibility: `image-spec/considerations.md` says
implementations must ignore unknown fields), but **do** validate `schemaVersion == 2` and reject
`mediaType` values we do not know.

### 13.7 Content-addressed-store crates

`[R]` **HIGH — none. Write it.** Surveyed options (`cacache`, various `cas`/`blobstore` crates) all
bring their own metadata format, their own locking, and their own opinions about layout, while the
thing we need is: "write to a temp file, fsync, hash, rename to `blobs/sha256/<hex>`, fsync the
dir". That is 40 lines, it is the piece most worth understanding, and any crate that abstracts it
will also abstract away the OCI-layout compatibility (§7.1) we specifically want.

### 13.8 Syscall access

`[R]` HIGH — **`rustix`** for `openat2`, `linkat`, `symlinkat`, `mknodat`, `fchownat`, `fchmodat`,
`utimensat`, `renameat`, `fsync`, and the `fsopen`/`fsconfig`/`fsmount`/`move_mount` family. D1 §11
already recommends it for the runtime side for a reason specific to `clone()` safety; for us the
argument is simpler — it exposes `openat2` with typed `ResolveFlags` and the `*at` family with real
`OwnedFd`s, which is exactly the shape §6 needs, without hand-rolling `libc` calls.
`[R]` MED — verify `rustix` exposes the new mount API (`fsopen`/`fsconfig`) in the version we pin;
if not, that is ~30 lines of `libc::syscall` with the constants from `linux/mount.h`, which is what
I used for the §8.5 measurement.

### 13.9 Summary table

| Need | Take the crate | Write ourselves |
|---|---|---|
| HTTP transport | `ureq` (or `reqwest`) | the auth state machine, redirect/credential policy |
| OCI JSON types | `oci-spec` | raw-bytes-preserving manifest wrapper |
| Registry protocol | — (`oci-client` as reference only) | **all of it** |
| tar headers/stream | `tar` (parser only, `>= 0.4.45`) | **the applier, whiteouts, safety layer** |
| gzip | `flate2` (rust backend) | the limit-enforcing reader |
| sha256 | `sha2` | `DigestReader`, `Digest` newtype |
| JSON | `serde_json` | — |
| syscalls | `rustix` | — |
| content store | — | **all of it** |
| overlay mount | — | **all of it** |

---
## 14. Handoff

**14.1 To D1 (namespaces, `pivot_root`, mount mechanics).**

1. **We will hand you a mount *description*, not a mounted path** (§9.4). Shape:
   `Mount { source: "overlay", target, fstype: "overlay", options: [lowerdir, upperdir, workdir] }`,
   plus the rest of the rootfs mount set you specified in your §6. Please execute the overlay
   mount inside the mount namespace at the point your §10.1 sequence dictates, not before.
2. **Your §12 note is satisfied:** an overlay mount *is* a mount point, so the `pivot_root`
   requirement is met without the recursive self-bind. The self-bind is still needed for
   `--storage=native` (§10.1), which produces a plain directory. Both backends will exist.
3. **Please use the new mount API for overlay: `fsopen("overlay")` → repeated
   `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", <path>)` → `fsconfig(…, "upperdir"/"workdir")` →
   `FSCONFIG_CMD_CREATE` → `fsmount` → `move_mount`.** `[O]` HOST-A 6.11 — this mounts **500
   lower layers / 12,000 bytes of paths**, versus a hard **4,096-byte total option-string ceiling**
   for classic `mount(2)` (which fails with a misleading `ELOOP`). This is what lets us delete
   Docker's entire `l/` short-symlink subsystem (§9.2). It needs Linux ≥ 6.8 for `lowerdir+`.
   `move_mount` semantics inside your namespace sequence are your call — flagging that the
   fd-based mount is created *before* it has a location, which may actually simplify your
   ordering.
4. **`MS_NODEV` on the container rootfs and every writable mount is load-bearing for us**, not just
   hygiene: §6.5 explains that we must faithfully create device nodes from layers (we cannot
   distinguish a legitimate `char 0:0` from an overlay whiteout otherwise), so extraction-time
   rejection is not available and mount-time `nodev` is the defence.
5. `[O]` HOST-A confirms your note: Docker's live container overlay mounts carry `nouserxattr`.
   Our root-first MVP matches. The `userxattr` variant is a rootless-only change and must be paired
   with a matching layer store (§8.6) — a layer written with `trusted.*` markers cannot be mounted
   `userxattr` and vice versa.

**14.2 To D2 (runtime architecture, lifecycle, async).**

1. **Your §8.3 ruling resolves our HTTP question entirely** — pull as a separate process, so we
   can use whatever client we like. We are choosing a *blocking* one (`ureq`) anyway, because
   blocking `Read` composes directly with the decompress→hash→tar pipeline (§4.2); so if the
   process split ever needs to collapse, nothing in D3 blocks it. **No async will enter the mount
   path.**
2. **Naming, please enforce project-wide:** `ImageConfig` (the content-addressed blob) vs
   `RuntimeSpec`/`Spec` (the bundle's `config.json`). They are different specs and share no
   fields. Never `Config` (§1.2).
3. **We produce the bundle inputs, you produce the bundle.** From the image config we can hand you
   `Env`, `Cmd`, `Entrypoint`, `WorkingDir`, `User`, `StopSignal`, `ExposedPorts`, `Volumes`,
   `Labels` — pre-translated per `image-spec/conversion.md`. The `User` field may be a name, not a
   uid, which means resolving it against `/etc/passwd` **inside the extracted rootfs** — that is a
   D3-side read of a container file and a small hazard of its own (a malicious `/etc/passwd`);
   tell us where you want that boundary.
4. `[O]` LOCAL — Docker's **image ID is the config blob digest**, not the manifest digest. Whatever
   `inspect`/`ps` shows, be explicit about which digest is meant; users will paste both.

**14.3 To D4 (networking).**

- `/etc/resolv.conf`, `/etc/hosts`, `/etc/hostname`: Docker puts these in a per-container `-init`
  layer; `[R]` MED we plan to **bind-mount them over the merged view instead** (§9.3), which means
  you own the file *contents* and we own the mount entry. Say if you would rather have a writable
  copy in the container's upper layer.

**14.4 To D5 (cgroups, caps, seccomp, rootless).**

1. **We own the storage side of rootless; here is what it needs from you.** Three separate kernel
   features get conflated (§8.9): unprivileged overlay mount in a userns (5.11 upstream, backported
   much earlier by Ubuntu and RHEL), `-o userxattr` (5.11), and the fact that **whiteouts still
   need `mknod`, which a userns cannot do**. Your `[O]` ROCKY finding — `mknod` `EPERM` with a full
   41-cap `CapEff` — is exactly the corroboration for the third; thank you, it saves us a claim.
2. **The uid-shifting decision is joint.** Two options: (a) extract layers inside a userns, which
   makes the store keyed by `chainID + idmap` (layers stop being shareable across users), or
   (b) extract once as real uid/gid and use **idmapped mounts** (`mount_setattr` +
   `MOUNT_ATTR_IDMAP`, Linux 5.12+) to shift at mount time, keeping one shared store. **(b) is
   strictly better and it is the one I would build.** It needs your userns mapping and D1's mount
   sequencing; flagging it now because option (a) would change our on-disk keying, which §11.5
   lists as an expensive-to-change decision.
3. **`security.capability` in layer xattrs is a privilege-escalation vector** (§6.7). We allowlist
   xattr namespaces at extraction and reject `trusted.*`/`security.selinux`/`security.ima`/
   `security.evm`. Whether `security.capability` should be accepted at all depends on your
   userns/no-userns posture: without a userns, a v2 file capability from a layer applies with full
   host force. Tell us the policy and we will implement it as a config knob, defaulting to your
   answer.
4. `[O]` HOST-A — `apparmor_restrict_unprivileged_userns = 1` on Ubuntu 24.10 (your finding) will
   also block *our* rootless overlay experiments on that host, not just container creation.

---

## 15. Open questions

**15.1 The `old-host` gap — the largest hole in this file.**
`[O]` The host was unreachable for the entire session: `ssh -o BatchMode=yes -o ConnectTimeout=15
old-host` → `ssh: Could not resolve hostname old-host.example.edu`, and an independent `nslookup
old-host.example.edu` returned **NXDOMAIN**, so this is a DNS/hostname problem rather than a transient
outage. Three things I was asked to verify empirically and could not:
1. **What makes an unprivileged overlay mount succeed on a 4.18 kernel.** My documentary claim
   (§8.9) is that RHEL 8 carries a backport of the userns-mountable overlayfs work. **Unverified.**
   What would settle it: `grep -i overlay /proc/filesystems`, `cat /sys/module/overlay/parameters/*`,
   `mount -t overlay` inside `unshare -Ur -m` with and without `-o userxattr`, and
   `rpm -q --changelog kernel | grep -i 'overlay.*userns'`.
2. **The exact NFS-`upperdir` failure.** I expect `EINVAL` plus `overlayfs: filesystem on '<upper>'
   not supported as upperdir` in `dmesg`, but I did not observe it and have not claimed it as `[O]`.
   What would settle it: `mount -t overlay -o lowerdir=/tmp/l,upperdir=$HOME/u,workdir=$HOME/w` and
   capture errno + `dmesg`.
3. **Whether `-o userxattr` even exists at 4.18** — if the backport predates the `userxattr` option,
   then rootless overlay there works via a *different* mechanism, which changes the §8.9 story.
   `fuse-overlayfs` is installed on that host, which is consistent with `userxattr` being absent.

**15.2 `[O]` LOW — an unreproduced silent-truncation observation.** In one early run, `mount(8)`
(util-linux 2.40.2) with an ~8.6 KB overlay option string appeared to **succeed** while exposing
only ~141 of 300 lower layers. A careful re-run at 4,229 / 5,269 / 7,869 bytes failed cleanly with
`ELOOP` every time, through both `mount(8)` and direct `mount(2)`, so I could not reproduce it and
have not claimed it. If real, it is worse than an error (a silently wrong filesystem). It does not
change any recommendation — we use `fsconfig` and check lengths ourselves — but it is the one
observation in this file I would want a second person to try to reproduce.

**15.3 `[C]` MED — do `HEAD` requests count against Docker Hub's rate limit?** `[O]` LOCAL: our
manifest `GET` decremented `x-ratelimit-remaining` from 100 to 99. I did not isolate a `HEAD`. This
matters because §12.2's cheapest optimisation is "HEAD the tag to skip a no-op pull". Docker's
published policy changed in 2024 to make HEAD count. What would settle it: N sequential HEADs on
one tag, watching `x-ratelimit-remaining`. Cheap; skipped here to avoid burning the production
host's IP budget.

**15.4 `[C]` MED — the two disagreeing rate-limit windows.** The JWT says
`pull_limit=100 / pull_limit_interval=21600` (6 h); the response headers say `100;w=3600` (1 h).
Both from the same anonymous session, same minute. I do not know which governs. Recommendation
(§2.3) is to ignore both and back off on `429` + `Retry-After`, so nothing depends on the answer.

**15.5 `[R]` LOW — kernel maximum overlay stacking depth.** I verified **500 lower layers** mount
via `fsconfig` on 6.11. I did not find the ceiling. There is a separate `FILESYSTEM_MAX_STACK_DEPTH`
(2) governing overlay-on-overlay nesting, which is a different limit. Does not matter until someone
builds a 500-layer image, but worth knowing before we advertise "no layer limit".

**15.6 `[R]` MED — `tar-split`, decided-by-default rather than decided.** §5.5/§11.5: skipping it
means a future `push` produces different blob digests than the ones we pulled, and it cannot be
retrofitted without re-extracting the whole store. I recommend skipping. Flagging it as the one
"cheap now, expensive later" item so it is a decision and not an oversight.

**15.7 `[C]` LOW — non-Hub registry deviations.** Everything empirical in §2 is Docker Hub. GHCR,
ECR, GCR/AR, Quay and Harbor each have quirks (ECR's token endpoint is not the Docker protocol at
all; GHCR historically returned a token for *any* scope; some return `application/json` as the
manifest content type). We should test against at least GHCR and a local `registry:2` before
claiming registry independence. Not done here — Docker Hub was the specified target.

**15.8 `[R]` LOW — `User` resolution inside an untrusted rootfs.** The image config's `User` may be
a username, requiring us to read `/etc/passwd` *from the extracted image* to get a uid. That is
parsing attacker-controlled data to decide what uid a process runs as. Bounded (cap file size, cap
line count, reject non-numeric uids out of range), but it is a real trust boundary that neither
D2's file nor mine currently owns. Raised in §14.2.3.

---

## 16. Sources

**OCI specifications (primary)**
- Image Spec, overview — <https://github.com/opencontainers/image-spec/blob/main/spec.md>
- Image Spec, media types — <https://github.com/opencontainers/image-spec/blob/main/media-types.md>
- Image Spec, descriptor — <https://github.com/opencontainers/image-spec/blob/main/descriptor.md>
- Image Spec, manifest — <https://github.com/opencontainers/image-spec/blob/main/manifest.md>
- Image Spec, image index — <https://github.com/opencontainers/image-spec/blob/main/image-index.md>
- Image Spec, config (incl. ChainID, `rootfs.diff_ids`, `history`) — <https://github.com/opencontainers/image-spec/blob/main/config.md>
- Image Spec, layer / changesets / whiteouts — <https://github.com/opencontainers/image-spec/blob/main/layer.md>
- Image Spec, image layout — <https://github.com/opencontainers/image-spec/blob/main/image-layout.md>
- Distribution Spec — <https://github.com/opencontainers/distribution-spec/blob/main/spec.md>
- Distribution Spec conformance suite — <https://github.com/opencontainers/distribution-spec/tree/main/conformance>
- Runtime Spec, filesystem bundle — <https://github.com/opencontainers/runtime-spec/blob/main/bundle.md>
- Runtime Spec, `config.json` — <https://github.com/opencontainers/runtime-spec/blob/main/config.md>

**Kernel**
- `Documentation/filesystems/overlayfs.rst` — <https://raw.githubusercontent.com/torvalds/linux/master/Documentation/filesystems/overlayfs.rst> (rendered: <https://www.kernel.org/doc/html/latest/filesystems/overlayfs.html>)
- `openat2(2)` (`RESOLVE_IN_ROOT`, `RESOLVE_NO_MAGICLINKS`) — <https://man7.org/linux/man-pages/man2/openat2.2.html>
- `mount(2)`, `mount_setattr(2)`, `rename(2)`, `mknod(2)`, `xattr(7)`, `capabilities(7)`, `chown(2)` — man7.org
- new mount API (`fsopen`/`fsconfig`/`fsmount`/`move_mount`) — <https://man7.org/linux/man-pages/man2/fsconfig.2.html>

**Implementations**
- moby `daemon/graphdriver/overlay2/overlay.go` (linkDir, maxDepth=128, idLength=26, pageSize fallback) — <https://github.com/moby/moby/blob/master/daemon/graphdriver/overlay2/overlay.go>
- containerd `archive/tar.go` (`applyNaive`, whiteout handling, `createTarFile`, xattr/PAX, lchown-before-lchmod) — <https://github.com/containerd/containerd/blob/release/1.7/archive/tar.go>
- containerd `archive/tar_unix.go` (`skipFile`, `handleTarTypeBlockCharFifo`) — <https://github.com/containerd/containerd/blob/release/1.7/archive/tar_unix.go>
- containerd/continuity `fs/path.go` (`RootPath`, `walkLink`, 255-link limit) — <https://github.com/containerd/continuity/blob/main/fs/path.go>
- distribution `docs/spec/manifest-v2-2.md` (Docker Schema 2 / manifest list) — <https://github.com/distribution/distribution/blob/v2.8.3/docs/spec/manifest-v2-2.md>
- `distribution/reference` (`ParseNormalizedNamed`, `splitDockerDomain`) — <https://github.com/distribution/reference>
- `vbatts/tar-split` — <https://github.com/vbatts/tar-split>
- ORAS `rust-oci-client` — <https://github.com/oras-project/rust-oci-client>
- `composefs/tar-rs` — <https://github.com/composefs/tar-rs>

**Vulnerabilities**
- RUSTSEC-2026-0148 / CVE-2026-46703 — boxlite OCI layer symlink escape, `openat2(RESOLVE_IN_ROOT)` fix — <https://rustsec.org/advisories/RUSTSEC-2026-0148.html>
- RUSTSEC-2026-0067 / CVE-2026-33056 — `tar` `unpack_in` chmod via symlink, fixed 0.4.45 — <https://rustsec.org/advisories/RUSTSEC-2026-0067.html>
- CVE-2025-59825 — astral-tokio-tar path traversal, symlink-pair bypass — <https://github.com/advisories/GHSA-3wgq-wrwc-vqmv>
- CVE-2025-62518 — async-tar ("TARmageddon") — <https://linuxsecurity.com/news/security-vulnerabilities/linux-tar-async-tar-vulnerability-tarmageddon>
- CVE-2019-1002101 — `kubectl cp` symlink traversal — <https://unit42.paloaltonetworks.com/disclosing-directory-traversal-vulnerability-kubernetes-copy-cve-2019-1002101/>
- CVE-2018-15664 — `docker cp` TOCTOU symlink traversal — <https://medium.com/@cpuguy83/docker-symlink-traversal-cve-2018-15664-explained-3f75cd4259ff>
- CVE-2020-15157 ("ContainerDrip") — credential leak via manifest `urls` — referenced in §6.13
- CVE-2026-54574 — proot-distro symlink escape (same two-entry shape) — <https://github.com/advisories/GHSA-9xq3-3fqg-4vg7>

**Registry protocol / HTTP**
- RFC 7235 (HTTP Authentication), RFC 6750 (Bearer), RFC 9110 §14 / RFC 7233 (Range), RFC 1952 (gzip), RFC 8478 (zstd)
- Docker token authentication (`realm`/`service`/`scope`) — <https://distribution.github.io/distribution/spec/auth/token/>

**Crate metadata** — crates.io API, retrieved 2026-09-05: `oci-spec` 0.10.0, `oci-client` 0.17.0,
`oci-distribution` 0.11.0 (stale since 2024-03), `tar` 0.4.46, `flate2` 1.1.10, `zstd` 0.14.0,
`sha2` 0.11.0, `ring` 0.17.14 (stale since 2025-03), `reqwest` 0.13.4, `ureq` 3.4.0,
`hyper` 1.11.1, `serde_json` 1.0.151.

**Empirical**
- HOST-A `host-a` / `HOST-A`, Ubuntu 24.10, kernel `6.11.0-29-generic`,
  Docker `28.3.1`, containerd `1.7.27`, util-linux `2.40.2`, overlay2 on extfs.
  All mount experiments in `unshare -m --propagation private` with scratch under `/tmp`;
  every experiment directory and mount verified removed afterwards
  (`grep d3 /proc/mounts` clean, `ls -d /tmp/d3*` empty).
- LOCAL workstation, `curl`, anonymous — `registry-1.docker.io`, `auth.docker.io`,
  `production.cloudfront.docker.com`. No credentials used at any point.
- ROCKY `old-host` — **unreachable**, see §15.1.

**Companion research files** — `research/D1-kernel-isolation.md` (§6 rootfs mount set, §12
handoff), `research/D2-runtime-architecture.md` (§8.3 the process-split ruling that governs our
async choice), `research/D4-networking.md`.
