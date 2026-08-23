# V2 — Adversarial verification of D2 §1–§11 (Runtime architecture)

**Scope:** `D2-runtime-architecture.md` §1–§11 only. §12 (Rust substrate) belongs to another verifier.
**Method:** primary sources fetched 2026-09-05 (runc `main`, crun `main`, youki `main`, containerd `main`, OCI runtime-spec `main`, conmon `main`, netavark `main`), plus read-only empirical checks on `host-a` (`HOST-A`, Ubuntu 24.10, kernel 6.11.0-29-generic, Docker 28.3.1 / containerd 1.7.27 / runc 1.2.5 — versions re-confirmed this session).
**Stance:** falsification. Agreement is reported in one line; disagreement in detail.
**`old-host` is unreachable** (`old-host.example.edu` → NXDOMAIN). Every claim resting solely on it is marked **UNSUPPORTED — pending host**.

This file is appended to incrementally as checks complete.

---
## §1 — OCI Runtime Specification: mandate vs latitude

Checked against `opencontainers/runtime-spec` `main` (= v1.3.0 line) and against tags `v1.0.2`, `v1.1.0`, `v1.2.0`, `v1.3.0`, all fetched 2026-09-05.

### §1.1 "The spec defines exactly four status values" — **CORRECTED**

D2: `[F]` HIGH "The spec defines exactly four status values: `creating`, `created`, `running`, `stopped`", and then "`runc state` maps `paused` out to the JSON `status` field **even though the spec does not define it**. So the spec's four states are a *reporting* contract, not an implementation constraint."

Spec text (`runtime.md`, State):

> **`status`** (string, REQUIRED) is the runtime state of the container. The value **MAY** be one of: … *Additional values MAY be defined by the runtime, however, they MUST be used to represent new runtime states not defined above.*

Two corrections, and they run in opposite directions from what D2 assumed:

1. The spec does **not** define an exhaustive set. It names four and explicitly permits runtime-defined additions. runc reporting `paused` is **spec-conformant**, not an extension that violates a reporting contract.
2. Consequently D2's framing is inverted: the four states are neither a closed reporting contract nor merely internal — they are a *floor*. The real constraint is the one D2 skipped: any additional value MUST represent a state not already covered by the four.

Practical effect on the design: D2 §6.4's proposed internal set (`creating|created|running|exited|unknown`) is fine, but `unknown` is **not** spec-legal as a reported `status` in the sense D2 wants — "we lost the exit code" is not a *new runtime state*; the container is `stopped`. Report `stopped` with a null exit code, and keep `unknown` as a separate field. Cheap fix, and it matters the moment D2 §7.6 phase 3 ("be a drop-in runc") lands.

### §1.1 / §1.4 poststart failure semantics — **OUTDATED** (changed in spec v1.3.0, released 2025-11-04)

D2 states, twice and at `[F]` HIGH:
- §1.1 lifecycle listing: "` 9. poststart hooks   fail -> log warning, continue`"
- §1.4 hook table: "`poststart` … Failure → **log a warning**, the container keeps running."

That is the text of **v1.0.2 / v1.1.0 / v1.2.0**. Verified by fetching each tag:

| tag | step 9 text |
|---|---|
| v1.0.2 | "MUST **log a warning**, but the remaining hooks and lifecycle continue as if the hook had succeeded" |
| v1.1.0 | same |
| v1.2.0 | same |
| **v1.3.0 / main** | "MUST **generate an error, stop the container, and continue the lifecycle at step 12**" |

So under the current spec a failing `poststart` hook is fatal to the container, identically to `prestart`/`createRuntime`/`createContainer`. Only `poststop` (step 13) retains warn-and-continue.

This is not cosmetic. D2 §1.4's whole rationale for deferring hooks rests on a hook-failure severity table that is now wrong for one row, and any future `poststart` implementation written from D2's table would be non-conformant in the *unsafe* direction (it would leave a container running that the spec says must be stopped). D2's own §15 already records that v1.3.0 is the latest release, so the source list and the body disagree.

### §1.5 "What the spec explicitly does NOT mandate" — **MISCLASSIFIED (by omission), and it under-sells §9**

D2 lists as latitude #1: *"Error reporting. 'In cases where the specified operation generates an error, this specification does not mandate how, or even if, that error is returned or exposed to the user.'"*

The quoted sentence is accurate. **The sentence immediately after it in the same paragraph is a MUST and D2 dropped it:**

> Unless otherwise stated, **generating an error MUST leave the state of the environment as if the operation were never attempted** — modulo any possible trivial ancillary changes such as logging.

That is the spec mandating **failure atomicity**. D2 §9 argues for rollback entirely on engineering-quality grounds (`[R]` HIGH throughout, `[R]` MED for the persisted rollback list) and never cites a spec obligation, because §1.5 had already filed error handling under "latitude". The correction upgrades §9: a `create` that fails and leaves an overlay mount, a cgroup, or a live init process behind is **not spec-conformant**, it is not merely untidy. Anyone using D2 to prioritise will underweight §9 by exactly this much.

Same paragraph-halving happens at §1.5 item 3: D2 quotes "Whether other entities … can see that container is out of scope" but drops the preceding MUST — "The entity using a runtime to create a container MUST be able to use the operations defined in this specification against that same container." Minor, but it is the sentence that forbids a design where `create` and `kill` cannot be issued by the same caller.

### §1.3 `config.json` required vs optional — **CONFIRMED**

Field-by-field against `config.md` `main`: `ociVersion` REQUIRED/SemVer-v2 ✓; `root` "(object, OPTIONAL)" at top but "On all other platforms, this field is REQUIRED" ✓ (D2's "REQUIRED on Linux" is right); `root.path` REQUIRED, `root.readonly` OPTIONAL default false ✓; `process` OPTIONAL top-level, "REQUIRED when `start` is called" ✓; `process.cwd` REQUIRED ✓; `process.args` OPTIONAL but "at least one entry is REQUIRED (non-Windows)" ✓; `user.uid`/`user.gid` REQUIRED on POSIX, `umask`/`additionalGids` OPTIONAL ✓; `mounts` OPTIONAL, `destination` REQUIRED ✓; `linux` OPTIONAL ✓; hooks entry `path` REQUIRED and "MUST be absolute", `timeout` "MUST be greater than zero" ✓.

Two things D2 omits that bear on its own `[R]`:
- `root.path` may be **relative to the bundle** ("the value SHOULD be the conventional `rootfs`"), and "a directory MUST exist at the path declared". D2's minimal-config recommendation treats `root.path` as a bare string; the resolution rule is a spec MUST and the bundle-relative case is the common one.
- `mounts` — "The runtime MUST mount entries in the listed order." An ordering MUST that D2 §4 (which is explicitly about ordering constraints) does not carry.

### §1.4 hook namespaces — **CORRECTED (one row)**

D2's table gives `createContainer` and `startContainer` both as "runs in container namespace", and its `[R]` says both "require executing a **host** binary *inside* a partially-constructed container namespace".

Spec is finer than that, and the difference is exactly the hard part:
- `createContainer`: *path* MUST resolve in the **runtime** namespace; hook MUST be **executed** in the container namespace. So yes — host binary, container namespace. D2 is right here, and this is the row that is genuinely hard.
- `startContainer`: *path* MUST resolve in the **container** namespace; executed in the container namespace. So it is a **container** binary (D2's own example, `ldconfig`, confirms this — it is the image's `ldconfig`). No host-binary-into-container problem at all.

D2's stated reason for deferring both is therefore only half true. `startContainer` is the *easier* of the pair, not an equal.

### §1.2 the five operations — **CONFIRMED**

MUST/error wording for `state`/`create`/`start`/`kill`/`delete` matches verbatim, including "these operations are not specifying any command-line APIs". D2's §1.2 `create` row compresses "All … except `process` MUST be applied / `process.args` MUST NOT be applied until `start` / the remaining `process` properties MAY be applied" into "MUST apply all config.json properties except `process`" — imprecise in the table but D2 states it correctly in §1.5 item 5, so no verdict.

### §1.5 `[R]` "OCI compatibility is a serialization + lifecycle-semantics contract, not an architecture" — **CONFIRMED**

Well supported by the spec text quoted, and strengthened by the Scope-of-a-Container section. The one qualification is the failure-atomicity MUST above: it *is* a constraint on the implementation, not just on serialization.

---
## §2 — The Docker chain, boundary by boundary

### §2.1 process-tree observations — **CONFIRMED (re-run this session)**

`ps -eo pid,ppid,pgid,sid,comm` on `host-a`, 2026-09-05:

```
    912       1     912     912 containerd
  70234       1   70234   70234 dockerd
  82248       1   82248     912 containerd-shim
  82506       1   82506     912 containerd-shim
2585480       1 2585480     912 containerd-shim
2588118       1 2588118     912 containerd-shim
2590591       1 2590591     912 containerd-shim
2588176   70234   70234   70234 docker-proxy   (×4)
```

Every claim holds: five shims, all `PPid: 1`, all `PGID == own pid`, all `SID == 912 == containerd's session`; the four `docker-proxy` are direct children of dockerd in dockerd's own pgid/sid. `Live Restore Enabled: false` re-confirmed. `runc --root /run/docker/runtime-runc/moby list` from an unrelated ssh process returned all five containers — the `--root`-plus-subcommand interop surface is real.

Mechanism confirmed in source, not just inferred: `cmd/containerd-shim-runc-v2/manager/manager_linux.go:110` sets `cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}` and **nothing else** — no `Setsid`. The daemon shim is `cmd.Start()`ed by the short-lived `shim start` invocation which then exits, orphaning it to PID 1. `pkg/shim/shim.go:232` `if !config.NoSubreaper { subreaper() }`. D2's reading is exactly right, with one wording nit: it is the *starter* that requests `setpgid` for the child via `SysProcAttr`, not the shim calling `setpgid(0,0)` on itself. Same outcome.

### §2.2 the shim fd table — **CORRECTED (two rows), and the correction kills a `[R]` HIGH in §5.3**

Re-read `/proc/2585480/fd` this session. The fd *numbers* are stable and D2's table is right about the ttrpc socket (9), the log FIFO (2/4/7), the epoll/eventfd/inotify set, cwd = bundle, host namespaces, and cgroup `/system.slice/containerd.service`. Two rows are wrong, and the access modes are the whole point:

| fd | actual mode | actual role |
|---|---|---|
| 11, 13 | `lr-x` | **read** ends of the container's stdout/stderr pipes — the shim holds *only* the read ends |
| 12, 15 | `l---------` (**`O_PATH`**) | path references to the docker-side stdout FIFO — neither readable nor writable |
| 14 | `l-wx` | write end of the stdout FIFO |
| 16 | `lr-x` | read end of the stdout FIFO |
| 17, 20 | `l---------` (**`O_PATH`**) | same, stderr FIFO |
| 19 | `l-wx` | write end of the stderr FIFO |
| 21 | `lr-x` | read end of the stderr FIFO |

D2 calls fds 12/14/15/16 "shim's write + read ends of the docker-side stdout FIFO". Half of them are `O_PATH` descriptors, which are neither.

**The consequence is the important part.** §5.3 elevates this to a `[R]` HIGH:

> "the supervisor must hold **both** ends of the container's stdout/stderr pipes … `[O]` HIGH — the shim demonstrably holds both ends of the FIFOs … **This is the single most-copied trick in the fd table and the easiest one to miss.**"

and §5.6 bug #4 repeats it. **REFUTED as generalized.** The observation supports the opposite generalization:

1. The shim holds both ends of the **FIFO** (14 + 16, 19 + 21) — the hop to *dockerd*, a separate process that must be able to re-open by path after a restart.
2. The shim holds **only the read end** of the **container's pipes** (11, 13). It does not hold the write ends. It cannot: the write ends went to the container, and if the shim kept a copy it would never see EOF when the container dies.

D2's own architecture has no FIFO hop — §5.3 explicitly recommends deleting it ("Do not put a FIFO between the supervisor and the log file"). So the trick D2 says to copy applies only to the component D2 says to remove. Applying it to the container pipes, as §5.3's sentence literally instructs, **breaks §6.5 step 3** ("Drain the stdio pipes to EOF and flush the log") — a supervisor holding a write-end copy never gets EOF and hangs forever on the drain.

The correct rule for D2's architecture, and it is much simpler: *the supervisor creates the pipe, hands the write end to the child, closes its own copy of the write end, and never closes the read end while the container lives.* That is the whole of it. The EPIPE hazard §5.6 #4 names is real but its stated fix is wrong.

### §2.3 Boundary 3 — the five shim jobs — **CONFIRMED (4), CORRECTED (1)**

Jobs 1, 2, 4, 5 verified: `docs/terminals.md` quotes are verbatim in current runc `main` (the "through no fault of the user" passage at lines 219–224; the pty-master passage at line 341); the `delete`-subcommand contract and `TaskCreate → TaskStart → TaskExit → TaskDelete` ordering MUSTs are verbatim in `docs/runtime-v2.md` lines 264–281 and 390–399; the "shim MUST implement a `start` subcommand" text at line 193.

Job 3, "be the reaping parent" — the quote is verbatim, but D2 stops at the quote and misses the paragraph that follows it, which materially narrows the claim. See §5.4 below, where I test it.

**BootstrapParams (2.3+)** — **CONFIRMED**, and D2's version gate is right. `docs/runtime-v2.md` has a "Bootstrap protocol (2.3+)" section: `BootstrapParams` on stdin, `BootstrapResult` on stdout, `pkg/shim` tries the new protocol and falls back to the legacy CLI-flag mechanism, which is "deprecated and will be removed in a future release". Note for the corpus: `host-a` runs containerd **1.7.27**, so every `[O]` observation here is of the *legacy* path (`-namespace`/`-id`/`-address` argv, address on stdout), which is what the argv on the host shows.

### §2.3 the three containerd issues — **CORRECTED (status)**

Fetched from the GitHub API this session:

| issue | state | opened | note |
|---|---|---|---|
| #13849 | **open** | 2026-07-25 | live; D2's characterisation matches the title |
| #10727 | **closed** | 2024-09-26 | D2 presents it as a current failure mode |
| #3334 | **closed** | 2019-06-11 | seven years old, pre-shim-v2-maturity |

D2 rates all three `[C]` MED on the grounds that they are issue reports rather than post-mortems — fair — but does not say two of the three are closed, and #3334 predates most of the shim's current lifecycle handling. The `[R]` HIGH lesson D2 draws from them ("the exit status of a supervised process exists in exactly one place — the reaping parent's memory") is sound and survives independently of the issue statuses. But #10727 and #3334 should not be cited as evidence of present-day behaviour.

### §2.3 Boundary 2's FIFO-backpressure claim — **CONFIRMED, and stronger than D2 states**

D2: "if dockerd is down, the container's output blocks once the FIFO buffer (64 KiB default) fills", `[F]` MED, "confidence MED because I did not measure the 64 KiB pipe capacity on this host".

The fd-mode data above settles the *mechanism* independently of the buffer size: because the shim also holds a **read** end of each FIFO (fd 16, fd 21), the FIFO can never reach zero readers, so the shim's writes can never fail with `EPIPE` — they can only block. That is not a tunable, it is a structural consequence of the fd layout, and it is exactly the condition D2 describes. The 64 KiB figure remains unmeasured (`F_GETPIPE_SZ` needs a process holding the fd) but it is not load-bearing.

### §2.3 Boundary 4 "`runc` is a stateless CLI over a state directory" — **CONFIRMED**

Re-verified: `runc --root /run/docker/runtime-runc/moby list` and `state <id>` from an unrelated process return correct data for all five live containers; `options.json` is `{"binary_name":"runc","root":"/var/run/docker/runtime-runc","systemd_cgroup":true}` — `binary_name` really is a plain string.

---
## §3 / §4 / §10 — the create/start split, `nsexec`, and "what is a Go artifact"

### §3.2 the `exec.fifo` mechanism — **CONFIRMED verbatim** against runc `main`

`libcontainer/container_linux.go`: `unix.Mkfifo(fifoName, 0o622)` at line 503 followed by `os.Chmod(…, 0o622)` at 512 (the umask defence); `os.OpenFile(fifoName, unix.O_PATH|unix.O_CLOEXEC, 0)` at 529; `handleFifo(path, c.initProcess.pid())` at 235; `readFromExecFifo` at 329; state dir `0o711`; and the created-vs-running discriminator comment at 932–935. Every quoted line is present. `[R]` "adopt the FIFO rendezvous verbatim" is well-founded.

### §3.3 `nsexec.c` — **CONFIRMED verbatim**

The eight `SYNC_*` constants and their values (`0x40`–`0x47`) are exact; `STAGE_PARENT 0` / `STAGE_CHILD 1` / `STAGE_INIT 2` exact; `clone_parent()` really is `clone(child_func, ca.stack_ptr, CLONE_PARENT | SIGCHLD, &ca)` at line 322; `setjmp`/`longjmp` staging confirmed. One placement nit: the `__attribute__((constructor))` is not in `nsexec.c`, it is in the cgo preamble of `libcontainer/nsenter/nsenter.go`. Substance unaffected.

The `nsenter/README.md` NOTE that D2 does *not* quote is the cleanest possible support for its own §3.4 kernel-constraint row and is worth having: *"We do both `setns(2)` and `clone(2)` even if we don't have any `CLONE_NEW*` clone flags because we must fork a new process in order to enter the PID namespace."*

### §3.4 the Go-artifact / kernel-imposed table — **CONFIRMED (as a table)**

Row by row: the `setns(2)`-is-per-thread fact, the `runc init` re-exec, the C constructor, the TLV bootstrap, the `setjmp`/`longjmp` staging, the `CLONE_NEWPID` fork, the userns map-writer, and the cgroup-before-fork ordering all check out against source and man pages. The cgroup comment is verbatim at `process_linux.go:822`: *"Do this before syncing with child so that no children can escape the cgroup."* `clone3` landed in 5.3 and `CLONE_INTO_CGROUP` in 5.7, so the "require ≥ 5.7" recommendation is correctly gated.

The line is drawn in the right place **in the table**. It is drawn in the wrong place in §10.1 and §7.3, which is where the table's conclusion gets over-extended. Those are next.

### §10.1 "crun never re-execs itself … no memfd-clone, no C constructor trick" — **REFUTED**

D2, `[C]` HIGH: *"Verified by grepping `src/libcrun/container.c` … **There is no self re-exec, no `/proc/self/exe`, no memfd-clone, no C constructor trick.**"* And on that basis, `[R]` HIGH: *"runc's `runc init` re-exec, `nsexec.c`, the `__attribute__((constructor))`, the netlink TLV bootstrap and the `setjmp`/`longjmp` staging are **100% Go artifacts**."*

crun `main`, `src/libcrun/linux.c` lines 7001–7011:

```c
/* Protection for attacks like CVE-2019-5736.  */
int ensure_cloned_binary ();
__attribute__ ((constructor)) static void
libcrun_rexec (void)
{
  if (ensure_cloned_binary () < 0)
    {
      fprintf (stderr, "Failed to re-execute libcrun via memory file descriptor\n");
      _safe_exit (EXIT_FAILURE);
    }
}
```

and `src/libcrun/cloned_binary.c` exists and is 13,929 bytes of C. So crun has, in the same binary: a self re-exec, a memfd clone, and a `__attribute__((constructor))`. The grep missed it because it was scoped to `container.c`; the mechanism lives in `linux.c`. D2's own §11.8 cites `libcrun_rexec` correctly — the two sections contradict each other and §10.1 is the wrong one.

The narrow claim survives: crun needs no `nsexec`-style *namespace* staging, no `runc init`, no TLV, no `setjmp`/`longjmp`. That is the real finding and it does support the design. "100% Go artifacts" is not.

### §7.3 "design so we never re-exec into the container at all … eliminates the entire CVE class rather than mitigating it" — **REFUTED**

`[R]` HIGH, and called "the second-biggest Rust win after the threading one". Two independent refutations:

**1. The mechanism does not require a runtime re-exec.** CVE-2019-5736 works because the runtime's `/proc/self/exe` is the host binary and the *container's entrypoint* can be a symlink to `/proc/self/exe`. In D2's own §7.2 the cloned child does `execve(user program)` after `pivot_root`. If the user program resolves to `/proc/self/exe`, the child becomes `mydocker`-the-binary running as the container process; anything inside the container then opens `/proc/<pid>/exe`, waits for it to stop executing, and writes to it — overwriting `/usr/local/bin/mydocker` on the host. Nothing about doing setup in the cloned child's address space closes that. The exposure is created by `execve` of an attacker-controlled path, which every runtime does by definition.

**2. All three shipping runtimes disagree with the claim in code.** crun has *exactly* the architecture §7.3 recommends (setup in the cloned child, no namespace re-exec, direct `execv` of the user program) and still ships the memfd constructor above. youki, also in Rust and also without a namespace re-exec, calls `pentacle::ensure_sealed()` as the first statement of `main()` (`crates/youki/src/main.rs:76`) with the comment *"This vulnerability was first discovered in runc and was assigned as CVE-2019-5736, but it also affects youki."* runc's is at `libcontainer/exeseal/`.

The correct statement: not re-execing removes one *additional* exposure (the namespace-entering re-exec), but the base exposure is unchanged and the memfd self-seal is mandatory, not optional. D2 §11.8 already says "do it early; retrofitting means auditing every exec path" — §11.8 is right, §7.3 is wrong, and §7.3 is the one that reads like a design principle.

### §11.8 "`[C]` LOW — youki appears not to have this [CVE-2019-5736 mitigation]" — **REFUTED**

`crates/youki/src/main.rs:65–76` is the first thing `main()` does. D2's §12.9 resolves this correctly; §11.8's text is left standing and is the version a reader of §1–11 gets. The `[R]` conclusion drawn from it — *"we cannot assume 'youki does it' for security-critical mitigations"* — is built on a false premise, though it is decent advice anyway.

### §10.2 youki's structure — **CONFIRMED, every quoted line**

`crates/libcontainer/src/process/fork.rs` (390 lines, fetched this session): `clone3` via raw `libc::syscall(libc::SYS_clone3, …)` with the comment *"For now, we can only use clone3 as a kernel syscall. Libc wrapper is not available yet"*; fallback to `libc::clone` with an `mmap_anonymous` 8 MB stack and a one-page guard; the double-boxed closure; the `CLONE_PARENT`/`clone3`/`EINVAL`-on-exit-signal comment at lines 45–50; and the nix rejection at lines 206–213, verbatim: *"The nix wrapper also does not treat the closure memory correctly. The wrapper implementation fails to pass the right ownership to the new child process."*

`container_main_process.rs` (841 lines): main → intermediate → init, `container_clone_sibling` vs `container_clone` selected by `as_sibling`, `wait_for_mapping_request`/`setup_mapping`, time-offset request, `waitpid` on the intermediate. `notify_socket.rs:10 NOTIFY_FILE = "notify.sock"`, `wait_for_container_start` blocking in `self.socket.accept()`. `crates/libcontainer/Cargo.toml` has **no tokio** — §8.2's corroboration confirmed.

### §10.3's FIFO-vs-`notify.sock` verdict — **REFUTED on its deciding argument**

D2 picks the FIFO at `[R]` MED, and the argument that decides it is: *"The `sun_path` limit is a real, path-length-dependent trap that we would hit with 64-hex ids"*, backed by §11.1 item 9's `[R]` HIGH: *"this will bite us: `/var/lib/mydocker/containers/<64-hex-id>/attach.sock` is 105 bytes before the prefix."*

Arithmetic, done:

```
/var/lib/mydocker/containers/<64 hex>/attach.sock   = 105 bytes + NUL = 106
/run/mydocker/<64 hex>/attach.sock                  =  90 bytes + NUL =  91
sun_path                                            = 108 bytes
```

105 **is** the whole path, not "before the prefix" — there is no prefix. It fits, with 2 bytes to spare. The trap does not exist at the path layout D2 itself specifies in §6.4. D2's §14 open question 7 flags this as unsettled; it is now settled, and it settles against §10.3.

Worse for the argument: youki already demonstrates a two-line workaround that removes path length as a consideration at any depth — `notify_socket.rs:55–77` does `chdir(parent)` → `UnixListener::bind("notify.sock")` → `chdir(back)`. No `O_PATH`, no `/proc/self/fd`, no symlink shortening, none of the conmon machinery §11.1 item 9 says to copy.

The FIFO may still be the right call for the reasons D2 lists second (mode `0622` works across a userns with no socket-permission subtleties; `start` needs only `open` + `read`; no fd has to survive the create/start gap). But those were presented as the *supporting* arguments and the false one was presented as decisive. The verdict should be re-derived, not re-asserted. Note that 2 bytes of headroom is genuinely thin — a longer state prefix or a suffixed socket name breaks it — so "keep the socket path short" survives as a design caution even though "this will bite us" does not.

### §4 the creation-sequence ordering — **CORRECTED (two inversions)**

Both are ordering errors in a section whose entire subject is ordering.

**(a) Steps 8 and 9 are swapped.** D2 lists `waitForChildExit` (8) then `getPipeFds` (9). Current runc `process_linux.go:850–866` does the reverse:

```go
childPid, err := p.getChildPid()
fds, err := getPipeFds(childPid)        // ← record the fds FIRST
p.setExternalDescriptors(fds)
if err := p.waitForChildExit(childPid); err != nil {   // ← reap SECOND
```

D2's own stated rationale for step 9 ("record them BEFORE the container can move them") argues for runc's order, not D2's.

**(b) The failure path is inverted, and this one produces a bug if copied.** D2: *"On any sync failure runc does, in order: `syncSockParent.Shutdown()` → `p.wait()` → deferred `manager.Destroy()` (remove cgroup dirs) → `intelRdtManager.Destroy()` → `p.terminate()` (SIGKILL + wait)."*

Actual deferred block, `process_linux.go:788–820`, in execution order: `OOMKillCount()` → **`p.terminate()`** → `p.manager.Destroy()` → `intelRdtManager.Destroy()`. With runc's own comment on why: *"Terminate the process to ensure we can remove cgroups."* A cgroup directory with a live process in it cannot be `rmdir`ed. D2 §9.1 independently identifies this ("An orphan cgroup dir cannot be removed while any process is in it") — §4's list contradicts §9.1 and would leak the cgroup.

The OOM-annotation trick D2 recommends copying is confirmed verbatim, including the string `"container init was OOM-killed (memory limit too low?)"` and the comment about reading the count *before* killing init because systemd may remove the cgroup.

### §4 / §6.1 — "`external_descriptors` is how `runc exec` wires a new process's stdio" — **REFUTED**

D2 asserts this twice at `[C]` HIGH (§4 step 9, §6.1 field table) and builds on it in §6.4 (`external_fds [string;3]` in the state model, "for `exec` defaults") and §7.5 ("`exec` … `external_fds` — both already persisted for exactly this").

`externalDescriptors()` has exactly two consumers in runc `main`: the state serializer (`container_linux.go:971,993`) and **`criu_linux.go:489`**, which marshals them into CRIU's `descriptors.json` for checkpoint/restore. There is no reader on the `exec` path. runc's own comment at the capture site says so: *"Save the standard descriptor names before the container process can potentially move them (e.g., via `dup2()`). If we don't do this now, we won't know **at checkpoint time** which file descriptor to look up."*

`runc exec` wires stdio from the **caller's** fds via the same `setupIO` used by `create`/`run` (`inheritStdio` when detached, `setupProcessPipes` in foreground) — the container's create-time fds are irrelevant to it.

D2 explicitly puts CRIU out of scope. So `external_fds` in the §6.4 state model has no consumer at all and should be deleted — a small free simplification the corpus argued itself into carrying.

---
## §5 — Detached execution and I/O mechanics

### §5.1 the three detach mechanisms — **CONFIRMED**

fork-and-exit / `setsid()` / double-fork are described correctly, including `setsid(2)`'s `EPERM` when the caller is already a process-group leader and the fact that the second fork of a double-fork buys exactly one thing (never able to acquire a controlling terminal). `runc create` being unconditionally detached is verbatim at `utils_linux.go:224` — `detach := r.detach || (r.action == CT_ACT_CREATE)` — and again at 343. `[R]` MED "setsid only on the detached path" is sound.

### §5.2 the console-socket protocol — **REFUTED (who allocates the pty, and where)**

D2's `[C]` HIGH sequence:

```
runc:   openpty()  -> (master, slave)
runc:   dup slave onto container init's 0/1/2 ; ioctl(TIOCSCTTY) in the container
runc:   connect(path) ; sendmsg(SCM_RIGHTS, [master])
runc:   exit
```

That reads the pty as being allocated by the runc *parent*, on the host, before the container exists. It is not. `runc`'s parent only dials the socket and hands the connected fd down to the child (`utils_linux.go:125–136` → `_LIBCONTAINER_CONSOLE` → `init_linux.go:198–205`). The pty is allocated **by `runc init`, inside the container, after `pivot_root`**, and the master is sent *up* the socket from there. `libcontainer/init_linux.go:370–416`:

```go
// setupConsole sets up the console from inside the container, and sends the
// master pty fd to the config.Pipe (using cmsg). This is done to ensure that
// consoles are scoped to a container properly (see runc#814 and the many
// issues related to that). This has to be run *after* we've pivoted to the new
// rootfs (and the users' configuration is entirely set up).
func setupConsole(socket *os.File, config *initConfig, mount bool) error {
	pty, peerPty, err := safeAllocPty()
	...
	if err := cmsg.SendRawFd(socket, pty.Name(), pty.Fd()); err != nil { return err }
	return dupStdio(peerPty)
}
```

`runc docs/terminals.md` is vague here ("sent using `SCM_RIGHTS` by `runc`"), which is presumably where D2's reading came from — this is exactly the case where the narrative and the source differ.

**Why it matters, concretely.** The pty comes from the *container's* `/dev/ptmx`, i.e. its own `devpts` instance, mounted inside its own mount namespace, with the container's `gid=`/`mode=` options. Allocating it on the host and passing the slave in is runc#814 and its family: a host-devpts pty inside a container leaks a host resource across the isolation boundary and gets the ownership/permission model wrong.

**Impact on D2's own design.** §7.2's process diagram puts `(optional) openpty` in the **supervisor**, before `clone()`, on the host — precisely the arrangement runc moved away from. If `-it` is implemented from §7.2's diagram, it will be built the wrong way round. The correct shape: the supervisor creates a `socketpair`, passes one end to the cloned child, the child allocates the pty from the container's `/dev/ptmx` after `pivot_root`, `dup`s the peer onto 0/1/2, and sends the master back over `SCM_RIGHTS`. `checkTerminal()`'s two error strings and the (b)/(c) mode taxonomy are otherwise confirmed verbatim.

### §5.2/§5.6 the `TIOCSTI` and detached-pass-through warnings — **CONFIRMED verbatim**

`docs/terminals.md` lines 281–309: the `TIOCSTI` host-shell injection paragraph, the interleaved-output and non-deterministic-`stdin`-split bullets, and the `issue-1721` note that "detached pass-through will result in the container hanging if the `stdout` or `stderr` is a pipe". `[R]` "never give a container a file descriptor that refers to the invoking terminal" is well-supported.

### §5.3 the Docker log pipeline — **CONFIRMED**, with the fd-role correction from §2.2 above

`docker logs` reads a file, `docker attach` reads the live FIFO through dockerd's hijacked connection — confirmed by the fd tables. `[R]` "the supervisor writes the log file directly; no FIFO hop" is right and is independently corroborated by conmon's design (§11.4's own note). The `[R]` HIGH "hold both ends of the container's stdout/stderr pipes" is **REFUTED** — see §2.2.

### §5.4 `PR_SET_CHILD_SUBREAPER` — **CONFIRMED for the container init; REFUTED for the stated `exec` benefit**

D2's `[O]` proof that a subreaper recovers a grandchild's exit status is correct and I did not need to re-run it. What I did test is the claim D2 builds on it — `[R]` HIGH §5.4:

> "Benefit: exit codes are recoverable, **and `mydocker exec`'d processes whose intermediate parent died still get reaped instead of accumulating zombies in the container**."

and §2.3 job 3, which quotes containerd's *"The shim process takes responsibility as a sub-reaper to cleanup exited containers or `setns(2)` processes"* and stops at the quote. The two sentences containerd writes immediately after that quote are the ones that matter (`docs/runtime-v2.md:583–588`):

> "However, [\[PATCH\] exit: fix the setns() && PR_SET_CHILD_SUBREAPER interaction] **prevents any cross-namespace reparenting in kernel**. Assume that container is in X-namespace and P in root-namespace setns into X-namespace. P forks child C. The child C forks a grandchild G and exits. The G will be reparented to X instead of P's reaper. If the PID namespace is different from shim process, **the container init process should cleanup any orphaned reparented processes created by setns process (exec operation)**."

**Tested on `host-a`, kernel 6.11.0-29-generic** (own short-lived processes only, no containers touched; script removed after the run). Supervisor S sets `PR_SET_CHILD_SUBREAPER`; a child unshares `CLONE_NEWPID` and forks G (PID 1 in the new ns) then exits; separately a process E `setns()`es into G's pidns, forks F, F forks H, both F and E exit:

```
SUPERVISOR pid=3120122 prctl(PR_SET_CHILD_SUBREAPER,1)->0
G host pid = 3120124
G PPid after C exited: 3120122          <- init DID reparent to the host subreaper
SUPERVISOR reaped = [(3120123,0), (3120127,0), (3120128,0), (3120124,0)]
G reaped         = [(2,0), (3,9), (5,11)]
```

Two results, both load-bearing:

1. **CONFIRMED** — the container init (host pid 3120124, PID 1 in its own pidns) reparented to the host-side subreaper S when its creator exited. This is exactly the shim↔`runc create`↔container-init relationship D2 §2.1 infers, now demonstrated directly rather than inferred.
2. **REFUTED** — the orphan created *inside* the pidns by the `setns`ed process (H, ns-local pid 5, exit status 11) was reaped by **G, the container's PID 1** — not by S. So was the plain in-namespace orphan (ns-local pid 3, status 9). `PR_SET_CHILD_SUBREAPER` on our supervisor buys **nothing** for zombies inside the container. The kernel refuses cross-namespace reparenting; the container's own PID 1 is the only reaper that can ever get them.

What the flag actually buys, precisely: reaping of orphans whose dying parent was at the **host** pid-namespace level. That covers the container init itself (the whole point) and any `exec` helper process we fork on the host side. It does not cover anything the container's PID 1 is responsible for.

**Design consequence D2 does not draw.** A container whose PID 1 is `nginx`, `bash`, or any app that does not reap will accumulate zombies from `mydocker exec` orphans, and no supervisor-side flag can fix it. Docker's answer is `--init` (a tiny reaping PID 1 — `docker-init 0.19.0` is installed on `host-a` and D2's own version block records it). D2 never mentions `--init` or the problem it solves. Add it to §7.5's "cheap later" list; it is ~30 lines and it is the only fix.

### §5.4 "runc sets it in its foreground path" — **CORRECTED (minor)**

`utils_linux.go:265–270` places `if r.enableSubreaper { system.SetSubreaper(1) }` *before* the `if !detach` guard, so runc sets it on both paths (uselessly on the detached one, since runc then exits). The quoted comment "set us as the subreaper before registering the signal handler for the container" is verbatim.

### §5.4 the runc reap loop — **CONFIRMED verbatim**

`signals.go` in current `main`: `signalBufferSize = 2048` with the "large buffer size so that we do not miss any signals" comment; `signal.Notify(s, unix.SIGCHLD)` first with the "signal.Notify is actually quite expensive … (all ~65 of them)" comment (now additionally citing runc#5208); the `Wait4(-1, &ws, WNOHANG, &rus)` drain loop; `SIGURG` explicitly ignored for Go's async preemption; `SIGWINCH` → `tty.resize()`; default → forward to pid1 with the "forwarding signal %d (%s) to %d" log line.

### §5.5 signal semantics for a PID-namespace init — **CONFIRMED**

`pid_namespaces(7)` matches: a signal from an ancestor namespace reaches the namespace's init only if init installed a handler, `SIGKILL`/`SIGSTOP` excepted. The `[R]` (TERM → timeout → KILL, and document why) follows.

### §5.5 `cgroup.kill` — **CONFIRMED, and D2's `[F]` MED should be HIGH**

D2 hedged: *"Confidence MED on the exact semantics because I read the file's presence, not the kernel doc … What would settle it: `Documentation/admin-guide/cgroup-v2.rst`."* Fetched it. `cgroup-v2.rst:1051–1064`:

> "A write-only single value file which **exists in non-root cgroups**. The only allowed value is "1". Writing "1" to the file causes the cgroup and all descendant cgroups to be killed … all processes located in the affected cgroup tree will be killed via SIGKILL. **Killing a cgroup tree will deal with concurrent forks appropriately and is protected against migrations.** In a threaded cgroup, writing this file fails with EOPNOTSUPP…"

Every element of D2's claim is confirmed, including the root-cgroup absence (which the doc makes definitional, not incidental). One addition D2 does not have: **`EOPNOTSUPP` in a threaded cgroup** — worth a branch if we ever use `cgroup.type threaded`.

### §5.5 `cgroup.events` — **CONFIRMED**

`cgroup-v2.rst:955–966`: exists on non-root cgroups; `populated` is "1 if the cgroup or its descendants contains any live processes; otherwise, 0"; and "a value change in this file generates a file modified event", which is what makes it `poll()`-able. D2's `[R]` (use it as a cross-check for the unsupervised case, not as an exit-code source) is correct and well-founded.

---
## §6 — Persistent state

### §6.1 / §6.2 / §6.3 — **CONFIRMED**, all re-checked

- `state.json` non-`config` fields on `host-a` match D2's transcript byte for byte, including `init_process_start: 1420814509` and `external_descriptors: ["/dev/null","pipe:[22485389]","pipe:[22485390]"]` matching shim fds 11/13.
- Atomic write confirmed at `container_linux.go:882–905`: `os.CreateTemp(c.stateDir, "state-")` → `WriteJSON` → `Close` → `Rename`. **No `fsync`, no lock file** — exactly as D2 says.
- PID-reuse guard confirmed at `container_linux.go:948`: `if stat.StartTime != c.initProcessStartTime || stat.State == system.Zombie || stat.State == system.Dead`.
- containerd `meta.db` is fd 3 of pid 912, held for the daemon's life; bbolt's single-writer `flock` is what makes containerd a daemon. Confirmed.
- Docker `config.v2.json` top-level key set and `State` sub-object match exactly (re-read this session). The unbounded-log footgun is now **69,526,164 bytes** for the container D2 measured at 27,498,753 — the same file, 24 days later, still unrotated. D2's `[R]` "cap it from day one" is if anything understated.
- `/proc/sys/kernel/random/boot_id` present and readable. The boot-id reboot marker `[R]` is sound and is strictly better than Podman's `alive`-file proxy, as D2 argues in §11.1.

The one caveat worth adding to §6.1: `external_descriptors` should be **dropped** from D2's state model — see the §4/§6.1 refutation above. It has no consumer outside CRIU.

### §6.6 concurrency — **CONFIRMED**, plus two gaps in the recommended design

runc's TOCTOU is real and is actually *wider* than D2 describes: `factory_linux.go` does `os.Stat(stateDir)` at line 52, then a cgroup existence-and-emptiness check (lines 58–88), then `os.Mkdir(stateDir, 0o711)` at line 91. That is ~40 lines of window, not two adjacent calls. `validateID`'s character set and `securejoin.SecureJoin` usage confirmed. `flock(2)` semantics as D2 states them (associated with the open file description, auto-released on death or last close) are correct.

Two things D2's `[R]` HIGH concurrency design does not handle, both classic and both cheap to fix:

**(a) The flock-on-a-removable-file race.** §6.6 puts the lock on `<dir>/lock`, and §9.4 says `delete --force` removes the state dir. Process A holds `LOCK_EX` on `<dir>/lock`; process B (running `rm --force`) unlinks it and a third process re-creates and locks a *new* inode — two holders, no contention, because `flock` locks the inode via the open file description, not the path. Standard mitigations: after acquiring, `fstat` the held fd and `stat` the path and compare `st_dev`/`st_ino`, retrying if they differ; or keep the lock file in a directory that is never removed (`/run/mydocker/locks/<id>`). Given D2 also recommends the `st_dev`/`st_ino` assertion for `pivot_root` in §11.7, the idiom is already in the document — it just is not applied here.

**(b) The lock is inherited by the supervisor.** §7.2 has the CLI take `LOCK_EX`, `fork/exec` the supervisor, then "release lock, exit 0". `flock(2)`: a lock is released "either by an explicit `LOCK_UN` … or when **all** such file descriptors have been closed", and `fork(2)` gives the child a descriptor referring to the *same* open file description. If the lock fd reaches the supervisor, the CLI's exit does not release the lock and every later `start`/`kill`/`rm` on that container blocks for the container's lifetime. Rust's `File` sets `FD_CLOEXEC` by default so an `exec`-ed supervisor is fine in practice — but the design note "auto-release on death is the whole reason to use `flock`" deserves the companion note "and that requires that exactly one process holds it".

---

## §7 — The architecture recommendation (the highest-stakes section)

The recommended shape — one binary, no daemon, per-container supervisor, per-container `flock`, exit code on disk, sync core — is **defensible and I do not refute it**. What follows are the places where the stated justification is weaker than the tone, where a premise is a convention presented as a fact, and where the section contradicts other sections of the same document.

### §7.4 "No. Never, in any mode, including foreground" vs §11.7 "the container is a process, not a service" — **the corpus contradicts itself, and §7.6 phases the wrong one first**

§7.4, `[R]` HIGH: *"Should the CLI ever be in the container's process ancestry? **No. Never, in any mode, including foreground.**"*

§11.7's principled-simplifications list, `[R]` HIGH, item 1: *"**The container is a process, not a service.** Foreground execution with the CLI as supervisor needs no shim, no exit file, no attach socket, no reconnect protocol. Apptainer proves the 95% interactive case needs none of it."* And item 2: *"**Exit status via `wait()`.** Podman's exit-file + `--exit-command` + inotify dance exists *only* because the supervisor must outlive the client."*

These cannot both be adopted. §11.7 says a general engine "can and should adopt" foreground-CLI-as-supervisor; §7.4 forbids it in every mode.

**The evidence §7.4 rests on does not carry it.** It cites runc `docs/terminals.md`'s "through no fault of the user" passage. That passage is verbatim (lines 219–224) — but D2 quotes the drawback and omits the recommendation two paragraphs earlier in the same file (lines 185–188):

> *"In general we recommend using foreground because it's the most straight-forward to use, with the only downside being that you will have a long-running `runc` process. **Detached mode is difficult to get right and generally requires having your own `stdio` management.**"*

runc's own document reaches the opposite default from §7.4, using the same facts. Quoting the con and dropping the pro of the same paragraph-pair is selective citation in support of a conclusion already reached.

The quoted hazard is also narrower than §7.4 makes it. It is about the foreground process *owning the container's stdio* — kill it and the container dies of `SIGPIPE`. It is not about *ancestry*. A foreground CLI that is the container's parent but whose death is survivable (write end of the log file dup'd into the child; `SIGPIPE` ignored; exit status lost but the container fine) is not the failure the quote describes.

**The phasing makes it concrete.** §7.6 puts `run -d` in Phase 1 and foreground `run` in Phase 2. So the *simpler* thing (CLI forks, `waitpid`s, exit code is the CLI's exit code, no state file needed for it, no log rotation, no subreaper, no detach) is deferred behind the *harder* thing (detach, supervisor, handshake, durable exit status, log writer with a size cap). For a learning-oriented single-machine project that is inverted, and it is inverted against D2's own §11.7 analysis.

**MISCLASSIFIED**: `-d` is treated throughout §7 as a requirement, but no stated scope item requires it. §11.8's closing paragraph is the only place that admits this — *"Our §7 design deliberately does create it … and that is a **priced choice**, made because we want `-d` and a durable exit code"* — and it is 400 lines away from the recommendation it qualifies. §7 should carry that sentence, and should price the alternative (Phase 1 = foreground only, two processes, no supervisor, no state file for exit codes) rather than skipping it.

### §7.2 does not deliver the resolution §2.3 promises — **unfulfilled forward reference**

§2.3 Boundary 4 rates collapsing the shim→runc split at `[R]` **MED**, and names the exact reason:

> *"Confidence MED not HIGH because collapsing this boundary means our supervisor process **is** the process that called `clone(CLONE_NEWPID|...)`, and that couples the supervisor's lifetime to the creation path in a way runc's design deliberately avoids. **§7.2 resolves this with a three-process shape that keeps the collapse but not the coupling.**"*

§7.2's three processes are CLI → supervisor → container init. The supervisor still calls `clone()`. The coupling is not removed; it is restated. §7.2's own `[R]` MED addresses a *different* question — thread-safety at the moment of `clone()` — and concludes "calling `clone()` directly is simpler and correct **as long as the supervisor is single-threaded at that moment**". Single-threadedness is not what §2.3 was worried about. §2.3 was worried about **lifetime**: a bug or a panic anywhere in the ~15-step create sequence takes down the process that is supposed to hold the container's stdio and exit status for the container's whole life — which is precisely the property runc buys by letting `runc` exit.

The honest resolution is one of: (i) accept the coupling and say so (the create path is short and can be made panic-free, and a create failure means there is no container to supervise anyway — this is actually a good argument, D2 just never makes it); or (ii) add the intermediate fork, i.e. the supervisor forks a *creator* that does the clone and dies, and the container init reparents to the supervisor via `PR_SET_CHILD_SUBREAPER` — which is exactly the shim↔runc shape, verified working in my §5.4 test. Either is fine. Claiming §7.2 already resolved it is not.

### §7.2's state-writer story is inconsistent with §7.5's own rule

§7.2: *"`mydocker start <id>` = flock, open `exec.fifo` `O_RDONLY`, read one byte, **set `status=running`**, unlock. **That's it — no IPC with the supervisor at all.**"*

§7.5: *"**One module owns all state writes, from day one. Nothing else opens `state.json`.** This is the cheapest expensive-thing-to-defer on the list."*

Under §7.2 there are two *processes* writing `state.json` — the CLI (on `start`) and the supervisor (on `created` and `exited`) — coordinated only by the `flock`. That is workable, but the diagram never says the supervisor takes the lock. As drawn, the supervisor writes `state.json (status=created)` and later `state.json (exited, code)` with no lock shown, while `start` does a locked read-modify-write. A `start` that races the supervisor's `exited` write will resurrect a dead container as `running`. One added line — "the supervisor takes the same `flock` for every state write" — closes it, and the diagram should have it because the diagram is what gets implemented.

### §7.1 "a startup/reconciliation path" listed as a daemon-only cost — **MISCLASSIFIED**

§7.1's daemon row lists as a cost of having a daemon: *"a startup/reconciliation path ('what containers existed when I died?')"*. The daemonless design does not avoid that work; §6.4's cases C, D and E **are** that reconciliation, and §11.1 says so of Podman: *"podman reconciles from files on the next CLI run"*. The cost is relocated from once-per-daemon-start to once-per-CLI-invocation, not eliminated. The daemon row is otherwise well argued; this line should move to "moved, not avoided".

### §7.5 "A daemon, later — because *nothing* in the design depends on there being no daemon" — **UNSUPPORTED as stated**

One thing does: there is no push channel. §6.6 says so two sections earlier — *"no event stream (`docker events`) without a daemon or a shared append-only log — genuinely a gap, and the honest answer is 'phase 3, and it needs a design'"* — and §7.5's own table lists `events` under "cheap later" while conceding it is "not free". A daemon added later would have to poll N state files or add a socket to every supervisor, which is the thing §7.5 says is cheap ("adding a socket is cheap") but which is a change to *every* supervisor, not a new front-end. The weaker and true claim is: nothing in the design *forbids* a daemon, and the state directory plus per-container `flock` is a reasonable substrate for one.

### §7.3 single binary with subcommands — **CONFIRMED**, one editing artifact

Reasons 1–4 are sound; the `/proc/self/exe` re-exec, `shim-binary-path` in the bundle, and the three-way version matrix are all confirmed on the host. `file` re-run this session: `containerd-shim-runc-v2` **statically linked**; `runc` and `dockerd` dynamic PIE; `ldd /usr/bin/runc` → `libseccomp.so.2`, `libc.so.6`. runc's `runc init` re-exec through `exeseal.CloneSelfExe` confirmed at `container_linux.go:547–568`, with the comment that names the threat model exactly: *"if a container manages to overwrite the file, it cannot affect other containers on the system."*

One flaw of presentation, not fact: the sentence *"So does Podman (`podman --module`… no: Podman re-execs itself for rootless via `podman-in-userns`)"* is an unresolved editing note left inside a `[C]` HIGH claim. It should be cut or finished.

The `[R]` HIGH about never re-execing into the container is **REFUTED** — see §7.3 above under §10.

### A simpler architecture that meets the stated requirements — not considered

§7 compares its design against "a daemon" and against "runc + shim". It never compares against the smaller thing: **`run` always supervises in the foreground; `-d` is the same code path after `setsid()` + stdio redirected to the log file.** That is two processes, one code path, no `supervise` subcommand, no handshake pipe, no second state writer, and the exit code is `waitpid`'s return in both modes (durable on disk only in the `-d` case, where the supervisor is the same process that would have been the CLI). It is strictly less machinery than §7.2 for the same feature set, and it is what conmon does modulo conmon being a separate binary. Whether it wins is arguable; not evaluating it in the section that claims to pick "the simplest technically sound" architecture is the gap.

---
## §8 — Sync vs async

### §8.1 the post-fork hazard — **CORRECTED on the mechanism; the conclusion gets *stronger*, not weaker**

D2's `fork(2)` quotes are verbatim and `signal-safety(7)`'s allowlist really is that small. The correction is to this sentence, `[F]` HIGH:

> "Note that `clone(2)` with namespace flags is subject to **exactly the same constraint** as `fork` — it's the same kernel path, and glibc's `fork()` is itself implemented as `clone(SIGCHLD)`."

The *kernel* path is the same. The *libc* path is not, and the libc path is where the protection lives. glibc `posix/fork.c` (fetched this session) does, when the process is multithreaded, before it clones:

```c
      call_function_static_weak (__nss_database_fork_prepare_parent, &nss_database_data);
      _IO_proc_file_chain_lock ();
      _IO_list_lock ();
      /* Acquire malloc locks.  This needs to come last because fork
         handlers may use malloc … */
      call_function_static_weak (__malloc_fork_lock_parent);
  pid_t pid = _Fork ();
  if (pid == 0) { … call_function_static_weak (__malloc_fork_unlock_child); fresetlockfiles (); … }
```

So `fork()` from a multithreaded glibc program leaves the child with **usable** malloc arenas, a reset stdio lock state, and reset NSS state. A raw `clone`/`clone3` — which is what D2's §7.2 and §10.2 specify, following youki's `libc::syscall(SYS_clone3, …)` — runs **none** of that. The child of a raw `clone3` from a multithreaded process really can deadlock in the allocator; the child of `fork()` normally cannot.

This makes §8's conclusion more necessary, not less: the moment you choose `clone3` for `CLONE_INTO_CGROUP` and namespace flags, you give up glibc's atfork protection, and "the process that calls `clone()` never has more than one thread, ever" stops being belt-and-braces and becomes the only thing standing between you and a nondeterministic hang. Worth stating in the document, because a reader who knows the glibc behaviour and reads D2's sentence will conclude the hazard is overstated and reach for a thread pool.

Two smaller notes on the same paragraph: Rust `std::sync::Mutex` released by a *panicking* thread is unlocked-and-poisoned, not left locked, and Rust has no thread cancellation — so D2's stated reason "a thread that has exited can still have left a lock held at the moment it was cancelled" does not apply to Rust threads. The real reason is the one D2 gives second and should give first: **you cannot audit every transitive dependency for thread spawning**, and C libraries in the graph (`libseccomp`, anything with a background reaper) have no such guarantees.

### §8.2 / §8.3 — **CONFIRMED**

tokio's multi-threaded runtime spawning workers at construction is correct. The corroboration from youki is confirmed: `crates/libcontainer/Cargo.toml` fetched this session has **no tokio** and no async dependency, while the workspace root does. `[R]` "put the pull path in a separate process, not just a separate module — structural beats disciplined" is the right call and is the strongest recommendation in this section.

The `tracing`-in-the-child ban is confirmed as convergent practice: runc's `_LIBCONTAINER_LOGPIPE`, crun's `crun_set_output_handler(log_write_to_sync_socket, …)`, conmon's always-created stderr pipe (`conmon.c:179`: *"We always create a stderr pipe, because that way we can capture runc stderr messages before the tty is created"*). D2's own MED hedge on whether `tracing` specifically deadlocks is appropriate and the design does not depend on the answer.

---

## §9 — Error handling and failure atomicity

### §9.2 what runc does — **CONFIRMED**

Defer-based reverse unwind with the named-return-error pattern; `--keep` confirmed at `utils_linux.go:418` (`shouldDestroy: !cmd.Bool("keep")`) and the gate at 228 (`if !detach || retErr != nil { r.destroy() }`); the OOM-count annotation confirmed verbatim. The `[F]` HIGH mount-namespace teardown property (`mount_namespaces(7)`) is correct and D2's `[R]` — *"do as little as possible in the host mount namespace, and everything else inside the new one"* — is the single best idea in §9 and should be flagged to D3 as D2 says.

The ordering error in §4's copy of runc's failure path (Destroy before terminate) is corrected above and contradicts §9.1's own analysis.

### §9.1's spec status — **upgraded by the §1.5 correction**

Per §1 above, the OCI spec makes failure atomicity a **MUST**: *"generating an error MUST leave the state of the environment as if the operation were never attempted."* §9 argues the whole case on engineering grounds and never invokes this. It should; it changes §9 from good practice to conformance.

### Adjudication 1 — the networking agent's "a leaked IP is a misroute, not slow exhaustion; release order is a security property"

**Both are right about different failure modes, and D2's severity table hides the dangerous one.**

D2 §9.1's table: *"A leaked IP from the pool is a slow exhaustion. A leaked iptables/nft rule pointing at a dead IP is a latent misroute."*

- **IP acquired and never released** → the pool believes it is in use → it is never handed out again → **exhaustion**. D2 is correct, and this is genuinely the low-severity case.
- **IP released while something still references it** — a DNAT/forward rule, a conntrack entry, an ARP/neighbour entry, a published-port mapping — → the next container gets that address and **inherits the previous container's traffic**. That is a misroute, it is a cross-tenant data path, and it is a security bug rather than a housekeeping one.

So the hazard is not the leak, it is the **inversion**: releasing the IP before the things that point at it are gone. The networking agent's framing — *release order is a security property* — is the correct one and D2's table does not express it, because a severity table indexed by "which resource leaked" cannot express a constraint that is about *relative ordering between two resources*.

The good news for D2: its own §9.3 machinery already produces the right order for free. Creation order in §9.1 is `8. move veth into netns → allocate IP → add rule`; a strict reverse unwind of the `Vec<Cleanup>` removes the rule before it calls `ReleaseIp(addr)`. What is missing is the *statement* that this particular adjacency is load-bearing, so that nobody later "optimises" the release to happen early (a very natural thing to do — releasing the IP first shortens the window in which the pool is short). **Recommended amendment to §9.3:** annotate `Cleanup::ReleaseIp` as order-critical, and add a conntrack flush for the address between the rule removal and the release, because deleting an nftables rule does not tear down flows already in conntrack.

Verdict on D2 §9.1's table row: **CORRECTED** — "a leaked IP is a slow exhaustion" is true but is the wrong risk to record; the recordable risk is premature release, and it belongs in §9.3 as an ordering invariant, not in §9.1 as a severity.

### Adjudication 2 — netavark's `firewall/` replay directory as precedent for §9.3's persisted rollback list

**The precedent is real, it is a shipping Rust implementation, and it is cheap — but it is a *replay* list, not a *rollback* list, and that distinction is exactly where the risk is.**

Verified: `containers/netavark`, `src/firewall/state.rs` (354 lines, `main`, fetched this session). Layout, from the file's own header comment:

```
$config/firewall/
                - firewall-driver -> name of the firewall driver
                - networks/$netID -> network config setup
                - ports/$netID_$conID -> port config
```

with `const FIREWALL_LOCK_FILE: &str = "firewall-reload.lock"`, `use fs2::FileExt`, and `wrap!(lock_file.lock_exclusive(), "lock firewall lock file")?` at line 115. The lock's purpose is documented in the struct:

> *"This file is required to ensure that `remove_fw_config` is not racing against the firewall reload service, i.e. without it would be possible that we read the config files and then during re-adding the rules the file got removed."*

The consumer is `src/commands/firewall_reload.rs` → `read_fw_config(config_dir)` → for each stored network `fw_driver.setup_network(...)` and for each stored port `fw_driver.setup_port_forward(...)`.

So what netavark persists is **desired state, to be re-applied** after firewalld restarts. It is not an undo log. That matters in one specific way:

- A **replay** list is naturally idempotent — re-adding an nftables rule that already exists is harmless, so a crash mid-replay is safe and the list can be re-run any number of times.
- A **rollback** list is not — `ReleaseIp(addr)` executed twice hands the same address to two containers, and `Umount(path)` executed twice can unmount someone else's mount at the same path. A persisted rollback list therefore needs per-entry completion marking, or every `Cleanup` variant must be made idempotent by construction (release only if still owned by this container id; unmount only if the mount id matches).

**Adjudication:** netavark establishes at HIGH confidence that the *mechanism* D2 proposes — a small per-entity directory of JSON records plus one `flock`, written during acquisition and read by a separate recovery command — is practical, cheap, and shipping in Rust in this exact domain. It establishes nothing about the rollback direction. D2's `[R]` MED rating should therefore **stay MED, with the reason replaced**: not "is this worth phase-1 complexity" (netavark shows the cost is a few hundred lines), but "a persisted undo log must be idempotent or crash-marked, and D2 has not specified either". D2's stated cheap alternative — *"on `rm`, best-effort try every possible cleanup and report what failed"* — is in fact the idempotent-by-construction version of the same thing, and is the right phase-1 answer for that reason rather than the "80% as good for 20% of the work" reason D2 gives.

### §9.3's child error channel — **CONFIRMED and independently corroborated**

crun `src/libcrun/container.c` lines 71–102 really do define `struct sync_socket_message_s { int type; int error_value; char message[512]; }` written with a single `write()`, and crun really does redirect its whole log output handler into that socket while in the child. D2's `#[repr(C)] { kind: u32, errno: i32, msg: [u8; 512] }` is the same shape. Good.

### §9.4 `delete --force` must survive arbitrary corruption — **CONFIRMED as sound, no evidence needed**

Well argued; the failure mode named (bail on first `ENOENT`, container becomes undeleteable, user `rm -rf`s the state dir and leaks the mount) is the real-world one.

---

## §11 — Architecture families

### §11.1 Podman + conmon — **CONFIRMED, every line number checked**

`containers/conmon` `main`, `src/conmon.c` (552 lines), fetched this session: the start-pipe block comment at **line 66** (*"Block for an initial write to the start pipe before spawning any children or exiting, to ensure the parent can put us in the right cgroup"*); the double-fork parent's `_exit(0)` at 103; `setsid()` at **132**; `set_subreaper(true)` at **138** with the comment *"Set self as subreaper so we can wait for container process and return its exit code"*; the always-created stderr pipe at **179** with D2's quoted rationale; the `reset_oom_adjust()` comment *"We don't want runc to be unkillable"* at 282. D2's `[R]` HIGH about the cgroup-placement handshake (caller places the supervisor, then releases it) is a genuinely good steal and is correctly sourced.

Note `setsid()` at 132 is immediately followed by the subreaper call at 138 — which is exactly D2 §5.4's `[R]` ("set `PR_SET_CHILD_SUBREAPER` as the very first thing after `setsid()`"), independently arrived at. Nice.

### §11.1 item 9 — the `sun_path` argument — **REFUTED** (arithmetic; see §10.3 above)

### §11.2 CRI-O — **CONFIRMED**

`internal/lib/container_server.go:729–739`: `ContainerStateToDisk` → `ioutils.NewAtomicFileWriter(ctr.StatePath(), 0o644)`; `FromDisk()` restore at 518 and 691. No database. D2's `[R]` HIGH — *"CRI-O runs production Kubernetes nodes … on a pile of `state.json` files, atomic-rename writes, and no database"* — is correctly sourced and is a fair reading.

### §11.3 containerd — **CONFIRMED**

The "does not directly launch containers … acts as a higher-level manager or hub" quote is verbatim at the top of `docs/runtime-v2.md`. The plugin-architecture and namespaces-are-not-a-security-boundary points are as D2 states. `meta.db` as containerd's fd 3, held for the daemon's life, re-confirmed on the host. `[R]` "an interface with one implementation is not architecture" — agreed, and it is the right lesson.

### §11.4 Docker/moby — **CONFIRMED**

Four `docker-proxy` children of dockerd with `-use-listen-fd` for :80/:443 over v4+v6 re-observed. The dockerd-holds-the-FIFO-read-ends observation re-confirmed from the fd tables, and the "be conmon here, not dockerd" conclusion is right — and, per §2.2, is *better* supported than D2 knew: the shim's extra read-end fd on each FIFO makes the stall structural rather than buffer-size-dependent.

### §11.5 gVisor — **CONFIRMED verbatim**

Checked against `g3doc/architecture_guide/platforms.md` on `master`, because this is the section most likely to be received wisdom. It is not:

> *"`systrap` replaced `ptrace` as the default gVisor platform in **mid-2023**."* (lines 59, 73)
> *"While `ptrace` continues to exist in the codebase, it is **no longer supported and is expected to eventually be removed entirely**."* (lines 73–76)
> KVM: *"allow the Sentry to act as both guest OS and VMM."* (line 40)
> systrap: *"relies on `seccomp`'s `SECCOMP_RET_TRAP` feature."* (line 53)

Every phrase D2 attributes to gVisor's docs is there. `[R]` "not our problem, but say so in the README — a namespace-based engine does not protect against a hostile image" is correct and worth keeping.

### §11.6 Kata / Firecracker — **CONFIRMED**

`kata-containers` `docs/design/architecture/README.md`: `kata-agent` described as *"supervisor for managing the containers and the workload"*; ttRPC over VSOCK for runtime↔agent (lines 293, 323); virtio-fs for the container rootfs into the guest (lines 121, 167); DAX section at 429–448 covering the guest image mapping. The shim-implements-the-contract-but-never-calls-runc point is consistent with the document's structure (line 191 references runc only as the *other* thing Kata is contrasted with). `[R]` "we can ignore the spec's `vm` section entirely and still be conformant" is right — `vm` is OPTIONAL per `config.md` line 527.

### §11.7 Apptainer — **CONFIRMED (source claims); UNSUPPORTED — pending host (every `[O]`)**

Source-side, all checked against `apptainer/apptainer` `main`:
- `cmd/starter/c/starter.c` is **exactly 1715 lines**, as D2 states.
- `enum goexec goexecute;` at line 75; `STAGE1`/`STAGE2`/`MASTER`/`RPC_SERVER` dispatch confirmed at 1316, 1524, 1554, 1701.
- `fork_ns()` is `clone()` wrapped in `sigsetjmp` with the verbatim comment *"This is hack to make clone() behave like fork()"* (lines 101–110).
- `fork_ns(CLONE_FILES)` for STAGE1 at 1303; `fork_ns(clone_flags)` at 1463; `fork_ns(CLONE_FS)` for the RPC server at 1518.
- The `joinOnly` instance path and *"Don't execute RPC server, joining instance"* at 1397/1545.
- The weak-chroot assertion D2 recommends stealing, verbatim at 1666: *"Running inside a weak chrooted environment, prefer pivot_root instead of chroot"*.
- `cmd/starter/main_linux.go` exists (HTTP 200) — so §11.7's citation of it is valid. **This corrects the corpus's own §15 unresolved-citation note #2**, which flags `main_linux.go` as a dead reference: the only `main_linux.go` cited in the §1–§11 body is Apptainer's, at line 1275, and it resolves. It is runc that has no `main_linux.go`, and §1–§11 never cites one for runc. Nothing in §1–§11 rests on that dead reference.

Empirical side: **`old-host` is unreachable** (`old-host.example.edu` → NXDOMAIN). Every `[O]` in §11.7 and §11.8 sourced from it is **UNSUPPORTED — pending host**: `APPTAINER_SUID_INSTALL=0`; the absence of `starter-suid`; `fusermount3` being setuid while `newuidmap`/`newgidmap` are not; the missing `/etc/subuid` entry; `sysctl user.max_user_namespaces`; the `apptainer.conf` values; `SESSIONDIR`; `~/.apptainer` permissions; `cat /proc/self/cgroup`; the `apptainer exec --help` flag listings; the `[O]` MED `--fakeroot` consequence; and §11.8's "the whole rootless stack is installed next to podman on `OLD-HOST`". The commands that would settle them are the ones D2 already quotes inline; they need only a reachable host.

The `[F]` HIGH list of what a suid helper does that a userns cannot (loop-backed `mount(2)`, multi-range id maps, real `overlayfs` below 5.11, kernel dm-crypt) is correct as kernel semantics and does not depend on the host.

### §11.7 "Principled vs HPC-specific" — **CONFIRMED as analysis**, but item 1 is in direct conflict with §7.4

The split is the best analytical passage in the document and I have no correction to the individual items. Item 1 ("the container is a process, not a service" — foreground execution with the CLI as supervisor) is `[R]` HIGH *adopt*, and §7.4 is `[R]` HIGH *never*. See §7 above. One of the two has to change, and the document should say which.

### §11.8 — **REFUTED (youki)**, **CORRECTED (crun/runc file identity)**

- *"`[C]` LOW — youki appears not to have this [the CVE-2019-5736 mitigation]"* — refuted; `crates/youki/src/main.rs:76` calls `pentacle::ensure_sealed()` as the first statement of `main()`.
- *"runc and crun ship the **same file** for that last one (`cloned_binary.c`…)"* — **half stale.** crun's `src/libcrun/cloned_binary.c` exists and is still C (13,929 bytes). runc's is gone: `libcontainer/nsenter/cloned_binary.c` → **404** on `main`; the mitigation is now Go at `libcontainer/exeseal/cloned_binary_linux.go`. The line reference `src/libcrun/linux.c:6986` has drifted to 7003–7004. The substantive point (three implementations converge on memfd self-sealing; do it early) is correct and, with youki added, is now four-for-four.
- The closing `[R]` — *"almost every mechanism above the fork/exec/namespace/cgroup layer exists to solve 'my supervisor is not my client's parent'"* — is the sharpest sentence in the document and I could not falsify it. It is also the sentence that most directly indicts §7.4.

---
## Ledger

Scope §1–§11 contains 40 `[F]`, 114 `[C]`, 102 `[R]`, 71 `[O]` tagged claim lines. Exactly **one** LOW-confidence claim exists in scope (§11.8, youki/CVE-2019-5736) and it is REFUTED.

### REFUTED (8)

| # | § | Claim | Refuted by |
|---|---|---|---|
| 1 | 5.3, 5.6#4, 2.2 | `[R]` HIGH "the supervisor must hold **both** ends of the container's stdout/stderr pipes … the single most-copied trick in the fd table" | `/proc/<shim>/fd` modes: shim holds only the **read** ends (11, 13 = `lr-x`) of the container pipes. Both-ends applies to the docker-side **FIFO** (14+16, 19+21), the hop D2 recommends deleting. Following the rule as written breaks §6.5 step 3's drain-to-EOF. |
| 2 | 5.2 | `[C]` HIGH console-socket sequence: runc `openpty()`s, dups the slave onto init, sends the master | `libcontainer/init_linux.go:370–416` — `runc init` allocates the pty **inside the container, after `pivot_root`**, and sends the master *up*. Comment: *"consoles are scoped to a container properly (see runc#814)"*. §7.2's diagram puts `openpty` in the supervisor, on the host — the arrangement runc moved away from. |
| 3 | 5.4 | `[R]` HIGH subreaper benefit: "`exec`'d processes whose intermediate parent died still get reaped instead of accumulating zombies **in the container**" | Tested on `host-a` 6.11.0-29. Orphans inside the pidns (including from a `setns`ed process) reparent to the container's PID 1, never to the host subreaper. Kernel forbids cross-namespace reparenting; containerd documents this two sentences after the quote D2 used. |
| 4 | 7.3 | `[R]` HIGH "design so we never re-exec into the container … eliminates the entire CVE class rather than mitigating it" | `execve(user program)` of a path that resolves to `/proc/self/exe` reproduces CVE-2019-5736 with no runtime re-exec involved. crun has D2's exact architecture and still ships the memfd constructor; youki (Rust, no re-exec) calls `pentacle::ensure_sealed()`. |
| 5 | 10.1 | `[C]` HIGH "crun … **no self re-exec, no `/proc/self/exe`, no memfd-clone, no C constructor trick**" → `[R]` HIGH "100% Go artifacts" | crun `src/libcrun/linux.c:7001–7011` — `__attribute__((constructor)) static void libcrun_rexec(void)` calling `ensure_cloned_binary()`; `src/libcrun/cloned_binary.c` is 13,929 bytes. D2's grep was scoped to `container.c`. |
| 6 | 10.3, 11.1#9 | `[R]` HIGH "`sun_path` … this will bite us: `/var/lib/mydocker/containers/<64-hex>/attach.sock` is 105 bytes before the prefix" — the deciding argument for FIFO over `notify.sock` | 105 **is** the whole path; +NUL = 106 ≤ 108. It fits. youki's `chdir`-then-`bind("notify.sock")` removes path length as a factor entirely. Settles D2 §14 Q7, against §10.3. |
| 7 | 4, 6.1 | `[C]` HIGH `external_descriptors` "is how `runc exec` knows what to wire a new process's stdio to" | Only consumers in runc `main` are the state serializer and `criu_linux.go:489`. runc's own comment says *"at checkpoint time"*. `exec` uses the caller's fds. D2 defers CRIU, so `external_fds` in §6.4's state model has no consumer — delete it. |
| 8 | 11.8 | `[C]` LOW "youki appears not to have this [CVE-2019-5736 mitigation]" | `crates/youki/src/main.rs:65–76`, first statement of `main()`. |

### OUTDATED (1)

| # | § | Claim | Changed |
|---|---|---|---|
| 9 | 1.1, 1.4 | `[F]` HIGH "poststart hooks — fail → log warning, continue" / "the container keeps running" | Spec **v1.3.0**, released **2025-11-04**: "MUST generate an error, **stop the container**, and continue the lifecycle at step 12". True for v1.0.2/v1.1.0/v1.2.0, verified at each tag. Implementing from D2's table is non-conformant in the unsafe direction. |

### CORRECTED (9)

| # | § | Correction |
|---|---|---|
| 10 | 1.1 | The spec does not define "exactly four" status values — it names four the value MAY be and adds "Additional values MAY be defined by the runtime". runc reporting `paused` is conformant. Consequence: D2 §6.4's `unknown` is not a legal *reported* status; report `stopped` with a null exit code. |
| 11 | 1.4 | `startContainer`'s `path` MUST resolve in the **container** namespace (it is a container binary, e.g. the image's `ldconfig`). Only `createContainer` is the hard host-binary-in-container-namespace case. D2's stated reason for deferring both is half true. |
| 12 | 2.2 | fds 12/15/17/20 are `O_PATH` (`l---------`), not "write + read ends". |
| 13 | 2.3 | containerd #10727 (2024) and #3334 (2019) are **closed**; only #13849 is open. |
| 14 | 4 | Steps 8 and 9 are swapped: runc does `getPipeFds` **before** `waitForChildExit` (`process_linux.go:857–866`) — which is what D2's own rationale argues for. |
| 15 | 4 | The failure path is inverted: runc's deferred block is `OOMKillCount` → **`terminate()`** → `manager.Destroy()`, with the comment *"Terminate the process to ensure we can remove cgroups."* D2 lists Destroy before terminate, which would fail to `rmdir` the cgroup — and contradicts §9.1's own analysis. |
| 16 | 5.4 | runc sets `PR_SET_CHILD_SUBREAPER` on **both** paths (`utils_linux.go:265`, before the `if !detach` guard), not only foreground. |
| 17 | 8.1 | `clone(2)` is **not** subject to "exactly the same constraint" as `fork` in userspace. glibc `posix/fork.c` locks NSS, the libio list and the **malloc arenas** before cloning and resets them in the child; a raw `clone3` (which D2 specifies) runs none of it. The conclusion gets *stronger*: choosing `clone3` forfeits glibc's atfork protection. Also, Rust threads cannot be cancelled and a panicking `Mutex` unlocks-and-poisons, so D2's stated reason for "never had a second thread" should be the second one it gives (unauditable transitive dependencies), not the first. |
| 18 | 9.1 | "A leaked IP from the pool is a slow exhaustion" is true but records the wrong risk. The dangerous mode is **premature release** — an address returned to the pool while a DNAT rule or a conntrack entry still points at it hands the next container the previous one's traffic. Belongs in §9.3 as an ordering invariant on `Cleanup::ReleaseIp` (plus a conntrack flush), not in §9.1 as a severity row. |
| 19 | 11.8 | runc's `cloned_binary.c` is gone (404 on `main`); the mitigation is Go, at `libcontainer/exeseal/cloned_binary_linux.go`. crun's `src/libcrun/cloned_binary.c` still exists and is still C. `linux.c:6986` has drifted to 7003. Separately, **§15's unresolved-citation #2 is itself wrong**: the only `main_linux.go` cited in the §1–§11 body is *Apptainer's* (§11.7, line 1275), and `apptainer/cmd/starter/main_linux.go` resolves 200. Nothing in §1–§11 rests on runc's non-existent `main_linux.go`. |

### MISCLASSIFIED (4)

| # | § | Issue |
|---|---|---|
| 20 | 1.5 | The "what the spec does NOT mandate — read this list twice" list quotes the error-*reporting* latitude and silently drops the sentence beside it: *"generating an error MUST leave the state of the environment as if the operation were never attempted."* That is a spec **mandate** for failure atomicity, filed under latitude. It upgrades all of §9 from good practice to conformance and D2 never invokes it. |
| 21 | 7.4 / 7.6 vs 11.7 | `-d` is treated as a requirement throughout §7; no stated scope item requires it. §7.4's `[R]` HIGH "the CLI must never be in the ancestry, in any mode" quotes runc `docs/terminals.md`'s foreground *drawback* while omitting the same file's recommendation two paragraphs earlier — *"In general we recommend using foreground … Detached mode is difficult to get right"* — and directly contradicts §11.7's `[R]` HIGH item 1 (*"the container is a process, not a service … foreground execution with the CLI as supervisor"*, listed as principled and adopt-worthy). §7.6 then phases the harder thing first. Only §11.8, 400 lines away, admits this is "a priced choice". |
| 22 | 7.1 | "a startup/reconciliation path" is listed as a cost of having a daemon. §6.4's cases C/D/E *are* that reconciliation, paid per CLI invocation instead of per daemon start; §11.1 says exactly this of Podman. Relocated, not avoided. |
| 23 | 2.3→7.2 | §2.3 Boundary 4 rates the collapse `[R]` MED because "our supervisor process **is** the process that called `clone()`", and says "**§7.2 resolves this**". §7.2 does not — its three processes still have the supervisor calling `clone()`, and its `[R]` MED answers a thread-safety question, not the lifetime question §2.3 raised. Either accept the coupling with an argument, or add the creator fork (which my §5.4 test shows works: init reparents cleanly to a subreaping supervisor). |

### UNSUPPORTED (2 groups)

| # | § | Issue |
|---|---|---|
| 24 | 7.5 | "A daemon, later — because ***nothing*** in the design depends on there being no daemon." There is no push channel; §6.6 concedes the event-stream gap two sections earlier. Adding a daemon later means touching every supervisor, not adding a front-end. The supportable claim is that nothing *forbids* a daemon. |
| 25 | 11.7, 11.8 | Every `[O]` sourced from `old-host` — `APPTAINER_SUID_INSTALL=0`, the absent `starter-suid`, setuid `fusermount3` vs non-setuid `newuidmap`/`newgidmap`, the missing `/etc/subuid` entry, `user.max_user_namespaces`, the `apptainer.conf` value list, `SESSIONDIR`, `~/.apptainer` perms, `/proc/self/cgroup`, the `apptainer exec --help` flag listings, the `[O]` MED `--fakeroot` consequence, and "the whole rootless stack is installed next to podman". **UNSUPPORTED — pending host**: `old-host.example.edu` → NXDOMAIN. The commands D2 quotes inline are sufficient; they need only a reachable host. The Apptainer *source* claims in the same sections are all confirmed. |

### Design gaps found (not claim verdicts)

1. **flock on a removable file** (§6.6/§9.4). The lock lives at `<dir>/lock` and `rm --force` removes the dir. Two processes can hold "the lock" on different inodes. Fix: `fstat`/`stat` inode comparison after acquiring, or lock under a directory that is never removed. D2 already recommends the identical `st_dev`/`st_ino` idiom for `pivot_root` in §11.7.
2. **flock is per open-file-description, shared across `fork`** (§7.2). If the lock fd reaches the supervisor, the CLI's exit does not release it and every later operation on that container blocks for the container's lifetime. `FD_CLOEXEC` (Rust's default) covers it, but the design note should say so.
3. **§7.2's supervisor never takes the lock** in the diagram, while `start` does a locked read-modify-write of the same file. A `start` racing the supervisor's `exited` write resurrects a dead container as `running`. One line fixes it.
4. **No `--init` / in-container reaper** anywhere in §1–§11. Given finding #3 above, a container whose PID 1 does not reap (`nginx`, `bash`) will accumulate zombies from `exec` orphans and no supervisor-side flag can help. `docker-init 0.19.0` is installed on `host-a` and appears in D2's own version block. Belongs on §7.5's "cheap later" list; ~30 lines.
5. **No conntrack flush** in the §9.3 unwind (see correction #18). Deleting an nftables rule does not tear down flows already in conntrack.
6. **`root.path` may be bundle-relative** and "a directory MUST exist at the path declared" (`config.md`) — a spec MUST that §1.3's minimal-config `[R]` treats as a bare string.
7. **`mounts` MUST be applied in listed order** (`config.md`) — an ordering MUST absent from §4, whose entire subject is ordering.

### CONFIRMED (30 substantive checks)

§1.2 the five operations' MUST/error wording · §1.3 the whole required/optional field table · §2.1 the full process tree, re-run, plus the `Setpgid`-not-`Setsid` mechanism in containerd source · §2.3 the shim's five jobs (4 of 5), the `start`/`delete` subcommand contract, the mandated event ordering, the 2.3+ `BootstrapParams` protocol and its version gate · §2.3 boundary 4's `binary_name`-is-a-string and `runc --root` interop, re-run · §3.2 the entire `exec.fifo` mechanism, verbatim · §3.3 `nsexec.c`'s eight sync constants, three stages, `CLONE_PARENT`, `setjmp`/`longjmp` · §3.4 the Go-artifact/kernel-imposed table, row by row · §4 the cgroup-before-fork comment and the OOM-annotation trick, verbatim · §5.1 the three detach mechanisms and `runc create`'s unconditional detach · §5.2 the mode taxonomy, `checkTerminal`'s error strings, the `TIOCSTI` and pipe-hang warnings · §5.4 runc's reap loop verbatim (2048 buffer, SIGCHLD-first, `Wait4(-1,WNOHANG)`, SIGURG) · §5.4 that a container init *does* reparent to a host-side subreaper (now demonstrated, not inferred) · §5.5 `pid_namespaces(7)` signal semantics · §5.5 `cgroup.kill` — D2's `[F]` MED should be **HIGH**, `cgroup-v2.rst:1051` confirms every element and adds `EOPNOTSUPP` in threaded cgroups · §5.5 `cgroup.events`/`populated` and its modify-event · §6.1 `state.json` contents, the atomic temp+rename with no fsync, the start-time PID-reuse guard · §6.2 boltdb as fd 3 and the single-writer flock · §6.3 Docker's key set and `State` object (log now 69.5 MB, up from 27.5 MB) · §6.4 `boot_id` present and the reasoning for it · §6.6 runc's Stat-then-Mkdir TOCTOU (window is ~40 lines, wider than described) and `validateID` · §7.3 single-binary reasoning, `/proc/self/exe` re-exec via `exeseal`, static shim vs dynamic runc/dockerd, `ldd` output · §8.2/§8.3 tokio's threading, youki's tokio-free `libcontainer`, the child-gets-a-raw-fd convergence · §9.2 runc's defer-unwind, `--keep`, and the mount-namespace teardown property · §9.3 crun's `sync_socket_message_s` fixed-size error record · §10.2 every quoted line of youki's `fork.rs`, `container_main_process.rs`, `notify_socket.rs` · §11.1 conmon line-by-line (66, 103, 132, 138, 179, 282) · §11.2 CRI-O's `ContainerStateToDisk` + `NewAtomicFileWriter`, no database · §11.3 containerd's plugin/namespace/boltdb claims · §11.5 gVisor's platform text verbatim, including "systrap replaced ptrace as the default in mid-2023" and ptrace being unsupported · §11.6 Kata's agent-as-supervisor, ttRPC-over-VSOCK, virtio-fs, DAX · §11.7 Apptainer's `starter.c` (1715 lines exactly), `enum goexec`, `fork_ns`/`sigsetjmp`, the weak-chroot assertion, `joinOnly`.

### Verdict on §7's architecture recommendation

**Not refuted. Sound in shape, over-claimed in justification, and it never evaluates the smaller design.**

The shape — one binary, no daemon, one supervisor per container, per-container `flock`, exit code written to disk, sync single-threaded create core — survives every attack I could mount. The per-container-supervisor argument (a shared supervisor reintroduces the daemon's blast radius) is the strongest reasoning in the document, and the sync-core decision is correct and, per correction #17, more urgent than D2 realises.

What does not survive:

- **§7.3's CVE claim is false** and is presented as a design principle ("the second-biggest Rust win"). The memfd self-seal is mandatory regardless.
- **§7.2 does not deliver what §2.3 forward-referenced**, and §2.3's MED rating on collapsing the shim→runc boundary therefore stands unearned rather than resolved.
- **§7.4's central `[R]` HIGH is selectively cited and contradicts §11.7.** The document contains both "never let the CLI be the container's parent" and "the container is a process, not a service — adopt it", each at `[R]` HIGH, 400 lines apart.
- **The premise `-d` is a requirement is a convention, not a stated need.** §7 never prices Phase 1 = foreground-only (two processes, exit code from `waitpid`, no supervisor, no state file for exit codes, no log rotation, no subreaper) against Phase 1 = detached. §11.8 concedes the choice is priced; §7 does not.
- **The simpler architecture is never compared**: `run` always supervises in the foreground, and `-d` is that same code path after `setsid()` with stdio redirected to the log. One code path, two processes, no `supervise` subcommand, no handshake pipe, no second state writer. For a section that claims to select "the simplest technically sound" design, not evaluating it is the gap.

Recommendation: keep the architecture; delete §7.3's CVE paragraph and replace it with "seal `/proc/self/exe` into a memfd at startup, like all three reference runtimes"; resolve §7.4 against §11.7 explicitly and in writing; re-phase so foreground `run` ships before `-d`; and fix the four design gaps above (lock inode check, supervisor takes the lock, `--init`, drop `external_fds`).
