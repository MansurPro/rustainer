# V3 — adversarial verification: OCI specifications & the registry protocol

Verifier V3. Cluster: D3 §0–§3 + §12, D2 §1.1–§1.5. Date of verification **2026-10-23/24**.
I did not write the research under review. Everything below is an attempt to falsify it.

## Method and evidence base

**Documents read.** `D3-images-storage.md` §0–§3 (lines 1–656) and §12 (lines 2037–2090);
`D2-runtime-architecture.md` §1.1–§1.5 (lines 1–130); `VERIFIER-CONTRACT.md`.
D3 §4.4's ChainID formula was read *only* because adjudication target 7 names it — the rest
of §4 onward is V4's and I have not touched it.

**Primary sources, pinned.** All spec quotations are from tagged releases resolved to commit
SHAs on 2026-10-23, never from `main`:

| Spec | Tag | Tag object | Commit SHA | Released |
|---|---|---|---|---|
| runtime-spec | `v1.3.0` | `0eaca84ca040a452ebf87480b3f7bedf79d46f46` | `92249139eea7161e13745abd4cb6d0ea02a3227a` | 2025-11-04 |
| image-spec | `v1.1.1` | — | (tag `v1.1.1`) | 2025-03-03 |
| distribution-spec | `v1.1.1` | — | (tag `v1.1.1`) | 2025-01-29 |

**Empirical work.** All network probes were run from the **local macOS machine** (`curl 8.7.1`,
LibreSSL 3.3.6, HTTP/2), anonymous tokens only, no credentials, read-only `GET`/`HEAD`.
**Nothing was run against `host-a`** — every claim I needed to test was testable from here,
and the production host adds risk for no information. Egress IP as reported by Hub:
`docker-ratelimit-source: 198.51.100.20`.

Five registry backends were probed, deliberately: `registry-1.docker.io` (Docker Hub),
`ghcr.io` (GitHub), `quay.io` (Red Hat Quay), `us-east5-docker.pkg.dev` (Google Artifact
Registry) and the `registry.k8s.io` redirector in front of it. That breadth is the single
most important methodological difference between this verification and D3 §2, which probed
Docker Hub and read the `WWW-Authenticate` header of three others.

---

# Part 1 — The `root` conflict, and the rest of D2 §1.3's table — **CORRECTED (D3 wrong, D2 right on substance but incomplete)**

The previous attempt stopped at *"Found a possible D2/D3 conflict on `root`. Let me check the
exact spec text."* Here is the exact spec text, and the settlement.

## 1.1 What the spec actually says — **CONFIRMED**, and it says *both* things

`opencontainers/runtime-spec@92249139eea7161e13745abd4cb6d0ea02a3227a/config.md#L27-L33`
(tag `v1.3.0`), verbatim:

```
## Root

**`root`** (object, OPTIONAL) specifies the container's root filesystem.
On Windows, for Windows Server Containers, this field is REQUIRED.
For Hyper-V Containers, this field MUST NOT be set.

On all other platforms, this field is REQUIRED.

* **`path`** (string, REQUIRED) Specifies the path to the root filesystem for the container.
* **`readonly`** (bool, OPTIONAL) ... defaults to false.
```

So the governing text carries **three** platform conditions, not one:

| Platform | `root` |
|---|---|
| Linux / POSIX / "all other platforms" | **REQUIRED** |
| Windows — Windows Server Containers | **REQUIRED** |
| Windows — **Hyper-V Containers** | **MUST NOT be set** |

The type annotation `(object, OPTIONAL)` is the *union* annotation across platforms; the
normative requirement lives in the prose immediately below it. There is exactly one platform
class where `root` is forbidden, and it is the one nobody mentions.

**Verdict on D2 §1.3 — CONFIRMED on substance, CORRECTED on completeness.** D2 says
"`root` — **REQUIRED** on Linux. Subfields: `path` **REQUIRED**, `readonly` OPTIONAL
(default false)." Every word of that is true. What it omits is that the spec's own
annotation reads OPTIONAL, which matters the moment anyone generates bindings (see §1.3).

## 1.2 D3 §1.3 is the one that is wrong — **CORRECTED**

D3 §1.3, line 114, defines the bundle as:

> runtime-spec `bundle.md`: a directory containing **exactly two things a runtime must find**:
> a `config.json` in the bundle's top-level directory, and *"a directory representing the root
> filesystem of the container"*, referenced by `root.path` in that `config.json`.

Two defects, both in one sentence.

**(a) The quoted string is not in the current spec.** `bundle.md` at `v1.3.0` reads:

```
2. container's root filesystem: the directory referenced by `root.path`,
   if that property is set in `config.json`.
```

`opencontainers/runtime-spec@92249139eea7161e13745abd4cb6d0ea02a3227a/bundle.md#L17`.
The phrase *"a directory representing the root filesystem of the container"* is **stale text
from a pre-1.1 revision**. D3 quoted a spec edition it did not name.

**(b) The conditional was dropped, and it inverts the meaning.** The current text says the
rootfs directory is part of the bundle **"if that property is set"** — and only item 1
(`config.json`) carries the word REQUIRED ("This REQUIRED file MUST reside in the root of the
bundle directory"). D3's "exactly two things a runtime must find" states the opposite of what
`bundle.md` says. `bundle.md` is deliberately conditional precisely because of the Hyper-V
case in §1.1.

**This is the D2/D3 conflict, adjudicated: D2 is right, D3 is wrong.** D3's sentence should
read: *the bundle MUST contain `config.json` at its top level; it contains a root filesystem
directory referenced by `root.path` if and only if `root` is set, which on Linux it MUST be.*

## 1.3 The finding neither document has — **CORRECTED, and it invalidates a §12 target**

I checked the prose against the spec's **own JSON Schema**, `schema/config-schema.json` at
`v1.3.0`. The top-level `required` array is:

```json
"required": ["ociVersion"]
```

**That is the entire list.** `root` is not in it. Nor is `process.user`. Nor is
`mounts[].destination`. The `process` subschema's `required` is `["cwd"]` alone — `args` is
absent, despite the prose "at least one entry is REQUIRED (non-Windows)".

| Field | `config.md` prose | `config-schema.json` |
|---|---|---|
| `ociVersion` | REQUIRED | in `required` ✓ |
| `root` | REQUIRED on Linux | **not required** |
| `root.path` | REQUIRED | in `root.required` ✓ |
| `process.cwd` | REQUIRED | in `process.required` ✓ |
| `process.args` | ≥1 entry REQUIRED non-Windows | **not required** |
| `process.user.uid`/`.gid` | REQUIRED on POSIX | **`user` has no `required`** |
| `mounts[].destination` | REQUIRED | **items have no `required`** |

So: **a `config.json` containing only `{"ociVersion":"1.3.0"}` validates cleanly against the
official OCI JSON Schema and is simultaneously illegal on Linux per the prose.** The schema is
systematically weaker than the normative text. It is a convenience artifact, not the
specification.

**Consequence for us.** D3 §12.1 lists, in the *Intermediate* Image-Spec column,
"+ validate against the JSON schemas in CI". For the **runtime** spec that target is close to
worthless as a correctness gate — it will pass configs our runtime must reject, and D2 §1.3's
`[R]` HIGH ("those five are hard errors") cannot be implemented by schema validation. The five
hard errors must be hand-written checks. Keep schema validation as a *shape* check only and
say so.

## 1.4 Audit of the rest of D2 §1.3 — one entry at a time

Because one entry was questionable, I checked all of them against
`config.md@92249139` rather than assuming the table was spot-checked.

| D2 §1.3 claim | Spec text (`config.md` v1.3.0) | Verdict |
|---|---|---|
| `ociVersion` — REQUIRED, SemVer v2.0.0 | L17: "(string, REQUIRED) MUST be in SemVer v2.0.0 format" | **CONFIRMED** |
| `root` — REQUIRED on Linux; `path` REQUIRED, `readonly` OPTIONAL default false | L29–L44 | **CONFIRMED** (incomplete — see §1.1, Hyper-V `MUST NOT`) |
| `process` — OPTIONAL top-level, "REQUIRED when `start` is called" | L239–L240: "(object, OPTIONAL) specifies the container process. This property is REQUIRED when `start` is called." | **CONFIRMED — verbatim** |
| `mounts` — OPTIONAL array; `destination` REQUIRED; `source`/`options`/`type`/`uidMappings`/`gidMappings` OPTIONAL | L65: "(array of objects, OPTIONAL)"; L70 `destination` REQUIRED; L80 `source` OPTIONAL | **CONFIRMED** |
| *(unstated by D2)* | L66: **"The runtime MUST mount entries in the listed order."** | **GAP** — see below |
| *(unstated by D2)* | L71–L73: Linux `destination` SHOULD be absolute; relative MAY be accepted, interpreted relative to `/`, and is **deprecated** | **GAP** |
| `hostname`, `domainname`, `hooks`, `annotations` — all OPTIONAL | L497, L509, L552, L718 — all "(… OPTIONAL)" | **CONFIRMED** |
| `linux` — OPTIONAL, "MAY be set" | L523: "(object, OPTIONAL) … This MAY be set if the target platform of this spec is `linux`." | **CONFIRMED** |
| `windows` — REQUIRED only if target platform is windows | L531: "(object, OPTIONAL) … This **MUST** be set if the target platform of this spec is `windows`." | **CONFIRMED** (same OPTIONAL-annotation/MUST-prose shape as `root`) |
| `freebsd`/`solaris`/`vm`/`zos` OPTIONAL | L521–L536, all "MAY be set" | **CONFIRMED** |
| `process.cwd` REQUIRED | L248: "(string, REQUIRED) … MUST be an absolute path" | **CONFIRMED** (D2 omits "MUST be an absolute path") |
| `process.args` OPTIONAL but ≥1 entry REQUIRED non-Windows; `commandLine` substitutes on Windows | L250–L253, verbatim | **CONFIRMED** |
| `process.user.uid`/`.gid` REQUIRED on POSIX; `umask`, `additionalGids` OPTIONAL | L370–L373 | **CONFIRMED** |
| `hooks[*].path` REQUIRED, "MUST be absolute"; `timeout` OPTIONAL, "MUST be greater than zero" | L555–L561 | **CONFIRMED** |

**The table survives.** Thirteen of thirteen entries hold. The `root` entry was correct all
along; the conflict was D3's, not D2's.

Two substantive **gaps**, both cheap to close and both load-bearing for an implementation:

1. **`[F]` HIGH — "The runtime MUST mount entries in the listed order."** (`config.md` L66.)
   This is one of very few ordering MUSTs in the whole runtime spec and D2 §1.3 does not
   record it. Mount order is observable behaviour (a later mount shadowing an earlier one) and
   a HashMap-shaped `mounts` representation silently violates it. Record it next to the
   lowerdir-ordering rule as a second place where order is correctness, not style.
2. **`[F]` MED — relative `mount.destination` is deprecated but legal on Linux**, interpreted
   relative to `/`. Our parser must either accept-and-normalise or reject with a message that
   names the deprecation — "invalid path" is the wrong error.

## 1.5 What this changes

- D3 §1.3's bundle definition must be rewritten (stale quote + inverted conditional).
- D2 §1.3's `[R]` HIGH five-hard-errors parser **stands**, with the addition that it cannot be
  delegated to the OCI JSON Schema.
- D3 §12.1's "validate against the JSON schemas in CI" must be re-scoped to the *image* spec
  (where the schemas are tighter) and demoted for the *runtime* spec.
- Add: `root` **MUST NOT** be set for Hyper-V containers — irrelevant to us as a Linux-only
  engine, but it is the reason the annotation says OPTIONAL, and knowing that stops a future
  reader "fixing" the spec reading back the other way.

---

# Part 2 — "Docker Hub ignores `Accept`" — **CONFIRMED for Hub, and the generalisation is FALSE and DANGEROUS**

This is D3's headline empirical claim and its MVP item 3. I re-ran it, widened it, and the
result is the most consequential finding in this verification.

## 2.1 Docker Hub — **CONFIRMED, and strengthened**

`GET https://registry-1.docker.io/v2/library/alpine/manifests/latest`, anonymous token,
ten different `Accept` values:

| `Accept:` sent | Status | `Content-Type` returned | `Docker-Content-Digest` |
|---|---|---|---|
| *(header omitted)* | 200 | `application/vnd.oci.image.index.v1+json` | `sha256:be2656061b…` |
| `…oci.image.index.v1+json` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `…docker.distribution.manifest.list.v2+json` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| both of the above | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `…oci.image.manifest.v1+json` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `…docker.distribution.manifest.v2+json` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `…docker.distribution.manifest.v1+prettyjws` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `*/*` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `application/x-total-nonsense` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |
| `text/plain` | 200 | `…oci.image.index.v1+json` | `be2656061b…` |

`[O]` HIGH, local macOS, `curl 8.7.1`, 2026-10-24. Identical `Content-Type`, identical digest,
identical `content-length: 9218` in all ten. **D3 §2.4's claim is CONFIRMED** and now rests on
ten cases instead of six, including two media types that are not media types a registry could
plausibly hold. Hub does not negotiate; it returns the stored bytes with the stored type.

(Note the digest has drifted from D3's `sha256:28bd5fe8…` to `sha256:be2656061b…`, exactly as
D3 §2 warned — `alpine:latest` was rebuilt. Not a defect; the warning was correct.)

## 2.2 The generalisation — **CORRECTED. Three registries, three incompatible behaviours.**

D3 never claims "all registries are lenient", but D3 §3.4 and MVP item 3 are written as if
Accept-agnosticism were the problem to solve. It is **half** the problem, and the other half
fails the opposite way.

### ghcr.io — **strict. A wrong or absent `Accept` yields `404 MANIFEST_UNKNOWN`.**

`GET https://ghcr.io/v2/astral-sh/uv/manifests/latest`, anonymous token:

| `Accept:` sent | Status | Body |
|---|---|---|
| *(omitted)* | **404** | `{"errors":[{"code":"MANIFEST_UNKNOWN","message":"OCI index found, but Accept header does not support OCI indexes"}]}` |
| `*/*` | **404** | same |
| `…docker.distribution.manifest.list.v2+json` | **404** | same |
| `…oci.image.manifest.v1+json` | **404** | same |
| `…oci.image.index.v1+json` | 200 | `…oci.image.index.v1+json`, `sha256:32a7bb39b4…` |
| full containerd list | 200 | `…oci.image.index.v1+json`, `sha256:32a7bb39b4…` |

Two things to take from that. First, **a client that omits `Accept` gets a 404 that reads
exactly like "this image does not exist"** — the failure is silent and misattributed. Second,
**`Accept: */*` does not match.** ghcr.io does literal string membership on the Accept list, in
violation of RFC 9110 §12.5.1, under which `*/*` matches every media type. We cannot argue with
it; we can only send the explicit list.

### Google Artifact Registry (`*.pkg.dev`, behind `registry.k8s.io`, `gcr.io`) — **also strict**, and it tells you so

`GET https://us-east5-docker.pkg.dev/v2/k8s-artifacts-prod/images/pause/manifests/3.9` with
`Accept: application/vnd.oci.image.index.v1+json`:

```
HTTP/2 404
{"errors":[{"code":"MANIFEST_UNKNOWN","message":"Manifest has media type
 \"application/vnd.docker.distribution.manifest.list.v2+json\" but client accepts
 [\"application/vnd.oci.image.index.v1+json\"]"}]}
```

Same class as ghcr.io, with the most useful error message of any registry tested — it names
both sides of the mismatch. Note what it proves: an **OCI-only** client is not a correct
client. `registry.k8s.io/pause:3.9` is stored as a **Docker manifest list** and you cannot get
it without asking for one. D3 §3.1's "support Docker Schema 2 media types alongside OCI
(~10 lines of string matching)" is not a nicety — on two of five registries, omitting the
Docker types from `Accept` makes real images unreachable.

### quay.io — **negotiates *and converts*: the same tag returns three different digests**

This is the worst case and nothing in D3 anticipates it.
`GET https://quay.io/v2/podman/stable/manifests/latest`, anonymous token, same tag, one minute
apart:

| `Accept:` sent | `Content-Type` returned | size | `Docker-Content-Digest` | `sha256(body)` matches? |
|---|---|---|---|---|
| *(omitted)* | `…docker.distribution.manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` | yes |
| `…docker.distribution.manifest.v2+json` | `…manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` | yes |
| `…docker.distribution.manifest.list.v2+json` | `…manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` | yes |
| `…oci.image.index.v1+json` | `…oci.image.index.v1+json` | 901 | `sha256:55d98815a3…` | yes |
| `…oci.image.manifest.v1+json` | `…oci.image.manifest.v1+json` | 2988 | `sha256:4daf0e142d…` | yes |
| full containerd list | `…oci.image.index.v1+json` | 901 | `sha256:55d98815a3…` | yes |

`[O]` HIGH. Three distinct documents, three distinct digests, one tag, one instant in time.
Parsed `schemaVersion` confirms it: `1` for the 16337-byte bodies (`fsLayers`, `history`,
`name`, `tag`), `2` for the other two.

Three consequences, all of which change code:

1. **"tag → digest" is not a function of the tag on quay.io. It is a function of
   (tag, `Accept`).** Any cache, any "is my image still current?" check, any lockfile that
   records a tag's digest **must record the `Accept` list that produced it**, or it will
   thrash. D3 §2.4's `[R]` HIGH "record tag→digest" is under-specified for this registry.
2. **quay.io's default — no `Accept` header — is Docker Schema 1.** Not a legacy corner: this
   is `quay.io/podman/stable`, a live first-party Red Hat image, today.
3. quay's rule appears to be *"if the client's `Accept` contains no OCI type, downgrade to
   schema 1"* — asking for `manifest.v2+json` (schema **2**) still got schema **1**. So a
   Docker-schema2-only client silently receives schema 1.

### Summary — five backends, four behaviour classes

| Registry | Behaviour on `Accept` | Failure mode if you get it wrong |
|---|---|---|
| Docker Hub | **ignores it entirely**; returns stored type | you parse the wrong document |
| ghcr.io | **strict literal match**; `*/*` does not match | `404 MANIFEST_UNKNOWN` — looks like "no such image" |
| Google Artifact Registry / `gcr.io` / `registry.k8s.io` | **strict**, error names both sides | `404 MANIFEST_UNKNOWN` |
| quay.io | **negotiates and converts**; schema-1 fallback | different digest per request; silent schema-1 downgrade |
| `registry.k8s.io` front | 307-redirects **manifests** cross-origin (see Part 3) | auth realm changes mid-request |

## 2.3 What a correct client must therefore do — **D3's rule is right but insufficient**

D3 §2.4 `[R]` HIGH says: *"Dispatch on the response `Content-Type`, and cross-check it against
the parsed document, never on what you asked for."* **That half is CONFIRMED and necessary.**
But a reader who internalises "Accept doesn't matter, Hub ignores it" will write a client that
404s on two of five registries. The complete rule is two-sided:

> **`[R]` HIGH (V3, replaces D3 §2.4's single rule).**
> **Sending side:** always send the full explicit `Accept` list — all four modern types
> (`oci.image.index.v1+json`, `oci.image.manifest.v1+json`,
> `docker.distribution.manifest.list.v2+json`, `docker.distribution.manifest.v2+json`) — on
> *every* manifest `GET` and `HEAD`. Never omit it; never send `*/*` and expect it to work;
> never narrow it per-call. Omitting it is a hard 404 on ghcr.io and GAR, and a silent
> schema-1 downgrade on quay.io.
> **Receiving side:** never assume you got what you asked for. Classify by in-document
> `mediaType` → `Content-Type` header → structural sniff, exactly as D3 §3.4 says.
> **Pinning:** record the `Accept` list alongside any cached tag→digest mapping; on quay.io the
> digest is a function of both.

Note the two sides are independent and the reasoning differs: you send the full list to make
the *request* succeed, and you ignore it on receipt to make the *parse* correct. D3 derived
the second from Hub alone and never derived the first, because Hub cannot teach it.

## 2.4 Verdicts for Part 2

| Claim | Verdict |
|---|---|
| D3 §2.4 `[O]` HIGH "Docker Hub ignores `Accept` completely" | **CONFIRMED** (10/10 cases, incl. `*/*` and nonsense types) |
| D3 §2.4 `[R]` HIGH "dispatch on `Content-Type`, not what you asked for" | **CONFIRMED** — necessary, and necessary for a reason D3 did not find (quay converts) |
| Implicit "registries are lenient about `Accept`" | **CORRECTED** — false on 3 of 5 backends; two 404, one converts |
| D3 §12.2 MVP item 3 "Accept-agnostic manifest classification — 40 lines" | **CORRECTED** — needs a sending-side rule too; still ~40 lines, but the *other* 5 lines (always send the full list) are the ones that decide whether ghcr.io works at all |
| D3 §3.1 "Docker Schema 2 media types — ~10 lines, pure win" | **CONFIRMED and upgraded** — not a convenience; `registry.k8s.io` is unreachable without them |

---

# Part 3 — The pull trace in D3 §2, step by step against the Distribution Spec — **mostly CONFIRMED, two `[F]` tags are wrong**

I re-ran the whole trace and checked every step against
`opencontainers/distribution-spec` tag `v1.1.1` (`spec.md`, 859 lines). The trace is good
work; two claims tagged `[F]` HIGH are not in the spec, and one spec quote is a MUST that is
actually a SHOULD.

## 3.1 Step 0 — short-name resolution — **CONFIRMED, now pinned, plus two cases D3 missed**

D3 §2.1 is right that all four defaults are convention. Its citation (`normalize.go`) was
unpinned; here it is pinned:
`distribution/reference@v0.6.0/normalize.go#L20,#L31,#L36,#L39` —
`legacyDefaultDomain = "index.docker.io"`, `defaultDomain = "docker.io"`,
`officialRepoPrefix = "library/"`, `defaultTag = "latest"`. **CONFIRMED.**

`splitDockerDomain` (`normalize.go#L126-L170`) has **five** branches, not the three D3
describes:

1. no `/` at all → `docker.io/library/<name>` (fast path)
2. first component `== "localhost"` → domain
3. first component `== "index.docker.io"` → canonicalised to `docker.io`
4. first component contains `.` or `:` → domain
5. **first component is not all-lowercase → domain** ← *D3 misses this one*

and only then, `if domain == docker.io && remoteName has no "/"` → prepend `library/`.

Two test-table cases D3's ~40-case list should gain:

- `Foo/bar` → domain `Foo`, because of branch 5. Upper-case namespaces are illegal on Hub, so
  the parser treats the component as a hostname.
- `localhost` **alone, with no `/`** → `docker.io/library/localhost`, **not** the host
  `localhost`. This is an acknowledged upstream defect, carried as a `FIXME(thaJeztah)` in
  `normalize.go#L135`: *"account for bare `localhost` or `example.com` names, which SHOULD be
  considered a domain."* We must decide deliberately whether to copy the bug (interop) or fix
  it (correctness). My read: **copy it**, because `docker pull localhost` resolving to Hub is
  what every other tool does, and diverging makes us the odd one out for a name nobody types.

`[F]` HIGH — the grammar D3 quotes is CONFIRMED verbatim at
`distribution-spec@v1.1.1/spec.md#L155-L167`: `<name>` regex and the ≤128-char tag regex, plus
one thing D3 omits — `<reference>` **"MUST be either (a) the digest of the manifest or (b) a
tag. The `<reference>` MUST NOT be in any other format."** And an implementers' note: many
clients cap `host + "/" + <name>` at 255 characters.

## 3.2 Steps 1–3 — the ping, the challenge, the token — **CONFIRMED, with three new failure modes**

**The `/v2/` ping.** `[O]` 2026-10-24, six registries:

| Registry | `GET /v2/` | `WWW-Authenticate` |
|---|---|---|
| `registry-1.docker.io` | 401 | `Bearer realm="https://auth.docker.io/token",service="registry.docker.io"` |
| `ghcr.io` | 401 | `…realm="https://ghcr.io/token",service="ghcr.io",scope="repository:user/image:pull"` |
| `quay.io` | 401 | `…realm="https://quay.io/v2/auth",service="quay.io"` |
| `registry.k8s.io` | 401 | `…realm="https://registry.k8s.io/token",service="registry.k8s.io"` |
| `public.ecr.aws` | 401 | `…realm="https://public.ecr.aws/token/",service="public.ecr.aws",scope="aws"` |
| `mcr.microsoft.com` | **200** | *(none — no auth at all)* |

D3's four-row table is **CONFIRMED** unchanged after seven weeks. Two additions:

- **`public.ecr.aws` sends `scope="aws"`** — a second registry with a non-repository scope in
  the challenge. D3's `[R]` HIGH ("synthesize the scope client-side; use the challenge only
  for `realm` and `service`") was justified by ghcr.io alone; it now has two independent
  witnesses and should be promoted from "the only thing that works everywhere" to a rule with
  a stated reason: **the `scope` field of a `WWW-Authenticate` challenge is not reliably
  about the resource you requested.**
- **`mcr.microsoft.com` never challenges at all.** A client whose flow is "ping → expect 401 →
  get token → retry" will either hang on a missing challenge or send a pointless token. D3's
  `[R]` MED "skip `GET /v2/` in normal operation; handle the 401 lazily on the real request"
  is **CONFIRMED and strengthened** — lazy 401 handling is the only shape that covers both a
  registry that always challenges and one that never does.
- `quay.io`'s realm is `https://quay.io/v2/auth` — *inside* `/v2/`. A client with a blanket
  "any `/v2/` request needs a bearer token" rule will try to authenticate to the auth
  endpoint. Scope the rule to the resource endpoints, not the path prefix.

**The challenge on a resource request carries the scope.** `[O]` HIGH:

```
GET /v2/library/alpine/manifests/latest   (no Authorization)
HTTP/2 401
www-authenticate: Bearer realm="https://auth.docker.io/token",service="registry.docker.io",scope="repository:library/alpine:pull"
```

D3's §2.10 line [2] is **CONFIRMED** exactly, including the `scope` string format
`repository:<name>:pull`.

**The token request — three silent failure modes D3 does not record.** `[O]` HIGH,
`auth.docker.io`, decoding each returned JWT's `access` claim:

| `scope=` sent | HTTP | granted `access` claim |
|---|---|---|
| `repository:library/alpine:pull` | 200 | `[{type:repository, name:library/alpine, actions:[pull]}]` |
| `repository:library/alpine:pull,push` | 200 | `[{… actions:**[pull]**}]` ← push silently dropped |
| `repository:library/alpine` *(no action)* | **200** | **`[]`** |
| `library/alpine:pull` *(no `repository:` prefix)* | **200** | **`[]`** |
| `repository:library/alpine:pull:extra` | **200** | **`[]`** |
| *(no `scope` param at all)* | **200** | — |
| *(no `service` param at all)* | **200** | — |

**A malformed scope is not an error.** You get `200 OK` and a perfectly well-formed JWT that
grants nothing, and the failure surfaces one request later as a `401` on the manifest — which
reads as "your credentials were rejected". This is the same misattribution trap as D3 §2.9's
nonexistent-repo case, one layer up.

> **`[R]` HIGH (V3, new).** After fetching a token, **decode its payload and check the `access`
> claim actually grants what you asked for** before using it. It is ~10 lines (base64url the
> middle JWT segment, parse JSON, look for `name == <repo>` and `"pull" ∈ actions`) and it
> converts three silent 401s into one accurate error at the point of cause. Do **not** verify
> the signature — we are not the audience and the `x5c` chain is Docker's; this is a sanity
> read, not a security check. Tradeoff: a JWT-shaped assumption about a token the spec says
> is opaque — so guard it, and on a parse failure proceed as if the check passed.

**Anonymous token issuance for a repo that does not exist — CONFIRMED.** `[O]` HIGH: the token
endpoint issues a token for `library/definitelynotarealimagexyz`, and the manifest request
then returns `401 UNAUTHORIZED` (not `404 NAME_UNKNOWN`) with
`detail:[{Type:repository, Name:library/definitelynotarealimagexyz, Action:pull}]`. D3 §2.9
reproduces exactly, seven weeks later. Its `[R]` HIGH ("say *not found or not accessible*,
never *authentication failed*") stands.

**The `exp`/`expires_in` discrepancy — CONFIRMED, and explained.** D3 §2.3 flagged
`exp - nbf = 600s` against `expires_in: 300` as a disagreement. Decoding today's token:
`iat=1792802991`, `nbf=1792802691`, `exp=1792803291`. So `nbf = iat - 300` and
`exp = iat + 300`. There is no disagreement: `expires_in` is `exp - iat`, and `nbf` is
backdated 300s for clock skew. D3's `[R]` HIGH (cache on `expires_in` minus 30s skew) is
**CONFIRMED** and the puzzle it flagged can be closed. Lifetime is genuinely 300 seconds from
issuance, so D3's warning that a 20-layer pull can outlive its token is real.

## 3.3 Step 4 — `Docker-Content-Digest`: may a client trust it? — **CORRECTED (D3 quoted MUST; the spec says SHOULD)**

D3 §2.4 states, tagged `[F]` HIGH:

> distribution-spec: "A successful response **MUST** contain the digest of the uploaded blob in
> the header `Docker-Content-Digest`."

The spec says **SHOULD**, in all three places it says it
(`distribution-spec@v1.1.1/spec.md#L177`, `#L195`, `#L213`):

> "A successful response **SHOULD** contain the digest of the uploaded blob in the header
> `Docker-Content-Digest`."

And `spec.md#L49-L52` is more pointed still, under the heading *Legacy Docker support HTTP
headers*:

> "Because of the origins of this specification, the client MAY encounter Docker-specific
> headers, such as `Docker-Content-Digest`, or `Docker-Distribution-API-Version`.
> **These headers are OPTIONAL and clients SHOULD NOT depend on them.**"

**Verdict: CORRECTED.** The header is optional and the spec explicitly tells clients not to
depend on it. D3's conclusion was right and its evidence was wrong — which matters, because a
future reader who trusts the MUST might build a "registry is broken, abort" path around a
missing header. Do not. **A missing `Docker-Content-Digest` is spec-legal.**

**The direct answer to "may a client trust it or must it recompute?"** The spec is precise
(`spec.md#L182-L183` for manifests, `#L196-L198` for blobs):

> "Most clients MAY ignore the value, but if it is used, the client **MUST verify** the value
> matches the returned manifest."
> "If the `<reference>` part of a manifest request is a digest, clients **SHOULD verify** the
> returned manifest matches this digest."
> *(blobs)* "If present, the value of this header **MUST** be a digest matching that of the
> response body. … Clients SHOULD verify that the response body matches the requested digest."

So: **you may ignore the header entirely, but you may not believe it without checking.** There
is no third option where you trust it. D3's `[R]` HIGH ("treat it as a hint only, always
recompute") is **CONFIRMED** and is in fact the only spec-compliant way to use the header at
all.

**And there is a case where naive recompute-and-compare *fails*.** `[O]` HIGH,
`docker/whalesay:latest` on Hub, 2026-10-24:

```
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
content-length: 20401
docker-content-digest: sha256:178598e51a26abbc958b8a2e48825c90bc22e641de3d31e18aaf55f3258ba93b
sha256(response body) = 1e64de7b237bd224bf54dd3c4364143201b5f8e02810781357b286be7045512f   ← DOES NOT MATCH
```

For a **signed schema-1** manifest the digest is over the *canonicalised, signature-stripped*
payload. I reproduced the canonicalisation: the JWS `protected` header base64url-decodes to
`{"formatLength":20127,"formatTail":"Cn0="}`, and
`sha256(body[:20127] + b64url_decode("Cn0="))` = `178598e51a…` — the advertised digest,
exactly. So a client that recomputes `sha256(body)` and compares will **reject a manifest the
registry served correctly**. The right handling is not a JWS implementation (D3 §3.3 is right
to refuse that); it is to detect schema 1 *before* the digest check and fail with the
schema-1 message. **Order matters: classify, then verify.** D3 §2.10's condensed trace does
[4] verify-digest before any classification step, which is the wrong order for this one case.

## 3.4 Steps 6–8 — the blob redirect — **CORRECTED: D3's `[F]` HIGH redirect quote is not in the spec**

D3 §2.7 states, tagged `[F]` HIGH:

> distribution-spec: "Registries MAY respond to any request with a redirect per RFC 9110;
> clients SHOULD follow such redirects."

**That sentence does not exist.** The string `redirect` appears **zero times** in
`distribution-spec@v1.1.1/spec.md` (859 lines) and zero times in `content-negotiation.md`. I
grepped case-insensitively for it and for `RFC 9110`; the only `RFC 9110` reference in the
pull section is the `Range` one.

The closest the spec comes is in the **push** flow, where `<location>` "does not necessarily
need to be provided by the registry itself … for example, in the case of a signed URL from
some cloud storage provider" (`spec.md#L319`). That sanctions off-registry URLs for uploads,
not 307s on `GET /v2/.../blobs/<digest>`.

**Verdict: CORRECTED — retag `[F]` HIGH → `[C]` HIGH.** Blob redirection to a CDN is universal
registry convention (Hub → CloudFront, ECR → S3, GAR → Google front-ends) and we must support
it, but it is **not** a spec requirement, and nothing in the spec obliges a client to follow
it. Practically this changes nothing we build; it changes what we may claim, and the contract
says a wrong `[F]` HIGH is the most expensive thing in the corpus.

Everything else about the redirect **reproduces exactly**, seven weeks on. `[O]` HIGH,
`library/alpine` amd64 layer `sha256:7b229517ed…` (3 853 109 bytes — note this has changed
from D3's `55afa1ec…`/3 846 391 because `alpine:latest` was rebuilt):

```
GET /v2/library/alpine/blobs/sha256:7b229517ed…        HTTP/2 307
location: https://production.cloudfront.docker.com/registry-v2/.../data
          ?Expires=1792805992&Signature=…&Key-Pair-Id=K31B9XRZB58O8Z
```

| Test | Result |
|---|---|
| Does `curl -L` forward `Authorization` cross-host? | **No.** Verbose trace shows the second request carries only `Host: production.cloudfront.docker.com` — no `Authorization`. |
| Does the presigned URL work with **no** `Authorization` at all? | **Yes.** `206`, `server: AmazonS3`. The bearer token is genuinely useless there. |
| Does forwarding `Authorization` to CloudFront **break** anything? | **No.** `206`, `x-cache: Hit from cloudfront`. It works fine. |
| `curl --location-trusted` (forces forwarding) | `307` → `206`. Works. |
| `Range: bytes=1000-1999` on the **`/v2/` URL**, following the redirect | `307` → `206`, `content-range: bytes 1000-1999/3853109`, `accept-ranges: bytes` |
| `HEAD` on the blob | `200` direct from `registry-1.docker.io`, **no redirect**, `content-length: 3853109`, `docker-content-digest`, `accept-ranges: bytes` |

**This sharpens D3's rule 1 in an important way.** D3 says "Do not forward the `Authorization`
header across the redirect" and justifies it with *"the bearer token is useless there and
leaking it to a third-party host is a credential disclosure."* The first half is true but
misleading as a *reason*: forwarding the header **does not break the request**. An implementer
who tests "does it still work with the header?" will get `206` and conclude the rule is
cargo-cult. The rule has exactly one justification and it should be stated alone:

> **`[R]` HIGH (V3, restated).** Never forward `Authorization` across a cross-origin redirect,
> **for the single reason that it discloses a bearer credential to a third party**. It is not
> a functional requirement — it works either way — which is precisely why it must be enforced
> by policy (`reqwest`'s default cross-origin stripping) and never "fixed" by someone
> debugging an unrelated 403. Add a test that asserts the header is absent on the second hop.

D3's rules 2 (presigned URLs expire; re-request `/v2/…` for a fresh redirect) and 3 (digest
verification is unconditionally the client's job) are **CONFIRMED**. Rule 3 now has the
stronger spec backing from §3.3 above, plus the CDN observation: the CloudFront response
carries no `Docker-Content-Digest`, only an S3 MD5 `ETag`, so there is nothing else to trust.

On resume: D3 says a resumed download "must re-request the `/v2/.../blobs/<digest>` URL to get
a fresh redirect, not reuse the stored CDN URL." **CONFIRMED, and it is also the easy path** —
a `Range` header on the `/v2/` URL rides through the 307 untouched and yields a `206` from the
CDN. So resume needs no special-casing at all: always request `/v2/`, always set `Range`, let
the redirect happen. There is no reason to ever persist a CDN URL.

`[F]` HIGH — `Range` support: **CONFIRMED verbatim** at `distribution-spec@v1.1.1/spec.md#L202`:
"A registry SHOULD support the `Range` request header in accordance with RFC 9110." Note
SHOULD, not MUST — a registry may refuse, so resume must degrade to restart-from-zero, not
fail.

## 3.5 The step D3's trace does not have: **manifest** redirects across an auth boundary

`[O]` HIGH, 2026-10-24 — `registry.k8s.io` 307-redirects **manifest** requests, not just blobs:

```
GET https://registry.k8s.io/v2/pause/manifests/3.9
HTTP/2 307
location: https://us-east5-docker.pkg.dev/v2/k8s-artifacts-prod/images/pause/manifests/3.9?rid=…
```

The redirect target is a **different registry with a different auth realm and a different
repository name**. A client that follows it with the `registry.k8s.io` token, or that assumes
the repository name in the URL still matches what it asked for, is wrong on both counts. D3's
model — redirects happen on blobs, to a dumb CDN, with a presigned URL — does not cover this.

> **`[R]` MED (V3, new).** Treat a 3xx on a manifest request as *"restart the request at the
> new URL, from the top of the auth flow"*: drop the old `Authorization`, re-run the
> 401/challenge/token dance against the new origin, keep the `Accept` list. Tradeoff: a redirect
> loop guard is now required (cap at 5 hops). Cost is maybe 20 lines; without it,
> `registry.k8s.io` — the canonical source for `pause`, which any CRI-adjacent work needs — is
> simply unreachable.

## 3.6 Why the registries disagree at all — the spec says so itself

`distribution-spec@v1.1.1/content-negotiation.md` is the document `spec.md#L173` points to for
`Accept` handling. Its entire body is:

```
## Content Negotiation

TODO - Please see issue #212.
```

followed by a descriptive table of what Docker Hub *used to* do — whose first three rows are
`<not present>` → `manifest.v1+prettyjws`, `*/*` → `manifest.v1+prettyjws`,
`application/json` → `manifest.v1+prettyjws`, each annotated *"for backward compatibility with
old, non-v2 clients"*, and two rows carrying ⚠️ markers noting the behaviour is internally
inconsistent.

**OCI has never specified content negotiation.** That is the root cause of everything in
Part 2: Hub ignores `Accept`, ghcr.io and GAR 404, quay.io converts — and all three are
conformant, because there is nothing to conform to. The quay.io behaviour I measured in §2.2
(no `Accept` → schema 1) is *this table, still implemented, in 2026*. Any plan that assumes
registries will converge here should be discounted accordingly.

## 3.7 Verdict table for Part 3

| D3 claim | Verdict |
|---|---|
| §2.1 short-name resolution is four client-side conventions | **CONFIRMED** (pinned to `distribution/reference@v0.6.0`); two parser cases missing |
| §2.1 `[F]` `<name>`/tag grammar | **CONFIRMED** verbatim; add "`<reference>` MUST be a digest or a tag, MUST NOT be any other format" |
| §2.2 `/v2/` → 401 + `WWW-Authenticate`, no `scope` on the ping | **CONFIRMED**; add `mcr.microsoft.com` returns 200 with no challenge |
| §2.2 `[O]` ghcr.io placeholder scope; synthesize scope client-side | **CONFIRMED**, second witness (`public.ecr.aws` → `scope="aws"`) |
| §2.2 `[R]` skip `GET /v2/`, handle 401 lazily | **CONFIRMED and strengthened** |
| §2.3 token fetch, `token`/`access_token` duplication, `expires_in: 300` | **CONFIRMED** |
| §2.3 "`exp - nbf = 600s` … they disagree" | **CORRECTED** — no disagreement; `nbf = iat − 300` (skew), `exp = iat + 300` |
| §2.3 scope string `repository:library/alpine:pull` | **CONFIRMED** verbatim from the live challenge |
| *(new)* malformed scope → `200` + empty `access` claim | **new finding**, three variants |
| §2.4 `[F]` "MUST contain `Docker-Content-Digest`" | **CORRECTED** — spec says SHOULD, and "clients SHOULD NOT depend on them" |
| §2.4 `[R]` treat the digest header as a hint, always recompute | **CONFIRMED** — the spec permits only ignore-or-verify |
| *(new)* schema 1: `sha256(body) ≠ Docker-Content-Digest` | **new finding** — classify before verifying |
| §2.7 `[F]` "Registries MAY respond to any request with a redirect per RFC 9110" | **CORRECTED** — no such text; retag `[F]`→`[C]` |
| §2.7 `[R]` do not forward `Authorization` across the redirect | **CONFIRMED as policy, CORRECTED as reasoning** — forwarding works fine; the reason is disclosure alone |
| §2.7 presigned URL expires; re-request `/v2/` | **CONFIRMED**, and `Range` rides the redirect, so resume needs no CDN-URL state |
| §2.7 `HEAD` on a blob does not redirect | **CONFIRMED** |
| §2.7 `[F]` `Range` SHOULD be supported | **CONFIRMED** verbatim (SHOULD — degrade to restart) |
| §2.9 nonexistent repo → `401`, not `404` | **CONFIRMED** |
| *(new)* manifest requests can 307 across an auth boundary | **new finding** (`registry.k8s.io`) |

---

# Part 4 — Media types and manifest versions (D3 §3) — **one headline claim FALSIFIED**

## 4.1 The complete current set — **CONFIRMED**

`image-spec@v1.1.1/media-types.md` lists exactly nine current types and three deprecated:

```
application/vnd.oci.descriptor.v1+json
application/vnd.oci.layout.header.v1+json
application/vnd.oci.image.index.v1+json
application/vnd.oci.image.manifest.v1+json
application/vnd.oci.image.config.v1+json
application/vnd.oci.image.layer.v1.tar
application/vnd.oci.image.layer.v1.tar+gzip
application/vnd.oci.image.layer.v1.tar+zstd
application/vnd.oci.empty.v1+json
--- deprecated, "not recommended for future use" ---
application/vnd.oci.image.layer.nondistributable.v1.tar[+gzip|+zstd]
```

D3 §3.1's table is **CONFIRMED** against this. Two additions worth making:

- D3's table lists `…foreign.diff.tar.gzip` (Docker's name) and says "reject with a clear
  message". The **OCI** equivalents are the three `nondistributable` types, and the spec marks
  them **deprecated** at v1.1. Our reject-list should match all four names, not just Docker's.
- D3 does not list `application/vnd.oci.descriptor.v1+json` or
  `application/vnd.oci.layout.header.v1+json`. Neither appears on the wire; the second is the
  `oci-layout` file's type and matters only if we write an OCI layout, which D3 §12.1 puts in
  the MVP. Minor, but the MVP column should name it.

**`media-types.md`'s own conflict-resolution rule is directly relevant and D3 §3.4 did not
cite it** (`image-spec@v1.1.1/media-types.md`, "Media Type Conflicts"):

> - Implementations that do not have an expected media type for the blob SHOULD respect the
>   returned media type.
> - Implementations that have an expected media type which matches … SHOULD respect the
>   matched media type.
> - Implementations that have an expected media type which does **not** match SHOULD:
>   **respect the expected media type if the blob matches the expected digest** (and MAY warn);
>   return an error if the blob does not match the expected digest; return an error if they do
>   not have an expected digest.

D3 §3.4's `[R]` HIGH resolution order — in-document `mediaType` → `Content-Type` header →
structural sniff — is **CONFIRMED as spec-aligned**, and now has a citation it was missing.
The spec's phrasing is sharper than D3's and should be adopted: **the descriptor's expected
media type wins over the header whenever the digest checks out.** That is a better rule than
"the document is what was hashed", because it also covers the blob case where there is no
in-document `mediaType` at all.

## 4.2 **Is Docker schema 1 dead? — the Hub half of D3 §3.3 is FALSE.**

D3 §3.3 states, `[C]` HIGH:

> **Yes, for our purposes.** Docker Hub stopped serving Schema 1 manifests in November 2024
> (Docker announcement "Docker Hub Registry: deprecating Docker Image Manifest V2, Schema 1");
> Docker Engine 27 removed the ability to *push* schema 1 and Engine 28 removed schema-1 pull
> support entirely…

### The daemon half — **CONFIRMED, with corrected version numbers**

Pinned source, `moby/moby@docker-v29.9.2` (tag object `d7dfd6629b22aed77600050ab53695149b8c550d`;
Engine v29.9.2 released 2026-10-20, so this is current):

- `daemon/internal/distribution/pull_v2.go#L413-414` and `#L761-762`:
  `case MediaTypeDockerSchema1Manifest, MediaTypeDockerSchema1SignedManifest: return … DeprecatedSchema1ImageError(ref)`
- `daemon/internal/distribution/errors.go#L230-L235`:
  ```go
  msg := "Docker Image Format v1 and Docker Image manifest version 2, schema 1 support has been removed."
  msg += " Suggest the author of " + ref.String() + " to upgrade the image to the OCI Format or Docker Image manifest v2, schema 2."
  msg += " More information at https://docs.docker.com/go/deprecated-image-specs/"
  ```
- The vendored `github.com/docker/distribution/manifest/schema1` package, still present at
  `v28.3.1` and `v28.5.1`, is **gone** from the `v29.9.2` tree. The only remaining `schema1`
  paths are `Microsoft/hcsshim/internal/hcs/schema1`, which is unrelated (Windows HCS).

**Correction to the versions.** Docker's own deprecation page gives: deprecated **v19.03**,
disabled by default **v26.0**, removed **v28.2**. D3's "Engine 27 removed push, Engine 28
removed pull" is close but not what Docker documents, and the page says nothing at all about
push versions. Use **v26.0 disabled-by-default, v28.2 removed**.

### The Hub half — **FALSIFIED. Docker Hub serves schema 1 today, 2026-10-24.**

Two independent witnesses, both with the **full modern containerd `Accept` list** that does
*not* include any schema-1 media type:

```
GET /v2/docker/whalesay/manifests/latest
Accept: …oci.image.index.v1+json, …oci.image.manifest.v1+json,
        …docker.distribution.manifest.list.v2+json, …docker.distribution.manifest.v2+json
HTTP/2 200
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws      ← NOT in Accept
content-length: 20401
→ schemaVersion: 1, fsLayers: 11, signatures: 1
```
```
GET /v2/library/ubuntu/manifests/10.04        (an OFFICIAL library image)
same Accept
HTTP/2 200
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
content-length: 7322
docker-content-digest: sha256:f6695b2d24dd…
→ schemaVersion: 1, fsLayers: 4, signatures: 1
```

And on **quay.io**, schema 1 is the *default* response when `Accept` is omitted (Part 2.2):
`quay.io/podman/stable:latest` → `application/vnd.docker.distribution.manifest.v1+json`.

**Verdict: CORRECTED.** The November 2024 Docker announcement D3 cites was about Hub ceasing
to **accept schema-1 pushes**, not about ceasing to **serve** them. D3 conflated the two.
Schema 1 is dead in the Docker *daemon* and alive in the Docker *registry* — and on quay.io it
is the no-`Accept` default. The practical consequence is concrete and checkable:
**`docker pull docker/whalesay` and `docker pull ubuntu:10.04` fail on Docker Engine ≥ 28.2**,
because Hub serves a manifest the daemon refuses. The image is not gone; the client dropped it.

### What survives, and it is the important part

D3 §3.3's `[R]` HIGH — **do not implement schema 1; detect `fsLayers` in the parsed JSON and
emit "re-push this image with a modern client"; a 5-line detection that saves a 500-line
parser** — is **CONFIRMED and is exactly what Docker itself does**, down to the wording of the
error. Its three stated reasons all hold; I verified reason (b) directly in Part 3.3 by
reproducing the JWS canonicalisation (`formatLength`/`formatTail`).

But the *framing* must change, and it changes a priority:

> **`[R]` HIGH (V3, replaces D3 §3.3's framing).** Schema-1 detection is not a defensive
> nicety for "some ancient private registry image". It is **MVP-mandatory**, because the two
> registries we will test against both serve schema 1 today — Hub for old tags, quay.io for
> *any* request with a narrow or absent `Accept`. Without the detection, those responses hit
> the digest check (which fails for `+prettyjws`, Part 3.3) or the index/manifest classifier
> (which finds neither `manifests` nor `layers`) and produce an incomprehensible error. Order
> the pipeline **classify → reject schema 1 → verify digest → parse**, and add
> `docker/whalesay:latest` to the test corpus as the schema-1 fixture. ~10 lines.

This belongs on D3 §12.2's MVP list of seven, which currently does not mention schema 1 at all.

## 4.3 zstd — **WEAK-EVIDENCE for "permitted but not served"; defer is still right**

- **Permitted:** `image-spec@v1.1.1/media-types.md#L12` registers
  `application/vnd.oci.image.layer.v1.tar+zstd`. Not in question.
- **Produced:** BuildKit can emit it (`--output compression=zstd`).
- **Served:** I scanned the amd64 layer media types of seven images across three registries —
  `library/alpine`, `library/nginx`, `library/postgres` (Hub), `ghcr.io/astral-sh/uv`,
  `quay.io/podman/stable`, `quay.io/prometheus/node-exporter`, plus `hbenali/zstd` (a Hub repo
  whose *name* promises zstd). **Every one is `+gzip`** —
  `application/vnd.oci.image.layer.v1.tar+gzip` on five, Docker's
  `application/vnd.docker.image.rootfs.diff.tar.gzip` on `prometheus/node-exporter`.
- **Consumed:** `moby@docker-v29.9.2` vendors `klauspost/compress/zstd` but has **no
  non-vendor path containing `zstd`** and no `zstd` reference in
  `daemon/internal/distribution/pull_v2.go`.

**Verdict: WEAK-EVIDENCE.** Seven images is a sample, not a census, so I cannot say "no
registry serves zstd". What I can say is the operationally useful thing: **zstd is nowhere in
the default path of any image a new engine would first be pointed at.** D3 §11's "safe to
defer: zstd" and §12.1's placement of `tar+zstd` in the *Intermediate* column are both
**CONFIRMED** as correctly weighted.
*The test that would settle it:* push a zstd-compressed image to Hub/ghcr/quay with
`docker buildx build --output type=image,compression=zstd,push=true` and re-read the manifest.
That is a **push**, so it is out of scope here and out of MVP scope anyway.

D3 §3.2's `[R]` HIGH — accept unknown layer types by **suffix match** (`+gzip`, `+zstd`, bare
`tar`) — is **CONFIRMED** as the right call, and it is what makes deferring zstd free: the
classifier already routes `+zstd` correctly the day we link a decoder.

## 4.4 Platform matching — **CORRECTED. D3's rule is wrong for `arm64`, and the spec has no algorithm.**

### What the spec says — almost nothing

`image-spec@v1.1.1/image-index.md`:

- `platform` is **OPTIONAL**; "This property SHOULD be present if its target is
  platform-specific." (D3 §2.5's `[F]` MED is **CONFIRMED**.)
- `architecture` and `os` are **REQUIRED** *within* the platform object.
- `os.version` OPTIONAL — "Implementations **MAY refuse** to use manifests where `os.version`
  is not known to work with the host OS version."
- `os.features` OPTIONAL — only `win32k` is standardised, and only for `os: windows`;
  elsewhere "values are implementation-defined".
- `variant` OPTIONAL.
- `features` — **RESERVED**, do not emit, do not match on.
- And the only matching guidance in the entire spec:
  > **"If multiple manifests match a client or runtime's requirements, the first matching
  > entry SHOULD be used."**

**That is the whole algorithm.** There is no spec definition of "match". D3 §2.5 correctly
presents its rules as `[R]`, but the document should state plainly that the spec is silent —
otherwise a reader assumes there is a standard to be checked against.

**CORRECTION to D3 §2.5's `[F]` HIGH on GOARCH/GOOS.** D3 says image-index.md "normatively
references the Go values". It does not — it says *"Image indexes **SHOULD** use, and
implementations **SHOULD** understand, values listed in the Go Language document for
`GOARCH`"* (and the same for `GOOS`). SHOULD, not MUST. An index with a non-Go `architecture`
is spec-legal and we must not hard-error on it; we must fail to match it and say so.

### The Platform Variants table — and the trap in D3's rule (c)

`image-index.md`, "Platform Variants":

| ISA/ABI | `architecture` | `variant` |
|---|---|---|
| ARM 32-bit | `arm` | `v6`, `v7`, `v8` |
| ARM 64-bit | `arm64` | `v8`, `v8.1`, … |
| POWER8+ | `ppc64le` | `power8`, `power9`, … |
| RISC-V | `riscv64` | `rva20u64`, … |
| **x86-64** | **`amd64`** | **`v1`, `v2`, `v3`, …** |

D3 §2.5's `[R]` HIGH rule (c) is: *"if `variant` is present it must match, with the `arm`
special cases (`arm/v7` etc.) deferred until we care."*

**That rule is wrong twice, and one of them bites on our own architecture.**

1. **`amd64` has variants.** `v1`/`v2`/`v3` are the GOAMD64 microarchitecture levels, and they
   are in the spec's table. Our host platform is `{os: linux, architecture: amd64}` with no
   variant. Under rule (c) — "if variant is present it must match" — an index entry
   `{linux, amd64, v2}` is **rejected**, and an image published only at v2/v3 yields
   "no matching platform". This is not hypothetical-arm territory; it is the architecture D3
   declares as our target.
2. **`arm64/v8` is the mainstream aarch64 entry, not a deferred edge case.** `[O]` HIGH — the
   live `alpine:latest` index I pulled today publishes its aarch64 image as literally
   `linux/arm64/v8`:

   ```
   real  linux/amd64        real  linux/arm/v6     real  linux/arm/v7   real  linux/arm64/v8
   real  linux/386          real  linux/ppc64le    real  linux/riscv64  real  linux/s390x
   + 8 × ATTEST unknown/unknown
   ```
   (16 entries, 8 real + 8 attestations — D3 §2.5's count **CONFIRMED** exactly, seven weeks
   on, including the `unknown/unknown` attestation convention.)

   A client asking for bare `arm64` and applying rule (c) literally finds **zero** matches for
   alpine on aarch64.

### What Docker/containerd actually do — normalise first, then rank

Pinned: `containerd/platforms@v1.0.0-rc.1`.

**`database.go`, `normalizeArch(arch, variant)`** — this runs on *both* sides before any
comparison:

| input | normalised |
|---|---|
| `arm64` + `8` / `v8` / `v8.0` | `arm64` + **`""`** |
| `arm64` + `9` / `9.0` / `v9.0` | `arm64` + `v9` |
| `aarch64` | `arm64` |
| `amd64` + `v1` | `amd64` + **`""`** |
| `x86_64`, `x86-64` | `amd64` |
| `i386` | `386`, variant cleared |
| `arm` + `""` or `7` | `arm` + **`v7`** |
| `arm` + `5`/`6`/`8` | `arm` + `v5`/`v6`/`v8` |
| `armhf` / `armel` | `arm/v7` / `arm/v6` |

**This is the direct answer to "arm64/v8 vs arm64": they are the same platform.** `v8`
normalises to the empty variant, so bare `arm64` matches `arm64/v8` after normalisation and
only after it. Likewise `amd64/v1` ≡ `amd64`. Note `arm` with **no** variant defaults to
**v7**, not v6 — a non-obvious default that costs a wrong image if guessed.

**`platforms.go`, `matcher.Match`** is then exact equality:

```go
return m.OS == normalized.OS &&
    m.Architecture == normalized.Architecture &&
    m.Variant == normalized.Variant &&
    m.matchOSVersion(platform)
```

**`compare.go`, `platformVector`** is what "what does Docker do when nothing matches exactly"
actually means. The default resolver does not use bare `Match`; it builds a *ranked
compatibility vector* from the host platform and takes the best available:

- `amd64/vN` → `vN`, `v(N-1)`, … `v1`, **and then `386`**. An amd64 host will run a 386 image
  if no amd64 manifest exists.
- `arm/vN` → `vN`, `v(N-1)`, … down to `v5`.
- `arm64` with empty variant is **treated as `v8`** and expanded over `v8.x` descending.

So the honest summary: **the spec says "first matching entry SHOULD be used" and defines
neither matching nor ordering; containerd defines both, with normalisation plus a
downgrade-tolerant ranked vector; and that is the de facto standard every other client copies.**

> **`[R]` HIGH (V3, replaces D3 §2.5 rule (c)).** Implement `normalize(os, arch, variant)`
> first — port `containerd/platforms@v1.0.0-rc.1/database.go#normalizeArch` directly, it is
> ~25 lines of `match` — and compare normalised triples. Do **not** write "if variant is
> present it must match" against a raw index entry; that rejects `alpine`'s aarch64 image and
> any `amd64/v2` image. Keep D3's rules (a) drop `unknown/unknown`, (b) require `os == linux`,
> and (d) fail with the list of platforms on offer — all three are **CONFIRMED good**, and (a)
> is confirmed still necessary by today's 8-of-16 attestation count. Defer the ranked
> downgrade vector (`amd64 → 386`, `arm/v7 → v6 → v5`): it is a nicety for hosts whose image
> is missing, and we are amd64-first. Tradeoff: ~25 lines now instead of 3, to avoid a bug
> that only shows up the first time someone runs us on a Raspberry Pi or an Ampere box and
> gets "no matching platform" for `alpine`.

## 4.5 OCI vs Docker schema 2 — **CONFIRMED**

D3 §3.2's five deltas all hold against `image-spec@v1.1.1` and
`distribution/distribution@v2.8.3/docs/spec/manifest-v2-2.md` (the schema-2 reference
`media-types.md`'s compatibility matrix itself points to). Specifically:
in-document `mediaType` required by Docker and OPTIONAL in OCI (**CONFIRMED** — the OCI
`manifests[]` entry text even says *"An encountered `mediaType` that is unknown to the
implementation MUST NOT generate an error"*, which is a stronger statement of D3's
forward-compatibility point than D3 makes); `subject`/`artifactType`/`application/vnd.oci.empty.v1+json`
are OCI-only additions (**CONFIRMED**, all three present in v1.1.1); layer and config media-type
renames are format-identical (**CONFIRMED**); Docker index entries may carry
`platform.os.version`/`os.features` (**CONFIRMED** — and so may OCI ones; the field is in
`image-index.md`, it is not Docker-only. Minor correction to D3's delta 5: the *fields* are
OCI-standard, only their *Windows use* is a Docker-world convention).

D3 §3.2's `[R]` HIGH — one permissive struct with `Option<_>` fields plus an explicit
classifier, not `#[serde(untagged)]` — is **CONFIRMED** as the right shape, and Part 2 makes it
load-bearing rather than defensive: on quay.io the document you get is a function of your
`Accept` header, so the parser must be prepared for any of the four (five, with schema 1).

---

# Part 5 — §12 OCI conformance targets — **CONFIRMED with one significant omission**

## 5.1 Spec versions — **CONFIRMED, nothing moved**

The contract warns that seven weeks of drift can move a `[C]`. I checked all three release
feeds on 2026-10-23:

| Spec | D3/D2 recorded | Current latest | Moved? |
|---|---|---|---|
| runtime-spec | v1.3.0 (2025-11-04) | **v1.3.0** (2025-11-04) | **no** |
| image-spec | v1.1.1 (2025-03-03) | **v1.1.1** (2025-03-03) | **no** |
| distribution-spec | v1.1.1 (2025-01-29) | **v1.1.1** (2025-01-29) | **no** |

All three are unchanged, and each is the newest non-prerelease. **CONFIRMED.** D3/D2's version
record needs no update.

What *did* move in those seven weeks is **Docker Engine: 29.9.2** (released 2026-10-20). D2's
host table records 28.3.1 on `host-a`, which is correct as an `[O]` about that host, but
any `[C]` reasoning about "current Docker" should be checked against the v29 line — and in
Part 4.2 it mattered (the vendored schema-1 package is gone at v29 and present at v28).

## 5.2 The conformance suites that actually exist — **CONFIRMED**

**`opencontainers/distribution-spec/conformance` at `v1.1.1`** — 13 files, Ginkgo-based, built
with `go test -c` into a `conformance.test` binary, driven by environment variables
(`OCI_ROOT_URL`, `OCI_NAMESPACE`, `OCI_USERNAME`, `OCI_PASSWORD`, `OCI_CROSSMOUNT_NAMESPACE`)
and four workflow switches: `OCI_TEST_PULL`, `OCI_TEST_PUSH`, `OCI_TEST_CONTENT_DISCOVERY`,
`OCI_TEST_CONTENT_MANAGEMENT`. It emits `junit.xml` and `report.html`.

D3 §12's `[F]` HIGH description is **CONFIRMED and precise**. Two sharpenings:

- It tests a **live registry**, and most of it needs **write credentials and a writable
  namespace**. It is not merely "not a client test" — it is structurally unable to be one.
- Therefore **nothing in D3 §12.1's Distribution-Spec column is testable by this suite at
  any maturity level**, including the Mature row. If we ever ship a registry, it applies.
  As a puller, it never does.

**`opencontainers/image-spec` validation at `v1.1.1`** — ships
`schema/{config-schema,content-descriptor,defs-descriptor,defs,image-index-schema,image-layout-schema,image-manifest-schema}.json`.
D3's "JSON Schemas for every document type" is **CONFIRMED** (there is no layer schema because
a layer is a tar, not JSON).

**But `opencontainers/image-tools` — the `oci-image-tool` validator CLI — is ARCHIVED**
(last push 2023-03-09). So there is no maintained OCI reference validator binary for images;
there are only the schemas and the Go structs in `specs-go/`. D3 does not claim otherwise, but
anyone planning §12.1's "validate against the JSON schemas in CI" should know the only
supported route is a JSON-Schema library, not an OCI tool.

And carry Part 1.3 forward: the **runtime-spec** schema's top-level `required` is
`["ociVersion"]` alone, so runtime-config schema validation is a shape check, not a
correctness gate.

## 5.3 **The omission: `opencontainers/runtime-tools` tests exactly what we are building**

D3 §12 says, `[F]` HIGH: *"There is a real conformance suite: `distribution-spec/conformance`
… and `image-spec` ships JSON Schemas … **Neither tests a *client*.** … There is no badge to
earn on the client side."*

That is true of the two suites named, and it leads to a correct conclusion for the *image and
registry* halves of our work. **But it skips the third suite, and the third suite tests a
runtime — which is the part of this project that has the most MUSTs to satisfy.**

`opencontainers/runtime-tools` (commit `bd3b0bd00f44`, **2026-10-17 — six days ago**;
not archived; 81 open issues) contains:

- `validation/` — **59 test directories** run against a *real runtime binary*:
  `create`, `delete`, `delete_only_create_resources`, `delete_resources`, `kill`,
  `kill_no_effect`, `killsig`, `hooks`, `hooks_stdin`, `hostname`, `default`,
  `config_updates_without_affect`, `linux_cgroups_{blkio,cpus,devices,hugetlb,memory,network,pids}`
  and their `linux_cgroups_relative_*` twins, `linux_devices`, `linux_masked_paths`, … 
- `validate/` — a `config.json` validator that enforces the **prose** rules, not just the schema
- `generate/` — a `config.json` generator useful for building test bundles
- `specerror/` — the spec's MUST/SHOULD statements enumerated as identified error codes

The release feed is misleading: the last tagged release is **v0.9.0 from 2019-03-13**, which
reads like an abandoned project. It is not — development is on `master` and is current.

**Verdict: CORRECTED (omission).** D3's "there is no badge to earn on the client side" holds
for images and registries and is wrong for the runtime. Note this cuts against D3's own
framing but *in our favour*: there is a real, runnable, currently-maintained conformance
target for the runtime core.

> **`[R]` MED (V3, new).** Add a fourth row to D3 §12.1, **Runtime Spec**, with
> `opencontainers/runtime-tools@master`'s `validation/` suite as the yardstick:
> **MVP** = the `default`, `create`, `kill`, `delete`, `killsig`, `kill_no_effect`,
> `delete_only_create_resources` cases (lifecycle and operation semantics — D2 §1.1–§1.2, the
> part with the most MUSTs);
> **Intermediate** = `hostname`, `linux_masked_paths`, `linux_devices`, the
> `linux_cgroups_*` group;
> **Mature** = `hooks`, `hooks_stdin`, `config_updates_without_affect`, the
> `linux_cgroups_relative_*` group.
> Tradeoff: it is a Go test harness, so running it means Go in CI and a Linux box. Pin the
> commit, because the repo has no usable tag. Cost is one CI job; the payoff is that the one
> area where we *do* have external MUSTs to satisfy stops being self-graded.

## 5.4 Target-by-target audit of D3 §12.1

| Target | Testable by an existing suite? |
|---|---|
| **Image / MVP** — parse index+manifest (OCI and Docker), parse config, apply `tar`/`tar+gzip`, verify digests, read/write OCI layout | **Partly.** The JSON Schemas validate documents we *write*; nothing validates our *parsing*. The layout round-trip is testable by the `skopeo`/`podman` check D3 §12.2 already names — which is the strongest client-side test available and should be stated as *the* image-spec conformance proxy. |
| **Image / Intermediate** — `tar+zstd`, annotations, `variant`, JSON-schema CI, podman-consumable layout | **Partly**, same split. The schema-CI item is real for image documents (unlike the runtime config, Part 1.3). |
| **Image / Mature** — write manifests/configs, `subject`/referrers, non-sha256 algorithms | **No suite.** Aspiration. |
| **Distribution / all three rows** | **No suite applies to a client, at any row.** `distribution-spec/conformance` grades registries and needs push credentials. Aspiration — but the right kind: the real test is D3 §12.2's `mydocker run alpine` from a cold store, which is an integration test we own. |
| **Runtime handoff / all rows** | **No suite** — the image→bundle conversion is the deliberate spec gap D3 §1.2 identifies. Genuinely untestable externally, which is precisely why D3's `[R]` to isolate it as a pure function is well-judged. |
| **Storage / all rows** | **No suite.** Correctly framed as ours. |
| *(missing)* **Runtime spec** | **Yes — `runtime-tools/validation`, 59 cases, currently maintained.** See §5.3. |

**Net:** D3 §12's honesty is **CONFIRMED** — of the four columns it lists, three have no
external grader and it says so. The correction is that a fifth column exists and does.

## 5.5 §12.2's seven MVP items — audit

Six of seven hold unchanged. Item 3 is widened by Part 2, and one item is missing.

| # | Item | Verdict |
|---|---|---|
| 1 | `docker.io`/`library/`/`latest` normalisation, ~50 lines | **CONFIRMED**; add the two parser cases in Part 3.1 |
| 2 | Anonymous bearer auth, scope synthesised client-side, ~80 lines | **CONFIRMED**; second witness for the synthesise rule (`public.ecr.aws`); add the token-`access`-claim check (Part 3.2) |
| 3 | Accept-agnostic manifest classification, ~40 lines | **CORRECTED** — needs the *sending* rule too, or ghcr.io and GAR return 404 (Part 2.3) |
| 4 | Docker Schema 2 media types alongside OCI, ~10 lines | **CONFIRMED and upgraded** — mandatory, not free-win; `registry.k8s.io` is Docker-list-only |
| 5 | `unknown/unknown` platform filtering, 5 lines | **CONFIRMED** — still 8 of 16 entries on `alpine:latest` today |
| 6 | Dual hashing + `diff_id` verification, ~20 lines | **CONFIRMED** — reproduced end-to-end in Part 7 |
| 7 | Correct lowerdir ordering, 1 line | *(V4's section — not adjudicated here)* |
| **8** | *(missing)* **Schema-1 detect-and-reject, ~10 lines** | **ADD** — Hub and quay.io both serve schema 1 today (Part 4.2) |

D3's "well under 500 lines of actual decision-making" survives: items 3 and 8 add perhaps 20
lines between them.

---

# Part 6 — Distribution-spec features D3 may have skipped — **mostly covered; three real gaps**

## 6.1 The complete endpoint table (D3 never reproduces it)

`distribution-spec@v1.1.1/spec.md`, "Endpoints" — this is the whole API surface, and it is
worth having in one place:

| ID | Method | Endpoint | Success | Failure |
|---|---|---|---|---|
| end-1 | `GET` | `/v2/` | 200 | 404/401 |
| end-2 | `GET`/`HEAD` | `/v2/<name>/blobs/<digest>` | 200 | 404 |
| end-3 | `GET`/`HEAD` | `/v2/<name>/manifests/<reference>` | 200 | 404 |
| end-4a | `POST` | `/v2/<name>/blobs/uploads/` | 202 | 404 |
| end-4b | `POST` | `/v2/<name>/blobs/uploads/?digest=<digest>` | 201/202 | 404/400 |
| end-5 | `PATCH` | `/v2/<name>/blobs/uploads/<reference>` | 202 | 404/416 |
| end-6 | `PUT` | `/v2/<name>/blobs/uploads/<reference>?digest=<digest>` | 201 | 404/400 |
| end-7 | `PUT` | `/v2/<name>/manifests/<reference>` | 201 | 404/413 |
| end-8a | `GET` | `/v2/<name>/tags/list` | 200 | 404 |
| end-8b | `GET` | `/v2/<name>/tags/list?n=<int>&last=<tag>` | 200 | 404 |
| end-9 | `DELETE` | `/v2/<name>/manifests/<reference>` | 202 | 404/400/405 |
| end-10 | `DELETE` | `/v2/<name>/blobs/<digest>` | 202 | 404/400/405 |
| end-11 | `POST` | `/v2/<name>/blobs/uploads/?mount=<digest>&from=<other_name>` | 201/202 | 404 |
| end-12a | `GET` | `/v2/<name>/referrers/<digest>` | 200 | 404/400 |
| end-12b | `GET` | `/v2/<name>/referrers/<digest>?artifactType=<type>` | 200 | 404/400 |
| end-13 | `GET` | `/v2/<name>/blobs/uploads/<reference>` | 204 | 404 |

**Our entire MVP is end-1, end-2 and end-3.** Three of seventeen. That is worth stating
explicitly in §12.1 — it is the strongest available argument that a pull-only client is a
small, bounded piece of work.

## 6.2 `_catalog` — **D3's omission is correct, and the reason is stronger than D3 says**

`_catalog` appears **zero times** in `distribution-spec@v1.1.1`. It is not an OCI endpoint at
all — it is a Docker Registry v2 convention that OCI declined to standardise. The spec's
content-discovery surface is `tags/list` (end-8a/b) and `referrers` (end-12a/b) only.

`[O]` HIGH, Hub: `GET /v2/_catalog` with an anonymous repository-scoped token →
```
HTTP/2 401 {"errors":[{"code":"UNAUTHORIZED","detail":[{"Type":"registry","Name":"catalog","Action":"*"}]}]}
```
It needs a `registry:catalog:*` scope Hub does not grant anonymously. **Do not implement it,
and do not list it as a deferred feature** — it is not a feature of the spec we are targeting.

## 6.3 Tag list and pagination — **D3 covers it; the spec has a trap D3 does not mention**

D3 §2.9 observed `tags/list?n=5` returning a `Link` header. **CONFIRMED.** The spec adds two
rules that decide whether a pagination loop terminates correctly
(`spec.md#L543-L565`):

- "The response … **MAY return fewer than `<int>` results**, but only when the total number of
  tags … is less than `<int>` **or a `Link` header is provided**."
- "Previous versions of this specification did not include the `Link` header. **Clients
  depending on the number of tags returned matching `n` may prematurely stop pagination** on
  registries using the `Link` header. When available, clients should prefer the `Link` header
  over using the `last` parameter."
- "When `n` is zero, this endpoint MUST return an empty list, and MUST NOT include a `Link`
  header."

So the termination condition is **"no `Link: …; rel=next` header"**, never "fewer than `n`
results". The spec says in so many words that the obvious loop is wrong. D3 §12.1 puts
`tags/list` pagination in the Intermediate column — **CONFIRMED** as the right level — but the
`Link`-not-count rule should be written down now, because it is the kind of thing that gets
implemented wrong once and silently truncates.

## 6.4 `HEAD` on manifests — **CONFIRMED, including the quota finding**

`[O]` HIGH, Hub, 2026-10-24: `HEAD /v2/library/alpine/manifests/latest` →
`200`, `content-type: …oci.image.index.v1+json`, `content-length: 9218`,
`docker-content-digest: sha256:be2656061b…`, no body. The spec sanctions it as end-3 and says
a successful response SHOULD carry `Docker-Content-Digest` and `Content-Length`
(`spec.md#L206-L213`).

**And D3's uncomfortable observation is CONFIRMED: `HEAD` still costs a rate-limit token.**
`ratelimit-remaining` went `99 → 98` across my HEAD/GET pair. So `HEAD` saves bytes, not
quota — exactly as D3 §2.4 says. Its `[R]` HIGH (use `HEAD` for cache validation) is sound,
with the caveat already stated.

One addition: a `HEAD` must carry the **same full `Accept` list** as the `GET`, or ghcr.io and
GAR will 404 it for the reason in Part 2.2 — and a 404 on a cache-validation HEAD is far more
likely to be misread as "the tag was deleted" than a 404 on a GET.

## 6.5 Referrers API — **CONFIRMED live on Hub; D3's deferral is correctly weighted**

`[O]` HIGH, 2026-10-24: `GET /v2/library/alpine/referrers/sha256:c26612752c78…` →
`200`, `content-type: application/vnd.oci.image.index.v1+json`, an index with
**0 manifests**. The API is live and the empty result is correct — `alpine`'s attestations use
the BuildKit index-entry convention (`vnd.docker.reference.type`), not the OCI `subject` field.
D3 §2.9's "OCI 1.1 referrers API is live on Hub" is **CONFIRMED**.

**The fallback tag scheme** (`spec.md#L717-L734`), which D3 never mentions:

```
<alg>-<ref>        e.g.  registry.example.org/project:sha256-aaaa…
```
`<alg>` is the digest algorithm, `<ref>` the `subject` digest's encoded part (≤64 chars). The
client rules are MUSTs:

> "A client querying the referrers API and receiving a `404 Not Found` **MUST** fallback to
> using an image index pushed to a tag described by the referrers tag schema."
> "If the response to the referrers API is a 404, and the tag schema does not return a valid
> image index, the client SHOULD assume there are no referrers."

`[O]` — I checked the fallback tag on Hub (`…/manifests/sha256-c26612752c78…`) → `404`, which
is the correct result for a registry whose referrers API works.

**Verdict:** D3 §11's "safe to defer: the `referrers` API" and §12.1's placement of it in the
Mature column are **CONFIRMED**. Nothing in a pull needs it. Its `[R]` MED trap — "implementing
the `referrers` API before implementing `HEAD`: novel before useful" — is well-aimed. The one
thing to record: *if* we ever implement it, the 404-fallback is a **MUST**, not an optimisation,
so it is two endpoints of work, not one.

## 6.6 Blob mount-from, and push — **out of MVP, and now examined rather than assumed**

- **Cross-repository blob mount** is end-11:
  `POST /v2/<name>/blobs/uploads/?mount=<digest>&from=<other_name>` → `201` if the registry
  mounted it, `202` (a normal upload session) if it declined. It is **push-only** — it exists
  to avoid re-uploading a blob the registry already has in a sibling repository. It has **no
  pull-side equivalent and no pull-side use**. D3 §12.1 lists it in the Mature/push cell,
  which is right.
- **Upload**: monolithic (end-4b, `POST …?digest=`), POST-then-PUT (end-4a then end-6), and
  chunked `PATCH` (end-5) with strict rules — "Chunks MUST be uploaded in order"; an
  out-of-order chunk MUST get `416`; the first chunk's range MUST begin at `0`; each
  subsequent `PATCH` SHOULD use the `Location` from the previous response. Session status is
  end-13 (`GET …/blobs/uploads/<ref>` → `204` + `Range`).
- **`DELETE`** (end-9, end-10) is explicitly optional: "Registries MAY implement deletion or
  they MAY disable it."

**Stated plainly, as the brief asks: push is out of MVP scope, and I examined it rather than
assuming.** Nothing in the push flow leaks into pull except the `Location`-redirect precedent
discussed in Part 3.4. Deferring all of end-4 through end-11 costs us nothing on the pull path
and D3 §12.1's placement of push in the Mature column is **CONFIRMED**.

## 6.7 Docker Hub anonymous rate limits — **DISPUTED, now adjudicated**

D3 §2.3 recorded two numbers that disagree and recommended trusting neither. Both reproduce
exactly, seven weeks on, and the disagreement can now be settled.

`[O]` HIGH, 2026-10-24, anonymous token for `library/alpine`, decoded claims:

```json
"access":[{"type":"repository","name":"library/alpine","actions":["pull"],
  "parameters":{"pull_limit":"100","pull_limit_interval":"21600","registry_region":"us-east-1"}}]
```

`[O]` HIGH, response headers on the manifest `GET` — **four** headers, not the two D3 recorded:

```
ratelimit-limit:       100;w=3600
ratelimit-remaining:    98;w=3600
x-ratelimit-limit:     100;w=3600          ← D3 did not record the x- prefixed pair
x-ratelimit-remaining:  98;w=3600
docker-ratelimit-source: 198.51.100.20
```

**Docker's current published limits** (docs.docker.com, Docker Hub usage/pulls):
**unauthenticated = 100 pulls per 6-hour window**, counted **per IPv4 address or per IPv6 /64
subnet**; authenticated Personal (free) = 200 per 6 hours. The documented header example is
`ratelimit-limit: 100;w=21600`.

**Adjudication: the token claim and the documentation agree (100 / 21600 s). The live response
header's window is wrong.** `w=3600` matches neither. So D3's "trust neither number" can be
tightened to something actionable:

> **`[R]` HIGH (V3, replaces D3 §2.3's "trust neither").** The **limit and window** are
> 100 per 6 hours for anonymous pulls — take them from the token's
> `access[].parameters.pull_limit` / `pull_limit_interval`, which match Docker's docs. The
> response header's `w=` value is unreliable and must not be used to compute a reset time.
> The header's **`ratelimit-remaining` count** is still the live counter and is the only
> thing that tells you how close you are, so surface it in verbose output. For control flow,
> rely on `429` + `Retry-After` with exponential backoff, which is what D3 already
> recommends and which remains correct. Tradeoff: we depend on a JWT-internal claim that is
> not specified anywhere — so treat its absence as "unknown limit", not as an error.

Two additions to D3's model:
- The bucket is **per IPv4 address or per IPv6 /64**, not merely "per source IP". A machine
  with IPv6 shares a bucket with everything else in its /64, which makes D3's CI-behind-NAT
  warning broader than stated.
- `docker-ratelimit-source` echoes the IP Docker is charging. **It is the only way to find
  out which bucket you are in**, and it is a Docker extension in no spec.
- A pull is counted at the **manifest** request, and a `HEAD` counts (§6.4). Layer blob GETs
  do not add to it.

---

# Part 7 — The dual-digest model and ChainID — **CONFIRMED end to end, one tag CORRECTED**

## 7.1 The two digests, reproduced from scratch

D3 §2.8's mechanic is the one I most wanted to break, because everything in the store keys off
it. I re-derived it from the wire with no reference to D3's numbers.
`[O]` HIGH, local macOS, `curl 8.7.1` + `python3` `hashlib`/`zlib`, 2026-10-24,
`library/alpine:latest` → amd64:

```
index          sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a   9218 B
 └ manifest    sha256:c26612752c78b18faacd63c7d50f4592c2e7b0f25e0e20e82bd74e465c57ab0d
    ├ config   sha256:b92841763c5c2140a20eeb2a44e94ebb0e80fd51d9d6b5895ea72fa999525d68    611 B
    └ layer[0] sha256:7b229517ed222597277036d94892173207d01a731ab3a88eeb1de2e963a2c788

sha256(config blob bytes)        = sha256:b92841763c5c…   == manifest.config.digest   ✓  (= the IMAGE ID)
config.rootfs.type               = "layers"
config.rootfs.diff_ids           = ["sha256:75c137aedfb2760f88629236cb7b797630aed0c3d91d626d1d1bb694878fc2ba"]

one pass over the layer body, two hashers, gzip inline:
  compressed   3 853 109 B   sha256:7b229517ed22… == manifest.layers[0].digest        ✓
  uncompressed 8 709 632 B   sha256:75c137aedfb2… == config.rootfs.diff_ids[0]        ✓
```

**Every identity D3 §2.8 asserts is CONFIRMED**, at different digests (the tag was rebuilt
between 2026-09-04 and today — D3's `55afa1ec…`/`34884abb…`/3 846 391 B are stale, as D3
predicted they would be). The distinction holds exactly as stated:

| | compressed-blob digest | `diff_id` |
|---|---|---|
| hashes | the blob **as transferred**, gzip and all | the **uncompressed tar** stream |
| appears in | `manifest.layers[i].digest` | `config.rootfs.diff_ids[i]` |
| who addresses by it | **the registry** — it is the `/v2/<name>/blobs/<digest>` key | **nobody on the wire** |
| what it identifies | one particular *encoding* of a changeset | the changeset's *content*, compression-independent |
| recompress the layer | **changes** | **unchanged** |

D3's `[R]` HIGH — pipeline as
`HTTP body → sha256(compressed) → gzip decoder → sha256(uncompressed) → tar reader → extractor`,
commit nothing until both verify — is **CONFIRMED** as both correct and cheap: I ran exactly
that pipeline in 64 KiB chunks over a 3.85 MB blob with two `sha256` states and one
`zlib.decompressobj`, and it is a dozen lines.

**Which digest our CAS keys on, stated plainly:**

- **Blob store → the compressed digest.** It is what the registry gave us, what the manifest
  references, what `HEAD`/`Range`/resume address, and what the OCI image layout wants at
  `blobs/sha256/<hex>` so that `skopeo`/`podman` can read our store (D3 §12.2's corroborating
  test). It is also the only one we can verify *before* decompressing — i.e. before handing
  attacker-controlled bytes to a decompressor.
- **Extracted-layer store → the ChainID** (derived from `diff_id`s), per §7.3.
- **Both are required and neither substitutes for the other.** The manifest gives us only
  compressed digests; the config gives us only `diff_id`s; nothing on the wire maps between
  them. The mapping is *produced* by the act of downloading — which is why D3 §12.1's
  `diffid-by-digest` index exists at all, and why a puller that stores only one of the two
  can never answer "do I already have this layer?" from a manifest alone.

## 7.2 `diff_ids` ordering — **CONFIRMED verbatim**

`image-spec@v1.1.1/config.md`, `rootfs`:

> - **type** _string_, REQUIRED — MUST be set to `layers`. **"Implementations MUST generate an
>   error if they encounter a unknown value while verifying or unpacking an image."**
> - **diff_ids** _array of strings_, REQUIRED — **"An array of layer content hashes
>   (`DiffIDs`), in order from first to last."**

and `manifest.md`: "The array MUST have the base layer at index 0. Subsequent layers MUST then
follow in stack order."

So **both arrays are base-first** and index `i` of `layers` corresponds to index `i` of
`diff_ids`. D3 §2.10 step [8] (`sha256(gunzip(stream)) must == config.rootfs.diff_ids[i]`, in
manifest order) is **CONFIRMED**.

Two MUSTs worth lifting into the parser, neither of which D3 §0–§3 states:

- `rootfs.type` **MUST** be `layers` and an unknown value **MUST** be an error — one of the few
  places the image spec says MUST-error rather than SHOULD. Cheap, so do it.
- `history` is OPTIONAL and, when present, "ordered from first to last" — but history entries
  include empty layers, so **`history` is not index-aligned with `diff_ids`**. Anyone matching
  them up must skip `empty_layer: true` entries. Worth a sentence somewhere before someone
  writes the naive zip.

## 7.3 ChainID — **CONFIRMED, and the tag is wrong: this is `[F]`, not `[C]`**

D3 §4.4 (V4's section; the brief assigns me the formula) gives:

```
ChainID(0) = DiffID(0)
ChainID(n) = sha256( ChainID(n-1) ++ " " ++ DiffID(n) )    # ASCII, with the "sha256:" prefixes, one space
```

tagged `[C]` HIGH and sourced to `moby/layer/layer.go` (`createChainIDFromParent`) and
containerd's `identity.ChainID`.

**The formula is exactly right. The tag and the source are both wrong.** ChainID is
**normative in the OCI image specification itself**, `image-spec@v1.1.1/config.md`,
§"Layer ChainID" → "Definition":

```text
ChainID(L₀) =  DiffID(L₀)
ChainID(L₀|...|Lₙ₋₁|Lₙ) = Digest(ChainID(L₀|...|Lₙ₋₁) + " " + DiffID(Lₙ))
```

with the spec's own expansion:

```text
ChainID(A|B|C) = Digest(Digest(DiffID(A) + " " + DiffID(B)) + " " + DiffID(C))
```

and the spec's stated reason, which is the one that matters for our store design:

> "We differentiate the changeset `C`, from the order-dependent application `A|B|C` by saying
> that the resulting rootfs is identified by ChainID(A|B|C) … Most importantly, we can easily
> see that `ChainID(C) != ChainID(A|B|C)`."

**And the image-spec ships a reference implementation**: `image-spec@v1.1.1/identity/chainid.go`,
`ChainIDs()`:

```go
parent := digest.FromBytes([]byte(dgsts[0] + " " + dgsts[1]))
```

Since `digest.Digest` stringifies as `"sha256:<hex>"`, the bytes hashed are
`"sha256:<hexA> sha256:<hexB>"` — **prefixes included, exactly one ASCII space, no newline,
no trailing separator** — which is precisely the detail D3's comment calls out.

`[O]` HIGH — I implemented it from the spec text alone and checked it three ways:
`ChainID(A) == DiffID(A)` ✓; `ChainID(A|B) == sha256("sha256:aaa… sha256:bbb…")` ✓;
`ChainID(A|B|C)` equals the fully-expanded nested form above ✓; and for the real single-layer
`alpine`, `ChainID == diff_ids[0]` ✓ (the base case).

> **CORRECTED (for V4 to action in §4.4): retag `[C]` HIGH → `[F]` HIGH**, cite
> `image-spec@v1.1.1/config.md` §"Layer ChainID" as the primary source and
> `image-spec@v1.1.1/identity/chainid.go` as the reference implementation, and keep the
> moby/containerd citations as corroboration only. This is a *strengthening*: D3 §4.4's
> `[R]` HIGH "key extracted layer directories by ChainID, not `diff_id`" currently reads as
> "it is what Docker and containerd do". It is better than that — **it is what the image
> specification defines, for exactly the reason we want it** (`ChainID(C) != ChainID(A|B|C)`:
> the same changeset applied onto different bases is a different rootfs and must not collide).

## 7.4 A failure mode I produced by accident, and what it proves

Mid-verification my anonymous token expired (300 s, §3.2) between fetching the manifest and
fetching the blobs. `curl -sSL -o file` happily wrote the **401 error body** into both blob
files:

```
$ wc -c config.blob layer.blob
157 config.blob     157 layer.blob
$ cat config.blob
{"errors":[{"code":"UNAUTHORIZED","message":"authentication required", … }]}
```

A 157-byte JSON document, written to disk where a 3.85 MB gzip layer was expected, by a
command that **exited 0**. Nothing but the digest check caught it.

This is D3 §2.7's rule 3 ("digest verification is the client's job, unconditionally") and
§2.3's token-expiry warning, demonstrated in one accident. Two things follow that are worth
writing down because they are easy to leave out:

1. **Check the HTTP status before you check the digest, and before you write anything.** A
   401/404 body is a valid JSON document of plausible size; it will fail the digest check, but
   only if you get as far as the digest check, and the error you report should say
   "unauthorized" rather than "digest mismatch".
2. **A mid-pull 401 is the expected case, not an edge case.** With `expires_in: 300` and a
   large image, the token *will* expire during the pull. D3 §2.3's `[R]` already requires
   "handle a mid-pull 401 by re-authenticating and retrying the *same* request once" —
   **CONFIRMED as load-bearing**, not defensive. Our retry must be per-request, not per-pull.

---

# Part 8 — D2 §1.1, §1.2, §1.4, §1.5 against `runtime.md` — **one material `[F]` HIGH error**

Breadth check over the rest of my assigned D2 range, against
`opencontainers/runtime-spec@92249139eea7161e13745abd4cb6d0ea02a3227a/runtime.md` (v1.3.0, 150 lines).

## 8.1 §1.1 the state machine — **CONFIRMED, with a framing correction**

The four statuses and their definitions are **CONFIRMED verbatim** (`runtime.md`, State):
`creating`, `created`, `running`, `stopped`.

Two notes:

- D2 paraphrases `running` as *"the user-specified program is executing (or has executed and
  the runtime cannot tell it exited)"*. The v1.3.0 text is simply **"the container process has
  executed the user-specified program but has not exited (after step 8 in the lifecycle)"**.
  The parenthetical is from an older revision; drop it.
- D2 says *"There is **no** `paused` status in the spec … the spec's four states are a
  *reporting* contract, not an implementation constraint."* The conclusion is right but the
  spec is more permissive than D2 realises: **"Additional values MAY be defined by the
  runtime, however, they MUST be used to represent new runtime states not defined above."**
  So runc's `paused` is not merely tolerated — it is **explicitly sanctioned**. If we ever add
  a status of our own, the only rule is that it must not re-describe one of the four.
- Unrecorded by D2 and worth having: **`pid` is REQUIRED when `status` is `created` or
  `running` on Linux**, and the state document MUST validate against
  `schema/state-schema.json`.

## 8.2 §1.2 the five operations — **CONFIRMED, all five rows**

Every MUST and every error condition in D2 §1.2's table matches `runtime.md` Operations
verbatim, including the "not specifying any command-line APIs" note and the
create/start split ("`process.args` MUST NOT be applied until triggered by the `start`
operation"). **CONFIRMED.**

One MUST NOT D2 omits, from Delete, and it constrains our storage design directly:

> "Deleting a container MUST delete the resources that were created during the `create` step.
> **Note that resources associated with the container, but not created by this container,
> MUST NOT be deleted.**"

That is the spec forbidding exactly the bug where deleting a container removes a shared layer
directory or a bind-mount source. Record it next to the store layout.

## 8.3 §1.1/§1.4 the lifecycle — **CORRECTED: `poststart` failure MUST stop the container**

D2 §1.1's 13-step lifecycle table says:

```
 9. poststart hooks                        fail -> log warning, continue
```

and D2 §1.4's hook table repeats it: *"`poststart` … Failure → **log a warning**, the container
keeps running."* tagged `[F]` HIGH.

`runtime.md` v1.3.0, step 9, verbatim:

> 9. The `poststart` hooks MUST be invoked by the runtime.
>    If any `poststart` hook fails, the runtime MUST **[generate an error](#errors), stop the
>    container, and continue the lifecycle at step 12.**

**Only step 13 (`poststop`) is "log a warning and continue":**

> 13. … If any `poststop` hook fails, the runtime MUST [log a warning](#warnings), but the
>     remaining hooks and lifecycle continue as if the hook had succeeded.

**Verdict: CORRECTED.** D2 got `poststop` right and `poststart` wrong. The
"log a warning, keep running" behaviour for `poststart` is from runtime-spec **1.0.x**; it was
changed so that all of steps 3–9 fail the same way. In v1.3.0, **steps 3, 4, 5, 7 and 9 all
have identical failure semantics**: generate an error, stop the container, jump to step 12.

This is a `[F]` HIGH asserted twice at high confidence, and it is the kind that survives into
code: a runtime that logs and continues on a `poststart` failure is non-conformant, and the
divergence only shows up when someone's notification hook starts failing and their containers
*don't* die the way the spec says they should. D2 §1.4's `[R]` MED (defer hooks to phase 1,
implement `createRuntime` + `poststop` first) is **unaffected** — but when hooks are
implemented, the failure table must be the uniform one.

Corrected table:

| Lifecycle step | Hook | On failure |
|---|---|---|
| 3 | `prestart` (DEPRECATED) | error, stop container, → step 12 |
| 4 | `createRuntime` | error, stop container, → step 12 |
| 5 | `createContainer` | error, stop container, → step 12 |
| 7 | `startContainer` | error, stop container, → step 12 |
| **9** | **`poststart`** | **error, stop container, → step 12** ← *not* "log a warning" |
| 13 | `poststop` | log a warning, lifecycle continues |

Everything else in D2 §1.1's 13-step table is **CONFIRMED verbatim**, including step 2's
"the user-specified program … MUST NOT be run at this time" and "Any updates to `config.json`
after this step MUST NOT affect the container".

## 8.4 §1.4 hook execution namespaces — **CONFIRMED, all six**

D2 §1.4's "Runs in" column matches `config.md@v1.3.0#L561-L588` exactly:

| Hook | `path` resolves in | Executes in |
|---|---|---|
| `prestart` | runtime ns | **runtime ns** |
| `createRuntime` | runtime ns | **runtime ns** |
| `createContainer` | **runtime** ns | **container ns** |
| `startContainer` | **container** ns | **container ns** |
| `poststart` | runtime ns | **runtime ns** |
| `poststop` | runtime ns | **runtime ns** |

Note the asymmetry D2 implies but does not spell out: for `createContainer` the **binary comes
from the host** but **runs inside the container's mount namespace**, while for
`startContainer` both are inside. That split is precisely the structural difficulty D2 §1.4's
`[R]` MED cites for deferring hooks, so the recommendation is **well-founded** — it is harder
than it looks for exactly the reason given.

The stdin claim is **CONFIRMED verbatim** (`runtime.md#L26-L27`): "For hooks executed in the
runtime namespace, it is the pid as seen by the runtime. For hooks executed in the container
namespace, it is the pid as seen by the container."

## 8.5 §1.5 "what the spec does not mandate" — **CONFIRMED, all eight**

I grepped each of D2 §1.5's eight verbatim quotes against `runtime.md` v1.3.0. **All eight are
present.** Error reporting, warning reporting, cross-instance visibility, optional validation,
which `process` properties are applied at create, extra state fields, ID uniqueness-and-reuse,
and the CLI disclaimer — every one is quoted accurately.

D2 §1.5's `[R]` HIGH — *"OCI compatibility is a serialization + lifecycle-semantics contract,
not an architecture"* — is **CONFIRMED** as a fair reading of those eight. It is the
best-supported recommendation in my assigned range.

---

## Corrections that change a recommendation

- **D3 §2.4 / §12.2 item 3 — "Docker Hub ignores `Accept`" is true, but the client rule
  derived from it is half a rule.** ghcr.io returns `404 MANIFEST_UNKNOWN` when `Accept` is
  absent, when it is `*/*`, or when it lacks the stored type; Google Artifact Registry
  (`registry.k8s.io`, `gcr.io`) does the same; quay.io silently converts and returns a
  **different digest per `Accept`**. → **Always send the full four-type `Accept` list on every
  manifest `GET` and `HEAD`, and still classify by `Content-Type`/in-document `mediaType`.**
  Shifts D3 §2.4's `[R]` HIGH and widens §12.2's MVP item 3.
- **D3 §2.4 quotes distribution-spec as "MUST contain … `Docker-Content-Digest`". The spec
  says SHOULD**, and `spec.md#L49-L52` says these headers "are OPTIONAL and clients SHOULD NOT
  depend on them". → A missing digest header is spec-legal; do not build an abort path on it.
  D3's "always recompute" `[R]` is unaffected and is in fact the only compliant option.
- **D3 §2.7's `[F]` HIGH redirect quote does not exist in distribution-spec v1.1.1** (the word
  "redirect" appears zero times). → Retag `[F]` → `[C]`. Blob redirection is universal
  convention, not spec; nothing obliges a client to follow it.
- **D3 §2.7's reason for not forwarding `Authorization` across the CDN redirect is wrong.**
  Forwarding it **works** (`206` from CloudFront). The only reason is credential disclosure to
  a third party. → Restate the `[R]` with that single justification and add a test asserting
  the header is absent on hop 2, or someone will "fix" the policy while debugging.
- **D3 §3.3 "Docker Hub stopped serving Schema 1 in November 2024" is FALSE.** Hub serves
  `application/vnd.docker.distribution.manifest.v1+prettyjws` today for `docker/whalesay:latest`
  **and `library/ubuntu:10.04`**, even against the full modern `Accept` list; quay.io returns
  schema 1 by default when `Accept` is omitted. The daemon half is right (removed in Engine
  **v28.2**, not 27/28 as stated). → **Schema-1 detect-and-reject becomes an MVP item (~10
  lines), not a defensive nicety**; add it as item 8 of §12.2's seven.
- **Schema-1 manifests break naive digest verification**: `sha256(body) ≠ Docker-Content-Digest`
  because the digest is over the signature-stripped canonical form (reproduced via
  `formatLength`/`formatTail`). → Order the pipeline **classify → reject schema 1 → verify
  digest → parse**, not D3 §2.10's verify-then-classify.
- **D3 §2.5's platform rule (c) "if `variant` is present it must match" is wrong for `arm64`
  and for `amd64`.** `alpine:latest` publishes `linux/arm64/v8`; the spec's variant table gives
  `amd64` the variants `v1`/`v2`/`v3`. → **Normalise before comparing** (port
  `containerd/platforms@v1.0.0-rc.1/database.go#normalizeArch`, ~25 lines: `arm64/v8 → arm64`,
  `amd64/v1 → amd64`, `arm/"" → arm/v7`, `aarch64 → arm64`). Rules (a), (b), (d) are confirmed
  good.
- **D3 §2.5's `[F]` HIGH "image-index.md normatively references the Go `GOARCH`/`GOOS` values"
  overstates it** — the spec says SHOULD. → Do not hard-error on a non-Go `architecture`; fail
  to match it and name it in the error.
- **D3 §1.3's bundle definition is wrong on two counts** — it quotes pre-1.1 spec text and
  drops the conditional. `bundle.md@v1.3.0` marks only `config.json` REQUIRED and says the
  rootfs directory is referenced by `root.path` *"if that property is set"*. D2 §1.3's
  "`root` REQUIRED on Linux" is correct. → Rewrite D3 §1.3; D2 needs only the Hyper-V
  footnote (`root` **MUST NOT** be set there, which is why the annotation reads OPTIONAL).
- **The OCI runtime-spec JSON Schema's top-level `required` is `["ociVersion"]` alone** —
  `root`, `process.args`, `process.user`, `mounts[].destination` are all absent. A config of
  `{"ociVersion":"1.3.0"}` validates and is illegal on Linux. → D3 §12.1's "validate against
  the JSON schemas in CI" is a shape check, not a correctness gate, for the runtime spec;
  D2 §1.3's five hard errors must be hand-written.
- **D2 §1.1 and §1.4 both state that a failing `poststart` hook is a logged warning. The spec says the opposite.** `runtime.md@v1.3.0` step 9: a failing `poststart` hook means the runtime MUST generate an error, **stop the container**, and jump to step 12. Only `poststop` (step 13) is log-and-continue. → Steps 3, 4, 5, 7 and 9 share one failure rule; fix D2's two tables before hooks are implemented.
- **D2 §1.3 omits `config.md`'s "The runtime MUST mount entries in the listed order."** → Mount
  order is correctness, not style; do not represent `mounts` in an unordered container.
- **D3 §12 omits `opencontainers/runtime-tools`** (59 `validation/` cases against a real
  runtime, commits as of 2026-10-17 despite a 2019 last tag). → Add a **Runtime Spec** row to
  §12.1; it is the only external conformance grader that applies to anything we build.
  `opencontainers/image-tools` is archived, so the schemas are the only image-side validator.
- **D3 §2.3's rate-limit "they disagree, trust neither" can be settled.** The token claim
  (`pull_limit=100`, `pull_limit_interval=21600`) matches Docker's current docs (100 pulls /
  6 h, per IPv4 or IPv6 **/64**); the response header's `w=3600` matches neither. → Take limit
  and window from the token claim, use `ratelimit-remaining` only as a live counter, and keep
  429 + `Retry-After` backoff for control flow.
- **D3 §2.3's `exp - nbf = 600s` "disagreement" is not one** — `nbf = iat − 300` (clock skew),
  `exp = iat + 300`. → Token lifetime is genuinely 300 s; the `[R]` to cache on `expires_in`
  minus skew stands, and the mid-pull-401 retry is load-bearing (I triggered it accidentally).
- **New: a malformed token `scope` returns `200` with an empty `access` claim**, surfacing one
  request later as a 401 that reads like bad credentials. → Decode the returned JWT and check
  `access[]` grants the repo before using it (~10 lines; proceed on parse failure).
- **New: manifest requests can 307 across an auth boundary** (`registry.k8s.io` →
  `*.pkg.dev`, different realm *and* different repository name). → Treat a 3xx on a manifest as
  "restart at the new URL from the top of the auth flow", with a 5-hop cap.
- **D3 §4.4's ChainID formula is right but tagged `[C]` and sourced to moby/containerd.** It is
  normative in `image-spec@v1.1.1/config.md` §"Layer ChainID", with a reference implementation
  at `identity/chainid.go`. → Retag `[F]` HIGH (V4's section). This strengthens the
  "key layer dirs by ChainID" `[R]` from convention to specification.

## Still unverifiable, and the exact test

- **"No major registry serves `tar+zstd` layers today."** Seven images across Hub/ghcr/quay
  were all `+gzip`; that is a sample, not a census. → Settled by a **push**:
  `docker buildx build --push --output type=image,compression=zstd,name=<repo>:zstd .` against
  Hub, ghcr.io and quay.io in turn, then `GET /v2/<repo>/manifests/zstd` and read
  `layers[].mediaType`. Out of MVP scope (push) and out of scope for a read-only verifier.
- **"Docker Hub's `ratelimit-limit: …;w=3600` window is wrong, not an undocumented second
  bucket."** Token claim and docs both say 21600 s. → Settled only by exhausting the quota:
  from a dedicated IP, `for i in $(seq 1 105); do curl -sS -o /dev/null -w '%{http_code} ' -H "Authorization: Bearer $TOK" -H "Accept: <full list>" https://registry-1.docker.io/v2/library/alpine/manifests/latest; done`
  then poll until the 429 clears and time it. **Do not run this from `host-a`** — it is
  production and shares the egress IP with live traffic; use a throwaway host.
- **"Docker Engine v29.9.2 refuses `docker pull ubuntu:10.04` with
  `DeprecatedSchema1ImageError`."** Proven from pinned source
  (`moby@docker-v29.9.2/daemon/internal/distribution/pull_v2.go#L413`), not from a running
  daemon. → `ssh host-a 'docker pull ubuntu:10.04'` would settle it — **but that is a
  mutating command on live production and must not be run.** A throwaway Linux box with Docker
  ≥ 28.2 settles it instead; `host-a` runs 28.3.1, so even there the expected result is the
  refusal.
- **"`runtime-tools/validation`'s 59 cases are the right MVP/Intermediate/Mature split."** I
  read the directory listing, not the test bodies. → `git clone` at commit `bd3b0bd00f44`,
  `go build ./validation/...`, and run the `default`/`create`/`kill`/`delete` cases against
  `runc` on a Linux host to see which assertions they actually make. Needs a Linux box with Go;
  not possible from macOS.
- **"quay.io's conversion rule is *no OCI type in `Accept` ⇒ schema 1*."** Inferred from five
  `Accept` variants on two repositories. → A full matrix (every subset of the five media types,
  across ≥5 quay repositories, recording `Content-Type` and `Docker-Content-Digest`) would
  confirm the rule rather than the symptom. Runnable from any host with `curl`; I stopped at
  the point where the operational conclusion ("always send the full list") was already forced.
