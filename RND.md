# Rust Container Engine — R&D

**Status:** research complete. No code, no `SPEC.md`, no task list.
**Corpus:** 24,500 lines across 17 files in [`research/`](research/). This document is the
digest and the router — it carries the conclusions; the corpus carries the evidence.

**How to use this document.** Load the [Priority Index](#priority-index) alone to orient.
Pull an individual section only when you need it. Pull a `research/` file only when a section
tells you the detail is load-bearing for what you are deciding.

**Epistemic status.** Every conclusion here has survived an adversarial verification pass in
which seven fresh agents — none of whom wrote the research — attempted to falsify it. Across
1,828 tagged claims: **419 confirmed, 111 corrected, 74 refuted, 52 unsupported, 49
misclassified, 25 outdated.** Roughly a quarter of the load-bearing claims needed revision.
**The research files (`D1`–`D5`) were deliberately not edited afterwards**, so where a `V*`
file contradicts a `D*` file, *the verification wins*. The full ledger of what changed is
[`research/PHASE4-risks-and-reconciliation.md`](research/PHASE4-risks-and-reconciliation.md)
Part 1.

Three categories are kept strictly separate throughout, per the research standard:

- **Fact** — what Linux or the OCI specs actually require.
- **Convention** — what Docker/containerd/runc/podman happen to do.
- **Recommendation** — what this engine should do, with the tradeoff stated.

---

## Priority Index

Sorted by importance to future architecture, not by document order.

### P0 — decide these correctly or pay for it repeatedly

| | Section |
|---|---|
| P0 | [What a Linux container actually is](#2-what-a-linux-container-actually-is) — there is no kernel object called a container |
| P0 | [The container creation sequence](#4-the-container-creation-sequence) — 42 steps; namespaces *are* the clone |
| P0 | [Recommended architectural boundaries](#7-recommended-architectural-boundaries) — daemonless, one binary, sync core |
| P0 | [Kernel primitives and what is actually required](#3-kernel-primitives) — the corrected classification table |
| P0 | [Root filesystem construction](#10-root-filesystem-and-layers) — silent truncation makes this a correctness issue, not a performance one |
| P0 | [Security model](#13-security-model) — process isolation ≠ security isolation, and the ordering bug that erases hardening |
| P0 | [Decisions we should make now](#18-decisions-we-should-make-now) — 22 of them |
| P0 | [Architectural risks](#20-architectural-risks) — the four that constrain everything else |

### P1 — shapes the build, but recoverable

| | Section |
|---|---|
| P1 | [Container lifecycle](#5-container-lifecycle) — create/start split, stop semantics, the missing reaper |
| P1 | [Images and registries](#9-images-and-registries) — the pull path, and what is genuinely irreversible |
| P1 | [cgroups and resource control](#12-cgroups-and-resource-control) — v2 only, path shape, declared vs effective |
| P1 | [Networking](#11-networking) — three modes, not one ladder; own your own chains |
| P1 | [The Rust substrate](#15-rust-ecosystem) — `rustix` primary, and the allocation window |
| P1 | [Implementation progression](#17-recommended-implementation-progression) — 16 rungs, L0–L15 |
| P1 | [OCI ecosystem](#8-oci-ecosystem) — three specs, and the v1.3.0 change |

### P2 — real, but later

| | Section |
|---|---|
| P2 | [Runtime architecture comparison](#6-runtime-architecture-comparison) — why the Docker chain exists, and which reasons are ours |
| P2 | [Rootless containers](#14-rootless-containers) — one seam, deferred deliberately |
| P2 | [Reference implementation lessons](#16-reference-implementation-lessons) |
| P2 | [Decisions to postpone](#19-decisions-we-should-deliberately-postpone) |
| P2 | [Open research questions](#21-open-research-questions) |
| P2 | [Source index](#22-source-index) |

---

## 1. Executive findings

The twelve findings that most change what this project should build.

1. **A container is not a kernel object.** No syscall creates one. It is a process whose
   namespace membership was chosen at `clone3` time and whose policy was applied in the
   window between `clone3` and `execve`. Everything else — images, networks, the CLI — is
   userland arranging that window. *Fact.*

2. **Namespaces are not a step after process creation; they are the process creation.**
   The common mental model ("fork, then set up namespaces") is wrong for every namespace
   except those a process may join later. Getting the flag set right at `clone3` is the whole
   game. *Fact.* See [§4](#4-the-container-creation-sequence).

3. **`mount(2)` silently truncates over-length options at 4,095 bytes and parses what
   remains.** Verified by re-running: 6 of 34 realistic layer layouts mounted **with layers
   missing and no error**; one mounted read-only with `EROFS` on write and no error. This
   turns overlay assembly from a performance concern into a **silent-corruption** concern,
   and mandates an explicit length refusal. *Fact, measured, kernel 6.11.*

4. **The obvious capability-drop ordering is inverted and fails silently.** `setuid` before
   `PR_CAPBSET_DROP` clears `CAP_SETPCAP` from the effective set, so every bounding-set drop
   returns `EPERM` and the container runs with the **full 41-capability bounding set** while
   appearing hardened. `PR_SET_KEEPCAPS` does not save you — it preserves *permitted*, not
   *effective*. *Fact.* This is why security moved down the ladder to rung 4.

5. **`clone3` is not subject to glibc `fork`'s constraint** — it does not lock NSS, libio or
   the malloc arenas. This *strengthens* the sync-only design rather than weakening it, and
   it is the real reason a Rust runtime can be simpler here than runc's Go one. *Fact.*

6. **The OCI Runtime Spec changed in v1.3.0 (2025-11-04):** `poststart` hook failure now
   requires the runtime to error, **stop the container**, and proceed to teardown. Anything
   written from the v1.2.0 text is non-conformant in the unsafe direction. *Fact.*

7. **Registries disagree about `Accept`, so the request predicts nothing.** Docker Hub ignores
   the header entirely, Quay synthesises Schema 1 *at serve time*, GHCR returns 404. The rule
   is to branch on the response `Content-Type` — and note that `Accept` can change *which
   content a tag resolves to*. *Fact, measured across three registries.*

8. **Host state outlives the process, and a daemon restart does not fix it.** A live
   production host was found carrying a bridge, a MASQUERADE rule and two anti-spoof DROPs
   for containers long gone, invisible to `docker network inspect` and absent from its own
   database. Docker's own cleanup only touches the eleven chains it owns; the leaked rules sat
   in built-in chains. The design rule is **never write state into chains you don't own.**
   *Observation, 186 days uptime.*

9. **Orphaned `exec` children reparent to the container's PID 1, not to a host subreaper** —
   the kernel forbids cross-namespace reparenting. A container init that does not reap
   accumulates zombies, so `--init` is required rather than a convenience. *Fact, measured.*

10. **`openat2(RESOLVE_IN_ROOT)` is not a complete extraction defence.** An escape to
    `/etc/hosts` was demonstrated using an openat2-resolved parent plus one plain `openat`.
    `RESOLVE_NO_XDEV` is needed as well, and `RESOLVE_BENEATH` is the better primitive because
    it errors where `RESOLVE_IN_ROOT` clamps silently. *Fact, measured.*

11. **`rustix` is the right substrate, for a reason nobody stated.** It survived every
    coverage check, but the argued justification (glibc locking) is unsupported. The real
    mechanism is that `__errno_location()` is a TLS read, so under `CLONE_VM` without
    `CLONE_SETTLS` the child writes the **parent's** errno slot. Also: rustix places
    `clone3`/`fork`/`execve` explicitly out of scope, so the two syscalls at the centre of
    this project were always ours to write. *Fact.*

12. **Documentation lies, and primary sources include the source.** Three separate
    documentation defects were found and confirmed against kernel code: `ip-sysctl.rst`
    misdescribes what enabling `ip_forward` does; `unshare(2)` wrongly claims `CLONE_NEWPID`
    implies `CLONE_THREAD`; and the overlayfs docs misdate `lowerdir+`. Where a claim is
    load-bearing, read the kernel, not the manual.

---

## 2. What a Linux container actually is

**Fact.** There is no kernel object, syscall, or data structure called a container. `ps` on
the host shows an ordinary process. What distinguishes it is the conjunction of:

- **namespace membership**, chosen in the `clone3` flag set at creation (and, for a few
  namespaces, joinable afterwards via `setns`);
- **a root filesystem** the process was moved into with `pivot_root`, after mount propagation
  was made non-shared;
- **cgroup membership**, ideally established atomically at creation via `CLONE_INTO_CGROUP`;
- **policy applied in the pre-`execve` window** — capability bounding set, `no_new_privs`,
  seccomp filter, LSM label, closed file descriptors.

**Fact.** Each of these is independent. A new PID namespace changes process-ID visibility for
descendants and nothing else: it does not isolate mounts, networking, resources, capabilities,
or the filesystem. A process in all eight namespaces with no capability drops, no seccomp
filter and a writable `/sys/fs/cgroup` is fully *process-isolated* and trivially escapable.

**Recommendation.** Treat "container" as a userland abstraction this engine defines, and be
explicit in the code about which of the four groups above a given stage has actually
established. The ladder's rungs 1–4 produce things that look like containers and are not
secure; [§13](#13-security-model) states where the line is crossed.

Depth: [`D1-kernel-isolation.md`](research/D1-kernel-isolation.md) §1, corrected by
[`V1`](research/V1-kernel-isolation-verification.md).

---

## 3. Kernel primitives

**Fact — the eight namespaces.** `mnt`, `pid`, `net`, `uts`, `ipc`, `user`, `cgroup`, `time`.
Load-bearing details that are commonly gotten wrong:

- `CLONE_NEWPID` does not move the caller; it affects children. The first child is PID 1.
- `setns` on a **pidfd** enters *many* namespaces atomically — since Linux 5.8. `mydocker exec`
  is therefore one `setns` on a pidfd we already hold, not six sequential calls.
- Only **PID** is children-only for `setns`. Time is not — `setns` on a time-ns fd moves the
  caller. *(Corrected: the research said otherwise.)*
- `setns(CLONE_NEWTIME)` returns `EUSERS` when multithreaded — a second single-threaded
  restriction beyond the well-known `CLONE_NEWUSER` one.
- `unshare(2)`'s claim that `CLONE_NEWPID` implies `CLONE_THREAD` is **a documentation bug**;
  the kernel adds `CLONE_THREAD` only for `CLONE_NEWUSER`.

**Fact — mount propagation and `pivot_root`.** `pivot_root(2)` has **six** preconditions, not
five. `new_root == "/"` yields `EBUSY`, not `EINVAL`. Tests that run while `/` is still
`MS_SHARED` measure nothing else, because the kernel's shared check short-circuits the rest.

**Fact — the corrected requirement tiers.** The research's classification table had three
entries in the wrong tier:

| Mechanism | Research said | Verified |
|---|---|---|
| `MS_REC\|MS_SLAVE` on `/` | required | **Not `MS_SHARED`** is required; crun ships private |
| `/proc` mounted | required | Convention — `sh`, `id`, `python3` all run without it |
| `CLONE_NEWPID` | required | Convention — `--pid=host` ships in Docker |
| `CAP_MKNOD` dropped | hardening | **Required** — it is a correctness boundary, not polish |
| `CLONE_INTO_CGROUP` | convenience | **Required** for race-free accounting |

**Recommendation.** Probe features at runtime; never version-gate. RHEL 8's 4.18 kernel has
unprivileged overlay that upstream only got in 5.11 — a version check would wrongly exclude a
capable host, and wrongly include an incapable one.

Depth: [`D1`](research/D1-kernel-isolation.md) §1–§9;
[`V1`](research/V1-kernel-isolation-verification.md) for every correction.

---

## 4. The container creation sequence

**The full annotated trace is
[`research/TRACE-container-creation.md`](research/TRACE-container-creation.md)** — 42 steps,
each with actor, kernel primitive, privilege requirement, MVP status, failure mode and
ordering constraint. Read it before writing any of the runtime.

**Fact — the shape.** Five actors, not two: CLI, puller, supervisor, parent, child.

```
HOST (CLI / puller / supervisor)
 1 parse → 2 pull → 3 extract → 4 assemble overlay dirs → 5 mount overlay
 → 6 state dir → 7 cgroup + limits + dirfd → 8 IP alloc → 9 exec.fifo + socketpairs
 → 10 assert Threads==1 → 11 pre-build argv/envp/paths/seccomp
 → 12 clone3(6 namespaces + CLONE_PIDFD + CLONE_INTO_CGROUP)

PARENT (concurrent with child)
 13 pidfd → 14 uid_map → 15 cgroup verify → 16 netns plumb → 17 hooks
 → 18 ready/state.json → 19 start reads fifo → 20 supervise

CHILD (must be the child; the parent cannot do these)
 21 / not MS_SHARED → 22 MS_PRIVATE on new_root's parent → 23 self-bind
 → 24 cgroup ns → 25 mounts → 26 devices → 27 binds → 28 pivot_root
 → 29 re-open /dev/null → 30 masking → 31 hostname → 32 pty from container's /dev/ptmx
 → 33 setsid/TIOCSCTTY → 34 chdir → 35 LSM → 36 seccomp-A → 37 caps
 → 38 no_new_privs → 39 close_range → 40 fifo write → 41 seccomp-B → 42 execve
```

**Fact — why the split exists.** The parent cannot mount into a mount namespace it is not in.
LSM labels, seccomp filters and capability changes are per-thread and irreversible. The parent
*could* join via pidfd-`setns`, but must not — that would move the supervisor itself out of the
host.

**Corrections against the naive sequence** (the brief's proposal, and most tutorials):
namespaces are the clone rather than a step after it; UID/GID mapping is a **parent** action;
cgroup setup happens **before** the clone; the capability/seccomp order is two branches
depending on `no_new_privs`; and five steps are missing entirely from the naive version —
`Threads == 1`, pre-building all allocations, `close_range`, propagation-first, and the memfd
self-seal.

**Fact — a trap in the hardening itself.** The read-only remount must re-specify
`nosuid`/`nodev`/`noexec`, read back via `statfs` — otherwise the remount *clears* them, and
the hardening step weakens the container.

---

## 5. Container lifecycle

**Fact.** The OCI lifecycle is `creating → created → running → stopped`, with `create`,
`start`, `kill`, `delete`, `state`. The `create`/`start` split exists so that a caller can
place the process in cgroups, plumb the network and run hooks while the container is fully
constructed but not yet executing the user's program. runc's rendezvous is `exec.fifo`: the
child blocks writing to it, `start` unblocks by reading.

**Fact.** "Kill the container" is not "kill PID 1". SIGTERM to a handler-less PID 1 is silently
discarded — even from the host — and `exec` children are not in PID 1's process tree. The
cgroup is the authoritative membership list, and `cgroup.kill` (5.14) is the only race-free
stop. Below 5.14, walk `cgroup.procs` repeatedly.

**Fact — the reaper gap.** Orphaned `exec` children reparent to the *container's* PID 1
because the kernel forbids cross-namespace reparenting. A host-side subreaper catches only
host-level orphans. Without an in-container reaper, zombies accumulate inside the container.

**Recommendation.** Ship `--init`. Stop = SIGTERM → grace period → `cgroup.kill`. State is a
file per container written by atomic rename, guarded by a per-container `flock`, carrying a
start-time PID guard and `boot_id` so a reused PID cannot resurrect a dead container.

Depth: [`D2`](research/D2-runtime-architecture.md) §1, §5, §6; corrections in
[`V2`](research/V2-runtime-architecture-verification.md); risk entries R13, R16.

---

## 6. Runtime architecture comparison

**Convention — why the Docker chain has four layers.** `docker` CLI → `dockerd` → `containerd`
→ `containerd-shim-runc-v2` → `runc`. Each boundary buys something specific: the daemon
survives CLI exit; containerd owns image and snapshot state; **the shim exists so containers
survive a daemon restart** — it holds the container's stdio open, reparents when the runtime
exits, and collects exit status; runc is a short-lived program that configures and `execve`s.

**Fact — what the shim actually holds.** Inspecting a live shim's fd table: it holds only the
**read** ends of the container's pipes. The both-ends arrangement applies to the docker-side
FIFO — the hop this design deletes. *(The research asserted the opposite, and following it as
written would have broken the drain-to-EOF path.)*

**Fact — the console socket.** runc does *not* openpty on the host. `runc init` allocates the
pty **inside the container after `pivot_root`** and passes the master up via `SCM_RIGHTS`. The
host-side arrangement is one runc deliberately moved away from (runc#814).

**Convention — the families.** Podman/conmon is daemonless with a per-container supervisor.
CRI-O is Kubernetes-shaped. gVisor moves the boundary into a userspace kernel. Kata/Firecracker
move it into a VM. **Apptainer/Singularity** is the fourth family and the most instructive: no
daemon, no persistent state, no cgroups, a single-file image, and the stance that a container
is a process you `exec`, not a service you manage.

**Recommendation.** The one sentence worth carrying from all of them: *almost every mechanism
above the fork/exec/namespace/cgroup layer exists to solve "my supervisor is not my client's
parent."* Apptainer's architecture is the observation that if you never create that problem,
you need none of the solutions. This engine creates it deliberately — but only at the rung
where `-d` is introduced, and the price is then explicit.

Depth: [`D2`](research/D2-runtime-architecture.md) §2, §10, §11; [`V2`](research/V2-runtime-architecture-verification.md).

---

## 7. Recommended architectural boundaries

All **Recommendation**, with the reasoning stated.

| Boundary | Decision | Why |
|---|---|---|
| Daemon | **None.** Per-container supervisor, per-container `flock` | Nothing in the design forbids adding one later; a daemon's only unique capability is a push channel we don't need |
| Binary | **One**, with subcommands | CLI↔runtime is an argv + state-file contract |
| Supervisor | Exists **only** where a feature requires it | `-d` needs it; foreground `run` does not |
| Phase order | **Foreground `run` first**, `-d` is the same code path after `setsid()` | Inverts the research's phasing; the detached-first ordering contradicted its own "a container is a process, not a service" position and rested on a selectively quoted source that recommends the opposite |
| Concurrency | **Sync core. The cloning process has never had a second thread** | `clone3` forfeits glibc's atfork protection; a second thread makes the pre-`execve` window unsound |
| Async | Confined to a **separate process** for the pull path | Registry I/O is genuinely parallel; the runtime core is syscall-sequential |
| Image ↔ execution | Coupled only through **the store on disk plus a `Mount` description** | Keeps the puller replaceable and the runtime testable offline |
| Privilege | One `Privilege` struct; **`geteuid()` appears exactly once in the codebase** | Every other call site is a future rootless audit |
| Host state | **One table of our own, our own netns directory, live-set GC** | See [§11](#11-networking) |

**Fact that constrains all of the above.** `setns` needs no dedicated thread when the process
is single-threaded — it moves the process. This dissolves an apparent conflict between the
networking design (which asked for a dedicated OS thread) and the sync-core rule (which forbids
one): save `/proc/self/ns/net`, `setns` into the container's, do the work, `setns` back.

Depth: [`PHASE4`](research/PHASE4-risks-and-reconciliation.md) R1, R2, R3, R7, R11.

---

## 8. OCI ecosystem

**Fact — what each specification controls.**

| Spec | Controls | Does not control |
|---|---|---|
| **Image Spec** | Layer/manifest/index/config formats, media types, digests, whiteout conventions, the on-disk image layout | How you run anything |
| **Distribution Spec** | The registry HTTP API: `/v2/`, auth challenge, manifest and blob endpoints, referrers | What the bytes mean |
| **Runtime Spec** | The bundle (`config.json` + `rootfs`), lifecycle state machine, hooks, namespace/cgroup/capability/seccomp configuration | **Networking** — deliberately out of scope; images; registries |

**Fact — the terminology trap.** The *image* config and the *runtime* `config.json` are
different files with different schemas. An OCI **image** is manifest + config + layers, content
addressed. An OCI **bundle** is a directory containing a `config.json` and an extracted
`rootfs`. Converting the first into the second is the engine's job and is not itself specified.

**Fact — a recent change.** Runtime Spec **v1.3.0 (2025-11-04)** redefined `poststart` hook
failure: the runtime MUST generate an error, **stop the container**, and proceed to teardown.
Earlier text merely logged a warning.

**Fact — a spec MUST that is easy to file as a nicety.** *"Generating an error MUST leave the
state of the environment as if the operation were never attempted."* This is a conformance
requirement, and it is what makes the failure-unwind path in
[`TRACE`](research/TRACE-container-creation.md) §6 mandatory rather than tidy.

**Recommendation — conformance targets.** MVP: none claimed; aim for *interoperability*
(running a real `alpine` from Docker Hub) rather than paper conformance. Intermediate: bundle
compatibility, so an existing `config.json` can be consumed. Mature: the runtime CLI surface and
a conformance claim. Interoperability is cheaper and worth more than either.

---

## 9. Images and registries

**Fact — the pull path.** `alpine` → `docker.io/library/alpine:latest` (the `library/` and
registry defaults are *convention*, not spec) → `GET /v2/` → `401` with
`WWW-Authenticate: Bearer realm/service/scope` → token → manifest `GET` → index → platform
selection → manifest → config blob → layer blobs, verifying the digest at every step.

**Fact — three registries, three behaviours.** Docker Hub **ignores `Accept` entirely** (verified
six ways, including a nonsense header); Quay **synthesises Schema 1 at serve time** — the same
tag returns three different digests for three `Accept` headers; GHCR **404s**. Therefore: send
a four-type `Accept`, but **branch on the response `Content-Type`**. The descriptor beats the
HTTP header. Note the consequence: `Accept` can change *which content a tag resolves to*.

**Fact — indexes are not all images.** Modern indexes are roughly half non-image entries —
BuildKit attestation manifests at `platform: unknown/unknown`. Platform selection must be a
**positive filter**; "first entry" or "exclude unknown/unknown" will eventually try to `exec`
an in-toto attestation as a rootfs, and the latter also lets Windows entries through.

**Fact — digests.** A layer has two: the **compressed** digest that names the blob in the
registry, and the **uncompressed** `diff_id` listed in the image config's `rootfs.diff_ids`. A
puller must compute both. Chain IDs identify a layer *stack*, and the formula reproduces
Docker's `layerdb` directory names byte-exactly — verified across 25 layers in 4 images.

**Fact — security.** Strip `Authorization` on cross-host redirect: this is a spec **MUST NOT**,
not a recommendation. Ignore the descriptor `urls` field entirely — `CVE-2020-15157` is the
foreign-layer field plus an attacker-forged `401`, leaking registry credentials.

**Recommendation — what is actually irreversible.** Not the CAS keying: re-keying is an offline
recompute with no re-download. The irreversible decision is **`tar-split` capture**, because it
can only happen *during* extraction — retrofitting means re-extracting the whole store. Decide
it deliberately at the extractor rung. The xattr namespace policy is the other one-way door.

Depth: [`D3`](research/D3-images-storage.md) §1–§5, §7, §12;
[`V3`](research/V3-oci-registry-verification.md) for 11 refutations and 19 corrections.

---

## 10. Root filesystem and layers

**Fact — and this is a correctness issue, not a tuning issue.** `mount(2)` **silently truncates**
an over-length option string at 4,095 bytes and parses what remains. It does not fail. Measured
on kernel 6.11: the boundary is exactly 4095 OK / 4096 FAIL; **6 of 34** docker-shaped layouts
mounted with layers missing (185 of 200 in one case); a request for a writable overlay with 139
layers came back **read-only with 131 layers and `EROFS` on write, with no error**. It is
deterministic per layout, which is precisely why casual re-testing does not reproduce it.
`ELOOP` is *not* the characteristic errno — it genuinely means overlapping layers.

**Fact — `lowerdir+` escapes the limit, with three ceilings.** `fsconfig(FSCONFIG_SET_STRING,
"lowerdir+", …)` accepts far more than one page — 500 layers verified. But: the stack ceiling is
exactly `OVL_MAX_STACK = 500`; each value is capped at 255 bytes; and the **non-append**
`lowerdir` form is capped at 255 too, which is *worse* than `mount(2)`. It landed in **v6.7**
(the kernel docs say 6.8 and are wrong), and `FSCONFIG_SET_FD` is v6.13.

**Fact — ordering.** `lowerdir` is the exact **reverse** of `rootfs.diff_ids` — leftmost is the
topmost layer. Proved three ways, including a parent-walk on a live production container.

**Fact — assorted traps.** A read-only overlay needs **≥2 lower layers**, so a single-layer image
(`alpine`) cannot satisfy the naive "mount it read-only" test. Shared `upperdir`/`workdir`
between two mounts does **not** give `EBUSY` — the kernel only warns, so exclusivity is ours to
enforce. tmpfs *does* accept `trusted.overlay.*` and works as an upperdir.

**Recommendation.** Probe for `lowerdir+`; fall back to `mount(2)` **with a mandatory 4,095-byte
refusal** and `lowerdir` written last. Never version-check. The kernel floor is 5.10, so
`lowerdir+` is a probe-gated fast path, not a baseline assumption.

**Recommendation — extraction safety.** `openat2` with `RESOLVE_IN_ROOT` **and**
`RESOLVE_NO_XDEV` — or better, `RESOLVE_BENEATH`, which errors rather than clamping silently.
Every path component must go through it: an openat2-resolved parent plus one plain `openat` was
shown to escape. Use the `tar` crate as a **parser only**, never `unpack`/`unpack_in`
(`RUSTSEC-2026-0067`), and pin ≥ 0.4.45 for the parser bug too (`RUSTSEC-2026-0068` — a PAX
size-header desync, which is a layer-content-versus-digest confusion primitive). `lchown` before
`lchmod`, or setuid bits *and* every `security.capability` xattr silently vanish.

Depth: [`D3`](research/D3-images-storage.md) §6, §8–§11;
[`V4`](research/V4-storage-overlay-verification.md) — the truncation reproduction is the single
most valuable result in the corpus.

---

## 11. Networking

**Fact.** The OCI Runtime Spec deliberately does not define networking. This is ours to design.

**Fact — the published-port path.** Inbound → `nat/PREROUTING` → the `DOCKER` chain → DNAT to
the container IP → `filter/FORWARD` → bridge → container; egress via `MASQUERADE` in
`nat/POSTROUTING`. `docker-proxy` coexists with DNAT rather than replacing it, and exists for
the cases DNAT alone cannot serve (localhost publishing, address-family crossing).

**Fact — a widely repeated warning that is false.** Enabling `net.ipv4.ip_forward` does *not*
reset `rp_filter`, `route_localnet`, `accept_local` or `arp_ignore`. `inet_forward_change()` at
v6.11 does exactly four things, and an unchanged write is a no-op. The kernel's own
`ip-sysctl.rst` is stale here. The real side-effect it *does* have, undocumented in the research:
LRO is disabled permanently on every netdev and never restored.

**Fact — a published port cannot be closed by the host firewall.** `ufw route deny 80` lands in
`ufw-user-forward`, *after* `DOCKER-FORWARD`'s terminal ACCEPT. No ufw command closes a published
port. This is the most dangerous single operation in the networking ladder — not `ip_forward`.

**Observation — leaks are real and survive restarts.** A production host was carrying a bridge
with an address, a MASQUERADE rule and two anti-spoof DROPs for containers long gone, invisible
to `docker network inspect` and absent from its own boltdb, alongside 6 sandbox records against
5 netns mounts. Six innocent explanations were ruled out individually. Crucially, a daemon
restart would *not* have cleaned it: `removeIPChains()` only touches the eleven Docker-owned
chains, and the leaked rules were in built-in ones.

**Recommendation — the design rules that follow.**
- **Never write state into chains you don't own.** One table of our own; our own netns
  directory; garbage-collect against the live set at startup.
- **Release order is a security property.** Release an IP *last*, after every rule naming it is
  gone, and flush conntrack — otherwise a premature release is a cross-tenant misroute rather
  than mere address exhaustion.
- **Three modes, not one ladder.** `--network=host` is a permanent orthogonal mode (omit
  `CLONE_NEWNET`); empty-netns is part of basic isolation; routed NAT egress comes *before* the
  bridge, since veth + `ip_forward` + MASQUERADE + `resolv.conf` is already `apt-get`-capable.
- Speak netlink via the codec crates, sync. `iptables` on modern distros is nftables underneath.

Depth: [`D4`](research/D4-networking.md); [`V5`](research/V5-networking-verification.md).

---

## 12. cgroups and resource control

**Fact — v2 model.** Unified hierarchy; `cgroup.subtree_control` enables controllers *for
children*; the **no-internal-process rule** forbids a cgroup from holding processes and
distributing to children simultaneously. This dictates the layout directly: `mydocker/`
distributes and holds nothing, `mydocker/<id>/` holds processes and distributes nothing.

**Recommendation.** Create `mydocker/<id>/init` from day one. Adding a per-container `exec`
sub-cgroup later would otherwise require moving a live PID 1.

**Fact — limits.** `--memory` → `memory.max` (with `memory.high` for backpressure rather than
hard kills, and separate `memory.swap.max`); `--cpus` → `cpu.max` quota/period (distinct from
`cpu.weight`, which is shares); `--pids-limit` → `pids.max`. Detect OOM via `memory.events`, not
by inference.

**Observation — declared is not effective.** Every live container on the host showed
`pids.max = 19147`, inherited from systemd's `DefaultTasksMax` — while `docker inspect` reported
no limit at all. **`mydocker inspect` must be able to report effective-vs-declared**, or it will
confidently lie.

**Fact — build order is settled, not a coin flip.** runc's systemd cgroup driver *calls*
`fsMgr.Set()` in addition to D-Bus (`UnifiedManager.Set`, v1.2.5). The fs driver is a component
of the systemd driver, not an alternative to it. Build `FsManager` first.

**Recommendation.** cgroup v2 only. Detect v1/v2/hybrid at startup and **fail loudly** if a
limit was requested and cannot be enforced; warn once if none was requested; never silently
succeed. On a machine like a Slurm-managed HPC login node, "we cannot limit resources here" is
the honest answer and should be said out loud.

**Fact — `io.weight` caveat.** That file belongs to iocost; BFQ's is `io.bfq.weight`. On a host
with neither model loaded it is a silent no-op — but that is a per-host property, not a reason
to drop the feature. Probe and refuse loudly.

Depth: [`D5`](research/D5-resources-security.md) §1–§6; [`V6`](research/V6-resources-security-verification.md).

---

## 13. Security model

**Fact — the distinction, stated operationally.** *Process isolation* means the process cannot
*see* host resources. *Security isolation* means it cannot *reach* them. A process in all eight
namespaces, with all 41 capabilities, no seccomp filter and a writable `/sys/fs/cgroup` has
complete process isolation and is trivially escapable. The two are independent, and the first
is not evidence of the second.

**Fact — the ordering bug that erases hardening.** `setuid` before `PR_CAPBSET_DROP` clears
`CAP_SETPCAP` from the effective set, so every bounding-set drop returns `EPERM` — leaving the
full 41-capability bounding set, with no error and no visible symptom. `PR_SET_KEEPCAPS` does
not rescue it: it preserves *permitted*, not *effective*; a `capset` must sit between.

**Fact — capabilities.** The **bounding set** is the security boundary; permitted and effective
are convenience. A root container regains its whole bounding set on every `execve`. `CAP_SYS_ADMIN`
is effectively root. `CAP_MKNOD` is a **required** drop, not hardening — gate `--cap-add MKNOD`
the way `SYS_ADMIN` is gated.

**Fact — seccomp.** Docker's default profile is an **allowlist** (`defaultAction:
SCMP_ACT_ERRNO`, 426 names), not "blocks 44 syscalls" — and an allowlist structurally eliminates
the x32 bypass, though i386 numbers *collide* and still need an explicit architecture prologue.
`clone3` is denied with **`ENOSYS`, not `EPERM`**, so glibc falls back rather than failing. Note
that `ptrace`, `process_vm_readv` and `process_vm_writev` are **allowed** by that profile — the
"capabilities and seccomp are defence in depth" story has no second layer here.

**Fact — masking.** Masked directories become read-only tmpfs; masked files become bind mounts
of the container's own `/dev/null`; read-only paths are self-bind plus remount-ro. Four of
Docker's twelve masked paths do not exist on kernel 6.11, so masking must skip `ENOENT` — **and
only** `ENOENT`.

**Fact — escapes worth designing against.** `CVE-2019-5736` (re-opening `/proc/self/exe`; the
bait is inside the container and it covers `run`, not just `exec`), `CVE-2022-0492` (cgroup v1
`release_agent`), `CVE-2024-21626` (leaked fds — close everything before `execve`), and
`CVE-2021-30465` (mount-target TOCTOU; the research cited the wrong CVE here).

**Recommendation — defaults, with the corrected reasoning.** `no_new_privs` **on** (its real
benefit is collapsing the two-branch seccomp install and enabling LSM transitions — the
"closes the setuid escape" headline is overstated for a *root* container, which regains
capabilities regardless). Drop **`CAP_MKNOD`** (required) and **`CAP_NET_RAW`** (pair it with
`net.ipv4.ping_group_range=0 2147483647` in the container's netns, which removes the `ping`
cost entirely; the unmentioned real costs are `AF_PACKET`/DHCP and `IP_TRANSPARENT`). Deny
`ptrace` by default. Seal `/proc/self/exe` into a memfd at startup and set `PR_SET_DUMPABLE(0)`
on the `exec` path.

**The shortcuts this engine must not ship silently:** `chroot` instead of `pivot_root`; leaving
`/` shared; retaining all capabilities; no seccomp; writable `/proc` or `/sys`; sharing the host
IPC/net/PID namespace by default; no cgroup limits (a fork bomb then takes the host down); fds
leaked across `execve`; mounting the host `/` or the docker socket.

Depth: [`D5`](research/D5-resources-security.md) §7–§15; [`V6`](research/V6-resources-security-verification.md).

---

## 14. Rootless containers

**Decision already taken: root first, rootless later.** This section exists to keep that cheap.

**Fact — what actually breaks without privilege.** veth attached to a host bridge needs
`CAP_NET_ADMIN` **in the host namespace** — impossible rootless, so usermode networking
(`pasta`, preferred over `slirp4netns`) is the only route. cgroup limits need a delegated
subtree, which needs a systemd user session — on a Slurm-managed host there is none, and the
honest answer is that limits cannot be enforced. Without `/etc/subuid` entries only a
single-UID map is possible, so any image with files owned by a second UID breaks. Overlay
needs `userxattr`, or `fuse-overlayfs`.

**Recommendation — the seam.** One `Privilege` struct, probed once at startup, carrying the
idmap / cgroup / network / device / overlay strategies. **`geteuid()` appears exactly once in
the entire codebase.** Not an `is_root: bool` — the interesting cases are *root with no writable
cgroup* and *rootless with full delegation*, and a boolean cannot express either. Build it with
only the rootful arms implemented and `unimplemented!()` for the rest; bind-mount device nodes
from day one rather than `mknod`, since that path works in both modes.

Depth: [`D5`](research/D5-resources-security.md) §16–§17; [`PHASE4`](research/PHASE4-risks-and-reconciliation.md) R8.

---

## 15. Rust ecosystem

**Recommendation — the substrate.** **`rustix` primary**, `libc` for syscall numbers and struct
layouts only. It survived every coverage check: pidfds, the full capget/capset + bounding +
ambient surface, non-broadcasting `set_thread_res_uid`, and the new mount API — none of which
`nix` has in full. The `linux_raw` backend issues syscalls by inline asm with no libc.

**Fact — the real justification, which the research did not state.** Not "glibc takes locks"
(its thin syscall wrappers do not). It is that `__errno_location()` is a **TLS read**, so under
`CLONE_VM` without `CLONE_SETTLS` the child writes the *parent's* errno slot.

**Fact.** `rustix` places `clone3`, `fork` and `execve` explicitly out of scope, and has no
`signalfd`. So the two syscalls at the centre of this project were always ours to write, and the
supervisor needs a hand-written `signalfd4` **plus** a pidfd — a pidfd alone gives exit
detection only, and a supervisor with no signal handler dies mid-`poll` on SIGTERM without
forwarding to PID 1 or writing its state file.

**Fact — the allocation window.** Both `nix` and `rustix` fall back to allocating for long
paths. The rule "child-side signatures take `&CStr` only" is necessary but **not sufficient** —
it misses seccomp filter `Vec`s, argv/envp arrays, and any `std::fs` in the child. Pre-compile
the filter and pass `&[sock_filter]`; pre-build argv/envp; add an aborting allocator in the child.
*(Two agents reached this independently.)*

**Fact — advisories and licences.** `RUSTSEC-2026-0067` and `-0068` (tar; pin ≥ 0.4.45);
`RUSTSEC-2025-0126` (`nftnl`, heap overflow); `RUSTSEC-2025-0069` (`daemonize`).
**`rustables` is GPL-3.0-or-later** — a licensing landmine. `seccompiler`'s repository was
archived in August 2026. `ureq` spawns a detached thread per DNS lookup when a timeout is set,
which conditionally breaks a `Threads == 1` assumption.

**Recommendation — the test.** *Depend on what hides typing; write what hides mechanism.*
Adopt: `rustix`, `libc`, `clap`, `serde`/`serde_json`, `thiserror`, `tracing`. Separate process
only: the async pull stack. Write ourselves: the netlink socket/ack/seq loop, the seccomp filter,
the tar applier, cgroup writes (cgroupfs is `std::fs::write` — a crate is pure overhead).
Read as reference, do not depend on: youki's `libcontainer`/`libcgroups`. Phase-1 `Cargo.toml`
is plausibly six entries.

Depth: [`D2`](research/D2-runtime-architecture.md) §12; [`V7`](research/V7-rust-substrate-verification.md).

---

## 16. Reference implementation lessons

**runc** — the `create`/`start` split via `exec.fifo` is worth copying; the two-stage
`nsexec`/`runc init` design is substantially a **Go artifact** (runtime threads versus `setns`)
that Rust does not inherit. Its failure path calls `terminate()` *before* `Destroy()`
specifically "to ensure we can remove cgroups" — an ordering worth stealing.

**crun** — proof that the architecture works in a single-threaded language, and a caution: it
has the same shape this design proposes and *still* ships the memfd constructor, which is why
"we never re-exec, so `CVE-2019-5736` cannot affect us" is false.

**youki** — the closest reference; calls `pentacle::ensure_sealed()`. It left the `containers`
GitHub org in October 2024 because CNCF Sandbox onboarding required it — benign.

**containerd/Docker** — mostly solving problems this engine does not have (a metadata database,
a plugin architecture, a shim API, CRI). Its overlay2 constants (`maxDepth=128`, `idLength=26`)
are artifacts of the 4,096-byte mount-option limit, derived exactly: `28 × 128 = 3584 = 4096 − 512`.

**Apptainer** — the most useful architectural mirror: it demonstrates how much machinery
disappears when the supervisor is never separated from the client.

Depth: [`D2`](research/D2-runtime-architecture.md) §10–§11.

---

## 17. Recommended implementation progression

**The full ladder is
[`research/LADDER-implementation-stages.md`](research/LADDER-implementation-stages.md)** — 16
rungs, each with goal, new concept, commands supported, explicit non-goals, a runnable success
test, risks, its verified-claim dependencies, and the specific trap that is easy to hit there.

| Rung | Goal | First introduces |
|---|---|---|
| **L0** | Dev VM (≥6.7) + `mydocker probe` | Runtime feature detection; never version-gating |
| **L1** | Fork/exec/wait, exit status | The `Threads == 1` invariant |
| **L2** | Six namespaces, propagation change, fresh `/proc`, `lo` up | `clone3` |
| **L3** | `pivot_root` onto a directory rootfs | The minimum mount set |
| **L4** | Privilege floor | Bounding-set drop, `no_new_privs`, `close_range`, masking, memfd seal |
| **L5** | Seccomp allowlist + arch prologue | **Crosses into security isolation** |
| **L6** | cgroup v2 limits | `CLONE_INTO_CGROUP`; declared-vs-effective reporting |
| **L7** | Supervisor, create/start, durable state | `exec.fifo`, `cgroup.kill` stop, `rm` |
| **L8** | Content store + registry client | Verified bytes, no extraction |
| **L9** | Extractor + `native` storage | **`mydocker run alpine` — first genuinely useful rung** |
| **L10** | Routed NAT egress (veth, no bridge) | First *network* payoff; `resolv.conf` rewrite |
| **L11** | Bridge, published ports | Anti-spoof, foreign-firewall warning |
| **L12** | OverlayFS, writable layer, GC | Layer reuse |
| **L13** | `exec`, `-it`, in-container reaper | pty from the container's `/dev/ptmx` |
| **L14** | OCI runtime CLI, hooks | Conformance *claims* |
| **L15** | *Not a rung* — rootless | The seam that must already exist by L4 |

**Three answers the ladder gives explicitly:**

- **First genuinely useful rung: L9.** Everything before it is infrastructure with no
  user-visible payoff. Saying so is more useful than pretending otherwise.
- **Security isolation is crossed at L5.** Rungs below it produce things that look like
  containers and must never be described to a user as secure.
- **Security moved from rung 10 to rungs 4–5**, departing from the conventional ordering. Three
  reasons: verification re-tiered `CAP_MKNOD` and `CLONE_INTO_CGROUP` from hardening to
  *required*; the failure mode is silent (the capability ordering bug leaves all 41 caps with no
  error); and every item lives inside the same pre-`execve` window L3 has just written, so
  deferring means writing that window twice. Stated cost: no resource limits until L6, which is
  acceptable only on a destroyable VM.

**Other departures from the conventional ladder:** networking is three things rather than one
rung; the registry client precedes the extractor; layered storage comes *after* networking
(`native` storage is correct-but-wasteful, while egress is blocking); foreground `run` precedes
`-d`; and rung 0 exists because the dev VM does not.

---

## 18. Decisions we should make now

Twenty-two decisions that are cheap today and expensive later. Full reasoning and evidence in
[`PHASE4`](research/PHASE4-risks-and-reconciliation.md) Part 2.

| # | Decision |
|---|---|
| R1 | Daemonless; per-container supervisor; per-container `flock`. Nothing forbids a daemon later |
| R2 | Foreground `run` in phase 1; `-d` is the same code path after `setsid()` in phase 2 |
| R3 | One binary, subcommands; CLI↔runtime is an argv + state contract; accept the create/supervise lifetime coupling explicitly |
| R4 | Keep `diff_id`/chain-ID keying, but treat **`tar-split` capture and the xattr namespace** as the irreversible decisions |
| R5 | `lowerdir+` probe-gated; `mount(2)` fallback with a **mandatory 4,095-byte refusal**, `lowerdir` written last |
| R6 | Kernel floor 5.10; probe, never version-gate; record the 6.7 overlay/new-mount-API row |
| R7 | Sync core; async only in a separate process; `setns` needs no thread |
| R8 | Build `Privilege` now, rootful arms only; bind-mount device nodes from day one |
| R9 | cgroup v2 only; `mydocker/<id>/` with `init` from day one; `FsManager` before `SystemdManager` |
| R10 | One firewall table of our own, our own netns dir, live-set GC; `ReleaseIp` is order-critical; add the conntrack flush |
| R11 | Pull path in a separate **process**; the interface is the on-disk store plus a `Mount` description |
| R12 | `no_new_privs` on; drop `CAP_MKNOD` (required) and `CAP_NET_RAW` (with `ping_group_range`); deny `ptrace` |
| R13 | Ship `--init`; a host subreaper covers only host-level orphans |
| R14 | The child allocates the pty from the container's `/dev/ptmx` after `pivot_root`, sends the master up a socketpair |
| R15 | `--device` is gated on the eBPF controller, not shipped without it |
| R16 | Atomic rename + start-time PID guard + `boot_id`; fix the flock inode race and the unlocked supervisor write; delete `external_fds` |
| R17 | Seal `/proc/self/exe` into a memfd at startup; `PR_SET_DUMPABLE(0)` on the `exec` path |
| R18 | signalfd **plus** pidfd; a pidfd alone cannot implement signal forwarding |
| R19 | `&CStr` signatures **plus** pre-compiled `&[sock_filter]` and argv/envp; no `std::fs` in the child; aborting child allocator |
| R20 | Own the mount-exclusivity bookkeeping; the kernel only warns on shared upper/workdir |
| R21 | Split networking stage 3: routed NAT egress first, bridge later |
| R22 | Four-type `Accept`, branch on response `Content-Type`; the descriptor beats the HTTP header; never authenticate against a host named by image content |

---

## 19. Decisions we should deliberately postpone

**`MS_SLAVE` vs `MS_PRIVATE` on `/`** — genuinely unsettled; crun, runc and `pivot_root(2)`'s own
EXAMPLES program disagree. What is *required* is only "not `MS_SHARED`". Mis-tiered as required
in the research. Reversible because it is one flag at one call site.

**OCI hooks** — defer the hook machinery, but adopt **failure atomicity now**: leaving the
environment as if the operation were never attempted is a spec MUST, and retrofitting an unwind
path into code that never had one is expensive. Also fix the v1.3.0 `poststart` semantics before
writing any hook.

**Everything at L14–L15** — the OCI runtime CLI surface, conformance claims, and rootless. All
reversible provided the `Privilege` seam (R8) and the bundle/`Mount` boundary (R11) exist from
the start. What would make them irreversible: scattering `geteuid()` through the codebase, or
letting the puller and the runtime share in-process state.

---

## 20. Architectural risks

**The four decisions that most constrain everything else**, ranked:

1. **Sync core — the cloning process has never had a second thread.** Decides the crate list,
   the process topology, netlink access, logging and seccomp compilation. It became *more*
   load-bearing when verification showed `clone3` forfeits glibc's atfork protection.
2. **Foreground `run` first; `-d` is the same path after `setsid()`.** Decides whether half the
   supervision machinery exists at all. The research contradicted itself here at `[R] HIGH`, and
   the detached-first side rested on a selectively quoted source that recommends the opposite.
3. **The `Privilege` seam — `geteuid()` exactly once.** Every privileged call site either goes
   through it or becomes a future rootless audit.
4. **Never write state into chains you don't own.** Proven by a live leak that a daemon restart
   would not have cleaned. The failure mode is unrecoverable, not untidy.

**Experiment before deciding** — four experiments, all on the dev VM, under an hour total:

| # | Experiment | Decides |
|---|---|---|
| R25 | Does a compiled `rustix` `linux_raw` binary actually keep libc off the syscall path, and are the needed functions reachable under our feature set? | Static-musl and the seccomp substrate |
| R26 | `CLONE_INTO_CGROUP` end-to-end, the no-internal-process errnos, and the flock-on-removed-file race | Cgroup creation path and state locking |
| R27 | The `no_new_privs` breakage inventory across five image classes, including a file-capability + non-root-`USER` image | **Blocking** for the R12 default |
| R28 | `IFLA_NET_NS_FD` inside `VETH_INFO_PEER` — is the peer ever visible host-side, and what does a failure leave behind? | Whether veth creation is atomic enough to be crash-safe |

---

## 21. Open research questions

**Genuinely unsettled, and nothing in the corpus resolves them:**

1. **Does `CLONE_INTO_CGROUP` resolve before `CLONE_NEWCGROUP` captures the namespace root**
   when both are in the same `clone3`? Not addressed anywhere, and not answerable from the
   production host — `/proc/<pid>/cgroup` renders relative to the *reader's* cgroup namespace,
   so the observation tool sits inside the thing being observed. Needs the dev VM.
2. **`MS_SLAVE` vs `MS_PRIVATE` on `/`** — sources disagree; see [§19](#19-decisions-we-should-deliberately-postpone).
3. **Host- vs child-side overlay mount** — which side should perform it, given the child is the
   only one inside the mount namespace but the host has the layer metadata.
4. **The in-container reaper's shape** — the *problem* is established; the design is not.

**Unsupported pending an unreachable host.** `old-host` (`old-host.example.edu`) went to NXDOMAIN
mid-session and never came back, so 17 claims about rootless, cgroup v1, kernel 4.18 and NFS
remain documentary rather than measured — including what makes unprivileged overlay work at
4.18, the exact errno for an NFS `upperdir`, and the single-UID-map errnos (`EINVAL` vs
`EPERM`). Exact settling commands are listed in
[`PHASE4`](research/PHASE4-risks-and-reconciliation.md) §1J. **If that host returns, those are
an hour's work.**

**Needs a Rust toolchain** — two questions belong to rung L0, before any `Cargo.toml` exists.

---

## 22. Source index

**This document is a digest. The evidence lives here:**

| File | Lines | Contents |
|---|---:|---|
| [`research/README.md`](research/README.md) | 65 | Corpus map, environments, status |
| [`research/00-RESEARCH-PLAN.md`](research/00-RESEARCH-PLAN.md) | 331 | The five-domain decomposition and method |
| [`research/D1-kernel-isolation.md`](research/D1-kernel-isolation.md) | 1,960 | Namespaces, process creation, mounts, `pivot_root`, PID 1 |
| [`research/D2-runtime-architecture.md`](research/D2-runtime-architecture.md) | 1,933 | OCI runtime spec, the Docker chain, state, I/O, Rust substrate |
| [`research/D3-images-storage.md`](research/D3-images-storage.md) | 2,700 | OCI image/distribution specs, pull, extraction, OverlayFS |
| [`research/D4-networking.md`](research/D4-networking.md) | 2,133 | netns, veth, bridge, NAT, DNS, host-state leaks, IPAM |
| [`research/D5-resources-security.md`](research/D5-resources-security.md) | 3,544 | cgroup v2, capabilities, seccomp, masking, escapes, rootless |
| [`research/V1`](research/V1-kernel-isolation-verification.md)–[`V7`](research/V7-rust-substrate-verification.md) | 7,058 | Adversarial verification: 419 confirmed, 111 corrected, 74 refuted, 49 misclassified, 25 outdated, 52 unsupported |
| [`research/PHASE4-risks-and-reconciliation.md`](research/PHASE4-risks-and-reconciliation.md) | 1,747 | The reconciliation ledger (196 entries) and the 28-item risk register |
| [`research/TRACE-container-creation.md`](research/TRACE-container-creation.md) | 1,372 | The 42-step annotated creation trace |
| [`research/LADDER-implementation-stages.md`](research/LADDER-implementation-stages.md) | 1,657 | The 16-rung implementation ladder |

Per-file `## Sources` sections list every primary source used — kernel documentation and man
pages (read on kernel 6.11 with man-pages 6.8-2 and cross-checked against man7.org), OCI
specifications, and the source trees of runc, crun, youki, containerd, moby, netavark, podman
and Apptainer.

**Empirical basis.** 390 observations were taken on live hosts, marked `[O]` with the host,
kernel version and exact command. **Host names are anonymised**: `host-a` is a modern
x86_64 Linux observation host, `old-host` an unprivileged Rocky 8.8 / kernel 4.18 HPC login
node; addresses are in RFC 5737 documentation ranges and container/bridge/veth/MAC
identifiers are pseudonymised. Measurements, kernel versions and commands are unaltered. The primary observation host was Ubuntu 24.10 / kernel
6.11.0-29 with cgroup v2, native overlay, `iptables-nft` and AppArmor, running production
containers — inspected **read-only throughout**. Observations are scoped to that host and
kernel; where the corpus generalised one into a fact about Linux, verification flagged it.

**Method note.** Three documentation defects were confirmed against kernel source during
verification: `ip-sysctl.rst` on `ip_forward`'s side effects, `unshare(2)` on
`CLONE_NEWPID`/`CLONE_THREAD`, and the overlayfs docs on when `lowerdir+` landed. One fabricated
citation was caught in the research (a quote attributed to `descriptor.md` that appears nowhere
in the spec — the substance was right, the citation invented). Treat unverified citations in the
`D*` files accordingly; the `V*` files record which were checked.
