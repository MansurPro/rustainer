# PHASE 4 — Reconciliation ledger and architecture risk register

Scope: an R&D-only synthesis of Phase 2 research (`D1`–`D5`, 12,695 lines, 1,828 tagged claims) and
Phase 3 adversarial verification (`V1`–`V7`, 7,058 lines). **No code was written.**

**Precedence rule, applied throughout:** where a `V` file refutes or corrects a `D` claim, the `V`
verdict wins. The `D` files were never edited after verification, so every `D` citation below is
either (a) explicitly CONFIRMED by its `V` file, or (b) carried with the `V` correction attached.
Reading a `D` file alone will hand you claims now known to be wrong.

Tagging matches the corpus: `[F]` fact / `[C]` convention / `[R]` recommendation / `[O]` observation,
with `HIGH` / `MED` / `LOW` confidence. Where I am making a judgement rather than reporting one, the
sentence says so explicitly.

---

# PART 1 — RECONCILIATION LEDGER

Every claim whose status **changed** during verification. Grouped by domain. One line each.
"Consequence" = what would have gone wrong had we built on the original.

## 1A. Kernel isolation — namespaces, `pivot_root`, PID 1 (V1 vs D1, D2 §4)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| `setns(2)` needs a fork for **TIME** as well as PID | D1 §3.1 | `[F]` HIGH | **REFUTED** — `setns` *does* move the caller into a time ns; `timens_install()` sets both `time_ns` and `time_ns_for_children` (V1 §C1, measured) | We'd have built a pointless fork into `exec` for time namespaces, and mis-modelled the one flag (PID) that genuinely is children-only |
| "All **five** preconditions, measured … every documented precondition reproduces exactly" | D1 §5.2 | `[O]` HIGH | **REFUTED** — six restrictions; four tested; three of the four confounded by a shared `/` (V1 §B2) | A `pivot_root` error handler built to this table mis-reports two of the real errnos; we'd have believed we had measured coverage we never had |
| `pivot_root` C5 `new_root == "/"` → `EINVAL` | D1 §5.2 | `[O]` HIGH | **REFUTED** — it is **`EBUSY`** under a private `/`; D1's `EINVAL` was the shared-root check firing first (V1 §B2, T2) | Error-mapping code would never match the real errno; the "pathological `/`" case would fall through to a generic failure |
| §2 version table sourced to "`namespaces(7)` version column" | D1 §2 | `[F]` HIGH | **REFUTED** — no version column exists in `namespaces(7)` (man-pages 6.8-2). Numbers themselves CONFIRMED against `clone(2)` (V1 §D1a) | Sourcing rot: the next person "verifies" against a page that cannot confirm it, and trusts a table nobody can re-derive |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| `unshare(CLONE_NEWPID)`: the caller's `pid_for_children` **changes** | D1 §1.2 | `[O]` | **CORRECTED** — it goes **empty**; `readlink` fails `ENOENT` until the first child (V1 §C4) | A health probe reading `pid_for_children` to confirm `unshare` succeeded would `ENOENT` on the *healthy* path and abort every start |
| `pivot_root` §5.2 heading "five preconditions" | D1 §5.2 | `[O]` | **CORRECTED** — six (V1 §B2a) | Two untested preconditions silently assumed |
| §10.1 step 17 "5 documented `EINVAL` preconditions" | D1 §10.1 | `[F]` | **CORRECTED** — six `EINVAL` **plus** `EBUSY`, `ENOTDIR`, `EPERM` (V1 §B4) | `ENOTDIR` hit on V1's first try; an exhaustive-looking match arm that isn't |
| Forced-signal mechanism = `sig_task_ignored()`'s `force` logic | D1 §7.1 | `[F]` MED | **CORRECTED** — it is `force_sig_info_to_task()` clearing `SIGNAL_UNKILLABLE` outright (V1 §E2). D1 Open Q4 **settled** | Two consequences missed: PID-1 unkillability is **not durable** (one `SIG_DFL` fault retracts it permanently), and only *forced* signals bypass |
| "write ~60 lines of `capget`/`capset`/`PR_CAPBSET_DROP`/`PR_CAP_AMBIENT` ourselves" | D1 §11.1 | `[R]` MED | **CORRECTED — obsolete.** rustix 1.1.4 `rustix::thread` ships the entire surface, alloc-free (V1 §H3) | ~60 lines of hand-rolled `syscall(2)` capability code, unsafe, untested, for no reason. D1 assessed only the `caps` crate |
| The post-clone no-allocation rule set is **sufficient** | D1 §11.2 | `[R]` HIGH | **CORRECTED** — insufficient; both `nix` 0.31.3 and `rustix` 1.1.4 re-allocate for over-length paths inside their own `&str`/`&Path` conversion (V1 §H4) | Length-dependent: passes every short-path test, allocates post-clone on real `/var/lib/mydocker/overlay2/<64-hex>/merged/...` paths |
| "`CVE-2019-14271`-style symlink-TOCTOU" on mount targets | D1 §4.5 | `[F]` | **CORRECTED** — wrong CVE. That one is `docker cp` `libnss_*.so` injection. The family is **CVE-2021-30465** (canonical) and **CVE-2019-16884** (V1 §G2) | Wrong CVE copied into design doc → commit message → security posture claim |
| The `chroot` escape "requires `CAP_SYS_CHROOT`, which container root has" | D1 §5.5 | `[F]` | **CORRECTED** — the load-bearing half (hold `O_DIRECTORY` fd across `chroot`, `fchdir`, read by relative path) needs **no capability at all** (V1 §B6) | We'd have believed dropping `CAP_SYS_CHROOT` from the bounding set mitigates it. It does not |
| §2's `ENOSPC`/`EUSERS` note applies to all eight per-type limits | D1 §2 | `[F]` | **CORRECTED (scope)** — the `EUSERS` history is specific to the **nested user-namespace** limit (3.11–4.8); per-type counts are a separate `ENOSPC` entry since 4.9 (V1 §D1b) | Errno handling written for the wrong limit class |
| "`CAP_SYS_TIME` … useless outside the initial user namespace" | D1 §1.6 | `[F]` | **CORRECTED** — since Linux 5.6, `CAP_SYS_TIME` in the userns owning the *time* ns can write `timens_offsets` (V1 §D2b). `CAP_SYS_MODULE`/`CAP_MKNOD` unaffected | Quoted into the D5 rootless handoff as a flat fact; would mis-scope a future rootless time-ns feature |
| §3.5 rule 8: netns plumbing must precede the child's mounts, sourced to runc | D1 §3.5 | `[C]` | **CORRECTED (citation)** — in runc v1.5.1 `createNetworkInterfaces()` (parent) and `prepareRootfs()` (child) run **concurrently**; the kernel-imposed constraint is only "before caps drop / seccomp" (V1 §C6 r8, §I3) | We'd have treated a self-imposed serialization as a kernel law and never been able to overlap it for start latency |
| D2 §4's 14-step runc parent-side listing | D2 §4 | `[F]` | **CORRECTED** — steps 8/9 transposed (real order `getChildPid` → `getPipeFds` → `waitForChildExit`); two steps missing (`setupNetworkDevices`, config `WriteJSON`) (V1 §I1) | D2's own commentary argues for the *real* order, so the argument survives the listing error — but a literal implementation records fds too late |
| §9.3 row "non-recursive `MS_PRIVATE` on the rootfs mount" | D1 §9.3 | `[F]` required | **CORRECTED (wording)** — the kernel precondition is on the **parent mount of `new_root`**, which is what runc's `rootfsParentMountPrivate()` loop achieves (V1 §F1, §D5) | The row names the wrong mount; an implementation following it literally makes the wrong thing private and still fails `pivot_root` |

### OUTDATED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "each call to `setns()` changes just one of the caller's namespace memberships" | D1 §3.1 | `[F]` HIGH | **OUTDATED (Linux 5.8)** — the **pidfd form** atomically moves the caller into *many*: `setns(fd, CLONE_NEWUSER\|CLONE_NEWNET\|CLONE_NEWUTS)` (V1 §C2) | `mydocker exec` written as six `open`+`setns` pairs, with a partially-joined failure state that has no recovery — instead of **one atomic call** on the pidfd D1 already recommends keeping |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "no allocation post-clone" presented as a derived `[F]` | D1 §11.2 | `[F]` | **MISCLASSIFIED** — POSIX scopes async-signal-safety to **multithreaded** forks. The `Threads == 1` assert and the no-alloc rule are **alternatives, not a stack** (V1 §H4) | A reader who cannot reach `Threads == 1` (async runtime in-process) believes the no-alloc rule saves them. It does not — the whole async-signal-safety surface applies |
| `MS_REC\|MS_SLAVE` on `/` = **required** | D1 §9.3 | required | **MISCLASSIFIED** — the requirement is "**not `MS_SHARED`**"; slave vs private is a choice. crun defaults private; `pivot_root(2)`'s own EXAMPLES program uses `MS_REC\|MS_PRIVATE` (V1 §F2) | The table promotes to "required" a choice D1's own Open Q8 calls unsettled — foreclosing a real tradeoff (slave keeps host→container propagation, which volume lifecycle wants) |
| `/proc`, `CLONE_NEWPID`, `CLONE_NEWNET` = **required** | D1 §9.1/§9.4 | required | **MISCLASSIFIED** against D1's own tier definition ("no container without it"). `--pid=host` and `--network=host` ship; D1 §6.1 shows `id`/`python3`/`openssl`/`df` all work with **no** `/proc` (V1 §F1) | Tier inflation hides which rows are genuinely irreducible — exactly the information the MVP scope decision needs |
| `[C]` HIGH crun `OPEN_TREE_NAMESPACE` path paired with `[F]` LOW on whether the flag exists | D1 §4.5 | mixed | **MISCLASSIFIED confidence** — incoherent pairing; both halves now settled (V1 §G1) | Cannot be HIGH-confident a runtime ships a path while unable to establish the flag it needs exists |
| §9.2 row "`CLONE_PARENT` \| strongly recommended (with 3 stages)" | D1 §9.2 | `[R]` | **MISCLASSIFIED / incomplete** — `clone(2)`: `CLONE_NEWUSER` **cannot** be combined with `CLONE_PARENT`. The userns must come from a separate `unshare` (V1 §F4, §C8) | A guaranteed `EINVAL` the first time the three-stage userns topology is built from clone flags |
| `CLONE_INTO_CGROUP` = "strongly recommended" | D1 §9.x | strongly recommended | **MISCLASSIFIED (under-tiered)** — between `clone` and the `cgroup.procs` write the child sits in the **parent's** cgroup and can fork/spike/allocate past the limit. Tier: **required for correct resource limits** (V1 §F3) | A memory limit that does not hold during the start window. This is a correctness bug, not a hardening nicety |
| `close_range` sweep before `execve` — present only in §11.2 as `[R]`, absent from §9 entirely | D1 §9 | absent | **MISCLASSIFIED (omitted)** — leaked host fd into a container is a documented escape class (CVE-2024-21626, cited correctly). Belongs in §9.5 at hardening tier, arguably required — it is one syscall (V1 §F3) | The cheapest single mitigation in the whole design is not in the table that drives implementation order |
| `CLONE_NEWCGROUP` = "hardening" | D1 §9.x | hardening | **MISCLASSIFIED (over-tiered)** — it hides a path also derivable from `/proc/self/mountinfo`; closer to **Docker convenience**. Its stated justification is work the read-only cgroupfs mount already does (V1 §F3) | Minor. Effort spent on a row whose benefit is already bought elsewhere |
| D2 §4 rule 2(a) fallback "child writes its own pid to `cgroup.procs` as its first act" | D2 §4 | `[R]` | **MISCLASSIFIED — unsourced invention.** No runtime does this; runc has the **parent** write (`manager.Apply(p.pid())`). Two unstated preconditions: cgroupfs reachable by path (only pre-`pivot_root`, or via a held dirfd) (V1 §I2) | A fallback that works only in a window D1 §5.4 closes — and that no upstream has ever tested |

### Also settled by V1 (new findings, no prior status)

- `[F]` HIGH — **`unshare(2)`'s own `CLONE_NEWPID` paragraph is wrong** ("automatically implies `CLONE_THREAD`"), and D1's §3.4 measurement proves it. `ksys_unshare()` adds `CLONE_THREAD` for `CLONE_NEWUSER` only (V1 §C5). D1 collected the disproof and did not notice it.
- `[F]` HIGH — `setns(CLONE_NEWTIME)` from a multithreaded process returns **`EUSERS`**, not `EINVAL`; second namespace op with a single-threaded requirement, missing from D1 §3.6 (V1 §C1).
- `[F]` HIGH — `setns` into a **mount** ns requires `CAP_SYS_CHROOT` **and** `CAP_SYS_ADMIN`, and refuses a caller sharing `CLONE_FS` state (V1 §C2). Directly constrains `mydocker exec`.
- `[F]` HIGH — `unshare(CLONE_NEWUSER)` from inside a `chroot` → `EPERM` since Linux 3.9; `CLONE_NEWNS`+`CLONE_FS` in one `clone()` not permitted; `CLONE_PARENT` forbidden to any namespace init (V1 §C8).
- `[F]` MED — `clone3(CLONE_NEWTIME | CLONE_VM)` leaves the child in the **parent's** time ns (`timens_on_fork` is gated on `CLONE_VM` being clear) (V1 §D2a).
- `[O]` — a forwarding init cannot identify the sender: `si_pid` is **zero** in the PID-1 handler (V1 §E1). Bears directly on the `--init` design (risk R13).
- `[F]` MED — orphans can reparent **outside** the container under the `unshare` topology (`pid_namespaces(7)`'s elided qualifier), though not under the `clone3` MVP topology (V1 §E3).

## 1B. Runtime architecture — OCI spec, supervision, state, I/O (V2 vs D2 §1–§11)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "The supervisor must hold **both** ends of the container's stdout/stderr pipes — the single most-copied trick in the fd table" | D2 §5.3, §5.6#4, §2.2 | `[R]` HIGH | **REFUTED** — `/proc/<shim>/fd` modes show the shim holds only the **read** ends (11, 13 = `lr-x`) of the *container* pipes. Both-ends applies to the docker-side **FIFO** (14+16, 19+21) (V2 §2.2) | Direct deadlock: a supervisor holding a write-end copy **never sees EOF**, so §6.5 step 3 ("drain the stdio pipes to EOF") hangs forever on every container exit. The trick belongs to the FIFO hop D2 itself says to delete |
| Console-socket sequence: runc `openpty()`s on the host, dups the slave onto init, sends the master | D2 §5.2 | `[C]` HIGH | **REFUTED** — `runc init` allocates the pty **inside the container, after `pivot_root`**, from the container's own `/dev/ptmx`, and sends the master *up* (`init_linux.go:370–416`; comment cites runc#814) (V2 §5.2) | §7.2's diagram puts `openpty` in the supervisor on the host — **the exact arrangement runc abandoned**. A host-devpts pty inside a container leaks a host resource across the isolation boundary and gets the ownership/permission model wrong |
| Subreaper benefit: "`exec`'d processes whose intermediate parent died still get reaped instead of accumulating zombies **in the container**" | D2 §5.4 | `[R]` HIGH | **REFUTED** — measured on `host-a` 6.11.0-29: orphans inside the pidns (including from a `setns`ed process) reparent to the **container's PID 1**, never to the host subreaper. The kernel forbids cross-namespace reparenting (V2 §5.4) | We'd have shipped believing `--init` was unnecessary. A container whose PID 1 is `nginx`/`bash` accumulates zombies from every `exec` orphan and **no supervisor-side flag can fix it** |
| "Design so we never re-exec into the container at all … eliminates the entire CVE class rather than mitigating it" | D2 §7.3 | `[R]` HIGH ("second-biggest Rust win") | **REFUTED** — `execve` of a user path resolving to `/proc/self/exe` reproduces CVE-2019-5736 with **no runtime re-exec involved**. crun has D2's exact architecture and still ships the memfd constructor; youki calls `pentacle::ensure_sealed()` first thing in `main()` (V2 §7.3) | A false security guarantee presented as a design principle. The memfd self-seal would have been skipped as "already eliminated" — leaving the base exposure fully open |
| "crun … no self re-exec, no `/proc/self/exe`, no memfd-clone, no C constructor trick" → "100% Go artifacts" | D2 §10.1 | `[C]` HIGH → `[R]` HIGH | **REFUTED** — crun `linux.c:7001–7011` has `__attribute__((constructor)) static void libcrun_rexec(void)`; `cloned_binary.c` is 13,929 bytes of C. D2's grep was scoped to `container.c` (V2 §10.1) | Same as above — reinforces the false "Go artifact" story. D2 §11.8 cites `libcrun_rexec` **correctly**; the two sections contradict each other and §10.1 is the wrong one |
| "`sun_path` … `/var/lib/mydocker/containers/<64-hex>/attach.sock` is 105 bytes **before the prefix**" — the deciding argument for FIFO over `notify.sock` | D2 §10.3, §11.1#9 | `[R]` HIGH | **REFUTED (arithmetic)** — 105 **is** the whole path; +NUL = 106 ≤ 108. It fits. youki's `chdir(parent)` → `bind("notify.sock")` → `chdir(back)` removes path length as a factor at any depth (V2 §10.3). Settles D2 §14 Q7 **against** §10.3 | A decision made on a false constraint. The FIFO may still win on its *other* grounds, but the verdict must be re-derived, not re-asserted. (2 bytes of headroom is thin — "keep the socket path short" survives as a caution) |
| `external_descriptors` "is how `runc exec` knows what to wire a new process's stdio to" | D2 §4 step 9, §6.1 | `[C]` HIGH | **REFUTED** — the only consumers in runc `main` are the state serializer and `criu_linux.go:489`. runc's own comment says *"at checkpoint time"*. `exec` wires from the **caller's** fds via `setupIO` (V2 §4/§6.1) | `external_fds [string;3]` was carried into §6.4's state model and §7.5's exec design **with no consumer at all** — D2 defers CRIU. Free deletion the corpus argued itself into carrying |
| "youki appears not to have this [CVE-2019-5736 mitigation]" | D2 §11.8 | `[C]` LOW | **REFUTED** — `crates/youki/src/main.rs:65–76`, first statement of `main()` (V2 §11.8) | The `[R]` drawn from it ("we cannot assume 'youki does it' for security-critical mitigations") is built on a false premise — though it is decent advice anyway |

### OUTDATED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "`poststart` hooks — fail → log a warning, the container keeps running" | D2 §1.1, §1.4 | `[F]` HIGH | **OUTDATED** — spec **v1.3.0** (released 2025-11-04): "MUST generate an error, **stop the container**, and continue the lifecycle at step 12". True only for v1.0.2/v1.1.0/v1.2.0, verified at each tag (V2 §1.1) | Non-conformant **in the unsafe direction**: we'd leave a container running that the spec requires be stopped. D2's whole hook-deferral rationale rests on this table, and D2 §15 already records v1.3.0 as latest — the body and the source list disagree |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "The spec defines **exactly four** status values" | D2 §1.1 | `[F]` HIGH | **CORRECTED** — the spec names four the value MAY be, then explicitly permits runtime-defined additions representing *new* states. runc's `paused` is conformant (V2 §1.1) | Inverted framing. §6.4's `unknown` is **not** spec-legal as a reported status ("we lost the exit code" is not a new runtime state) — report `stopped` with a null exit code and keep `unknown` as a separate field |
| `createContainer` and `startContainer` both "run a host binary inside a partially-constructed container namespace" | D2 §1.4 | `[C]` | **CORRECTED (one row)** — `startContainer`'s *path* MUST resolve in the **container** namespace: it is a container binary (D2's own `ldconfig` example proves it). Only `createContainer` is the hard case (V2 §1.4) | D2's stated reason for deferring both is half true; `startContainer` is the *easier* of the pair, not an equal |
| Shim fd table: fds 12/14/15/16 are "write + read ends of the docker-side stdout FIFO" | D2 §2.2 | `[O]` HIGH | **CORRECTED** — 12/15/17/20 are `O_PATH` (`l---------`), neither readable nor writable (V2 §2.2) | The mode column is the whole point; misreading it produced the refuted "both ends" rule above |
| containerd issues #13849 / #10727 / #3334 cited as current failure modes | D2 §2.3 | `[C]` MED | **CORRECTED (status)** — #10727 **closed** (2024-09-26), #3334 **closed** (2019-06-11, pre-shim-v2-maturity); only #13849 is open (V2 §2.3) | The `[R]` lesson drawn ("the exit status exists in exactly one place — the reaping parent's memory") is sound independently, but two of three citations do not evidence present-day behaviour |
| runc parent-side steps 8/9: `waitForChildExit` then `getPipeFds` | D2 §4 | `[F]` | **CORRECTED** — runc does `getPipeFds` **first**, then `waitForChildExit` (`process_linux.go:850–866`) (V2 §4a) | D2's own rationale ("record them BEFORE the container can move them") argues for runc's order. A literal implementation records the fds too late to be trustworthy |
| Failure path: `syncSockParent.Shutdown()` → `p.wait()` → `manager.Destroy()` → `intelRdt.Destroy()` → `p.terminate()` | D2 §4 | `[F]` | **CORRECTED (inverted)** — actual deferred order is `OOMKillCount()` → **`p.terminate()`** → `manager.Destroy()` → `intelRdt.Destroy()`, with runc's comment *"Terminate the process to ensure we can remove cgroups"* (V2 §4b) | **Produces a bug if copied.** A cgroup dir with a live process in it cannot be `rmdir`ed → guaranteed cgroup leak on every failed create. D2 §9.1 independently states this rule; §4 contradicts §9.1 |
| "runc sets `PR_SET_CHILD_SUBREAPER` in its foreground path" | D2 §5.4 | `[C]` | **CORRECTED (minor)** — set on **both** paths (`utils_linux.go:265`, before the `if !detach` guard); uselessly on the detached one since runc then exits (V2 §5.4) | Cosmetic |
| "`clone(2)` with namespace flags is subject to **exactly the same constraint** as `fork` — same kernel path" | D2 §8.1 | `[F]` HIGH | **CORRECTED (mechanism); conclusion gets STRONGER** — the *kernel* path is the same, the *libc* path is not. glibc `posix/fork.c` locks NSS + libio + **malloc arenas** before cloning and resets them in the child; a raw `clone3` (which D2 specifies) runs **none** of it (V2 §8.1) | A reader who knows glibc's atfork behaviour reads D2's sentence, concludes the hazard is overstated, and reaches for a thread pool. In fact choosing `clone3` for `CLONE_INTO_CGROUP` **forfeits** glibc's protection: "never more than one thread at clone time" goes from belt-and-braces to load-bearing |
| Rationale for single-threadedness: "a thread that exited can have left a lock held at the moment it was cancelled" | D2 §8.1 | `[F]` | **CORRECTED** — Rust has no thread cancellation, and a `Mutex` released by a panicking thread is unlocked-and-poisoned, not left locked. The real reason is D2's *second* one: **you cannot audit every transitive dependency for thread spawning** (C libs like `libseccomp` give no guarantees) (V2 §8.1) | Right conclusion, wrong lead argument — and the wrong argument is falsifiable, which invites dismissing the rule |
| "A leaked IP from the pool is a slow exhaustion" (severity table) | D2 §9.1 | `[C]` | **CORRECTED** — true, but records the wrong risk. The dangerous mode is **premature release**: an address returned to the pool while a DNAT rule or conntrack entry still points at it hands the next container the previous one's traffic (V2 §9.1 Adjudication 1) | A severity table indexed by "which resource leaked" **cannot express a constraint about relative ordering between two resources**. Belongs in §9.3 as an ordering invariant on `Cleanup::ReleaseIp`, plus a conntrack flush |
| "runc and crun ship the same file (`cloned_binary.c`)" | D2 §11.8 | `[C]` | **CORRECTED (half stale)** — runc's is **404** on `main`; the mitigation is now Go at `libcontainer/exeseal/cloned_binary_linux.go`. crun's is still C. `linux.c:6986` has drifted to 7003 (V2 §11.8) | Substantive point (memfd self-sealing convergence) survives and is now **four-for-four** with youki added |
| D2 §15's unresolved-citation note #2 (runc `main_linux.go` is a dead reference) | D2 §15 | note | **CORRECTED** — the only `main_linux.go` cited in the §1–§11 body is **Apptainer's** (§11.7 line 1275) and it resolves 200. Nothing in §1–§11 rests on a dead reference (V2 §11.7) | A self-flagged defect that was not one |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "What the spec explicitly does NOT mandate — read this list twice": error reporting filed under **latitude** | D2 §1.5 | latitude | **MISCLASSIFIED** — the very next sentence in the same spec paragraph is a MUST: *"generating an error MUST leave the state of the environment as if the operation were never attempted."* That is **mandated failure atomicity** (V2 §1.5) | We'd have underweighted §9 by exactly this much. A `create` that fails leaving an overlay mount, a cgroup, or a live init behind is **non-conformant**, not merely untidy. Also dropped: "The entity using a runtime … MUST be able to use the operations against that same container" |
| "Should the CLI ever be in the container's process ancestry? **No. Never, in any mode, including foreground.**" | D2 §7.4 vs §11.7 item 1 | `[R]` HIGH both | **MISCLASSIFIED / self-contradictory** — §11.7 `[R]` HIGH says *"the container is a process, not a service … foreground execution with the CLI as supervisor"*, adopt-worthy. Both cannot hold. §7.4's citation quotes runc's foreground *drawback* and omits the same file's recommendation two paragraphs earlier: *"In general we recommend using foreground … Detached mode is difficult to get right"* (V2 §7.4) | Selective citation in support of a conclusion already reached. And `-d` is treated as a requirement throughout §7 when **no stated scope item requires it** — §7.6 then phases the harder thing (detach + supervisor + handshake + durable exit status + log rotation) *before* the simpler one |
| "A startup/reconciliation path" listed as a **cost of having a daemon** | D2 §7.1 | cost-of-daemon | **MISCLASSIFIED** — §6.4's cases C/D/E **are** that reconciliation, paid once per CLI invocation instead of once per daemon start. §11.1 says exactly this of Podman (V2 §7.1) | Overstates the daemonless win. The work is **relocated, not avoided** |
| §2.3 Boundary 4 rates the shim→runc collapse `[R]` MED and says "**§7.2 resolves this** with a three-process shape that keeps the collapse but not the coupling" | D2 §2.3 → §7.2 | `[R]` MED, resolved | **MISCLASSIFIED — unfulfilled forward reference.** §7.2's supervisor still calls `clone()`; the coupling is restated, not removed. §7.2's `[R]` MED answers a *thread-safety* question, not the **lifetime** question §2.3 raised (V2 §7.2) | The MED rating stands **unearned rather than resolved**. A panic anywhere in the ~15-step create sequence takes down the process that must hold stdio and exit status for the container's whole life |

### UNSUPPORTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "A daemon, later — because ***nothing*** in the design depends on there being no daemon" | D2 §7.5 | `[R]` | **UNSUPPORTED as stated** — there is no push channel. §6.6 concedes the `docker events` gap two sections earlier. A later daemon must poll N state files or add a socket to **every supervisor** (V2 §7.5) | The supportable claim is only that nothing *forbids* a daemon, and that the state dir + per-container `flock` is a reasonable substrate for one |
| Every `[O]` sourced from `old-host` in §11.7/§11.8 (Apptainer rootless stack) | D2 §11.7, §11.8 | `[O]` | **UNSUPPORTED — pending host** (`old-host.example.edu` → NXDOMAIN). Apptainer *source* claims in the same sections all CONFIRMED (V2 §11.7) | See §1J below for the full list and exact commands |

### Design gaps found by V2 (not claim verdicts, but they bind Part 2)

1. **`flock` on a removable file** (D2 §6.6 / §9.4). Lock lives at `<dir>/lock`; `delete --force` removes the dir. A holds `LOCK_EX`; B unlinks; C re-creates and locks a *new inode* → **two holders, no contention**, because `flock` locks the inode via the open file description, not the path. Fix: compare `st_dev`/`st_ino` of the held fd against the path after acquiring, or lock under a never-removed dir (`/run/mydocker/locks/<id>`). D2 already recommends this exact idiom for `pivot_root` in §11.7 — it just is not applied here.
2. **`flock` is per open-file-description and survives `fork`** (D2 §7.2). If the lock fd reaches the supervisor, the CLI's exit does **not** release it and every later `start`/`kill`/`rm` blocks for the container's lifetime. Rust's `File` sets `FD_CLOEXEC` by default so an `exec`ed supervisor is fine — but "auto-release on death is the whole reason to use `flock`" needs the companion note "and that requires exactly one holder".
3. **§7.2's supervisor never takes the lock in the diagram**, while `start` does a locked read-modify-write of the same file. A `start` racing the supervisor's `exited` write **resurrects a dead container as `running`**. One line fixes it, and the diagram is what gets implemented.
4. **No `--init` / in-container reaper anywhere in D2 §1–§11.** Given the refuted subreaper benefit, this is now a functional gap, not a nicety. `docker-init 0.19.0` is installed on `host-a` and appears in D2's own version block. ~30 lines.
5. **No conntrack flush** in the §9.3 unwind. Deleting an nftables rule does not tear down flows already in conntrack.
6. **`root.path` may be bundle-relative** and "a directory MUST exist at the path declared" — a spec MUST that §1.3's minimal-config `[R]` treats as a bare string.
7. **`mounts` MUST be applied in listed order** — an ordering MUST absent from §4, whose entire subject is ordering.

### Adjudications V2 performed (recorded, since they settle cross-document disputes)

- **Adjudication 1 — release order.** D2 §9.1's "leaked IP = slow exhaustion" vs D4/V5's "release order is a security property". **Both right about different failure modes; D2's table hides the dangerous one.** The hazard is the **inversion**, not the leak. D2 §9.3's strict reverse unwind already produces the right order *for free* — what is missing is the *statement* that the adjacency is load-bearing, so nobody later "optimises" the release to happen early (a very natural thing to do, since releasing the IP first shortens pool pressure). Amendment: annotate `Cleanup::ReleaseIp` as order-critical, and flush conntrack for the address **between** rule removal and release.
- **Adjudication 2 — netavark's `firewall/` directory as precedent for a persisted rollback list.** The precedent is real and shipping in Rust (`src/firewall/state.rs`, 354 lines, `FIREWALL_LOCK_FILE`, `fs2::FileExt`) — **but it is a *replay* list, not a *rollback* list**, and that is exactly where the risk is. Replay is naturally idempotent (re-adding an existing nftables rule is harmless). Rollback is not: `ReleaseIp(addr)` executed twice hands one address to two containers; `Umount(path)` twice can unmount someone else's mount. Verdict: netavark establishes the *mechanism* at HIGH confidence and establishes **nothing** about the rollback direction. D2's `[R]` MED stays MED **with the reason replaced** — not "is it worth phase-1 complexity" but "a persisted undo log must be idempotent or crash-marked, and D2 specified neither". D2's own cheap alternative ("on `rm`, best-effort try every cleanup and report what failed") is the idempotent-by-construction version, and is right *for that reason*.

## 1C. OCI specs, registry and pull (V3 vs D3 §0–§5, §7, §12)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "The registry stores and serves bytes; conversion (if any) happened at **push** time" | D3 §2.5 | `[C]` HIGH | **REFUTED** — Quay **synthesises Schema 1 at serve time**: three different digests for one tag depending on `Accept` (`57ba692b…` list, `d86ce8f3…` manifest, `49b68533…` schema 1) (V3 §G.1) | Two clients with different `Accept` headers **disagree about what a tag is**. Any cache keyed on `(repo, tag)` is wrong unless it also keys on the `Accept` set or pins the resolved digest. Nothing in D3 §2 or §7 says this |
| "As root the `S_ISUID`/`S_ISGID` bits are **not** cleared by `chown`" | D3 §6.6 | `[F]` HIGH | **REFUTED** — cleared as root too, **since Linux 2.2.13**; `chown(2)`'s own next lines say so. Measured on 6.11: `chmod 4755` → `chown 1000:1000` → mode `755` (V3 §J) | A reader concludes "we run the extractor as real root, so the ordering is a nicety". **Silent failure**: setuid vanishes from every image. The §6.14 checklist order (chown→xattr→chmod) is right; only the *explanation* was wrong |
| "`HEAD` requests count against the rate limit on Hub the same as `GET`" | D3 §2.4, §15.3 | `[C]` MED | **REFUTED** — 11 consecutive HEADs produced **zero** decrements; GETs decrement monotonically (V3 §C.5) | We'd have avoided the cheapest freshness check we have. HEAD-the-tag costs **zero** rate-limit budget, not one unit. D3 §15's open question is closed. (Caveat: Hub-specific, no spec backing, measured once — prefer HEAD, never *depend* on it being free) |
| Distribution Spec: a registry that cannot satisfy `Accept` "SHOULD return `406 Not Acceptable`" | D3 §2.5 | `[F]` MED | **REFUTED** — the string `406` appears **exactly once** in the whole Distribution Spec, inside a git SHA in an appendix URL. No such requirement exists (V3 §E.3) | D3 frames Hub as *deviating* from a spec requirement that does not exist. Hub is not deviating; the spec is simply silent |
| Descriptor digest is "a cryptographic checksum of the content **as it is transported**" (in quotation marks, attributed to `descriptor.md`) | D3 §1.2 | `[F]` | **REFUTED as a quote** — the phrase appears nowhere in `descriptor.md` or the image spec. The *substance* is true but is established by `layer.md`'s `+gzip` media types plus `config.rootfs.diff_ids` (V3 §E.2) | Citation integrity: "a verifier who checks one quote and finds it invented will distrust the other forty, all of which are fine" |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| CVE-2020-15157 is a cross-host **redirect** credential leak | D3 §2.8 | `[F]` | **CORRECTED** — it is a **`urls`/foreign-layer** bug: containerd directly fetches the attacker's URL, the attacker replies `401` with Docker-style `WWW-Authenticate`, containerd walks the token flow. Leaks **registry username/password or cloud instance credentials**, not the 5-min scoped token. containerd < 1.2.14; 1.3.0+ never affected (V3 §F.3) | **We'd have implemented one rule and needed two.** (a) strip `Authorization` cross-host — real, and a spec MUST NOT; (b) **never run an auth handshake against a host learned from image content**. D3 cites the CVE as evidence for (a); the CVE *is* (b), and (b) is stated nowhere in §2.8 |
| Spec "allows **307** on blob GET and the client **MUST** follow it" | D3 §2.8 | `[F]` HIGH | **CORRECTED (twice)** — the spec says any RFC 9110 §15.4 redirect on *any* request, and clients **SHOULD** follow. 307 is Hub's choice (V3 §E.5) | Over-tight branch on a specific status code; over-strong obligation |
| "For **every** real platform manifest there is a paired attestation manifest" | D3 §2.6 | `[F]` (universal) | **CORRECTED** — false for `busybox` (`linux/arm/v6`, no partner) and `hello-world` (2 `windows/amd64`, no partner) (V3 §B.2) | A "filter out `unknown/unknown`, take what's left" matcher hands a **`windows/amd64` manifest to a Linux extractor** on `hello-world`. D3's actual rule matches `os == "linux"` positively, so the rule survives — only the premise was over-tight |
| `Docker-Content-Digest` "SHOULD" be included in the response | D3 §2.4 | `[F]` | **CORRECTED / OUTDATED** — current spec says **MUST**, in three separate places (manifest GET, blob GET, HEAD) (V3 §E.6) | D3's defensive posture (hash the body yourself) is right anyway — and is itself spec-backed: "if it is used, the client MUST verify the value matches" |
| `image-layout` is "**explicitly** not designed as a live engine's store" | D3 §1.3 | `[F]` HIGH "by omission" | **CORRECTED** — the spec is **silent**; it neither endorses nor forbids engine use. §7.1's own three reasons are the honest argument (V3 §E.8) | Attributing an inference to a spec. The conclusion is right; the sourcing is not |
| "**MUST** error if the digest does not match or you have no expected digest" (media-type conflict rule) | D3 §3.6 | `[F]` | **CORRECTED** — the spec puts **all three clauses** under one **SHOULD** (V3 §E.7) | Keep the behaviour, fix the tag |
| ChainID recursive case written as `SHA256(…)` | D3 §4.5 | `[F]` HIGH | **CORRECTED** — the spec says `Digest(…)`, **algorithm-agnostic** (V3 §D.2) | D3's own `[R]` MED ("store chain IDs as `sha256:<hex>`, not bare hex, in case sha512 appears") shows the author knew — but the `[F]`-tagged formula hardcodes the algorithm |
| alpine index has "20 entries", 9218 bytes | D3 §2.6 | `[O]` | **CORRECTED** — **16** entries (9218 bytes is exact). 20 is `hello-world`'s count — a transcription crossover (V3 §B.3) | Nothing downstream depends on it |
| "107 directories in `/var/lib/docker/overlay2/` and 106 symlinks in `overlay2/l/`" | D3 §7.8 | `[O]` | **CORRECTED** — **106** dirs (excluding `l/` itself) + 106 symlinks. The 1:1 correspondence point is right and is *nicer* with the corrected numbers (V3 §K.1) | Trivial |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "Strip `Authorization` when the redirect crosses to a different host" | D3 §2.8 | `[R]` HIGH (our recommendation, with a tradeoff) | **MISCLASSIFIED** — it is a spec **`MUST NOT`**: *"clients … MUST NOT forward `Authorization` headers across host boundaries unless explicitly configured to do so"* (V3 §E.5) | **The dangerous direction of misclassification.** D3 *under*-claimed a genuine normative requirement, so a reader may treat a conformance obligation as a judgement call. Retag `[F]` HIGH and delete the tradeoff sentence |
| "`[C]` HIGH — non-Hub registries: GHCR/Quay/ECR **store what you push**" | D3 §3.3 | `[C]` HIGH | **MISCLASSIFIED** — Quay stores what you push **and also synthesises other representations on demand** (V3 §G.1) | Same root cause as the refutation above |
| `[O] LOCAL HIGH` on Hub's `Accept` behaviour, generalized to "Docker Hub does not content-negotiate", then to registries generally | D3 §2.5, §3.3 | `[O]` → `[C]` HIGH | **MISCLASSIFIED (confidence one notch hot)** — true for Hub (V3 extended it to 7 repos and 2 disjoint-`Accept` probes); **false for Quay and GHCR** (V3 §A.2, §G.1) | The client rule survives — but for a *different reason* than D3 gives. Not "registries ignore `Accept`" but "**no registry's behaviour can be predicted from the request**, so the response `Content-Type` is the only thing you can branch on." Three registries, three behaviours, one rule |

### UNSUPPORTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "Unlinking a live overlay `lowerdir` does not fail" | D3 §7.7 | `[F]` HIGH | **UNSUPPORTED** — unsourced and untested (testing it means mutating a live overlay on a production host). Consistent with dcache behaviour but not established (V3 §K.3) | The *recommendation* (GC must consult `/proc/self/mountinfo` before unlinking) is right regardless |
| Rootless `mknod` EPERM inside a userns; `fuse-overlayfs` vs kernel `userxattr`; Linux ≥ 5.11 | D3 §5.4 | `[C]` (correctly, not `[O]`) | **UNSUPPORTED — pending host** (V3 §I.4) | See §1J for exact commands |

### The V3 re-ranking that matters most for Part 2

**D3 §4.5's storage-key recommendation is right; its "expensive to change" justification is
MISCLASSIFIED** (V3 §D.4). Re-keying a content-addressed store is **not** an expensive migration:
every key is derivable from data already on disk. Given the layer directories and the manifests you
can recompute every `diff_id` and every chain ID **offline** and rename directories — a one-shot
local script, no re-download, no re-extraction.

The genuinely irreversible decision in D3 is **`tar-split` capture during extraction** (D3 §5.5,
§15.6): it can only happen *while* the layer is being extracted, so retrofitting it means
**re-extracting the entire store**. V3 §D.4 and §L both state that D3 §11.5 should rank `tar-split`
first and demote the storage keys. This directly rewrites risk R4 in Part 2.

### Omissions V3 found that change the design

1. **D3 §5.3 lacks the `trusted.overlay.opaque = "x"` marker.** `overlayfs.rst`: a merge directory containing regular files with `trusted.overlay.whiteout` **"should be additionally marked by setting the xattr `trusted.overlay.opaque` to `x`"**. So the xattr has **two** values: `"y"` = opaque directory, `"x"` = "contains xattr-form whiteouts, look for them". Writing xattr-form whiteouts without it produces **whiteouts the kernel may not honour** (V3 §I.2).
2. **Reversing `lchown`/`lchmod` also destroys every `security.capability` xattr**, not just setuid bits — `chown(2)`: *"When the owner or group of an executable file is changed (by any user), all capability sets for the file are cleared."* Measured both directions. `ping`, `newuidmap`/`newgidmap` and every capability-based setuid replacement stop working **with no error anywhere**, and the file mode looks untouched apart from the missing bit — arguably the harder of the two to debug (V3 §J.1).
3. **`history` is OPTIONAL** in `config.md`; D3 §4.4 assumes it is always present (V3 §H).
4. **`manifest.layers` is not marked REQUIRED** — the spec says only "For portability, `layers` SHOULD have at least one entry" (V3 §E.9).
5. **`404` is not proof of absence.** GHCR returns `404` for a wrong `Accept`. Combined with the already-noted "401 means private-or-absent", **neither `401` nor `404` is a reliable existence signal** (V3 §G.2).
6. **D3 §3.4's Schema 1 error text is wrong advice on Quay** ("image was pushed before 2017; ask the publisher to re-push") — there Schema 1 is what a *bad `Accept`* gets you for a modern 2026 image. Correct text: "registry returned manifest schema 1; check the `Accept` header, or the image predates schema 2" (V3 §G.2).
7. **GHCR's `/v2/` challenge carries a fake scope** (`repository:user/image:pull`, a placeholder). D3 §2.2's `[R]` HIGH "do not pre-flight `GET /v2/`" is right, and this is a **sharper reason than the one given** (saving a round trip): on GHCR the pre-flight challenge is *actively misleading*. Also `HEAD /v2/` on GHCR → `405` (V3 §G.3).
8. **Better evidence than D3 has for "descriptor wins over HTTP `Content-Type`"**: GHCR serves the *config blob* of `ghcr.io/distribution/distribution:latest` as `application/vnd.zarf.layer.v1.blob`. A client trusting the HTTP header would conclude the config is a Zarf layer (V3 §E.7).
9. **Better citations D3 should use**: `manifest.md` states layer ordering **normatively** ("The array MUST have the base layer at index 0"); `image-index.md` gives the tie-break rule D3 §2.6's filter needs ("If multiple manifests match … the first matching entry SHOULD be used"); and `distribution-spec/content-negotiation.md` — still marked `TODO`, open issue #212 — is the **documentary origin** of the "Hub down-converts" belief and is now factually stale, which is a far better `[OUTDATED]` citation than the one D3 offers (V3 §E.4, §E.9).

## 1D. Layer extraction, OverlayFS and storage layout (V4 vs D3 §6, §8–§11, §13.3)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| Over-length `mount(2)` options "FAIL with `ELOOP`" | D3 §8.5 | `[O]` | **REFUTED** — `copy_mount_options()` copies at most one page and **never reports "too long"**; `path_mount()` forcibly NULs byte 4095. The string is **silently truncated and then parsed as written**. Boundary measured: **4095 OK / 4096 FAIL** (V4-2.1) | **The single most dangerous belief in the corpus.** We'd have treated the `mount(2)` fallback as fail-safe. It is not: unchecked, it produces a *wrong filesystem* with no error |
| The silent-truncation observation "could not be reproduced; careful re-runs failed cleanly with `ELOOP` every time" | D3 §15.2 | `[O]` LOW, declined | **REFUTED — reproduced on demand.** **6 of 34 (18%)** docker-shaped layouts mounted **successfully with layers missing**. Rule is exact: silent truncation occurs **iff the truncated tail names an existing directory that is not an ancestor of a surviving layer** — which a `<store>/<id>/diff` layout produces by construction (V4-2.3) | It is **deterministic per store layout**, which is exactly why one run reproduces and another does not — D3's re-runs changed the paths. And `<store>/<id>/diff` is the shape D3 §7.4/§11.4 proposes for **us** |
| A shared `upperdir`/`workdir` is "not allowed and may fail with `EBUSY`" (relied on as a rule) | D3 §8.1 | `[F]` | **REFUTED** — at 6.11 two overlay mounts with **identical** upperdir *and* workdir both returned **OK**; only a `dmesg` warning. The doc's operative sentence is the *next* one: *"the behavior of the overlay is undefined"* (V4-5.3) | Starting the same container twice is **not stopped by the kernel** — it silently corrupts the writable layer. New `[R]` HIGH: we must keep our own mount bookkeeping (exclusive lock or refcount per snapshot) |
| tmpfs has "no `trusted.*` xattr support" and needs `userxattr` | D3 §8.8 | `[F]` | **REFUTED — backwards.** tmpfs accepts `trusted.overlay.*` and works as `upperdir` in the **default** namespace. The late arrival is **`user.*`** (`XATTR_USER_PREFIX` absent at v6.5, present at v6.6) (V4-5.6) | **`/tmp` on the host is tmpfs**, so every `[O] HOST-A` overlay experiment in D3 §8 ran on a filesystem §8.8 declares unusable. The results happen to be valid; the §8.8 entry is not |
| `openat2(RESOLVE_IN_ROOT\|RESOLVE_NO_MAGICLINKS)` is "the whole answer" / makes escape "impossible" | D3 §6, §6.3 | `[R]` HIGH | **REFUTED — four gaps** (V4-3.2): (a) `RESOLVE_IN_ROOT` **clamps silently, never reports**; (b) no `RESOLVE_NO_XDEV` ⇒ **mount points inside the root are crossed**; (c) **one plain `openat` with a two-component basename escaped to `/etc/hosts`**; (d) says nothing about what you do with the fd | The safety is `openat2` **plus** single-component basenames **plus** unlink-before-create. Drop any one and the hole reopens. D3 presents it as one structural decision that "kills most of the list at once" |
| "The parsing is boring, well-tested, and not where the danger lives" (argument for using `tar` as parser-only) | D3 §13.3 | `[R]` | **REFUTED** — **RUSTSEC-2026-0068 / CVE-2026-33055**, same crate, same versions, filed the **same day** as 0067: tar-rs ignores PAX size headers when the base header size is nonzero. A **parser-level** entry-smuggling bug in the exact component D3 declares safe (V4-4.2) | Remediation is unchanged (`tar >= 0.4.45`) but the *reasoning* must be: parser-only still needs defence in depth. New `[R]`: after each entry, assert bytes consumed == header size, and reject entries where PAX `size` and the ustar header size disagree |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| The overlay layer ceiling is unknown ("verified 500 … did not find the ceiling") | D3 §15.5, §9.2 | open question | **CORRECTED / CLOSED — exactly 500.** `#define OVL_MAX_STACK 500` in `fs/overlayfs/params.h`, enforced in `ovl_parse_layer()`. Present unchanged since ≤ v5.10, so it binds the `mount(2)` path too (V4-1.1a) | D3's "500 verified to work" **landed exactly on the constant** and read it as an unexplored floor. §9.2's "impose no 128-layer cap" should read "**raise the cap from 128 to the kernel's 500**" |
| (unstated) each `lowerdir+` value length | D3 §8.5 | absent | **CORRECTED (new limit)** — capped at **255 bytes** (`strndup_user(_value, 256)` in `fs/fsopen.c`); 256+ returns `EINVAL` (V4-1.1b) | Harmless for a digest-named store (~100 B) but must be asserted. Combines with §6.10: store root + digest + `/fs` must stay under 255 |
| "Use the new mount API" as the escape from the 4096-byte limit | D3 §8.5, §9.2 | `[R]` | **CORRECTED — a trap.** The **non-append** `fsconfig(SET_STRING,"lowerdir", …)` is capped at **255 too**, i.e. **16× worse than `mount(2)`**. Only `lowerdir+`, one path per call, escapes (V4-1.1c) | A fallback reaching for `fsopen`+`lowerdir` on 6.5/6.6 (new mount API, no `lowerdir+`) fails at 255 bytes. The rule is "`lowerdir+`", not "the new mount API" |
| `lowerdir+` "since Linux **6.8**" | D3 §8.5 | `[F]` HIGH | **CORRECTED / OUTDATED** — present at tag **v6.7** (`fsparam_string("lowerdir+", …)` in `fs/overlayfs/params.c`), absent at v6.6. The kernel's own `overlayfs.rst` also says 6.8 — the prose is one release conservative (V4-1.3) | Low practical impact, but D3 tags it `[F]` HIGH and it is wrong by one release |
| `FSCONFIG_SET_FD` with `lowerdir+` (layers as file descriptors) | D3 §8.5 | `[F]` HIGH, no version | **CORRECTED / UNSUPPORTED as stated** — that is a **separate, later feature: v6.13**. On the 6.11 test host the call would fail (V4-1.3) | An asserted `[F]` HIGH that is unavailable on the very host D3 tested on |
| The usable `mount(2)` option-string maximum | D3 §8.5 | 4096 | **CORRECTED — 4095.** Corroborated by moby's own guard `if len(mountData) > pageSize-1` (V4-2.1, V4-6.1) | Off-by-one on the *only* check standing between the fallback and silent corruption |
| "`mountFrom` **forks a helper** that `chdir`s… requires a fork exec to chroot" | D3 §9.2 | `[C]`, quoting moby | **CORRECTED — the comment is stale.** Current `overlay2/mount.go` is `go func(){ runtime.LockOSThread(); unix.Unshare(unix.CLONE_FS); unix.Chdir(dir); unix.Mount(...) }` — **no fork, no exec, no chroot** (V4-2.6) | Weakens the "largest single simplification" framing further: the expensive-sounding component of the thing being deleted **is not expensive**. Rust equivalent ≈ 15 lines |
| "`4096/95 ≈ 43` layers … the indirection buys 128 instead of ~43 … 28 bytes per layer instead of 51" | D3 §9.2 | `[O]` | **CORRECTED** — an absolute `l/` entry is **54 B** with the colon, not 51. And `28 × 128 = 3584` is the budget for the **relative** form used only by `mountFrom`; Docker's primary path emits **absolute** `l/` paths plus `index=off,`+`upperdir=`+`workdir=`, giving ≈ 3,871 B ⇒ **≈ 71 layers** (V4-6.2) | **Docker cannot reach its own `maxDepth = 128` without falling back to `mountFrom`** — the constant is solved for the fallback, not the common path. This *strengthens* the delete-the-subsystem argument on the merits |
| "`workdir` must be an empty directory" | D3 §8.1 | `[F]` | **CORRECTED** — documented but **not enforced** at 6.11: a workdir containing a stray file mounted fine (V4-5.2) | A leftover file from a crashed run **will not be caught**. Enforce it ourselves. (Also newly established: `workdir` inside `upperdir`, or equal to it, **is** `EINVAL`) |
| `volatile`'s marker "forces a fresh upper on next mount" | D3 §8.7 | `[R]` LOW | **CORRECTED** — it does not. Remount without `volatile` → **`EINVAL`**, `dmesg: overlay with incompat feature 'volatile' cannot be mounted`. Kernel doc: *"refuses to mount if present"*. The caller must delete the workdir (V4-5.7) | Would produce a **stuck container on first use**. The `[R]` survives; the mechanism sentence is wrong |
| "Reject `security.selinux`, `security.ima`, `security.evm`" (implying the kernel helps) | D3 §6.7 | `[R]` | **CORRECTED** — on this host `security.selinux` **is settable** (no policy loaded) while `security.ima` returns `EINVAL` (V4-3.3) | Rejecting is still right; **do not expect the kernel to reject them for you** |
| "`async-tar` (**CVE-2025-62518**, 'TARmageddon') and `astral-tokio-tar` (CVE-2025-59825)" | D3 §6.3, §13.3 | `[F]` | **CORRECTED** — `async-tar` has **no RustSec advisory at all**; the family advisory is **RUSTSEC-2025-0111 against `tokio-tar`**. **CVE-2025-62518 is filed against `astral-tokio-tar`** (≤ 0.5.5, patched 0.5.6). CVE-2025-59825 (`astral-tokio-tar` ≤ 0.5.3, fixed 0.5.4) and its paired-symlink quote are CONFIRMED (V4-4.3) | Wrong crate/CVE pairing. The lineage D3 presumably meant (all derived from the original `async-tar` fork of tar-rs) is in the advisory text |
| "Never let a global header mutate defaults — **that mechanism is exactly what TARmageddon-class bugs exploit**" | D3 §6.12 | `[F]` | **CORRECTED** — the mechanism is *"incorrect stream position advancement when PAX headers override size information"*, a **per-entry `x` header size desync**, not a `g` global header (V4-4.4) | Ignoring `g` is correct hygiene and should stay — **but it does not mitigate the bug class D3 names it for**. The mitigation that does is the byte-consumed assertion above |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| Deleting Docker's `l/` symlink farm is "**the largest single simplification available to us** versus Docker's design" | D3 §9.2 | `[R]`, unconditional | **MISCLASSIFIED** — it is a recommendation **contingent on a 6.7 kernel floor** that another part of the corpus does not grant (V4-1.4) | 6.7 excludes Debian 12 (6.1), RHEL 9 (5.14), Ubuntu 22.04 (5.15); Ubuntu 24.04 (6.8) just clears it. **That is most of the installed base** — not a rounding error. See risk R5 |
| "Docker carries `cache-id` **because** it cannot rename the overlay2 directories" | D3 §11.5 | `[O]` | **MISCLASSIFIED — inference presented as observation.** `cache-id` lives in the graphdriver-**agnostic** layer store (aufs, btrfs, zfs, vfs, devicemapper use it identically — "cannot rename the directory" is meaningless for a thin block device or a zfs dataset). The real reason is in `migration.go`: `RegisterByGraphID` — the Docker 1.10 content-addressability migration had to adopt pre-existing randomly-named directories under newly computed chain IDs **without moving data** (V4-6.4) | The premise would mislead a reader into thinking a kernel or filesystem limitation is at work. The recommendation is fine; it just does not follow from the stated premise. (And to the extent renaming *is* hard for overlay2, that is self-inflicted via `l/` and `lower` — which D3's own recommendation deletes) |
| "`ELOOP` is a spectacularly misleading errno for 'your option string is too long'" | D3 §8.5 | `[O]` generalized | **MISCLASSIFIED** — a single-layout observation generalized to Linux. Sweeps: flat layout **ENOENT 33 / ELOOP 7**; docker-shaped **ENOENT 23 / ELOOP 5 / SILENT 6**. `ELOOP` here genuinely means `ovl_lookup_trap_inode()` "Caught in a trap of overlapping layers" (V4-2.2) | **The errno is a property of your directory naming, not of the kernel's length handling.** `ENOENT` dominates; `ELOOP` appears when the cut lands on a separator; silent success when it lands on an existing non-ancestor directory |
| The 5,000,000-entry tar cap | D3 §6.9 | `[R]` LOW (honestly admitted as a guess) | **MISCLASSIFIED** — a number that **cannot bind before two other limits do**: 5M zero-byte entries at 512 B/header is a 2.4 GB archive, already stopped by the `descriptor.size` cap and the 10 GiB uncompressed cap. The binding constraint is **inode exhaustion**, a filesystem property (V4-3.4) | Replace with a check against `statfs(store).f_ffree` at extraction start, or drop it |
| btrfs/zfs "irrelevant to us" | D3 §10.4 | `[R]` LOW | **MISCLASSIFIED (mild)** — a project decision, not a finding. Supporting facts (zfs out-of-tree, btrfs snapshot-count scaling) are correct and uncontested (V4-7) | Noted for completeness only |

### UNSUPPORTED

| Claim (one line) | Where | Now | Settles it |
|---|---|---|---|
| Unprivileged overlay at RHEL-8 4.18 — which mechanism | D3 §15.1 | **UNSUPPORTED — pending host** (`old-host` NXDOMAIN, re-checked) | See §1J |
| NFS-as-`upperdir` errno | D3 §15.1 | **UNSUPPORTED — pending host.** The kernel doc supports the *reasoning* (v6.11 line 101-104: upper must supply `d_type` and `trusted.*` and/or `user.*`, *"so NFS is not suitable"*), but the errno is unobserved | See §1J |
| `fuse-overlayfs` "2–5× on metadata-heavy workloads" | D3 §10.3 | **UNSUPPORTED as a number** — no citation, not measured. The qualitative claim (userspace round-trips are slower) is uncontroversial and changes no recommendation | A benchmark on any host |

### NEW hazards V4 found that D3 does not record

1. **A read-write overlay silently becomes read-only.** Truncation eats the **end** of the string, so `lowerdir=…,upperdir=…,workdir=…` loses `upperdir` and `workdir` entirely — and overlay then *legally* mounts read-only, since both are optional. Measured: asked for 139 layers + upper + work (4,357-byte option string), **got a read-only mount with 131 of 139 layers and `EROFS` on write** (V4-2.4). D3 §8.5 explicitly claims "identical results whether the string is `lowerdir=…,upperdir=…,workdir=…` or `upperdir=…,workdir=…,lowerdir=…`" — the *errno* may look alike, but **option order decides which options are destroyed, and putting `lowerdir` last is strictly safer**.
2. **A read-only overlay needs ≥ 2 lower layers.** `lowerdir=<one>` with no upper → `EINVAL`, `dmesg: at least 2 lowerdir are needed while upperdir nonexistent`. Duplicating the layer gives **`ELOOP` "conflicting lowerdir path"**. So D3 §11.4 step 6's acceptance criterion is **unachievable for `alpine`** — the single-layer image D3 uses as its running example throughout (V4-5.4). This is also a **third** legitimate source of `ELOOP` unrelated to option length.
3. **`workdir` inside `upperdir`, or equal to it, is `EINVAL`** (enforced). `snapshots/<id>/{fs,work}` is fine; `snapshots/<id>/fs/work` is not (V4-5.2).
4. **A 500-layer `lowerdir+` mount renders a 15 KB single line** in `/proc/mounts` / `mountinfo`, as **repeated `lowerdir+=` keys** — so anything parsing for `lowerdir=` (including D3 §8.4's own methodology, and plenty of monitoring agents) **will not find it** (V4-1.2).
5. **Mount exclusivity must be enforced in our own code** — the kernel will not do it (V4-5.3).
6. **The xattr namespace a layer was extracted with belongs on the "expensive to change" list.** A layer whose whiteouts/opaque markers are `trusted.overlay.*` cannot be used under a `userxattr` mount and vice versa; switching later means **re-extracting the entire store**, exactly like `tar-split`. D3 says "record it in `info.json`", which is necessary but does not make it cheap to change (V4-6.5). Related and unreconciled: D3 §6.8 concedes that under a userns "the store must be keyed by `chainID + idmap`, not chainID alone" — which **contradicts §11.5's flat `diff_id` keying** unless idmapped mounts (5.12+) are adopted.
7. **`RESOLVE_BENEATH` is the better primitive for an extractor** and D3 never mentions it: measured, the same escaping path returns **`EXDEV`** instead of being silently clamped, so the attack becomes an error rather than a silent rewrite (V4-3.2a).
8. **There is no `openat2` form of** `mkdirat`, `symlinkat`, `linkat`, `mknodat`, `unlinkat`, `fchownat`, `fchmodat`, `utimensat` or `setxattr` — all take `dirfd + name` with ordinary resolution. In particular **`fchmodat` on Linux has no working `AT_SYMLINK_NOFOLLOW`**, so D3 §6.14 step 10 applied to a name an earlier entry made a symlink **is literally RUSTSEC-2026-0067** (V4-3.2c).
9. **Returning a `Mount` *description* rather than a mounted path is not optional.** Because `lowerdir+` mounts must be built with `fsopen`/`fsconfig` then `move_mount`ed into the target namespace, the storage layer **physically cannot** "return a mounted path" without mounting in the wrong namespace. The description must carry the **list** of lower layers, not a pre-joined `lowerdir=` string, or the whole `lowerdir+` benefit is thrown away at the interface (V4-6.6).
10. **The kernel validates `security.capability` blobs** — an arbitrary value is rejected `EINVAL`, so a malformed `SCHILY.xattr.security.capability` record fails loudly rather than silently. A small mercy D3 does not mention (V4-3.3).
11. **Host-cleanliness note:** `/tmp` on `host-a` still contains six files left by the D3 session (`ovl611.rst`, `ovlparams611.c`, `ovlsuper611.c`, `ubparams.c`, `v3_accept_test.py`, `e.3088863`, all dated Sep 4). Harmless, deliberately not deleted on a production host — but "left the host clean" is not quite accurate, and the next session should not inherit a false baseline (V4-11).

## 1E. Networking (V5 vs D4, all but §12 crate metrics)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| Writing `net.ipv4.ip_forward` *"resets all configuration parameters to their default state"* — blowing away `rp_filter`, `accept_local`, `route_localnet`, `arp_ignore` | D4 §1.1, §9.8 Rule 2, §11.2 | `[F]` HIGH, stated 3× | **REFUTED** — `inet_forward_change()` (v6.11 `net/ipv4/devinet.c`) sets **only** `all.accept_redirects = !on`, `default.forwarding = on`, `<iface>.forwarding = on` for every device, and **disables LRO on every netdev**. Nothing else. Live: `rp_filter = 2` (Ubuntu's value, not the kernel default 0) **survives** on a host with `ip_forward = 1` (V5 §A1) | The quote is **real**; `ip-sysctl.rst` has been **stale since the 2.2/2.4 era**, when the RFC1122-host/RFC1812-router distinction did drive a wider set. All four named knobs are safe. Also: **a no-op write is genuinely a no-op** (`if (write && *valp != val)`), so "a blind write is a gratuitous reset of the admin's tuning" is refuted at the kernel level too |
| "The single most dangerous operation in the ladder is stage 3's `ip_forward` write" | D4 §11.2 | ranking | **REFUTED** (follows from the above) — the most dangerous operation is **stage 4's first `-p` publish on a host running a foreign firewall manager** (D4 §4.6): verified, exploitable by accident, and **no ufw command can undo it** (V5 §A1, §C5) | We'd have hardened the harmless operation and shipped the dangerous one unwarned |
| Stage 2 (veth + manual routes) is "not user-useful, a test fixture rather than a feature"; the first useful rung is bridge+NAT | D4 §11.1, §11.2 | `[R]` | **REFUTED** — **the bridge is not what makes it useful.** veth + `ip_forward` + MASQUERADE + `resolv.conf` = full `apt-get`-capable egress **with no bridge**. That is exactly the **CNI `ptp` plugin** and **Docker 28's `gateway_mode: routed`** (V5 §F2, §F3) | We'd inherit the entire bridge tax (`br_netfilter`, hairpin, `IFLA_BRPORT_ISOLATED` vs `--icc`, FDB/STP, MTU-inheritance-that-doesn't-happen, and a class-C device that leaks forever) on day one, for a benefit (L2 adjacency) a single-host MVP may never need |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "Docker's reconciliation runs at daemon start and this daemon has not restarted since March" — offered as the **cause** of the observed live leak | D4 §9.3 | `[O]` MED | **CORRECTED — a daemon restart would NOT have cleaned it.** `removeIPChains()` removes **eleven Docker-owned chains** and never touches built-in `nat/POSTROUTING` or `raw/PREROUTING`, where all three leaked rules live; nothing enumerates orphan `br-*` devices (V5 §A2). The **observation** is fully re-verified and six innocent explanations are each ruled out | **The single most important design correction in D4.** The lesson is not "reconcile more often" — reconciling at every start would not have found this. It is: *the engine wrote state into chains it does not own, and then lost the key needed to find it again.* D4 already holds the quote that explains it (§9.4 rule 4, from moby: *"Without seeing config … the iptabler's cleaner can't clean up network or port-specific rules that may have been added to iptables built-in chains"*) — it just never connected it to §9.3 |
| "nginx binds `:80` with no sysctl change at all" in a rootless container netns | D4 §8.5 | `[F]` | **CORRECTED** — true only while the process **retains** `CAP_NET_BIND_SERVICE`. For the very common non-root `USER` image the effective set is empty, `ns_capable` fails regardless of netns ownership, and the bind returns `-EACCES`. Live: **all five containers have `ip_unprivileged_port_start = 0` with `HostConfig.Sysctls = null`** — Docker sets it **per-netns, unasked**, and that is the half the capability does not cover (V5 §G1) | The correction of the folklore is right; the **fix is incomplete**. Add the per-netns sysctl as remedy 0. D4's own §8.2 already uses this exact per-netns technique for `ping_group_range` and never applies it to the port threshold |
| `--network=host` is "1 line" / "omit `CLONE_NEWNET`" | D4 §11.1 | `[R]` | **CORRECTED** — also requires **short-circuiting the entire §4 publish path** (`-p` is meaningless in host netns; Docker warns and discards) and a **second `resolv.conf` branch** (the host `127.0.0.53` stub *is* reachable here and nowhere else). ~10 lines with a real branch (V5 §F1) | An engine that omits `CLONE_NEWNET` and still runs its DNAT/anti-spoof/helper path against a container with no distinct IP **writes nonsense rules**. Also missing from the cost column: **the abstract UNIX socket namespace is per-netns** (`network_namespaces(7)`), so host mode hands over every `@`-prefixed host listener — a well-known lateral-movement surface, not what "zero isolation, zero overhead" conveys |
| Docker docs quoted as "For bridge networks, iptables and nftables have the same functionality" | D4 §5.3 | `[C]` | **CORRECTED** — the same docs also call the nftables backend **experimental** and say *"With the experimental nftables backend, Docker does not enable IP forwarding itself."* Not the same functionality. Also missing: the operator-facing **`--firewall-backend {iptables\|nftables}`** daemon flag (V5 §D3) | Matters because D4 §11's ladder puts the `ip_forward` write at stage 3 — whether the backend does it for you is exactly that stage's question |
| `inet_port_requires_bind_service()` cited as living in `af_inet.c` | D4 §8.5 | `[F]` | **CORRECTED (trivial)** — it lives in `include/net/ip.h`. The code itself is quoted character-for-character correctly (V5 §G1) | Citation hygiene only |
| §4.6 quotes `ufw-user-input` as three rules | D4 §4.6 | `[O]` | **CORRECTED** — it has **five** (the `:22` rule carries `-m comment --comment "'dapp_OpenSSH'"`, plus a UDP/443 QUIC rule and a duplicate TCP/443) (V5 §C1) | Changes nothing about the argument |

### OUTDATED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "netavark has **both** an iptables and an nftables firewall driver … the iptables driver is deprecated and slated for removal" | D4 §5.3 | `[C]`, `[LOW]` on timeline | **OUTDATED — already removed.** `src/firewall/` on `main` = `firewalld.rs`, `fwnone.rs`, `mod.rs`, `nft.rs`, `state.rs` — **no `iptables.rs`**; `mod.rs` defines only `FIREWALLD`/`NFTABLES`/`NONE`, default selected at compile time between `nftables` and `none` (V5 §D3) | Net effect is a **strengthening**: D4 §5.4's "direction of travel" argument is one step further along than stated. The cited Fedora change (F41) only ever proposed *switching the default* and kept an iptables contingency — the `[LOW]` hedge was the right instinct, but the timeline was **shorter**, not longer |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "**Every IPv6 connection to this production web server is being carried by `docker-proxy`**, in userspace, and nothing else … this host proves it / verified live above" | D4 §4.4 | `[O]` HIGH | **MISCLASSIFIED** — `ip -6 addr show scope global` is **empty**; only `fe80::/64` link-local. The host has **no IPv6 connectivity** and carries ~zero IPv6 connections (the two `[::]` proxies show the same 3 s CPU over 24 days as the idle v4 ones). The claim is **vacuously true and presented as an empirical demonstration** (V5 §C3) | The *structural* argument (a NAT rule cannot change address family, so `[::]:80 → 172.18.0.4:80` requires a process that terminates one connection and opens another) is **correct and is the real content** — but it is a `[F]` about NAT, not an `[O]` about this box. **The honest `[O]` is narrower and more actionable:** Docker starts the address-family-bridging helper **unconditionally, without checking whether the host has IPv6 at all**. Two idle supervised processes per published port, on the majority of hosts. → gate our helper on the host actually having a global address in that family |

### UNSUPPORTED

| Claim (one line) | Where | Now | What would settle it |
|---|---|---|---|
| "`rust-netlink` is a GitHub **organisation** maintained by the nmstate/Red Hat networking people", contrasted with `neli`'s "bus factor of one" | D4 §12.2 | **UNSUPPORTED** — no governance document, README, or org description names Red Hat or nmstate; self-description is *"building blocks for netlink"*; **two** visible members (@cathay4t, @JohnTitor). Two members with one doing the bulk is **the same bus-factor shape** as `neli` (V5 §H2) | Drop the organisation argument. The two **technical** reasons (a 293-file rtnetlink attribute surface including `InfoVeth::Peer`; consumption by netavark) carry the recommendation on their own |
| The random-high-port embedded-resolver design exists for conflict avoidance with a container binding `0.0.0.0:53` | D4 §6.1 | **UNSUPPORTED (correctly self-flagged `[MED]`)** — `daemon/libnetwork/resolver.go` on `master` has **no comment explaining the choice**; it defines `dnsPort = "53"`, takes the port as a parameter to `SetupFunc(port uint16)`, and calls `setupNAT()` with no rationale (V5 §E1) | The commit message / design doc for libnetwork's resolver. The hedge and D4's own "what would settle it" stand verbatim |
| `IFLA_NET_NS_FD` **inside** the `VETH_INFO_PEER` nest creates the peer directly in the target netns — "Tradeoff: none that I found" | D4 §2.2 | **UNSUPPORTED** — a strong no-downside claim about a non-obvious kernel path, not host-checkable and not verified (V5 §H6) | A round-trip test against a real kernel, which D4 §2.6 already budgets for. **The design leans hard on this** — §9.2 item 5 claims it eliminates a whole failure class |
| Every `old-host` observation in §8.1–§8.4, §8.6; the §1.1 sysctl table; §1.3; §1.6 empirical; §2.2 ifindex ordering; §2.5 MTU experiment; §2.6(4) grammar drift | D4 various | **UNSUPPORTED — pending host.** Kernel-source halves verified at v6.11 (`netns_install`, `rtnl_link_get_net_capable`, `__inet_bind`, `rtnl_dev_combine_flags`) — all reproduced verbatim, so the citation discipline is good | See §1J for exact commands |
| §8.7 pasta throughput: RootlessKit CI 0.24 Gbps vs passt CI 2.5 Gbps at MTU 1500 (10× discrepancy) | D4 §8.7 | **UNSUPPORTED / open** — D4 flags it and it remains open (V5 §I) | A controlled benchmark |

### Upgrades V5 produced (see also §1H)

| Claim | Where | Was | Now |
|---|---|---|---|
| A fresh netns **inherits** `ip_forward` from `init_net`'s live values rather than getting the documented default 0 | D4 §1.1 | observation HIGH, **mechanism `[LOW]`** | **`[F]` HIGH** — `devinet_init_net()` → `net_inherit_devconf()`, governed by **`net.core.devconf_inherit_init_net`** (= 0 on the host: copy `init_net`'s live values). Confirmed on a **second host and a different kernel** (6.11 vs 4.18): host `ip_forward=1`, every container netns `ip_forward=1`. Design advice ("read it, don't guess") now well-founded |
| Widen `ping_group_range` inside the container netns — per-netns and free | D4 §8.2 | `[R]` from libslirp source + `old-host` | **Confirmed in Docker's production behaviour** on a different host: host `1 0`, every container netns `0 2147483647` |
| `ufw deny 80` does not stop `-p 80:80` | D4 §4.6 | `[F]` HIGH | **Stronger** — a knowledgeable admin's *correct* incantation **`ufw route deny 80` also fails**: it lands in `ufw-user-forward`, reached from `ufw-before-forward` at FORWARD position 4, still after `DOCKER-FORWARD`'s terminal ACCEPT. **There is no ufw command at all that closes a Docker-published port.** `DOCKER-USER` is the only lever |
| The live leak (§9.3) | D4 §9.3 | `[O]` HIGH, cause undetermined | Observation **fully re-verified** independently; six candidate innocent explanations each ruled out (no swarm, no second engine, no compose artifact, nothing in flight, no boltdb record, all counters zero); and the **mechanism identified** |

### Design gaps V5 found

1. **D4 §5.4's own points 3 and 6 are never joined.** Coexistence with a foreign iptables `-P FORWARD DROP` is correctly called the strongest argument against native nft — and this host demonstrates it. But the mitigation is not only "warn": nftables lets us add an `accept` in a table at a **lower priority number**, ahead of the foreign chain, which iptables cannot do (V5 §D4).
2. **D4 §6.5 stage 3's shortcut is under-priced.** "Bind `127.0.0.11:53` directly and skip the DNAT/SNAT rules" is a reasonable trade, but it costs: you can no longer serve DNS to a container sharing the netns (`--network=container:X`, pods, §7.3) that runs its own `0.0.0.0:53`, **and the failure lands at container-start for the second container**, not at resolver-setup. With DNAT the resolver's socket can be re-bound on a different port without touching the container's view; with a fixed `:53` bind it cannot (V5 §E4).
3. **D4 §5.4's `nftnl` `[R]` MED should be deleted, not annotated.** §12.3's self-correction is itself correct (nobody drives nftables via `libnftnl`: moby uses `nft -f -` or `libnftables` via cgo; netavark uses `nft -j` through the `nftables` crate), and D4 marks §5.4 "superseded by §12.3" — but leaves a wrong recommendation standing three sections earlier (V5 §D4).
4. **Recommended re-split of D4's stage 3** (V5 §F2): **3a — routed NAT egress** (veth, no bridge, `ip_forward`, MASQUERADE, `resolv.conf` rewriting) is the first user-useful rung, creates **no class-C device**, and gets L3 policy for free — container↔container traffic is *routed through the host*, so it traverses `filter/FORWARD` and per-pair policy is an ordinary rule, dissolving D4 §3.4's entire `--icc=false` problem. **3b — bridge**, added when L2 adjacency, a shared gateway, or address density actually demands it, and that is where the §3.4/§3.5/§7.2/§9 tax gets paid.

## 1F. Resource control and security (V6 vs D5)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| `ptrace`, `process_vm_readv`, `process_vm_writev` are **denied** by Docker's default profile "because their gating capabilities are not in the default set" | D5 §8.5 | `[F]` | **REFUTED** — moby v28.3.1 `default.json` **allows** all three via `{"names":[…],"action":"SCMP_ACT_ALLOW","includes":{"minKernel":"4.8"}}`. `includes.minKernel` resolves against the **host kernel**, not capabilities, and matches on every kernel we would target (V6 §A.2) | **The dangerous direction.** D5 uses this list as its evidence for "cap↔seccomp correlation is defence in depth" — but for `ptrace` there **is no second layer**. A container process can `ptrace`/`process_vm_readv` another process of the same uid in the same container, defeating in-container privilege separation (nginx master/worker, a `--user`-dropped helper beside a root sidecar). With `--pid=host` + default profile on a host with no LSM, a root container can `process_vm_readv` **any host root process** — reading needs no `CAP_SYS_PTRACE` when uids match, which D5 §15.3 understates. Host-A has `kernel.yama.ptrace_scope = 1`, which mitigates but is host-configurable and **defaults to 0 on RHEL/Fedora** |
| `--oom-kill-disable` "errors in v2"; Docker "errors out with *OOM kill disable is not supported*" | D5 §3.0, §3.1 | `[F]` | **REFUTED** — moby v28.3.1 `daemon_unix.go` **warn-and-discards** it (`"Your kernel does not support OomKillDisable. OomKillDisable discarded."`). **The quoted string does not exist in the source** (V6 §M.2) | We'd have copied a hard-fail behaviour Docker does not have, and cited a fabricated error string |

### CORRECTED (ordered by consequence — V6's own ordering)

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| Capability sequence listed as `setgid/setuid` **then** `PR_CAPBSET_DROP` | D5 §7.7 listing, §20 C9–C13 | normative listings | **CORRECTED — inverted.** `setuid` from 0 clears `CAP_SETPCAP` from the **effective** set, and `PR_SET_KEEPCAPS` preserves only **permitted**, so every subsequent `PR_CAPBSET_DROP` fails `EPERM`. Correct order: `capbset_drop → capset → [KEEPCAPS] → setgroups/setgid/setuid → capset → ambient` (V6 §B.4) | **The single most consequential error in D5.** The container runs with the **full 41-capability bounding set**, silently, if the return value is ignored — precisely the trap the section itself warns about, reproduced in its own numbered listing. §20 C9's `[only if uid change + caps wanted]` guard makes it worse: with no caps wanted, KEEPCAPS is skipped entirely. **The prose recommendation is right; the two normative listings are wrong, and the listings are what an implementer copies** |
| CVE-2024-21626 requirement 4: "confirm the cwd's `st_dev` matches the rootfs's" | D5 §11.4 | `[R]` | **CORRECTED** — the container's cwd legitimately sits on a **different `st_dev`** in ordinary use: any bind-mounted volume, any tmpfs, `/dev/shm`, an overlay whose upper/lower differ. Correct check: *"the cwd is **beneath the rootfs in the mount tree**"* — resolve via `openat2(rootfs_fd, …, RESOLVE_BENEATH)`, or walk `mountinfo` and confirm the cwd's mount id is the rootfs mount or a descendant (V6 §H.4) | **A false-positive generator that an implementer will delete the first time a `-v` volume breaks — taking the backstop with it.** Requirements 1 (`O_CLOEXEC`) and 2 (`close_range(3, ~0U, 0)`) and 3 (`openat2 RESOLVE_BENEATH\|RESOLVE_NO_MAGICLINKS`) are correct and are exactly runc's fix |
| `io.weight` is the BFQ knob; `--blkio-weight=500` → `4954` | D5 §3.6, §3.0 | `[F]`/`[O]` | **CORRECTED** — `io.weight` is **iocost's** file; BFQ's is **`io.bfq.weight`**, and runc writes `io.bfq.weight` **first**, falling back to `io.weight` only when BFQ is absent. The fallback value is **4950**, not 4954. Also `253:0` is `vda` per `lsblk`, not "the dm/vda device" — there is no device-mapper in this stack — and `253:16` appears in the **root** cgroup's `io.stat`, not the container's (V6 §G.3, §M.1) | Writing the wrong file to the wrong scheduler |
| CVE-2019-5736 mechanism | D5 §11.1 | `[F]` | **CORRECTED (garbled two ways)** — (a) the `#!/proc/self/exe` shebang is the **bait planted inside the container**, not what is written to the host binary; the payload is written by a **second** process via `/proc/<pid>/exe` after `ETXTBSY` clears; (b) the CVE covers **`docker run` with a malicious image**, not only `docker exec` (V6 §H.1) | The derived requirement survives — but **design 2 ("never exec our binary in container ns") does not cover `mydocker exec`**: the forked child that `setns`es into an *existing* container has `/proc/self/exe` pointing at our host binary for the whole window between `setns` and `execve`, visible to the container's existing processes. `ETXTBSY` is the only protection and it evaporates on restart. **Add `PR_SET_DUMPABLE(0)` before `setns`, or a sealed memfd for the `exec` path.** The two designs are **complements, not alternatives** |
| "Root gets its bounding set back **whatever** we set P and E to" | D5 §7.2 | `[F]` | **CORRECTED** — omits **`SECBIT_NOROOT`**, which disables exactly that rule. Also: rule (1) triggers on **real or effective** uid 0 while rule (2) needs **effective** uid 0 (V6 §N.2) | A stated absolute that has a documented off-switch |
| §8.3 rule 1: "every i386 syscall number is denied by construction" | D5 §8.3 | `[F]` | **CORRECTED** — false; i386 numbers **collide** with x86_64 numbers. Only the **x32** half is structural. The i386 denial comes entirely from rule 2 (check `arch`) (V6 §A.4) | Rule 1 must not be stated as independent — believing it means dropping the `arch` check that is doing all the work |
| `--memory-swappiness` is "silently dropped" | D5 §3.0 | `[O]` | **CORRECTED** — dropped **with a client-visible warning** (V6 §N.2) | Minor |
| "Only **7** masked paths appear in the live mountinfo" | D5 §10.3 | `[O]` | **CORRECTED** — it is **8** (12 − 4 absent), re-counted from mountinfo (V6 §F.2) | Minor; the 4-absent count itself is CONFIRMED (`asound`, `timer_stats`, `sched_debug`, `powercap`, plus the dynamic `thermal_throttle` paths) |
| `aikserver-redis-1` "runs `USER redis`" | D5 §7.3 | `[O]` | **CORRECTED** — `Config.User` is **empty on all five containers**; the uid change is `su-exec` in `redis:7-alpine`'s entrypoint (V6 §N.2) | The measurement is evidence about the **kernel's uid-change rule**, not about runtime behaviour |
| `syslog` gated on `CAP_SYS_ADMIN`; `reboot` on the enumerated caps | D5 §8.5 | `[F]` | **CORRECTED** — `syslog` is `CAP_SYS_ADMIN` **or `CAP_SYSLOG`**; `reboot` is `CAP_SYS_BOOT`. Both still denied under the default 14 — the conclusion holds, the attribution does not (V6 §A.2) | Citation hygiene |
| `CAP_CHECKPOINT_RESTORE` since 5.8 | D5 §6.1 | `[F]` | **CORRECTED** — **5.9**. (`CAP_BPF`/`CAP_PERFMON` are 5.8) (V6 §I.1) | Off by one release in a version-floor table |
| `cgroup.max.descendants` bounds dying cgroups | D5 §4.7 | `[F]` | **CORRECTED** — it caps **live** descendants only; **there is no limit on dying ones** (V6 §K.2) | The sentence implies a bound that does not exist |
| `cgroup.kill` quoted as *"not supported in threaded cgroups"* | D5 §4.2 | quoted | **CORRECTED** — a **paraphrase in quotation marks**. The kernel doc's actual sentence names the errno: **`EOPNOTSUPP`** (V6 §C.5) | Typographically misleading; substantively right |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "`io.weight` is a silent no-op — **never implement `--blkio-weight`**"; tiered "anti-feature" | D5 §3.6, §19.1 | `[R]` | **MISCLASSIFIED** — a **single-host observation** (mq-deadline + BFQ module unloaded + iocost unconfigured) generalized into a universal rule, **contradicting D5's own §5.4 "hard-fail on requested-but-impossible" policy**. On a BFQ or iocost host the flag works, and **BFQ is `CONFIG_IOSCHED_BFQ=m` on this very box — one `modprobe` away** (V6 §G.3) | Correct recommendation: **implement it, probe `io.bfq.weight`/`io.cost.qos`, refuse loudly when inert** |
| "Dropping `CAP_MKNOD` + seccomp `mknod` denial + `nodev` rootfs buys **~95% of the device cgroup's value** for zero dependencies" | D5 §14.1 | `[R]` | **MISCLASSIFIED / UNSUPPORTED** — an unsupported number hiding a **categorical** gap. The device cgroup gates **`open()`**, i.e. *access*, not creation. `--device /dev/sda` or any bind of a host device node produces a live node with **no `mknod` anywhere**, and `nodev` on the *rootfs* superblock does not apply to a different mount. Under Docker `--device` still passes a kernel-enforced allowlist; under the proposal it is enforced **only by our own userspace code, with no kernel backstop** (V6 §G.2) | Restate as: *"covers the malicious-image and in-container-`mknod` threat **completely**; covers **nothing** about devices the operator explicitly passes in."* Therefore **`--device` is gated on the eBPF controller, not optional-with-it.** Also lost: access modes (`--device /dev/sdb:r`, `--device-cgroup-rule`, per-device `rwm`) are unimplementable — and D5 discards the **third** enforcement subsystem while §8.5 argues *for* subsystem independence, without noticing the inconsistency |
| §14.1 bullet (b): the device control is redundant because "`CAP_MKNOD` is a no-op outside the initial user namespace" | D5 §14.1 | `[F]` | **MISCLASSIFIED** — irrelevant to the default rootful container, which **is** in the initial userns (re-measured: `uid_map = 0 0 4294967295`, `ns/user → user:[4026531837]`). **Its own next bullet contradicts it** (V6 §G.2) | Delete (b) or scope it to rootless |
| `CapPrm = CapEff = CapBnd` on the root container presented as confirming what runc set | D5 §7.2, §7.3 | `[O]` | **MISCLASSIFIED** — the kernel's root-exec rule would produce that result **regardless**. The measurement cannot distinguish the two hypotheses (V6 §N.3) | An observation that cannot support the inference drawn from it |
| `hidepid` dismissed because "inside a container everything usually runs as the same uid" | D5 §10.4 | `[R]` | **MISCLASSIFIED** — an empirical generalization doing a security argument's work, and **false under `--user`**. The conclusion (skip it) is right for a different reason (V6 §K.3) | Right answer, wrong reason — the reason breaks exactly where it matters |
| The cgroup `EBUSY` errno tagged `[F]` **HIGH** while §23 item 3 admits it was **inferred**, not read | D5 §1.3 | `[F]` HIGH | **MISCLASSIFIED (tag inflation)** — should be `[F]` MED or `[R]`. **The value is correct** (`cgroup_subtree_control_write` returns `-EBUSY` when `cgroup_is_populated`), but D5 did not check it and says so (V6 §C.2) | Confirming needs a cgroup write — D5 §21 item 3's dev-VM experiment |
| "ROCKY has SELinux disabled" carried as an `[O]` with **no `(D1)` provenance marker and no demotion**, unlike every other ROCKY claim | D5 §12.2 | `[O]` | **MISCLASSIFIED** (V6 §N.3) | Inconsistent provenance discipline hides an unverifiable claim among verified ones |
| The read-only remount recommendation (`mount_setattr(AT_RECURSIVE)` on ≥5.12) presented as **describing runc's mechanism** | D5 §10.3 | `[R]` | **MISCLASSIFIED — a deviation presented as standard.** runc binds **recursively** (`MS_REC`) but remounts read-only **non-recursively**, so under runc a submount under `/proc/sys` is present and writable. D5's `[R]` is *stronger* than runc, which is fine — but must be **labelled a deliberate improvement** so it does not get "simplified" back in review (V6 §F.4) | And D5 **omits** the requirement visible in the same runc snippet: the remount must **re-specify `MS_NOSUID\|MS_NODEV\|MS_NOEXEC`**, read back via `statfs(2)`. `MS_REMOUNT\|MS_BIND` sets per-mount flags to exactly what is passed; **flags not passed are cleared**. A naive `MS_REMOUNT\|MS_BIND\|MS_RDONLY` on `/proc/sys` makes it read-only **and simultaneously clears `nosuid`, `nodev`, `noexec`** — a net security regression introduced by the hardening step. Belongs in §10.3 and §20's C7 |

### UNSUPPORTED — pending `old-host`

| Claim | Where | Command that settles it |
|---|---|---|
| `newuidmap` failure text with no `/etc/subuid` entry | D5 rootless | `newuidmap $$ 0 1000 1` as the unprivileged user |
| cgroup **v1** write errno on a Slurm-owned hierarchy (`EACCES` vs `EPERM`) | D5 | `mkdir /sys/fs/cgroup/memory/test` |
| RHEL overlay-in-userns backport | D5 §6.2, §23 item 6 | `rpm -q --changelog kernel \| grep -i overlay` |

**Not host-blocked after all:** the `EINVAL` claims in D5 §9.3 (`chown`/`setuid` to an unmapped uid) are **decidable from kernel source** — `fs/open.c:chown_common()` and `kernel/sys.c:__sys_setuid()` — and V6 §L.1 **CONFIRMED** them. Promote §9.3 to `[F]` HIGH on a source citation.

### Open questions V6 closes with no experiment

- **D5 §23 item 2 / §21 item 6** (`cgroup.kill` errno in a threaded cgroup): the kernel document D5 already cites states it — **`EOPNOTSUPP`** (= `ENOTSUP`, 95). Close it; drop the scheduled experiment.
- **D5 §23 item 1, bullets 1–2** (`EINVAL` for `chown`/`setuid` to an unmapped uid): decidable from kernel source, CONFIRMED.

### The V6 adjudications that drive Part 2

1. **`FsManager` before `SystemdManager`, settled by source.** runc v1.2.5 `UnifiedManager.Set` ends with `return m.fsMgr.Set(r)` **after** the D-Bus call — verbatim. So **the fs driver is a *component* of the systemd driver, not an alternative**, and the build order is forced: `FsManager` first, `SystemdManager { dbus, fs: FsManager }` second (V6 §D.1). Also confirmed: `cgroupFilesToChown()` with its three-file fallback — and D5's `[R]` "copy this exactly; hardcoding the three-file list is a live bug on ≥ 4.15" is right, because **the host publishes five files** in `/sys/kernel/cgroup/delegate`.
2. **The cgroup path shape `mydocker/<id>/` is forced, not chosen** (V6 §C.3). The no-internal-process rule permits only that shape given (a) we need controllers usable on `<id>`, so `mydocker` must distribute, and (b) a distributing non-root cgroup may hold no processes. D5's careful reading is right, including the paragraph most summaries drop: *"the restriction doesn't get in the way if there is no enabled controller in the cgroup's `subtree_control`"*. And **`container_cgroup(id).join("init")` from day one is nearly free** — with `CLONE_INTO_CGROUP` the child is *born* in the target cgroup, so adopting the `init/` sub-path costs one extra `mkdir` and a different fd, with no runtime cost. Without it, adding a per-container `exec` sub-cgroup later requires a **live migration of a running container's PID 1**.
3. **5.10 floor vs `lowerdir+` — adjudicated as a probe-gated fast path** (V6 §I.3, §N.8). `lowerdir+` first appears at **v6.7** (absent v6.6); the kernel doc says 6.8. Either way **flatly incompatible with a 5.10 floor as a *requirement***. Note `fsopen`/`fsconfig` themselves are **5.2** — overlayfs was not converted to `fs_context` until 6.7, so on 5.10 `fsconfig` exists but **overlayfs cannot be configured through it at all**. Both recommendations survive **because D5 §6.2's own probe-don't-version-gate rule resolves it**: try `fsopen("overlay")` + `fsconfig(SET_STRING,"lowerdir+",…)`, fall back on `ENOSYS`/`EINVAL`/`ENOPARAM` to `mount(2)`. **Required edit to D5:** §6.1's table has no overlayfs/new-mount-API row and §6.2's probe list does not include it — without that row, a reader pairing §6.3 ("5.10 gives us every mechanism this document treats as load-bearing") with D3's `lowerdir+` recommendation will conclude they are compatible **without** the fallback.
4. **A security argument for the 5.10 floor that D5 does not use:** kernels **before 5.4** mis-dispatch x32 syscall numbers 512–547 (`seccomp(2)`), so 5.10 buys **correctness of the seccomp filter itself**, not just convenience — stronger than any convenience reason D5 cites (V6 §I.3, §A.4).

### V6's verdict on the two proposed deviations

**`no_new_privs` ON by default — adopt, rewrite the rationale.**
- Benefit 1 ("closes the setuid-root escape") is **OVERSTATED for the default root container**. Docker's default container is uid 0 **in the initial userns**; a setuid-root binary changes nothing (already euid 0), and a file-capability binary grants nothing new because `capabilities(7)`'s "execution of programs by root" rule already gives `P' = E' = P(bounding)` on **every** `execve` — and **NNP does not disable that rule** (it is gated on `SECBIT_NOROOT`, not NNP). The capability-axis benefit is real **only under `--user`**.
- Benefit 2 — **REAL and the strongest**: `seccomp(2)` requires `CAP_SYS_ADMIN` in the caller's userns **or** NNP; mandating NNP **deletes one branch** and lets the filter be installed last. Promote to first place.
- Benefit 3 — **unstated in D5 but real**: NNP blocks `execve`-time transitions to a *more permissive* AppArmor/SELinux profile, and **this applies to root containers too**.
- **Costs D5 misses:** (1) **file-capability binaries under `--user` break** — the modern hardening idiom (`setcap cap_net_bind_service=+ep /usr/sbin/nginx` + non-root user) goes inert, and it breaks *hardened* images specifically; the fix moves the grant from the image to *our* `--cap-add`, which is a **policy change**, not just a compatibility break (an image can no longer declare its own least-privilege needs); (2) **AppArmor profile transitions** — flagged in §12.1, absent from §7.4's cost list; if NNP and our AppArmor plan collide, the deviation is what gives; (3) **irreversibility and inheritance** — NNP cannot be unset, is inherited across `fork` and `execve`, applies to **every `mydocker exec` for the container's life**, with no per-exec override; `--no-new-privileges=false` is a create-time decision only; (4) nested rootless tooling (`newuidmap`/`newgidmap` are setuid/fcap helpers).
- **§21 item 11 (the breakage inventory) should be BLOCKING, not optional** — this is the deviation whose cost D5 has not actually measured.

**Drop `CAP_MKNOD` + `CAP_NET_RAW` — adopt both, with three changes.**
- **`CAP_MKNOD` is re-tiered from "hardening (deviation)" to REQUIRED.** It is the load-bearing compensation for skipping the device cgroup; if someone later restores it for compatibility, **the device story collapses silently**. Gate `--cap-add MKNOD` the way §7.6 gates `--cap-add SYS_ADMIN`.
- **State that `CAP_MKNOD` gates only char/block nodes.** `vfs_mknod` checks the cap only for `S_IFCHR`/`S_IFBLK`, so `mkfifo`, UNIX sockets and regular files via `mknod` all keep working. This converts a scary-looking deviation into a nearly free one. (Cost D5 misses: images that create device nodes at runtime — `/dev/fuse`, `/dev/loop*`, `/dev/net/tun`, DinD — where the only answer left is `--device`, which per the misclassification above has no kernel backstop under this design.)
- **`CAP_NET_RAW`'s stated cost (ping) is removable.** Modern `ping` (iputils ≥ 20200821, busybox) prefers `socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)`, which needs **no capability** — only membership of a gid inside `net.ipv4.ping_group_range`, a **per-netns** sysctl. Measured: host and all containers read `1 0` (an empty range), so today the cost is real; setting `0 2147483647` in the container netns makes it **zero**. Ship the drop **with** the sysctl default.
- **The actual unmentioned costs of dropping `CAP_NET_RAW`:** `AF_PACKET` (DHCP clients `dhclient`/`udhcpc`, `tcpdump`, `nmap`, `arping`, raw-socket `traceroute`) — irrelevant for managed bridge networking, **relevant** for macvlan/ipvlan and `--net=host` containers running their own DHCP; and **`IP_TRANSPARENT` binding** (Envoy/HAProxy transparent-intercept mode), the least obvious and most likely to surface as a mystery in a service mesh; plus `SO_BINDTODEVICE` on some paths.
- **Sign-off note both deviations deserve:** they do not change the runtime's security **class**. D5's own §9.2 finding — **container root *is* host root by default** — dominates both. Only a user namespace changes the class.

## 1G. The Rust substrate (V7, cross-cutting over D1 §11, D2 §12, D3 §13, D4 §12, D5 §18)

### REFUTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "The parsing is boring, well-tested, and not where the danger lives; the **applying** is where every CVE lives" | D3 §13.3 | `[R]` | **REFUTED** — **RUSTSEC-2026-0068 / CVE-2026-33055**, same crate, same day as 0067, **parser**, and with **no `[affected.functions]` scope** at all (V7 §2.2) | Second agent, same finding (see §1I). Same pin fixes it (`tar >= 0.4.45`); the **reasoning does not survive** |
| signalfd option **(c)** — "drop signalfd: `pidfd_open` the container init and `poll` the pidfd for exit" — rated preferred | D2 §12.1.7 | `[R]` MED, **preferred** | **REFUTED — not equivalent** (V7 §5.3). Three losses, of which D2 names only the third | **(1) A functional regression, not a caveat.** With no signalfd and no handler (D2 §12.10 explicitly wants no handler), a `SIGTERM` to the supervisor takes **default disposition — the supervisor dies mid-`poll`**, without forwarding to PID 1, without writing an exit status, leaving the container orphaned. **Option (c) as written cannot implement D2 §5.5.** **(2)** A pidfd wakes only on the direct child's exit, so a subreaped orphan **produces no wake at all** and stays a zombie until init exits — "the sweep is not optional" is right, but the sweep having something to run on is the missing half. **(3)** Coalescing is a wash. **Revised recommendation: prefer (b) + pidfd** — the pidfd for exit detection, plus signalfd (or a minimal self-pipe handler) for `SIGTERM`/`SIGINT`/`SIGCHLD`. This **inverts §12.1.7's stated preference and resolves D2 §14 open question 3** |
| "`ureq` … keeps the option open of one day linking the puller into the supervisor" | D3 §13.1 (third stated reason) | `[R]` | **REFUTED** — `ureq-3.4.0/src/unversioned/resolver.rs:147` **spawns a detached OS thread per DNS resolution** whenever a `resolve` or `global` timeout is configured (V7 §8.2) | **Worse than a tokio runtime, three ways:** (1) **lazy and conditional** — the thread appears only when a timeout is set *and* a name needs resolving, so a `Threads == 1` assert before `clone()` **passes on most runs**; (2) the thread is **detached and never joined**, so on timeout the caller returns while it keeps running; (3) that thread sits **inside glibc `getaddrinfo`/NSS**, which allocates, takes locks and `dlopen`s modules — the textbook post-`fork` deadlock, reproducing **only when DNS is slow**. `ureq` is still the right client **for a separate pull process**; the third reason must be **struck**. It is exactly the discipline-invariant reasoning D2 §8.3 rejected by name (*"Structural beats disciplined"*), resurfacing in another document as a selling point |
| "runc is the **only dynamically-linked binary** in the docker→containerd→shim→runc chain" | D5 §18.2 | `[F]` | **REFUTED** — on this very host `containerd`, `dockerd` (three shared libs), `docker-proxy` and `ctr` are **all dynamic**; the **shim is the only *statically* linked binary — the exact inverse** (V7 §11) | D2 §12.11's own phrasing is careful and survives (*"the reason runc is the one dynamically-linked binary in the chain is, **at least in part**, libseccomp"*, citing only the runc/shim comparison it ran). **D5 escalated a two-binary observation into a five-binary universal and attributed it to D2.** The load-bearing inference (taking `libseccomp` means C FFI, which breaks static-musl) is untouched — it rests on runc's own `ldd` — but the sentence is the kind of wrong that gets quoted |

### CORRECTED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "Enforce it with a newtype: the child's setup functions take `&CStr` and nothing else — with that discipline **both crates are allocation-free on the paths we use**" | D2 §12.1.3 | `[R]` HIGH | **CORRECTED — necessary, not sufficient.** True of **paths only**; the child touches **three other allocation channels** (V7 §4.3) | **(1) The seccomp filter is the big one and is unaddressed anywhere in the corpus.** `seccompiler-0.5.0/src/backend/bpf.rs:9` — `pub type BpfProgram = Vec<sock_filter>`; **compiling a filter allocates a `Vec`**. D2 §8.2 names it as a hazard and neither D2 §12.11 nor D5 §18.2 resolves it. **The clean resolution:** `BpfProgramRef<'a> = &'a [sock_filter]` (`:12`), and `apply_filter` takes it; `apply_filter_with_flags` builds `sock_fprog` **on the stack** and allocates nothing → **compile before the clone, pass `&[sock_filter]`, install with `apply_filter`**. **(2) argv/envp** — an array of pointers is not a `&CStr`; pass a pre-built `&[*const c_char]`. **(3) `std::fs::write` in the child** for `uid_map`/`gid_map`/sysctls/`/proc/self/attr` — allocates, and nothing in the `&CStr` rule prevents it; **rule needed: `openat`/`write` on pre-opened fds, never `std::fs`**. **(4)** rustix functions that allocate by contract (`thread::name()` → `CString`, `fs::readlink` grows a `Vec`) |
| "Our state paths sit near rustix's 256-byte threshold — not a hypothetical" | D2 §12.1.3 | `[R]` justification | **CORRECTED — overstated ~2×.** The deepest routine path is **114 bytes**; **142 of headroom** (V7 §12.1) | **The rule is still right, for a different and better reason:** the path that can exceed 256 is the **user-supplied bundle path** (`--bundle /home/…/deeply/nested/…`), which we do not control and **cannot bound** — an argument that cannot be designed away by shortening our own directory names. Also partly settles D2 §14 Q9: youki uses `nix` (threshold **1024**) and no realistic bundle path reaches it, so **the 256-byte exposure is created by choosing rustix** — a real if small cost that §12.1.7's "what we give up" list omits |
| signalfd option **(b)** is "a 3-argument syscall, ~25 lines" | D2 §12.1.7 | `[R]` | **CORRECTED** — it also needs `libc::sigprocmask`; **rustix refuses the signal-mask surface by policy** (V7 §5.2) | Understates the cost — but (b) is still the recommended route once (c) is refuted |
| D4's bus-factor argument against `neli` | D4 §12.2 | `[C]` | **CORRECTED** — `netlink-packet-route` is **78 of 88 commits by one person** (`cathay4t`); `netlink-sys` 3 of 4. **Bus factor is one on both sides** (V7 §3.3) | Second agent, same finding (see §1I). Keep the **coverage** argument; drop the governance one |
| "`async-tar` (CVE-2025-62518)" | D3 §6.3, §13.3 | `[F]` | **CORRECTED** — 62518 is `astral-tokio-tar`/`tokio-tar`; **`async-tar` has no advisory**. CVE-2025-59825 → `astral-tokio-tar` is right (V7 §2.3) | Second agent, same finding (see §1I) |
| `rustix::thread::CapabilitySet` constants named `CAP_*` | D5 §18.3 | `[F]` | **CORRECTED** — the constants **drop the `CAP_` prefix**: `CapabilitySet::BPF`, not `CapabilitySet::CAP_BPF` (V7 §6, §12.2) | Copy-paste breakage |
| rustix's `*at` functions named `fchownat`/`fchmodat` | D3 §13.8 | `[F]` | **CORRECTED** — they are **`chownat`/`chmodat`**. `linkat`, `symlinkat`, `mknodat`, `utimensat`, `renameat`, `openat`, `openat2` are as named (V7 §12.3) | Trivial, but D3's list is meant to be copy-able |
| Line-number citations for `CapabilitySet` deprecation (`prctl.rs:~358`) and `set_secure_computing_mode` (`:163-176`) | D2 §12.3, §12.1.2 | `[F]` | **CORRECTED (drift)** — `:184` and `:159`/`:173` respectively. Both claims substantively correct (V7 §12.5) | Citation hygiene |

### OUTDATED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| `seccompiler` health: "rust-vmm/Firecracker provenance, actively maintained" | D5 §18.2 | `[C]` | **OUTDATED** — **`rust-vmm/seccompiler` was archived on 2026-08-18** into the rust-vmm monorepo. Not abandonment, but the **cited repo is read-only and 0.5.0 is 18 months old**. The counterpart `libseccomp-rs` has **0 commits in 180 days** (V7 §3.1, §3.2) | The verdict (hand-write the filter) is **unaffected and strengthened** |
| `oci-spec` attributed to "containers-org" | D3 §13.2 | `[C]` | **OUTDATED** — the repo moved to **`youki-dev/oci-spec-rs` in October 2024**; crates.io metadata says so. D2 §12.2/§12.12 has it right (V7 §8.4) | Accuracy only |

### MISCLASSIFIED

| Claim (one line) | Where | Was | Now | Consequence if we'd believed the original |
|---|---|---|---|---|
| "No crate is going to abstract `clone3` for us" | D2 §12.1 | `[F]` | **MISCLASSIFIED** — `nc 0.9.8` (which **youki uses**), the `clone3` crate and `syscalls` all expose it (V7 §1.3). The **conclusion survives** (we still write it) | The reason must be "we want the mechanism visible", not "nothing exists" |
| §12.1.7 justification #1: rustix is "the only option that **structurally** reduces the post-clone hazard" | D2 §12.1.7 | `[R]`, **load-bearing** | **UNSUPPORTED as stated** — glibc's thin syscall wrappers take no lock. The real win is **`__errno_location()` TLS under `CLONE_VM`**, which the corpus never argues (V7 §1.5) | The **decision is unaffected**, but the strongest-sounding argument for it is the weakest one, and a reviewer who checks it will discount the rest |
| "Only crate with" list (pidfds, capabilities, non-broadcasting `setresuid`, new mount API) | D2 §12.1 | `[F]` | **CONFIRMED vs `nix`; UNSUPPORTED as a universal** — `caps` has the cap sets, `nc` has the mount API, a `pidfd` crate exists (V7 §1.6) | Scope the claim to the comparison actually run |
| `pentacle` → "read, then write" on **maintenance** grounds | D2 §12 | `[R]` | **MISCLASSIFIED** — no advisory, narrow job, **used in production by youki**; "untouched" here means **finished** (V7 §13 item 34) | The verdict survives on **pedagogy**, not health. Restate the reason — otherwise we reject a crate for a defect it does not have |

### New advisory findings the corpus did not report (V7 §2.5)

| Crate | Advisory | Status |
|---|---|---|
| **`tar`** | **RUSTSEC-2026-0068** (CVE-2026-33055), **parser**, patched ≥ 0.4.45 | **Missed by D3.** Same pin fixes it; the reasoning does not survive |
| **`nftnl`** | **RUSTSEC-2025-0126** — *"Heap-buffer-overflow in `nftnl::Batch::with_page_size`"*, patched ≥ 0.9.0 | **Missed by D4**, which spends a paragraph on `nftnl`'s health, calls it *"real production code… actively released"*, counts its **194 `unsafe`** blocks — and does not mention that those blocks produced a **heap buffer overflow advisory**. D4's verdict (read, don't adopt) is **better supported than D4 knew** |
| **`daemonize`** | **RUSTSEC-2025-0069** (2025-09-14, `informational = "unmaintained"`) — **live**. RUSTSEC-2021-0147 is **withdrawn** (2023-02-19) | D2 §12.10's verdict **strengthened, by one advisory not two**. D2's *design* argument remains the better one |
| **`anyhow`** | RUSTSEC-2026-0190, unsound `Error::downcast_mut()`, patched **≥ 1.0.103** | D2 cites 1.0.104. **Write the floor into `Cargo.toml`** |
| **`tracing-subscriber`** | RUSTSEC-2025-0055, ANSI-escape log poisoning, patched **≥ 0.3.20** | **On-topic**: D2 §12.7 adopts `tracing` in the supervisor, whose log lines carry container IDs, image refs and child error strings — all attacker-influenced. D2 cites 0.3.23. **Write the floor down and do not regress it** |
| **`ring`** | RUSTSEC-2025-0007 "unmaintained" — but **`withdrawn = "2025-02-22"`**, two days after filing; RUSTSEC-2025-0010 applies only to `< 0.17` | **No finding. D3 was right.** `ring 0.17.14` is flagged by neither, and `cargo audit` does not report withdrawn advisories. D3 §13.5's `[R]` MED *"excellent crypto but slow-moving"* is **exactly correct** and must **not** be upgraded to an unmaintained verdict. Recorded because it is the trap this cluster is about: **an advisory file existing is not an advisory standing** |

**Clean — no advisory of any kind:** `rustix`, `libc`, `clap`, `serde`, `serde_json`, `thiserror`, `oci-spec`, `ureq`, `reqwest`, `flate2`, `zstd`, `sha2`, `caps`, `cgroups-rs`, `libcontainer`, `libcgroups`, `liboci-cli`, `pentacle`, `prctl`, `procfs`, `signal-hook`, `seccompiler`, `libseccomp`, `aya`, `libbpf-rs`, `netlink-packet-route`, `netlink-packet-core`, `netlink-sys`, `rtnetlink`, `neli`, `nftables`, `rustables`, `iptables`, `ipnet`, `fs2`, `oci-client`, `oci-distribution`, `clone3`, `nc`, `syscalls`, `sc`, `zbus`, `rayon`, `subtle`, `miniz_oxide`. **The primary substrate is clean.**

### Cross-file conflicts V7 found

1. **UNRESOLVED CONFLICT — D4 requires a dedicated OS thread for `setns`; D2 §8.3 forbids one. Neither document noticed.** D4 states it **three times** and explicitly hands it to D2 **twice** (§2.3 line 186, §2.6(4) line 557 *"a dedicated OS thread. This is an architectural consequence, not a detail → Handoff to D2"*, §2.x line 201, §12.2). D2 §8.3: *"the container creation path must run in a process that has **never had a second thread**."* D2 §13's handoff places netns plumbing **in the supervisor** and answers only the *async* half, **never addressing the thread requirement at all** — even though D4 flagged it before D2 §13 was written. **As written the design says: the supervisor must never have a second thread, and the supervisor must spawn a thread.**
   **RESOLUTION — it dissolves, and cleanly, by combining the two premises rather than compromising either** (V7 §8.1). D4's requirement is **conditional and D4 says so**: *"in a **multithreaded** Rust program you must `setns` on a dedicated thread."* D2 **guarantees the supervisor is single-threaded**. In a single-threaded process the only thread *is* the process, so `setns(netns_fd, CLONE_NEWNET)` moves the whole process and there is nothing to isolate the change from:
   `host_ns = open("/proc/self/ns/net", O_RDONLY|O_CLOEXEC)` → `setns(container_netns_fd, CLONE_NEWNET)` → `socket(AF_NETLINK, …)` (now bound to the container netns) → netlink work → `setns(host_ns, CLONE_NEWNET)`.
   **No thread.** The socket stays bound to the netns it was created in — the property D4 §2.6(4) wanted the thread for — and it **outlives the `setns` back**. **Caveats that must be written down:** the restore must happen on **every** exit path including the error path (a `Drop` guard, plus `panic = "abort"` in the supervisor so a panic cannot skip it), and **everything between the two `setns` calls runs in the container's netns**, so no host-netns I/O may happen there.
   **Action:** D2 §13's handoff needs a fourth bullet; D4 §2.6(4)/§12.2 need the conditional made explicit. **Until that is written down this is the highest-risk unstated assumption in the corpus**, because the naive reading — "D4 says spawn a thread, so spawn one" — silently breaks §8.3's central invariant **in the process that calls `clone()`**.
2. **CONFIRMED CONSISTENT — D5's cgroup verdict and D2 §12.8.** Both reach *"no cgroup crate; cgroupfs is `mkdir` + `std::fs::write`"* **independently and on the same grounds** (v1 support and the systemd D-Bus driver are the crates' value, and we want neither). `cgroups-rs 0.5.1` re-derived: 21 commits/180d under `kata-containers`, no advisory — so D5's *"both are healthy. Neither should be adopted"* is a **correctly separated health-vs-fit judgement**, which is the discipline this cluster most often fails (V7 §8.3). D5 §18.1's `std::fs::write`-is-one-`write(2)` claim is also correct and a real subtlety: `std::fs::write` on a small buffer issues a single `write`, while `BufWriter` may split — **and cgroup interface files are not stream files**.
3. **Three minor conflicts, all resolvable in D2's favour** (V7 §8.4): D2 §12.13 rejects `fs2`/`fd-lock`/`file-lock` outright while D4 §12.5's table lists `fs2` as "adopt or stdlib" — **D2 wins**, `rustix::fs::flock` exists and `std::fs::File::lock` is `flock(2)` on Linux; delete the row (a reader building `Cargo.toml` from D4's table adds `fs2`). D3 §13.2's `oci-spec` org is stale. D4 §12.5 defers to **D1 §11.1's** nix+rustix call, which **D2 §12 explicitly supersedes** with rustix-primary — real consequence, since D4's netlink code will use whichever crate it thinks is "the call".

### The `no_std` rustix option V7 found (recorded, not adopted)

`with_c_str_slow_path` is **feature-gated**: under `#[cfg(not(feature = "alloc"))]` rustix uses a `MaybeUninit<[u8; PATH_MAX]>` stack buffer and returns `Errno::NAMETOOLONG` for anything longer — **there is no allocation on any path**. But `default = ["std"]` and `std = [… "alloc" …]`, so the recommended feature set turns the allocating path **on**, and **Cargo unifies features across a build graph**, so a workspace whose CLI enables `rustix/std` turns `alloc` back on for the child too. The `no_std` route therefore works **only if the child is a separately built artifact** — not merely a separate module or even a separate workspace member. That is the same *shape* of answer as D2 §8.3's separate-process rule (**structural, not disciplinary**), but it costs `std` in the child, which is a large ask. **Record it as an option; do not adopt it now** — it matters that a hard guarantee exists, because the `&CStr` rule is only a soft one (V7 §4.2).

**The enforcement mechanism that would cover all four allocation channels** (V7 §4.3): the `Threads == 1` assert catches thread-spawning but not allocation. For allocation, the child's entry point can install a `#[global_allocator]` shim whose `alloc` **aborts** (or writes a byte to the error pipe and `_exit`s) once a static `IN_CHILD` flag is set. That turns every hole from a nondeterministic deadlock into a **deterministic, loud failure at the exact call site**. ~20 lines, and it is the only mechanism that does not depend on auditing every transitive call. **D2 §14 Q13 already proposes exactly this shim** as a one-off settling experiment; the extension is to make it a **permanent guard** and to cover the seccomp `Vec` case Q13 does not name.

---

## 1H. Claims UPGRADED by verification — our most reliable foundations

These are claims verification made **stronger** than the D files left them: LOW→HIGH, inferred→measured,
single-source→two-kernel, `[O]`→`[F]`, or an open question closed. **Build on these first.**

### Promoted from single-host `[O]` to `[F]` / two-kernel

| Claim | Was | Now | Why it is stronger |
|---|---|---|---|
| Which filesystems a userns-root can mount, and which namespace each requires (`proc`→pidns, `sysfs`→netns, `mqueue`→ipcns, `devpts`/`tmpfs` always) | `[O]` ROCKY 4.18 only, host unreachable | **`[F]`, two kernels, two distros** — reproduced identically on HOST-A 6.11.0-29, seven major versions newer (V1 §D9) | And **procfs→pidns is a documented kernel rule**: `user_namespaces(7)` — *"Holding `CAP_SYS_ADMIN` within the user namespace that owns a process's PID namespace allows (since Linux 3.8) that process to mount `/proc` filesystems"*. **sysfs→netns is a kernel rule too**, in source rather than the man page: `sysfs_init_fs_context()` → `kobj_ns_current_may_mount(KOBJ_NS_TYPE_NET)` → `net_ns_current_may_mount()` → `ns_capable(current->nsproxy->net_ns->user_ns, CAP_SYS_ADMIN)`. **The rule is stated exactly backwards from how one would guess:** it is not "sysfs needs a netns", it is "**sysfs needs your netns to be *yours***" |
| A fresh netns **inherits** `ip_forward` from `init_net`'s live values, not the documented default 0 | mechanism `[LOW]` | **`[F]` HIGH** — `devinet_init_net()` → `net_inherit_devconf()`, governed by **`net.core.devconf_inherit_init_net`** (V5 §H1) | Confirmed on a **second host and a different kernel**. The documented "default 0" describes a freshly booted `init_net`, **not a fresh child netns** |
| Widen `ping_group_range` inside the container netns | `[R]` from libslirp source + unreachable host | **Confirmed in Docker's production behaviour** — host `1 0`, every container netns `0 2147483647` (V5 §G1) | Independent confirmation on a different host and kernel |
| A container init **does** reparent to a host-side subreaper when its creator exits | inferred from the shim↔runc↔init relationship | **Demonstrated directly**, measured on 6.11.0-29 (V2 §5.4) | The half of D2 §5.4's claim that survives is now measured rather than inferred |

### Open questions closed

| Question | Was | Closed to |
|---|---|---|
| Kernel max overlay stacking depth (D3 §15.5) | "verified 500 … did not find the ceiling" | **Exactly 500** — `#define OVL_MAX_STACK 500`, `fs/overlayfs/params.h`, enforced in `ovl_parse_layer()`, present unchanged since ≤ v5.10 (V4-1.1a). D3's "500 verified to work" **landed on the constant itself** |
| The silent-truncation anomaly (D3 §15.2) | "could not reproduce", `[O]` LOW, declined | **Reproduces on demand**, deterministically per store layout: **6 of 34 docker-shaped layouts** mount with layers missing (V4-2.3) |
| `devpts newinstance` no-op — the exact commit (D1 §13 Q3) | `[F]` MED, "I did not read the commit" | **SETTLED** — *"devpts: Make each mount of devpts an independent filesystem."*, Eric W. Biederman, 2016-06-05, reachable from **v4.7**, with `CONFIG_DEVPTS_MULTIPLE_INSTANCES`. **Upgrade §6.3's `[F]` MED to `[F]` HIGH** (V1 §D11) |
| Forced-vs-generated signal mechanism (D1 §13 Q4) | `[F]` MED, inferred from behaviour | **SETTLED — and D1's guess was wrong.** It is `force_sig_info_to_task()` clearing `SIGNAL_UNKILLABLE` outright, not `sig_task_ignored()`'s `force` logic. **Should be `[F]` HIGH** (V1 §E2) |
| `OPEN_TREE_NAMESPACE`'s first kernel version (D1 §13 Q2) | `[F]` LOW | **SETTLED** — `OPEN_TREE_NAMESPACE = 2`, submitted by Christian Brauner 2025-12-29 (LWN 1052262), landing in **Linux 7.0** — i.e. **newer than every kernel this project has access to**. crun 1.29.1 ships it behind a runtime probe with a local `#define`, which is the pattern to copy (V1 §G1) |
| `cgroup.kill` errno in a threaded cgroup (D5 §23 item 2 / §21 item 6) | `[F]`, errno not found in source | **CLOSED with no experiment** — the kernel document D5 already cites states it: **`EOPNOTSUPP`** (= `ENOTSUP`, 95). Drop the scheduled experiment (V6 §C.5) |
| `EINVAL` for `chown`/`setuid` to an unmapped uid (D5 §23 item 1) | assumed to need `old-host` | **Decidable from kernel source** — `fs/open.c:chown_common()`, `kernel/sys.c:__sys_setuid()` — and **CONFIRMED**. Promote §9.3 to `[F]` HIGH on a source citation (V6 §L.1) |
| `sun_path` length for the attach socket (D2 §14 Q7) | "unsettled" | **SETTLED by arithmetic**, and **against** §10.3's stated reason: 105 + NUL = 106 ≤ 108. It fits (V2 §10.3) |
| Why youki / `oci-spec-rs` left the `containers` org (D2) | `[C]` LOW, refused to speculate | **SETTLED from primary sources** — CNCF Sandbox onboarding required a separate org: youki #2962 (utam0k) + cncf/sandbox #297 (idvoretskyi, 2024-10-22). **Benign. The refusal to speculate was correct** (V7 §7) |
| Does rustix expose the new mount API at the pinned version (D3 §13.8) | `[R]` MED, open | **SETTLED — yes, at 1.1.4.** `fsopen`, `fsmount`, `fsconfig_*` (9 variants), `move_mount`, `open_tree`, plus `mount`, `mount_bind`, `mount_bind_recursive`, `mount_change`, `mount_move`, `mount_remount`, `unmount`. **No `mount_setattr`**, as stated (V7 §12.4) |
| Is async forced into the runtime core (D4 §12.1) | adjudication | **CONFIRMED, all three legs — no.** `rtnetlink` has **no synchronous constructor** and hard-depends on `netlink-proto` + `futures-channel` + `futures-util` as **non-optional** deps; `netlink-packet-route`/`-core` have **zero** async deps and `netlink-sys`'s are **all optional behind a default-empty feature set**; netavark takes exactly that route and **does not depend on `rtnetlink` at all** (V5 §B1, V7) |

### Claims whose confidence should rise, or whose reasoning got a better source

| Claim | Where | Upgrade |
|---|---|---|
| Cgroup **namespace** must come after cgroup **placement** | D1 §3.5 rule 5 | **`[C]` → `[F]`.** D1 cites only youki's comment; `cgroup_namespaces(7)` states it as **kernel behaviour**: *"When a process creates a new cgroup namespace … its current cgroups directories become the cgroup root directories of the new namespace"* (V1 §C6) |
| `cgroup.kill` semantics | D2 §5.5 | **`[F]` MED → HIGH.** `cgroup-v2.rst:1051–1064` confirms **every** element, including that the root-cgroup absence is **definitional**, not incidental — plus `EOPNOTSUPP` in a threaded cgroup, which D2 did not have (V2 §5.5) |
| The `/proc`/`/dev` propagation leak (D1 §4.2's measured leak) | D1 §4.2 | **Now confirmed by primary source as well as observation** — `mount_namespaces(7)`'s own `MS_SHARED`/`MS_PRIVATE` example reproduces it from the opposite direction (V1 §D4) |
| Failure atomicity in error paths | D2 §9 | **Upgraded from good practice to conformance** — the OCI spec sentence D2 §1.5 dropped is a **MUST**: *"generating an error MUST leave the state of the environment as if the operation were never attempted."* A `create` that fails leaving an overlay mount, cgroup, or live init behind is **non-conformant** (V2 §1.5) |
| `ufw deny 80` does not stop `-p 80:80` | D4 §4.6 | **Stronger** — **`ufw route deny 80` also fails.** There is **no ufw command at all** that closes a Docker-published port; `DOCKER-USER` is the only lever (V5 §C5) |
| The FIFO-backpressure mechanism | D2 §2.3 | **Stronger and no longer buffer-size-dependent** — because the shim also holds a **read** end of each FIFO, the FIFO can never reach zero readers, so its writes **can never `EPIPE`, only block**. That is structural, not a tunable; the 64 KiB figure is not load-bearing (V2 §2.3) |
| `D5 §18.3`'s capability count | D5 §18.3 | **CONFIRMED — 41 exactly**, counted at `rustix thread/libcap.rs:22-115`, bits from `linux_raw_sys` UAPI, complete vs `CAP_LAST_CAP = 40` on 6.11 (V7 §6) |
| `rustables` is GPL-3.0-or-later | D4 §12.3 | **CONFIRMED to the digit** (0.8.8, 2026-06-23, gitlab.com/rustwall/rustables, 47,750 downloads). V7 calls it **"best catch in the corpus"**; V5 calls it **"the cleanest fully-confirmed claim in the crate-adjacent material"** (V5 §B2, V7 §2.4) |
| Docker's default is 6 of 8 namespaces | D1 §0.1 | **CONFIRMED, re-measured** — `user` and `time` byte-identical to host PID 1; `lsns` shows a single namespace of each holding all 164 processes (V1 §E7) |
| The chain-ID formula and store keying | D3 §4.5 | **Re-derived from scratch across 4 images and 25 layers, zero misses, zero mismatches** — every `diff` file matches `diff_ids[i]`, every `parent` matches the previous chain ID (V3 §D.3) |
| The whole `exec.fifo` rendezvous mechanism | D2 §3.2 | **CONFIRMED verbatim** line by line against runc `main`, including the `0o622` + `os.Chmod` umask defence and the created-vs-running discriminator comment (V2 §3.2) |

---

## 1I. Independent convergence — two agents, same conclusion, arrived at separately

These carry the most weight in the corpus: **two verifiers working different source documents reached the
same finding without coordination.** Treat them as the highest-confidence corrections we have.

| Finding | Agent 1 | Agent 2 | Why the convergence matters |
|---|---|---|---|
| **The `&CStr` allocation rule** — post-clone, pass only `&CStr` to `nix`/`rustix`; never `&str`/`&Path`/`&OsStr`/`String`/`PathBuf`, and **enforce it in the child's function signatures so the type system forbids the allocating path** | **V1 §H4** (from D1 §11.2): found `rustix`'s `with_c_str_slow_path` → `CString::new` and `nix`'s `with_nix_path_allocating`, and wrote the strengthened rule | **V7 §4.1/§4.3** (from D2 §12.1.3): independently confirmed both thresholds at the exact cited lines — `nix` **1024** (`lib.rs:329/331/354/358`), `rustix` **256** (`path/mod.rs:9`, `arg.rs:959/997`) — and confirmed `impl Arg for &CStr` is literally `f(self)` (`arg.rs:482-488`, `:517-523`) | Two agents, two source documents, same hazard, same fix. **And they extend each other:** V1 established the rule is *necessary*; V7 established it is **not sufficient** (seccomp `Vec<sock_filter>`, argv/envp, `std::fs` in the child). The union is the rule we should ship |
| **RUSTSEC-2026-0068** (CVE-2026-33055) — tar-rs ignores PAX size headers when the base header size is nonzero; a **parser-level** entry-smuggling bug, filed the **same day** as 0067 against the **same versions** | **V4 §4.2** (reviewing D3 §13.3) | **V7 §2.2** (reviewing the same claim from the substrate side) | Both refute the same argument — *"the parsing is boring and not where the danger lives"* — with the same advisory. V4 adds that tar-rs is described in the advisory as **an outlier**: other parsers (including Go's `archive/tar`) unconditionally honour the PAX size override. **The remediation is unchanged (`tar >= 0.4.45`); the reasoning is not** |
| **The `async-tar` / CVE-2025-62518 misattribution** — CVE-2025-62518 is `astral-tokio-tar`, not `async-tar`; `async-tar` has **no RustSec advisory at all**; the family advisory is **RUSTSEC-2025-0111 against `tokio-tar`** | **V4 §4.3** | **V7 §2.3** | Same crate/CVE pairing error, found independently from two directions |
| **The bus-factor argument does not survive** — `rust-netlink` is not materially better than `neli` on maintainer concentration | **V5 §H2** (from the governance side: no governance doc, no README, no org description naming Red Hat or nmstate; two visible members) | **V7 §3.3** (from the commit-history side: `netlink-packet-route` **78 of 88 commits by `cathay4t`**; `netlink-sys` **3 of 4**) | Two completely different evidence channels, same verdict. **Keep the coverage argument; drop the governance one** |
| **`rustables` is GPL-3.0-or-later** and must not be linked | **V5 §B2** | **V7 §2.4** | Both confirmed to the digit against crates.io. Both flag it as among the best catches in the corpus |
| **The `mount(2)` 4095-byte ceiling** | **V4-2.1** (from kernel source: `copy_mount_options()` + `path_mount()`'s forcible NUL at index 4095) | **V4-6.1** independently corroborated by **moby's own guard** `if len(mountData) > pageSize-1` in `overlay2/overlay.go` | Kernel source and a shipping implementation agree on the same off-by-one. **4095, not 4096** |
| **Async is not forced into the runtime core** | **V5 §B1** (three legs: `rtnetlink` has no sync constructor; the codec crates have zero async deps; netavark takes that route) | **V7 §9.2** (transitive-runtime sweep over every recommended supervisor/child crate: **one leak, `ureq`**, and D2 §8.3's separate-process rule already contains it) | Same conclusion from dependency-graph analysis and from a transitive sweep. V5 adds the sharper framing: *"there is no conflict because the async lives entirely in `netlink-proto`, which is an `rtnetlink`-only dependency; the codec and socket layers below it were **designed** to be usable without it, and are"* — a **structural fact about the `rust-netlink` family's layering**, not a lucky accident. V5 also adds the honest caveat: **netavark never calls `clone()`**, so it witnesses the *crate* claim, not D2 §8.3's thread-purity claim |
| **`PR_SET_CHILD_SUBREAPER` immediately after `setsid()`** | **D2 §5.4** reached it as an `[R]` | **V2 §11.1** found conmon does exactly that — `setsid()` at `src/conmon.c:132`, `set_subreaper(true)` at `:138` | Independent arrival at the same ordering; conmon's comment names the same reason |
| **The kernel forbids cross-namespace reparenting** | **V2 §5.4** measured it on 6.11.0-29 and found containerd documents it two sentences after the quote D2 used | **V1 §E3** independently found `pid_namespaces(7)`'s elided qualifier, which is the same rule from the man-page side | Both land on the same conclusion: **the container's own PID 1 is the only reaper that can ever get in-container orphans** — which is why risk R13 (`--init`) exists |

---

## 1J. UNSUPPORTED — pending the unreachable host `old-host`

`old-host.example.edu` → **NXDOMAIN**, re-checked independently by V1, V2, V3, V4, V5 and V6 during Phase 3.
These claims are **not disbelieved** — they are unverifiable from here. Each row gives the exact command
that settles it.

**Wherever an equivalent measurement was possible on `host-a`, the verifiers made it there instead,
which is strictly better evidence** (a second kernel and a second distro) — see §1H.

| # | Claim | Source | Exact command |
|---|---|---|---|
| 1 | `CAP_SYS_TIME`/41-cap set after `unshare(CLONE_NEWUSER)` on RHEL 8 | D1 §1.6 | `ssh old-host "python3 -c 'import ctypes;ctypes.CDLL(\"libc.so.6\").unshare(0x10000000)' ; grep -E \"^(Uid\|CapEff)\" /proc/self/status"` |
| 2 | Which of the 13 new syscalls exist on 4.18 (the ROCKY column of D1 §2.1) | D1 §2.1 | `ssh old-host "python3 - <<'X'`<br>`import ctypes;l=ctypes.CDLL('libc.so.6',use_errno=True)`<br>`for n in (428,429,430,431,432,433,435,436,437,442,457,458):`<br>`    l.syscall(ctypes.c_long(n)); print(n, ctypes.get_errno())`<br>`X"` — read which return **38** (`ENOSYS`) |
| 3 | `mknod` → `EPERM` inside a userns | D1 §6.2 | `ssh old-host "unshare -U -m --map-root-user sh -c 'mknod /tmp/nul c 1 3; echo rc=\$?'"` |
| 4 | Time namespace availability on RHEL 8 (D1 Open Q7) | D1 §2 | `ssh old-host "ls -l /proc/self/ns/time 2>&1; unshare --time true; echo rc=\$?"` |
| 5 | Unprivileged overlay at 4.18 — **which** mechanism | D3 §15.1, D5 §6.2 | `grep -i overlay /proc/filesystems`<br>`cat /sys/module/overlay/parameters/* 2>/dev/null`<br>`unshare -Ur -m sh -c 'mkdir -p /tmp/o/{l,u,w,m}; mount -t overlay overlay -o lowerdir=/tmp/o/l,upperdir=/tmp/o/u,workdir=/tmp/o/w /tmp/o/m; echo rc=$?'`<br>`unshare -Ur -m sh -c 'mkdir -p /tmp/o2/{l,u,w,m}; mount -t overlay overlay -o userxattr,lowerdir=/tmp/o2/l,upperdir=/tmp/o2/u,workdir=/tmp/o2/w /tmp/o2/m; echo rc=$?'` |
| 6 | RHEL overlay-in-userns **backport** provenance | D5 §6.2, §23 item 6 | `rpm -q --changelog kernel \| grep -i 'overlay.*\(userns\|user namespace\)' \| head` |
| 7 | NFS-as-`upperdir` **errno** | D3 §15.1 | `mkdir -p /tmp/nl "$HOME/ovl-u" "$HOME/ovl-w" /tmp/nm`<br>`strace -f -e trace=mount mount -t overlay overlay -o lowerdir=/tmp/nl,upperdir=$HOME/ovl-u,workdir=$HOME/ovl-w /tmp/nm ; dmesg \| tail -5` |
| 8 | Rootless `mknod` EPERM / `userxattr` overlay behaviour | D3 §5.4 | `unshare -Urm sh -c 'mknod /tmp/w c 0 0; echo $?'` (expect EPERM); then the `userxattr` mount from row 5; `grep -w overlay /proc/filesystems` |
| 9 | The `EPERM` wall: veth into the **host** netns from `unshare -Urn` | D4 §8.1 | `sleep 300 & H=$!; unshare -Urn sh -c "ip link add v0 type veth peer name v1; ip link set v1 netns $H; echo exit=\$?"` |
| 10 | Full bridge+veth+nft-nat stack **inside** `unshare -Urn` | D4 §8.1 | `unshare -Urn sh -c 'ip link add br0 type bridge && ip link add v0 type veth peer name v1 && ip link set v0 master br0 && ip addr add 10.99.0.1/24 dev br0 && ip link set br0 up && nft add table ip t && nft add chain ip t c "{ type nat hook postrouting priority srcnat; }"'` |
| 11 | The `setns` double-`CAP_SYS_ADMIN` check / slirp4netns failure | D4 §1.4, §8.2 | `unshare -Urn sh -c 'ip link set lo up; sleep 8' & sleep 1; slirp4netns --configure --mtu=65520 $! tap0` |
| 12 | `mkdir /var/run/netns` fails as root-in-userns | D4 §8.1 | `unshare -Urmn sh -c 'mkdir -p /var/run/netns'` |
| 13 | Fresh-netns sysctl table | D4 §1.1 | `unshare -Urn sh -c 'for f in ipv4/ip_forward ipv4/ping_group_range ipv4/ip_unprivileged_port_start ipv4/conf/all/rp_filter; do printf "%s=" $f; cat /proc/sys/net/$f; done'` — **note: the `ip_forward` half of this is already independently confirmed on the host (§1H)** |
| 14 | `BIND_80_OK` inside `unshare -Urn` after `ip_unprivileged_port_start=80` | D4 §8.5 | as printed in D4 §8.5 |
| 15 | `newuidmap` failure text with no `/etc/subuid` entry | D5 rootless | `newuidmap $$ 0 1000 1` as the unprivileged user |
| 16 | cgroup **v1** write errno on a Slurm-owned hierarchy (`EACCES` vs `EPERM`) | D5 | `mkdir /sys/fs/cgroup/memory/test` |
| 17 | The whole Apptainer rootless stack: `APPTAINER_SUID_INSTALL=0`, absent `starter-suid`, setuid `fusermount3` vs non-setuid `newuidmap`/`newgidmap`, missing `/etc/subuid` entry, `user.max_user_namespaces`, `apptainer.conf` values, `SESSIONDIR`, `~/.apptainer` perms, `/proc/self/cgroup`, `apptainer exec --help` flags, the `--fakeroot` consequence | D2 §11.7, §11.8 | The commands D2 quotes inline; they need only a reachable host. **The Apptainer *source* claims in the same sections are all CONFIRMED** (`starter.c` is **exactly 1715 lines**, `enum goexec` at :75, `fork_ns`/`sigsetjmp` at :101-110 with the verbatim comment *"This is hack to make clone() behave like fork()"*, the weak-chroot assertion at :1666, `joinOnly` at :1397/:1545) |

**Two `old-host` items turned out not to need the host at all:**
- The `EINVAL` claims in D5 §9.3 are **decidable from kernel source** and are **CONFIRMED** (V6 §L.1).
- Three of the four load-bearing ROCKY claims in D1 (§6.1, §6.5, §6.6) were **replaced with equivalent HOST-A measurements**, which is strictly better evidence (V1 §D9).

**One item that needs a git log, not a host:** runc's devicemapper rationale for `MS_SLAVE` over `MS_PRIVATE` on `/` is not present in v1.5.1's `prepareRoot`. `git log -S devicemapper libcontainer/rootfs_linux.go` would settle whether it moved or is only in history (V1 §J).

**Two items that need a dev VM (not `old-host`, not production):** `CLONE_INTO_CGROUP` end-to-end (D1 Open Q1), and D2 §14 Q1/Q2 — whether a *compiled* binary keeps libc off rustix's syscall path, and whether the functions are reachable under our feature set. V7 closed Q2 **at module granularity** (the nine modules are gated exactly by the nine feature names D2 lists); per-function `cfg` gating still needs a compiler. **Both are ten minutes on a dev VM and both are worth doing before any `Cargo.toml` is written** (V7 §14).

---

# PART 2 — ARCHITECTURE RISK REGISTER

Each decision is classified as **exactly one** of:

- **DECIDE NOW** — cheap to decide today, expensive to change later. A recommended decision is given.
- **DEFER SAFELY** — genuinely reversible later. What keeps it reversible is stated, and so is what would
  make it irreversible.
- **EXPERIMENT FIRST** — the evidence does not yet support a decision. The specific experiment, where it
  must run, and what result decides it each way are named.

Where the corpus disagrees with itself, the entry carries an **Adjudication**. Where I am exercising
judgement rather than reporting one of the corpus's, the sentence says so.

---

## R1 — Daemon vs daemonless

**Decision:** No daemon. A per-container supervisor, per-container `flock`, state on disk, exit code on
disk. Do **not** design around a future daemon; do not forbid one either.

**Classification: DECIDE NOW.**

**Reasoning.** The shape survives every attack V2 could mount, and the strongest argument in D2 is the one
that decides it: a *shared* supervisor reintroduces the daemon's blast radius, so if you are going to have
a supervisor at all it must be per-container. The precedent is not theoretical — CRI-O runs production
Kubernetes nodes on a pile of `state.json` files, atomic-rename writes and **no database**. Reversing this
later means introducing a process that owns every container's lifetime, which touches every code path;
that is why it is DECIDE NOW rather than DEFER.

**Evidence.**
- `V2-runtime-architecture-verification.md` §"Verdict on §7's architecture recommendation": *"Not refuted. Sound in shape, over-claimed in justification."* The per-container-supervisor argument is called *"the strongest reasoning in the document"*.
- `V2` §11.2 — CRI-O's `ContainerStateToDisk` → `ioutils.NewAtomicFileWriter`, `FromDisk()` restore, **no database**, CONFIRMED at the cited lines.
- `V2` §11.3 — containerd is a daemon **because** `meta.db` is bbolt with a single-writer `flock` held as fd 3 for the daemon's life. That is the mechanism that *forces* a daemon, and we are not adopting it.
- `D2-runtime-architecture.md` §7.1, §11.1–§11.3.

**Adjudication — two of D2's supports for this do not hold, and the decision survives both.**
1. D2 §7.1 lists *"a startup/reconciliation path"* as a **cost of having a daemon**. It is not: D2 §6.4's cases C/D/E **are** that reconciliation, paid once per CLI invocation instead of once per daemon start, and D2 §11.1 says exactly this of Podman (`V2` §7.1, MISCLASSIFIED). The work is **relocated, not avoided** — the daemonless win here is smaller than D2 claims.
2. D2 §7.5's *"a daemon, later — because **nothing** in the design depends on there being no daemon"* is **UNSUPPORTED**. There is no push channel; D2 §6.6 concedes the `docker events` gap two sections earlier; a later daemon must poll N state files or add a socket to **every supervisor** (`V2` §7.5). The supportable claim, and the one to write down, is: *nothing **forbids** a daemon, and the state directory plus per-container `flock` is a reasonable substrate for one.*

---

## R2 — Whether a per-container supervisor process exists at all, and the `run` / `-d` phase order

**Decision:** **Flip the phase order.** Phase 1 is foreground `run` only: the CLI forks, `waitpid`s, and its
exit code *is* the container's. No supervisor subcommand, no handshake pipe, no durable exit file, no log
rotation, no subreaper. Phase 2 adds `-d` as **the same code path** after `setsid()` with stdio redirected
to the log file. Two processes, one code path, in both modes.

**Classification: DECIDE NOW** — because the phase order determines what gets built first, and building the
supervisor machinery first makes the foreground path a special case of the harder thing rather than the
other way round.

**Reasoning.** This is the design V2 says was **never compared**, in a section that claims to select *"the
simplest technically sound"* architecture. It is strictly less machinery than D2 §7.2 for the same feature
set, and it is what conmon does modulo conmon being a separate binary. `-d` is treated as a requirement
throughout D2 §7 while **no stated scope item requires it** — only D2 §11.8, four hundred lines away,
admits it is *"a priced choice"*.

**Evidence.**
- `V2` §"A simpler architecture that meets the stated requirements — not considered": *"`run` always supervises in the foreground; `-d` is the same code path after `setsid()` + stdio redirected to the log file… strictly less machinery than §7.2 for the same feature set… not evaluating it in the section that claims to pick 'the simplest technically sound' architecture is the gap."*
- `V2` ledger MISCLASSIFIED #21 — `-d` as an unstated requirement; D2 §7.6 phases the harder thing first.
- `D2` §11.7 items 1–2 (`[R]` HIGH, adopt): *"The container is a process, not a service… Apptainer proves the 95% interactive case needs none of it"* and *"Exit status via `wait()` — Podman's exit-file + `--exit-command` + inotify dance exists **only** because the supervisor must outlive the client."*

**Adjudication — the corpus flatly contradicts itself here and one side is selectively cited.**
D2 §7.4 says `[R]` HIGH: *"Should the CLI ever be in the container's process ancestry? **No. Never, in any
mode, including foreground.**"* D2 §11.7 says `[R]` HIGH the opposite, and calls it principled. **These
cannot both be adopted.** V2 found §7.4's evidence does not carry it: it quotes runc `docs/terminals.md`'s
foreground *drawback* (the "through no fault of the user" passage, lines 219–224, verbatim) and **omits the
recommendation two paragraphs earlier in the same file**: *"In general we recommend using foreground because
it's the most straight-forward to use… **Detached mode is difficult to get right and generally requires
having your own `stdio` management.**"* runc's own document reaches the **opposite default from §7.4 using
the same facts**.

The quoted hazard is also narrower than §7.4 makes it: it is about the foreground process *owning the
container's stdio* — kill it and the container dies of `SIGPIPE` — **not about ancestry**. A foreground CLI
that is the container's parent but whose death is survivable (log-file write end dup'd into the child,
`SIGPIPE` ignored, exit status lost but the container fine) is **not the failure the quote describes**.

**I adjudicate for §11.7.** (This is my judgement, not the corpus's: V2 stops at "one of the two has to
change, and the document should say which.") The reason is that §11.7's position is the one that survives
its own citation check, and §7.4's is the one built on a selective quotation of a source that recommends
the opposite.

---

## R3 — CLI / runtime coupling: single binary, and whether the CLI is in the container's ancestry

**Decision:** One binary with subcommands. The CLI↔runtime boundary is a **subcommand and an argv/state
contract**, never a library call — so the runtime half can later be invoked by something that is not our
CLI (including a real `runc`-shaped `--root`-plus-subcommand interop surface) without refactoring.
Following R2, the CLI **is** in the container's ancestry in foreground mode, and that is accepted
deliberately, with the survivability property written down.

**Classification: DECIDE NOW.**

**Reasoning.** The single-binary decision is confirmed on four independent grounds and is the cheapest
structural decision in the corpus to get right and the most annoying to reverse (it determines how every
internal boundary is drawn). The ancestry question follows R2 and is not separately reversible.

**Evidence.**
- `V2` §7.3 — D2's four reasons for a single binary are CONFIRMED; `/proc/self/exe` re-exec via `exeseal`, `shim-binary-path` in the bundle, and the three-way version matrix all verified on the host. `file` re-run: shim **statically linked**, `runc`/`dockerd` dynamic PIE.
- `V2` §2.3 Boundary 4 — `runc --root /run/docker/runtime-runc/moby list` and `state <id>` from an **unrelated** process return correct data for all five live containers, and `options.json`'s `binary_name` is a plain string. **The stateless-CLI-over-a-state-directory interop surface is real and worth preserving.**
- `D2` §7.3.

**Adjudication — D2 §2.3's forward reference to §7.2 is unfulfilled and must be settled explicitly.**
D2 §2.3 rates collapsing the shim→runc boundary `[R]` **MED** *"because our supervisor process **is** the
process that called `clone(CLONE_NEWPID|…)`, and that couples the supervisor's lifetime to the creation
path in a way runc's design deliberately avoids. **§7.2 resolves this with a three-process shape that keeps
the collapse but not the coupling.**"* **§7.2 does not resolve it** — its supervisor still calls `clone()`;
its `[R]` MED answers a *thread-safety* question, not the *lifetime* question §2.3 raised (`V2` §7.2).

V2 names the two honest resolutions. **I recommend the first**, and note that D2 never actually makes its
argument: *accept the coupling and say so — the create path is short, can be made panic-free
(`panic = "abort"`), and a create failure means there is no container to supervise anyway.* The
alternative — the supervisor forks a *creator* that does the clone and dies, with the container init
reparenting to the supervisor via `PR_SET_CHILD_SUBREAPER` — **is verified to work** (`V2` §5.4 measured
exactly that reparenting on 6.11.0-29), and is the shim↔runc shape. It is available later if the create
path grows. What is not acceptable is leaving §2.3's MED rating standing as though §7.2 had earned it.

One editing artifact to cut: D2 §7.3 contains an unresolved editing note inside a `[C]` HIGH claim —
*"So does Podman (`podman --module`… no: Podman re-execs itself for rootless via `podman-in-userns`)"*
(`V2` §7.3).

---

## R4 — Image store layout: keying, and what actually cannot be retrofitted

**Decision:** Key `blobs/` by **compressed digest**, `layers/` by **`diff_id`**, `snapshots/` by
**chain ID** — the recommendation stands, and it is what both incumbents do. But **re-rank what is
irreversible**: capture **`tar-split` during extraction** from day one, and record the **xattr namespace**
each layer was extracted with. Those two are the expensive-to-change decisions; the keys are not.

**Classification: DECIDE NOW** — but for `tar-split` and the xattr namespace, **not** for the keys.
(The keys are, on the corrected evidence, DEFER SAFELY; they are listed here because D3 §11.5 ranked them
as the lock-in and that ranking is wrong.)

**Reasoning.** Re-keying a content-addressed store is an **offline recompute**: every key is derivable from
data already on disk. Given the layer directories and the manifests you can recompute every `diff_id` and
every chain ID and rename directories — a one-shot local script, no re-download, no re-extraction. By
contrast, `tar-split` can only be captured **while the layer is being extracted**, and the xattr namespace
a layer's whiteouts were written in determines which mounts can ever consume it. Both retrofits mean
**re-extracting the entire store**.

**Evidence.**
- `V3-oci-registry-verification.md` §D.4: *"Re-keying a content-addressed store is **not** an expensive migration… The genuinely irreversible decision in this document is **`tar-split` capture during extraction** (§5.5/§15.6), which cannot be retrofitted without re-extracting the store. §11.5 should rank that first and demote the storage keys."* Repeated in V3 §L's headline judgments.
- `V3` §D.3 — the chain-ID formula reproduced **byte-exactly across 25 layers in 4 images**, zero misses; every `layerdb/sha256/<chainID>/` record carries `cache-id`, `diff`, `parent`, `size`, `tar-split.json.gz`.
- `V4-storage-overlay-verification.md` §6.5: *"A fourth [lock-in] belongs there… **the xattr namespace a layer was extracted with**. A layer whose whiteouts/opaque markers are `trusted.overlay.*` cannot be used under a `userxattr` mount and vice versa; switching later means re-extracting the entire store, exactly like `tar-split`."*
- `D3-images-storage.md` §4.5, §5.5, §7.4, §11.5, §15.6.

**Adjudication — D3 contradicts itself on keying under a user namespace, and V4 flags it unreconciled.**
D3 §6.8 concedes that under a userns *"the store must be keyed by `chainID + idmap`, not chainID alone"*,
which **contradicts §11.5's flat keying** unless idmapped mounts (5.12+) are adopted (`V4` §6.5). Since the
rootless seam is deferred (R8), the resolution is: **key by chain ID now, and reserve a namespace-qualifier
field in the snapshot record** so the later split is a key extension rather than a re-key. That keeps the
cheap-to-change property genuinely cheap.

**Extraction-order invariants that must be fixed at the same time** (all three are extraction-time-only,
i.e. the same irreversibility class as `tar-split`):
- **`fchownat` → xattrs → `fchmodat`**, in exactly that order. D3 §6.14 steps 8→9→10 have it right; D3 §6.6's *explanation* is REFUTED (`chown` clears setuid **as root too**, since Linux 2.2.13) — **fix the prose, keep the code** (`V3` §J).
- Reversing it also **destroys every `security.capability` xattr**, not just setuid bits, silently, with the file mode looking untouched apart from the missing bit (`V3` §J.1).
- **Opaque whiteouts must be collected and applied before other entries under that directory** — `layer.md`: *"`a/.wh..wh..opq` is applied first… **regardless of the ordering in which the whiteout file was encountered**"*. A naive single-pass streaming extractor is **wrong** here (`V3` §I.1, all twelve quotes verbatim).
- If xattr-form whiteouts are ever written, the parent merge directory needs **`trusted.overlay.opaque = "x"`** (not `"y"`) or the kernel may not honour them (`V3` §I.2).

---

## R5 — `mount(2)` vs `fsconfig("lowerdir+")`, and what the fallback must do to be safe

**Decision:** `lowerdir+` as a **probe-gated fast path**; `mount(2)` as the fallback — and the fallback
carries a **mandatory hard refusal above 4,095 bytes**, with `lowerdir` written **last** in the option
string. The length check is not a nicety and not for a nicer error message: **it is the only thing standing
between the fallback and silent image corruption.**

**Classification: DECIDE NOW.**

**Reasoning.** This is the most dangerous single correction in the corpus. Over-length `mount(2)` options do
**not** fail — `copy_mount_options()` copies at most one page and never reports "too long", and
`path_mount()` forcibly NULs byte 4095. **The truncated string is then parsed as if we had written it.**
Whether you get an error at all is decided by what the chopped string happens to mean, which is a property
of **your directory naming**, not of the kernel.

**Evidence — all reproduced, not inferred.**
- `V4` §2.1 — mechanism from kernel source v6.11, boundary measured: **4090–4095 OK, 4096 FAIL**. Corroborated independently by moby's own guard `if len(mountData) > pageSize-1`.
- `V4` §2.3 — **6 of 34 (18%) docker-shaped layouts mounted successfully with layers missing.** The rule is exact: silent truncation occurs **iff the truncated tail names an existing directory that is not an ancestor of a surviving layer** — which a `<store>/<id>/diff` layout produces **by construction**, and that is the shape D3 §7.4/§11.4 proposes for us. It is **deterministic per store layout**, which is precisely why D3's re-runs did not reproduce it: they changed the paths.
- `V4` §2.4 — **worse than anything D3 records:** truncation eats the *end* of the string, so `lowerdir=…,upperdir=…,workdir=…` loses `upperdir` **and** `workdir`, and overlay then *legally* mounts read-only. Measured: asked for 139 layers + upper + work, **got a read-only mount with 131 layers and `EROFS` on write.** D3 §8.5's claim that option order gives "identical results" is wrong on **failure semantics**: order decides **which options are destroyed**, and putting `lowerdir` last is strictly safer.
- `V4` §2.2 — `ELOOP` is the **minority** outcome (flat layout: ENOENT 33 / ELOOP 7; docker-shaped: ENOENT 23 / ELOOP 5 / **SILENT 6**), and it genuinely means `ovl_lookup_trap_inode()` "Caught in a trap of overlapping layers".
- `V4` §1.1 — three ceilings D3 missed: `OVL_MAX_STACK = **500**`; each `lowerdir+` value capped at **255 bytes** (`strndup_user(_value, 256)`); and **the non-append `fsconfig(SET_STRING,"lowerdir", …)` is capped at 255 too — 16× worse than `mount(2)`.** So the rule is "**use `lowerdir+`, one path per call**", **not** "use the new mount API".

**Adjudication — V6's "probe-gated fast path" verdict is right, and I would add two conditions.**
`V6-resources-security-verification.md` §I.3 / §N.8 adjudicates the kernel-floor tension by invoking D5
§6.2's own probe-don't-version-gate rule, and that is correct: try `fsopen("overlay")` +
`fsconfig(SET_STRING,"lowerdir+",…)`, fall back on `ENOSYS`/`EINVAL`/`ENOPARAM`. Two additions:

1. **V6 under-prices the fallback exactly as D3 did.** V6 says the 5.10 floor *"costs one fallback path that has a shipped reference implementation"*. It costs a fallback path **plus a mandatory length guard**, because per V4 the unguarded fallback does not fail — it silently produces a wrong filesystem. V6 did not have V4's truncation result. **The two must be read together, and V4 wins on this point.**
2. **The shipped reference implementation is cheaper than either document says.** `mountFrom` is **not a fork-exec** — moby's current `overlay2/mount.go` is `go func(){ runtime.LockOSThread(); unix.Unshare(unix.CLONE_FS); unix.Chdir(dir); unix.Mount(...) }`, no fork, no exec, no chroot (`V4` §2.6). In Rust that is a dedicated `std::thread` doing `unshare(CLONE_FS)` + `fchdir(store_fd)` + `mount`, ~15 lines. **Caveat that binds R7:** this thread must not exist in the process that calls `clone()`. Since the storage layer returns a `Mount` *description* rather than a mounted path (below), the mount happens in the child or in a process that never clones, and the conflict does not arise — **but it must be written down.**

**Two consequences that are not optional:**
- **Returning a `Mount` description rather than a mounted path is forced, not a design nicety.** Because `lowerdir+` mounts are built with `fsopen`/`fsconfig` and then `move_mount`ed into the target namespace, the storage layer **physically cannot** "return a mounted path" without mounting in the wrong namespace. The description must carry the **list** of lower layers, not a pre-joined `lowerdir=` string, or the entire `lowerdir+` benefit is thrown away at the interface (`V4` §6.6).
- **Raise the layer cap from 128 to 500, not to "no cap"** (`V4` §1.1a). And note **Docker cannot reach its own `maxDepth = 128`** on the absolute-path form (~71 layers) without falling back to `mountFrom` — the constant is solved for the fallback, not the common path (`V4` §6.2).

---

## R6 — The kernel floor, and "probe features, never version-gate"

**Decision:** Floor at **5.10**. **Probe every optional mechanism at runtime; never parse a kernel version.**
Add the missing rows to the feature table before anyone reads it: overlayfs-via-new-mount-API (`lowerdir+`,
**6.7**), `CLONE_INTO_CGROUP` (**5.7**), idmapped mounts (**5.12**), unprivileged overlay + `userxattr`
(**5.11**), `openat2 RESOLVE_*` (**5.6**), `FSCONFIG_SET_FD` for overlay layers (**6.13**).

**Classification: DECIDE NOW.**

**Reasoning.** The probe rule is the strongest recommendation in the corpus and is independently reached in
three documents. The floor number is cheap to pick and expensive to raise later (raising it retroactively
breaks users); the probe discipline makes the floor much less load-bearing than it looks, which is exactly
why both can be decided now.

**Evidence.**
- `V6` §I.2 — *"never version-gate features, probe them"* — **CONFIRMED and the strongest recommendation in the document.**
- `V1` §D1c — the same `[R]` HIGH in D1 §2.1 CONFIRMED as sound, and **independently reinforced**: `OPEN_TREE_NAMESPACE` is exactly the case where a version parse would mislead, and **crun implements precisely this probe-don't-parse pattern** with a local `#define` (`V1` §G1).
- `V1` §I2 — D2's `syscall(435,0,0)` → `EINVAL` (22) not `ENOSYS` (38) is CONFIRMED as a **valid availability inference**, and is the same discipline.
- `V6` §I.3 / §N.8 and `V4` §1.4 — the floor-vs-`lowerdir+` adjudication, above.
- `V4` §5.7 — landing versions established by presence/absence at consecutive tags: `metacopy` **4.19**, `volatile` **5.10**, `userxattr` **5.11**, overlay `FS_USERNS_MOUNT` **5.11**, `lowerdir+` **6.7** (D3 said 6.8), `FSCONFIG_SET_FD` **6.13**, `RESOLVE_IN_ROOT` **5.6**, `VFS_CAP_REVISION_3` **4.14**.

**Adjudication — two things the corpus gets wrong about its own floor.**
1. **D5 §6.1's table has no overlayfs/new-mount-API row and §6.2's probe list does not include it.** Without that row, a reader pairing D5 §6.3's *"5.10 gives us every mechanism this document treats as load-bearing"* with D3's `lowerdir+` recommendation **will conclude they are compatible** — they are, but **only through the fallback**, and that has to be written down (`V6` §I.3). Note the subtlety that makes this easy to get wrong: `fsopen`/`fsconfig` themselves are **5.2**, but **overlayfs was not converted to `fs_context` until 6.7**, so on 5.10 `fsconfig` exists and overlayfs **cannot be configured through it at all**.
2. **The best argument for 5.10 is one D5 does not use, and it is a security argument:** kernels **before 5.4** mis-dispatch x32 syscall numbers 512–547, so 5.10 buys **correctness of the seccomp filter itself** — stronger than any of the convenience reasons cited (`V6` §I.3, §A.4).

Judgement of mine, not the corpus's: **5.10 is the right floor precisely because the probe discipline makes
it cheap.** 6.7 would exclude Debian 12 (6.1), RHEL 9 (5.14), Ubuntu 22.04 (5.15) — *"most of the installed
base"* (`V4` §1.4) — for a fast path we can probe for.

---

## R7 — Async vs sync internals

**Decision:** The core is **synchronous**, and the process that calls `clone()` **has never had a second
thread**. Async, if it appears at all, lives in a **separate process** (the image pull path), never a
separate module. **No dedicated OS thread is needed for `setns`** — the conflict dissolves.

**Classification: DECIDE NOW.**

**Reasoning.** Two verifiers independently confirmed that async is not forced into the core, and a third
finding makes the sync requirement *more* urgent than D2 realised, not less.

**Evidence.**
- `V5` §B1 — all three legs CONFIRMED: `rtnetlink` has **no synchronous constructor** (five public constructors, none blocking, all returning a `netlink_proto::Connection` future) and hard-depends on `netlink-proto` + `futures-channel` + `futures-util` as **non-optional normal dependencies**, so `default-features = false` removes the *tokio socket*, not the async machinery; `netlink-packet-route` 0.33.0 and `netlink-packet-core` 0.9.0 have **zero** async deps; `netlink-sys` 0.9.0's are **all optional behind a default-empty feature set**. netavark takes exactly that route and **does not depend on `rtnetlink`**.
- `V7-rust-substrate-verification.md` §9.2 — transitive-runtime sweep over every recommended supervisor/child crate: **one leak, `ureq`**, and D2 §8.3's separate-process rule already contains it.
- `V2` §8.1 — **the correction that makes the sync rule load-bearing:** `clone(2)` is **not** subject to "exactly the same constraint" as `fork` in userspace. glibc `posix/fork.c` locks NSS, the libio list and **the malloc arenas** before cloning and resets them in the child; **a raw `clone3` runs none of it.** So the moment you choose `clone3` for `CLONE_INTO_CGROUP`, "never more than one thread" stops being belt-and-braces and becomes **the only thing standing between you and a nondeterministic hang**.
- `D2` §8.2/§8.3, `D4` §12.1, `D5` §18.1.

**Adjudication — the D2/D4 dedicated-thread conflict, and how V7 dissolved it.**
D4 states **three times**, and hands to D2 **twice**, that `setns` for a netns *"means a dedicated OS
thread. This is an architectural consequence, not a detail → Handoff to D2."* D2 §8.3 requires a process
that has **never had a second thread**, and D2 §13's handoff places netns plumbing **in the supervisor** —
answering only the *async* half and **never addressing the thread requirement at all**.

**Resolution (V7 §8.1), and it is the good kind — it falls out of combining the two premises rather than
compromising either:** D4's requirement is **conditional and D4 says so** — *"in a **multithreaded** Rust
program you must `setns` on a dedicated thread."* D2 **guarantees single-threadedness**. In a
single-threaded process the only thread *is* the process, so `setns` moves the whole process and there is
nothing to isolate the change from:

```
host_ns = open("/proc/self/ns/net", O_RDONLY|O_CLOEXEC)
setns(container_netns_fd, CLONE_NEWNET)
socket(AF_NETLINK, …)            // bound to the container netns, and outlives the restore
… netlink work …
setns(host_ns, CLONE_NEWNET)
```

**What would reopen it, and these are the things to guard:**
1. **Anything that spawns a thread in the cloning process.** Named offenders already found: `ureq` with any timeout configured spawns a **detached** thread per DNS lookup, **inside glibc NSS** (`V7` §8.2); `tracing-appender`'s `non_blocking` spawns one (`V7` §13 item 31); tokio's multi-threaded runtime spawns workers at construction (`V2` §8.2). The enforcement is the `Threads == 1` assert — but note it **passes on most runs** against the `ureq` case, because that thread is lazy and conditional.
2. **Forgetting that the restore must run on every exit path**, including panics. Requires a `Drop` guard **and** `panic = "abort"` in the supervisor.
3. **Doing host-netns I/O between the two `setns` calls** — everything in that window runs in the container's netns.
4. **A C library in the dependency graph spawning a background thread.** This — not thread cancellation — is the real reason for the rule; D2's stated first reason (*"a thread that has exited can still have left a lock held at the moment it was cancelled"*) **does not apply to Rust**, which has no thread cancellation and whose `Mutex` unlocks-and-poisons on panic (`V2` §8.1). **Lead with the unauditable-transitive-dependency argument**, which is falsification-proof, rather than the one a reader can disprove.

**Action:** D2 §13's handoff needs a fourth bullet; D4 §2.6(4)/§12.2 need the conditional made explicit.
Until then this is, in V7's words, *"the highest-risk unstated assumption across the corpus"*, because the
naive reading — "D4 says spawn a thread, so spawn one" — silently breaks the invariant **in the process
that calls `clone()`**.

---

## R8 — Root-required assumption and the single rootless seam

**Decision:** Build the `Privilege` struct (D5 §17.3) **now**, with only the rootful arm of each strategy
implemented and the other arm as `unimplemented!()`. Enforce in review: **`geteuid()`, `getuid()` and any
`if root` appear exactly once in the codebase, inside `Privilege::probe()`.** Bind-mount device nodes from
day one rather than `mknod`-ing them.

**Classification: DECIDE NOW** for the seam and the bind-mount-devices choice; the rootless *implementation*
is DEFER SAFELY behind that seam.

**Reasoning.** D5 identifies exactly which retrofits are structurally expensive, and all three are expensive
for the same underlying reason: a privileged operation written the cheap way is a direct syscall that
happens to work because we are root, and retrofitting means auditing **every** syscall site. The seam costs
nothing today and converts "expensive retrofit" into "the compiler lists every site the day we do it" —
*"that is the whole difference between 'deferred' and 'expensive'"* (D5 §17.3).

**Evidence.**
- `D5` §17.3 — the struct is deliberately **not** `is_root: bool`, because the interesting cases are root-but-no-writable-cgroup (container-in-container), rootless-with-full-delegation (a good systemd desktop), and rootless-without-subuid. Each has a **different answer per field**, and a boolean forces every call site to re-derive the real question from the wrong one. Each field is *"a strategy the caller obeys, not a fact the caller interprets"*, populated by **probing** (per R6), never by reading a uid or a version.
- `D5` §17.1 — cheap later: bind-mounted device nodes, idmapped mounts, `userxattr`, seccomp (identical both modes), capability sets (identical API), the cgroup-root probe.
- `D5` §17.2 — **the three structurally expensive ones**: (1) scattered "am I root?"; (2) **the network model is not a fallback, it is a different implementation** — rootful is veth+bridge+netfilter, rootless is a **userspace TCP/IP stack** (`slirp4netns`/`pasta`) with its own process, lifecycle, performance profile and no ICMP, and *"there is no `if rootless {}` branch that turns one into the other"*; (3) **the UID model** — if uid handling is spread across storage as bare `u32`s, retrofitting is a type-level change through the whole storage layer.
- `V6` §J.2 sign-off — *"the deviations are worth taking, but they do not change the security **class** of the runtime. Container root **is** host root by default (D5 §9.2). Only a user namespace changes the class."*

**Adjudication — one D5 fact quoted into the rootless handoff is CORRECTED.** D1 §1.6's *"`CAP_SYS_TIME` is
useless outside the initial user namespace"* is repeated in the D5 rootless material as a flat fact; since
Linux 5.6, `CAP_SYS_TIME` **in the userns owning the time namespace** can write `timens_offsets` (`V1`
§D2b). `CAP_SYS_MODULE` and `CAP_MKNOD` are unaffected — those two stand. Low MVP impact, but it would
mis-scope a future rootless time-namespace feature.

**What makes it irreversible:** letting a single `geteuid()` or a bare `u32` uid escape the seam. Both are
review-enforceable today and unrecoverable at scale.

**Two rootless-adjacent facts to record now, since they are cheap and change later decisions** (`V5` §G1):
per-netns `ip_unprivileged_port_start = 0` is what makes a **non-root `USER`** image bind `:80` — the
capability alone does not, and **Docker sets this in every container netns, unasked**; and
`ping_group_range = 0 2147483647` per-netns removes the entire stated cost of dropping `CAP_NET_RAW`.

---

## R9 — cgroup v1 support, cgroup path shape, and the fs-vs-systemd driver order

**Decision:** **cgroup v2 only.** Path shape `mydocker/<id>/`, with the container's own processes addressed
through a **`container_cgroup(id).join("init")`** helper from day one. Build **`FsManager` first**, then
`SystemdManager { dbus, fs: FsManager }`.

**Classification: DECIDE NOW** — all three parts.

**Reasoning.**

*v2-only* is a security argument, not just a simplicity one: **cgroup v2 has no `release_agent`**, which is
the entire mechanism of CVE-2022-0492. D5's three independent controls (never mount cgroupfs writable;
prefer v2; keep `CAP_SYS_ADMIN` out of the default set, mirrored by the seccomp `mount` denial) are correct,
and *"the claim that cgroup v2 has no `release_agent` is CONFIRMED"* (`V6` §H.2).

*The path shape is forced, not chosen.* The no-internal-process rule permits only `mydocker/` (distributing,
zero procs) over `mydocker/<id>/` (procs, empty `subtree_control`), given that we need controllers usable on
`<id>`. `V6` §C.3: *"The consequence is forced, not chosen."*

*The `init/` sub-path costs almost nothing now and a live migration later.* To turn `mydocker/<id>/` into a
distributor you must first move **every process it holds — PID 1 included — into `mydocker/<id>/init/`**;
that is a live migration of a running container's init. And with `CLONE_INTO_CGROUP` the child is **born**
in the target cgroup, so adopting the sub-path costs **one extra `mkdir` and a different fd, with no runtime
cost** (`V6` §C.4 — a point V6 adds that D5 does not make).

*The driver order is settled by source.* runc v1.2.5's `UnifiedManager.Set` ends with `return m.fsMgr.Set(r)`
**after** the D-Bus call. **The fs driver is a *component* of the systemd driver, not an alternative**
(`V6` §D.1, verbatim).

**Evidence.**
- `V6` §C.1 — the no-internal-process rule quoted verbatim from `cgroup-v2.rst` v6.11, including **the paragraph most summaries drop**: *"Note that the restriction doesn't get in the way if there is no enabled controller in the cgroup's `subtree_control`."* D5 did not drop it, and its three-bullet gloss is exactly right.
- `V6` §C.3 — live shape re-measured: `system.slice/cgroup.subtree_control` = 8 controllers with **0** lines in `cgroup.procs`; the container scope's `subtree_control` **empty**, `cgroup.type` `domain`.
- `V6` §D.1 — `cgroupFilesToChown()` confirmed verbatim including the three-file fallback; **the host publishes five files** in `/sys/kernel/cgroup/delegate`, so D5's *"copy this exactly; hardcoding the three-file list is a live bug on ≥ 4.15"* is right.
- `V6` §D.2 — `pids.max = 19147` came from systemd's `DefaultTasksMax` (15.0% of `threads-max = 127653`), **not from Docker** — CONFIRMED, re-measured on two containers.
- `V1` §F3 / §I2 — **`CLONE_INTO_CGROUP` is under-tiered.** Between `clone` and the `cgroup.procs` write the child sits in the **parent's** cgroup and can fork, spike CPU, or allocate past the memory limit. **Required for correct resource limits**, not "strongly recommended". runc pays for the missing atomicity with a synchronisation protocol: *"Do this before syncing with child so that no children can escape the cgroup."*
- `V6` §C.2 — the `EBUSY` errno is **correct but tag-inflated** (`[F]` HIGH on an admittedly-inferred value); demote to `[F]` MED until the dev-VM write confirms it.

**Adjudication — D2's non-`clone3` fallback is an unsourced invention and should be replaced.**
D2 §4 rule 2(a) offers *"child writes its own pid to `cgroup.procs` as its first act"*. It works, but
carries two unstated preconditions and **no runtime does it**: runc has the **parent** write
(`manager.Apply(p.pid())`), which is why the "before syncing with child" comment exists at all, and D2's own
step 3 shows the parent doing it. The child-write variant additionally requires the cgroupfs to be
**reachable by path** from the child — true only *before* `pivot_root` + `umount2(put_old, MNT_DETACH)`, or
via a dirfd held across the pivot (`V1` §I2). **Use the parent-write fallback**, which needs no cgroupfs
inside the container.

**One implementation rule that is easy to lose:** `std::fs::write` on a small buffer issues a **single**
`write(2)`, while a `BufWriter` may split — **and cgroup interface files are not stream files** (`V7` §8.3,
confirming D5 §18.1).

---

## R10 — Networking ownership: never write state into chains you do not own

**Decision:** **One table of our own** (`table inet mydocker`), our own netns directory, and a live-set
reconcile keyed to entities **we own** — never to rule text we would have to reconstruct. Teardown is
`delete table inet mydocker`. Nothing of ours is ever written into a built-in chain.

**Classification: DECIDE NOW.** This is the single highest-value structural decision in the networking
cluster and it cannot be retrofitted, because retrofitting means finding rules you have already lost the key
to.

**Reasoning — and the evidence is a live production leak that nobody could explain away.**
`host-a` carries an orphan bridge `br-ac0ab0eb6175` (`172.19.0.1/16`, zero ports), a surviving
`nat/POSTROUTING` MASQUERADE rule, and two `raw/PREROUTING` anti-spoof DROPs — with **no matching network in
`docker network ls`, and zero bytes matching either the network id or its subnet in boltdb**. V5 ruled out
six innocent explanations one by one (another name; swarm/overlay — wrong pool and the feature is off;
a compose artifact — no container on `172.19.x`, running or stopped; an in-flight operation — dockerd up
187 days, orphan bridge at `ifindex 26`, live bridge at `67`; a second engine — one dockerd, no podman, and
the Docker-28-specific anti-spoof rule shape no admin writes by hand; something actively broken — all three
rules read **0 packets / 0 bytes**, which is exactly what lets it survive unnoticed).

**And D4's diagnosis is CORRECTED in the direction that makes the design conclusion stronger.**
D4 §9.3 blames *"reconciliation runs at daemon start and this daemon has not restarted since March"* and
concludes *"any design whose only reconciliation point is 'when the long-lived process starts' will drift
exactly this way."* **A restart would not have cleaned this up.** `removeIPChains()` removes **eleven
Docker-owned chains** and never touches built-in `nat/POSTROUTING` or `raw/PREROUTING` — where **all three
leaked rules live** — and nothing at startup enumerates `br-*` devices looking for orphans.

So the lesson is **not** "reconcile more often". It is: *the engine wrote state into chains it does not own,
and then lost the key needed to find it again.* D4 **already holds the quote that explains this** at §9.4
rule 4, from moby — *"Without seeing config (interface names, addresses, and so on), the iptabler's cleaner
can't clean up network or port-specific rules that may have been added to iptables built-in chains"* — and
never connects it to §9.3.

**Evidence.** `V5` §A2 (the whole finding, clause by clause, with each innocent explanation ruled out);
`D4` §5.4 (*"one table of our own"*), §1.2 (our own netns directory), §9.4 rule 3, §9.7, §9.5 (CNI's `GC`
verb **is** the reconcile pass §9.7 needs — *"here is everything that should exist, delete the rest"*).

**Two ordering invariants that must be written into the unwind, not left to emerge.**

1. **Release order on IP addresses is an ordering invariant, not a severity rating.** D2 §9.1's table says *"a leaked IP from the pool is a slow exhaustion"* — true, and it is **the wrong risk to record**. The dangerous mode is **premature release**: an address returned to the pool while a DNAT rule, a conntrack entry, an ARP/neighbour entry or a published-port mapping still references it means **the next container inherits the previous container's traffic** — a cross-tenant data path, not housekeeping. A severity table indexed by "which resource leaked" **cannot express a constraint about relative ordering between two resources** (`V2` §9.1 Adjudication 1). D2 §9.3's strict reverse unwind **already produces the right order for free**; what is missing is the *statement* that the adjacency is load-bearing, so nobody later "optimises" the release to happen early — a very natural thing to do, since releasing the IP first shortens pool pressure. **Annotate `Cleanup::ReleaseIp` as order-critical.**
2. **The conntrack flush is missing everywhere in the corpus.** Deleting an nftables rule **does not tear down flows already in conntrack**. Flush conntrack for the address **between** rule removal and address release (`V2` §9.1, `V2` design gap 5).

**Adjudication — netavark's `firewall/` directory is a *replay* precedent, not a *rollback* one.**
It establishes at HIGH confidence that the **mechanism** (a small per-entity directory of JSON records plus
one `flock`, written during acquisition, read by a separate recovery command) is practical, cheap and
shipping in Rust in this exact domain — `src/firewall/state.rs`, 354 lines, `FIREWALL_LOCK_FILE`,
`fs2::FileExt`. It establishes **nothing** about the rollback direction, because **replay is naturally
idempotent** (re-adding an existing nftables rule is harmless) and **rollback is not**: `ReleaseIp(addr)`
twice hands one address to two containers; `Umount(path)` twice can unmount someone else's mount. D2's
`[R]` MED on a persisted rollback list **stays MED with the reason replaced**: not *"is this worth phase-1
complexity"* (netavark shows the cost is a few hundred lines) but *"a persisted undo log must be idempotent
or crash-marked, and D2 specified neither"*. **D2's own cheap alternative — on `rm`, best-effort try every
cleanup and report what failed — is the idempotent-by-construction version, and is right for that reason**
(`V2` §9.1 Adjudication 2).

**Two more decisions in this cluster that the corrections change:**
- **The most dangerous operation in the networking ladder is the first `-p` publish on a host running a foreign firewall manager**, not the `ip_forward` write. `ufw deny 80` does not stop it — and neither does the knowledgeable admin's `ufw route deny 80`, which lands in `ufw-user-forward` at FORWARD position 4, still after `DOCKER-FORWARD`'s terminal ACCEPT. **There is no ufw command at all that closes a Docker-published port**; `DOCKER-USER` is the only lever (`V5` §C5). D4 §4.6's recommendation — warn on first `-p` when a foreign firewall manager is detected — is therefore **more** valuable than D4 rates it.
- **Gate any address-family-bridging helper on the host actually having a global address in that family.** Docker starts it unconditionally: this host has **no global IPv6 address at all** and still runs two `[::]` `docker-proxy` processes that will never see a packet (`V5` §C3). Two idle supervised processes per published port, on the majority of hosts.

---

## R11 — Coupling OCI image handling to container execution

**Decision:** **Keep them in separate processes, not merely separate modules.** The registry client and the
layer extractor are one artifact; the runtime core is another. The interface between them is the **store on
disk** plus a `Mount` description — never a function call, and never a shared address space.

**Classification: DECIDE NOW.**

**Reasoning.** D2's own rule is the right one and it is the strongest recommendation in its section:
*"put the pull path in a separate process, not just a separate module — **structural beats disciplined**"*
(CONFIRMED, `V2` §8.2). The corpus then supplies a live demonstration of why "disciplined" fails: D3 §13.1
recommends `ureq` partly because *"it keeps the option open of one day linking the puller into the
supervisor"* — and `ureq` **spawns a detached OS thread per DNS resolution inside glibc NSS** whenever a
timeout is configured, which is lazy, conditional, and **passes a `Threads == 1` assert on most runs**
(`V7` §8.2). That is the exact discipline-invariant reasoning D2 rejected by name, **reappearing in another
document as a selling point for a crate**. That is how these invariants actually get broken.

**Evidence.** `V2` §8.2; `V7` §8.2; `D2` §8.2/§8.3; `D3` §13.1; `V4` §6.6 (the `Mount`-description
interface, forced by `lowerdir+`); `D3` §11 (the progression, whose step-4 OCI-layout import makes steps
5–8 need no network — **confirmed by V4 §8 to contain no backward dependency**).

**Adjudication.** D3's `ureq` **recommendation stands** — it is the right client for a separate pull
process, and the structural separation makes the thread harmless there. **D3's third reason must be
struck.** This also settles a subtler question the brief asks: coupling image handling to execution is not
just an aesthetic layering choice; on the corpus's own evidence it is the thing that would import a thread
into the process that calls `clone()`.

**What keeps it reversible in the *other* direction (merging later, if ever wanted):** the store-on-disk
interface. What would make it irreversible: a shared type that owns both an HTTP connection pool and a
container handle.

---

## R12 — Security defaults that are hard to reverse once users depend on them

**Decision:** Adopt **all three** — `no_new_privs` on by default, drop `CAP_MKNOD`, drop `CAP_NET_RAW` — with
the **corrected reasoning**, the corrected tiering, and one added sysctl.

**Classification: DECIDE NOW.** Defaults are the hardest thing in the register to reverse, because reversing
them is a *user-visible security regression* and adding them later is a *user-visible breakage*. There is no
cheap moment later.

### 12a — `no_new_privs` ON by default

**Corrected rationale, in priority order** (`V6` §J.1):
1. **It deletes a branch.** `seccomp(2)` requires `CAP_SYS_ADMIN` in the caller's userns **or** NNP; mandating NNP removes the `CAP_SYS_ADMIN`-shaped branch and lets the filter be installed **last**. **This is the strongest argument and D5 has it third.** (Independently confirmed: runc has **two** guarded seccomp call sites for exactly this reason — `V1` §C6 rule 11.)
2. **LSM transitions** — NNP blocks `execve`-time transitions to a *more permissive* AppArmor/SELinux profile, and **this applies to root containers too**. Unstated in D5, and real.
3. **The `--user` half of the setuid argument** — NNP stops a setuid-root binary restoring uid 0 and stops a file-capability binary granting a capability.

**The headline benefit is OVERSTATED for the default root container and must be rewritten.** Docker's
default container is uid 0 **in the initial user namespace** (re-measured: `uid_map = 0 0 4294967295`,
`ns/user → user:[4026531837]`). A setuid-**root** binary changes nothing — it is already euid 0. A
**file-capability** binary grants nothing new, because `capabilities(7)`'s "execution of programs by root"
rule already gives `P' = E' = P(bounding)` on **every** `execve` — **and NNP does not disable that rule**;
it is gated on `SECBIT_NOROOT`, not on NNP. For the default root container, NNP buys **essentially nothing
on the capability axis.**

**Costs D5 misses, all four of which belong in the deviation note:**
1. **File-capability binaries under `--user` break** — `setcap cap_net_bind_service=+ep /usr/sbin/nginx` + a non-root user is the modern hardening idiom, and NNP makes file capabilities inert. **It breaks *hardened* images specifically.** The fix (D5 §7.5's ambient sequence) moves the grant from the image to **our `--cap-add`**, which is a **policy change**: an image can no longer declare its own least-privilege needs.
2. **AppArmor profile transitions** — flagged in D5 §12.1, absent from §7.4's cost list. **If NNP and our AppArmor plan collide, the deviation is what gives.**
3. **Irreversibility and inheritance** — NNP cannot be unset, is inherited across `fork` and `execve`, and therefore applies to **every `mydocker exec` for the container's whole life**. There is no per-exec override; `--no-new-privileges=false` is a **create-time decision only**.
4. **Nested rootless tooling** — `newuidmap`/`newgidmap` are setuid/file-capability helpers, so podman-in-our-container fails. Niche, but it is what people file bugs about.

**`V6` makes D5 §21 item 11 (the breakage inventory) BLOCKING, not optional** — *"this deviation is the one
whose cost the document has actually not measured."* I concur.

### 12b — `CAP_MKNOD`: **reclassified from hardening to REQUIRED**

Because D5 §14.1 **also declines the eBPF device controller**, dropping `CAP_MKNOD` is not a deviation — it
is **the load-bearing compensation for having no device cgroup**. *"If someone later restores `CAP_MKNOD`
for compatibility, the device story collapses silently"* (`V6` §J.2). **Gate `--cap-add MKNOD` exactly the
way D5 §7.6 gates `--cap-add SYS_ADMIN`.**

**State the scope, because it makes a scary-looking deviation nearly free:** `vfs_mknod` checks the
capability **only for `S_IFCHR`/`S_IFBLK`**. `mkfifo`, UNIX sockets and regular files via `mknod` all keep
working. Measured context: the container's `/dev` is `rw,nosuid` — **not `nodev`** — and the container is in
the initial user namespace, so `CAP_MKNOD` is fully functional there and dropping it closes node creation
**completely and permanently**.

**The cost D5 misses:** images that create device nodes at runtime — `/dev/fuse` (fuse-overlayfs-in-
container), `/dev/loop*`, `/dev/net/tun` (VPN containers), LXD/Docker-in-Docker. Each is a published
workaround pattern (`--cap-add MKNOD --device /dev/fuse`), and with the eBPF controller absent the only
answer we can offer is `--device` — which under this design **has no kernel backstop** (see R15).

### 12c — `CAP_NET_RAW`: adopt, but ship it **with a sysctl** and restate the cost

**The stated cost is removable.** Modern `ping` (iputils ≥ 20200821, busybox) prefers
`socket(AF_INET, SOCK_DGRAM, IPPROTO_ICMP)`, which needs **no capability** — only membership of a gid
inside `net.ipv4.ping_group_range`, a **per-netns** sysctl. Measured: host and all five containers read
`1 0`, an empty range, so **today the cost is real**; setting `0 2147483647` in the container netns makes it
**zero**. Ship the drop as: *"drop `CAP_NET_RAW` **and** default `net.ipv4.ping_group_range` to
`0 2147483647`."* (Independently corroborated: **Docker already widens it per container** — host `1 0`,
every container netns `0 2147483647`, `V5` §G1.)

**The actual costs, which D5 does not mention:** `AF_PACKET` breaks — DHCP clients (`dhclient`, `udhcpc`),
`tcpdump`, `nmap`, `arping`, raw-socket `traceroute`; irrelevant for managed bridge networking, **relevant**
for macvlan/ipvlan and `--net=host` containers running their own DHCP. And **`IP_TRANSPARENT` binding**
(Envoy/HAProxy transparent-intercept mode) — *"the least obvious and most likely to surface as a mystery in
a service-mesh deployment"*. Plus `SO_BINDTODEVICE` on some paths.

**Sign-off note both deviations deserve** (`V6` §J.2, and I endorse it): **they do not change the runtime's
security class.** D5's own §9.2 finding — container root **is** host root by default — dominates both. Only
a user namespace changes the class, and that is R8.

### 12d — One default the corpus argues *against* Docker on, and I agree

**Deny `ptrace`/`process_vm_readv`/`process_vm_writev` by default and re-allow under `--cap-add SYS_PTRACE`.**
Docker's default profile **allows** all three unconditionally on kernel ≥ 4.8 — this was moby's deliberate
compatibility decision (Java/`gdb`/`strace` in containers), and since Linux 4.8 a tracee's seccomp filter is
not escapable via the tracer, so the historical reason to block disappeared. But **we are not carrying
moby's compatibility debt**, and the consequence of allowing them is that **there is no second layer**: a
container process can `ptrace`/`process_vm_readv` any same-uid process in the same container, defeating
in-container privilege separation; and with `--pid=host` on a host with no LSM, a root container can
`process_vm_readv` **any host root process** — reading needs no `CAP_SYS_PTRACE` when uids match (`V6`
§A.2). `gdb`-in-container is exactly what a documented `--cap-add SYS_PTRACE` is for. **This is V6's
recommendation (b), and my judgement concurs.**

---

## R13 — In-container reaping (`--init`)

**Decision:** Ship a minimal reaping PID 1 behind `--init`, and put it on the roadmap **now** rather than on
a "cheap later" list. ~30 lines. `PR_SET_CHILD_SUBREAPER` on the supervisor stays — it does a different,
real job — but it must be documented as covering **only host-level orphans**.

**Classification: DECIDE NOW** — not because the code is expensive, but because the *design currently has no
reaper anywhere*, and the reason it has none is a refuted claim. Leaving it out is a decision made on false
evidence.

**Reasoning.** D2 §5.4 claims `PR_SET_CHILD_SUBREAPER` on the supervisor means *"`mydocker exec`'d processes
whose intermediate parent died still get reaped instead of accumulating zombies **in the container**."*
**Measured on `host-a` 6.11.0-29 and REFUTED**: the orphan created inside the pidns by a `setns`ed
process (ns-local pid 5, exit status 11) was reaped by **the container's PID 1**, not by the host subreaper —
as was the plain in-namespace orphan. **The kernel forbids cross-namespace reparenting**, and containerd
documents this **two sentences after the quote D2 used**:

> *"…prevents any cross-namespace reparenting in kernel… If the PID namespace is different from shim
> process, **the container init process should cleanup any orphaned reparented processes created by setns
> process (exec operation)**."*

**What the flag actually buys, precisely:** reaping of orphans whose dying parent was at the **host**
pid-namespace level — the container init itself (the whole point, and V2 demonstrated that half directly)
and any `exec` helper we fork host-side. **It covers nothing the container's PID 1 is responsible for.**

**The consequence D2 never draws:** a container whose PID 1 is `nginx`, `bash`, or any app that does not
reap **will accumulate zombies from every `mydocker exec` orphan, and no supervisor-side flag can fix it.**
Docker's answer is `--init`; `docker-init 0.19.0` is installed on `host-a` and appears in D2's own
version block. **D2 §1–§11 never mentions `--init` or the problem it solves.**

**Evidence.** `V2` §5.4 (the measurement, the containerd quote, and the design gap); `V2` design gap 4;
`V1` §E3 — the same rule from the man-page side, plus the qualifier D1 elides: under the **`unshare`**
topology (which D1 keeps for the userns path, §10.1 step 8a) orphans can reparent **outside** the container;
under the `clone3` MVP topology the simple reading holds.

**Two constraints on the reaper's design, both from the corpus:**
- **A forwarding init cannot identify the sender:** `si_pid` is **zero** in the PID-1 handler (`pid_namespaces(7)`, `V1` §E1). Relevant the moment D1 §7.5's design-B init ships.
- **PID-1 unkillability is not durable.** `force_sig_info_to_task()` clears `SIGNAL_UNKILLABLE` outright when the target has `SIG_DFL`, so **one synchronous fault with a default disposition retracts the protection permanently for that process** (`V1` §E2). The reaper must install handlers, not rely on being unkillable. Conversely a container's own `kill(1, SIGSEGV)` stays discarded — which is why `unshare -pf sh -c 'kill -SEGV $$'` exits **0**, not 139, and that belongs in the CLI's exit-status handling.

---

## R14 — Where the pty is allocated (`-it`)

**Decision:** The supervisor creates a **`socketpair`** and passes one end to the cloned child. The **child**
allocates the pty from the **container's own `/dev/ptmx`, after `pivot_root`**, `dup`s the peer onto 0/1/2,
and sends the **master back up** over `SCM_RIGHTS`.

**Classification: DECIDE NOW.** The protocol direction (who allocates, which end travels) is baked into the
handshake and into the ordering of the create sequence; changing it later means rewriting both.

**Reasoning.** D2 §5.2 describes runc as allocating the pty in the **parent, on the host**, and D2 §7.2's
process diagram accordingly places `(optional) openpty` in the supervisor before `clone()`. **That is the
arrangement runc moved away from.** `libcontainer/init_linux.go:370–416` — `runc init` allocates the pty
**inside the container after `pivot_root`** and sends the master *up* the socket, with the comment naming
the reason:

> *"setupConsole sets up the console from inside the container, and sends the master pty fd to the
> config.Pipe (using cmsg). This is done to ensure that **consoles are scoped to a container properly (see
> runc#814** and the many issues related to that). This has to be run ***after*** we've pivoted to the new
> rootfs."*

**Why it matters concretely:** the pty must come from the *container's* `/dev/ptmx`, i.e. its own `devpts`
instance in its own mount namespace with the container's `gid=`/`mode=` options. Allocating on the host and
passing the slave in **leaks a host resource across the isolation boundary and gets the ownership/permission
model wrong** — that is runc#814 and its family.

**Evidence.** `V2` §5.2 (REFUTED, with the source); `V2` ledger REFUTED #2. `runc docs/terminals.md` is
vague here (*"sent using `SCM_RIGHTS` by `runc`"*), which is presumably where D2's reading came from — **an
instructive case where the narrative and the source differ**. The rest of D2 §5.2 (the (a)/(b)/(c) mode
taxonomy, `checkTerminal()`'s two error strings, the `TIOCSTI` host-shell injection warning, the
detached-pass-through pipe-hang note) is **CONFIRMED verbatim**.

**Dependency to record:** D1 §10.1 step 21 makes `setsid()` + `TIOCSCTTY` depend on `/dev/pts`, so for the
stated MVP goal (`mydocker run alpine /bin/sh`) `/dev/pts` is **simply required** (`V1` §F3).

---

## R15 — `--device` and the missing kernel backstop

**Decision:** **`--device` is gated on the eBPF device controller, not shipped without it.** For the MVP,
skip both: no eBPF controller **and** no `--device`. If `--device` is wanted before the controller exists,
bind-mount the node with `MS_NOSUID|MS_NOEXEC` and **refuse block devices by default** — and say plainly in
the docs that the policy is enforced in our userspace only.

**Classification: DECIDE NOW** (the gating rule), even though the eBPF controller itself is DEFER SAFELY.

**Reasoning.** D5 §14.1's *"~95% of the device cgroup's value for zero dependencies"* is an unsupported
number **hiding a categorical gap**. The substitute (drop `CAP_MKNOD`, deny `mknod`/`mknodat` in seccomp,
mount the rootfs `nodev`) covers the malicious-image and in-container-`mknod` threat **completely** — node
creation has no other syscall, and a node baked into a layer tar is inert under `nodev`. It covers
**nothing** about devices the operator passes in: **the device cgroup gates `open()`**, i.e. *access*, not
creation, and `nodev` on the *rootfs* superblock does not apply to a different mount. Under Docker a
`--device` still passes a kernel-enforced allowlist; under this proposal `--device` policy has **no kernel
backstop at all**.

**Evidence.** `V6` §G.2. Also: access modes (`--device /dev/sdb:r`, `--device-cgroup-rule`, per-device
`rwm`) become **unimplementable**; and D5 **discards the third enforcement subsystem** while §8.5 argues
*for* subsystem independence (*"the capability check and the seccomp check are enforced by different kernel
subsystems; a bug in one is covered by the other"*) — **the two remaining layers share a failure mode**, and
D5 does not notice the inconsistency. Confirmed context: **two `cgroup_device` eBPF programs per container**
on all five (runc's anonymous 552-byte program with the identical tag `ab4bc4523b7fe6b4`, plus systemd's
`sd_devices`), `BPF_F_ALLOW_MULTI` (`V6` §G.1).

**I judge the MVP skip to be correct** — the dependency cost is real and the image-side threat is fully
covered. What must change is the *claim*: restate as *"covers the malicious-image and in-container-`mknod`
threat completely; covers nothing about devices the operator explicitly passes in."*

---

## R16 — Runtime state layout, durability, `flock`, and concurrency between simultaneous invocations

**Decision:** Adopt runc's state shape (atomic `CreateTemp` + `Rename`, no database, a start-time PID-reuse
guard, a `boot_id` reboot marker) **plus four fixes the corpus found and D2 does not carry**. **One module
owns every state write, from day one** — and that module, not just the CLI, takes the lock.

**Classification: DECIDE NOW.** Three of the four fixes are one line each; the fourth (single state-writer)
is *"the cheapest expensive-thing-to-defer on the list"* in D2's own words, and all four become archaeology
once state writes are spread across processes.

**Evidence for the shape (all CONFIRMED, `V2` §6.1–§6.4).** `state.json` non-`config` fields match the live
transcript byte for byte; atomic write at `container_linux.go:882–905` — `os.CreateTemp(c.stateDir,
"state-")` → `WriteJSON` → `Close` → `Rename`, **no `fsync`, no lock file**; PID-reuse guard at `:948`
(`stat.StartTime != c.initProcessStartTime || stat.State == Zombie || Dead`); CRI-O runs production
Kubernetes on exactly this with no database; `boot_id` present and readable, and D2's reboot marker is
**strictly better than Podman's `alive`-file proxy**. Docker's unbounded container log is confirmed as a
live footgun: **69,526,164 bytes** for the container D2 measured at 27,498,753 — the same file, 24 days
later, still unrotated. **D2's "cap it from day one" is if anything understated.**

**The four fixes.**

1. **The `flock`-on-a-removable-file race.** D2 §6.6 puts the lock at `<dir>/lock`; D2 §9.4 says `delete --force` removes the state dir. A holds `LOCK_EX`; B unlinks; C re-creates and locks a **new inode** — **two holders, no contention**, because `flock` locks the inode via the open file description, not the path. **Fix:** `fstat` the held fd and `stat` the path, compare `st_dev`/`st_ino`, retry if they differ — *or* keep the lock file in a directory that is never removed (`/run/mydocker/locks/<id>`). **D2 already recommends this exact `st_dev`/`st_ino` idiom for `pivot_root` in §11.7; it just is not applied here** (`V2` §6.6a).
2. **The lock is per open-file-description and survives `fork`.** D2 §7.2 has the CLI take `LOCK_EX`, `fork/exec` the supervisor, then "release lock, exit 0". If the lock fd reaches the supervisor, **the CLI's exit does not release the lock** and every later `start`/`kill`/`rm` blocks for the container's lifetime. Rust's `File` sets `FD_CLOEXEC` by default so an `exec`ed supervisor is fine in practice — but *"auto-release on death is the whole reason to use `flock`"* needs the companion note *"and that requires exactly one holder"* (`V2` §6.6b).
3. **The supervisor never takes the lock in D2 §7.2's diagram**, while `start` does a locked read-modify-write of the same file. **A `start` racing the supervisor's `exited` write resurrects a dead container as `running`.** One added line — *"the supervisor takes the same `flock` for every state write"* — closes it, **and the diagram is what gets implemented** (`V2` §7.2).
4. **Delete `external_fds` from the state model.** D2 asserts twice at `[C]` HIGH that `external_descriptors` is how `runc exec` wires stdio, and builds on it in §6.4 and §7.5. **REFUTED:** the only consumers in runc `main` are the state serializer and `criu_linux.go:489`; runc's own comment says *"…which file descriptor to look up **at checkpoint time**"*; `runc exec` wires from the **caller's** fds via `setupIO`. **D2 defers CRIU, so the field has no consumer at all** — a free simplification the corpus argued itself into carrying (`V2` §4/§6.1).

**Concurrency, adjudicated.** runc's own TOCTOU is real and **wider than D2 describes** — `factory_linux.go`
does `os.Stat(stateDir)` at line 52, a cgroup existence-and-emptiness check at 58–88, then
`os.Mkdir(stateDir, 0o711)` at line 91: **~40 lines of window, not two adjacent calls** (`V2` §6.6). Our
per-container `flock` closes it, **provided** fixes 1–3 hold. `validateID`'s character set and
`securejoin.SecureJoin` usage are confirmed and worth copying.

**One conformance correction to the state model.** D2 §6.4's internal set
`creating|created|running|exited|unknown` is fine internally, but **`unknown` is not spec-legal as a
*reported* `status`**: the spec permits runtime-defined values only when they *"represent new runtime states
not defined above"*, and "we lost the exit code" is not a new state — **the container is `stopped`**. Report
`stopped` with a null exit code and keep `unknown` as a separate field (`V2` §1.1).

---

## R17 — Sealing `/proc/self/exe` (CVE-2019-5736)

**Decision:** **Seal the binary into a `memfd` at startup, unconditionally, like all four reference
runtimes.** Additionally, `prctl(PR_SET_DUMPABLE, 0)` on the child before `setns` on the `exec` path.

**Classification: DECIDE NOW.** D2 §11.8 says it exactly right — *"do it early; retrofitting means auditing
every exec path"* — and that is the definition of this category.

**Reasoning — the design principle D2 §7.3 states is false, and it is the one that reads like a principle.**
D2 §7.3 claims at `[R]` HIGH, calling it *"the second-biggest Rust win after the threading one"*, that
designing so we never re-exec into the container *"eliminates the entire CVE class rather than mitigating
it."* **Two independent refutations** (`V2` §7.3):
1. **The mechanism does not require a runtime re-exec.** CVE-2019-5736 works because the runtime's `/proc/self/exe` is the host binary and the *container's entrypoint* can resolve to it. In D2's own §7.2 the cloned child does `execve(user program)` after `pivot_root`; if that path resolves to `/proc/self/exe`, the child becomes our binary running as the container process, and anything inside opens `/proc/<pid>/exe`, waits for `ETXTBSY` to clear, and writes. **The exposure is created by `execve` of an attacker-controlled path, which every runtime does by definition.**
2. **All shipping runtimes disagree in code.** crun has *exactly* the architecture §7.3 recommends and **still** ships `__attribute__((constructor)) static void libcrun_rexec(void)` calling `ensure_cloned_binary()` (`linux.c:7001–7011`, with `cloned_binary.c` at 13,929 bytes). youki — Rust, no namespace re-exec — calls `pentacle::ensure_sealed()` as the **first statement of `main()`** (`crates/youki/src/main.rs:76`) with the comment *"…it also affects youki."* runc's is at `libcontainer/exeseal/`.

**Related corrections to carry:** D2 §10.1's *"crun … no self re-exec, no memfd-clone, no C constructor
trick → 100% Go artifacts"* is **REFUTED** — D2's grep was scoped to `container.c`; the mechanism lives in
`linux.c`, and **D2 §11.8 cites it correctly**, so the two sections contradict each other and §10.1 is the
wrong one. D2 §11.8's *"youki appears not to have this"* (`[C]` LOW) is **REFUTED**. And runc's
`cloned_binary.c` is **gone** — 404 on `main`; the mitigation is now Go at
`libcontainer/exeseal/cloned_binary_linux.go` (`V2` §10.1, §11.8).

**The `exec`-path gap D5 leaves open** (`V6` §H.1): D5 §11.1's design 2 (*"nothing of ours ever `execve`s
inside the container's namespaces"*) is sufficient for `run` — a fresh PID+mount namespace contains no other
process that could open `/proc/<pid>/exe` — but **not for `mydocker exec`**: the forked child that `setns`es
into an *existing* container has `/proc/self/exe` pointing at our host binary for the whole window between
`setns` and `execve`, visible in the container's own `/proc`. *"The only thing protecting the binary in that
window is `ETXTBSY` from our own running process — a property that evaporates on restart, and an attacker
can hold the loop open indefinitely."* **The two designs are complements, not alternatives.**

**Also correct the CVE description itself:** the `#!/proc/self/exe` shebang is the **bait planted inside the
container**, not what is written to the host binary; the payload is written by a **second** process; and the
CVE covers **`docker run` with a malicious image**, not only `docker exec` (`V6` §H.1).

**Recommendation for D2 §7:** delete §7.3's CVE paragraph and replace it with *"seal `/proc/self/exe` into a
memfd at startup, like all four reference runtimes."*

**Note on `pentacle`:** D2 rates it "read, then write" on **maintenance** grounds. That is
**MISCLASSIFIED** — no advisory, narrow job, in production use by youki; *"untouched" here means finished*.
The verdict survives on **pedagogy**, not health (`V7` §13 item 34). Restate the reason, or we reject a
crate for a defect it does not have.

---

## R18 — The supervisor's event loop: `signalfd` **and** pidfd, not pidfd alone

**Decision:** **Option (b) + pidfd.** The pidfd for race-free "has the container's init exited"; `signalfd`
(or a minimal `sigaction` handler writing to a self-pipe) for `SIGTERM`/`SIGINT`/`SIGCHLD`. This **inverts**
D2 §12.1.7's stated preference and resolves D2 §14 open question 3.

**Classification: DECIDE NOW.** The event loop's shape determines the supervisor's whole control flow.

**Reasoning.** D2 §12.1.7 prefers option (c) — drop signalfd, `pidfd_open` the init and `poll` it, with a
`waitpid(-1, WNOHANG)` sweep on each wake. **REFUTED as equivalent** (`V7` §5.3), with three losses of which
D2 names only the third:

1. **The supervisor loses the ability to receive its own signals — at all.** `signalfd` delivers *every* signal directed at the supervisor. D2 §5.5 **requires** the supervisor to forward `SIGTERM` to the container's PID 1, and D2 §12.10 states *"we do not want a signal handler at all."* Under (c) there is no signalfd and no handler, so a `SIGTERM` to the supervisor takes **default disposition: the supervisor dies immediately, mid-`poll`, forwarding nothing, writing no exit status, leaving the container running orphaned.** **This is a functional regression, not a caveat, and it is unmentioned in §12.1.7. Option (c) as written cannot implement §5.5.**
2. **A pidfd wakes only on the direct child's exit.** D2's mitigation assumes wakes happen; with no `SIGCHLD` path into the loop, **a subreaped orphan produces no wake at all**, so it stays a zombie until init exits and the pidfd finally fires. Severity is genuinely reduced by D2 §7.2's own design (in-container orphans reparent to the container's init, per R13), but "reduced" is not "none", and the failure is **silent zombie accumulation**.
3. **Coalescing is a wash** and D2 handles it correctly — standard signals coalesce, `signalfd` does not fix that, and the `waitpid(-1, WNOHANG)` **loop** is required either way.

**Non-child processes are not a loss:** `pidfd_open(2)` works on any thread-group leader and `POLLIN`
reports exit for non-children too; what you cannot do is `waitpid` them, and that is a `wait` limitation
which applies equally under signalfd.

**Cost correction:** option (b) is **not** "just a 3-argument syscall" — it also needs `libc::sigprocmask`,
because **rustix refuses the signal-mask surface by policy** (`V7` §5.2). Still ~25 lines, and it gives the
handler-free design D2 §12.10 explicitly wants.

**Evidence.** `V7` §5.1–§5.3; `D2` §5.5, §7.2, §12.1.7, §12.10, §14 Q3; `V2` §5.4 (runc's reap loop
confirmed verbatim — 2048-entry buffer, `SIGCHLD` registered first, `Wait4(-1, &ws, WNOHANG, &rus)` drain
loop, `SIGURG` ignored for Go's async preemption).

---

## R19 — Post-clone allocation discipline

**Decision:** **`&CStr` in the child's function signatures** (so a `&str` is a compile error, not a latent
deadlock) — **plus** three more rules the `&CStr` rule does not reach — **plus** a `#[global_allocator]`
shim in the child that **aborts** once an `IN_CHILD` flag is set.

**Classification: DECIDE NOW.** Type signatures and a global allocator are structural; retrofitting them
means re-auditing every child code path.

**Reasoning.** Two agents independently found the hazard (see §1I), and V7 found the proposed fix is
**necessary but not sufficient**. The `&CStr` rule closes the **path-argument** hole and nothing else, and
the child touches three other allocation channels:

1. **The seccomp filter — the big one, unaddressed anywhere in the corpus.** `seccompiler-0.5.0/src/backend/bpf.rs:9`: `pub type BpfProgram = Vec<sock_filter>`. **Compiling a filter allocates a `Vec`.** D2 §8.2 names "the seccomp program" as a thing the child wants to allocate, flags it as a hazard, and **neither D2 §12.11 nor D5 §18.2 ever resolves it.** **The resolution is clean and must be written down as a rule:** `BpfProgramRef<'a> = &'a [sock_filter]` (`:12`) and `apply_filter` takes it; `apply_filter_with_flags` builds `sock_fprog` **on the stack** and allocates nothing. → **Compile before the clone, pass the child a `&[sock_filter]`, install with `apply_filter`.**
2. **argv / envp** — an array of pointers is not a `&CStr`. D1 §10.1 step 7 covers the `CString`s; the signature rule does not cover the array. **Pass a pre-built `&[*const c_char]`.**
3. **`std::fs::write` in the child** — `uid_map`/`gid_map`/`setgroups`, sysctls, `/proc/self/attr` for AppArmor/SELinux. It allocates, and nothing in the `&CStr` rule prevents it. **Rule: the child does `openat`/`write` on pre-opened fds, never `std::fs`.**
4. **rustix functions that allocate by contract** regardless of argument type (`thread::name()` → `CString`, `fs::readlink` grows a `Vec`). None are needed in the child; nothing stops one being called.

**The framing must also be corrected.** D1's own `[F]` line scopes async-signal-safety to **multithreaded**
forks and then presents an absolute prohibition as a kernel/POSIX fact. **The `Threads == 1` assert and the
no-allocation rule are *alternatives, not a stack*** — and a reader who cannot reach `Threads == 1` (an
async runtime in-process) will believe the no-alloc rule saves them. **It does not**: with real threads the
*whole* async-signal-safety surface applies (`V1` §H4). Two rules survive **regardless of thread count** and
should be labelled so: `println!`/`eprintln!` — not because of the lock but because of the **inherited
unflushed buffer**, which duplicates output at the child's exit (D1 observed exactly this and attributed it
to the lock — *correct the reason, keep the rule*); and panic/unwind. **And the glibc `setxid` rule is the
most important one in §11.2** and is scoped exactly right, to `clone` rather than `fork`: glibc's `fork()`
wrapper resets its thread bookkeeping, a raw `clone()` does not, so the child inherits a thread list
describing threads that do not exist and the `setxid` broadcast waits forever. **Keep "use raw syscalls" as
an absolute.**

**Better justification than D2 gives:** our own deepest routine path is **114 bytes** with **142 bytes of
headroom** to rustix's 256 — the rule is right, but the path that can exceed 256 is the **user-supplied
bundle path**, which we cannot bound and cannot shorten (`V7` §12.1). Worth naming as a real cost of
choosing rustix over `nix` (threshold 1024), which §12.1.7's "what we give up" list omits.

**Recorded but not adopted:** rustix under `#[cfg(not(feature = "alloc"))]` **cannot allocate here at all**
(a `MaybeUninit<[u8; PATH_MAX]>` stack buffer, `Errno::NAMETOOLONG` beyond). But Cargo unifies features
across a build graph, so the `no_std` route works **only if the child is a separately built artifact** — the
same *shape* as R11's separate-process rule, but it costs `std` in the child (`V7` §4.2). Record that a hard
guarantee exists; the `&CStr` rule is only a soft one.

**Also decided by this entry:** rustix ships the **entire** capability surface alloc-free
(`capabilities()`/`set_capabilities()`, bounding-set read/drop, all four ambient ops, `get/set_keep_capabilities()`,
`no_new_privs()`/`set_no_new_privs()`, `CapabilitySets`, plus `capabilities_secure_bits()`), so D1 §11.1's
*"write ~60 lines of `capget`/`capset` ourselves"* is **obsolete** — D1 assessed only the `caps` crate. If
we still hand-write capabilities **for the stated pedagogical reason**, that is a legitimate `[R]`, **but it
must be argued on those grounds, not on a crate-inadequacy premise that is false** (`V1` §H3). The
hand-written list becomes *"~120 lines: `clone3`, `mount_setattr`, `close_range`"*.

---

## R20 — Overlay mount exclusivity (the kernel will not enforce it for us)

**Decision:** Keep our **own** mount bookkeeping — an exclusive lock or refcount per snapshot — and make
"starting the same snapshot twice returns an error, not a second mount" an acceptance criterion of the
first overlay milestone.

**Classification: DECIDE NOW.** It is cheap now and it is a silent data-corruption bug otherwise.

**Reasoning.** D3 §8.1 quotes the kernel doc as a **rule**: *"Using an upper layer path and/or a workdir
path that are already used by another overlay mount is not allowed and may fail with `EBUSY`."*
**Measured at 6.11: two overlay mounts with identical `upperdir` AND `workdir` both returned OK.** The
kernel **warns in `dmesg` and mounts anyway**; the doc's operative sentence is the *next* one — *"the
behavior of the overlay is undefined."* **Starting the same container twice will not be stopped by the
kernel; it will silently corrupt the writable layer** (`V4` §5.3).

**Three more overlay facts that belong in the same milestone** (`V4` §5.2, §5.4):
- **`workdir` "must be empty" is documented but not enforced** — a leftover file from a crashed run mounts fine. Enforce it ourselves.
- **`workdir` inside `upperdir`, or equal to it, IS `EINVAL`** (enforced). `snapshots/<id>/{fs,work}` is fine; `snapshots/<id>/fs/work` is not.
- **A read-only overlay needs ≥ 2 lower layers** (`dmesg: at least 2 lowerdir are needed while upperdir nonexistent`), and duplicating a layer gives **`ELOOP` "conflicting lowerdir path"**. So **D3 §11.4 step 6's acceptance criterion is unachievable for `alpine`** — the single-layer image D3 uses as its running example throughout. Restate step 6 as *"read-only mount of a ≥2-layer committed stack; single-layer stacks fall back to `native` or a bind mount of the diff."*

---

## R21 — Routed-first networking: split the ladder's stage 3

**Decision:** **Stage 3a — routed NAT egress**: veth (no bridge) + `ip_forward` + MASQUERADE +
`resolv.conf` rewriting. That is the first user-useful rung. **Stage 3b — bridge**, added only when
container↔container L2 adjacency, a shared gateway, or address density actually demands it.

**Classification: DECIDE NOW** (the ladder order). The bridge itself is then **DEFER SAFELY**.

**Reasoning.** D4 §11 claims the first genuinely useful rung is bridge+NAT *"because before it images cannot
`apt-get`"*. **The `apt-get` argument is right and the bridge is not what makes it work.** veth +
`ip_forward` + MASQUERADE + DNS gives complete internet egress with **no bridge at all** — that is exactly
the **CNI `ptp` plugin** (*"creates a point-to-point link between a container and the host by using a veth
device"*, with `ipMasq` and `ipMasqBackend` options) and **Docker 28's own `gateway_mode: routed`**
(*"sets no NAT/masquerading but maintains firewall rules"*). D4's *"stage 2 is a test fixture rather than a
feature"* is therefore **REFUTED**: its honest label is *"the routed model, one step short of useful"* — and
**the step is two rules, not a bridge** (`V5` §F2, §F3).

**What the split buys, and it is not small.** The bridge is where D4's own hard problems live:
`bridge-nf-call-iptables` (§3.4), hairpin (§3.5), `IFLA_BRPORT_ISOLATED` vs `--icc` (§7.2), FDB/STP (§3.1),
the MTU-inheritance-that-doesn't-happen (§2.5), and **the `br-*` device that D4 §9.1 itself classes "leaks
forever"** — the very class of the live leak in R10. A routed stage has **no class-C device at all**
(veth is class A, self-healing). And **intra-network policy becomes ordinary L3 filtering**: D4 §3.4 spends
a page on why `--icc=false` is hard because same-bridge traffic is L2-switched and invisible to netfilter
unless you load `br_netfilter` (global, with conntrack side-effects) or use `IFLA_BRPORT_ISOLATED`
(all-or-nothing per port). **On a routed setup none of that exists** — container↔container traffic is routed
through the host, traverses `filter/FORWARD`, and per-pair policy is a normal rule. D4's own conclusion that
*"anyone needing real policy should use separate networks"* is **a limitation of the bridge, not of
container networking**.

**Costs of routed, stated fairly** (they are why Docker defaults to bridge): an address or a /30 per
container instead of one shared gateway; a host route per container; container↔container pays a routing hop
instead of an L2 hop; and broadcast/multicast between containers does not work, which some legacy software
wants.

**Also settled in this entry:** `--network=host` is a **permanent orthogonal mode, not rung 0** — CONFIRMED
in substance. But it is **not "1 line"**: it also requires **short-circuiting the entire publish path**
(`-p` is meaningless in host netns, and an engine that omits `CLONE_NEWNET` and still runs its DNAT /
anti-spoof / helper path **writes nonsense rules**) and a **second `resolv.conf` branch** (the host
`127.0.0.53` stub *is* reachable here and nowhere else). And its cost column must name that **the abstract
UNIX socket namespace is per-netns** (`network_namespaces(7)`), so host mode hands the container **every
`@`-prefixed host listener** — a lateral-movement surface that "zero isolation, zero overhead" does not
convey (`V5` §F1).

**One refuted premise to stop repeating:** writing `net.ipv4.ip_forward` does **not** "reset all
configuration parameters". `inet_forward_change()` sets `all.accept_redirects = !on`, propagates
`forwarding` to `default` and every interface, and **permanently disables LRO on every netdev** — and
nothing else. A no-op write is genuinely a no-op. `rp_filter = 2` survives on this host, which it would not
if the claim were true. The **LRO** effect is the one genuine "we changed the admin's machine" consequence
and it belongs where the false claim currently sits (`V5` §A1).

---

## R22 — Registry client dispatch rules

**Decision:** Send all four modern media types in `Accept`; **branch on the response `Content-Type`, never
on what you requested**; treat the **descriptor** as authoritative over the HTTP `Content-Type` for blobs;
key any tag cache on the `Accept` set **or** resolve once and pin the digest; and **never run an
authentication handshake against a host learned from image content**.

**Classification: DECIDE NOW.** These are protocol-handling invariants; getting them wrong produces
*wrong and confusing* answers rather than graceful failures, and the cache-key one is not retrofittable
without invalidating the cache.

**Reasoning — the rule is right, but for a different reason than D3 gives.** D3 generalises from *"Docker
Hub ignores `Accept`"* (confirmed six ways, and **understated**: Hub does not content-negotiate on the
manifest endpoint **at all**, returning 200 with an OCI index even for a wholly disjoint `Accept`). But
**Quay content-negotiates and synthesises Schema 1 at serve time** — three different digests for one tag
depending on `Accept` — and **GHCR content-negotiates and answers `404`** for a Docker-only `Accept`.
**Three registries, three behaviours, one rule:** *no registry's behaviour can be predicted from the
request, so the response `Content-Type` is the only thing you can branch on* (`V3` §A, §G.1, §G.2).

**Consequences that change the client:**
- **The four-type `Accept` is load-bearing, not politeness.** An OCI-only `Accept` on Quay gets **Schema 1** for a modern 2026 image — which D3 §3.4 then tells us to hard-refuse with the message *"image was pushed before 2017; ask the publisher to re-push"*, **wrong advice**. A Docker-only `Accept` on GHCR gets **404** — "image not found" for an image that exists.
- **Neither `401` nor `404` is a reliable existence signal.** D3 §2.2 already notes 401 means private-or-absent; GHCR adds 404-means-wrong-`Accept`.
- **`Accept` can change what a tag resolves to.** Any cache keyed on `(repo, tag)` must key on the `Accept` set too, or resolve once and pin the digest. **Nothing in D3 §2 or §7 says this.**
- **CVE-2020-15157 needs two rules, and D3 implements one.** (a) strip `Authorization` on cross-host redirect — and this is a spec **`MUST NOT`**, not our recommendation with a tradeoff (D3 tags it `[R]` HIGH; **that is the dangerous direction of misclassification**, because a reader may treat a conformance obligation as a judgement call). (b) **never perform an auth handshake against a host that came from image content** (`urls`, or a redirect target) — **that is what the CVE actually is**, it leaks the registry username/password or cloud instance credentials rather than a 5-minute scoped token, and **rule (b) is stated nowhere in D3 §2.8**. Gate the token-fetch path on the host being the **configured registry for this pull** (`V3` §F.3, §E.5).
- **`HEAD` on the tag is free** — 11 HEADs, zero rate-limit decrements; GETs decrement. **Prefer HEAD; do not depend on it being free** (Hub-specific, no spec backing, measured once) (`V3` §C.5).
- **Don't filter platforms by "not `unknown/unknown`".** `busybox` has a `linux/arm/v6` with no attestation partner and `hello-world` has two `windows/amd64` — a "filter out `unknown/unknown`, take what's left" matcher **hands a Windows manifest to a Linux extractor**. D3's actual rule matches `os == "linux"` positively and is right; only the premise was over-tight (`V3` §B.2).
- **Parser robustness:** `history` is **OPTIONAL** in `config.md` and `manifest.layers` is **not marked REQUIRED** (*"For portability, `layers` SHOULD have at least one entry"*) (`V3` §H, §E.9).

---

## R23 — `MS_REC|MS_SLAVE` vs `MS_REC|MS_PRIVATE` on `/`

**Decision:** **Defer.** Implement the requirement — *`/` (and `new_root`'s parent) must not be
`MS_SHARED`* — behind one flag, and pick slave or private as a one-line configuration, not a hard-coded
constant.

**Classification: DEFER SAFELY.**

**What keeps it reversible:** the propagation mode is a single `mount()` flag argument set once, at a known
point in the create sequence (D1 §10.1 steps 11–12), with no downstream code depending on which was chosen.

**What would make it irreversible:** shipping volume-lifecycle behaviour that depends on host→container
propagation. `MS_SLAVE` keeps that channel open (which volume lifecycle wants, and which is *also* a one-way
information/behaviour channel into the container); `MS_PRIVATE` closes it. Once users depend on a
host-side mount appearing inside a running container, switching to private is a breaking change.

**Reasoning.** D1 §9.3 files `MS_REC|MS_SLAVE` as **required**. **MISCLASSIFIED**: the kernel requires only
"not `MS_SHARED`" — slave, private and unbindable all satisfy it. The evidence against "slave is required"
is four-fold and includes D1's own text: **crun defaults to `MS_REC|MS_PRIVATE`** (D1 says so itself in §4.2
and §5.3); **`pivot_root(2)`'s own EXAMPLES program uses `MS_REC | MS_PRIVATE`** with the comment *"Ensure
that 'new_root' and its parent mount don't have shared propagation"* — **the canonical man-page example
takes the option D1's table forbids**; **D1's own Open Question 8** calls it *"a genuine disagreement among
mature runtimes rather than a settled fact"*; and **D1's own stated rationale for slave is `[C]` MED and
unverified** — V1 could not find the cited devicemapper comment in runc v1.5.1's `prepareRoot` at all
(`V1` §F2, §D5).

**So the table promotes to "required" a choice its own body marks unsettled.** Correct classification:
*"`/` must not be `MS_SHARED`"* is **required**; *slave rather than private* is a **recommendation with a
real tradeoff**. Keeping the `[R]` HIGH in §4.2 is fine; the §9.3 row is not.

**What would settle it:** `git log -S devicemapper libcontainer/rootfs_linux.go` — either the rationale
moved, or it lives only in history (`V1` §J).

**One wording fix in the same table:** the row *"non-recursive `MS_PRIVATE` on the rootfs mount"* **names the
wrong mount.** The kernel precondition is on the **parent mount of `new_root`**, which is what runc's
`rootfsParentMountPrivate()` loop achieves by walking `filepath.Dir(path)` on `EINVAL` until `/`. The 12→13
ordering (make the parent private, *then* self-bind) is load-bearing and D1 has it in the right order
(`V1` §F1, §D5).

---

## R24 — OCI hook surface and spec-conformance posture

**Decision:** **Defer hooks entirely** for the MVP. But **adopt failure atomicity now**, because it is a
spec MUST rather than a quality goal, and **fix the `poststart` severity row before anyone implements it**.

**Classification: DEFER SAFELY** for hooks; the failure-atomicity half is folded into R16's single
state-writer and D2 §9.3's reverse unwind.

**What keeps hooks reversible:** they are an additive surface with a defined insertion point in the
lifecycle, and no internal structure depends on their absence.

**What would make it irreversible:** implementing `poststart` from D2's current table. The spec **changed in
v1.3.0 (released 2025-11-04)**: a failing `poststart` hook *"MUST generate an error, **stop the container**,
and continue the lifecycle at step 12"*. D2 states the old warn-and-continue behaviour **twice at `[F]`
HIGH**, and that is **non-conformant in the *unsafe* direction** — it would leave a container running that
the spec requires be stopped. Only `poststop` retains warn-and-continue. **D2 §15 already records v1.3.0 as
latest, so the body and the source list disagree** (`V2` §1.1).

**Three more spec facts to carry** (`V2` §1.5, §1.3, §1.4):
- **Failure atomicity is a MUST**, and D2 §1.5 filed it under *latitude* by quoting the error-reporting sentence and dropping the one beside it: *"generating an error MUST leave the state of the environment as if the operation were never attempted."* This **upgrades all of D2 §9 from good practice to conformance**, and D2 never invokes it.
- **`mounts` MUST be applied in the listed order** — an ordering MUST absent from D2 §4, whose entire subject is ordering. And **`root.path` may be bundle-relative** with *"a directory MUST exist at the path declared"*, which D2 §1.3's minimal-config `[R]` treats as a bare string.
- **`startContainer` is the easy one.** Its *path* MUST resolve in the **container** namespace — it is a container binary (D2's own `ldconfig` example proves it). Only **`createContainer`** is the hard host-binary-in-container-namespace case. D2's stated reason for deferring both is half true.

---

## R25 — Static linking vs the `libseccomp` FFI

**Decision:** **EXPERIMENT FIRST.**

**Classification: EXPERIMENT FIRST.**

**The experiment, and where it must run.** On a **dev VM** (not `host-a`, which is production and must
not get a toolchain; not macOS, which has no Linux target here). Two ten-minute checks, and they should
happen **before any `Cargo.toml` is written**:

1. **D2 §14 Q1 — does a *compiled* binary actually keep libc off rustix's syscall path?** Build a trivial binary using rustix's `linux_raw` backend on `x86_64-unknown-linux-musl`, then `nm`/`objdump` it for libc symbols and `strace` it to confirm the syscalls are issued directly.
2. **D2 §14 Q2 — are the functions we need reachable under our feature set?** V7 closed this **at module granularity** (the nine modules are gated by exactly the nine feature names D2 lists), but per-function `cfg` gating inside those modules still needs a compiler (`V7` §14).

**What each result decides.**
- **If the binary is libc-free and the functions are reachable:** take rustix primary, target static musl, and hand-write the seccomp filter rather than linking `libseccomp` — which keeps the whole binary static. This is the path the corpus already leans toward, and the health evidence supports it: **`rust-vmm/seccompiler` was archived 2026-08-18** into the rust-vmm monorepo (not abandonment, but a read-only repo and an 18-month-old 0.5.0), and **`libseccomp-rs` has 0 commits in 180 days** — *"verdict unaffected and strengthened"* (`V7` §3.1, §3.2).
- **If libc leaks in, or the functions are gated out:** the static-musl goal is compromised and the seccomp decision must be re-made on other grounds.

**Supporting evidence already in hand.** Measured on the host: `runc` is dynamic against `libseccomp.so.2`
and `libc.so.6`; the shim is **static**; `runc --version` reports `spec: 1.2.0` (D2 §12.2's calibration that
*"even production runtimes lag the spec by a minor version"*). **The load-bearing inference — taking
`libseccomp` means C FFI, which breaks static-musl — rests on runc's own `ldd` and is untouched.** But D5
§18.2's generalisation *"runc is the **only** dynamically-linked binary in the chain"* is **REFUTED**:
`containerd`, `dockerd` (three shared libs), `docker-proxy` and `ctr` are all dynamic; **the shim is the
only *static* one — the exact inverse.** D2 §12.11's own careful phrasing (*"at least in part"*, citing only
the runc/shim comparison it ran) survives; **D5 escalated a two-binary observation into a five-binary
universal and attributed it to D2** (`V7` §11).

---

## R26 — `CLONE_INTO_CGROUP` end-to-end, and the cgroup errnos

**Decision:** **EXPERIMENT FIRST.**

**Classification: EXPERIMENT FIRST.**

**The experiment, and where.** A **dev VM** with a writable cgroup hierarchy (D1 was right to decline this
on production, and so were all six verifiers). Three things in one sitting:

1. **`CLONE_INTO_CGROUP` end-to-end** (D1 Open Q1): create `mydocker/test/`, `open` it `O_DIRECTORY|O_RDONLY`, `clone3` with `CLONE_INTO_CGROUP` and the fd, and confirm from the parent that the child appears in `mydocker/test/cgroup.procs` **before it has run any code**.
2. **The no-internal-process errnos** (D5 §21 item 3): attempt `echo <pid> > cgroup.procs` on a cgroup with non-empty `subtree_control`, and attempt `echo +cpu > subtree_control` on a populated cgroup. Record both errnos.
3. **The `flock`-on-a-removed-file race** (R16 fix 1): reproduce the two-holders-different-inodes case and confirm the `st_dev`/`st_ino` check catches it.

**What each result decides.**
- **If `CLONE_INTO_CGROUP` works as documented on the floor kernel:** it becomes a **required** mechanism, not "strongly recommended" — because without it, between `clone` and the `cgroup.procs` write the child sits in the **parent's** cgroup and can fork, spike CPU, or allocate past the memory limit. **That is a limit that does not hold, i.e. a correctness bug, not hardening** (`V1` §F3). The `[F]` MED "requires ≥ 5.7" is CONFIRMED, and the `syscall(435,0,0)` → `EINVAL`-not-`ENOSYS` probe is a valid availability inference (`V1` §I2).
- **If it does not:** the fallback is the **parent** writing `cgroup.procs` before releasing the child from the sync pipe — runc's actual design, with its verbatim comment *"Do this before syncing with child so that no children can escape the cgroup"* — **not** D2's unsourced child-writes-its-own-pid variant (R9's adjudication).
- **If the errnos are `EBUSY` both ways:** promote D5 §1.3 from an inferred `[F]` HIGH to a measured one. **If not**, the error-handling paths change.

---

## R27 — The `no_new_privs` breakage inventory

**Decision:** **EXPERIMENT FIRST, and blocking.**

**Classification: EXPERIMENT FIRST.**

**Reasoning.** V6 explicitly reclassifies D5 §21 item 11 from optional to **blocking**: *"this deviation is
the one whose cost the document has actually not measured"* (`V6` §J.1). Every other cost in R12 was
established from man pages and source; this one requires running real images.

**The experiment, and where.** A **dev VM**, against a set of real images chosen to cover the cases the
corpus names: (a) a plain root image (`alpine`, `nginx`) — expect **no change**, since the benefit is
essentially nil for a default root container; (b) an image using `sudo`/`su`/`passwd`/`newgrp`; (c) a
**hardened** image using file capabilities plus a non-root `USER` — the `setcap cap_net_bind_service=+ep` +
`USER nginx` idiom; (d) an image running rootless tooling that shells out to `newuidmap`/`newgidmap`; (e) a
container under an AppArmor profile that performs an `execve`-time transition.

**What the results decide.**
- **If only (b) breaks:** ship NNP on by default with a documented `--no-new-privileges=false` escape, and note in the docs that it is a **create-time** decision inherited by every later `exec` for the container's life.
- **If (c) breaks widely:** the deviation costs more than D5 prices, because the fix moves the capability grant from the image to our `--cap-add` — **a policy change, not a compatibility break**: an image can no longer declare its own least-privilege needs. That may still be the right call, but it must be taken knowingly.
- **If (e) collides:** per V6, **the deviation is the thing that gives**, not the AppArmor plan.

---

## R28 — `IFLA_NET_NS_FD` inside the `VETH_INFO_PEER` nest

**Decision:** **EXPERIMENT FIRST.**

**Classification: EXPERIMENT FIRST.**

**Reasoning.** D4 §2.2 claims that placing `IFLA_NET_NS_FD` **inside** the `VETH_INFO_PEER` nest creates the
peer **directly** in the target netns, with *"Tradeoff: none that I found."* That is a **strong no-downside
claim about a non-obvious kernel path**, it is not host-checkable, and it was not verified. **The design
leans hard on it** — D4 §9.2 item 5 claims it eliminates a whole failure class (the window in which a veth
peer exists in the host netns and can leak) (`V5` §H6).

**The experiment, and where.** A **dev VM** (this needs a real kernel and a mutating netlink call, so not
`host-a`). Build the `RTM_NEWLINK` message with the nested attribute, create the pair, and check three
things: (1) does the peer appear **only** in the target netns and never in the host's `ip link` — sample the
host's link list from a concurrent process during creation; (2) what happens on a **failure path**, e.g. an
invalid netns fd or a name collision in the target — is the *first* device left behind in the host netns;
(3) does it work on the **floor kernel (5.10)**, not just current.

**What the results decide.**
- **If the peer never appears host-side and failures are clean:** adopt it, and D4 §9.2's claim stands — one whole leak class removed at no cost.
- **If either the creation is observable host-side or a failure leaves a device behind:** the "no tradeoff" claim is false, the leak class is only *narrowed*, and the cleanup path must still handle a host-side orphan veth — which changes what R10's reconcile has to look for.

D4 §2.6 already budgets round-trip tests against a real kernel; **make this one of them, and do it before
the design leans on it.**

---

# The four decisions that most constrain everything else

Ranked. Each is a decision that, once taken, silently determines what the other decisions are allowed to be.

**1. R7 — the core is synchronous and the cloning process has never had a second thread.**
Everything else in the substrate is downstream of this one invariant: it decides the crate list, the process
topology (R11's separate pull process), how netlink reaches a container netns (no dedicated thread — the
conflict dissolves), which logging is permitted, how the seccomp filter is compiled and passed (R19), and it
became *more* load-bearing once V2 established that choosing `clone3` forfeits glibc's atfork protection
entirely.

**2. R2 — foreground `run` first; `-d` is the same code path after `setsid()`.**
This decides whether half the machinery in D2 §7 exists at all — the `supervise` subcommand, the handshake
pipe, the durable exit file, the log writer with a size cap, the second state writer, the subreaper — and
the corpus contradicts itself here at `[R]` HIGH in two places, with the side that forbids it resting on a
selective quotation of a source that recommends the opposite.

**3. R8 — the `Privilege` struct as the single seam, with `geteuid()` appearing exactly once.**
Every privileged operation in every module either goes through this seam or becomes a site that a future
rootless retrofit must individually audit; D5 names this as the first of its three structurally expensive
retrofits precisely because the cheap way to write each call is the way that scatters the assumption.

**4. R10 — never write state into chains you do not own.**
This one is proven by a live production leak that a daemon restart would **not** have cleaned, because
`removeIPChains()` never touches the built-in chains where the leaked rules live — and its failure mode is
unrecoverable rather than merely untidy, since reconstructing the rule text requires config that was deleted
along with the record.

**Just below these, and irreversible in a different way:** R4's extraction-time invariants (`tar-split`
capture, the xattr namespace, and the `chown → xattr → chmod` order) and R5's mandatory 4,095-byte guard.
Neither constrains other *decisions* the way the four above do — but both are the difference between a store
we can migrate offline and a store we have to rebuild, and in R5's case between a loud failure and a
silently wrong filesystem.

---

## How to use this document in the next session

1. **Read Part 1 before touching any `D` file.** The `D` files were not edited after verification; 156 tabulated verdicts plus ~40 narrative findings changed status, and several of the most confident-sounding `[F]` HIGH claims are among them.
2. **Start from §1H (upgrades) and §1I (independent convergence).** Those are the claims verification made *stronger*, and the ones two agents reached separately. They are the safest foundations in the corpus.
3. **Treat §1J as a standing to-do.** Seventeen claims are unverifiable only because `old-host.example.edu` is NXDOMAIN; each row carries the exact command. Two of them turned out not to need the host at all, and three were replaced with better evidence on a second kernel.
4. **Part 2's DECIDE NOW entries are the ones to write down before code exists.** The EXPERIMENT FIRST entries (R25, R26, R27, R28) all run on a **dev VM** — not `host-a`, which is production, and not `old-host`, which is gone — and together they are well under an hour of machine time.

**Classification distribution — and a deliberate skew.** 22 DECIDE NOW, 2 DEFER SAFELY, 4 EXPERIMENT
FIRST. The skew is a property of the bar, not of optimism: the register admits only *"decisions that could
cause an expensive rewrite"*, and a decision that is genuinely cheap to reverse mostly fails that bar and
never gets an entry. The two DEFER SAFELY entries are here because the corpus **mis-tiered** them — R23 is
filed as `required` in D1 §9.3 when D1's own body calls it unsettled, and R24's hook deferral rests on a
severity table the spec changed under it. Read the DEFER SAFELY entries for the "what would make this
irreversible" line; that is the part worth remembering.

**Ledger scope:** 156 tabulated entries (32 REFUTED, 74 CORRECTED, 5 OUTDATED, 32 MISCLASSIFIED, 13
UNSUPPORTED) plus roughly 40 further findings carried as narrative lists — design gaps, new hazards,
omissions and cross-file conflicts that are not single-claim verdicts. Several table rows are **group
verdicts** covering many underlying claims (for example, one row covers every `old-host`-sourced observation
in D4 §8.1–§8.4/§8.6, and another covers the twelve Apptainer `[O]`s in D2 §11.7–§11.8), which is how ~196
entries account for the ~259 individual verdicts recorded across the seven `V` files.
