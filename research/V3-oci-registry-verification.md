# V3 — Adversarial verification: OCI specs, registry & pull

Scope under review: `D3-images-storage.md` §0–§5, §7, §12 (specs, pull trace, media types, layer
format/digests/chain IDs, whiteouts, CAS/store layout, conformance). §6 (extraction hazards),
§8–§11 (OverlayFS, overlay2, progression) belong to another verifier and are **not** covered here,
except where §5's whiteout translation hands a privilege claim to §8.

**Verifier note.** I did not write the source. Every verdict below is either (a) checked against
the primary spec/kernel/man text, or (b) re-run by me from scratch. Where I could not re-run, the
verdict says so.

Verifier environment:

| Tag | What | When |
|---|---|---|
| **LOCAL-V** | macOS workstation, `curl 8.7.1`, anonymous only, no credentials | 2026-09-05 |
| **HOST-A** | `host-a` (`HOST-A`), Ubuntu 24.10, kernel `6.11.0-29`, Docker 28.3.1 — **read-only**, no `docker pull/run/rmi`, nothing under `/var/lib/docker` modified | 2026-09-05 |
| **ROCKY** | `old-host` / `old-host.example.edu` — **NXDOMAIN, unreachable.** Confirmed independently. | 2026-09-05 |

Verdict vocabulary: **CONFIRMED** / **CORRECTED** / **REFUTED** / **OUTDATED** / **UNSUPPORTED**
(cited evidence doesn't establish the claim) / **MISCLASSIFIED** (recommendation dressed as fact,
or a single-host/single-registry observation generalized to `[F]`/`[C]`).

*(This file is appended to as verification proceeds; sections appear in review order, not in
importance order. The summary is at the end.)*

---

## A. The headline claim: "Docker Hub ignores `Accept` entirely" (§2.5, §3.3, §3.4)

### A.1 The four-way `Accept` matrix on `library/alpine:latest` — **CONFIRMED**

Re-run independently, anonymous token, six variants (the document tested four; I added two):

```
TOK=$(curl -s 'https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/alpine:pull' | jq -r .token)
curl -sI -H "Authorization: Bearer $TOK" -H '<Accept variant>' \
  https://registry-1.docker.io/v2/library/alpine/manifests/latest
```

| `Accept:` sent | `content-type` returned | HTTP |
|---|---|---|
| Docker list + Docker manifest **only** (no OCI) | `application/vnd.oci.image.index.v1+json` | 200 |
| OCI index + OCI manifest only | `application/vnd.oci.image.index.v1+json` | 200 |
| `*/*` | `application/vnd.oci.image.index.v1+json` | 200 |
| header entirely absent (`-H 'Accept:'`) | `application/vnd.oci.image.index.v1+json` | 200 |
| **`…manifest.v1+prettyjws` only** (my addition) | `application/vnd.oci.image.index.v1+json` | 200 |
| **`application/x-total-nonsense`** (my addition) | `application/vnd.oci.image.index.v1+json` | 200 |

`docker-content-digest` identical (`sha256:6c1a91f698e4…943f8b`) in all six. My two extra variants
are the strongest form of the test: a request whose `Accept` set is *disjoint* from everything the
registry could serve still gets 200 with an OCI index, not `406`. **The document's claim is not
just confirmed, it is understated** — this is not "Hub ignores the header for these types", it is
"Hub does not implement content negotiation on the manifest endpoint at all".

The derived client rule — **branch on the response `Content-Type`, never on what you requested** —
is therefore sound, and §2.10 step 3 correctly encodes it.

### A.2 …but the *scope* of the claim — **MISCLASSIFIED (tag), CONFIRMED (substance)**

§2.5 is tagged `[O] LOCAL HIGH` and then §3.3's conclusion generalizes to "Docker Hub does not
content-negotiate", full stop. What was measured is: **one registry, the `library/*` namespace,
the manifest endpoint, six tags.** I extended it to `library/nginx`, `library/postgres`,
`library/registry` (below) and got the same, so the generalization holds *for Hub*. But the tag
should be `[O]` scoped to Hub + `[C] MED` for "registries in general", not `[C] HIGH`. The
document does say the rule is "always branch on `Content-Type`", which is the safe rule regardless
— so the recommendation survives even if the observation were narrower. Substance stands; the
confidence tag is one notch hot.

**One thing the document gets wrong by omission:** it frames this as "the belief that Hub
down-converts is dead". The belief was never about *these* media types. Hub's historical
down-conversion was **Schema 2 → Schema 1 for clients that only accepted Schema 1**, a feature of
`distribution` v2 that was removed years ago. The document's test cannot speak to that, because it
never sent a Schema-1-only `Accept`. I did (row 5 above): the response was an OCI index, i.e. no
conversion happens. So the conclusion is right, but the evidence the document offers for it is not
the evidence that settles it — my row 5 is.

### A.3 "`alpine:2.6` still returns `manifest.v1+prettyjws`" — **CONFIRMED**

```
$ curl -sI -H "Authorization: Bearer $TOK" \
   -H 'Accept: …oci.image.index.v1+json,…oci.image.manifest.v1+json,…docker.distribution.manifest.v2+json' \
   https://registry-1.docker.io/v2/library/alpine/manifests/2.6
HTTP/2 200
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
```

Also with `Accept: application/vnd.docker.distribution.manifest.v2+json` **alone** (the strictest
possible exclusion of Schema 1): still `v1+prettyjws`, digest
`sha256:e9cec9aec697d8b9d450edd32860ecd363f2f3174c8338beb5f809422d182c63`. Body confirmed
`"schemaVersion": 1` with `fsLayers`/`v1Compatibility`, exactly as quoted.

`alpine:3.9` → `application/vnd.docker.distribution.manifest.list.v2+json`. **CONFIRMED.**
I added `alpine:3.19` → `application/vnd.oci.image.index.v1+json`, which usefully brackets the
Docker-list→OCI-index cutover between 3.9 and 3.19 rather than leaving it at "before ~2022".

Consequence for §3.3's "an OCI-only client fails on `alpine:3.9`": **CONFIRMED**, and the
recommendation to read Docker Schema 2 from day one is correct and cheap for the reason given
(the OCI struct is a superset — see §D below, where I checked that against the spec's own
compatibility matrix rather than taking the document's word).

---

## B. "Modern indexes are ~50% non-images" (§2.6)

### B.1 The ratio — **CONFIRMED for the shape, CORRECTED in the specifics**

I pulled seven `latest` indexes anonymously and counted:

| repo | entries | `unknown/unknown` | carrying `vnd.docker.reference.type=attestation-manifest` |
|---|---|---|---|
| `library/alpine` | 16 | 8 | 8 |
| `library/busybox` | 17 | 8 | 8 |
| `library/ubuntu` | 12 | 6 | 6 |
| `library/hello-world` | 20 | 9 | 9 |
| `library/nginx` | 16 | 8 | 8 |
| `library/postgres` | 16 | 8 | 8 |
| `library/registry` | 14 | 7 | 7 |

So "roughly half, and `unknown/unknown` ⟺ attestation annotation" holds in every case. Good claim.

### B.2 "For **every** real platform manifest there is a paired attestation manifest" — **CORRECTED**

This is stated as a universal in §2.6 and it is false. Two counterexamples from the same seven:

- `library/busybox`: `linux/arm/v6` has **no** attestation partner (9 real platforms, 8
  attestations).
- `library/hello-world`: two `windows/amd64` entries (`os.version` `10.0.26100.33296` and
  `10.0.20348.5499`) have **no** attestation partners (11 real, 9 attestations).

The pairing is a BuildKit convention that applies to what BuildKit built in that push, not an
invariant of the index. **The correction does not weaken the recommendation — it strengthens it.**
A "filter out `unknown/unknown`, then take what's left" matcher would hand a `windows/amd64`
manifest to a Linux extractor on `hello-world`. The document's own rule (§2.6) *does* match on
`os == "linux"` positively, so the rule is right; only the stated justification is over-tight.
Rewrite the premise as: *some* entries are not runnable images, `unknown/unknown` is the common
kind, `windows` is another, and neither is exhaustive.

### B.3 "9218 bytes, 20 entries" for the alpine index — **CORRECTED**

`content-length: 9218` is exact (I got byte-identical 9218 and the same SHA-256). But the index
has **16** `manifests` entries, not 20. Minor, but it is an `[O]` claim with a miscounted number,
and 20 happens to be `hello-world`'s count — likely a transcription crossover. Nothing downstream
depends on it.

---

## C. The auth flow and rate limits (§2.2, §2.3, §2.4)

### C.1 The 401 + `WWW-Authenticate` handshake — **CONFIRMED, all three probes**

| Probe | Result |
|---|---|
| `GET /v2/` | `401`, `www-authenticate: Bearer realm="https://auth.docker.io/token",service="registry.docker.io"` — **no `scope`** |
| `HEAD /v2/library/alpine/manifests/latest` | `401` + same, **plus** `scope="repository:library/alpine:pull"` |
| `HEAD /v2/mansurpro/definitelynotarealrepo/manifests/latest` | `401` + `scope="repository:mansurpro/definitelynotarealrepo:pull"` — **not** a 404 |

All three reproduce exactly, including `docker-distribution-api-version: registry/2.0` on the bare
`/v2/`. The derived rule — don't pre-flight `/v2/`, drive auth off the 401 of the resource you
actually want, because that challenge carries the right scope — is sound and I have no quarrel
with it.

The "private repos hide behind 401, so never report *authentication failed* to an anonymous user"
observation is correct and is the kind of thing that produces bad UX bugs. Good catch by the
source; **CONFIRMED**.

### C.2 Token exchange, `expires_in: 300`, `token == access_token` — **CONFIRMED**

```
$ curl -s 'https://auth.docker.io/token?service=registry.docker.io&scope=repository:library/alpine:pull'
keys: ['access_token', 'expires_in', 'issued_at', 'token']
expires_in: 300     issued_at: 2026-09-05T13:23:26.619686042Z
token == access_token: True
```

Five-minute anonymous tokens, and the two fields are byte-identical. The warning that "a large
multi-layer pull can outlive one token" follows. **CONFIRMED.**

### C.3 The rate limit is readable from the JWT — **CONFIRMED**

Decoding segment 2 of my own freshly-issued token:

```json
{"access":[{"actions":["pull"],"name":"library/alpine",
  "parameters":{"pull_limit":"100","pull_limit_interval":"21600","registry_region":"us-east-1"},
  "type":"repository"}],
 "aud":"registry.docker.io","exp":1788614906,"iat":1788614606,
 "iss":"auth.docker.io","jti":"dckr_jti_…","nbf":1788614306,"sub":""}
```

`pull_limit=100`, `pull_limit_interval=21600`, `sub: ""` (anonymous). Byte-for-byte the same shape
the document reports, on an independently issued token. Note `exp - iat = 300`, matching
`expires_in`, and `iat - nbf = 300` (a 5-minute clock-skew allowance) — a detail the document
doesn't mention but which matters if we ever validate `nbf`.

### C.4 The 6-hour-vs-1-hour contradiction — **CONFIRMED as a real inconsistency; adjudicated**

Both values reproduce:

```
JWT:      pull_limit=100  pull_limit_interval=21600   (6 h)
Headers:  ratelimit-limit: 100;w=3600                  (1 h)
          x-ratelimit-limit: 100;w=3600
          docker-ratelimit-source: <my IPv4>
```

(The document lists only the `x-`-prefixed forms; Hub sends **both** the `x-ratelimit-*` and the
unprefixed RFC-9331-style `ratelimit-*` names. Minor omission.)

Adjudication, which the document declined to attempt: **the JWT's 21600 is the one that matches
Docker's published anonymous policy** (100 pulls per 6 hours per IPv4 address); the header's
`w=3600` is a stale window label from the rate-limit edge and does not correspond to any published
Hub tier. I cannot prove this by waiting out either window in a read-only session, so I record it
as: *JWT agrees with published policy, header does not; treat the header's `w` as unreliable.*
The document's actual recommendation — **drive backoff off `429` + `Retry-After`, treat both
counters as advisory** — is the correct engineering answer either way, and I endorse it unchanged.

### C.5 "HEAD counts against the rate limit the same as GET" (§2.4) — **REFUTED**

This is the one substantive empirical error I found in the registry sections. §2.4 says:

> Tradeoff: `HEAD` requests count against the rate limit on Hub the same as `GET` (`[C]` MED —
> Docker's own docs said HEAD stopped being free in 2024; the `x-ratelimit-remaining` header
> decremented on our GET but we did not isolate HEAD; see §15).

I isolated it. Eight consecutive `HEAD`s on `library/alpine:latest`, one `GET`, then two more
`HEAD`s, reading `ratelimit-remaining` off each response:

```
8 HEADs : 93 93 93 93 93 93 93 93     <- no movement
1 GET   : 93
2 HEADs : 93 93
```

and separately, five consecutive `GET`s:

```
5 GETs  : 92 92 91 91 90              <- monotonically decreasing
3 HEADs : 90 90 90                    <- no movement
```

**Eleven HEADs produced zero decrements; GETs decrement.** (The counter is reported one request
behind, which is why the GET row steps rather than dropping on every line — the trend is
unambiguous.) So on Docker Hub as of 2026-09-05, `HEAD /v2/<name>/manifests/<ref>` is **free** and
`GET` is not.

Impact: this *strengthens* the document's own §2.4 recommendation. The `HEAD`-the-tag freshness
check is not merely "one request instead of a full pull", it is **zero rate-limit budget**. The
stated tradeoff should be deleted, not softened. §15's open question on this is now closed.

Caveat I will not paper over: this is Hub-specific edge behaviour with no spec backing, it can
change without notice, and I measured it once from one IP. The client must not *depend* on HEAD
being free — it should just prefer HEAD because it is never worse.

---

## D. `diff_id`, compressed digest, and chain IDs (§4.2, §4.5)

### D.1 The two-digest distinction — **CONFIRMED against the spec text**

I fetched `image-spec/config.md` and `image-spec/descriptor.md` rather than trusting the
paraphrase. `rootfs` is **REQUIRED** and `rootfs.diff_ids` is **REQUIRED**, defined verbatim as
"An array of layer content hashes (`DiffIDs`), in order from first to last" — so both the
requiredness and the ordering claim in §4.3 are correct. `descriptor.md` defines the descriptor
digest over the content *as transported*. Two genuinely different hashes by construction, exactly
as §4.2 argues. The three operational reasons given for needing both (registry can only verify
what it stores; dedup must be compression-invariant; chain IDs are built from `diff_id`s) are
sound and I could not find a hole in them.

### D.2 The ChainID formula — **CONFIRMED, with one correction to the transcription**

Spec text (normative, `config.md` §"Layer ChainID"):

```
ChainID(L₀)               = DiffID(L₀)
ChainID(L₀|…|Lₙ₋₁|Lₙ)     = Digest(ChainID(L₀|…|Lₙ₋₁) + " " + DiffID(Lₙ))
```

**CORRECTED (minor):** the document writes the recursive case as `SHA256(…)`. The spec says
`Digest(…)` — algorithm-agnostic, resolved by whatever digest algorithm is in use. In practice it
is always sha256 today, and the document's own `[R] MED` ("store chain IDs as `sha256:<hex>`, not
bare hex, in case sha512 ever appears") shows the author knows this — but the formula as written
in §4.5 is tagged `[F] HIGH` and hardcodes the algorithm, which is not what the spec says. Fix the
transcription.

### D.3 "Reproduces Docker's `layerdb` directory names byte-exactly" — **CONFIRMED, and extended**

The document verified one record. I re-derived from scratch across four images and **25 layers**
on HOST-A (read-only: `docker inspect` + reads under `/var/lib/docker/image/overlay2/layerdb/`,
nothing written, no pulls):

| image | layers | chain-ID dirs found | `diff` file matches `diff_ids[i]` | `parent` file matches previous chain ID |
|---|---|---|---|---|
| `alpine:latest` | 1 | 1/1 | all | all (no `parent` at i=0, correct) |
| `nginx:stable` | 7 | 7/7 | all | all |
| `redis:7-alpine` | 8 | 8/8 | all | all |
| `ghcr.io/aikynetix/aikpy-backend:latest` | 9 | 9/9 | all | all |

Zero misses, zero mismatches. The formula, the space separator, the `sha256:`-prefixed inputs, and
the bare-hex directory name are all exactly as described. A nice free cross-check fell out:
`alpine:latest`'s single chain ID is `21ee5abca34e63fce933…`, identical to the `diff_ids[0]` I read
out of the config blob over the network in the §2.9 trace — registry and local store agree.

Note the 9-layer image the document cites is `ghcr.io/aikynetix/aikpy-backend`, a **GHCR-hosted,
non-Hub** image, which the document does not mention. That is worth saying out loud because §15
claims "GHCR untested" — at least the *stored* form of a GHCR image was in fact exercised.

### D.4 The storage-key recommendation — **SOUND, but the "expensive to change" framing is
MISCLASSIFIED**

The recommendation (§4.5): key `blobs/` by compressed digest, `layers/` by `diff_id`,
`snapshots/` by chain ID. The *reasoning* is correct and I verified each premise:

- `diff_id` alone genuinely cannot key a snapshot — the same changeset over different parents
  yields different results. True by construction.
- Chain ID is the content address of *the applied prefix*, which is what a lowerdir set
  represents. True, and confirmed by Docker doing exactly this (D.3).
- The claimed alignment with both incumbents holds: Docker keys `layerdb/sha256/<chainID>` with a
  separate `cache-id` indirection to `overlay2/<cache-id>/diff`, and containerd keys its content
  store by digest and its snapshotter by chain ID. I confirmed the Docker half directly — every
  `layerdb/sha256/<chainID>/` record on HOST-A contains `cache-id`, `diff`, `parent`, `size`,
  `tar-split.json.gz`.

Where I push back is the **framing**, and this matters because §11.5 uses it to rank what to decide
early. Re-keying a content-addressed store is *not* an expensive migration: every key is derivable
from data you already have on disk. Given the layer directories and the manifests, you can
recompute every `diff_id` and every chain ID offline and rename directories — it is a one-shot
local script, no re-download, no re-extraction. Contrast with the genuinely expensive decision the
same document identifies in §5.5: **`tar-split` must be captured *during* extraction**, so
retrofitting it means re-extracting every layer. That one is irreversible-without-redoing-work;
the storage keys are not.

So: keep the recommendation (it is right, and it is what the incumbents do), but demote the
"expensive to change" justification. It is right for the right reason — matching containerd's
model buys interop and clear thinking — not because re-keying would be costly.

---

## E. Spec claims checked against the actual spec text (§1, §3.2, §3.6, §7.1)

I downloaded `image-spec` (`spec.md`, `descriptor.md`, `manifest.md`, `image-index.md`,
`layer.md`, `config.md`, `media-types.md`, `image-layout.md`) and `distribution-spec`
(`spec.md`, `content-negotiation.md`) and `runtime-spec/bundle.md` from `main`, then grepped every
quoted phrase. Most quotes are accurate; the misses cluster in one place — **the document
upgrades SHOULDs into MUSTs and attributes to specs things they do not say.**

### E.1 Quotes that check out verbatim — **CONFIRMED**

`spec.md`'s "This specification defines an OCI Image, consisting of an image manifest, an image
index (optional), a set of filesystem layers, and a configuration" and "unpacked into an OCI
Runtime Bundle"; both `bundle.md` quotes (the `config.json` REQUIRED/MUST sentence and the
"that directory itself is not part of the bundle" sentence); `descriptor.md`'s SHA-256
MUST-implement / SHA-512 MAY; **all twelve `layer.md` whiteout quotes in §5.1**, verbatim,
including the awkward ones; `config.md`'s `diff_ids` and `empty_layer` definitions;
`media-types.md`'s "Interchangeable and fully compatible mime-types".

The §3.2 OCI-vs-Docker-Schema-2 delta list is **exactly** the spec's Compatibility Matrix —
index (`.annotations`, `.manifests[].annotations`, `.manifests[].urls`), manifest (`.annotations`,
`.config.annotations`, `.config.urls`, `.layers[].annotations`), layer (interchangeable), config
(`.config.Memory`/`.MemorySwap`/`.CpuShares`/`.Healthcheck` + `.ArgsEscaped`). Nothing added,
nothing dropped. **CONFIRMED**, and this is the load-bearing claim under "one struct parses both",
so it was worth checking.

Two nits on that one: the matrix compares against *pinned* versions (`distribution` v2.8.3, moby
v20.10.8) and is headed "Similar/related schema", so calling it "the *entire* difference set" is a
shade stronger than the spec claims for itself. And the spec adds a line the document omits:
`.config.StopSignal` and `.config.Labels` are undocumented in Docker Image Spec v1.2 but are not
OCI-specific.

Required/optional as stated in §1.2 also checks out: descriptor `{mediaType, digest, size}`
REQUIRED with `urls`/`annotations`/`data`/`artifactType` OPTIONAL; index `platform.architecture`
and `platform.os` REQUIRED when `platform` is present (`platform` itself OPTIONAL); config
`architecture`/`os`/`rootfs` REQUIRED, `history` OPTIONAL.

### E.2 `"the digest is a cryptographic checksum of the content as it is transported"` — **REFUTED as a quote**

§1.2 presents this in quotation marks, attributed to `descriptor.md`. **The phrase "as it is
transported" does not appear anywhere in `descriptor.md`, or anywhere in the image spec.** What
`descriptor.md` actually says is:

> The _digest_ property of a Descriptor acts as a content identifier… It uniquely identifies
> content by taking a collision-resistant hash of the bytes.

The *substance* the document draws from it — that a compressed layer's descriptor digest is the
digest of the gzip bytes, not the tar bytes — is **true**, but it is established by `layer.md`'s
`+gzip` media-type definitions plus the separate existence of `config.rootfs.diff_ids`, **not** by
this sentence. Replace the fabricated quote with a real citation. A verifier who checks one quote
and finds it invented will distrust the other forty, all of which are fine.

### E.3 `"a registry that cannot satisfy Accept SHOULD return 406 Not Acceptable"` (§2.5) — **REFUTED**

Tagged `[F] MED` and attributed to the Distribution Spec. **The string `406` appears exactly once
in the entire Distribution Spec, inside a git SHA in an appendix URL** (`…ba35f80796be…`). There
is no `406 Not Acceptable` requirement, recommendation, or mention. The spec says only:

> The client SHOULD include an `Accept` header indicating which manifest content types it supports.
> In a successful response, the `Content-Type` header will indicate the type of the returned manifest.

So the first half of the document's sentence is right and the second half is invented. This
matters because the document then says "Hub returns 200 with a type you did not ask for. Real-world
registries deviate" — framing Hub as deviating from a spec requirement that does not exist. Hub is
not deviating here; the spec simply doesn't say.

### E.4 The citation §2.5 *should* have used — and it strengthens the document

Two things in the Distribution Spec directly support the document's conclusion and are not cited:

1. > The `Content-Type` header SHOULD match what the client [pushed as the manifest's
   > `Content-Type`](#pushing-manifests).

   This is the **normative** version of "the registry stores and serves bytes; conversion happened
   at push time". It promotes that claim from `[C]` to `[F]`.

2. `distribution-spec/content-negotiation.md` — a real file in the spec repo — is the documentary
   origin of the "Hub down-converts for old clients" belief the document declares dead. It is
   **still marked `TODO`** (open issue #212) and still carries a table asserting that `Accept`
   absent, `*/*`, or `application/json` yields `manifest.v1+prettyjws` "for backward compatibility
   with old, non-v2 clients". My §A.1 measurements show that table is **now factually wrong for
   Docker Hub**. That is a far better citation than the document's current one: the belief was not
   folklore, it was written down in the spec repo, and it is stale. Worth an `[OUTDATED]` note
   pointing at that file.

### E.5 `[F] HIGH` on the 307 blob redirect (§2.8) — **CORRECTED, twice, and one MISCLASSIFICATION**

§2.8 says:

> `[F]` HIGH — Distribution Spec explicitly allows this: the registry "MAY" respond to a blob GET
> with a `307 Temporary Redirect` and the client "MUST" follow it.

The actual and only sentence on redirects in the whole spec:

> Registries MAY respond to any request with a redirect per [RFC 9110 (section 15.4)]; clients
> SHOULD follow such redirects, and **MUST NOT forward `Authorization` headers across host
> boundaries unless explicitly configured to do so**.

Three corrections:

1. **`307` is not in the spec.** The spec says "a redirect per RFC 9110 §15.4" — any 3xx, on *any*
   request, not specifically 307 and not specifically on blob GET. 307 is Hub's choice.
2. **"MUST follow" is wrong — the spec says clients SHOULD follow.**
3. **MISCLASSIFIED, and this is the one that matters.** The document presents "strip
   `Authorization` when the redirect crosses to a different host" as `[R] HIGH` — *our
   recommendation, with a tradeoff*. It is a **spec `MUST NOT`**. Not following it is a conformance
   failure, not a judgment call. Retag it `[F] HIGH` and delete the tradeoff sentence. This is the
   inverse of the usual over-read: the document *under*-claimed a genuine normative requirement,
   which is the more dangerous direction because a reader may treat it as optional.

Related, also uncited and worth adding to §2.8: "A registry SHOULD support the `Range` request
header in accordance with RFC 9110 (section 14)" — so the document's measured `206` is spec-backed,
not just a Hub nicety.

### E.6 `Docker-Content-Digest` is "SHOULD" (§2.4) — **CORRECTED / possibly OUTDATED**

§2.4 says the response "SHOULD" include `Docker-Content-Digest`. Current `distribution-spec/main`
says **MUST**, in three separate places:

> A successful response MUST contain the digest of the uploaded blob in the header
> `Docker-Content-Digest`.  *(manifest GET, blob GET, and HEAD each)*

The SHOULD wording is from the older spec; there is even an implementer's note about "registries
implementing earlier spec versions which did not require the `Docker-Content-Digest` header".
The document's defensive posture (treat it as a hint, hash the body yourself) is still right — and
is itself spec-backed: "Most clients MAY ignore the value, but if it is used, the client MUST
verify the value matches the returned manifest." So §2.4's `[R] HIGH` is, again, a spec MUST wearing
an `[R]` label.

### E.7 §3.6's media-type conflict rule — **CORRECTED (SHOULD, not MUST)**

The document: "you SHOULD respect the expected media type if the digest matches, MAY warn, and
**MUST error** if the digest does not match or you have no expected digest." The spec puts *all
three* clauses under one SHOULD:

> Implementations that have an expected media type which does not match the returned media type
> **SHOULD**: — Respect the expected media type if the blob matches the expected digest… — Return
> an error if the blob does not match the expected digest… — Return an error if they do not have
> an expected digest.

So the "MUST error" is a SHOULD. The recommendation to implement it as written is still the right
call — I'd keep the behaviour and fix the tag.

**And I found much better evidence for the rule than the document has.** §3.6 cites Hub sending
`application/octet-stream` on blob GETs. GHCR does something worse: fetching the *config blob* of
`ghcr.io/distribution/distribution:latest` returns

```
content-type: application/vnd.zarf.layer.v1.blob
```

on a blob whose descriptor says `application/vnd.oci.image.config.v1+json`. A client that trusted
the HTTP `Content-Type` for blobs would conclude the config is a Zarf layer. Descriptor wins.
Confirmed on a second registry, with a far more alarming value than `octet-stream`.

### E.8 §1.3 "`image-layout` … is explicitly not designed as a live engine's store" — **CORRECTED**

Tagged `[F] HIGH, by omission`. The spec is *silent* on engine use — it neither endorses nor
forbids it. What it actually says is:

> This layout MAY be used in a variety of different transport mechanisms: archive formats
> (e.g. tar, zip), shared filesystem environments (e.g. nfs), or networked file fetching
> (e.g. http, ftp, rsync).

That supports "it is framed as a transport/interchange format", which is what §7.1 correctly says.
But "explicitly not designed as a live engine's store" is the document's inference, not the spec's
statement. §7.1's own three reasons (no place for extracted layers/snapshots/chain IDs; `index.json`
is a single rewritten file; neither incumbent uses it live) are good, sufficient, and honest — §1.3
should just say that instead of claiming the spec does.

### E.9 Two normative points the document misses that support its own conclusions

- **`manifest.md`** states layer ordering normatively, which §4.3 only cites `config.md` for:
  "The array MUST have the base layer at index 0. Subsequent layers MUST then follow in stack
  order… The final filesystem layout MUST match the result of applying the layers to an empty
  directory." Stronger citation for the "§4.3 vs §8.3 direction" warning.
- **`image-index.md`**: "If multiple manifests match a client or runtime's requirements, the first
  matching entry SHOULD be used." Exactly the tie-break rule §2.6's filter needs and doesn't state.
- Spec quirk worth knowing: `manifest.layers` is **not** marked REQUIRED in `manifest.md` — it
  says only "For portability, `layers` SHOULD have at least one entry." Our parser should not
  assume it is present.

---

## F. Blob redirects, `Range`, and CVE-2020-15157 (§2.8)

### F.1 The measured redirect behaviour — **CONFIRMED in full**

Re-run against `library/alpine`'s layer blob
`sha256:94a16efeeef22bb5e5045f32dafee56272ffd89860bac26f6c32123439af26a4`:

| Probe | Result |
|---|---|
| `HEAD` blob | `200`, `content-length: 3846391`, `accept-ranges: bytes`, `cache-control: max-age=31536000`, **no `Location`** |
| `GET` blob | **`307`**, `location: https://production.cloudfront.docker.com/registry-v2/…/data?Expires=…&Signature=…&Key-Pair-Id=…` |
| CDN URL **with** our `Authorization` | `206`, 100 bytes |
| CDN URL **without** `Authorization` | `206`, 100 bytes |
| `Range: bytes=0-99` on the CDN URL | `206`, exactly 100 bytes |

The HEAD/GET asymmetry, the three presigned query parameters (`Expires`, `Signature`,
`Key-Pair-Id`), the fact that the bearer token is neither needed nor harmful at CloudFront, and
`Range` support are all exactly as described. **CONFIRMED.**

`curl -L` with `-H "Authorization: …"` follows the cross-host redirect and succeeds — which is
precisely why the document's warning is worth having: `curl` does *not* strip the manually-supplied
header, so a naive port of the trace into code inherits the leak.

### F.2 The same asymmetry on GHCR — **CONFIRMED, second registry**

§2.8 asserts `[C] HIGH` that "Docker Hub, GHCR, ECR, GCR, and Quay all do this in production" but
only measured Hub. I measured GHCR:

```
GET  ghcr.io/v2/distribution/distribution/blobs/sha256:49e11725…  -> 307
     location: https://pkg-containers.githubusercontent.com/ghcr1/blobs/sha256:49e11725…
HEAD ghcr.io/v2/distribution/distribution/blobs/sha256:49e11725…  -> 200 (no Location)
```

Different CDN host, identical shape — 307-on-GET, 200-on-HEAD, cross-host. Two of the five
registries in that `[C]` are now `[O]`. ECR/GCR untested.

### F.3 CVE-2020-15157 — **CORRECTED (the CVE is not quite what the document says)**

§2.8 cites it as: *"containerd credential leak via `foreign layer` URLs — a malicious manifest with
`urls` pointing at an attacker host caused containerd to send registry credentials there."*

The advisory (GHSA-742w-89gc-8m9c / NVD) says something more specific:

> If a container image manifest in the OCI Image format or Docker Image V2 Schema 2 format includes
> a URL for the location of a specific image layer (a "foreign layer"), the default containerd
> resolver will follow that URL to attempt to download it. In **v1.2.x but not 1.3.0 or later**, the
> default containerd resolver **will provide its authentication credentials if the server where the
> URL is located presents an HTTP 401 status code along with registry-specific HTTP headers**.

Three corrections:

1. **It is not a redirect bug.** It is a `urls`-field bug: containerd *directly fetches* the
   attacker's URL, then the attacker replies `401` with a Docker-style `WWW-Authenticate`, and
   containerd walks the token flow and hands over credentials. The document's own §2.8 framing
   ("would otherwise be handed our registry bearer token" on a redirect) describes a *different*,
   adjacent bug.
2. **The leaked credential is the registry username/password (or a cloud instance credential),
   not the already-scoped bearer token** — the attacker triggers a fresh auth handshake. That is
   strictly worse than leaking a 5-minute pull-scoped token.
3. **Affected: containerd < 1.2.14. 1.3.0+ were never affected.** The document gives no version.

Why this matters for our design: the correct lesson is **two** rules, not one.
- (a) strip `Authorization` on cross-host redirect — real, and per §E.5 a spec `MUST NOT`;
- (b) **never perform an authentication handshake against a host that came from image content**
  (`urls`, or a redirect target). CVE-2020-15157 is rule (b), and §2.8 currently cites it as
  evidence for rule (a). Rule (b) is not stated anywhere in §2.8. §6.13 discusses the `urls` field
  from an extraction standpoint (another verifier's section), but the *credential* half of the
  hazard belongs in the auth middleware, which is §2.

**Concrete addition for the auth layer (§2.3's middleware):** the token-fetch path must be gated on
the host being the *configured registry* for this pull, never a host learned from a manifest.

---

## G. Non-Hub registries — §15.7's "LOW confidence / untested" gap, now tested

Every registry empiric in §2–§3 is Docker Hub; §15.7 admits it and rates the gap `[C] LOW`. This
was cheap to close for two registries and the result **refutes a generalization in §2.5.**

### G.1 Three registries, three completely different `Accept` behaviours — **REFUTES §2.5's general claim**

Same probe set, anonymous, one tag per registry:

| `Accept:` sent | **Docker Hub** `library/alpine:latest` | **Quay** `prometheus/busybox:latest` | **GHCR** `distribution/distribution:latest` |
|---|---|---|---|
| all four modern types | `200` OCI index | `200` docker manifest **list** | `200` OCI index |
| OCI index + OCI manifest only | `200` OCI index | `200` **Schema 1** (`manifest.v1+json`) | `200` OCI index |
| Docker `manifest.v2+json` only | `200` OCI index | `200` docker **manifest** (single) | **`404`** |
| `application/x-total-nonsense` | `200` OCI index | `200` **Schema 1** | **`404`** |
| no `Accept` header | `200` OCI index | `200` **Schema 1** | **`404`** |

- **Hub** ignores `Accept` entirely (§A).
- **Quay content-negotiates and down-converts, including generating Schema 1 on the fly.** The
  three responses have **three different digests** for the same tag:
  `57ba692b…` (list), `d86ce8f3…` (manifest), `49b68533…` (schema 1, `schemaVersion: 1`, `fsLayers`,
  no `mediaType`). A registry that returns a Schema 1 body with a distinct digest for a tag it also
  serves as a manifest list is **synthesising** that manifest at request time.
- **GHCR** content-negotiates and its failure mode is a **`404 Not Found`** — it reports the image
  as absent rather than unacceptable.

**REFUTED:** §2.5's `[C] HIGH` conclusion, second sentence — *"The registry stores and serves bytes;
conversion (if any) happened at push time."* Quay demonstrably converts at **serve** time. The
first sentence (the "Hub down-converts" belief is false **for Hub**) stands; the generalization to
"the registry" does not.

**MISCLASSIFIED:** §3.3's "`[C]` HIGH — non-Hub registries: GHCR/Quay/ECR store what you push".
Quay stores what you push *and also* synthesises other representations on demand.

### G.2 Consequences the document does not draw — and they change the client

1. **The four-type `Accept` header is not politeness, it is load-bearing.** On Quay, an *OCI-only*
   `Accept` gets you a **Schema 1** manifest — which §3.4 then tells us to hard-refuse. So an
   OCI-only client would report "manifest schema 1 is not supported (image was pushed before 2017)"
   for a perfectly modern 2026 image. On GHCR, a *Docker-only* `Accept` gets a **404** — "image not
   found" for an image that exists. §12.1 item 3 already prescribes all four types; it should say
   **why**: omitting either family produces a *wrong and confusing* answer, not a graceful one.
2. **`Accept` can change which content a tag resolves to.** Three digests for one Quay tag means
   two clients with different `Accept` headers disagree about what `quay.io/prometheus/busybox:latest`
   *is*. Any cache keyed on `(repo, tag)` must also key on the `Accept` set, or record the resolved
   digest and never re-resolve. Nothing in §2 or §7 says this.
3. **`404` is not proof of absence.** GHCR's behaviour means "404 on a manifest GET" must not be
   surfaced as "no such image" without having sent a full `Accept`. Pairs badly with §2.2's
   already-noted "401 may mean private-or-absent": between the two, neither `401` nor `404` is a
   reliable existence signal.
4. **Schema 1 is not only a pre-2017 artifact.** §3.4's error message — "image was pushed before
   2017; ask the publisher to re-push" — is **wrong advice** on Quay, where Schema 1 is what *you*
   asked for by sending a bad `Accept`. The error text should say: "registry returned manifest
   schema 1; check the `Accept` header, or the image predates schema 2".

### G.3 GHCR's `/v2/` challenge carries a **fake** scope — new support for §2.2's recommendation

```
$ curl -s -D- https://ghcr.io/v2/
HTTP/2 401
www-authenticate: Bearer realm="https://ghcr.io/token",service="ghcr.io",scope="repository:user/image:pull"
```

`repository:user/image:pull` is a **placeholder**, not a real scope. A client that pre-flights
`/v2/` and reuses that challenge's scope requests a token for a repository named `user/image`.
The resource-level challenge is correct
(`scope="repository:distribution/distribution:pull"`). §2.2's `[R] HIGH` "do not pre-flight
`GET /v2/`" is right, and this is a sharper reason than the one given (saving a round trip):
**on GHCR the pre-flight challenge is actively misleading.** Also note `HEAD /v2/` on GHCR returns
`405 Method Not Allowed` — only `GET` produces the challenge.

### G.4 Verdict on §15.7

The open question is **partially closed**: GHCR and Quay tested anonymously; ECR, GCR/AR, Harbor
and a local `registry:2` still untested. The conclusions that survive across all three registries:
the Bearer challenge/token/retry flow, blob `GET`→3xx cross-host with `HEAD`→200, descriptor
media type beating HTTP `Content-Type`, and **dispatch on the response `Content-Type`**. The
conclusion that does **not** survive is "registries serve what was pushed".

---

## H. Layer format re-measured from scratch (§4.1, §4.2, §4.4, §2.9, §3.6)

I re-downloaded the alpine layer and config anonymously and recomputed everything. **Every number
in §4.1, §4.2, §4.4 and §2.9 reproduces exactly.** This is the most solid part of the document.

```
compressed   3,846,391 bytes   sha256 = 94a16efeeef2…af26a4   == manifest.layers[0].digest
uncompressed 8,697,856 bytes   sha256 = 21ee5abca34e…26ce3c   == config.rootfs.diff_ids[0]
ratio 2.26x
gzip header  1f 8b 08 00 00000000 00 ff      -> MTIME=0, XFL=0, OS=0xff  (BuildKit reproducibility)
offset 257   75 73 74 61 72 00 30 30         -> "ustar\0" + "00"  = POSIX.1-1988 ustar
entries      bin/, bin/arch -> /bin/busybox, …   (no leading "./")
config       sha256 = 1a8c3406406e…a11b  == manifest.config.digest
history      len 2, empty_layer flags [absent, true];  diff_ids len 1
blob GET     content-type: application/octet-stream
```

**CONFIRMED:** the two-digest table, the ratio, the ustar (not GNU, not PAX-only) determination,
the zeroed gzip header, the `history`-vs-`diff_ids` length mismatch with `empty_layer: true` on the
`CMD` step, the `application/octet-stream` on blob GETs, and the "both forms occur, so strip
leading `./` and `/`" normalization rule.

The §4.4 correspondence rule — walk `history` in order, skip `empty_layer: true`, i-th remaining
entry maps to `diff_ids[i]` — is the correct reading of the spec text (which I confirmed verbatim)
and matches this image. And the note that `history` is attacker-controlled text that must be
escaped before display is a good one that nothing else in the corpus says.

One addition: `history` is **OPTIONAL** in `config.md`, so the parser must handle its absence, not
just an `empty_layer` mismatch. §4.4 implies it is always there.

---

## I. Whiteouts: tar → OverlayFS translation, and the privilege claims (§5)

### I.1 The Image Spec side — **CONFIRMED, all twelve quotes verbatim**

Every whiteout quote in §5.1 is present word-for-word in `image-spec/layer.md`, including the two
that carry the most weight:

> "Whiteout files MUST only apply to resources in lower/parent layers."

> "When processing the second layer, `a/.wh..wh..opq` is applied first, before creating the new
> version of `a/b`, **regardless of the ordering in which the whiteout file was encountered**."

The document's derived conclusion — that a naive single-pass streaming extractor which applies
entries in stream order is **wrong** for opaque whiteouts, so opaque markers must be collected and
applied before other entries under that directory — follows correctly from that sentence.
**CONFIRMED**, and it is the sort of thing implementations get wrong.

The "Changeset over existing files" rule (§5.1's last paragraph) is also real: the spec requires
"removing the file path… recreating the file path, based on the contents and attributes of the
changeset entry" — i.e. unlink-then-recreate, not overwrite-in-place. The document's observation
that this is what stops a regular file being written *through* an existing symlink is correct and
is the strongest single argument in §5.

### I.2 The OverlayFS side — **CONFIRMED against `overlayfs.rst`**, with one omission

Kernel `Documentation/filesystems/overlayfs.rst` (Linux master), verbatim:

> A whiteout is created as a character device with 0/0 device number or as a zero-size regular
> file with the xattr "trusted.overlay.whiteout".

> A directory is made opaque by setting the xattr "trusted.overlay.opaque" to "y".

> The "-o userxattr" mount option forces overlayfs to use the "user.overlay." xattr namespace
> instead of "trusted.overlay.". This is useful for unprivileged mounting of overlayfs.

All three match §5.2. **CONFIRMED.**

**Omission worth fixing** — the same paragraph continues:

> An opaque directory should not contain any whiteouts… A merge directory containing regular files
> with the xattr "trusted.overlay.whiteout", should be additionally marked by setting the xattr
> **"trusted.overlay.opaque" to "x"** on the merge directory itself. This is needed to avoid the
> overhead of checking the "trusted.overlay.whiteout" on all entries during readdir.

So `trusted.overlay.opaque` has **two** values with different meanings: `"y"` = opaque directory,
`"x"` = "this directory contains xattr-form whiteouts, look for them". §5.3's translation table
lists the xattr-whiteout row (the rootless-friendly encoding the document recommends isolating
behind a trait) but **not** the `"x"` marker that must accompany it. An implementation that writes
zero-size + `overlay.whiteout` files without setting `opaque="x"` on the parent produces whiteouts
the kernel may not honour. Add the row.

### I.3 "Docker actually writes char device 0/0" — **CONFIRMED with real data**

§5.3 infers this from mount options showing `nouserxattr`. I checked the on-disk artifacts
directly (read-only `find`, nothing modified):

```
$ find /var/lib/docker/overlay2 -type c | wc -l
44
$ ls -l <one of them>
c--------- 1 root root 0, 0 Aug 11 23:23 …/diff/opt/conda/envs/prod/bin/numpy-config
```

44 whiteouts on this host, every one a character device, **major 0 minor 0, mode 0000, root:root** —
exactly the translation the table prescribes, down to the mode. Direct evidence, not inference.

### I.4 The privilege claims — **CONFIRMED against the man pages**

| Claim | Verified text |
|---|---|
| `mknod` of a char device needs `CAP_MKNOD` | `mknod(2)` EPERM: "…and the caller is not privileged (Linux: does not have the CAP_MKNOD capability)" |
| `trusted.*` xattrs need `CAP_SYS_ADMIN` | `xattr(7)`: "Trusted extended attributes are visible and accessible **only** to processes that have the CAP_SYS_ADMIN capability." |

**CONFIRMED.** Both are exactly as §5.3 states, and the downstream consequence the document draws
— that the tar→overlay whiteout translation is *precisely* what makes `docker pull` need
privileges that `tar -x` does not — is correct and is the cleanest statement of it I have seen in
this corpus.

The rootless story in §5.4 (mknod EPERM inside a userns despite holding namespaced `CAP_MKNOD`;
`fuse-overlayfs` vs kernel `userxattr`; Linux ≥ 5.11) is tagged `[C]` not `[O]`, correctly — the
host that could test it is unreachable. **UNSUPPORTED — pending host**, as the document itself
admits in §15.1. What would settle it, on any kernel ≥ 5.11 or on ROCKY's 4.18:

```
unshare -Urm sh -c 'mknod /tmp/w c 0 0; echo $?'          # expect EPERM
unshare -Urm sh -c 'mount -t overlay -o lowerdir=…,upperdir=…,workdir=…,userxattr overlay /mnt'
grep -w overlay /proc/filesystems
```

---

## J. `lchown` before `lchmod` (§6.6) — **REFUTED reasoning, CONFIRMED and strengthened conclusion**

*(§6 belongs to another verifier; verifying this specific mechanism was assigned to me, so I cover
only this claim and hand the rest over.)*

§6.6 states:

> `[F]` HIGH — `chown(2)`: "When the owner or group of an executable file is changed by an
> unprivileged user, the `S_ISUID` and `S_ISGID` mode bits are cleared." **As root the bits are
> *not* cleared by `chown`**, but the kernel's behaviour differs by path, which is why order
> matters.

**The bolded sentence is false on Linux.** The very next lines of the same man page say:

> POSIX does not specify whether this also should happen when root does the `chown()`; the Linux
> behavior depends on the kernel version, and **since Linux 2.2.13, root is treated like other
> users.**

Measured as root on HOST-A (kernel 6.11.0-29), in a `mktemp -d` scratch dir under `/tmp`,
removed afterwards — nothing under `/var/lib/docker` touched:

```
chmod 4755 f            -> mode 4755
chown 1000:1000 f       -> mode  755      # setuid CLEARED, as root
chmod 4755 f            -> mode 4755      # restored by chmod-after-chown
```

So the correct statement is: **`chown` clears `S_ISUID`/`S_ISGID` unconditionally on Linux,
including as root, since 2.2.13.** The document's parenthetical would let a reader conclude "we run
the extractor as real root, so the ordering is a nicety" — which is exactly the wrong conclusion,
and the failure is silent.

### J.1 The mechanism extends to file capabilities — not in the document at all

`chown(2)` also says: "When the owner or group of an executable file is changed (by any user), **all
capability sets for the file are cleared.**" Measured, same scratch dir:

```
--- correct order (chown, then setcap, then chmod) ---
chown 1000:1000 f ; setcap cap_net_bind_service=ep f   -> cap = cap_net_bind_service=ep
chmod 4755 f                                            -> mode 4755, cap = cap_net_bind_service=ep   OK

--- reversed order (chmod + setcap, then chown) ---
chmod 4755 g ; setcap cap_net_bind_service=ep g         -> mode 4755, cap = cap_net_bind_service=ep
chown 1000:1000 g                                       -> mode  755, cap = []          BOTH LOST
```

**So the claim "reversing it makes setuid silently vanish from every image" is CONFIRMED — and
understated. Reversing it also silently destroys every `security.capability` xattr in every
image**, which means `ping`, `newuidmap`/`newgidmap`, and every capability-based
setuid-replacement stop working, with no error anywhere. That is a second, independent failure the
document does not mention, and it is arguably the more confusing one to debug because the file
mode looks untouched apart from the missing bit.

### J.2 The §6.14 checklist ordering is correct

Steps 8→9→10 are `fchownat` → xattrs → `fchmodat`. Given J.1, this ordering is not merely correct
but **necessary in exactly that order**: chown first (it clears both mode bits and caps), then
xattrs (so `security.capability` survives), then chmod (to restore setuid). My run above confirms
that sequence preserves both. Whoever wrote the checklist got it right; only §6.6's *explanation*
is wrong. Fix the prose, keep the code.

Also confirmed for §6.6: `/var/lib/docker` is `drwx--x--- root:root` and
`/var/lib/docker/overlay2` is `drwx--x--- root:root` on HOST-A (`/var/lib/docker/image` is
`drwx------`). The "`--x` lets helpers traverse without listing" reading is right, and it is what
keeps the 64 setuid binaries I found under `/var/lib/docker/overlay2/*/diff/` out of reach of
non-root host users.

---

## K. §7 store design and §12 conformance

### K.1 §7.2 / §7.3 — **CONFIRMED, every empirical detail**

Re-checked read-only on HOST-A:

| Claim | Verified |
|---|---|
| containerd content store is `blobs/sha256/<hex>` + `ingest/` | yes; 12 blobs, `ingest/` present |
| blobs are mode `0444` | yes, `-r--r--r-- root root` |
| a blob is its own name | yes — `sha256sum` of `6e9ade9a77ab…` returns `6e9ade9a77ab…` |
| separate snapshotter dirs per plugin | yes — `overlayfs`, `native`, `btrfs`, `blockfile` all present |
| metadata in `io.containerd.metadata.v1.bolt/meta.db` | yes |
| Docker `layerdb/sha256/<chainID>/{cache-id,diff,parent,size,tar-split.json.gz}` | yes, all five files |
| `repositories.json` maps tag **and** `name@<manifest digest>` to the **config** digest | yes, byte-identical to the quoted JSON |
| Docker image ID == config blob digest | yes — `docker image inspect alpine --format '{{.Id}}'` = `sha256:d529dd0c…` = the config blob I pulled anonymously |
| `RootFS.Layers[0]` == the `diff_id` computed from the gzip stream | yes — `sha256:21ee5abca34e…` |
| `distribution/diffid-by-digest` and `v2metadata-by-diffid` exist | yes — **323 entries each** |

The three-way agreement (registry config blob ↔ local Docker store ↔ my own hashing of the gzip
stream) is real and is good evidence for the whole `diff_id` design.

**CORRECTED (trivial):** §7.8 says "107 directories in `/var/lib/docker/overlay2/` and 106 symlinks
in `overlay2/l/`". I count **106** directories excluding `l/` itself, and 106 symlinks — the 107
almost certainly counted `l/` as a layer directory. The point (1:1 correspondence between layer
dirs and short-link symlinks) is right and is nicer with the corrected numbers.

### K.2 §7.1's normative points — **CONFIRMED**

All five `image-layout.md` quotes check out verbatim, including the two easy-to-misremember ones
("The blobs directory MAY contain blobs which are not referenced by any of the refs" and "MAY be
missing referenced blobs, in which case the missing blobs SHOULD be fulfilled by an external blob
store"). The `oci-layout`/`index.json`/`blobs` MUST-exist trio is right. See §E.8 for the one
over-read, which is in §1.3 rather than here.

### K.3 §7.4–§7.8 recommendations — sound, two notes

The `rename(2)` atomicity argument, the same-filesystem `EXDEV` constraint, the ingest/staging
discipline, `fsync` the file then `fsync` the parent directory after `rename`, mark-and-sweep over
refcounting, and the grace-period-vs-leases tradeoff are all correct as stated, and the
`overlayfs.rst` quote about `workdir` needing to be on the same filesystem as `upperdir` is real.
Two notes:

- The claim that unlinking a live overlay `lowerdir` silently succeeds is tagged `[F] HIGH` but is
  not sourced to anything and I did not test it (testing it means mutating a live overlay on a
  production host). It is consistent with how the dcache works, but as written it is
  **UNSUPPORTED** — it needs either a citation or a test on a scratch host. The *recommendation*
  (GC must consult `/proc/self/mountinfo` before unlinking) is right regardless.
- §7.3's `diffid-by-digest` cache is called "the single highest-value cache in the whole puller".
  I think that is correct and under-argued: with 323 entries on a 6-repository host, it is what
  makes the second pull of a shared base layer cost *one manifest request*. Worth keeping.

### K.4 §12 conformance — mostly sound, three fixes from my findings

- Item 3 ("`Accept` with all four modern media types, and dispatch on the response
  `Content-Type`") is **more important than stated** — see §G.2. Add *why*: on Quay an OCI-only
  `Accept` yields Schema 1; on GHCR a Docker-only `Accept` yields `404`.
- Item 6 ("follow 307 to the CDN, strip `Authorization` cross-host") should be marked as a
  **spec `MUST NOT`**, not a recommendation (§E.5), since §12 is about conformance claims
  specifically.
- §12.2's `[R] MED` "the cheapest high-value item is HEAD on the tag" is right, and now stronger:
  HEAD costs **zero** Hub rate-limit budget (§C.5), not one unit.

The choice to frame conformance as "what is the cheapest thing that runs a real `alpine`" rather
than "how much of the spec" is good judgment and I have no argument with the staged table.

---

## L. Summary

### Not confirmed (lead with these)

| # | § | Claim | Verdict |
|---|---|---|---|
| 1 | §2.5 | "The registry stores and serves bytes; conversion (if any) happened at push time" | **REFUTED** — Quay synthesises Schema 1 at serve time; 3 digests for one tag depending on `Accept` (§G.1) |
| 2 | §6.6 | "As root the `S_ISUID`/`S_ISGID` bits are *not* cleared by `chown`" | **REFUTED** — cleared as root since Linux 2.2.13; measured on 6.11 (§J) |
| 3 | §2.4 / §15.3 | "`HEAD` requests count against the rate limit on Hub the same as `GET`" | **REFUTED** — 11 HEADs, zero decrements; GETs decrement (§C.5) |
| 4 | §2.5 | Distribution Spec: registry "SHOULD return `406 Not Acceptable`" | **REFUTED** — `406` appears nowhere in the spec (§E.3) |
| 5 | §1.2 | descriptor digest is "a cryptographic checksum of the content *as it is transported*" (quoted) | **REFUTED as a quote** — phrase is not in the spec; substance is still true (§E.2) |
| 6 | §2.8 | CVE-2020-15157 is a cross-host-**redirect** credential leak | **CORRECTED** — it is a `urls`/foreign-layer leak via a forged `401`, leaking registry *credentials*, containerd < 1.2.14 (§F.3) |
| 7 | §2.8 | `[F]`: spec allows **307** on blob GET and client "**MUST**" follow | **CORRECTED** — spec says any RFC 9110 §15.4 redirect, clients **SHOULD** follow (§E.5) |
| 8 | §2.8 | "strip `Authorization` cross-host" tagged `[R]` (our recommendation, with tradeoff) | **MISCLASSIFIED** — it is a spec **`MUST NOT`**; a conformance requirement (§E.5) |
| 9 | §2.6 | "For **every** real platform manifest there is a paired attestation manifest" | **CORRECTED** — false for `busybox` (`linux/arm/v6`) and `hello-world` (2 `windows/amd64`) (§B.2) |
| 10 | §2.4 | `Docker-Content-Digest` "SHOULD" be included | **CORRECTED / OUTDATED** — current spec says **MUST**, in three places (§E.6) |
| 11 | §1.3 | `image-layout` is "**explicitly** not designed as a live engine's store" | **CORRECTED** — spec is silent; §7.1's own three reasons are the honest argument (§E.8) |
| 12 | §3.6 | "**MUST** error if the digest does not match or you have no expected digest" | **CORRECTED** — the spec puts all clauses under one **SHOULD** (§E.7) |
| 13 | §4.5 | ChainID formula written as `SHA256(…)`, tagged `[F]` | **CORRECTED** — spec says `Digest(…)`, algorithm-agnostic (§D.2) |
| 14 | §3.3 | `[C]` "GHCR/Quay/ECR store what you push" | **MISCLASSIFIED** — Quay also synthesises on demand (§G.1) |
| 15 | §2.5 | `[O] LOCAL HIGH` generalized to "Docker Hub does not content-negotiate", then to registries | **MISCLASSIFIED** — true for Hub (I extended to 7 repos); false for Quay/GHCR (§A.2, §G.1) |
| 16 | §2.6 | alpine index has "20 entries" | **CORRECTED** — 16 entries (9218 bytes is exact) (§B.3) |
| 17 | §7.8 | "107 directories in `/var/lib/docker/overlay2/`" | **CORRECTED** — 106 dirs + 106 symlinks (§K.1) |
| 18 | §7.7 | `[F] HIGH` "unlinking a live overlay `lowerdir` does not fail" | **UNSUPPORTED** — unsourced, untested; recommendation still right (§K.3) |
| 19 | §5.4 | rootless `mknod` EPERM / `userxattr` behaviour | **UNSUPPORTED — pending host** (ROCKY NXDOMAIN); commands given (§I.4) |

### Omissions that change the design

- **§5.3 lacks the `trusted.overlay.opaque = "x"` marker** required alongside xattr-form
  whiteouts. Writing them without it can produce whiteouts the kernel ignores. (§I.2)
- **§J.1: reversing `lchown`/`lchmod` also destroys every `security.capability` xattr**, not just
  setuid bits. Second silent failure, not mentioned. (§J.1)
- **§2.3's auth middleware needs a second rule**: never run a token handshake against a host
  learned from image content (`urls`, redirect targets). That is what CVE-2020-15157 actually is.
  (§F.3)
- **`Accept` can change which content a tag resolves to** (Quay: 3 digests). Any cache keyed on
  `(repo, tag)` must key on the `Accept` set too, or resolve once and pin the digest. (§G.2)
- **`404` is not proof of absence** on GHCR when `Accept` is wrong; pairs badly with the already-
  noted "401 means private-or-absent". (§G.2)
- **§3.4's Schema 1 error text** ("image was pushed before 2017") is wrong advice on Quay, where
  Schema 1 is what a bad `Accept` gets you for a modern image. (§G.2)
- **`history` is OPTIONAL** in `config.md`; §4.4 assumes it is present. (§H)
- **`manifest.layers` is not marked REQUIRED** in `manifest.md`. (§E.9)

### Confirmed

**30 verdicts returned CONFIRMED** (several of them grouped — e.g. "all twelve `layer.md`
whiteout quotes" is one verdict over twelve checked items, so the count of individually verified
quotes and measurements is roughly double). They include every one of the load-bearing claims: the six-way
`Accept` matrix on Hub, the Schema-1-on-`alpine:2.6` behaviour, the `alpine:3.9` Docker manifest
list, the full 401→token→retry flow with its three challenge variants, `expires_in: 300`,
`token == access_token`, the JWT rate-limit parameters, the blob 307/CloudFront/`Range` triple,
all twelve `layer.md` whiteout quotes, all three `overlayfs.rst` quotes, both privilege man-page
citations, the char-device-0/0 representation with real on-disk evidence, the entire OCI-vs-Docker
compatibility matrix, every number in the two-digest layer measurement, the `history`/`empty_layer`
mismatch, the chain-ID formula across 25 layers, and every detail of the containerd and Docker
store layouts.

### The two headline judgments

**"Docker Hub ignores `Accept`" — TRUE, and I would go further.** I reproduced it six ways
including two disjoint-`Accept` probes the document didn't run. Hub does not content-negotiate on
the manifest endpoint at all. **But the client-design rule it drives is right for a reason the
document doesn't give:** the rule is not "registries ignore `Accept`" — Quay and GHCR very much do
not — it is that *no registry's behaviour can be predicted from the request*, so the response
`Content-Type` is the only thing you can branch on. Hub ignores it, Quay converts, GHCR 404s.
Three registries, three behaviours, one rule. The document's conclusion survives; its
generalization does not.

**The chain-ID / storage-key recommendation — ADOPT IT.** The formula is exactly right (verified
against normative spec text and reproduced byte-exactly across 25 layers in 4 images), the
`diff_id`-vs-chain-ID distinction is real and necessary, and the three-index split matches what
both incumbents do. My one objection is to the *justification*: re-keying a content-addressed store
is not an expensive migration, because every key is recomputable offline from data already on disk.
The genuinely irreversible decision in this document is **`tar-split` capture during extraction**
(§5.5/§15.6), which cannot be retrofitted without re-extracting the store. §11.5 should rank that
first and demote the storage keys.

### Unsettled

- Which rate-limit window governs (JWT 6 h vs header 1 h). JWT matches Docker's published anonymous
  policy; the header's `w=3600` matches nothing. Cannot be settled without waiting out a window.
  Nothing depends on it — back off on `429` + `Retry-After`.
- Whether Hub's free-`HEAD` behaviour is stable. Measured once, from one IP, one day, no spec
  backing. Prefer HEAD (never worse); do not depend on it.
- ECR, GCR/AR, Harbor, and a local `registry:2` remain untested. Given that two of the three
  registries I did test deviate from the Hub-derived model, I would not claim registry independence
  until at least `registry:2` and ECR are exercised.
- All rootless-overlay claims (§5.4, and §8.9 in the other verifier's scope) remain documentary —
  ROCKY is NXDOMAIN.
