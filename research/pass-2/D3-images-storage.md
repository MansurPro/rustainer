# D3 — OCI images, registries & layered filesystems

Research reference. Claim tags: `[F]` fact (OCI spec / Linux primary source, cited),
`[C]` convention (what Docker/containerd/registries happen to do, cited),
`[R]` recommendation (ours, with tradeoff — never presented as fact),
`[O]` empirically verified (host + kernel/tool version + exact command).
Confidence `HIGH`/`MED`/`LOW`. Unsourced ⇒ LOW, with a note on what would settle it.

Scope: we own *what to mount* and *where the bytes come from*. D1 owns how mounting works
inside a namespace; D2 owns runtime lifecycle and the async decision; D5 owns the privilege
model. Cross-domain findings are collected in §14 Handoff.

## 0. Method and empirical baseline

Two hosts. Every `[O]` is scoped to exactly one of them and never promoted to `[F]`.

| Tag | Host | Kernel | Relevant tooling |
|---|---|---|---|
| **HOST-A** | `host-a` = `HOST-A`, Ubuntu 24.10 | `6.11.0-29-generic` x86_64 | root; Docker 28.3.1; containerd 1.7.27; runc 1.2.5 (spec 1.2.0); storage driver `overlay2` on extfs. **Live production** — read-only observation; my own mounts confined to a private mount namespace with all paths under `/tmp`, verified cleaned. |
| **ROCKY** | `old-host` = `OLD-HOST`, Rocky Linux 8.8 | `4.18.0-425.3.1.el8` x86_64 | no root, uid `900000000`; `fuse-overlayfs 1.13-dev` / FUSE 3.3.0; `/` is ext4, `$HOME` is NFS (`172.17.254.3:/cxfs1/OLD-HOST-home`); `/tmp` is on the ext4 root. |

`[O]` HOST-A — `docker info`: `Storage Driver: overlay2`, `Backing Filesystem: extfs`,
`Supports d_type: true`, `Using metacopy: false`, `Native Overlay Diff: true`,
`userxattr: false`. 5 running containers, 7 images, 107 entries under
`/var/lib/docker/overlay2/`. Confidence HIGH.

`[O]` HOST-A — `for f in /sys/module/overlay/parameters/*; do echo -n "$f="; cat $f; done`:
`check_copy_up=N index=N metacopy=N nfs_export=N redirect_always_follow=Y redirect_dir=N
redirect_max=256 xino_auto=Y`. These are *module defaults*, overridable per-mount.
`[O]` ROCKY same command: identical except `redirect_always_follow=N` and `xino_auto=N`.
Confidence HIGH.

Network probes to `registry-1.docker.io` / `auth.docker.io` were run from HOST-A with
anonymous tokens only, `curl 8.x`, no credentials, no `docker pull`.

---

## 1. The three OCI specs — exact boundaries and definitions

Three separate documents, three separate jobs. The single most common confusion in this
area is that two of them define a file called `config.json` / "image config" and they are
**different artifacts with different schemas**. §1.4 makes that unambiguous.

### 1.1 What each spec controls

| Spec | Controls | Does **not** control |
|---|---|---|
| **Image Format Specification** (`opencontainers/image-spec`) | The *content-addressable* representation of an image at rest and in transit: descriptors, manifests, indexes, the image config JSON, layer changeset format (tar + whiteouts), the on-disk `image-layout`, media types, annotations | How the image is fetched over the wire; how a rootfs is assembled from layers; anything about processes |
| **Distribution Specification** (`opencontainers/distribution-spec`) | The HTTP API for moving those blobs and manifests: `/v2/` endpoints, pull, push, chunked upload, content discovery (tag list, referrers), deletion, error codes | Authentication (explicitly out of scope), storage layout, how blobs are produced |
| **Runtime Specification** (`opencontainers/runtime-spec`) | The **filesystem bundle** (a directory containing `config.json` + a rootfs), the runtime `config.json` schema, container lifecycle and operations (`create`/`start`/`kill`/`delete`), Linux-specific config (namespaces, cgroups, seccomp, mounts) | Where the rootfs came from; images; registries; layers; digests |

`[F]` HIGH — The Image Spec's own scope statement: it defines "an OCI Image" as consisting
of "an image manifest, an image index (optional), a set of filesystem layers, and a
configuration." Source: image-spec `spec.md`, "Overview".

`[F]` HIGH — Authentication is **out of scope** for the Distribution Spec. It only says the
`/v2/` endpoint "MAY be used for authentication/authorization purposes". Source:
distribution-spec `spec.md`, "Determining Support". **Consequence for us: the entire bearer
token dance in §2 is not in any OCI spec.** It is the Docker Registry v2 auth convention
plus RFC 6750/9110, and every registry we care about implements it. This is a `[C]`, and the
most important `[C]` in this whole document.

### 1.2 The handoff point — where Image Spec stops and Runtime Spec starts

`[F]` HIGH — image-spec `spec.md` states the goal is that the image, once unpacked, becomes
"a runtime bundle" that can be run by "an OCI Runtime". The conversion step — apply layer
changesets in order to produce a rootfs directory, and translate the *image* config's
`config` object (Env, Cmd, Entrypoint, User, WorkingDir, ExposedPorts, Volumes) into a
*runtime* `config.json` — is **specified by neither spec**. It is a gap, deliberately.

`[R]` HIGH — Name this component explicitly in our architecture (call it the *bundler*) and
keep it a pure function `(ImageConfig, RootfsPath, RunOptions) -> RuntimeConfig`. Reason: it
is the one place where image semantics and runtime semantics meet, it is where every "why
did my ENTRYPOINT not run" bug lives, and it is the only part of the image→run path with no
spec to test against. Tradeoff: an extra module boundary for what looks like glue code; the
payoff is that it is unit-testable without touching the kernel.

### 1.3 Cited definitions

`[F]` HIGH unless noted. Sources: image-spec `spec.md`, `descriptor.md`, `manifest.md`,
`image-index.md`, `config.md`, `layer.md`, `image-layout.md`; runtime-spec `spec.md`,
`bundle.md`, `config.md`.

- **Digest** — `descriptor.md`: a digest is `algorithm ":" encoded`, "a serialized hash
  result, consisting of an algorithm portion and an encoded portion". `sha256` is the
  registered required algorithm; `sha512` is registered/optional. Digests are the *only*
  identity in the format: "the digest serves as the *content identifier*". A digest is
  computed over the **exact bytes** of the blob, not over any parsed form.
- **Blob** — a content-addressable byte stream stored and retrieved by its digest. The Image
  Spec never calls it "file"; blobs are layers, configs and manifests alike.
- **Descriptor** — `descriptor.md`: the tuple `{mediaType, digest, size}` (all REQUIRED)
  plus optional `urls`, `annotations`, `data`, `artifactType`, `platform`. A descriptor is
  the *typed pointer* used everywhere; the whole format is descriptors pointing at blobs.
  Note `size` is REQUIRED — this is our download-bound and a security control (§6.9).
- **Layer** — `layer.md`: a "filesystem changeset", serialized as a tar archive of the
  *changes* relative to the layers beneath it, including whiteout entries for deletions.
  Media types `application/vnd.oci.image.layer.v1.tar`, `…tar+gzip`, `…tar+zstd`.
- **Manifest** — `manifest.md`: a single-platform image: `schemaVersion` (MUST be `2`),
  optional `mediaType`, a `config` descriptor, and a `layers` array. Ordering is normative:
  "The array MUST have the base layer at index 0. Subsequent layers MUST then follow in
  stack order."
- **Image Index** (a.k.a. manifest list) — `image-index.md`: "a higher-level manifest which
  points to specific image manifests, potentially for one or more platforms". Each entry is
  a descriptor with an optional `platform` object. It is the multi-arch fan-out point.
- **Image config** — `config.md`: the JSON describing "basic information about the image
  such as date created, author, as well as execution/runtime configuration", plus
  `rootfs.diff_ids` and `history`. "Each image's ID is given by the SHA256 hash of its
  configuration JSON" — i.e. **the image ID is the digest of the config blob**, which makes
  the image content-addressable through the whole DAG.
- **rootfs** (Image Spec sense) — the result of applying every layer changeset in
  `diff_ids` order onto an empty directory.
- **Filesystem bundle / rootfs** (Runtime Spec sense) — runtime-spec `bundle.md`: a
  directory containing exactly two things a runtime must find: a `config.json` in the
  bundle's top-level directory, and "a directory representing the root filesystem of the
  container", referenced by `root.path` in that `config.json`.
- **Registry** — distribution-spec: a service implementing the `/v2/` HTTP API over a
  content-addressable store of blobs and a mutable tag→manifest mapping.
- **Runtime** — runtime-spec: the thing that consumes a bundle and produces a running
  container; it has no knowledge of images or registries.

`[F]` HIGH — image-spec `image-layout.md` defines the on-disk layout: a `blobs/` directory
whose subdirectories are digest algorithms and whose files are named by the *encoded* part
of the digest (`blobs/sha256/<hex>`), an `oci-layout` file containing
`{"imageLayoutVersion": "1.0.0"}`, and an `index.json` which is an image index acting as the
entry point. `index.json` entries carry `org.opencontainers.image.ref.name` annotations to
express tags.

### 1.4 `config.json` — two different files, disambiguated

This is the trap. Both are JSON, both are called a "config", and they share almost no
fields.

| | **Image config** | **Runtime `config.json`** |
|---|---|---|
| Spec | image-spec `config.md` | runtime-spec `config.md` |
| Media type | `application/vnd.oci.image.config.v1+json` | none — it is a plain file in a bundle, not a blob |
| Identity | its own sha256 digest = the **image ID** | none; it is mutable, per-container |
| Lives | as a blob in the registry / CAS | as `<bundle>/config.json` on local disk |
| Top-level keys | `architecture`, `os`, `variant`, `created`, `author`, `config`, `rootfs`, `history` | `ociVersion`, `root`, `process`, `hostname`, `mounts`, `linux`, `hooks`, `annotations` |
| Process info | `config.Env`, `config.Cmd`, `config.Entrypoint`, `config.User`, `config.WorkingDir`, `config.ExposedPorts`, `config.Volumes`, `config.Labels`, `config.StopSignal` | `process.args`, `process.env`, `process.cwd`, `process.user.{uid,gid}`, `process.capabilities`, `process.rlimits` |
| Filesystem info | `rootfs.type` (`"layers"`), `rootfs.diff_ids[]` | `root.path`, `root.readonly`, `mounts[]` |
| Knows about layers | yes — that is its main job | **no** |
| Knows about namespaces/cgroups/seccomp | **no** | yes — `linux.namespaces`, `linux.resources`, `linux.seccomp` |

`[F]` HIGH — Note the field-name collision: the image config has a nested object literally
named `config`. So `imageConfig.config.Cmd` is the image's default command, while
`runtimeConfig.process.args` is what the runtime will `execve`. `[R]` HIGH — in Rust name
these types `ImageConfig` and `RuntimeSpec` (never `Config`), and never derive one from the
other implicitly. Reasoning: the compiler then catches the confusion for free. Tradeoff:
`RuntimeSpec` is a slightly odd name for a struct representing `config.json`; that is worth
it.

`[F]` HIGH — The Image Spec's `config.Cmd`/`Entrypoint` split has **no** Runtime Spec
equivalent. `process.args` is a single flat array. `[C]` HIGH — Docker's rule is
`args = Entrypoint ++ Cmd`, where a user-supplied command replaces `Cmd` only, and
`--entrypoint` replaces `Entrypoint` and *clears* `Cmd`. Source: Docker docs, "Dockerfile
reference — ENTRYPOINT / CMD interaction table". This is convention we must copy to be
useful, not a spec requirement.

---

## 2. The complete `docker pull alpine` trace

Every step below was executed on HOST-A with anonymous tokens. Digests are from
2026-09-04; they will drift as `alpine:latest` is rebuilt.

### 2.1 Step 0 — reference resolution: almost entirely convention

`alpine` → `docker.io/library/alpine:latest`. Four separate defaults, none of them in an
OCI spec:

1. `[C]` HIGH — **default registry** `docker.io`. Hardcoded in the client. Source:
   `distribution/reference` (`normalize.go`, `defaultDomain = "docker.io"`), used by both
   Docker and containerd.
2. `[C]` HIGH — **`library/` namespace injection** for single-component names. Same source:
   `officialRepoPrefix = "library/"`, applied only when the domain is `docker.io` and the
   path has no `/`. `ghcr.io/foo` is *not* rewritten to `ghcr.io/library/foo`.
3. `[C]` HIGH — **default tag** `latest`. Same source, `defaultTag = "latest"`. `latest`
   has no special meaning to a registry; it is an ordinary mutable tag.
4. `[C]` HIGH — **endpoint remap** `docker.io` → `registry-1.docker.io`. `[O]` HOST-A
   `curl -sS -o /dev/null -D - https://docker.io/v2/` → `HTTP/2 302`,
   `location: https://www.docker.com/v2/`. So `docker.io` is **not** a registry at all; the
   client rewrites the host. `[O]` `https://index.docker.io/v2/` and
   `https://registry-1.docker.io/v2/` both return `401` with the same
   `WWW-Authenticate`. Confidence HIGH.

`[F]` HIGH — What *is* specified is the grammar of the parts. distribution-spec: `<name>`
MUST match `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*` and a
tag MUST be ≤128 chars matching `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`.

`[R]` HIGH — Implement reference parsing as its own module with a table of ~40 cases
(`alpine`, `alpine:3.20`, `alpine@sha256:…`, `localhost:5000/x`, `example.com/a/b:t@sha256:…`,
`docker.io/library/alpine`). Reason: the domain-vs-first-path-component ambiguity
(`localhost:5000/x` is a registry; `foo/x` is a Hub user repo) is decided by a heuristic —
containerd/Docker treat the first component as a domain if it contains `.` or `:` or equals
`localhost`. Getting this wrong silently sends requests to the wrong host. Tradeoff: a
pile of unglamorous string code; it is the cheapest correctness win in the whole puller.

### 2.2 Step 1 — `GET /v2/` and the 401 challenge

`[O]` HOST-A, `curl -sS -i https://registry-1.docker.io/v2/`:

```
HTTP/2 401
docker-distribution-api-version: registry/2.0
www-authenticate: Bearer realm="https://auth.docker.io/token",service="registry.docker.io"
{"errors":[{"code":"UNAUTHORIZED","message":"authentication required","detail":null}]}
```

Note there is **no `scope`** on the `/v2/` challenge — it is a ping, not a resource.
Confidence HIGH.

`[O]` HOST-A — the challenge differs per registry, and one of them lies:

| Registry | `WWW-Authenticate` on `GET /v2/` |
|---|---|
| `registry-1.docker.io` | `realm="https://auth.docker.io/token",service="registry.docker.io"` |
| `ghcr.io` | `realm="https://ghcr.io/token",service="ghcr.io",scope="repository:user/image:pull"` |
| `quay.io` | `realm="https://quay.io/v2/auth",service="quay.io"` |
| `registry.k8s.io` | `realm="https://registry.k8s.io/token",service="registry.k8s.io"` |

`[O]` HIGH — **`ghcr.io` returns a literal placeholder scope `repository:user/image:pull`.**
A client that echoes back the challenge's `scope` verbatim will request a token for a
repository named `user/image`. `[R]` HIGH — always synthesize the scope from the reference
you are about to fetch (`repository:<name>:pull`), and use the challenge only for `realm`
and `service`. Tradeoff: technically less "correct" per RFC 6750 challenge-following; in
practice it is the only thing that works everywhere.

`[F]` MED — distribution-spec: a `200` on `/v2/` means the registry implements the spec;
`401` or `404` "indicates failure". The 401-then-auth flow is therefore *outside* the spec
(§1.1).

`[R]` MED — Skip `GET /v2/` in normal operation. Reason: it costs a round trip and yields
nothing a lazily-handled 401 on the real request would not. Go straight to the manifest
request, handle the 401, retry once with a token. Keep `/v2/` only behind a
`--check-registry` diagnostic. Tradeoff: you lose an early "this host is not a registry"
error, which becomes a confusing 404 later.

### 2.3 Step 2 — the token fetch (RFC 6750 bearer, Docker's token-server convention)

`[O]` HOST-A:
`curl -sS "https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/alpine:pull"`
→ 200 with keys `['access_token', 'expires_in', 'issued_at', 'token']`,
`expires_in: 300`, `issued_at: 2026-09-04T16:50:18Z`. Confidence HIGH.

`[C]` HIGH — `token` and `access_token` are duplicates; `access_token` is the OAuth2 name,
`token` the original Docker name. Source: Docker "Token authentication specification".
Read `token` and fall back to `access_token`.

`[O]` HIGH — The token is a 3-segment RS256 JWT, 2698 bytes, header carrying an `x5c`
chain (`CN=Docker, Inc. Eng JWT CA`). Decoded claims:

```json
{"iss":"auth.docker.io","aud":"registry.docker.io","sub":"","nbf":1788540318,"exp":1788540918,
 "access":[{"type":"repository","name":"library/alpine","actions":["pull"],
            "parameters":{"pull_limit":"100","pull_limit_interval":"21600",
                          "registry_region":"us-east-1"}}]}
```

**The anonymous rate limit is inside the token**: `pull_limit=100` per
`pull_limit_interval=21600` seconds (6 hours). `exp - nbf = 600s` even though `expires_in`
said 300. Confidence HIGH.

`[O]` HIGH — Response headers on manifest requests advertise a *different* window:
`ratelimit-limit: 100;w=3600` and `ratelimit-remaining: 95;w=3600` (RFC 9110-style
`RateLimit-*`), plus `docker-ratelimit-source: <our public IP>`. The header says a 1-hour
window; the token says 6 hours. **They disagree.** `[R]` HIGH — trust neither number;
implement retry-on-429 with exponential backoff and honour `Retry-After`, and surface
`ratelimit-remaining` in verbose output only. What would settle it: pulling until we get
a 429 and timing the recovery — not worth the quota on a production host.

`[O]` HIGH — anonymous limits are **per source IP** (`docker-ratelimit-source` is our
egress IP), so a CI runner behind NAT shares one bucket. This is the single most common
real-world pull failure and the reason a token cache is not optional.

`[R]` HIGH — Cache tokens keyed by `(realm, service, scope)` with the response's
`expires_in` minus a 30s skew. Reason: with `expires_in: 300`, a 20-layer pull that
re-authenticates per request would burn 20+ token requests and can outlive the token
mid-pull. Tradeoff: a small amount of state; must handle a mid-pull 401 by re-authenticating
and retrying the *same* request once.

### 2.4 Step 3 — manifest GET, and the `Accept` header that Docker Hub ignores

The `Accept` list a client must send (`[C]` HIGH — matches containerd's
`docker.Resolver` default set):

```
application/vnd.oci.image.index.v1+json
application/vnd.oci.image.manifest.v1+json
application/vnd.docker.distribution.manifest.list.v2+json
application/vnd.docker.distribution.manifest.v2+json
application/vnd.docker.distribution.manifest.v1+prettyjws   (optional, legacy)
application/json                                            (optional, legacy)
```

`[O]` HOST-A — `GET /v2/library/alpine/manifests/latest` with a token:

```
HTTP/2 200
content-type: application/vnd.oci.image.index.v1+json
content-length: 9218
docker-content-digest: sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b
etag: "sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b"
```

`sha256sum` of the 9218 response bytes = `28bd5fe8…` — matches. Confidence HIGH.

`[O]` HIGH — **Docker Hub ignores `Accept` completely for this repository.** Six requests,
identical `Content-Type: application/vnd.oci.image.index.v1+json` every time:

| `Accept:` sent | Content-Type returned |
|---|---|
| *(header omitted)* | `application/vnd.oci.image.index.v1+json` |
| `…docker.distribution.manifest.list.v2+json` only | `…oci.image.index.v1+json` |
| `…docker.distribution.manifest.v2+json` only | `…oci.image.index.v1+json` |
| `…docker.distribution.manifest.v1+prettyjws` only | `…oci.image.index.v1+json` |
| `…oci.image.manifest.v1+json` only | `…oci.image.index.v1+json` |
| `*/*` | `…oci.image.index.v1+json` |

`[R]` HIGH — **Dispatch on the response `Content-Type`, and cross-check it against the
parsed document, never on what you asked for.** Concretely: read `Content-Type`; if it is
absent or unrecognised, sniff by parsing JSON and branching on the presence of `manifests[]`
(index) vs `config`+`layers[]` (manifest) vs `fsLayers[]` (schema 1). Reason: the observed
behaviour above means a client that assumes it got what it asked for will mis-parse. This
also protects against a registry that omits `mediaType` inside the document. Tradeoff: a
tiny amount of sniffing logic that a spec-pure client would not need.

`[F]` HIGH — distribution-spec: "A successful response MUST contain the digest of the
uploaded blob in the header `Docker-Content-Digest`." The header name is frozen with
`Docker-` in it; there is no OCI-branded equivalent.

`[R]` HIGH — Treat `Docker-Content-Digest` as a *hint only*. Always recompute. Reason: for
a tag request it tells you the digest you would have gotten by pinning, which is exactly
what you want to record for the tag→digest mapping — but it is registry-supplied. For a
by-digest request you already know the expected value. Tradeoff: none; hashing 9KB is free.

`[O]` HIGH — HEAD works and is cheap: `curl -I` on the tag returns `200`, full
`content-length`, `docker-content-digest`, `etag`, and **no body**. `[R]` HIGH — use
`HEAD <tag>` for "is my cached image still current?" — one request, no body, and on Hub it
still consumed a rate-limit token (`ratelimit-remaining` decremented across the HEAD/GET
pair in our run), so it saves bytes, not quota.

### 2.5 Step 4 — platform selection from the index

`[O]` HOST-A — the `alpine:latest` index has `schemaVersion: 2`,
`mediaType: application/vnd.oci.image.index.v1+json`, and **16** entries. Two kinds:

```json
{ "mediaType":"application/vnd.oci.image.manifest.v1+json",
  "digest":"sha256:79ff19e9084a00eece421b2523fb93e22d730e2c0e525905de047e848e56d95f",
  "size":1022, "platform":{"architecture":"amd64","os":"linux"},
  "annotations":{"org.opencontainers.image.version":"3.24.1", ...} }
```
```json
{ "mediaType":"application/vnd.oci.image.manifest.v1+json",
  "digest":"sha256:f21951a6120df0f5f9329311202f7869e7b70120f4748632d58753cff662b126",
  "size":838, "platform":{"architecture":"unknown","os":"unknown"},
  "annotations":{"vnd.docker.reference.type":"attestation-manifest",
                 "vnd.docker.reference.digest":"sha256:79ff19e9…"} }
```

`[O]`/`[C]` HIGH — **half the index entries are BuildKit attestation manifests carrying
`platform: {architecture: "unknown", os: "unknown"}`.** 8 real platforms + 8 attestations.
Source for the convention: BuildKit "attestation storage" docs
(`vnd.docker.reference.type=attestation-manifest`).

`[R]` HIGH — Platform matching algorithm: (a) drop any entry whose
`platform.architecture` or `platform.os` is `"unknown"`; (b) require
`os == "linux"` and `architecture == "amd64"` (our target); (c) if `variant` is present it
must match, with the `arm` special cases (`arm/v7` etc.) deferred until we care; (d) if
zero survive, **fail with the list of platforms that were on offer** rather than a generic
"not found". Reason: the `unknown/unknown` filter is not optional — without it a naive
"first entry whose os is not the one I want, keep going… else take entry 0" search can
select an attestation manifest, whose "layers" are JSON documents, and you will extract a
tarball that is not a tarball. Tradeoff: an extra rule that reads like a hack because it
*is* a hack, imposed by BuildKit.

`[F]` MED — image-index.md makes `platform` OPTIONAL on entries. `[R]` MED — treat a
missing `platform` as non-matching rather than as a wildcard, and say so in the error.

`[F]` HIGH — `platform.architecture` values follow Go's `GOARCH` (`amd64`, `arm64`,
`386`, `ppc64le`, `s390x`, `riscv64`) and `platform.os` follows `GOOS`. Source:
image-spec `image-index.md`, which normatively references the Go values.

### 2.6 Step 5 — the platform manifest

`[O]` HOST-A — `GET /v2/library/alpine/manifests/sha256:79ff19e9…`:

```
content-type: application/vnd.oci.image.manifest.v1+json
content-length: 1022
docker-content-digest: sha256:79ff19e9084a00eece421b2523fb93e22d730e2c0e525905de047e848e56d95f
```
```json
{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json",
 "config":{"mediaType":"application/vnd.oci.image.config.v1+json",
           "digest":"sha256:d529dd0c6e5597ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b","size":611},
 "layers":[{"mediaType":"application/vnd.oci.image.layer.v1.tar+gzip",
            "digest":"sha256:55afa1ecc21d2bb5e5045f32dafee56272ffd89860bac26f6c32123439af26a4",
            "size":3846391}]}
```
`sha256sum` of the 1022 bytes = `79ff19e9…`. Confidence HIGH.

### 2.7 Step 6 — the config blob, and the blob redirect

`[O]` HOST-A — `GET /v2/library/alpine/blobs/sha256:d529dd0c…` (no `-L`):

```
HTTP/2 307
content-type: application/octet-stream
content-length: 0
location: https://production.cloudfront.docker.com/registry-v2/docker/registry/v2/blobs/sha256/d5/d529dd0c…/data?Expires=1788543652&Signature=…&Key-Pair-Id=K2C9XPB6FLAKUF
```
Following it: `HTTP/2 200`, `content-length: 611`, body is the image config, and
`sha256sum` = `d529dd0c…`. Confidence HIGH.

`[O]` HIGH — **`HEAD` on a blob does *not* redirect.** `curl -I` on the same blob URL
returns `HTTP/2 200` directly from `registry-1.docker.io` with
`content-length: 3846391`, `accept-ranges: bytes`, `cache-control: max-age=31536000`,
`docker-content-digest`, and `etag`. So HEAD is the cheap way to learn a blob's size and
confirm existence without touching the CDN.

`[O]` HIGH — **the CDN response carries no `Docker-Content-Digest`.**
`curl -sSL -r 0-99` on the layer blob ends at `HTTP/2 206` with
`server: AmazonS3`, `x-cache: Hit from cloudfront`,
`etag: "623d8efe162d9620ee60d0eed332ccfe"` (an S3 MD5, *not* the sha256),
`content-range: bytes 0-99/3846391`. There is no digest header to trust. Confidence HIGH.

`[R]` HIGH — Three hard rules from that redirect:
1. **Do not forward the `Authorization` header across the redirect.** The CloudFront URL is
   pre-signed (`Expires` + `Signature` + `Key-Pair-Id`); the bearer token is useless there
   and leaking it to a third-party host is a credential disclosure. In Rust: `reqwest`'s
   default redirect policy already strips sensitive headers on cross-origin redirects, but
   we must not "helpfully" re-add the header in a custom policy.
2. **The presigned URL expires** (`Expires` was ~50 min out in our capture). A resumed
   download must re-request the `/v2/.../blobs/<digest>` URL to get a fresh redirect, not
   reuse the stored CDN URL.
3. **Digest verification is the client's job, unconditionally.** distribution-spec only says
   "Clients SHOULD verify that the response body matches the requested digest" — and the CDN
   gives you nothing to verify against. `[R]` treat it as MUST.

`[F]` HIGH — distribution-spec: "Registries MAY respond to any request with a redirect per
RFC 9110; clients SHOULD follow such redirects." And: "A registry SHOULD support the `Range`
request header in accordance with RFC 9110."

`[O]` HIGH — Range works: `-H "Range: bytes=0-1023"` → `HTTP/2 206`,
`content-range: bytes 0-1023/3846391`, `accept-ranges: bytes`. So resumable layer downloads
are available on Hub today.

### 2.8 Step 7 — layer blobs, and the two digests of one layer

`[O]` HOST-A, single pass over the downloaded blob computing both hashes
(`python3` `hashlib.sha256` + `zlib.decompressobj(16+MAX_WBITS)`):

```
compressed   bytes 3846391  sha256:55afa1ecc21d2bb5e5045f32dafee56272ffd89860bac26f6c32123439af26a4
uncompressed bytes 8697856  sha256:34884abbe92863fce933ed7c39c0e045631af0ed86d5cc0dfbdf9fdca426ce3c
```
- `55afa1ec…` = `manifest.layers[0].digest` — the registry's identity for the blob.
- `34884abb…` = `config.rootfs.diff_ids[0]` — the *content* identity of the extracted layer.

Confidence HIGH. This is the mechanic worth internalising: **a layer blob's compressed
digest identifies the blob in the registry, while its uncompressed digest (`diff_id`) is
what the image config's `rootfs.diff_ids` lists — so a puller must hash both streams
simultaneously**, or it must store the compressed blob and decompress a second time later.

`[O]` HIGH — closure with the local store: `docker image inspect alpine:latest` on HOST-A
gives `Id=sha256:d529dd0c…` (= the config blob digest),
`RepoDigests=[alpine@sha256:28bd5fe8…]` (= the index digest),
`RootFS.Layers=[sha256:34884abb…]` (= the diff_id). Every identity in the local store is one
of the digests we saw on the wire.

`[R]` HIGH — Pipeline the download as
`HTTP body → sha256(compressed) → gzip decoder → sha256(uncompressed) → tar reader → extractor`,
all streaming, and commit nothing until both digests verify. Reason: it removes an entire
decompression pass and never materialises a full layer in memory. Tradeoff: on a digest
mismatch you have already written files to a staging directory and must delete it — which is
why the staging directory must be per-attempt and cheap to `rm -rf` (§7.5).

### 2.9 Errors and quirks worth coding for

`[O]` HOST-A — A nonexistent repository returns **`401 UNAUTHORIZED`, not `404
NAME_UNKNOWN`**, even holding a successfully-issued anonymous token:
```
GET /v2/library/definitelynotarealimagexyz/manifests/latest
HTTP/2 401
{"errors":[{"code":"UNAUTHORIZED","message":"authentication required",
  "detail":[{"Type":"repository","Name":"library/definitelynotarealimagexyz","Action":"pull"}]}]}
```
The token endpoint happily issued a token for it. So on Docker Hub "does not exist" and "you
may not see it" are **indistinguishable**. Confidence HIGH. `[R]` HIGH — our error message
for a 401 after a successful token fetch must say "not found or not accessible", never
"authentication failed", or every typo turns into a bogus credentials bug report.

`[O]` HIGH — A blob request for an all-zeros digest returns `404` with
`content-type: application/json`.

`[F]` MED — distribution-spec defines the error codes we should map: `BLOB_UNKNOWN`,
`MANIFEST_UNKNOWN`, `NAME_UNKNOWN`, `NAME_INVALID`, `DIGEST_INVALID`, `SIZE_INVALID`,
`UNAUTHORIZED`, `DENIED`, `TOOMANYREQUESTS`, `UNSUPPORTED`. The body is
`{"errors":[{"code","message","detail"}]}`.

`[O]` HIGH — Other live endpoints on Hub: `GET /v2/library/alpine/tags/list?n=5` →
`{"name":"library/alpine","tags":["2.6","2.7","20190228","20190408","20190508"]}` (paginated,
`Link` header for the next page); `GET /v2/library/alpine/referrers/<digest>` → `200` with
`application/vnd.oci.image.index.v1+json` (OCI 1.1 referrers API is live on Hub).

### 2.10 The trace, condensed

```
alpine
 └─ normalise (client-side convention)          docker.io/library/alpine:latest
 └─ endpoint remap (client-side convention)      https://registry-1.docker.io
 [1] GET  /v2/                                   401 + WWW-Authenticate: Bearer realm=…,service=…      (optional)
 [2] GET  /v2/library/alpine/manifests/latest    401 + …,scope="repository:library/alpine:pull"
 [3] GET  {realm}?service=…&scope=…              200 {"token":"eyJ…","expires_in":300}
 [4] GET  /v2/library/alpine/manifests/latest    200 Content-Type: …oci.image.index.v1+json
          Accept: <5 media types>                Docker-Content-Digest: sha256:28bd5fe8…
          → verify sha256(body) == that digest; record tag→digest
 [5] select platform: os=linux, arch=amd64, drop architecture=="unknown"
 [6] GET  /v2/library/alpine/manifests/sha256:79ff19e9…   200 …oci.image.manifest.v1+json
          → verify sha256(body) == 79ff19e9…
 [7] GET  /v2/library/alpine/blobs/sha256:d529dd0c…       307 → CloudFront (presigned) → 200 (611 B)
          → verify sha256(body) == d529dd0c…  == the image ID
 [8] for each layer descriptor, in manifest order:
     GET  /v2/library/alpine/blobs/sha256:55afa1ec…       307 → CloudFront → 200 (3 846 391 B)
          → stream: sha256(compressed) must == 55afa1ec…
                    sha256(gunzip(stream)) must == config.rootfs.diff_ids[i]  (34884abb…)
                    tar-extract into a staging dir, applying whiteouts
 [9] commit: blobs by digest, layer by chain-id, tag → index digest
```

---

## 3. Media types and manifest versions

### 3.1 The four you must handle, and the two you can drop

| Media type | Role | Handle? |
|---|---|---|
| `application/vnd.oci.image.index.v1+json` | OCI multi-platform index | **required** |
| `application/vnd.oci.image.manifest.v1+json` | OCI single-platform manifest | **required** |
| `application/vnd.docker.distribution.manifest.list.v2+json` | Docker manifest list | **required** |
| `application/vnd.docker.distribution.manifest.v2+json` | Docker Schema 2 manifest | **required** |
| `application/vnd.docker.distribution.manifest.v1+json` / `+prettyjws` | Docker Schema 1 (signed) | **drop** |
| `application/vnd.docker.container.image.v1+json` | Docker image config blob | **required** (same schema as OCI config) |
| `application/vnd.oci.image.config.v1+json` | OCI image config blob | **required** |
| `application/vnd.docker.image.rootfs.diff.tar.gzip` | Docker layer (gzip tar) | **required** |
| `application/vnd.oci.image.layer.v1.tar+gzip` | OCI layer (gzip tar) | **required** |
| `application/vnd.oci.image.layer.v1.tar+zstd` | OCI layer (zstd tar) | recommended |
| `application/vnd.oci.image.layer.v1.tar` | OCI layer (uncompressed) | cheap, do it |
| `application/vnd.docker.image.rootfs.foreign.diff.tar.gzip` | Windows base-layer marker | **reject with a clear message** |

`[O]` HOST-A HIGH — this is not theoretical. On one ordinary production host, the
containerd content store holds **all four** manifest kinds simultaneously
(`python3 json.load` over each blob in
`/var/lib/containerd/io.containerd.content.v1.content/blobs/sha256/`):

```
0c0142c3cd69  application/vnd.oci.image.index.v1+json                        16 entries
1504d8b94cde  application/vnd.oci.image.manifest.v1+json                      8 layers
263fe7ee9dde  application/vnd.docker.distribution.manifest.list.v2+json       3 entries   <- certbot/certbot
28bd5fe8b56d  application/vnd.oci.image.index.v1+json                        16 entries   <- alpine
909abba74538  application/vnd.docker.distribution.manifest.v2+json           11 layers
79ff19e9084a  application/vnd.oci.image.manifest.v1+json                      1 layer
```

`[R]` HIGH — **What breaks if we support only OCI media types: roughly half of Docker Hub.**
The seven images on this host split 5 OCI / 2 Docker. Any image built by a Docker Engine
older than ~23.0, or pushed with `--provenance=false` by older buildx, or produced by a
non-BuildKit tool, is Schema 2. Supporting Docker Schema 2 is essentially free — see §3.2 —
so refusing it is a pure loss.

### 3.2 OCI vs Docker Schema 2 — the diff is smaller than the noise suggests

`[F]`/`[C]` HIGH — Structurally, `application/vnd.docker.distribution.manifest.v2+json` and
`application/vnd.oci.image.manifest.v1+json` are the **same document** with different
`mediaType` strings, and Docker's manifest list is the same document as an OCI index. The
real deltas:

1. `[C]` The Docker manifest **requires** the top-level `mediaType` field; OCI made it
   OPTIONAL (and originally omitted it). So a parser must tolerate its absence.
2. `[F]` OCI added `subject` (referrers), `artifactType`, and the
   `application/vnd.oci.empty.v1+json` empty-descriptor convention (`sha256:44136fa3…`,
   the digest of `{}`). Not used by runnable images.
3. `[C]` Docker's layer media type is `application/vnd.docker.image.rootfs.diff.tar.gzip`;
   OCI's is `application/vnd.oci.image.layer.v1.tar+gzip`. Byte-for-byte the same format.
4. `[C]` Docker's config media type is
   `application/vnd.docker.container.image.v1+json`; OCI's is
   `application/vnd.oci.image.config.v1+json`. **The JSON schemas are compatible** — OCI's
   config spec was derived from Docker's. Docker's has extra fields
   (`container`, `container_config`, `docker_version`, `Healthcheck`, `OnBuild`) that OCI
   does not define.
5. `[C]` Docker's index entries may carry `platform.os.version` /
   `platform.os.features` for Windows.

`[R]` HIGH — **Deserialise both into one internal type.** In Rust: `#[serde(untagged)]`
is a trap here (poor error messages, silently picks the wrong arm); instead deserialise into
a permissive struct with `mediaType: Option<String>`, `manifests: Option<Vec<Descriptor>>`,
`config: Option<Descriptor>`, `layers: Option<Vec<Descriptor>>`, `fsLayers: Option<…>`,
then classify with an explicit function. Reason: this makes the "Docker Hub ignored my
Accept header" case (§2.4) a non-event. Tradeoff: `Option` soup at the parse boundary,
converted once into a clean enum.

`[R]` HIGH — Accept *unknown* config and layer media types with a suffix match: treat
anything ending `+gzip` as gzip, `+zstd` as zstd, and a bare `…tar` / `…diff.tar` as
uncompressed. Reason: it makes us forward-compatible with new registry-side conventions at
zero cost. Tradeoff: you lose the ability to reject a genuinely unsupported layer type
early — mitigated because the decompressor will fail loudly anyway.

`[R]` HIGH — Explicitly reject `…foreign.diff.tar.gzip` (Windows "foreign"/non-distributable
layers, which are not in the registry at all and carry `urls` in the descriptor) with
"Windows images are not supported". Reason: otherwise you get a 404 on a blob and an
inscrutable error.

### 3.3 Is Schema 1 dead?

`[C]` HIGH — **Yes, for our purposes.** Docker Hub stopped serving Schema 1 manifests in
November 2024 (Docker announcement "Docker Hub Registry: deprecating Docker Image
Manifest V2, Schema 1"); Docker Engine 27 removed the ability to *push* schema 1 and
Engine 28 removed schema-1 pull support entirely (moby release notes for v25–v28 track the
staged removal). containerd never supported pushing it and gates pulling it behind
`--allow-schema1`-style plumbing.

`[O]` MED — HOST-A: requesting `Accept: application/vnd.docker.distribution.manifest.v1+prettyjws`
alone for `library/alpine:latest` returned `application/vnd.oci.image.index.v1+json`. Hub
does not serve schema 1 even when it is the only acceptable type. This is one repository on
one day, hence MED rather than HIGH; a stronger test would be an image last pushed in 2015
that was never rebuilt, which I did not find.

`[R]` HIGH — **Do not implement Schema 1.** Reasons: (a) it has a fundamentally different
shape (`fsLayers[]` of blobSums in *reverse* order, `history[]` of embedded stringified
JSON v1 configs, and a JWS signature block); (b) its digest is computed over a
*canonicalised* form after stripping the signature, which drags in a JWS/JCS implementation;
(c) it does not carry `diff_id`s, so the image ID has to be *reconstructed* by walking the
v1 history. Tradeoff: some ancient private registry image will fail. `[R]` detect
`fsLayers` in the parsed JSON and emit "Docker Image Manifest V2 Schema 1 is not supported;
re-push this image with a modern client" — a 5-line detection that saves a 500-line parser.

### 3.4 The `mediaType` inside vs the `Content-Type` outside

`[F]` MED — image-spec makes the in-document `mediaType` OPTIONAL for manifests and
indexes; distribution-spec says the `Content-Type` header "SHOULD match what the client
pushed". Neither is a MUST that the two agree.

`[R]` HIGH — Resolution order when classifying a fetched manifest: (1) in-document
`mediaType` if present and recognised; (2) `Content-Type` header if recognised;
(3) structural sniffing (`manifests` ⇒ index, `config`+`layers` ⇒ manifest,
`fsLayers` ⇒ schema 1, else error). Then, if (1) and (2) both existed and disagreed, log a
warning but proceed with (1). Reason: the document is what was hashed; the header is not.
Tradeoff: we tolerate mildly broken registries, which means we will not catch them for the
maintainers.

---

## 4. Layer format: tar, compression, `diff_id`, chain ID

### 4.1 A layer is a tar stream, and the tar dialect matters

`[F]` HIGH — image-spec `layer.md`: a layer is a "filesystem changeset" and "Layer
Changesets … are _applied_, rather than simply extracted as tar archives". The archive
format is tar; the spec references the GNU Basic Tar Format for the type flags (notably
type `1` = hardlink) and permits PAX extended headers.

`[O]` HOST-A HIGH — `tar -tzvf` on the real `alpine` layer blob
(`sha256:55afa1ec…`): 515 entries, 8 697 856 uncompressed bytes; entries are dirs,
regular files, and a great many symlinks (`bin/ash -> /bin/busybox`); **zero** setuid/setgid
entries, **zero** device or char nodes, **zero** PAX/GNU-LongLink/sparse markers. So the
canonical smoke-test image exercises almost none of the hazards in §6 — which is precisely
why "it works on alpine" is not evidence of a safe extractor.

`[R]` HIGH — Build a hostile-layer fixture set *before* writing the extractor: a tar with
`../../etc/passwd`, an absolute `/etc/passwd`, a symlink `x -> /etc` followed by `x/passwd`,
a hardlink to `/etc/shadow`, a `c 1 3` device node, a `4755` root-owned binary, a
`security.capability` xattr, a 4 GiB-of-zeros gzip bomb, a 5000-byte path, and a PAX
`path=` override that disagrees with the ustar `name` field. Reason: these are the tests
that matter and they cannot be obtained from Docker Hub. Tradeoff: a fixture generator to
maintain; it is ~100 lines of Rust or Python and it is the highest-value test asset in the
project.

### 4.2 Compression: `+gzip`, `+zstd`, and none

`[F]` HIGH — image-spec `layer.md` registers three layer media types: `…layer.v1.tar`,
`…layer.v1.tar+gzip`, `…layer.v1.tar+zstd`. zstd was added in image-spec v1.1.

`[C]` HIGH — gzip is what you will actually meet. `[O]` HOST-A — every layer on the host
is `…tar+gzip` or `…rootfs.diff.tar.gzip`; no zstd layer present.

`[O]` HOST-A HIGH — `xxd -l 16` on the alpine layer blob:
`1f8b 0800 0000 0000 00ff …` — magic `1f8b`, method `08` (deflate), FLG `00`,
**MTIME `00000000`**, XFL `00`, OS `ff` (unknown). BuildKit zeroes the gzip header for
reproducibility. `[F]` HIGH — this matters because *the compressed digest depends on the
compressor*: MTIME, XFL, OS, and the deflate implementation's choices all land in the hash.

`[R]` HIGH — **Never recompress a layer.** Store and forward the compressed bytes exactly
as received. Reason: re-gzipping the same tar produces a different blob digest, which breaks
the manifest, breaks push, and breaks any cross-registry copy. This is the single most
common way a homegrown tool corrupts an image. Tradeoff: you cannot "optimise" storage by
re-compressing; if you want that, you must rewrite the manifest and config too, i.e. you
have built a new image.

`[R]` MED — Support zstd decompression from day one (it is one crate), but never *produce*
zstd layers. Reason: reading is cheap insurance against a registry serving zstd; writing
would strand our images on clients that only handle gzip (Docker only gained zstd pull
support relatively recently and older tooling will choke). Tradeoff: a dependency we may
not exercise.

### 4.3 The two digests, restated precisely

`[F]` HIGH — Two different hashes of the same layer, both normative, both required:

| | computed over | lives in | purpose |
|---|---|---|---|
| **blob digest** | the layer bytes **exactly as transferred** (i.e. the *compressed* stream for `+gzip`/`+zstd`) | `manifest.layers[i].digest`; the registry URL `/v2/<name>/blobs/<digest>`; the CAS filename | addresses the blob; verifies the transfer |
| **`diff_id`** | the **uncompressed** tar stream | `config.rootfs.diff_ids[i]` | identifies the layer's *content* independent of compression; the input to chain IDs |

`[F]` HIGH — image-spec `config.md`: `diff_ids` is "An array of layer content hashes
(`DiffIDs`), in order from first to last", and "Layer DiffID is the digest over the layer's
uncompressed tar archive". For an uncompressed layer (`…layer.v1.tar`) the two digests are
identical.

`[O]` HOST-A HIGH — verified for `alpine` (§2.8): `55afa1ec…` compressed / `34884abb…`
uncompressed, and `34884abb…` is exactly `config.rootfs.diff_ids[0]` and exactly what
`docker image inspect` reports as `RootFS.Layers[0]`.

`[F]` HIGH — `manifest.layers` and `config.rootfs.diff_ids` are **parallel arrays**: index
`i` in one corresponds to index `i` in the other, both base-layer-first. The spec never says
this in one sentence, but it follows from `manifest.md` ("The array MUST have the base layer
at index 0") plus `config.md` ("in order from first to last"). `[R]` HIGH — validate
`manifest.layers.len() == config.rootfs.diff_ids.len()` and refuse the image otherwise; a
mismatch means a corrupt or maliciously assembled image, and any code that zips the two
arrays would silently mis-associate.

`[F]` HIGH — `config.history` is **not** parallel to `layers`. Entries with
`empty_layer: true` consume no layer. `[O]` HOST-A — the alpine config has 2 history
entries and 1 layer; the second (`CMD ["/bin/sh"]`) has `"empty_layer": true`. `[R]` HIGH —
to associate history with layers, walk `history` and advance a layer cursor only when
`empty_layer` is absent or false. Getting this wrong is what produces a wrong
`docker history`. `[O]` HOST-A — `docker history alpine:latest` shows the `CMD` row with
size `0B` and image `sha256:d529dd0c…` (the config digest) while the layer-bearing row shows
`<missing>` — Docker has no per-layer image ID any more, and `<missing>` is literally what
it prints.

### 4.4 Chain IDs — how a layer *stack* is identified

The problem: a layer's `diff_id` identifies its content, but the same content applied on top
of *different* lower stacks produces a different filesystem. So the extracted result cannot
be cached by `diff_id` alone.

`[C]` HIGH — Docker/containerd solve this with a **ChainID**, defined recursively:

```
ChainID(0) = DiffID(0)
ChainID(n) = sha256( ChainID(n-1) ++ " " ++ DiffID(n) )       # ASCII, with the "sha256:" prefixes, one space
```

Source: `moby/layer/layer.go` (`createChainIDFromParent`), and the identical definition in
containerd's `identity.ChainID` (`containerd/images` / `rootfs`).

`[O]` HOST-A HIGH — **verified against the live store.** A python script recomputed the
chain for `nginx:stable` (7 layers) from `docker image inspect` `RootFS.Layers`, then looked
each chain ID up as a directory name under
`/var/lib/docker/image/overlay2/layerdb/sha256/`:

```
 [0] diff=1bb35e8b4de1 chain=1bb35e8b4de1 layerdb=YES cache-id=e055058cc36d parent=-            size=74810649
 [1] diff=9e4dacf518d3 chain=e4c840641964 layerdb=YES cache-id=b4ff9fb85140 parent=1bb35e8b4de1 size=117646777
 [2] diff=13db846cdc4d chain=d01a6fa3ca63 layerdb=YES cache-id=2b330e7434e0 parent=e4c840641964 size=1620
 [3] diff=5b74311a80c3 chain=e7330d2920f3 layerdb=YES cache-id=f524f93e01d4 parent=d01a6fa3ca63 size=2125
 [4] diff=9edb9517e14d chain=8e2ca32d91c6 layerdb=YES cache-id=661940fba4b7 parent=e7330d2920f3 size=389
 [5] diff=1507b2a9e1e4 chain=8d60e1ce63d5 layerdb=YES cache-id=60830e9ef6d2 parent=8e2ca32d91c6 size=3018
 [6] diff=b22c35a89de1 chain=baf694a87a40 layerdb=YES cache-id=1a74691a5d04 parent=8d60e1ce63d5 size=4619
```

All 7 chain IDs exist as layerdb directories, and each directory's `parent` file contains
its predecessor's chain ID. So **the layerdb key is the ChainID and `parent` makes it a
linked list**. Confidence HIGH.

`[O]` HOST-A HIGH — the layerdb entry contents (from
`/var/lib/docker/image/overlay2/layerdb/sha256/04bd4da1…/`):

| file | content | meaning |
|---|---|---|
| `diff` | `sha256:7ac624c6…` | this layer's `diff_id` |
| `parent` | `sha256:d3b8c360…` | the parent **ChainID** |
| `cache-id` | `4ac3d39eb1b4…` (64 hex, random) | the graphdriver's opaque directory name under `/var/lib/docker/overlay2/` |
| `size` | `1016` | uncompressed size in bytes |
| `tar-split.json.gz` | gzipped JSON | metadata to *reassemble the original tar byte-for-byte* from the extracted directory |

`[C]` HIGH — **the `cache-id` indirection is deliberate**: the content-addressed layer
identity (ChainID) is decoupled from the storage-driver's directory name, so a graphdriver
can name directories however it likes and be swapped without renaming content.
`[R]` HIGH — copy this indirection. Reason: it is what lets us change the layout of the
extracted-layer store later (per §11's expensive-decision list) without rewriting the
content-addressed metadata. Tradeoff: one extra lookup per layer, and a second place that
can leak entries.

`[C]` MED — `tar-split.json.gz` exists so Docker can *push* a layer it only has in extracted
form and still reproduce the original blob digest. Source: `vbatts/tar-split`.
`[R]` HIGH — **do not implement tar-split.** Reason: it is only needed to push layers we
did not download, and we are pull-only for a long time. If we later want push, the cheap
alternative is to keep the compressed blob in the CAS. Tradeoff: keeping compressed blobs
costs roughly 40% extra disk for gzip'd layers; tar-split costs a subtle reimplementation of
tar byte-preservation. Disk is cheaper.

### 4.5 What to key the extracted-layer cache on

`[R]` HIGH — Key extracted layer directories by **ChainID**, not `diff_id`. Reason: with
OverlayFS the extracted directory for layer `n` contains only layer `n`'s changeset, so
`diff_id` would in fact suffice for overlay — but the moment you support a driver that
materialises full snapshots (btrfs, or a plain-copy fallback), the contents depend on the
whole stack, and you would have to change the key. Using ChainID from the start costs
nothing and is what both Docker and containerd do. Tradeoff: ChainID means a shared base
layer is only reused when the *prefix* matches, which for real images (all deriving from the
same base) it does.

`[C]` HIGH — The reuse this buys, concretely: `[O]` HOST-A —
`/var/lib/docker/overlay2/08aa670a…/lower` = `l/MJDIM47…:l/MF4T4YG…`, and those same two
`l/` links appear in the `lowerdir=` of a live container's mount. Multiple images and
containers share the identical two base-layer directories. 7 images × ~8 layers would be
~56 layer directories; the host has 68 layerdb entries and 107 overlay2 directories
(the excess being per-container rw and `-init` layers).

`[C]` HIGH — Docker keeps two extra maps to reuse *downloads*, not just extractions.
`[O]` HOST-A:
- `/var/lib/docker/image/overlay2/distribution/diffid-by-digest/sha256/<compressed digest>`
  → `sha256:<diff_id>`. Example: `af5a453aa24d…` → `sha256:04d39b09c5e6…`. This lets a pull
  see a layer descriptor it already has and skip the download entirely.
- `/var/lib/docker/image/overlay2/distribution/v2metadata-by-diffid/sha256/<diff_id>` →
  `[{"Digest":"sha256:c2739fd3…","SourceRepository":"ghcr.io/HOST-A/backend","HMAC":""}]`.
  The reverse map, used at *push* time for cross-repository blob mounts.

`[R]` HIGH — Implement `diffid-by-digest` (a compressed-digest → `diff_id` map). Reason: it
is the difference between "pulling `nginx:stable` after `nginx:latest` downloads 190 MB" and
"downloads 5 KB". It is a single key-value map. Tradeoff: it is a cache that can go stale if
the CAS is GC'd out from under it, so the lookup must verify the layer still exists before
trusting it. Skip `v2metadata-by-diffid` until we implement push.

---

## 5. Whiteouts: tar convention → OverlayFS representation

Two entirely different encodings of "this path is deleted", and translating between them is
the extractor's most under-appreciated job.

### 5.1 The Image Spec side (a tar convention)

`[F]` HIGH — image-spec `layer.md`, quoted:
- "A whiteout filename consists of the prefix `.wh.` plus the basename of the path to be
  deleted."
- "A whiteout file is an empty file with a special filename that signifies a path should be
  deleted."
- "A `.wh.` file, without a basename to delete, is invalid and implementations SHOULD return
  an error when encountering such an entry."
- "Once a whiteout is applied, the whiteout itself MUST also be hidden."
- "Whiteout files MUST only apply to resources in lower/parent layers."
- Opaque: "An opaque whiteout entry is a file with the name `.wh..wh..opq` indicating that
  all siblings are hidden in the lower layer" — it "hides _all_ children of the `bin/`
  including sub-directories and all descendants".
- Ordering: "When processing the second layer, `a/.wh..wh..opq` is applied first, before
  creating the new version of `a/b`, **regardless of the ordering**."
- "Implementations SHOULD generate layers using _explicit whiteout_ files, but MUST accept
  both."
- Consequence stated in the spec: "it is not possible to create a filesystem which has a
  file or directory with a name beginning with `.wh.`".

`[R]` HIGH — The "regardless of the ordering" clause forces a **two-pass extraction per
layer**: pass 1 collects and applies every `.wh.`/`.wh..wh..opq` entry in the archive; pass 2
extracts the real entries. Reason: a layer may legally place `a/.wh..wh..opq` *after*
`a/b` in the tar stream, and a single-pass extractor would then delete the file it just
wrote. Tradeoff: you must either buffer entry metadata or read the archive twice (cheap for
a local blob, impossible while streaming from the network). Practical resolution: stream
once, but *defer* nothing — instead, when you meet an opaque marker for directory `D`,
record `D` in an "opaque set" and, at layer end, remove from the upper dir any entry under
`D` that came from a **lower** layer only. `[R]` MED — for the OverlayFS backend this is
much simpler (see §5.3): you set the opaque xattr and the kernel does it.

`[F]` HIGH — Also from `layer.md`, the "apply, don't extract" rules that bite:
- "If the entry and the existing path are both directories, then the existing path's
  attributes MUST be replaced by those of the entry in the changeset."
- Otherwise the implementation "MUST do the semantic equivalent of … removing the file path
  … recreating the file path". So a file replacing a directory means **`rm -rf` the
  directory first**, not "merge".
- Hardlinks are tar type `1`; "Extracting a layer with hardlink references to files outside
  of the layer may fail" — see §6.4 for why that is also a security boundary.

### 5.2 The OverlayFS side (a filesystem representation)

`[F]` HIGH — `Documentation/filesystems/overlayfs.rst`, quoted: "A whiteout is created as a
character device with 0/0 device number or as a zero-size regular file with the xattr
`trusted.overlay.whiteout`." Opaque directories: created by "setting the xattr
`trusted.overlay.opaque` to `'y'`". With `-o userxattr` the prefix becomes `user.overlay.`.

`[F]` HIGH — Same doc, the nested case: "This form is a regular, zero-size file with the
`overlay.whiteout` xattr set, inside a directory with the `overlay.opaque` xattr set to
`x`" — i.e. `opaque=x` (not `y`) marks a "whiteout base" directory used when overlay is
stacked on overlay.

### 5.3 The translation table

| Tar entry in a layer | What the extractor must produce in the layer's upper/diff directory |
|---|---|
| `path/.wh.name` (regular file, size 0) | `mknod path/name c 0 0`, and **do not** create `.wh.name` |
| `path/.wh..wh..opq` | `setxattr(path, "trusted.overlay.opaque", "y")` (or `user.overlay.opaque` under `userxattr`), and do not create the file |
| `.wh.` with empty basename | error out (spec SHOULD) |
| any other entry | extract normally |

`[R]` HIGH — Do the translation **at extraction time**, once per layer, into the layer's own
diff directory. Reason: the diff directory then *is* a valid overlay lower/upper layer with
no post-processing, and the kernel enforces the whiteout semantics for free — including the
"apply opaque before siblings regardless of ordering" rule, since the xattr is a property of
the directory rather than a stream event. Tradeoff: the extracted diff directory is now
overlay-specific, not a portable directory tree; a plain-copy backend would need the
`.wh.`-aware merge logic instead. This is one of the "expensive to change later" decisions
flagged in §11.

### 5.4 Does the translation require privilege? — measured, and the answer is *mostly no*

This is the pivotal rootless question, and the two halves of the translation have different
answers.

**Whiteout device: no privilege needed.**

`[O]` HOST-A 6.11.0-29 HIGH — as uid 65534 via
`setpriv --reuid=65534 --regid=65534 --clear-groups sh -c 'mknod wh_c00 c 0 0; mknod wh_c11 c 1 1'`
in a world-writable `/tmp` dir:
```
UNPRIV_MKNOD_C00_OK
mknod: wh_c11: Operation not permitted
```
`[O]` ROCKY 4.18.0-425 HIGH — identical result as the ordinary unprivileged login user
(uid 900000000): `c 0 0` succeeds, `c 1 1` gives EPERM.

`[F]` MED — The mechanism is a `CAP_MKNOD` exemption for the whiteout device specifically:
`fs/namei.c:vfs_mknod()` computes `bool is_whiteout = S_ISCHR(mode) && dev == WHITEOUT_DEV;`
and only demands `capable(CAP_MKNOD)` for char/block nodes that are **not** whiteouts. I did
not read the 4.18-RHEL and 6.11-Ubuntu trees directly, so the *mechanism* is MED while the
*behaviour* above is HIGH `[O]`. What would settle it: `grep -n is_whiteout fs/namei.c` on
each kernel's source.

`[R]` HIGH — This is a genuine and load-bearing win: **a rootless extractor can create
overlay whiteouts natively.** Do not reach for `fuse-overlayfs` merely because of whiteouts.

**Opaque xattr: privilege needed, unless the mount uses `userxattr`.**

`[O]` HOST-A + ROCKY HIGH — `setfattr -n trusted.overlay.opaque -v y <dir>` →
`Operation not permitted`, both as an ordinary unprivileged user and as uid-0-inside-a-user-namespace.
`setfattr -n user.overlay.opaque -v y <dir>` → succeeds in both cases.
`[F]` HIGH — `trusted.*` requires `CAP_SYS_ADMIN` in the **initial** user namespace
(`xattr(7)`: "Trusted extended attributes are visible and accessible only to processes that
have the CAP_SYS_ADMIN capability"), which a user namespace can never grant.

### 5.5 `userxattr` — the empirical finding that decides the rootless design

`[O]` HOST-A 6.11.0-29 HIGH — Mounting overlay **inside** `unshare -Urm`, *without* passing
`userxattr`:
```
$ grep " .../mrg " /proc/self/mounts
overlay .../mrg overlay rw,relatime,lowerdir=.../lo,upperdir=.../up,workdir=.../wk,\
redirect_dir=nofollow,uuid=on,userxattr 0 0
```
and, inspected from **outside** the namespace with `os.listxattr`, the upper dir carries
`user.overlay.uuid` and `user.overlay.impure` — the `user.overlay.` namespace, not
`trusted.overlay.`. **Kernel 6.11 turns `userxattr` on by itself for a user-namespace
mount.** `rm -rf` of a lower directory through that mount succeeded.

`[O]` ROCKY 4.18.0-425 HIGH — the same mount, same kernel-visible situation, reports
**no** `userxattr` in `/proc/self/mounts`, writes **no** `user.overlay.*` xattrs, and
`rm -rf` of a lower directory fails:
```
rm: cannot remove '.../mrg/d': Input/output error
```
Passing `,userxattr` explicitly on 4.18 fixes it: `rm -rf` succeeds, the upper dir gets
`user.overlay.impure="y"`, and the removed directory becomes a `c 0 0` whiteout.

`[F]` MED — I could not find the auto-enable in `fs/overlayfs/params.c` or `super.c`; the
only `userxattr` reference I located in `super.c` is the hint
`if (err == -EPERM && !ofs->config.userxattr) pr_info("try mounting with 'userxattr' option\n");`,
and the mainline `overlayfs.rst` still describes `userxattr` as something the admin passes.
So the *behaviour* on 6.11 is `[O]` HIGH but the *mechanism and the kernel version at which
it started* is **LOW**. What would settle it: `git log -S userxattr fs/overlayfs/` on
mainline, or bisecting the behaviour across kernels.

`[R]` HIGH — **Always pass `userxattr` explicitly when we are not CAP_SYS_ADMIN-in-init-userns,
and never pass it when we are.** Reasoning: on 6.11 it is a no-op (already on); on 4.18-class
kernels it is the difference between working and `EIO` on any `rm -rf` of an image directory;
and passing it when privileged would make our layers unreadable by Docker/containerd, which
write `trusted.overlay.*`. Tradeoff: the extracted layer store becomes
**privilege-mode-specific** — a layer extracted rootless carries `user.overlay.opaque`
markers that a root-mode overlay mount will ignore, and vice versa.

`[R]` HIGH — Therefore: **partition the layer cache by xattr mode.** Put rootless-extracted
layers under a different store root (e.g. `layers/userxattr/<chainid>` vs
`layers/trusted/<chainid>`) rather than sharing one directory. Reason: the alternative —
silently ignoring an opaque marker written in the other namespace — is a wrong-filesystem
bug that presents as "a file I deleted in the Dockerfile is back". Tradeoff: rootless and
root modes cannot share extracted layers, doubling disk when both are used. Accept it; it is
also what `containers/storage` effectively does by giving rootless a separate graph root
under `$XDG_DATA_HOME`.

`[F]` HIGH — `overlayfs.rst` "Permission model": "task creating the overlay mount MUST NOT
gain additional privileges", and "In the old mount api the credentials of the task calling
`mount(2)` are stashed. In the new mount api the credentials of the task creating the
superblock through `FSCONFIG_CMD_CREATE` … are stashed." `[R]` MED — relevant to D1: the
process that performs the overlay mount is the one whose credentials are checked for all
later access to the lower layers, so we must not mount the overlay from a more-privileged
helper and then hand it to a less-privileged container expecting the kernel to re-check.

### 5.6 The cost of the translation

`[R]` MED — Cheap in CPU, awkward in ordering. Per whiteout entry it is one `mknodat` or one
`fsetxattr`, versus one `openat`+`write` for a real file — so the translation is *faster*
than extracting the file it replaces. The real cost is that it forces the extractor to know
its target representation, i.e. the extractor cannot be a generic "untar into a directory"
function. `[R]` HIGH — express this as a trait with one method per filesystem event
(`create_file`, `create_dir`, `symlink`, `hardlink`, `whiteout`, `set_opaque`, `set_meta`)
and two implementations (overlay-native, plain-tree). Reason: it keeps the tar parsing and
the hazard checks (§6) in one place while allowing a non-overlay backend. Tradeoff: a trait
with two impls is exactly the kind of abstraction worth resisting — but here the second impl
is required for the §11 ladder's first rung, so it is not speculative.

---

## 6. Extraction hazards — the most dangerous parsing code we will own

Threat model, stated plainly: **the layer tarball is attacker-controlled input, processed by
a root-privileged process, whose output is a filesystem tree that will later be the root of
a container.** Every hazard below has produced a real CVE.

Two mitigations recur and are worth stating once:

`[R]` HIGH — **M1: extract through a pinned directory fd using `openat2(RESOLVE_IN_ROOT)`.**
Open the layer's destination directory once, keep the fd, and perform every subsequent
operation with `*at()` syscalls relative to it, with `RESOLVE_IN_ROOT` so the kernel treats
that fd as `/` for symlink and `..` resolution. Reason: this is the only mitigation that is
not a race — it moves the containment decision into the kernel's path walk instead of into
our own string checks. Tradeoff: `openat2` needs Linux **5.6+**, so we need a fallback
(lexical normalisation + `O_NOFOLLOW` on every component + re-`openat` per component) for
the ROCKY-class 4.18 target. `[F]` HIGH — `openat2(2)`: `RESOLVE_IN_ROOT` means "treat the
directory referred to by `dirfd` as the root directory… absolute symlinks are interpreted
relative to `dirfd`"; `RESOLVE_BENEATH` and `RESOLVE_NO_SYMLINKS` are the stricter siblings.

`[R]` HIGH — **M2: extract into a fresh empty staging directory, never into a populated
tree, and never into a shared root.** Reason: most escapes need something pre-existing to
aim at; an empty per-layer diff directory removes half the attack surface and makes cleanup
an `rm -rf`. Tradeoff: none for the overlay design, which wants per-layer diffs anyway.

### 6.1 `..` traversal and absolute paths

`[F]` HIGH — tar stores paths as opaque bytes; nothing prevents `../../etc/shadow` or
`/etc/shadow`. This is "Zip Slip"/"Tar Slip", the oldest archive bug there is.

CVEs: **CVE-2014-6407** (Docker ≤1.3.1: "Docker image extraction vulnerability" — path
traversal and hardlink/symlink escape in the archive extractor);
**CVE-2014-9356** (Docker: absolute-symlink path traversal in archive extraction).

Mitigation: `[R]` HIGH — strip a leading `/`, then lexically reject any component equal to
`..` **before** any filesystem call, then rely on M1. Do all three; the lexical check gives
a good error message and the kernel check gives the guarantee.
`[C]` HIGH — containerd normalises with `filepath.Clean`/`Join` and additionally uses
`securejoin` (`cyphar/filepath-securejoin`) in its `archive` package;
`containers/image`/`containers/storage` route every extraction path through
`securejoin.SecureJoin`.

### 6.2 Symlink-target escape, and the TOCTOU variant

`[F]` HIGH — The dangerous case is not the symlink itself but a *later* entry that resolves
through it: layer contains `escape -> /` (or `-> /etc`), then `escape/passwd`. A naive
extractor opens `<root>/escape/passwd`, the kernel follows the symlink, and you have written
to `/etc/passwd` on the host.

CVEs: **RUSTSEC-2026-0148 / boxlite < 0.9.0, CVSS 10.0 CRITICAL** — "Affected versions of
`boxlite` extract OCI image layer tarballs without fully containing path resolution to the
extraction root"; a layer with `escape -> /tmp` followed by entries resolving through it
writes outside the rootfs. **This is a Rust OCI-image extractor with exactly the bug we are
at risk of, disclosed in 2026, fixed by introducing a `SafeRoot` handle that routes all
filesystem operations through `openat2(RESOLVE_IN_ROOT)`.** Read that fix before writing
ours.
**CVE-2021-32760** (containerd ≤1.4.7 / 1.5.0–1.5.3): "Archive package allows chmod of file
outside of unpack target directory" — "pulling and extracting a specially-crafted container
image can result in Unix file permission changes for existing files in the host's
filesystem… can … set extended bits like setuid, setgid, and sticky". Fixed in 1.5.4/1.4.8.
**CVE-2018-15664** (Docker `cp`): the classic **TOCTOU** — the path is resolved, then a
concurrent process swaps a directory component for a symlink before the operation completes.
**CVE-2022-23648** (containerd CRI): a symlink inside an image's declared `Volumes` let a
container read arbitrary host files.
**RUSTSEC-2026-0067** (Rust `tar` ≤ 0.4.44): "`unpack_in` can chmod arbitrary directories by
following symlinks" — `unpack_dir` used `fs::metadata()` (which follows symlinks) to decide
whether an existing path was a directory, so a symlink entry followed by a same-named
directory entry caused a `chmod` of the symlink's target. Fixed in 0.4.45.

Mitigation: `[R]` HIGH — M1 is the answer, because it is the only one that closes the TOCTOU.
Lexical checking cannot: between "I verified this path is inside the root" and "I opened it"
an adversary — including *another entry of the same tarball* — can change the answer.
Additional rules: never follow a symlink when *replacing* an existing entry (use
`O_NOFOLLOW`/`AT_SYMLINK_NOFOLLOW` and `unlinkat` first, per `layer.md`'s "remove then
recreate"); never `chmod`/`chown`/`utimes` a path you did not just create through the pinned
root; and set the mode at `openat`/`mkdirat` time rather than with a follow-up `chmod`.

### 6.3 Device nodes in layers

`[F]` HIGH — tar types `3` (char) and `4` (block) carry `devmajor`/`devminor`. A layer
containing `dev/mem` as `c 1 1`, or `dev/sda` as `b 8 0`, extracted as root, creates a real
device node inside the future container rootfs. If the container is not blocked by
`devices` cgroup / seccomp / LSM, that is a direct read-write path to host memory or disk.

Mitigation: `[R]` HIGH — **refuse to create device nodes from layer content, always.**
Whitelist exactly one: char `0/0`, and only as the whiteout translation of §5. Reason: no
legitimate image needs a device node in a layer — the runtime creates `/dev` entries from
the runtime `config.json` (`linux.devices`), and Docker/containerd bind-mount or create
`/dev` themselves. Tradeoff: a handful of ancient images that ship `/dev/null` in a layer
will produce a warning instead of a node; that is the correct outcome. `[C]` HIGH — this is
also what a rootless extractor is *forced* into: `[O]` ROCKY/HOST-A — `mknod c 1 1` as an
unprivileged user returns `EPERM` (§5.4), so rootless extraction cannot create them anyway.
`[R]` MED — record the skipped node in the layer's metadata so `--strict` mode can fail
instead of warn.

### 6.4 Hardlink escape

`[F]` HIGH — tar type `1` names a *link target inside the archive*. `layer.md`:
"Hardlinked files are on a same device… and have the same inode", and "Extracting a layer
with hardlink references to files outside of the layer may fail". The attack: a layer
contains `link` as a hardlink whose `linkname` is `../../etc/shadow` (or `/etc/shadow`).
A `linkat` creates a second name for the *host's* inode inside the rootfs — bypassing the
"no symlink escape" defence entirely, because a hardlink is not a symlink and there is
nothing to "resolve out of". Worse, the container can then modify the host file through it,
and the link survives even if the original path's permissions change.

CVE: **CVE-2014-6407** covers this class in Docker's extractor.

Mitigation: `[R]` HIGH — resolve `linkname` through the *same* pinned root (M1) and require
the target to already exist **within this layer's staging directory**; refuse if it
resolves outside, and refuse absolute link targets outright. `[R]` MED — a hardlink whose
target is in a *lower* layer cannot be honoured at all under OverlayFS (the target is in a
different directory tree); the correct behaviour is to copy the target's content into the
layer, which is what `layer.md`'s "may fail" is hinting at. Tradeoff: copying breaks the
hardlink's shared-inode semantics and inflates the layer; failing breaks real images. Docker
copies. `[R]` copy, and warn.

### 6.5 setuid / setgid / sticky bits

`[F]` HIGH — tar mode bits include `04000`/`02000`. A layer legitimately contains setuid
binaries (`/bin/su`, `/usr/bin/sudo`, `ping`). Extracted as root, they are real setuid-root
binaries in the container rootfs.

`[O]` HOST-A/ROCKY HIGH — `chmod 4755` on a file **you own** succeeds unprivileged
(`UNPRIV_SETUID_OK mode=4755`). So the setuid bit is not itself privileged; what makes it
dangerous is the *owner*, which unprivileged extraction cannot set (§6.6).

Mitigation: `[R]` HIGH — preserve the bits by default (removing them breaks images), but
(a) never let a setuid bit land on a file outside the layer root — covered by M1 and by
"set mode at create time, never `chmod` a pre-existing path" (this is exactly
CVE-2021-32760's mechanism), and (b) offer `--no-new-privileges`-style layer flattening only
as an explicit opt-in. `[R]` HIGH — the real defence lives in D5's domain
(`NoNewPrivileges` in the runtime `config.json`, and dropping `CAP_SETUID`/bounding set), not
in the extractor. Say so and hand it over.

### 6.6 uid/gid ownership, root vs unprivileged

`[F]` HIGH — tar carries numeric `uid`/`gid` (and PAX `uname`/`gname`, which must be
ignored — name lookup at extraction time is a host-identity leak). Layers routinely contain
files owned by uid 0 and by service accounts (uid 100, 101, 999…).

`[O]` ROCKY HIGH — as an ordinary user, `chown 1234 f` → `Operation not permitted`. Inside
`unshare -Ur` (single-UID map, `/etc/subuid` empty on this host), `chown 65534 f` →
**`Invalid argument` (EINVAL)**, because 65534 is not in the namespace's uid map. So
rootless extraction on this host **cannot represent a multi-UID image at all**.

Mitigation: `[R]` HIGH — root mode: `fchownat(..., AT_SYMLINK_NOFOLLOW)` with the numeric
ids as given, done via the pinned root. Rootless mode: use the userns uid/gid map, which
requires `/etc/subuid`+`/etc/subgid` ranges and `newuidmap`/`newgidmap`; when no subuid range
exists, **detect it and fail with a specific message** rather than silently squashing every
file to the invoking uid. Reason: silent squashing produces images where `nobody`-owned
directories become writable by the container's root — a real privilege change, not a cosmetic
one. Tradeoff: we refuse to run on hosts like ROCKY. That is honest degradation, and D5's
brief already calls for it.
`[C]` HIGH — `AT_SYMLINK_NOFOLLOW` on the chown is not optional: without it, chowning a
symlink entry chowns its *target*, which is another escape (and is why `lchown` exists).

### 6.7 xattrs, and `security.capability` in particular

`[F]` HIGH — Layer tars carry xattrs in PAX records: OCI/Docker use
`SCHILY.xattr.<name>` (GNU/star convention, e.g.
`SCHILY.xattr.security.capability`); older Docker also wrote
`LIBARCHIVE.xattr.<urlencoded-name>`.

`[F]` HIGH — `security.capability` is a **file capability**: it grants a non-setuid binary
specific capabilities on exec. A layer containing `/usr/bin/evil` with
`security.capability = CAP_SYS_ADMIN` is a setuid-root binary in all but name.

`[O]` ROCKY HIGH — the empirical surprise: as a **plain unprivileged user**,
`setfattr -n security.capability` → `Operation not permitted`. But as **uid 0 inside a user
namespace**, it **succeeds** — and the kernel silently *rewrites the value*:
```
written inside userns : security.capability=0x0100000200002000000000000000000000000000   (v2)
read back outside     : security.capability=0x01000003000020000000000000000000000000004501802e   (v3 + rootid)
```
The trailing 4 bytes `45 01 80 2e` little-endian = `0x2e800145` = the host uid of the
namespace's root. `[F]` MED — this is `VFS_CAP_REVISION_3` namespaced file capabilities
(kernel 4.14+, "Introduce v3 namespaced file capabilities"): a v2 capability set written from
a user namespace is converted to v3 with a `rootid` field, and is only honoured for
processes whose namespace root maps to that uid. Mechanism MED (not read from source);
behaviour HIGH `[O]`.

`[R]` HIGH — Three consequences, and they are non-obvious:
1. **`security.capability` written rootless is not the same bytes as in the layer.** So the
   extracted tree cannot be re-hashed to reproduce a `diff_id`, and any "verify extraction by
   re-tarring" self-check must exclude xattrs.
2. **A rootless-extracted layer's file capabilities are bound to the extracting user's uid.**
   Move the store to another user, or map a different subuid range, and the capabilities go
   dead. `[R]` partition the store by mode (§5.5) covers this too.
3. `[R]` HIGH — Default to **dropping `security.*` xattrs entirely** unless
   `--preserve-capabilities` is given, and always drop `system.*`. Reason: the images that
   genuinely need file capabilities are few (`ping`, some Java installs), the blast radius of
   getting it wrong is a privilege escalation, and `security.selinux` written from a layer
   would let an image choose its own LSM label. Preserve `user.*` and `trusted.overlay.*`
   never (those are ours). Tradeoff: unprivileged `ping` breaks in some images; that is a
   documented, recoverable annoyance.

`[C]` HIGH — containerd's `archive` package has an explicit xattr allow/deny path and skips
`security.selinux` unless configured; `containers/image` applies its own filtering. Neither
preserves everything blindly.

### 6.8 Decompression amplification (gzip/zstd bombs)

CVEs: **CVE-2017-14992** (moby: "Gzip bomb / decompression DoS in `docker load`" — a tiny
tar.gz expands without bound). **CVE-2023-25153** (containerd ≤1.5.17, 1.6.0–1.6.17): "When
importing an OCI image, there was no limit on the number of bytes read for certain files. A
maliciously crafted image with a large file where a limit was not applied could cause a
denial of service." CWE-400; fixed in 1.6.18/1.5.18.

Mitigation: `[R]` HIGH — three independent limits, all cheap:
1. The layer descriptor's `size` is **REQUIRED** by the spec (§1.3). Read *at most* that many
   compressed bytes and error if the stream continues — this alone bounds the download.
2. A configured cap on **uncompressed** bytes per layer and per image (default: something
   like 10 GiB / 50 GiB), enforced by counting bytes out of the decompressor.
3. A cap on **entry count** per layer and on any single entry's size.
Reason: (1) is free and spec-blessed; without (2) a 3 MB blob can still be a 1 TB write.
Tradeoff: legitimate huge images (ML containers, 20 GB+) will hit the default and need a
flag. Make the limit configurable and the error message name the flag.
`[R]` MED — also cap the **manifest/config JSON** size (e.g. 4 MiB) before parsing. Reason:
that is the exact shape of CVE-2023-25153 — an unbounded read of a *metadata* file, not a
layer.

### 6.9 Long paths, and other name hazards

`[F]` HIGH — ustar caps `name` at 100 bytes and `prefix` at 155; longer paths require a PAX
`path=` record or GNU `L`/`@LongLink` entries. Linux caps a single component at `NAME_MAX`
(255 bytes) and a path at `PATH_MAX` (4096) for the *syscall interface* — but a path built by
repeated `mkdirat` under a pinned fd can exceed 4096 in total depth, which then becomes
undeletable by tools that build absolute paths. `[O]` ROCKY — I built a 3740-byte path with
`mkdir -p` successfully; the failure mode past that is `ENAMETOOLONG`.

Mitigation: `[R]` HIGH — reject any entry whose path exceeds a configured limit (4096) or
whose depth exceeds a limit (e.g. 256 components), and any component over 255 bytes, *before*
touching the filesystem. Reason: an unbounded-depth layer is a cleanup DoS (our own `rm -rf`
cannot delete it) as much as an extraction problem. Tradeoff: none realistic.

`[R]` HIGH — **PAX vs ustar disagreement is a security bug, not a curiosity.** If a PAX
extended header supplies `path=` and the following ustar header's `name` differs, an
extractor that validates one field and uses the other is exploitable. Rule: resolve the
effective path *first* (PAX `path` overrides `name`; PAX `linkpath` overrides `linkname`),
then validate, then use. Validate exactly the bytes you will pass to the kernel.

`[R]` MED — **Do not Unicode-normalise, do not case-fold, do not re-encode.** Treat tar
paths as opaque bytes (`OsStr`/`Vec<u8>`, never `String`). Reasons: (a) Linux paths are byte
strings and may be invalid UTF-8, so lossy conversion silently changes the target path;
(b) NFC/NFD normalisation makes two distinct legal paths collide, so a "check then write"
pair can disagree; (c) on a case-insensitive or normalising filesystem two entries can
collide, which is a real problem for macOS/APFS but not for our Linux-only target.
`[R]` HIGH — reject a path containing a NUL byte or a component that is exactly `.` or `..`,
and reject an entry name ending in `/` whose type is not directory.

### 6.10 Tar variant hazards

| Variant | Hazard | `[R]` |
|---|---|---|
| PAX extended headers (type `x`) | `path`/`linkpath`/`size`/`uid` overrides; a *global* header (type `g`) that silently applies to later entries | Apply per-entry `x` records; **ignore `g` records entirely** — no layer needs them and they are a confusion vector |
| GNU LongName/LongLink (`L`/`K`) | same override problem, plus an unbounded name length | Support, with the §6.9 length caps |
| GNU sparse (`S`, and PAX `GNU.sparse.*`) | a small archive describing a huge file; also two different encodings (0.0/0.1/1.0) | Support decoding but count *materialised* bytes against the §6.8 cap; a sparse file whose holes are not written is fine, one that expands to 1 TB is not |
| tar type `7` (contiguous) | rare; treat as regular | map to regular |
| tar types `A`–`Z` (vendor) | unknown semantics | **reject** with the type byte in the message |
| Trailing garbage after the two zero blocks | data smuggling; also breaks digest expectations | read to EOF for the digest, but stop *extracting* at the end-of-archive marker |
| Multiple entries for the same path | last-wins vs first-wins ambiguity; a validated entry can be replaced by an unvalidated one | Apply in stream order with `layer.md`'s remove-then-recreate rule; validate each independently |

`[F]` HIGH — **CVE-2021-41190** is the manifest-level analogue and worth internalising:
"the manifest and index documents were not self-describing and documents with a single
digest could be interpreted as either a manifest or an index… If a Content-Type header
changed between two pulls of the same digest, a client may interpret the resulting content
differently." Fixed by *recommending* `mediaType` in the document and *requiring* that it
match `Content-Type`. `[R]` HIGH — this is why §3.4's classification order puts the
in-document `mediaType` first: the bytes we hashed must decide what the bytes are. A
document containing **both** `manifests` and `layers` must be **rejected**, not disambiguated.

### 6.11 How the mature implementations do it — the short version

`[C]` HIGH — containerd (`archive/tar.go`): lexical `filepath.Clean`, `securejoin` for
symlink containment, an explicit `whiteout` handling pass, xattr filtering, and — after
CVE-2021-32760 — deferred `chmod` applied only to paths verified inside the root.
`[C]` HIGH — `containers/image` + `containers/storage`: all paths through
`cyphar/filepath-securejoin`; separate rootless graph root; `idtools` for uid/gid shifting;
explicit device-node and xattr policy.
`[C]` HIGH — `cyphar/filepath-securejoin` and its Rust sibling `openat2`/`pathrs` are the
canonical implementations of "resolve this path but keep it under that root", by the author
of the runc security fixes. `[R]` HIGH — **read `pathrs`/`openat2` before writing our own
path resolver**; the lexical-only approach is known-insufficient and the library exists
precisely because everyone got it wrong.

---

## 7. Content-addressable storage, image layout, GC and interrupted pulls

### 7.1 The OCI image layout (the only *specified* on-disk format)

`[F]` HIGH — image-spec `image-layout.md`:
```
./oci-layout                 {"imageLayoutVersion": "1.0.0"}          REQUIRED
./index.json                 an image index; the entry point            REQUIRED
./blobs/<algorithm>/<encoded>  e.g. blobs/sha256/<64 hex>              REQUIRED (dir may be empty)
```
- `blobs/<algorithm>/` mirrors the digest's two parts; the filename is the **encoded** part
  only (no `sha256:` prefix), and the file content's digest MUST equal the path.
- `index.json` is a full image index; tags are expressed as the
  `org.opencontainers.image.ref.name` annotation on its entries.
- Everything in the layout — manifests, indexes, configs, layers — is *just a blob*. There is
  no separate "manifest directory".

`[R]` HIGH — Adopt the OCI image layout **verbatim** as our blob store, at
`<root>/blobs/sha256/<hex>`. Reasons: (a) it is the one thing in this domain that is
specified, so it is the one thing we cannot get wrong by inventing; (b) `skopeo copy
docker://alpine oci:/path/to/our/store:alpine` produces exactly this, giving us a zero-code
import path and a zero-code test oracle; (c) `oci-layout` + `index.json` means our store is
directly consumable by every other tool. Tradeoff: a flat `blobs/sha256/` directory with
tens of thousands of entries is a mild directory-scaling annoyance on ext4 (mitigated by
`dir_index`, which is default). Docker's registry shards by the first two hex chars
(`blobs/sha256/d5/d529dd0c…/data` — visible in the CloudFront URL in §2.7); the OCI layout
does **not** shard, and we should not either.

### 7.2 What containerd does, and why

`[C]` HIGH — containerd splits the problem in two, and the split is the good idea:

| Component | Path on HOST-A | Holds | Keyed by |
|---|---|---|---|
| **content store** | `/var/lib/containerd/io.containerd.content.v1.content/blobs/sha256/<hex>` | every blob as transferred (manifests, indexes, configs, compressed layers) | **blob digest** |
| **ingest** | `…/io.containerd.content.v1.content/ingest/<ref>/{data,ref,startedat,updatedat}` | in-flight, partially-written blobs | an opaque *ref* string |
| **snapshotter** | `/var/lib/containerd/io.containerd.snapshotter.v1.overlayfs/snapshots/<N>` | extracted layers and container rw layers | small integer, mapped from a chain-id "key" in bolt |
| **metadata** | `/var/lib/containerd/io.containerd.metadata.v1.bolt/meta.db` | image→manifest, snapshot keys, leases, labels, refcounts | bolt buckets |

`[O]` HOST-A HIGH — verified: 12 blobs in the content store (media types listed in §3.1),
an **empty** `ingest/` directory (no interrupted pulls), 4 snapshotter roots present
(`overlayfs`, `native`, `btrfs`, `blockfile`) with 0 snapshots each (this host's Docker uses
the classic graphdriver, not the containerd snapshotter), and a 524 288-byte `meta.db`.
`[O]` HOST-A — BuildKit keeps its **own** content store in the identical layout at
`/var/lib/docker/buildkit/content/blobs/sha256/<hex>` plus `content/ingest/`. Two
independent components on one host both chose `blobs/sha256/<hex>` + `ingest/`.

`[C]` HIGH — Why the split matters: the content store is *immutable and content-addressed*;
the snapshotter is *mutable and stack-addressed*. Blob GC and layer GC are then independent
problems with independent lifetimes — you can drop a compressed layer blob while keeping its
extracted snapshot, or vice versa.

`[R]` HIGH — Copy the split. Three stores under one root:
```
<root>/blobs/sha256/<hex>            # OCI image layout, immutable, digest-addressed
<root>/ingest/<uuid>/{data,meta}     # partial downloads
<root>/layers/<mode>/<chainid>/      # extracted layer diffs (mode = trusted|userxattr, per §5.5)
<root>/containers/<id>/{upper,work,merged}
<root>/index.json + <root>/oci-layout  # tags and the entry point
<root>/meta/                         # tag→digest, chainid→layerdir, diffid-by-digest, refcounts
```
Tradeoff: four things to keep consistent instead of one. Mitigated by making only `meta/`
authoritative and everything else reconstructible.

### 7.3 Reference counting and GC

`[F]` HIGH — Nothing in any OCI spec says anything about GC. This is entirely our design.

`[C]` HIGH — containerd's model is **leases + labels + a mark-and-sweep GC**: a blob is
retained if some lease or a `containerd.io/gc.ref.*` label points at it; the GC walks roots
(images, leases, containers) and sweeps the rest. Docker's model is **explicit reference
counts** in the layer store plus `docker image prune`.

`[R]` HIGH — Use **mark-and-sweep from explicit roots**, not per-object refcounts. Roots =
(a) every entry in `index.json` (i.e. every tag), (b) every existing container. Sweep =
anything in `blobs/`, `layers/`, `ingest/` not reachable. Reasoning: refcounts require every
mutation to be transactional with the counter, and a crash between "write blob" and
"increment count" leaks or, worse, under-counts and deletes live data. Mark-and-sweep is
crash-safe by construction — an interrupted pull leaves garbage, and garbage is what the
sweeper is for. Tradeoff: GC is O(store size) rather than O(1) per delete, and you need a
lock (or a grace period) so the sweeper does not delete a blob a concurrent pull just wrote
but has not yet linked. `[R]` MED — solve that with a "young object" grace period
(don't sweep anything whose mtime is under N minutes) rather than a global lock; it is what
containerd's ingest expiry effectively does.

`[R]` HIGH — Dedup falls out for free at two levels: blobs are shared by digest across
images automatically; extracted layers are shared by chain-id. `[O]` HOST-A quantifies the
win — 7 images totalling ~1.5 GB of layers occupy 68 distinct layerdb entries, and multiple
images' mounts reference the *same* `l/` links (§4.5). No extra machinery is needed for this;
it is the consequence of content-addressing.

### 7.4 Tag → digest metadata

`[O]` HOST-A HIGH — Docker's is a single JSON file,
`/var/lib/docker/image/overlay2/repositories.json`:
```json
{"Repositories":{"alpine":{
  "alpine:latest":"sha256:d529dd0c6e5597ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b",
  "alpine@sha256:28bd5fe8b56d1bd048e5babf5b10710ebe0bae67db86916198a6eec434943f8b":
    "sha256:d529dd0c6e5597ac7e4a3e2dea65c3fcc6173f4cae713c409265c1dd9914a11b"}, …}}
```
Two things worth noticing: the values are **config digests (image IDs)**, not manifest
digests; and the `repo@sha256:<manifest digest>` form is stored as *another key* mapping to
the same image ID. So Docker's index is `(tag | manifest-digest) → image ID`.

`[R]` HIGH — Store tags in `index.json` as the OCI layout prescribes (annotation
`org.opencontainers.image.ref.name`), pointing at the **manifest or index digest**, and keep
a derived `(repo, digest) → config digest` map in `meta/` for `docker images`-style output.
Reason: `index.json` is the interoperable artefact; deriving the image ID is a one-line
lookup. Tradeoff: two places, but only one is authoritative.

`[R]` HIGH — Record *which* digest a tag resolved to at pull time — the **index** digest
(`28bd5fe8…`), not the platform manifest digest. Reason: that is what `RepoDigests` reports
and what `image@sha256:…` must resolve to; recording the platform manifest instead makes
`docker pull alpine@sha256:<what we printed>` fail on another architecture.

### 7.5 Interrupted pulls: partial blobs, atomic rename, fsync discipline

`[R]` HIGH — The invariant to defend: **a file under `blobs/sha256/<hex>` always has content
whose digest is `<hex>`.** Everything else follows.

Procedure per blob:
1. `open(ingest/<uuid>/data, O_CREAT|O_EXCL|O_WRONLY)` — a fresh, unguessable name per
   attempt, never a name derived from the digest. Reason: two concurrent pulls of the same
   blob must not share a partial file.
2. Stream, hashing as you write (both hashes for a layer, §2.8).
3. On completion, compare digests. Mismatch ⇒ `unlink` and fail, naming both digests.
4. `fsync(data_fd)`, then `close`.
5. `linkat`/`renameat(ingest/<uuid>/data → blobs/sha256/<hex>)`.
6. `fsync` the `blobs/sha256/` **directory** fd.

`[F]` HIGH — Steps 4–6 are the necessary and sufficient discipline: `rename(2)` is atomic
within a filesystem, but POSIX/Linux guarantee neither that the *data* is durable before the
rename is visible, nor that the *directory entry* is durable — hence the file fsync before
and the directory fsync after. Source: `rename(2)`, `fsync(2)` ("Calling fsync() does not
necessarily ensure that the entry in the directory containing the file has also reached
disk. For that an explicit fsync() on a file descriptor for the directory is also needed").

`[R]` MED — Do **not** fsync every extracted file during layer extraction. Reason: a layer
with 50 000 files would issue 50 000 fsyncs and take minutes; and we do not need per-file
durability because the layer directory is not published until the whole layer verifies.
Instead: extract into `layers/<mode>/.tmp-<uuid>/`, then a single `syncfs` (or nothing, if we
accept re-extraction after a crash), then `renameat` the directory into place, then fsync the
parent. Tradeoff: after an unclean shutdown a layer directory may be present-but-incomplete
if the rename was recorded and the data was not; that is why `meta/` should record the layer
as valid only *after* the rename+fsync, and why an unrecorded `layers/*` directory must be
treated as garbage on startup.

`[R]` HIGH — **Resume support**: keep `ingest/<uuid>/meta` with `{digest, expected_size,
bytes_written, started_at}`; on resume, re-`GET` the `/v2/.../blobs/<digest>` URL (never the
stored CDN URL — §2.7 rule 2) with `Range: bytes=<bytes_written>-`, and **feed the already-
written prefix back through the hasher first**. Reason: sha256 is not resumable across
process restarts without either re-reading the prefix or serialising the hasher state; re-
reading local disk is fast and obviously correct. Tradeoff: an O(n) local read on resume;
serialising `sha2`'s internal state is possible but fragile across versions.
`[R]` MED — expire `ingest/` entries older than a few hours at startup; containerd does the
same and it keeps the sweeper honest.

`[R]` HIGH — Concurrency: pull layers in parallel (bounded, e.g. 3–4 concurrent — `[C]`
Docker's default is 3 for downloads, 5 for uploads) but **apply them strictly in manifest
order**, because layer *n*'s extraction may depend on nothing, yet the chain-id of *n*
depends on *n-1*. `[R]` MED — a "download ahead, extract in order" pipeline is the standard
shape; the queue depth is the only tuning knob that matters.

---

## 8. OverlayFS mechanics, from the kernel documentation

Everything here is from `Documentation/filesystems/overlayfs.rst` or measured, not blogs.

### 8.1 The three directories

`[F]` HIGH — A read-write overlay mount takes:
- `lowerdir=` — one or more colon-separated read-only directories,
- `upperdir=` — the single writable directory receiving all changes,
- `workdir=` — an empty scratch directory "on the same filesystem as upperdir".

`[F]` HIGH — `workdir` exists so overlayfs can perform copy-up and whiteout creation
**atomically**: it creates the new object in `work/work/` and `rename`s it into place, and
`rename` is only atomic within one filesystem — hence the same-filesystem rule. It must be
empty at mount time and overlay will clean/recreate `work/work/` inside it.

`[O]` HOST-A + ROCKY HIGH — the rule is enforced, not advisory: `workdir` on `/dev/shm`
with `upperdir` on ext4 ⇒ mount fails
("wrong fs type, bad option, bad superblock on overlay"). Confidence HIGH.

`[F]` HIGH — A **read-only** overlay can be mounted with `lowerdir=` alone and no
upper/work. `[O]` ROCKY 4.18 — `mount -t overlay overlay -o lowerdir=$B/lo2:$B/lo $B/mrg`
succeeded and `/proc/self/mounts` shows the mount as `ro`; `touch` inside it returns
`EROFS`. Useful for `--read-only` containers with zero copy-up machinery.

### 8.2 lowerdir ordering — the classic error, settled empirically

`[F]` HIGH — `overlayfs.rst`, verbatim: "The specified lower directories will be stacked
beginning from the rightmost one and going left. In the above example lower1 will be the
top, lower2 the middle and lower3 the bottom layer."

**So `lowerdir=A:B:C` means A is the topmost lower layer and C the bottom. The LEFTMOST
entry is the most recent image layer.** And `upperdir` sits above all of them.

`[O]` HOST-A 6.11.0-29 and `[O]` ROCKY 4.18.0-425, both HIGH — two lower dirs each
containing a file `f`:
```
lowerdir=oa:ob  ->  f = FIRST      (oa/f contained "FIRST")
lowerdir=ob:oa  ->  f = SECOND     (ob/f contained "SECOND")
```
The first-listed directory wins. Verified independently on both kernels.

`[O]` HOST-A HIGH — Docker agrees. `/var/lib/docker/overlay2/<cache-id>/lower` for
`nginx:stable`'s top layer lists 6 entries, and resolving each symlink shows them in
**descending** layer order — index 5's diff first, index 0's diff last:
```
l/RAUWKKQZOGJ4KIYBHPBY67MKPO -> 60830e9ef6d2…/diff    (layer[5], the newest lower)
l/NWVJEXS6JQKAORRDL7ZP7DGRCH -> 661940fba4b7…/diff    (layer[4])
l/GEFLVWIMC6DQHCVCQEGH67UTOQ -> f524f93e01d4…/diff    (layer[3])
l/D4NSTOIOSLSOYHDFPWNWCTMSZY -> 2b330e7434e0…/diff    (layer[2])
l/URT2N4I6QV3L4CZMZBTPWR6SHS -> b4ff9fb85140…/diff    (layer[1])
l/OFELQJBR6K2OLXVPSUPROFGACJ -> e055058cc36d…/diff    (layer[0], the base)
```

`[R]` HIGH — Build the option string by **reversing** the manifest layer order:
`lowerdir = layers[n-1] : layers[n-2] : … : layers[0]`. Write one assertion test that
extracts two layers where the upper one overwrites a file, mounts, and reads the file. This
is a five-minute test that catches the single most common bug in this area.

### 8.3 Stacking limit and the mount-option length limit — two separate walls

`[F]` MED — There is a kernel constant `OVL_MAX_STACK`. Its use is visible in
`fs/overlayfs/params.c`: `if (ctx->nr == OVL_MAX_STACK) return invalfc(fc, "too many lower
directories, limit is %d", OVL_MAX_STACK);`. `[C]` LOW — the value is widely reported as
**500**, but I did not locate the `#define` (it is not in `ovl_entry.h` or the part of
`overlayfs.h` I read). **LOW confidence on the number.** What would settle it:
`grep -rn OVL_MAX_STACK fs/overlayfs/` on a checkout, or triggering the message.

`[F]` HIGH — The wall you actually hit first is the **`mount(2)` option-data page limit**.
`copy_mount_options()` copies at most one page and NUL-terminates, so the total option string
is capped at **4095 bytes** and — critically — **truncation is silent**. You do not get
`E2BIG`; you get a mount failure about a path that does not exist, because the last path in
your list was cut in half.

`[O]` HOST-A + ROCKY HIGH — measured with short lower paths so the stack limit could not be
the cause:
```
lowers=128  lowerdir-string=2579 B   OK
lowers=200  lowerdir-string=4091 B   FAIL
```
(The full option string at n=200 is `lowerdir=` + 4091 + `,upperdir=…,workdir=…` ≈ 4145 B.)
ROCKY's `mount` renders the failure as **"special device overlay does not exist"** — i.e.
`ENOENT` on a truncated path, exactly the silent-truncation signature. HOST-A's newer
`mount` just says "see dmesg". Confidence HIGH.

`[O]` ROCKY HIGH — the same limit reached from the other direction: with *long* paths, 130
lowers (option string 3311 B) mounted fine while 200 (5131 B) failed. So the binding
constraint is bytes, not count.

`[F]` HIGH — `overlayfs.rst`: "Since kernel version v6.8, directory names containing colons
can also be configured as lower layer using the `lowerdir+` mount options and the fsconfig
syscall from new mount api", and likewise `datadir+` for data-only layers.

`[O]` HOST-A 6.11.0-29 HIGH — `lowerdir+=` works and is reported back:
```
mount -t overlay overlay -o "lowerdir+=$B/1,lowerdir+=$B/2,upperdir=$B/u,workdir=$B/w" $B/m
→ overlay … lowerdir+=/tmp/…/1,lowerdir+=/tmp/…/2,upperdir=…,workdir=…,redirect_dir=nofollow,userxattr
```
`[O]` ROCKY 4.18 HIGH — `lowerdir+=` is rejected ("wrong fs type, bad option, bad
superblock"). Confidence HIGH.

`[R]` HIGH — **Use `fsopen`/`fsconfig`/`fsmount` with repeated `lowerdir+=` where available
(≥6.8), and fall back to a single `lowerdir=` string with the short-link trick (§9) below
that.** Reasons: (a) `fsconfig` passes each value as its own string, so the 4095-byte page
limit disappears entirely; (b) it is the only way to use a lower directory whose name
contains a `:`; (c) it gives real error strings via `FSCONFIG_CMD_CREATE`. Tradeoff: two code
paths, and the new mount API is D1's territory — see §14 Handoff. `[R]` HIGH — regardless of
path, **compute the option length and refuse before calling `mount(2)`** with a message that
names the layer count; a silent truncation is the worst possible failure mode.

### 8.4 The `trusted.overlay.*` / `user.overlay.*` xattrs

`[F]` HIGH — `overlayfs.h` enumerates them (prefix `overlay.`, on either the `trusted.` or
`user.` namespace per `userxattr`):

| xattr | on | meaning |
|---|---|---|
| `…overlay.opaque` | dir in upper | `"y"` = do not merge with lower; `"x"` = "whiteout base" dir for overlay-on-overlay |
| `…overlay.whiteout` | zero-size regular file | alternative whiteout encoding (instead of char 0/0) |
| `…overlay.xwhiteout` | — | the "extended" whiteout marker used with the above |
| `…overlay.redirect` | dir in upper | this upper dir corresponds to a *differently named* lower dir (set by `redirect_dir`) |
| `…overlay.origin` | upper file | file handle of the lower file it was copied up from (needs `index`) |
| `…overlay.impure` | dir in upper | this dir contains copied-up or redirected entries, so readdir must consult lower |
| `…overlay.nlink` | upper file | real link count for indexed hardlinks |
| `…overlay.upper` | index entry | back-pointer used by `index=on`/`nfs_export` |
| `…overlay.metacopy` | upper file | this upper inode holds metadata only; data lives lower |
| `…overlay.uuid` | upper root | superblock uuid, for `xino`/`uuid=on` |
| `…overlay.protattr` | — | protected-attribute marker |

`[O]` HOST-A 6.11 HIGH — a userns overlay mount wrote `user.overlay.uuid` and
`user.overlay.impure` on the upperdir root, confirming both the namespace switch and that
`impure` appears as soon as anything is copied up.

`[R]` HIGH — Our extractor writes exactly one of these (`…overlay.opaque = "y"`) and reads
none. Everything else is the kernel's private bookkeeping. `[R]` HIGH — when *reading* a
layer we produced, and when copying layers around, **preserve `…overlay.opaque` and strip
nothing**; when extracting from a tar, **never let a layer set an `overlay.*` xattr
directly** (a layer that ships `trusted.overlay.opaque` could hide arbitrary lower content,
or with `redirect_dir` could point a directory at another path — see §8.5's security note).

### 8.5 Feature options: `metacopy`, `redirect_dir`, `index`, `xino`, `nfs_export`, `volatile`

`[O]` ROCKY 4.18 HIGH — all of `metacopy=on`, `redirect_dir=on`, `index=on`, `xino=on`,
`volatile`, `nfs_export=on`, `userxattr` are **accepted** by the 4.18 RHEL kernel. So option
acceptance is not a useful kernel-version probe; behaviour must be tested.

`[F]` HIGH — `overlayfs.rst` per option, with the constraints that matter:
- **`metacopy=on`** — copy up *metadata only* on a chmod/chown, leaving data in the lower
  layer. Big win for `RUN chown -R` layers. Conflicts with `redirect_dir={off,nofollow}` and
  with `nfs_export=on`. Carries an explicit security warning: **"Do not use metacopy=on with
  untrusted upper/lower directories."** `[O]` HOST-A — Docker has it off
  (`Using metacopy: false`, and module default `metacopy=N`).
- **`redirect_dir=on`** — allows renaming a directory that exists only in a lower layer, by
  recording `…overlay.redirect`. Off by default. Also security-sensitive: a lower layer that
  can set `overlay.redirect` can make a directory alias another path.
- **`index=on`** — maintains an index of copied-up inodes so hardlinks survive copy-up and
  so `nfs_export` can work. `[O]` HOST-A module default `index=N`.
- **`xino=on|auto`** — synthesises unique inode numbers across layers by folding the layer's
  fsid into the high bits of `st_ino`; without it, two files in different layers can report
  the same inode number, which breaks `tar`, `rsync -H`, `find -samefile`, and anything doing
  hardlink detection. `[O]` HOST-A module default `xino_auto=Y`; ROCKY `xino_auto=N`.
- **`nfs_export=on`** — makes the overlay exportable; requires `index=on`.
- **`volatile`** — skips `sync`/`fsync` from the overlay entirely; "Volatile mounts are not
  guaranteed to survive a crash", and after any writeback error the mount is poisoned and
  must be remounted.

`[R]` HIGH — MVP: pass **none** of these; take the kernel defaults plus `userxattr` when
unprivileged. Reasons: they are all correctness/performance tradeoffs with sharp edges, and
Docker itself ships with `metacopy` and `redirect_dir` off on this exact kernel. `[R]` MED —
`xino=on` is the one worth revisiting early, because duplicate inode numbers across layers
is a *correctness* bug that shows up as "tar inside my container hardlinked two unrelated
files". Tradeoff: `xino` consumes high `st_ino` bits and falls back silently on overflow, so
it is not free either.
`[R]` MED — `volatile` is genuinely attractive for *layer extraction* (a throwaway
directory we will re-extract on crash anyway) and genuinely wrong for a container's rw
layer. If we use it, use it only on the extraction staging mount.

### 8.6 Which filesystems can be upper, and why NFS cannot

`[F]` HIGH — `overlayfs.rst`, verbatim: "The upper filesystem will normally be writable and
if it is it must support the creation of `trusted.*` and/or `user.*` extended attributes,
and must provide valid `d_type` in `readdir` responses, so **NFS is not suitable**." For the
lower: "a wide range of filesystems supported by Linux" and a read-only overlay "may use any
filesystem type".

`[O]` ROCKY 4.18 HIGH — measured, with the errno captured by `strace`:
```
upperdir on NFS ($HOME on 172.17.254.3:/cxfs1/OLD-HOST-home), lowerdir+mrg on ext4:
  mount("overlay", ".../mrg", "overlay", MS_MGC_VAL, "lowerdir=…,uppe"...) = -1 EACCES
  mount(… MS_MGC_VAL|MS_RDONLY, …)                                        = -1 EACCES
  util-linux renders it as: "cannot mount overlay read-only."
```
**The errno is `EACCES`, not `EINVAL` or `EXDEV`** — worth knowing, because `EACCES` looks
like a permission problem and will send you hunting for the wrong cause.

`[O]` ROCKY 4.18 HIGH — the converse works: **NFS as a `lowerdir` with an ext4 upper mounts
successfully** and the NFS content is readable through the overlay. So an image store on NFS
is usable read-only; only the writable layer must be local.

`[R]` HIGH — Probe the upper/work filesystem at startup and fail with a specific message:
check `statfs.f_type` against a deny-list (NFS `0x6969`, CIFS, FUSE, `tmpfs` is fine,
overlay-on-overlay needs care) *and* empirically try setting an xattr and reading `d_type`.
Reason: the `f_type` check alone misses "ext4 mounted `nodev`/`noexec`" and the xattr probe
alone misses the `d_type` requirement. Tradeoff: a few syscalls at startup; the alternative
is `EACCES` from `mount(2)` and a bad bug report.
`[R]` HIGH — Also require **upperdir and workdir on the same filesystem** and check it
ourselves (compare `statfs.f_fsid`/`st_dev`) rather than letting `mount(2)` fail — §8.1's
`[O]` shows the kernel's error is uninformative.

`[O]` HOST-A/ROCKY HIGH — duplicate lower directories are rejected:
`lowerdir=$W/lo:$W/lo` ⇒ `mount(2)` fails with **`ELOOP`** ("Too many levels of symbolic
links"). `[R]` MED — dedupe the lower list before building the option string; a repeated
chain-id (possible if two layers have identical `diff_id`s, which happens with empty layers)
would otherwise produce this baffling error.

---

## 9. overlay2 and snapshotters

### 9.1 What Docker's overlay2 driver puts on disk

`[O]` HOST-A HIGH — `/var/lib/docker/overlay2/` has 107 entries, classified by name shape:

| shape | count | what it is |
|---|---|---|
| 64 hex chars | 61 | a layer or container rw layer (the `cache-id` from §4.4) |
| 64 hex + `-init` | 5 | Docker's **init layer**, one per running container |
| `l` | 1 | the short-link directory |
| 25 lowercase alnum | 40 | `[C]` MED — BuildKit snapshot ids (`identity.NewID()` = base36 of 17 random bytes, zero-padded to 25); BuildKit shares the graphdriver root. Not confirmed against `snapshots.db` (a `strings` grep found no match, which a bolt page could easily hide), so MED. |

Per-layer directory contents (`[O]` HOST-A, `066a83ef…`):
```
committed          0-byte marker: this layer is read-only/finished
diff/              the layer's own changeset — this is the directory used as a lowerdir
link               26 bytes: "BT5CHWJ6MLUN2TCAECHIFOZRUS"  — this layer's short name
lower              "l/F6PW4EUS…:l/MKXBKG5X…:l/7CA7IR3I…" — the lowerdir list, topmost first
work/              overlay workdir (only meaningful while mounted)
merged/            the mountpoint (only exists while mounted)
```
And `l/` is a flat directory of symlinks: `[O]`
`l/BT5CHWJ6MLUN2TCAECHIFOZRUS -> ../ba89fa61…/diff`.

`[O]` HOST-A HIGH — a live container's actual mount, from `grep overlay /proc/mounts`:
```
overlay /var/lib/docker/overlay2/5661947892b0…/merged overlay
  rw,relatime,
  lowerdir=/var/lib/docker/overlay2/l/YXMKWDGGR4WAVV2US7SKMQ6WRH:…8 more…:/var/lib/docker/overlay2/l/MF4T4YGS632YSJZLJVMP3PJKCO,
  upperdir=/var/lib/docker/overlay2/5661947892b0…/diff,
  workdir=/var/lib/docker/overlay2/5661947892b0…/work,
  nouserxattr 0 0
```
Five containers, 8–12 lowers each, **every lower expressed via `l/<26 chars>`**, and
`nouserxattr` explicitly reported (root-mode mount, `trusted.overlay.*`).

### 9.2 Why the short-link indirection exists — the answer is arithmetic

`[C]` HIGH — moby `daemon/graphdriver/overlay2/overlay.go`, verbatim:
```go
const (
	driverName = "overlay2"; linkDir = "l"; diffDirName = "diff"; workDirName = "work"
	mergedDirName = "merged"; lowerFile = "lower"; maxDepth = 128

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
Do the arithmetic: `(26 + 1 + 1) * 128 = 3584` and `4096 - 512 = 3584`. **Exactly equal.**
The 26-character link name and the 128-layer depth limit are jointly derived from §8.3's
4095-byte `mount(2)` option-data page, with 512 bytes reserved for SELinux labels.

`[C]` HIGH — Without the indirection, one lower would cost
`len("/var/lib/docker/overlay2/") + 64 + len("/diff")` = 25 + 64 + 5 = **94 bytes**; with it,
`len("/var/lib/docker/overlay2/l/") + 26` = **53 bytes** — but the driver goes further and
uses **relative** paths plus a `chdir`, so a lower costs `len("l/") + 26` = **28 bytes**.
That is the 3584 figure. 94 bytes/layer would cap you at ~43 layers.

`[C]` HIGH — And there is a second fallback for when even that overflows. Same file: "Use
relative paths and mountFrom when the mount data has exceeded the page size. The mount
syscall fails if the mount data cannot fit within a page and relative links make the mount
data much smaller at the expense of requiring a fork exec to chroot." So Docker
`fork`+`chroot`s into `/var/lib/docker/overlay2` and issues the mount with purely relative
option paths.

`[R]` HIGH — **We do not need the short-link hack, because we should use
`fsopen`/`fsconfig` with `lowerdir+=` (§8.3) where the kernel supports it.** Reasoning: the
indirection exists solely to work around the option-page limit, and the new mount API
removes that limit at the source. Keeping short links would add a second name space to
garbage-collect (the `l/` symlinks are a real GC target Docker has leaked in the past) for no
benefit on a modern kernel. Tradeoff: on kernels below 6.8 we need the fallback anyway —
`[R]` MED — implement the fallback as **relative paths + `chdir` into the store root**, which
is Docker's `mountFrom` trick and gets us to ~28+len(chainid) bytes per layer without a
symlink farm. With a 64-hex chain-id that is 67 bytes/layer ⇒ ~53 layers, which covers
essentially every real image (nginx: 7, alpine: 1, and the 12-layer maximum on HOST-A).
`[R]` MED — if we ever need more, hash the chain-id to 16 chars for the directory name and
keep the full chain-id in metadata; that is the cheap version of Docker's `link` file.

### 9.3 The `-init` layer — a convention worth understanding before copying

`[O]` HOST-A HIGH — every container has an extra read-only layer between the image layers
and its rw layer. `/var/lib/docker/image/overlay2/layerdb/mounts/<container-id>/` holds:
```
mount-id : f3b8fd2296a2…            (the rw layer's cache-id)
init-id  : f3b8fd2296a2…-init       (the init layer)
parent   : sha256:baf694a87a40…     (the image's top ChainID)
```
and `[O]` the `-init` layer's `diff/` contains exactly:
```
/dev  /dev/pts  /dev/console  /dev/shm
/.dockerenv
/etc  /etc/mtab  /etc/hostname  /etc/hosts  /etc/resolv.conf
```
`[C]` HIGH — its purpose: those paths must **exist as mountpoints** for the runtime to
bind-mount over them, and they must not pollute the image's layers or the container's rw
diff (so that `docker diff` shows only the user's changes and `docker commit` does not
capture the host's `/etc/hosts`).

`[R]` MED — **Skip the init layer in the MVP; create the mountpoints directly in the
container's upper dir.** Reason: it is an extra layer, an extra GC root and an extra
lowerdir slot to buy a `docker diff`/`docker commit` nicety we will not have for a long time.
Tradeoff: when we do implement `commit`, the container's rw layer will contain
`/etc/resolv.conf` etc. and we will have to either add the init layer then (a storage-layout
change — see §11) or maintain an exclusion list. `[R]` MED — mitigate cheaply now by
recording the set of runtime-created paths in the container's metadata, so a future `commit`
can exclude them without a layout change.

### 9.4 containerd's snapshotter model: active vs committed

`[C]` HIGH — containerd replaced the graphdriver interface with a **snapshotter** whose
whole vocabulary is three verbs and two states. Source: containerd
`snapshots/snapshotter.go` and the "Snapshotters" documentation.

| | |
|---|---|
| `Prepare(key, parent)` → `[]Mount` | create an **active** (writable) snapshot on top of `parent`, and return *the mount options to use it* |
| `View(key, parent)` → `[]Mount` | same but read-only |
| `Commit(name, key)` | turn an active snapshot into a **committed** (immutable) one |
| `Mounts(key)` → `[]Mount` | recover the mount options for an existing active snapshot |
| `Remove` / `Stat` / `Walk` | lifecycle and query |

Two ideas are doing the work here:

1. **Active vs committed is the only distinction.** An image layer and a container's writable
   layer are the *same kind of object* at different points in its life. Unpacking layer *n*
   is: `Prepare(tmpkey, chainid(n-1))` → extract the tar into the returned upper dir →
   `Commit(chainid(n), tmpkey)`. Running a container is: `Prepare(containerid, chainid(top))`
   and never commit. That is why `docker commit` and `docker pull` share one mechanism in
   containerd and two in Docker.
2. **The snapshotter returns `[]Mount`, it does not mount.** `Prepare` hands back
   `[{Type:"overlay", Source:"overlay", Options:["lowerdir=…","upperdir=…","workdir=…"]}]`
   and someone else — the runtime, via the bundle's `config.json` — performs the mount. This
   is what makes overlayfs, btrfs, zfs, devmapper, erofs and stargz interchangeable without
   the caller knowing.

`[O]` HOST-A — the on-disk shape is
`/var/lib/containerd/io.containerd.snapshotter.v1.overlayfs/snapshots/<N>/{fs,work}` with
`<N>` a **small integer**; the chain-id "key" → integer mapping lives in
`io.containerd.metadata.v1.bolt/meta.db`. Four snapshotter roots exist on the host
(`overlayfs`, `native`, `btrfs`, `blockfile`), each with 0 snapshots — this Docker uses the
classic graphdriver, so the snapshotters are installed but idle. Confidence HIGH.

`[R]` HIGH — **Copy the two ideas, not the interface.** Specifically:
(a) adopt "unpacking a layer and creating a container rw layer are the same operation, ending
in `commit` or not" — it halves the code and is why containerd has one storage path;
(b) adopt "the storage layer computes mount options and returns them; a separate step
performs the mount" — that is exactly the D1/D3 boundary and it means our overlay logic is
testable without root by asserting on the returned option string.
**Do not** build the full six-method trait with pluggable backends in the MVP: that is one
implementation behind an interface, and §10 argues no second backend earns its place soon.
Tradeoff: when a second backend does arrive we will refactor — but by then we will know what
the interface should be, which today we do not.

`[R]` HIGH — Concretely, the MVP's storage API is two functions and one struct:
```
fn ensure_layer(chain_id) -> LayerDir            // extract if absent, idempotent
fn prepare_container(id, chain_ids) -> MountSpec // computes lowerdir/upperdir/workdir, mounts nothing
```
Reason: it is the smallest thing that expresses both ideas above, and `MountSpec` is the
handoff artefact to D1.

---

## 10. Alternatives worth knowing

One paragraph each, and an explicit verdict on whether it matters to us.

**Plain extracted directory (no union at all).** Extract every layer, in order, into one
directory; the container gets that directory as its rootfs. `[F]` HIGH — this is the only
approach that requires no kernel filesystem feature at all, and it is what containerd's
`native` snapshotter does (`[O]` HOST-A — `io.containerd.snapshotter.v1.native/snapshots/`
exists on the host). `[R]` HIGH — **build this first.** It is the §11 ladder's rung 1, it
makes the extractor testable without any mount privilege at all, it is the fallback for
"upper filesystem is unsuitable" (§8.6), and it is genuinely correct — just wasteful
(no sharing between containers, full copy per container, and `.wh.` files must be applied by
our own merge logic rather than by the kernel, per §5.3's tradeoff).

**Bind mounts.** `mount --bind` a directory into the container, or bind-mount a read-only
extracted image and overlay nothing. `[R]` HIGH — matters to us, but as a *different
feature*: it is how `-v`/`--mount` volumes work, and how the runtime injects
`/etc/resolv.conf`. It is not a storage driver: a bind mount cannot give per-container
copy-on-write. `[F]` HIGH — one relevant fact for us: a read-only bind of the extracted
image plus a `tmpfs` upper is a legitimate ephemeral-container design that needs no layer
store at all. `[R]` MED — worth keeping in mind as the "run this rootfs, throw away the
changes" mode; not a substitute for overlay.

**`fuse-overlayfs`.** A userspace reimplementation of overlayfs over FUSE, written for
rootless podman. `[O]` ROCKY — installed, `fuse-overlayfs 1.13-dev` with FUSE 3.3.0.
`[R]` MED — **it is the fallback for old kernels in rootless mode, and only that.** Our own
`[O]` measurements (§5.5) show that on kernel 6.11 the native overlay already works
unprivileged with automatic `userxattr`, and even on 4.18-RHEL it works with an explicit
`userxattr`. So the case for `fuse-overlayfs` narrows to kernels where unprivileged overlay
mounting is refused outright (mainline < 5.11 without the RHEL backport). Tradeoffs: a FUSE
context-switch on every filesystem operation (measurably slower for metadata-heavy
workloads), a userspace daemon per container whose death breaks the mount, and it uses the
`user.overlay.*`/`user.fuseoverlayfs.*` xattr namespaces so its layers are not
interchangeable with kernel-overlay layers. `[R]` HIGH — do not implement a `fuse-overlayfs`
backend; if we need it, *exec it*, since it takes the same `lowerdir=/upperdir=/workdir=`
options.

**btrfs and zfs drivers.** Use the filesystem's native subvolume + snapshot primitives: each
layer is a snapshot of its parent, so a container's rootfs is a writable snapshot.
`[C]` HIGH — both are supported by Docker and containerd (`[O]` HOST-A — the
`io.containerd.snapshotter.v1.btrfs` root exists). Genuine advantages: no union-mount
semantics at all (so no whiteout translation, no opaque xattrs, no lowerdir limit, no
copy-up latency for large files), plus cheap `commit`. `[R]` HIGH — **not for us.** Reasons:
they require the store to live on that specific filesystem (btrfs subvolume operations need
`CAP_SYS_ADMIN` or a btrfs-specific ioctl permission), zfs is out-of-tree, and the whiteout
translation we would skip is 60 lines we have to write anyway for the plain-directory
backend. Tradeoff: we give up excellent performance on hosts that already run btrfs.

**devicemapper — dead?** `[C]` HIGH — **yes, effectively.** Docker deprecated the
`devicemapper` graphdriver and removed it in Engine v25 (moby release notes); it required
either LVM thin pools or a loopback-mounted sparse file, was the notorious source of "device
or resource busy" unmount failures, and had per-container fixed-size volumes. `[R]` HIGH —
ignore entirely. Worth knowing only because a lot of old writing about Docker storage is
about devicemapper and is now misleading.

**erofs / composefs.** `erofs` is an in-kernel read-only filesystem image format;
`composefs` layers `erofs` metadata over a content-addressed directory of file payloads, so
an image is a single verity-protected metadata blob plus shared file objects. `[C]` MED —
this is where the ecosystem is actually heading: containerd has an `erofs` snapshotter and
the `composefs` model gives page-cache sharing between containers, `fs-verity` integrity per
file, and mount-time-constant image activation (no per-layer extraction at all). `[R]` MED —
**not MVP, but it is the reason to keep §9.3's "the storage layer returns a `MountSpec`"
boundary clean**, because an erofs/composefs backend returns a completely different mount and
would otherwise be a rewrite. Tradeoff: needs a modern kernel with `erofs` + `overlayfs`
data-only layers (`datadir+`, ≥6.8), which is exactly our stated dev target.

**Lazy pulling: stargz / eStargz / nydus.** `[C]` MED — reorganise a layer so it is
seekable (`stargz` = a gzip archive with per-file boundaries plus a table of contents, made
"estargz" by adding a prioritised-files section), then mount it over the network via a FUSE
snapshotter and fetch file ranges on demand; nydus does the same with its own RAFS format and
a chunk-level CAS. The payoff is real — container start time drops from "download 500 MB" to
"download the 5 MB actually executed" — and it is backwards compatible, because an estargz
layer is still a valid gzip tar for clients that do not know better. `[R]` HIGH — **out of
scope, but one design consequence today**: it is the strongest argument for keeping the
"download a blob" and "materialise a layer" steps as separate, replaceable stages (§7.2), and
for never assuming that a layer's bytes are all local before the container starts. Tradeoff of
adopting it later: it makes `[R]`§2.8's "verify the whole blob's digest before using it"
impossible, which is why stargz relies on per-chunk digests in the TOC and `fs-verity`
instead.

---

## 11. Recommended implementation progression

The project spec proposed:
`directory rootfs → OCI image extraction → immutable layers → OverlayFS → writable container layer`.

### 11.1 Verdict: the order is right, the granularity is wrong

`[R]` HIGH — **Validated in sequence, corrected in two places.** The proposed order gets the
dependency chain right: you cannot test extraction without a rootfs target, you cannot test
overlay without immutable layers, and you cannot test a writable layer without overlay. Two
corrections:

**Correction 1 — "OCI image extraction" is two rungs, not one, and they should be separated
by the registry.** As proposed, rung 2 conflates (a) parsing an image and applying layer
changesets and (b) fetching it over HTTP with bearer tokens, CDN redirects and rate limits.
These have nothing in common and fail for entirely different reasons. Split them, with the
**local OCI layout** (§7.1) as the seam: rung 2 consumes a directory produced by
`skopeo copy docker://alpine oci:./alpine-oci:latest`, rung 3 produces that directory itself.
Reason: this gives the extractor a fixed, offline, byte-identical input for its whole life —
including for the hostile fixtures of §4.1 — and it means a registry bug can never look like
an extraction bug. Tradeoff: a dependency on `skopeo` during early development, which is a
development-time-only cost and also gives us a free conformance oracle.

**Correction 2 — "immutable layers" is not a rung, it is a property of rung 2's output.**
Once the extractor writes one directory per layer keyed by chain-id and never touches it
again, immutability is achieved; there is nothing separate to build. What *is* a real
missing rung is the **layer store with GC** (§7.3), which the original list omits entirely
and which is the thing that keeps a dev machine from filling up.

### 11.2 The corrected ladder

| # | Rung | New concept | Success test | Explicit non-goals |
|---|---|---|---|---|
| 0 | **Reference parser** | name/tag/digest normalisation (§2.1) | a table test of 40 references, including `localhost:5000/x` and `a/b@sha256:…` | no network |
| 1 | **Directory rootfs** | a rootfs is just a directory; `MountSpec` is just data | `mydocker run --rootfs ./alpine-extracted /bin/sh` prints a prompt (D1 does the mounting) | no images, no layers, no CoW |
| 2 | **Layer application from a local OCI layout** | tar-as-changeset, whiteouts, the §6 hazard suite, `diff_id` verification | `oci:./alpine-oci` → a directory whose `find \| sort \| sha256sum` matches a golden value; **and every hostile fixture is rejected with the right error** | no HTTP, no sharing, no overlay — extract each layer into one flat tree |
| 3 | **Registry pull** | bearer auth, `Accept`/`Content-Type`, index → manifest → config → blobs, dual hashing, resume | `mydocker pull alpine` produces a `blobs/sha256/` store byte-identical to what skopeo produces for the same digest | no auth beyond anonymous; no push; no zstd production |
| 4 | **Layer store + chain-ids + GC** | content-addressed dedup, `diffid-by-digest`, mark-and-sweep | pulling `nginx:latest` then `nginx:stable` downloads only the differing layers; `gc` after `rmi` reclaims exactly the unreferenced blobs and layers | no leases, no cross-machine sharing |
| 5 | **OverlayFS read-only mount** | lowerdir ordering, option-length budget, `fsconfig` vs `mount(2)` | a 2-layer fixture where the upper overrides a file reads correctly through the mount, *and* the reversed-order variant fails the assertion | no writable layer yet |
| 6 | **Writable container layer** | upper/work lifecycle, per-container dirs, unmount on exit | write a file in the container, exit, confirm it is in the container's `diff/` and absent from every layer dir; `rm` the container and the dirs are gone | no `commit`, no `diff`, no init layer |
| 7 | **Rootless storage** | `userxattr`, uid-shifted extraction, store partitioning (§5.5) | the same fixtures pass as an unprivileged user in a userns with a subuid range; and on a host with no subuid range we **fail with the specific message** | no fuse-overlayfs |

`[R]` HIGH — Rung 2 before rung 3 is the single most important ordering claim here.
Reasoning: rung 2 contains all the security-critical code and rung 3 contains none of it; if
they are built together, the extractor gets tested against `alpine` (which, per §4.1's `[O]`,
exercises none of the hazards) instead of against fixtures.

### 11.3 Which step locks in an expensive decision

`[R]` HIGH — **Rung 2 does, and it is the whiteout representation.**

The decision: does the extractor write a *plain merged tree* (applying `.wh.` files by
deleting) or an *overlay-native diff* (translating `.wh.` to char 0/0 and `.wh..wh..opq` to
an opaque xattr, per §5.3)? Everything downstream depends on the answer:

- A plain tree cannot be a `lowerdir` in a stack — the deletions have already happened, so
  layer *n*'s tree contains layers 0..*n* and there is nothing to overlay. Rungs 5 and 6 must
  then re-extract everything.
- An overlay-native diff cannot be used directly as a standalone rootfs — it contains char
  devices and xattrs that only mean something to overlayfs. Rung 1's `--rootfs` mode must
  then merge them itself.
- And per §5.5, the *xattr namespace* choice (`trusted.` vs `user.`) is baked into the
  extracted bytes, so a store extracted in one privilege mode is silently wrong in the other.

`[R]` HIGH — **Resolution: make the extractor emit an overlay-native per-layer diff from
rung 2 onward, and provide the merged tree by mounting rather than by extracting.** Reasons:
(a) the per-layer diff is the form both Docker and containerd store, so it is the form all
the reference behaviour is documented against; (b) it is strictly more information than a
merged tree — you can always produce a merged tree from diffs, never the reverse; (c) it
makes rungs 5–6 a mount-option-string exercise rather than a re-extraction. Tradeoff: rung 1
either needs `overlay` earlier than the ladder implies, or accepts a "flatten diffs into a
tree" helper (~40 lines, using the same `Backend` trait from §5.6) for the no-overlay case.
Take the helper.

`[R]` HIGH — **The second expensive decision, made at rung 4: the store root layout and its
key.** Specifically `<root>/layers/<mode>/<chain-id>/diff` with `mode ∈ {trusted, userxattr}`.
Changing the *key* later (diff_id → chain-id, or adding the mode partition) means every user
re-downloads and re-extracts every image. Changing the *paths* later means a migration.
`[R]` HIGH — commit to (i) the OCI image layout verbatim for blobs, (ii) chain-id as the
layer key, (iii) the mode partition, **at rung 4, before anyone has a populated store**, and
(iv) write a one-line `storeVersion` file so a future migration is at least detectable.
Tradeoff: the mode partition looks like premature generality when rung 7 is far away — but
adding it later is a store-wipe, and adding it now is one path component.

`[R]` MED — Decisions that are explicitly **safe to defer**: zstd support, `metacopy`/`xino`
tuning, the init layer (§9.3), `tar-split` (§4.4), push, authenticated registries, the
`referrers` API, and any second storage backend. None of them changes the store layout or
the extracted bytes.

---

## 12. OCI conformance targets

`[F]` HIGH — There is a real conformance suite: `opencontainers/distribution-spec/conformance`
(a Ginkgo binary exercising pull/push/content-discovery/content-management workflows against
a live registry) and `opencontainers/image-spec` ships JSON Schemas for every document type.
Neither tests a *client*. `[R]` HIGH — so "conformance" for us means (a) our documents
validate against the image-spec schemas, (b) our store is a valid image layout, (c) we can
consume what real registries serve. There is no badge to earn on the client side.

### 12.1 The three targets

| | **MVP** | **Intermediate** | **Mature** |
|---|---|---|---|
| **Image Spec** | parse index + manifest (OCI *and* Docker media types); parse image config; apply `tar`/`tar+gzip` layers with whiteouts; verify all digests; read/write the OCI image layout | + `tar+zstd`; + `annotations` surfaced; + platform `variant`; + validate against the JSON schemas in CI; + produce a layout `skopeo`/`podman` can consume | + write manifests/configs (i.e. `commit`/`build`); + `subject`/referrers; + non-`sha256` digest algorithms behind a flag |
| **Distribution Spec** | pull only: `GET manifests` (tag + digest), `GET blobs`, anonymous bearer auth, follow redirects, `Range` resume | + `HEAD` for cache validation; + `tags/list` pagination; + registry credentials (`~/.docker/config.json`, credential helpers); + 429 backoff | + push (`POST/PATCH/PUT` chunked upload, `mount=` cross-repo), + `DELETE`, + `referrers` |
| **Runtime handoff** | emit a runtime `config.json` from the image config (Env/Cmd/Entrypoint/User/WorkingDir) | + Volumes, ExposedPorts, StopSignal, Labels | + Healthcheck, OnBuild (Docker-only fields) |
| **Storage** | overlay mount from per-layer diffs; per-container upper/work; chain-id keying | + GC; + `diffid-by-digest`; + rootless with `userxattr` | + a second backend behind `MountSpec` (erofs or native) |

### 12.2 The cheapest thing that makes us genuinely interoperable

`[R]` HIGH — Ranked by interoperability-per-line, the MVP must contain exactly these seven
things, and nothing else is load-bearing for "run a real `alpine` from Docker Hub":

1. **`docker.io`/`library/`/`latest` normalisation** (§2.1) — 50 lines. Without it, nothing
   the user types works.
2. **Anonymous bearer auth driven by the 401 challenge, with the scope synthesised
   client-side** (§2.2) — 80 lines. Without it, every request is a 401.
3. **Accept-agnostic manifest classification** (§3.4) — 40 lines. `[O]` §2.4 proves this is
   required, not defensive: Docker Hub ignores `Accept`.
4. **Docker Schema 2 media types alongside OCI** (§3.1) — ~10 lines of string matching.
   `[O]` §3.1 shows 2 of 7 images on a real host are Docker-typed.
5. **`unknown/unknown` platform filtering** (§2.5) — 5 lines. Without it, you eventually
   select a BuildKit attestation manifest and try to gunzip a JSON document.
6. **Dual hashing + `diff_id` verification** (§2.8) — 20 lines. This is what makes us
   *correct*, and it also catches (5) immediately.
7. **Correct lowerdir ordering** (§8.2) — 1 line, and the highest-consequence line in the
   project.

Total: well under 500 lines of actual decision-making. `[R]` HIGH — everything else in the
MVP column is mechanics.

`[R]` HIGH — The interoperability *test*, stated as one command:
`mydocker run alpine /bin/sh -c 'cat /etc/os-release'` from a cold store, on a host with no
Docker installed, printing Alpine's release. And the corroborating test that costs nothing:
`skopeo copy oci:./our-store:alpine containers-storage:...` — if podman can run what we
stored, our layout is right.

`[R]` MED — Two "correct on paper but useless" traps to avoid: validating against the JSON
schemas while failing on Docker media types (schema-valid, real-world-broken), and
implementing the `referrers` API before implementing `HEAD` (novel before useful).

---

## 13. Rust crates in this area

Version and activity data: `[O]` fetched from the crates.io API on 2026-09-04.

### 13.0 The constraint this area imposes on the async decision

`[R]` HIGH — D2 already decided the core is sync and that **the pull path runs as its own
process** (`D2-runtime-architecture.md` §8.3: "put the pull path in a separate process, not
just a separate module"; the supervisor must never link tokio). D3's constraint, stated
explicitly so D2 can hold the line:

- **Registry pull genuinely wants async or a thread pool** — 6–12 blob downloads in flight
  over TLS is a real wall-clock win, and every mature Rust OCI client (`oci-client`) is
  `tokio`-based.
- **Layer extraction wants neither** — it is `read`/`openat`/`write` on local disk, i.e.
  blocking syscalls that async cannot overlap usefully; a `std::thread` per layer is
  strictly better.
- **Therefore the process boundary must sit between "download blobs" and "extract layers",
  not around the whole pull.** That is a *stronger* statement than D2's: `pull` may be
  `#[tokio::main]`, but the extractor should be callable from a plain `fn` so it can also be
  invoked from `run` (rung 2 of §11.2 has no network at all) and from tests without a runtime.
- **Concretely: the extractor crate must not depend on tokio, reqwest, or any async runtime.**
  Its input is `impl std::io::Read` (or a file path), never `impl AsyncRead`. That single rule
  keeps §6's security-critical code testable, sync, and reusable, and it is the only
  async-related constraint this domain imposes.

### 13.1 HTTP client

| Crate | Version / activity | What it abstracts | Verdict |
|---|---|---|---|
| `reqwest` | 0.13.4, updated 2026-05-25, 175 M recent dl | TLS, connection pooling, redirect policy (incl. **stripping sensitive headers cross-origin** — §2.7 rule 1), gzip/brotli content-encoding, `Range` via headers, async over `hyper` | `[R]` **HIGH — use it, in the pull process only.** It hides nothing we want to learn (HTTP/1.1 and TLS are not this project's subject) and it gets the redirect-credential rule right by default. |
| `ureq` | 3.4.0, updated 2026-08-08, 56 M recent dl | the same, **synchronously**, with `rustls` and no tokio | `[R]` MED — the interesting alternative *because* it is sync: it would let the extractor and the fetcher live in one sync process and delete the §13.0 boundary. Tradeoff: parallel blob downloads become a thread pool with N connections rather than N tasks, which is fine at N=6 but means managing threads by hand. **Worth a serious look if the tokio dependency ever becomes a problem.** |
| `hyper` | 1.11.1, updated 2026-08-28, 195 M recent dl | HTTP/1+2 framing only — no TLS, no redirects, no pooling | `[R]` HIGH — **no.** Using `hyper` directly means implementing redirect following (and the credential-stripping rule), TLS setup, and connection reuse ourselves. That is HTTP plumbing, not container mechanics; it teaches us nothing about our actual subject. |

`[R]` HIGH — Whichever we pick: **disable automatic content-decoding** (`gzip` feature off,
or explicitly no `Accept-Encoding: gzip`). Reason: a layer blob is *already* gzip, and an
HTTP layer that transparently gunzips it would silently break the compressed-digest check.
This is a real footgun with `reqwest`'s `gzip` feature enabled.

### 13.2 OCI spec / client crates

| Crate | Version / activity | What it is | Verdict |
|---|---|---|---|
| `oci-spec` | **0.10.0, updated 2026-05-27**, 4.4 M recent dl | `serde` types for the image spec, distribution spec, and runtime spec, with builders; maintained under `containers/oci-spec-rs`, used by youki | `[R]` **HIGH — take it.** It abstracts JSON field names and optionality, which is exactly the kind of tedium with no educational value and a long tail of "is this field optional" bugs. It does **not** hide any mechanics: digests, chain-ids, whiteouts, mount options and every decision in §§2–9 remain ours. D2 already depends on it for the runtime side, so it is not a new dependency. |
| `oci-client` | **0.17.0, updated 2026-05-19**, 2.8 M recent dl | the ORAS project's async registry client: `Client`, `Reference`, bearer auth (`jsonwebtoken`, `http-auth`), pull/push, over `reqwest 0.13` + `tokio 1` | `[R]` **MED — read it, do not depend on it (yet).** Reasoning: the registry handshake in §2 *is* one of the things we are here to learn, it is ~400 lines, and owning it means we control the error messages for the exact quirks §2.9 documents (Hub's 401-for-nonexistent, ghcr's placeholder scope). Tradeoff: we will reimplement its auth caching and probably get rate-limit backoff wrong once. `[R]` MED — revisit at rung 3 if the handshake proves more tedious than instructive; it is a healthy, actively-maintained crate and depending on it would be defensible. |
| `oci-distribution` | 0.11.0, **updated 2024-03-27**, 189 K recent dl | the **former name** of the above | `[R]` HIGH — **do not use.** It was renamed to `oci-client` when it moved to ORAS; the last release is over two years old and downloads have collapsed 15× relative to `oci-client`. Anything citing `oci-distribution` is out of date. |

### 13.3 tar — and the answer to "do they handle the §6 hazards?"

`[C]` HIGH — **`tar` 0.4.46** (updated 2026-05-18, 51 M recent dl) is the only serious option.
And the honest answer to the question is: **no, and its defaults are wrong for us in the
opposite direction.**

`[F]` HIGH — `tar::Archive`'s configuration and defaults, from its docs:

| setting | default | consequence for us |
|---|---|---|
| `set_preserve_permissions` | **false** | setuid/setgid bits are **dropped** — a rootfs where `su` and `ping` silently do not work |
| `set_preserve_ownerships` | **false** | every file is owned by the extracting user — uid 0 files become root-owned only by accident, and multi-uid images are flattened (§6.6's silent squashing, by default) |
| `set_unpack_xattrs` | **false** | `security.capability` and everything else is dropped (which §6.7 recommends — but by accident, not by policy) |
| `set_preserve_mtime` | **true** | fine |
| `set_mask` | 0 | a umask-like mode mask, applied inverted |
| `set_overwrite` | — | whether existing files/symlinks are replaced |

So a naive `Archive::unpack()` is *safe-ish but incorrect*: it produces a rootfs that will
not run a real image. Turning the three flags on makes it correct **and** hands the archive
full control of ownership, setuid bits and xattrs — i.e. it re-arms every hazard in §6 with
no containment.

`[F]` HIGH — And the containment it does provide has failed twice, both in our exact use case:
- **RUSTSEC-2021-0080** (tar): "Links in archive can create arbitrary directories" — the
  crate created directories outside the destination before erroring on the file.
- **RUSTSEC-2026-0067** (tar ≤ 0.4.44, fixed 0.4.45): "`unpack_in` can chmod arbitrary
  directories by following symlinks" — `unpack_dir` used `fs::metadata()` (symlink-following)
  to test whether an existing path was a directory, so a symlink entry plus a same-named
  directory entry caused a `chmod` of the symlink's target.
- **RUSTSEC-2026-0148** (`boxlite` < 0.9.0, CVSS 10.0) — a Rust OCI-image tool with precisely
  this bug, fixed by routing all filesystem operations through `openat2(RESOLVE_IN_ROOT)`.

`[R]` HIGH — **Use `tar` as a header *parser*, and own the filesystem layer ourselves.**
Concretely: iterate `Archive::entries()`, read `entry.header()` and `entry.path_bytes()` /
`entry.link_name_bytes()`, apply §6's validation, and perform every filesystem operation
yourself through a pinned-root fd (§6's M1). Never call `unpack`/`unpack_in`. Reasons:
(a) we must implement the whiteout translation (§5.3) and the device-node/xattr policy (§6.3,
§6.7) anyway, so `unpack` cannot be used as-is; (b) the two RUSTSEC advisories are exactly
the code path we would be trusting; (c) tar *header* parsing (ustar/PAX/GNU, octal fields,
checksums, sparse maps) is genuinely fiddly, has no security surface once the fields are
treated as untrusted data, and is the right thing to delegate. Tradeoff: we own ~300 lines of
`*at()`-syscall extraction logic, which is the most dangerous code in the project — but we
own it *deliberately*, with tests, instead of inheriting someone else's escape.

`[R]` MED — `astral-tokio-tar` 0.7.0 (2026-08-28, 10.7 M recent dl) is the actively-maintained
async fork (the original `tokio-tar` last shipped 2023-07-14 and is abandoned). Per §13.0 we
do not want an async tar at all, so this is a "know it exists" entry.

`[R]` HIGH — `pathrs` **0.2.5** (2026-06-18) — the Rust binding to `libpathrs`, by the author
of `filepath-securejoin` and the runc path-safety fixes. **Read it, and probably depend on
it.** It is the `openat2(RESOLVE_IN_ROOT)` wrapper §6's M1 requires, with the lexical
fallback for pre-5.6 kernels already written. `[O]` low download count (155 K recent) reflects
a young crate, not neglect. `[C]` HIGH — D2's crate survey shows **youki already depends on
`pathrs`**, which is strong corroboration. `[R]` HIGH — `openat2` 0.1.2 (last released
2021-06-12) is **abandoned**; do not use it.

`[R]` MED — `cap-std` 4.0.3 (2026-08-20, 7.1 M recent dl) is the other candidate: a
capability-oriented `std::fs` replacement where a `Dir` is a pinned root and all paths are
relative to it. Tradeoff vs `pathrs`: `cap-std` is a whole filesystem API (nice ergonomics,
larger surface, and it does *not* use `openat2` on all paths) while `pathrs` is one focused
primitive. `[R]` MED — prefer `pathrs` for the extractor because the primitive is what we
need; `cap-std` is a reasonable second choice.

### 13.4 Compression and hashing

| Crate | Version / activity | Verdict |
|---|---|---|
| `flate2` | 1.1.10, 2026-08-28, 149 M recent dl | `[R]` **HIGH — use it** for gzip. It is the ecosystem default, supports pluggable backends (`rust_backend` pure-Rust / `zlib-ng` / `zlib`). `[R]` MED — start with the **pure-Rust backend** (no C, no build-time toolchain, no `unsafe` FFI surface) and only switch to `zlib-ng` if decompression measurably dominates a pull. `libz-ng-sys` 1.1.29 exists and is maintained if we do. |
| `zstd` | 0.13.3, 2025-02-20, 77 M recent dl | `[R]` MED — bindings to the C `libzstd`. Take it for *decompression only* (§4.2), accepting the C dependency. It is a thin, widely-used binding; the `unsafe` surface is the FFI boundary, not our code. |
| `async-compression` | 0.4.44, 2026-09-02, 49 M recent dl | `[R]` HIGH — **not needed**, and actively unwanted per §13.0: the decompressor lives on the sync side of the boundary. |
| `sha2` | **0.11.0**, 2026-03-25, 240 M recent dl | `[R]` **HIGH — use it.** Pure Rust, `RustCrypto`, with CPU-feature-detected SHA-NI acceleration on x86-64. It is the right dependency: we need one hash, we need it fast, and reimplementing SHA-256 teaches nothing about containers. Note 0.11 is a breaking release over 0.10 (the `digest` 0.11 traits); pin deliberately. |
| `ring` | 0.17.14, 2025-03-11, 152 M recent dl | `[R]` MED — faster in some benchmarks, but it is an assembly+C crypto library with a much larger `unsafe`/build surface, and we would pull it in transitively via `rustls` anyway. Using it *directly* for our digests adds nothing over `sha2`. |
| `openssl` | — | `[R]` HIGH — **no.** A system C library dependency, a build-time headache, and a licensing/version matrix, in exchange for a hash we already have. |
| `serde_json` | 1.0.151, 2026-07-20, 297 M recent dl | `[R]` HIGH — **use it**, with two disciplines: (a) `#[serde(deny_unknown_fields)]` is **wrong** here (registries add fields; §3.2 requires tolerance) so leave it off, and (b) **hash the raw bytes, never a re-serialised form** — `serde_json` round-tripping will not reproduce byte-identical JSON, and the digest is over the bytes (§1.3). |
| `filetime` | 0.2.29, 2026-05-12 | `[R]` MED — only if we set mtimes; `rustix::fs::utimensat` covers it with no extra dependency. Prefer rustix (D2 already chose it as the syscall crate). |

### 13.5 Content-addressed-store crates

`[R]` HIGH — **There is nothing worth taking, and that is the right answer.** The CAS we
need is: "write to a temp file, hash while writing, `rename` into `blobs/sha256/<hex>`,
fsync the directory" (§7.5). That is ~60 lines of `rustix`/`std::fs`. The general-purpose
CAS crates on crates.io are either content-addressed *caches* with their own index formats
(which would fight the OCI image layout we deliberately adopted in §7.1) or unmaintained.
`[R]` HIGH — writing it ourselves is *both* cheaper and more educational here: the fsync
ordering in §7.5 is a genuinely instructive piece of systems programming and it is the kind
of thing a crate would hide behind a `commit()` call we could not audit.

### 13.6 Summary of the dependency set for this domain

```
extractor crate (sync, no runtime):  tar, flate2, zstd, sha2, pathrs, rustix, serde_json, oci-spec
pull binary     (may be async):      reqwest (no gzip feature), tokio, + the above
```
`[R]` HIGH — Eight crates, none of which hides a mechanic we are trying to learn: HTTP, gzip,
zstd, SHA-256 and JSON are all *inputs* to this project's subject rather than the subject
itself, while every container-specific decision (digest discipline, chain-ids, whiteout
translation, path containment policy, mount options, store layout, GC) stays in our code.

---

## 14. Handoff

Cross-domain findings, stated as the interface each other domain needs from us.

### To D1 (namespaces, `pivot_root`, mount mechanics)

1. **The artefact we hand you is a `MountSpec`, not a mounted filesystem** (§9.4). Shape:
   `{fstype: "overlay", source: "overlay", target: <path>, options: Vec<String>}` where the
   options are `lowerdir=…`, `upperdir=…`, `workdir=…`, plus `userxattr` when we are not
   CAP_SYS_ADMIN-in-init-userns. D3 computes it; D1 performs it.
2. **We need `fsopen`/`fsconfig`/`fsmount` (the new mount API), not just `mount(2)`.**
   §8.3: `mount(2)`'s option data is capped at 4095 bytes and **truncates silently**, which
   caps us at ~50 layers with 64-hex directory names; `fsconfig` with repeated `lowerdir+=`
   removes the cap and is the only way to handle a lower path containing `:`. `[O]` verified
   working on HOST-A 6.11, rejected on ROCKY 4.18 — so we need both paths.
3. **`overlayfs.rst` "Permission model": the credentials of the task that calls `mount(2)`
   (or `FSCONFIG_CMD_CREATE`) are stashed and used for all later access to the lower
   layers.** So we must not mount the overlay from a privileged helper and hand it to a less
   privileged container expecting re-checks (§5.5).
4. **`workdir` must be on the same filesystem as `upperdir`** and the kernel's error when it
   is not is uninformative ("wrong fs type, bad option, bad superblock"). We check it; please
   do not "helpfully" relocate either directory.
5. **Duplicate lowerdirs give `ELOOP`** (`[O]` both hosts), and an NFS `upperdir` gives
   **`EACCES`** (`[O]` ROCKY, strace-confirmed) — two errnos that will otherwise be
   misdiagnosed.
6. The container's rootfs for `pivot_root` is the overlay's `merged` mountpoint. The
   `-init` layer that Docker inserts (§9.3) is where `/dev`, `/etc/hosts`, `/etc/resolv.conf`
   and `/etc/hostname` mountpoints come from; we are **not** creating it in the MVP, so D1
   must create those paths in the container's upper dir before bind-mounting over them.

### To D2 (runtime architecture, lifecycle, async)

1. **The image config → runtime `config.json` translation is specified by neither spec**
   (§1.2). It is a real component ("the bundler") and it belongs to whichever of us owns
   `RuntimeSpec`. D3's input to it: `Env`, `Cmd`, `Entrypoint`, `User`, `WorkingDir`,
   `ExposedPorts`, `Volumes`, `StopSignal`, `Labels` from the image config, plus
   `args = Entrypoint ++ Cmd` per Docker's convention (`[C]`, not `[F]`).
2. **Name the types `ImageConfig` and `RuntimeSpec`, never `Config`** (§1.4). The image
   config contains a nested field literally named `config`; the collision is a real bug
   source.
3. **The async boundary D3 needs is stricter than D2 §8.3's**: the extractor crate must not
   depend on tokio/reqwest and must take `impl std::io::Read`, because it is also called
   with no network at all (§11.2 rung 2) and from tests (§13.0). Pull-as-a-separate-process
   is compatible with this; "pull crate is async, extraction is a function inside it" is not.
4. `oci-spec` 0.10.0 covers the image spec as well as the runtime spec — it is one
   dependency for both of us, not two.
5. **`docker pull` and `docker run` share a cache but not a process.** Rung 3's output (a
   populated blob + layer store) is the only thing `run` consumes, so `run --offline`
   against a warm store is a legitimate and useful mode.

### To D5 (cgroups, capabilities, seccomp, rootless privilege model)

1. **Unprivileged whiteout creation works** (`[O]` both hosts, §5.4): `mknod c 0 0` succeeds
   for an ordinary user while `mknod c 1 1` gives `EPERM`. Whiteouts are not a rootless
   blocker.
2. **`userxattr` is the rootless overlay pivot, and kernels differ** (`[O]`, §5.5): kernel
   6.11 auto-selects `userxattr` for a user-namespace overlay mount (proven by
   `user.overlay.uuid`/`user.overlay.impure` appearing without the option); RHEL 4.18 does
   **not**, and without the explicit option `rm -rf` of a lower directory fails with
   **`EIO`**. Rootless mode must pass it explicitly.
3. **`trusted.*` xattrs are unreachable from any user namespace** (`[O]`: EPERM as both an
   unprivileged user and as userns-root). This is why the layer store must be partitioned by
   xattr mode (§5.5) and why root-mode and rootless-mode layers cannot be shared.
4. **`security.capability` written from inside a user namespace is silently rewritten by the
   kernel** (`[O]` ROCKY, §6.7): a v2 value becomes VFS_CAP_REVISION_3 with the namespace
   root's host uid appended (`…00000000 4501802e` = uid 780337477). Consequences: the
   extracted bytes differ from the layer, and the capability is bound to that uid. D3
   recommends dropping `security.*` by default; the *policy* on file capabilities is yours.
5. **No `/etc/subuid` range ⇒ multi-uid images are unrepresentable** (`[O]` ROCKY:
   `chown 65534` inside `unshare -Ur` returns **`EINVAL`**, not EPERM). D3's position is
   detect-and-fail-loudly rather than squash-silently, because silent squashing changes file
   ownership in a way that grants the container's root write access it should not have. This
   is the storage half of your "detect and degrade honestly" requirement.
6. **Device nodes in layers are refused by policy, not just by privilege** (§6.3). The
   `devices` cgroup / seccomp / LSM defence is yours; we simply never create them.
7. **setuid bits are preservable by an unprivileged user on files it owns** (`[O]` both
   hosts). The bit is not the privilege — the owner is. `NoNewPrivileges` and the bounding
   set are yours.

### To D4 (networking)

Only one item: the `-init` layer (§9.3) is where `/etc/resolv.conf`, `/etc/hosts` and
`/etc/hostname` exist as bind-mount targets in Docker's design. We are not creating it in the
MVP, so those files must be created in the container's upper directory. Nothing else in this
domain touches networking.

---

## 15. Open questions

Ordered by how much a wrong guess would cost.

1. **The `userxattr` auto-selection mechanism and the kernel version at which it starts.**
   `[O]` HIGH that 6.11-Ubuntu does it and 4.18-RHEL does not; **LOW** on why. I could not
   find the code in `fs/overlayfs/params.c` or `super.c`, and mainline `overlayfs.rst` still
   describes `userxattr` as admin-supplied. Cost of a wrong guess: our rootless mode passes
   or omits the option on the wrong kernels. Mitigation already recommended (§5.5): always
   pass it explicitly when unprivileged, which is correct either way. **Settle it with:**
   `git log -S userxattr --oneline fs/overlayfs/` on a mainline checkout, plus a behaviour
   bisect across 5.11 / 5.15 / 6.1 / 6.8 / 6.11 in a VM.
2. **`OVL_MAX_STACK`'s actual value.** `[F]` MED that the constant exists (the error string
   `"too many lower directories, limit is %d"` is in `params.c`); `[C]` **LOW** on the
   commonly-cited value 500. Cost: low, since §8.3's 4095-byte option limit binds first.
   **Settle it with:** `grep -rn OVL_MAX_STACK fs/overlayfs/` on a checkout.
3. **Whether the unprivileged `mknod c 0 0` exemption is `vfs_mknod`'s `is_whiteout` check.**
   Behaviour `[O]` HIGH on both hosts; mechanism `[F]` **MED** — I reasoned it from the
   `c 0 0` succeeds / `c 1 1` fails asymmetry rather than reading either kernel's source.
   Cost: if the real mechanism is narrower than I think (e.g. filesystem-specific), rootless
   whiteouts could fail somewhere I have not tested. **Settle it with:**
   `grep -n is_whiteout fs/namei.c` on 4.18-RHEL and 6.11-Ubuntu sources, and a test on
   tmpfs, xfs and btrfs upperdirs.
4. **VFS_CAP_REVISION_3 rewriting — is the trailing field really the rootid, and is the
   conversion version-dependent?** `[O]` HIGH that the value changes and that the appended
   4 bytes equal the host uid; `[F]` **MED** on the v3/rootid interpretation. Cost: our
   "drop `security.*` by default" recommendation is right regardless, so this is
   documentation accuracy rather than a design risk. **Settle it with:** `capabilities(7)`
   and `include/uapi/linux/capability.h` (`VFS_CAP_REVISION_3`, `struct vfs_ns_cap_data`).
5. **The identity of the 40 twenty-five-character directories in `/var/lib/docker/overlay2/`.**
   `[C]` **MED** — I attributed them to BuildKit's `identity.NewID()` (base36 of 17 random
   bytes, 25 chars) sharing the graphdriver root; a `strings` grep of
   `buildkit/snapshots.db` found no match, which a bolt page could easily hide. Cost: nil for
   design; it matters only for understanding what a real store contains. **Settle it with:**
   a bolt reader over `snapshots.db`, or `docker buildx du --verbose` on a non-production
   host.
6. **Docker Hub's real anonymous rate limit.** `[O]` HIGH that the JWT says
   `pull_limit=100 / pull_limit_interval=21600` while the response headers say
   `100;w=3600`; **LOW** on which governs. Cost: our backoff strategy. Mitigated by
   honouring 429 + `Retry-After` rather than predicting. **Settle it with:** a deliberate
   quota-exhaustion run from a throwaway IP — explicitly *not* on the production host.
7. **Whether `HEAD` on a manifest consumes rate-limit quota.** `[O]` MED — in our run
   `ratelimit-remaining` went 100 → 99 across a HEAD then a GET, which is consistent with
   either "HEAD is free" or "HEAD costs one and GET was served from cache". Cost: whether
   `HEAD`-based cache validation is actually cheaper than re-pulling. **Settle it with:**
   ten consecutive HEADs and reading the counter.
8. **Whether `metacopy=on` is safe for us and how much it saves.** `overlayfs.rst` carries an
   explicit "do not use with untrusted upper/lower directories" warning, and our layers *are*
   untrusted — but they are also validated by digest before use, which is arguably the
   condition the warning is about. `[R]` MED: keep it off. **Settle it with:** the
   `metacopy` commit messages and the CVE history around it (there was a known
   `metacopy`-related information-disclosure class), plus a benchmark on a `RUN chown -R`
   image.
9. **`erofs`/`composefs` viability on our dev target.** `[C]` MED that it is where the
   ecosystem is heading; unknown how much kernel plumbing (`datadir+`, `fs-verity`,
   `EROFS_FS_ONDEMAND`) our VM kernel will actually have. Cost: nil now; this is a rung-8+
   question. **Settle it with:** `zgrep -E "EROFS|FS_VERITY" /proc/config.gz` on the VM once
   it exists.
10. **Zstd layers in the wild.** No zstd layer was found on either host, so §4.2's
    recommendation to support zstd decompression is untested against a real registry blob.
    **Settle it with:** `skopeo copy --dest-compress-format zstd` into a local registry, or
    finding a public image that ships zstd layers.

---

## 16. Sources

Every URL actually used, grouped. Retrieved 2026-09-04 unless noted.

### OCI specifications (primary)
- Image Spec — manifest: https://raw.githubusercontent.com/opencontainers/image-spec/main/manifest.md
- Image Spec — config: https://raw.githubusercontent.com/opencontainers/image-spec/main/config.md
- Image Spec — layer (whiteouts, changesets): https://raw.githubusercontent.com/opencontainers/image-spec/main/layer.md
- Image Spec — index, descriptor, image-layout, spec overview: https://github.com/opencontainers/image-spec (`image-index.md`, `descriptor.md`, `image-layout.md`, `spec.md`)
- Distribution Spec: https://raw.githubusercontent.com/opencontainers/distribution-spec/main/spec.md
- Distribution Spec conformance suite: https://github.com/opencontainers/distribution-spec/tree/main/conformance
- Runtime Spec — bundle and config (via D2's material): https://github.com/opencontainers/runtime-spec

### Linux kernel (primary)
- `Documentation/filesystems/overlayfs.rst` (mainline): https://raw.githubusercontent.com/torvalds/linux/master/Documentation/filesystems/overlayfs.rst
- `Documentation/filesystems/overlayfs.rst` (kernel.org): https://www.kernel.org/doc/Documentation/filesystems/overlayfs.rst
- `fs/overlayfs/params.c`: https://raw.githubusercontent.com/torvalds/linux/master/fs/overlayfs/params.c
- `fs/overlayfs/super.c`: https://raw.githubusercontent.com/torvalds/linux/master/fs/overlayfs/super.c
- `fs/overlayfs/overlayfs.h` (the `ovl_xattr` enum): https://raw.githubusercontent.com/torvalds/linux/master/fs/overlayfs/overlayfs.h
- `fs/overlayfs/ovl_entry.h`: https://raw.githubusercontent.com/torvalds/linux/master/fs/overlayfs/ovl_entry.h
- man pages consulted on HOST-A: `openat2(2)`, `mount(2)`, `rename(2)`, `fsync(2)`, `xattr(7)`, `capabilities(7)`, `mknod(2)`, `statfs(2)`

### Implementations
- moby overlay2 graphdriver (the `idLength`/`maxDepth` derivation and the `mountFrom` chroot fallback): https://raw.githubusercontent.com/moby/moby/master/daemon/graphdriver/overlay2/overlay.go
- moby layer store / ChainID (`createChainIDFromParent`): https://github.com/moby/moby/tree/master/layer
- moby reference normalisation (`defaultDomain`, `officialRepoPrefix`, `defaultTag`): https://github.com/distribution/reference
- containerd snapshotter interface: https://github.com/containerd/containerd/blob/main/core/snapshots/snapshotter.go
- containerd archive/tar extraction: https://github.com/containerd/containerd/tree/main/archive
- containers/image and containers/storage: https://github.com/containers/image , https://github.com/containers/storage
- `cyphar/filepath-securejoin`: https://github.com/cyphar/filepath-securejoin
- `vbatts/tar-split`: https://github.com/vbatts/tar-split
- BuildKit attestation storage (`vnd.docker.reference.type`): https://github.com/moby/buildkit/blob/master/docs/attestations/attestation-storage.md

### CVEs and advisories
- CVE-2021-32760 — containerd "Archive package allows chmod of file outside of unpack target directory": https://github.com/containerd/containerd/security/advisories/GHSA-c72p-9xmj-rx3w
- CVE-2023-25153 — containerd "OCI image importer memory exhaustion": https://github.com/containerd/containerd/security/advisories/GHSA-259w-8hf6-59c2
- CVE-2021-41190 — OCI image/distribution spec manifest type confusion: https://nvd.nist.gov/vuln/detail/CVE-2021-41190 , https://seclists.org/oss-sec/2021/q4/123
- CVE-2018-15664 — Docker `cp` symlink-exchange TOCTOU: https://nvd.nist.gov/vuln/detail/CVE-2018-15664
- CVE-2022-23648 — containerd CRI image-volume symlink host file read: https://github.com/containerd/containerd/security/advisories/GHSA-crp2-qrr5-8pq7
- CVE-2014-6407 / CVE-2014-9356 — Docker archive-extraction traversal and absolute-symlink escape
- CVE-2017-14992 — moby gzip-bomb DoS in `docker load`
- RUSTSEC-2021-0080 — `tar`: "Links in archive can create arbitrary directories": https://rustsec.org/advisories/RUSTSEC-2021-0080.html
- RUSTSEC-2026-0067 — `tar` ≤0.4.44: "`unpack_in` can chmod arbitrary directories by following symlinks": https://rustsec.org/advisories/RUSTSEC-2026-0067.html
- RUSTSEC-2026-0148 — `boxlite` <0.9.0: "OCI layer symlink escape → arbitrary host write", CVSS 10.0: https://rustsec.org/advisories/RUSTSEC-2026-0148.html

### Registry behaviour and Docker docs
- Docker token authentication specification: https://distribution.github.io/distribution/spec/auth/token/
- Docker Hub Schema 1 deprecation: https://docs.docker.com/docker-hub/release-notes/ (Nov 2024 registry announcement)
- Docker Engine release notes (devicemapper removal in v25, schema-1 removal): https://docs.docker.com/engine/release-notes/
- Dockerfile ENTRYPOINT/CMD interaction table: https://docs.docker.com/reference/dockerfile/#understand-how-cmd-and-entrypoint-interact
- Docker storage driver / overlay2 documentation: https://docs.docker.com/engine/storage/drivers/overlayfs-driver/
- RFC 6750 (bearer tokens), RFC 9110 (Range, redirects): https://www.rfc-editor.org/rfc/rfc6750 , https://www.rfc-editor.org/rfc/rfc9110

### Rust crates (crates.io API, 2026-09-04)
- `oci-spec` 0.10.0 · `oci-client` 0.17.0 · `oci-distribution` 0.11.0 (renamed/stale)
- `tar` 0.4.46 (+ https://docs.rs/tar/latest/tar/struct.Archive.html for the defaults table)
- `flate2` 1.1.10 · `zstd` 0.13.3 · `async-compression` 0.4.44 · `libz-ng-sys` 1.1.29
- `sha2` 0.11.0 · `ring` 0.17.14 · `serde_json` 1.0.151
- `reqwest` 0.13.4 · `ureq` 3.4.0 · `hyper` 1.11.1
- `pathrs` 0.2.5 · `openat2` 0.1.2 (abandoned) · `cap-std` 4.0.3
- `astral-tokio-tar` 0.7.0 · `tokio-tar` 0.3.1 (abandoned) · `rustix` 1.1.4 · `nix` 0.31.3 · `filetime` 0.2.29
- `oci-client` docs: https://docs.rs/oci-client/latest/oci_client/

### Live endpoints probed (anonymous, read-only, from HOST-A)
- `https://registry-1.docker.io/v2/` and `/v2/library/alpine/{manifests,blobs,tags/list,referrers}/…`
- `https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/alpine:pull`
- `https://index.docker.io/v2/`, `https://docker.io/v2/` (302 to www.docker.com)
- `https://ghcr.io/v2/`, `https://quay.io/v2/`, `https://registry.k8s.io/v2/` (challenge comparison)
- `https://production.cloudfront.docker.com/registry-v2/…` (blob CDN, reached via 307)
