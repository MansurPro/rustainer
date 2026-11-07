# V2 — adversarial verification: runtime architecture (D2 §1–§11, §14)

Verifier V2. Fresh agent; did not write D2. Job is falsification. §12 (Rust substrate)
deliberately **not read** — that is V7's cluster.

---

## Method and evidence base

**Read in full:** `VERIFIER-CONTRACT.md`; `D2-runtime-architecture.md` §1–§11 and §14
(lines 1–1377, 1786–1902). §12–§13 skipped per brief.

**Primary sources fetched this session, all pinned.** Every citation below is to a tag or
a commit SHA, never `main`/`master`, per the contract's recoverability rule.

| Source | Pin |
|---|---|
| OCI runtime-spec `runtime.md`, `config.md` | `v1.2.0`, `v1.2.1`, `v1.3.0`, and `main` @ 2026-10-16 |
| Linux kernel | `torvalds/linux@v6.11` — `kernel/fork.c`, `kernel/user_namespace.c`, `fs/namespace.c` |
| man7 man-pages | `setns(2)`, `unshare(2)`, `flock(2)`, `pipe(7)`, `fifo(7)`, `pid_namespaces(7)`, `prctl(2)`, `wait(2)`, `pidfd_open(2)`, `setsid(2)`, `credentials(7)`, `tcsetpgrp(3)`, `tty_ioctl(4)` |
| runc | `opencontainers/runc@v1.5.2` (current release) and `@v1.2.5` (the droplet's); commit `3aacff695db6` |
| crun | `containers/crun@1.30.1` — `src/libcrun/container.c` (4,746 L), `src/libcrun/linux.c` (7,865 L) |
| youki | `youki-dev/youki@v0.7.0` — `crates/youki/src/main.rs`, `crates/libcontainer/src/process/{fork.rs,container_main_process.rs,container_intermediate_process.rs}`, `crates/libcontainer/src/container/init_builder.rs` |
| conmon | `containers/conmon@v2.2.1` (current release) — `src/{conmon.c,ctr_exit.c,ctrl.c,utils.c}` |
| moby / containerd | `moby/moby@v28.3.1` `vendor/modules.txt`; `containerd/containerd@v1.7.27` and `@v2.4.2` `go.mod` |
| Go runtime | `golang/go@go1.24.4` `src/runtime/os_linux.go` |

**Ran, read-only, on `my-droplet` (`AIKYNETIX-PROD-NY`, prod, kernel 6.11.0-29-generic):**
`uname -r`; `cat /proc/sys/fs/pipe-max-size`, `.../pipe-user-pages-soft`,
`.../pipe-user-pages-hard`; `getconf PAGESIZE`; `runc --version`; `docker version --format`;
`sudo grep -ho '"ociVersion":"[^"]*"' /run/containerd/.../moby/*/config.json`;
`sudo stat -c '%a %n'` and `sudo ls -la` on the runc state root and per-container dirs;
`sudo runc --root /run/docker/runtime-runc/moby list`.
No container, image, mount, firewall rule, cgroup file or systemd unit was touched. No writes
of any kind. `uc-arcc` not needed for this cluster.

**Verdict counts:** 18 CONFIRMED · 18 CORRECTED · 3 DISPUTED · 2 WEAK-EVIDENCE · 4 UNVERIFIABLE-HERE.
Fifteen of the corrections change a `[R]`; they are listed at the end.

---

## Part 1 — §3.4, the keystone: Go artefact or kernel-imposed? — **CONFIRMED, with two corrections and one addition**

D2's whole §7 rests on this table. I attacked it from both ends.

### 1.1 The Go side: the constraint is real, and D2 states it imprecisely — **CORRECTED**

D2 §3.4 row 1 says: *"`setns(2)` is **per-thread**, so you cannot reliably move a Go program
into a namespace."* True but underspecified, and the imprecision hides the stronger fact.
There are **two distinct kernel mechanisms**, and only one is "per-thread":

**(a) `unshare(CLONE_NEWUSER)` from a multithreaded process is a hard `EINVAL`.** Not
"unreliable" — impossible. Mechanism, pinned:

- `torvalds/linux@v6.11/kernel/fork.c#L3272-3273` (`ksys_unshare`):
  `if (unshare_flags & CLONE_NEWUSER) unshare_flags |= CLONE_THREAD | CLONE_FS;`
- `torvalds/linux@v6.11/kernel/fork.c#L3195-3198` (`check_unshare_flags`):
  `if (unshare_flags & (CLONE_THREAD|CLONE_SIGHAND|CLONE_VM)) { if (!thread_group_empty(current)) return -EINVAL; }`

`unshare(2)` states it in prose: *"CLONE_NEWUSER requires that the calling process is not
threaded; specifying CLONE_NEWUSER automatically implies CLONE_THREAD. Since Linux 3.9,
CLONE_NEWUSER also automatically implies CLONE_FS."*

**(b) `setns(fd, CLONE_NEWUSER)` from a multithreaded process is also a hard `EINVAL`**, and
for *two* independent reasons — `torvalds/linux@v6.11/kernel/user_namespace.c#L1351-1355`
(`userns_install`): `if (!thread_group_empty(current)) return -EINVAL;` **and**
`if (current->fs->users != 1) return -EINVAL;`. `setns(2)` ERRORS lists both verbatim:
*"The caller shares filesystem (CLONE_FS) state … and tried to join a new user namespace"* /
*"The caller is multithreaded and tried to join a new user namespace."*

**(c) `setns(fd, CLONE_NEWNS)` — the mount namespace — fails `EINVAL` in *any* multithreaded
glibc or Go program**, and this is the one D2 misses entirely.
`torvalds/linux@v6.11/fs/namespace.c#L5698-5699` (`mntns_install`):
`if (fs->users != 1) return -EINVAL;`. glibc's NPTL and the Go runtime both create threads
with `CLONE_FS`, so `fs->users` is the thread count. Pinned proof for Go:
`golang/go@go1.24.4/src/runtime/os_linux.go#L158-163` —
`cloneFlags = _CLONE_VM | _CLONE_FS | _CLONE_FILES | _CLONE_SIGHAND | _CLONE_SYSVSEM | _CLONE_THREAD`,
with the source comment on `_CLONE_FS` reading `/* share cwd, etc */`.

**(d) The genuinely "per-thread, so unreliable" case is `unshare(CLONE_NEWNS)`**, which
*succeeds* in a multithreaded process but gives only the **calling thread** a private mount
namespace. That is the silent-corruption case, and it is worse than an `EINVAL`: a Go
goroutine that is rescheduled onto another OS thread after `unshare(CLONE_NEWNS)` finds
itself back in the host's mounts with no error anywhere.

`[F]` HIGH — **Corrected statement for §3.4 row 1:** *for a user namespace, `unshare` and
`setns` both return `EINVAL` outright in a multithreaded process; for a mount namespace,
`setns` returns `EINVAL` (via `fs->users != 1`, which any pthread/Go thread trips) while
`unshare` silently succeeds for the calling thread only.* Both failure shapes are
language-agnostic consequences of `CLONE_FS` sharing and `thread_group_empty()`; neither is
specific to Go. **Go's only sin is making multithreading non-optional.** The constraint
D2 names is real and the "Rust inherits: NO" column is correct — but the reason is
`CLONE_FS`/`CLONE_THREAD` sharing, not "setns is per-thread".

### 1.2 The not-Go side: does the spec itself require a created-but-not-started state? — **CONFIRMED, and D2 understates it**

D2 §3.1 says the split is mandated and that *"the purpose of the gap is the hook window"*.
I read `runtime-spec@v1.3.0/runtime.md` end to end. The split is mandated by **four**
independent clauses, only one of which is about hooks:

1. Lifecycle step 2: *"While the resources requested in the `config.json` MUST be created,
   the user-specified program (from `process`) MUST NOT be run at this time."*
2. Create: *"All of the properties configured in `config.json` except for `process` MUST be
   applied. `process.args` MUST NOT be applied until triggered by the `start` operation."*
3. State: `created` is one of four REQUIRED `status` values and must be reportable by
   `state` — so the state is externally observable, not an implementation detail.
4. **Kill: *"Attempting to send a signal to a container that is neither `created` nor
   `running` MUST have no effect … and MUST generate an error."*** — i.e. the spec requires
   that a container which has been created and **not** started still be killable. That is a
   non-hook obligation on the `created` state, and D2 does not mention it.

`[F]` HIGH — **CONFIRMED and strengthened: the `create`/`start` split is spec-mandated
independently of hooks.** Even a runtime that never implements a single hook (which §1.4
recommends for phase 1) must still have a `created` state that is observable via `state` and
signallable via `kill`. D2's §1.4 `[R]` "defer hooks" does **not** let us defer the split.

### 1.3 Does `exec.fifo` exist for the spec or for the shim? — **CONFIRMED: for the spec**

Settled from runc's own history. The FIFO was introduced by
`opencontainers/runc@3aacff695db60634bfb1a2b30c7f5f9e54cd616f`, **2016-06-13**, message:

> *Use fifo for create/start — This removes the use of a signal handler and SIGCONT to
> signal the init process to exec the users process.*

It replaced a `SIGCONT` rendezvous between `runc create` and `runc start`. It predates
containerd's shim-v2 protocol entirely and has nothing to do with any shim. D2 §3.2's
`[R]` HIGH reasoning (*"`SIGSTOP`/`SIGCONT` is racy and visible to the container"*) is
**exactly the historical reason**, independently arrived at. CONFIRMED.

### 1.4 crun: evidence for or against? — **CORRECTED. D2 §10.1 is wrong, and §11.8 already knows it.**

D2 §10.1 `[C]` HIGH: *"**crun never re-execs itself.** … There is **no** self re-exec, no
`/proc/self/exe`, no memfd-clone, **no C constructor trick**."* and calls this *"the decisive
evidence"*.

**That is false.** `containers/crun@1.30.1/src/libcrun/linux.c#L7329-7337`:

```c
/* Protection for attacks like CVE-2019-5736.  */
int ensure_cloned_binary ();
__attribute__ ((constructor)) static void
libcrun_rexec (void)
{
  if (ensure_cloned_binary () < 0)
    { fprintf (stderr, "Failed to re-execute libcrun via memory file descriptor\n");
      _safe_exit (EXIT_FAILURE); }
}
```

crun **does** have a `__attribute__((constructor))` and **does** memfd-clone and re-exec
itself. D2's own §11.8 says so correctly (*"runc and crun ship the same file … in crun at
`src/libcrun/linux.c` as `__attribute__((constructor)) static void libcrun_rexec(void)`"*).
**§10.1 and §11.8 contradict each other inside the same document; §11.8 is right.**

The repair is a distinction D2 collapsed:

| Re-exec purpose | runc | crun | youki | Is it a Go artefact? |
|---|---|---|---|---|
| Re-exec to **enter namespaces** (`runc init`, `nsexec.c` staging, netlink TLV, setjmp/longjmp) | yes | **no** | **no** | **YES — 100% Go** |
| Re-exec once at startup to **seal the binary** (CVE-2019-5736) | yes | **yes**, constructor | **yes**, `pentacle::ensure_sealed()` | **NO — all three languages need it** |

`[C]` HIGH — **Corrected §10.1:** crun has no `crun init` re-exec *for namespace entry*; its
one re-exec is a security mitigation that runc, crun and youki all ship and **that we must
ship too** (§11.8 already says so). The "decisive evidence" for the Go-artefact claim
survives — but only for the first row. D2's §7.3 `[R]` HIGH *"design so we never re-exec
into the container at all … That eliminates the entire CVE class"* is **also partly wrong**:
sealing is not about re-execing *into* the container, it is about a container being able to
overwrite the runtime binary via `/proc/<pid>/exe` and win the *next* invocation. Not
re-execing into the container removes one attack path; it does not remove the class.
See Part 8.1.

### 1.5 The question D2's design actually needs: what does the extra process buy us? — **DISPUTED (D2 is right for the wrong reason)**

D2 concludes: Rust needs "two process boundaries, not four". I attacked that, and found
**three kernel/spec-level reasons** to fork, of which D2 names one and a half.

**Reason 1 — PID namespace (D2 has this, `[F]` HIGH, CONFIRMED).** `pid_namespaces(7)`:
`CLONE_NEWPID` via `unshare`/`setns` affects *subsequently created children only*. The
creator can never be PID 1. Unavoidable, every language.

**Reason 2 — the user-namespace map-writer (D2 has this, CONFIRMED).** Someone outside the
new userns, with `CAP_SETUID`/`CAP_SETGID` there, must write `uid_map`/`gid_map`. D2's
`[R]` MED that the supervisor can be that writer *and* init's parent — so still two
processes — holds, **provided the supervisor uses `clone(CLONE_NEWUSER|…)` rather than
`unshare`**. CONFIRMED by construction.

**Reason 3 — context preservation. D2 does not have this, and crun states it explicitly.**
`containers/crun@1.30.1/src/libcrun/linux.c#L6567-6568`, immediately before the first clone:

> *"Before attempting any `setns()` or `unshare()`, a `clone()` is required to not touch the
> caller context that can be used later on for running hooks."*

That is a **single-threaded C program explaining why it forks anyway**. `unshare` is
irreversible for the calling process: a supervisor that unshares in place loses the host
namespace context it needs afterwards for `createRuntime`/`poststop` hooks, host-side
teardown, and opening host paths. It is the cleanest available answer to the brief's
question — *does the two-stage design buy us anything we would otherwise lose?*

**Verdict `[R]` DISPUTED, in D2's favour on the conclusion:** D2's §7.2 design already
satisfies Reason 3 by accident, because it specifies `clone(CLONE_NEW*)` — a `clone` with
namespace flags creates the namespaces **in the child only**, leaving the supervisor's own
context intact. The two-process shape therefore holds. But D2 states the saving as *"three
of the five exist to work around Go"* while the actual count is **two** (the `runc init`
re-exec and the C constructor/longjmp/TLV machinery — which are one mechanism, not three),
plus one that is a shim-architecture artefact (`runc create` exiting). **Record the rule,
not the count: never `unshare()` in the supervisor; always `clone()` with the flags.**
If that rule is ever broken the two-process shape collapses and Reason 3 forces a third
process back in.

### 1.6 One more row D2's §3.4 table gets wrong — **CORRECTED**

Table row: *"`PR_SET_CHILD_SUBREAPER` in the supervisor — Needed because the process that
`clone()`d init exits. **Conditionally NO**."* The "conditionally NO" is right; the
alternative D2 never names is the one runc and youki actually use: **`CLONE_PARENT`**.
`torvalds/linux@v6.11/kernel/fork.c#L2502-2512`: *"CLONE_PARENT re-uses the old parent"* —
`p->real_parent = current->real_parent`. Subreaper is *reparenting after the fact*;
`CLONE_PARENT` is *never losing the parent in the first place*. The second is strictly
better where it is available. Two constraints on it, both pinned, both worth writing down
now because they bite later:

- `fork.c#L2162-2164`: `CLONE_PARENT` returns `EINVAL` if the caller has
  `SIGNAL_UNKILLABLE` — i.e. **a namespace's init cannot create siblings** ("prevent global
  and container-inits from creating siblings"). An intermediate that has already *entered*
  a new PID namespace cannot `CLONE_PARENT`. (It can if it has only `unshare`d it, since
  `unshare(CLONE_NEWPID)` does not move the caller.)
- `fork.c#L3043-3045` (`clone3_args_valid`): `clone3` rejects `CLONE_PARENT` together with a
  non-zero `exit_signal`. This is exactly youki's quoted comment, now pinned to the kernel
  rather than to youki.

---

## Part 2 — §7.2, the recommended architecture, attacked as a design — **3 defects found, shape survives**

The shape (CLI exits → detached supervisor → cloned init) is sound and I could not break it.
Four specific attacks, four specific answers; three of them change a line of the diagram.

### 2.1 (a) Is `PR_SET_CHILD_SUBREAPER` doing anything, or is it cargo-culted from conmon? — **CORRECTED: cargo-culted *for the container init*, load-bearing *for `exec`*.**

**Why conmon needs it.** `containers/conmon@v2.2.1/src/conmon.c#L131-140`, in order:
`setsid();` then `set_subreaper(true)` with the comment *"Set self as subreaper so we can
wait for container process and return its exit code."* conmon needs it because conmon does
**not** create the container: it forks a child that `execv`s `crun create`
(`conmon.c#L228-233`), and `crun create` forks again, detaches, clones the init and
**exits** (`crun@1.30.1/src/libcrun/container.c#L3014-3065` — the forked create-side process
ends at `exit (ret ? EXIT_FAILURE : 0)`). The container init is therefore *orphaned by
construction*, and only a subreaper catches it. **That is conmon's situation, and it is not
ours.**

**Our situation.** In D2's §7.2 the supervisor `clone()`s init directly. A direct child is
never reparented while its parent lives, so for init's own exit status the flag is a no-op.

**But it is not dead weight, for two reasons the brief asks about and D2 only half-states.**

`[F]` HIGH — `pid_namespaces(7)`, "Adoption of orphaned children": *"When a child process
becomes orphaned, it is reparented to the 'init' process **in the PID namespace of its
parent** (unless one of the nearer ancestors of the parent employed the `prctl(2)`
`PR_SET_CHILD_SUBREAPER` command…). Note that … this may be the 'init' process in the PID
namespace that is the parent of the child's PID namespace, rather than the 'init' process in
the child's own PID namespace."*

Read that against our design:

1. **Processes inside the container's PID namespace never reach us.** If a container process
   is orphaned, its parent was in the container's pidns, so it reparents to the container's
   PID 1. Our subreaper flag is irrelevant to them. (And `pid_namespaces(7)`: when a pidns
   init dies the kernel `SIGKILL`s the whole namespace, so nothing survives to be adopted.)
   **D2 §5.4's "`mydocker exec`'d processes whose intermediate parent died still get reaped
   instead of accumulating zombies *in the container*" is wrong** — zombies inside the
   container are the container PID 1's problem and no host-side flag fixes them.
2. **Processes we fork that live in the container's pidns *do* reach us** — because the rule
   keys on the **parent's** namespace, and our parent-side helper is in the host pidns. An
   `exec` implemented as *supervisor → fork → setns(pidns) → fork → execve* leaves the
   grandchild orphaned into the **host's** PID 1 when the middle process exits, unless the
   supervisor is a subreaper. **This is the real justification and D2 should state it this
   way.**
3. **`--pid=host` containers.** If the user asks for the host PID namespace (a feature we
   will eventually want), every process in the container is in our pidns and the subreaper
   flag becomes the whole mechanism.

`[R]` HIGH — **Keep the flag — one syscall, no downside — but delete the stated reason.**
Correct reasoning: it covers `exec` descendants and host-pidns containers, **not** the
container init and **not** zombies inside the container. And note what conmon does on the
way out, which D2 missed: `conmon@v2.2.1/src/ctr_exit.c#L172-174` *turns the flag off again*
before double-forking the exit command — *"We don't want the exit command to be reaped by
the parent conmon as that would prevent double-fork from doing its job."* A subreaper is not
a set-and-forget property.

`[R]` HIGH — **The mechanism D2's §3.4 table should have paired it with is `CLONE_PARENT`**
(Part 1.6). The day we add an intermediate process — which rootless forces — the intermediate
must clone init with `CLONE_PARENT` so init's parent stays the supervisor, exactly as runc
(`nsexec.c`) and youki (`container_intermediate_process.rs#L211`) both do. `CLONE_PARENT`
is deterministic; subreaping is a fallback.

### 2.2 (b) `setsid()` — **CONFIRMED for signals, CORRECTED for the pty path: it *does* conflict with §5.2**

`[F]` HIGH, `setsid(2)` + `credentials(7)`:

- `setsid()` creates a new session and process group and the caller **has no controlling
  terminal**. It fails `EPERM` if the caller is already a process-group leader — so it must
  be called in the forked child, which §7.2 step 4 does. *(Trap: `mydocker supervise` run
  directly from an interactive shell **is** a pgrp leader and `setsid()` will `EPERM`.
  Tolerate the error or always re-fork.)*
- Terminal hangup sends `SIGHUP` to **the session leader of the session that terminal
  controls**. A `setsid()`'d supervisor is in a different session with no controlling
  terminal, so the invoking terminal's hangup cannot reach it. **D2 §5.1's `[R]` MED is
  CONFIRMED**, with the precise mechanism.
- *"If a process that is a session leader terminates, then a `SIGHUP` is sent to each process
  in the foreground process group of the controlling terminal."* Our supervisor has no
  controlling terminal, so its death `SIGHUP`s nobody. Good.

**Now the conflict D2 does not see.** `credentials(7)`: *"The controlling terminal is
established when the session leader first opens a terminal (**unless the `O_NOCTTY` flag is
specified**)… A terminal may be the controlling terminal of at most one session."* And
`TIOCSCTTY(2const)`: *"The calling process must be a session leader and **not have a
controlling terminal already**… If this terminal is already the controlling terminal of a
different session group, then the ioctl fails with `EPERM`."*

So: `setsid()` makes the supervisor a session leader **with no ctty** — which is exactly the
state in which the next tty it opens is silently claimed as its controlling terminal. §7.2's
supervisor line reads `create cgroup; create pipes; (optional) openpty`. If the supervisor
calls `openpty()` and anything in that path opens the **slave** without `O_NOCTTY`, the
supervisor owns the pty, and the container's `setsid() + ioctl(TIOCSCTTY)` then fails
`EPERM`. (glibc's `openpty(3)` passes `O_NOCTTY`; a hand-rolled `open(ptsname(m))` or a thin
Rust pty crate may not. This is a real, silent, env-dependent bug.)

### 2.3 The bigger §5.2 correction: **runc does not allocate the pty where D2 says it does** — **CORRECTED**

D2 §5.2's protocol sketch is:

```
runc:   openpty()  -> (master, slave)
runc:   dup slave onto container init's 0/1/2 ; ioctl(TIOCSCTTY) in the container
runc:   connect(path) ; sendmsg(SCM_RIGHTS, [master])
```

`runc@v1.5.2/docs/terminals.md` (New Terminal): *"`runc` will create a brand-new 'console'
(or more precisely, a new pseudo-terminal using **the container's namespaced
`/dev/pts/ptmx`**)"*. And the source confirms the actor:
`runc@v1.5.2/libcontainer/standard_init_linux.go#L96-106` calls `setupConsole(...)` then
`system.Setctty()` — **inside the container init**, "*before we finalize the rootfs, but
after we've given the user the chance to set up all of the mounts they wanted*". The guard
`checkPtmxHandle` (`runc@v1.5.2/libcontainer/console_linux.go#L22-42`) *verifies* the handle
is on a real `DEVPTS_SUPER_MAGIC` mount with inode 2 — i.e. it specifically insists on a
devpts `ptmx`.

`[C]` HIGH — **Corrected protocol:**

```
caller:    socket(AF_UNIX, SOCK_STREAM); bind(path); listen()
caller:    exec runtime create --console-socket path …
init:      (inside the container mnt ns, after /dev/pts is mounted)
           open(container's /dev/pts/ptmx)  -> master
           TIOCGPTPEER (fallback: TIOCGPTN + open /dev/pts/N)  -> slave
           sendmsg(console_socket, SCM_RIGHTS, [master])
           dup2(slave, 0/1/2);  setsid();  ioctl(TIOCSCTTY)
runtime:   exits
caller:    recvmsg() -> master.  Sole owner from here on.
```

`[R]` HIGH — **Design consequence for §7.2: the supervisor must never call `openpty()`.**
Allocating the pty on the host's `/dev/ptmx` (a) hands the container a host pts device that
then has to be bind-mounted in, defeating the container's own `devpts` instance, (b) exposes
the supervisor to the `TIOCSCTTY` collision in 2.2, and (c) makes the pty's lifetime depend
on a host mount rather than the container's. **The pty is allocated by the container init
from the container's `/dev/pts/ptmx`; the supervisor only ever receives the master over a
socketpair.** Replace §7.2's `(optional) openpty` with `(optional) socketpair for the console
master`. One more detail worth copying: in new-terminal mode all three stdio fds are the
*same* file, so **stdout/stderr cannot be distinguished** — which is why Docker's
`stdcopy` framing only exists for `terminal: false` (§2.3, confirmed below).

### 2.4 (c) The supervisor dies, the container lives — **CONFIRMED, with the mechanism named**

- **Does the container keep running?** Yes. It is in its own namespaces and cgroup; nothing
  in the kernel ties it to the supervisor. The *only* thing that would is
  `PR_SET_PDEATHSIG`, and `PR_SET_PDEATHSIG(2const)`: the setting *"is cleared for the child
  of a `fork(2)`"*, so init would have to set it on itself. **Do not** — the caveat on the
  same page is that the signal fires on termination of *"the parent **thread**"*, so a
  supervisor that ever grew a second thread would kill its container when that thread exited.
  (conmon uses pdeathsig only on the short-lived process that `execv`s the runtime —
  `conmon.c#L232` — never on the container.)
- **Who reaps PID 1?** Per `pid_namespaces(7)` above: the nearest subreaper ancestor of the
  supervisor, else host PID 1. On a systemd host that is systemd, which `wait()`s and
  discards. The exit status is **gone**.
- **Who holds the exit status?** Nobody — unless it was already written to disk. This is
  containerd #13849's class and D2 §2.3's `[R]` HIGH conclusion (*"write the exit status to
  disk before publishing it anywhere"*) is **CONFIRMED and is the single most important
  recommendation in D2**.
- **What the mature ones do:** containerd re-dials the shim's `address` file and, failing
  that, invokes the shim's `delete` subcommand to clean up (`docs/runtime-v2.md`). conmon has
  no recovery path at all — if conmon is `SIGKILL`ed, the `exit` file is never written and
  the `--exit-command` never runs, so Podman reconciles on the next CLI invocation and
  reports what it can. **Neither can recover the exit code.** Our `unknown` status (§6.4
  case C/D) is therefore the honest state of the art, not a shortcut.

`[F]` HIGH — **one thing changed since D2 was written and it partially falsifies D2 §2.3's
framing.** D2 asserts *"the exit status of a supervised process is state that exists in
exactly one place — the reaping parent's memory"*. Since **Linux 6.15** that is no longer
strictly true: `ioctl(pidfd, PIDFD_GET_INFO, &info)` with `PIDFD_INFO_EXIT` returns
`__s32 exit_code` for the process the pidfd refers to —
`torvalds/linux@v6.18/include/uapi/linux/pidfd.h` (`PIDFD_INFO_EXIT` absent at `v6.14`,
present at `v6.15`; `PIDFD_GET_INFO` itself absent at `v6.12`, present at `v6.13`). The
status then lives in **any** held pidfd, not only in the parent. It still does not survive
the holder's death, so the disk write remains mandatory — but it means a non-parent that
opened a pidfd *before* the exit can report the exit code truthfully. `my-droplet` is
6.11, so this is a forward-looking capability, not one we can test here.

### 2.5 (d) `exec.fifo` vs pipe/eventfd/socketpair when both ends are ours — **CONFIRMED: keep the FIFO, and the property is "openable by name"**

The property a FIFO has that a pipe, an eventfd and a socketpair do not:

`[F]` HIGH — **a FIFO is a rendezvous between two processes that are not related and share no
fd.** `fifo(7)`: *"The FIFO must be opened on both ends (reading and writing) before data can
be passed. Normally, opening the FIFO blocks until the other end is opened also."* Opening
`O_WRONLY` **blocks until a reader opens**; that *is* the synchronisation, and it needs no
inherited descriptor, no `SCM_RIGHTS` hand-off and no live peer at create time. `mydocker
start` is a **fresh process invocation** that is not a descendant of the supervisor; it
cannot be handed a pipe fd. A pipe/eventfd/socketpair would force `start` to become an IPC
client of the supervisor — which is exactly the dependency D2's design is built to avoid
(*"no IPC with the supervisor at all"*). **The FIFO is the right mechanism and D2's §7.2
best-property claim survives.**

Two corrections to the surrounding reasoning:

`[C]` CORRECTED — **D2 §3.2 quotes runc code that has not existed since 2019.** The quoted
`handleFifo(path, pid)` / `readFromExecFifo` shape and the error string *"exec fifo is empty:
container init did not signal execve readiness (process died before writing, or fifo already
consumed)"* appear in **neither `runc@v1.2.5` nor `runc@v1.5.2`** — I grepped both; the
string does not exist in runc. Real code, `runc@v1.5.2/libcontainer/container_linux.go#L233-301`:
a goroutine does the blocking `open(O_RDONLY)` while the main loop wakes every **100 ms** and
`stat`s the init pid, bailing out if it is dead or a zombie; on that path it retries the open
with `O_NONBLOCK` to pick up a byte the dead process may already have written. The guard was
added by `opencontainers/runc@8541d9cf3d8b`, *"Fix race checking for process exit and waiting
for exec fifo"*, 2019-12-18.

`[R]` HIGH — **this is a real defect in §7.2.** D2 specifies: *"`mydocker start <id>` =
flock, open `exec.fifo` `O_RDONLY`, read one byte, set `status=running`, unlock. **That's
it**"*. If the container init died between `create` and `start` — a trivially reachable case,
e.g. the user's binary does not exist, or the cgroup OOM-killed it — **no writer will ever
open the FIFO and `start` blocks in `open()` forever**, holding the per-container `flock`,
which then wedges `kill`, `delete` and `rm` on that container too. The lock makes it worse
than it is for runc.

**The lazy correct fix, better than runc's:** open the read side `O_RDONLY|O_NONBLOCK`
(which `fifo(7)` guarantees *"succeeds even if no one has opened on the write side yet"*),
then `poll()` that fd **together with a `pidfd_open()` of `init_pid`**. `POLLIN` on the FIFO
means started; `POLLIN` on the pidfd means init died first. No goroutine, no 100 ms polling,
no PID-reuse window. `conmon@v2.2.1/src/ctrl.c#L289-293` does the same `O_RDONLY|O_NONBLOCK`-
first-then-`O_WRONLY` ordering for its control FIFO, for the same reason.

`[C]` CORRECTED — **the FIFO's `0622` mode is only safe because of a permission D2 omits.**
D2 §6.1 records the `0711` state dir and the `0622` world-writable FIFO and calls the pairing
"intentional". The pairing is incomplete: the thing that actually contains it is the
**`0700` state root**. `runc@v1.5.2/libcontainer/factory_linux.go#L45`:
`os.MkdirAll(root, 0o700)`; line 91: `os.Mkdir(stateDir, 0o711)`.
`[O]` HIGH — verified live on `my-droplet`: `stat -c '%a %n'` → `700
/run/docker/runtime-runc/moby`, `711` on every per-container dir, and `state.json` is `600`.
Without the `0700` root, any local user who knows a container id could `open(exec.fifo,
O_RDONLY)` and start a created container, or `O_WRONLY` and unblock its init. **Write the
root as `0700` explicitly; it is the only thing standing between a world-writable FIFO and an
unauthorised `start`.**

---

## Part 3 — §14.1 item 1, the `poststart` semantics change — **CONFIRMED exactly, and one thing D2 missed**

Verified independently by fetching and diffing the pinned revisions myself.

```
$ diff runtime-spec@v1.2.0/runtime.md runtime-spec@v1.3.0/runtime.md
73c73
<     If any `poststart` hook fails, the runtime MUST [log a warning](#warnings), but the
      remaining hooks and lifecycle continue as if the hook had succeeded.
---
>     If any `poststart` hook fails, the runtime MUST [generate an error](#errors), stop the
      container, and continue the lifecycle at step 12.
```

`[F]` HIGH — **CONFIRMED and stronger than D2 claimed.** D2 says this is the only line that
differs *in the lifecycle*. In fact it is the only line that differs **in the entire
`runtime.md` file** — 150 lines, one changed line, nothing else. Step 13 (`poststop`) still
says *"MUST log a warning"* in both. v1.3.0 was published **2025-11-04** (GitHub releases
API). CONFIRMED.

**Three things D2 got wrong or missed around it:**

1. `[C]` CORRECTED — **there is a `v1.2.1` (published 2025-02-27) that D2 never mentions.**
   Its `runtime.md` is **byte-identical to v1.2.0** (`diff` empty), so the lifecycle claim is
   unaffected — but the version matters for item 10, below, because v1.2.1 is what the
   ecosystem actually emits.
2. `[F]` CONFIRMED — **no v1.3.1 or later exists.** The full tag list ends at `v1.3.0`;
   nothing has been released in the ~12 months since. D2's seven-week drift worry does not
   materialise for the released spec.
3. `[F]` CORRECTED — **`config.md` also changed, and the change touches §1.4's hook table.**
   `diff config.md@v1.2.0 config.md@v1.3.0`, hooks table row:
   - v1.2.0: `prestart | runtime | After the **start** operation is called but before the
     user-specified program command is executed.`
   - v1.3.0: `prestart | runtime | **During the create operation**, after the runtime
     environment has been created and before the pivot root or any equivalent operation.`

   D2's §1.4 table says *"after env created, before `createRuntime`"*, which matches v1.3.0.
   So **§1.4 is already written against v1.3.0 while §1.1's step 9 is written against
   v1.2.0** — the document straddles two revisions. (The other `config.md` deltas are
   additive and harmless: `process.execCPUAffinity`, a z/OS `process` block, `freebsd`
   becoming OPTIONAL, link hygiene.)

`[F]` MED — **and one live change on `main` that nobody has released yet but that lands on a
`[F]` HIGH claim of D2's.** `diff runtime.md@v1.3.0 runtime.md@main` (main as of
2026-10-16):

```
< Attempting to `delete` a container that is not `stopped` MUST ... generate an error.
---
> Attempting to `delete` a container that is neither `created` nor `stopped` MUST ...
>   generate an error.
> Attempting to `delete` a container that is `created` MAY either have no effect ...
>   or terminate the container process and then delete the container.
```

D2 §9.4 opens with `[F]` HIGH *"`delete` 'MUST generate an error' if the container isn't
`stopped`"*. True for every released revision; **about to stop being true**. `delete` on a
`created` container (create succeeded, `start` never called — the exact state a failed `run`
leaves behind) becomes legal, and the runtime MAY kill-then-delete. `[R]` — implement
`delete` on `created` as *kill init, reap, then delete*, because that is the behaviour the
next spec revision blesses and the one users expect; do not hard-error on it.

---

## Part 4 — §14 item 10, which spec revision do we claim? — **CORRECTED. The premise is factually wrong and the question is less important than D2 thinks.**

D2: *"target **1.2.0** for phase 1 (it is what the reference runtime on our own test host
implements…)"*.

`[O]` HIGH — **the bundles on that very host declare `1.2.1`, not `1.2.0`.** On `my-droplet`:

```
$ sudo grep -ho '"ociVersion":"[^"]*"' /run/containerd/io.containerd.runtime.v2.task/moby/*/config.json | sort -u
"ociVersion":"1.2.1"
$ runc --version
runc version 1.2.5 … spec: 1.2.0
```

The writer is moby, not runc: `moby/moby@v28.3.1/vendor/modules.txt` →
`github.com/opencontainers/runtime-spec v1.2.1`. So on the single host D2 reasons from, the
**config says 1.2.1 and the runtime says 1.2.0, and nothing complains** — which is the actual
finding:

`[C]` HIGH — **no reference runtime validates `ociVersion` at all.**
`runc@v1.5.2/libcontainer/specconv/spec_linux.go#L622` simply **overwrites** it:
`config.Version = specs.Version` — the bundle's declared version is read into the struct and
then discarded in favour of runc's own vendored constant. There is no comparison, no range
check, no error. crun does not even round-trip it: `crun@1.30.1/src/libcrun/container.c#L3232`
hardcodes `const char *const OCI_CONFIG_VERSION = "1.0.0";` for `crun state` output and
`#L655` hardcodes `"0.2.0"` in the seccomp-notify payload, while the spec's own `state`
example in `runtime.md` shows `"ociVersion": "0.2.0"`. **The field is decorative.**

`[C]` HIGH — what the ecosystem emits **today** (all pinned):

| Producer | vendored runtime-spec | effect |
|---|---|---|
| runc **1.5.2** (current release; 1.6.0-rc.1 is out) | `v1.3.0` | `runc --version` → `spec: 1.3.0` |
| runc 1.2.5 (`my-droplet`) | `v1.2.0` | `spec: 1.2.0` |
| containerd **v2.4.2** (current) | `v1.3.0` | writes `ociVersion: 1.3.0` |
| containerd v1.7.27 (`my-droplet`) | `v1.1.0` | — |
| moby **v28.3.1** (`my-droplet`) | `v1.2.1` | writes `ociVersion: 1.2.1` |
| crun 1.30.1 | n/a (C) | declares `features` range `1.0.0 … 1.1.0+dev`; emits `1.0.0` |

`[R]` DISPUTED — **"match the runc on one Ubuntu box" is not a sound basis, and D2's own
stated reason for it is void.** Its second reason — *"it keeps the simpler `poststart`
semantics we are not implementing anyway"* — is self-cancelling: a semantic we do not
implement cannot be a reason to pick a revision. **Target `1.3.0`** (the current released
revision, what the current runc and containerd both vendor), accept **any** `ociVersion` that
parses as SemVer with major `1`, and record the delta rather than the pin. Concretely:

1. Emit `"ociVersion": "1.3.0"` in `state` output and in any bundle we generate.
2. On read, **do not reject** on version — log at WARN if major ≠ 1, and otherwise ignore the
   field, exactly as runc does. Rejecting is a compatibility liability with zero safety
   benefit, since the field is not load-bearing for any producer.
3. The thing that actually needs a decision is **which fields** we honour, refuse, or warn
   on — which §1.3's `[R]` HIGH already frames correctly. That is the real "revision" story.

`[R]` HIGH — a related phasing correction: §7.6 Phase 3 is *"be a drop-in `runc`"*. If that
is ever the goal, note the moving target: runc's current line is **1.5.2 / 1.6.0-rc.1**, not
the 1.2.x D2 observed, and `runc --version` is parsed by callers.

---

## Part 5 — §5.4, exit-status collection when the parent is gone — **CONFIRMED as to the experiment, CORRECTED as to what it proves**

D2's `[O]` HIGH experiment on `my-droplet` (supervisor / child / grandchild, status 7 lost
without the flag, recovered with it) is a valid demonstration of `PR_SET_CHILD_SUBREAPER`.
I did not re-run it (production host, and it was a `/tmp` script). What it does **not** prove
is the thing D2 uses it for — *"without `PR_SET_CHILD_SUBREAPER` in the supervisor, the
container's exit code goes to PID 1 and is lost forever."* That is true of **runc's** shape
(a create-side process that exits, orphaning init) and false of **D2's own §7.2 shape** (the
supervisor is init's direct parent). See Part 2.1.

**The precise kernel rule, which neither D2 nor `prctl(2)` states clearly.**
`torvalds/linux@v6.11/kernel/exit.c#L625-658`, `find_new_reaper()`:

```c
if (father->signal->has_child_subreaper) {
        unsigned int ns_level = task_pid(father)->level;
        /* Find the first ->is_child_subreaper ancestor in our pid_ns. ...
         * the exiting parent could be injected by setns() + fork(). */
        for (reaper = father->real_parent;
             task_pid(reaper)->level == ns_level;        /* <-- never crosses a pidns */
             reaper = reaper->real_parent) { ... }
}
return child_reaper;      /* = init of the DYING PARENT's active pid namespace */
```

`[F]` HIGH — **a subreaper only adopts an orphan whose *dying parent* was in the subreaper's
own PID namespace.** Otherwise the orphan goes to the init of the dying parent's namespace.
containerd documents the practical consequence independently —
`containerd/containerd@v2.4.2/docs/runtime-v2.md#L583-588`: *"Assume that container is in
X-namespace and P in root-namespace `setns` into X-namespace. P forks child C. The child C
forks a grandchild G and exits. **The G will be reparented to X instead of P's reaper.**"*

Mapped onto our design, this gives three clean statements:

| Who dies | Who adopts the orphan | Does our subreaper flag matter? |
|---|---|---|
| nothing (normal) — init is a direct child | n/a | no |
| an `exec` helper in the **host** pidns, whose child runs in the container's pidns | the supervisor (same `ns_level`) | **yes — this is the only case** |
| a process **inside** the container's pidns | the container's PID 1 | no, never |
| the supervisor itself | host PID 1 / the supervisor's own nearest subreaper ancestor | exit status **lost** |

### 5.1 Can a pidfd give you the exit status of a non-child? — **CORRECTED: not with `waitid`, but yes since Linux 6.15 with an ioctl**

- `waitid(P_PIDFD, fd, …)`: `wait(2)` says *"Wait for **the child** referred to by the PID
  file descriptor"*. `waitid` is a child-only interface; a pidfd for a non-child yields
  `ECHILD`. **So D2 §5.4's `[R]` MED pairing of `pidfd_open` with `waitid(P_PIDFD)` buys
  nothing a parent did not already have.**
- `pidfd_open(2)`: the fd *"becomes readable (`EPOLLIN`)"* when the task terminates and
  produces `EPOLLHUP` when it is reaped — **notification works for any process**, which is
  genuinely useful (it is PID-reuse-proof and composes with `poll`).
- `[F]` HIGH — **the exit code itself became retrievable in Linux 6.15.**
  `ioctl(pidfd, PIDFD_GET_INFO, &info)` with `mask |= PIDFD_INFO_EXIT` fills
  `__s32 exit_code`. Pinned: `PIDFD_GET_INFO` absent at `torvalds/linux@v6.12`, present at
  `@v6.13`; `PIDFD_INFO_EXIT` absent at `@v6.14`, present at `@v6.15`
  (`include/uapi/linux/pidfd.h`). You must hold the pidfd from **before** the exit.

`[R]` HIGH — **what this changes for us:** the supervisor should `pidfd_open(init_pid)`
immediately after the clone (or ask for `CLONE_PIDFD` in the `clone3` args and get it for
free, with no race at all) and keep it. It then has (a) a PID-reuse-proof liveness test,
(b) a `poll`-able death notification that composes with the exec-FIFO wait in Part 2.5, and
(c) on kernel ≥ 6.15, a second independent source for the exit code. **It does not change
the conclusion**: the pidfd dies with its holder, so the on-disk write stays mandatory and
D2 §6.5's `fsync`-on-terminal-status recommendation stands.

### 5.2 What the mature supervisors persist, and when — **CONFIRMED, and D2's §6.5 ordering is independently corroborated**

`conmon@v2.2.1/src/conmon.c#L470-529`, in this exact order:

1. `check_cgroup2_oom()` — read the OOM counter
2. `drain_stdio()` — drain the container's stdio pipes
3. `sync_logs()` — flush the log
4. compute status (`ctr_exit.c#L119-126`: `WIFEXITED → WEXITSTATUS`; `WIFSIGNALED → 128 +
   WTERMSIG`; otherwise `-1`)
5. `close_other_fds()` — *"Podman injects some fd's into the conmon process so that exposed
   ports are kept busy while the container runs. Close them before we notify the container
   exited, so that they can be reused immediately."*
6. write `<persist-dir>/exit` **and** `<exit-dir>/<cid>` — *"Writing to this directory helps
   if a daemon process wants to monitor all container exits using `inotify`."*

That is D2 §6.5's steps 2 → 3 → 4, arrived at independently by a different project.
**CONFIRMED.** `g_file_set_contents()` is glib's atomic write (temp + `rename`), matching
D2 §6.1's `[R]` HIGH.

Two details worth stealing that D2 does not have:

- `[R]` MED — **two exit records, not one**: a per-container one and an entry in a shared
  flat directory, so a watcher can `inotify` one directory instead of N. If we ever build
  `events` (§14 item 16), that directory **is** the event stream for exits, for free.
- `[R]` HIGH — **close host resources before announcing the exit**, not after. We will hit
  exactly conmon's case the moment D4 publishes ports: if we hold the listening socket to
  reserve the host port, releasing it after we write `exited` gives a window in which a
  racing `run -p` on the same port fails.
- `[C]` note — conmon flattens "killed by signal N" into `128+N`, which loses the
  distinction. D2 §6.4's `exit_code: Option<i32>` is better; keep a separate
  `signal: Option<i32>` / `oom_killed` rather than re-encoding.

**Verdict on D2 §5.4's title claim "the mechanism, proven":** the mechanism is real and the
experiment is sound, but *"proven"* overstates what the experiment establishes for **our**
architecture. Downgrade to: *proven for an orphaning create-side process (runc/conmon's
shape); not applicable to a supervisor that is the direct parent; load-bearing only for
`exec`.*

---

## Part 6 — §5.2 console socket / §5.3 attach and logs — **CONFIRMED except for the pty allocator (corrected in Part 2.3)**

`[C]` HIGH — **CONFIRMED verbatim against `runc@v1.5.2/docs/terminals.md`:**

- *"After `runc` exits, the only process with a copy of the pseudo-terminal master file
  descriptor is whoever read the file descriptor from the socket."* ✔
- The `--console-socket` + `SCM_RIGHTS` protocol and its four steps ✔
- `checkTerminal()`'s enforced pairing (`cannot allocate tty if runc will detach without
  setting console socket` / `cannot use console socket if runc will not detach or allocate
  tty`) ✔ — still present at `runc@v1.5.2/utils_linux.go#L328`.
- The detached-pass-through danger: *"A malicious container could take advantage of
  TTY-specific `ioctls` like `TIOCSTI` to fake input into the **host** shell"*; output
  interleaving; *"Any input to `stdin` will be non-deterministically split and given to either
  the container or the shell (because both are blocked on a `read(2)` of the same FIFO-style
  file descriptor)"*; and the `issue-1721` note *"using detached pass-through will result in
  the container hanging if the `stdout` or `stderr` is a pipe"*. ✔ All verbatim.
- **D2 §5.2's `[R]` HIGH "never give a container a file descriptor that refers to the
  invoking terminal" is CONFIRMED and is the single most important security line in §5.**

`[C]` CORRECTED — **who allocates the pty and which end crosses the socket.** See Part 2.3:
the **container init**, from the **container's `/dev/pts/ptmx`**, allocates it; the
**master** crosses the socket outward; the slave is `dup2`'d onto the init's 0/1/2 and
`TIOCSCTTY`'d there. D2's sketch attributes `openpty()` to the parent, which is wrong and
produces three real bugs if copied.

`[C]` CONFIRMED — **`terminal: false` + detached** is `inheritStdio(process)`: the container
gets the runtime's *actual* 0/1/2. For us that means the supervisor's pipes, which is what
§7.2 specifies. ✔

### §5.3 — **CONFIRMED, one citation repaired**

`[C]` HIGH — moby's `stdcopy` framing: `stdWriterPrefixLen = 8`, `stdWriterFdIndex = 0`,
`stdWriterSizeIndex = 4`, `Stdin=0, Stdout=1, Stderr=2, Systemerr=3`. CONFIRMED at
**`moby/moby@v28.3.1/pkg/stdcopy/stdcopy.go#L16-31`**.
`[C]` CORRECTED — D2 cites `api/pkg/stdcopy/stdcopy.go` on `master`. That path **404s at
v28.3.1**; the file is at `pkg/stdcopy/`. This is the document's *only* recorded full URL
(§15.5 says so) and it has already drifted — a tidy demonstration of the contract's pinning
rule. Use the pinned path above.

`[C]` CONFIRMED — the "hold both ends of the FIFO" trick, independently, in conmon:
`conmon@v2.2.1/src/ctrl.c#L260-293` — *"Open a dummy writer to prevent getting flood of
POLLHUPs when last writer closes"*, and `setup_fifo()` opens the read half
`O_RDONLY|O_NONBLOCK|O_CLOEXEC` **first**, then the write half `O_WRONLY|O_CLOEXEC`. The
ordering is not decoration: it is the only order that cannot block, per `fifo(7)`. D2 §5.3's
`[R]` HIGH ("hold both ends") is CONFIRMED; **add the ordering**.

`[C]` CONFIRMED — conmon's 1-byte stream tag over `SOCK_SEQPACKET` vs Docker's 8-byte framed
stream, and D2's `[R]` HIGH preference for SEQPACKET + 1-byte tag. The reasoning (SEQPACKET
preserves message boundaries so no length field is needed) is correct. Note the one cost
D2 omits: `SOCK_SEQPACKET` has a **maximum datagram size**, so an attach implementation must
chunk; Docker's length-prefixed framing does not care. Not a reason to change, but write it
down before someone debugs a truncated line.

---

## Part 7 — §6.6 concurrency and §9 error atomicity — **CONFIRMED with one serious new defect in §7.2, and `rollback.json` DISQUALIFIED**

### 7.1 runc's TOCTOU — **CONFIRMED, and it is still there three years later**

`runc@v1.5.2/libcontainer/factory_linux.go`: line 52 `if _, err := os.Stat(stateDir); err ==
nil { return nil, ErrExist }` … line 91 `if err := os.Mkdir(stateDir, 0o711); err != nil`.
Stat-then-Mkdir, unchanged, in the current release. D2's `[C]` HIGH ✔ and its `[R]` HIGH
fix (`mkdir` only, treat `EEXIST` as "exists") ✔ — this really is a correctness improvement
obtained by **deleting** a line.

`[C]` CONFIRMED — `validateID` allows `[A-Za-z0-9_+.-]`, rejects empty, and rejects anything
whose lexically-cleaned `/`+id differs (`factory_linux.go#L192-216`). D2's stricter regex
`^[A-Za-z0-9][A-Za-z0-9_.+-]{0,127}$` is a superset of runc's safety. ✔

### 7.2 `flock` semantics — **CORRECTED: the brief's footgun is backwards, and the real one bites §7.2's own diagram**

`[F]` HIGH — `flock(2)`: *"Locks created by `flock()` are associated with an **open file
description**… duplicate file descriptors (created by, for example, `fork(2)` or `dup(2)`)
refer to the same lock… the lock is released either by an explicit `LOCK_UN` on **any** of
these duplicate file descriptors, **or when all such file descriptors have been closed**."*

So a `flock` is **not** released when *a* dup is closed. It is released when **all** copies
are closed (or on an explicit `LOCK_UN`). D2 §6.6's `[F]` HIGH — *"released automatically on
process death or fd close"* — is therefore **imprecise in the dangerous direction**, and it
lands directly on §7.2's own sequence:

```
3. flock(<dir>/lock, LOCK_EX)          ← CLI takes the lock
4. fork/exec self "supervise <id>"     ← the lock fd is INHERITED by the supervisor
6. print id, release lock, exit 0      ← the CLI's close releases nothing:
                                         the supervisor still holds a copy
```

`[R]` HIGH — **as written, §7.2 leaves the per-container exclusive lock held for the entire
lifetime of the container**, so the next `mydocker start/kill/stop/rm` on that id blocks
forever (or `EWOULDBLOCK`s, if `LOCK_NB`). The fix is one flag — **open the lock file
`O_CLOEXEC`**, so `execve` of `mydocker supervise` drops it — but it must be stated, because
the failure is total, silent until the second command, and invisible in testing that only
ever runs one command. If the supervisor is reached by `fork` **without** `exec`, `O_CLOEXEC`
does not help and the child must `close()` the fd explicitly.

The genuine *early-release* footgun also exists, in Rust specifically, and it is a lifetime
bug rather than a dup bug: `File::open(p)?.lock()?` as a statement drops the temporary `File`
at the semicolon and releases the lock immediately. Bind it to a named variable that outlives
the critical section.

`[R]` HIGH — **ladder note: no crate needed.** `std::fs::File::{lock, lock_shared, try_lock,
try_lock_shared, unlock}` are stable since **Rust 1.89.0** and are documented as *"corresponds
to the `flock` function on Unix with the `LOCK_EX` flag"*. The std docs state the inheritance
rule in terms we should quote in our own code comment: *"The lock will be released when this
file (along with any other file descriptors/handles duplicated or inherited from it) is
closed."* `fs2` / `fd-lock` / `file-lock` are all unnecessary.

`[F]` CONFIRMED — `LOCK_EX|LOCK_NB` fails with **`EWOULDBLOCK`** (`flock(2)` ERRORS),
which is the clean "another `mydocker` is operating on this container" answer D2 wants.
Two further facts D2 should record: flock is **advisory only**, and *"If a process uses
`open(2)` … to obtain more than one file descriptor for the same file, these file descriptors
are treated independently by `flock()`"* — so a process that opens the lock file twice can
**deadlock against itself**. With one lock module owning the open (which §7.5 already
mandates for state writes), that is contained.

### 7.3 §9.2 — do mature runtimes roll back on a failed `create`, or leak and defer to `delete`? — **CONFIRMED: both roll back, and both do it by calling their own `delete`**

D2 checked runc only. I checked crun too, and the answer is the same and sharper.

- **runc** — `runc@v1.5.2/utils_linux.go#L222-228`: `defer func(){ if err != nil { r.destroy() } }()`,
  and `#L315-321` `destroy()` → `r.container.Destroy()`, gated by `shouldDestroy` (the
  `--keep` escape hatch). ✔ D2's account CONFIRMED at the current release.
- **crun** — `crun@1.30.1/src/libcrun/container.c`: every failure path of
  `libcrun_container_create()` calls `force_delete_container_status()` (`#L3004`, `#L3057`,
  and `#L2886`/`#L2949` for `run`), which is `container_delete_internal(..., force=true, ...)`
  at `#L2830-2833`. And when the detached child reports a non-zero code, the parent runs
  `libcrun_container_delete(context, def, context->id, /*force=*/true, &tmp_err)` with the
  log line *"Exit code is `%d`, deleting container"* (`#L3034-3037`).

`[R]` HIGH — **CONFIRMED and strengthened: D2 §9.4's recommendation (*"`delete` and `create`'s
rollback should be the same code"*) is not merely a good idea — it is literally what both
reference implementations do.** crun's failed `create` *is* a forced `delete`. Raise §9.4's
confidence and make it a structural rule.

`[R]` **DISPUTED → resolved against it: `rollback.json` (§9.3's `[R]` MED, §14 item 17) does
not earn phase-1 complexity.** Neither runc nor crun persists an acquisition list. Both
re-derive what to undo from the **state file plus the copied `config.json`** — which is
exactly why §6.4's `[R]` HIGH ("keep a verbatim copy of `config.json`, the bundle may vanish")
is load-bearing and CRI-O does the same (`FromDisk()` restoring volumes and ID mappings *from
the spec*). The cheaper alternative D2 itself names — *"on `rm --force`, best-effort try every
possible cleanup and report what failed"* — is what the state of the art actually ships.
**Drop `rollback.json`; keep the in-memory ordered `Vec<Cleanup>` for the in-process case,
and make `delete --force` idempotent and derivable from `state.json` + `config.json`.**
If a leak class later proves undiagnosable, add the file then — it is additive.

`[F]` CONFIRMED — §9.2's best insight stands and is the one to build around: *"everything the
child mounts **after** `unshare(CLONE_NEWNS)` is free to roll back — just let the child die"*
(`mount_namespaces(7)`). The corollary — do as little as possible in the host mount namespace
— is the right design pressure to put on D3.

---

## Part 8 — the three self-flagged weak spots (§14.1 item 2, §14 items 18 and 12)

### 8.1 Item 2 — youki's CVE-2019-5736 mitigation — **CONFIRMED against a pinned tree**

`youki-dev/youki@v0.7.0/crates/youki/src/main.rs#L65-76`: `pentacle::ensure_sealed()` is the
**first statement of `main()`**, before `Opts::parse()`, under a comment that names
CVE-2019-5736 and cites `opencontainers/runc@0a8e4117e7f715d5fbeef398405813ce8e88558b` and
the LXC commit. `pentacle` is a declared dependency at
`crates/youki/Cargo.toml#L33`. §14.1 item 2 is **CONFIRMED**; §11.8's `[C]` LOW exposure
claim should be struck, exactly as D2 says.

`[R]` HIGH — **but the generalisation §11.8 draws from it needs sharpening, and §7.3 needs
correcting.** All three reference runtimes seal themselves at startup, in the binary crate /
constructor, before anything else:

| | mechanism | location |
|---|---|---|
| runc | `cloned_binary.c`, `ensure_cloned_binary()` | C, pre-`main` |
| crun | `__attribute__((constructor)) libcrun_rexec()` → `ensure_cloned_binary()` | `crun@1.30.1/src/libcrun/linux.c#L7329-7337` |
| youki | `pentacle::ensure_sealed()` | `youki@v0.7.0/crates/youki/src/main.rs#L76` |

D2 §7.3 proposes to *"design so we never re-exec into the container at all — … That
eliminates the entire CVE class rather than mitigating it."* **That is false and it is the
most dangerous sentence in §7.** CVE-2019-5736 is not about re-execing *into* a container; it
is about a container process holding `/proc/<runtime-pid>/exe` and overwriting the **host
binary**, so that the **next** invocation runs attacker code. Not re-execing removes one
*route* to the fd; it does not remove the exposure, because our supervisor and our init
share an address space with the host binary's mapping for the whole setup window, and a
`--privileged`-ish or `CAP_SYS_PTRACE`-holding container only needs to reach
`/proc/self/exe`. **Three independent implementations, in three languages, concluded that
sealing is required. Take their word for it and do it on line one of `main()`** — about 60
lines: `memfd_create(MFD_CLOEXEC|MFD_ALLOW_SEALING)`, copy, `F_ADD_SEALS`, `fexecve`.
§11.8 already recommends this; §7.3 must stop claiming it is unnecessary.

### 8.2 Item 18 — youki's `as_sibling` / `CLONE_PARENT` — **CORRECTED. D2's reading is half right and the half that is wrong matters.**

A second read of the surrounding code (not one file) settles it. Three facts:

1. `youki@v0.7.0/crates/libcontainer/src/process/container_intermediate_process.rs#L203-214` —
   the intermediate clones **init** with `fork::container_clone_sibling(cb)`
   **unconditionally**, i.e. `CLONE_PARENT` always. Comment: *"We also want to clone the init
   process as a sibling process to the intermediate process. The intermediate process is only
   used as a jumping board… the intermediate process can just exit safely after the job is
   done."*
2. `.../process/container_main_process.rs#L96-105` — `as_sibling` selects
   `container_clone_sibling` vs `container_clone` for the **intermediate**, not for init.
3. `.../container/init_builder.rs#L40` — the default is `as_sibling: false`; it is a public
   builder setter for library consumers, and `youki create`/`run` do not set it.

So the ancestry is:

| `as_sibling` | intermediate's parent | init's parent (always `CLONE_PARENT` from intermediate) |
|---|---|---|
| `false` (default) | youki main | **youki main** |
| `true` | youki main's parent | **youki main's parent** (the shim/conmon) |

`[C]` CORRECTED — **youki does not "make the container a sibling of the launcher" by
default.** By default init's parent is youki main. `as_sibling` exists for an embedder whose
`create` process exits and who wants the container owned by *its* caller instead of being
reparented — stated verbatim in `fork.rs#L37-43`: *"If we clone a child process as the init
process, the calling process (likely the youki main process) will exit and the init process
will be re-parented to the process 1 (system init process), which is not the right behavior
of what we look for."*

`[R]` HIGH — **does the reasoning apply to us? No for the shape, yes for the mechanism.**
Our supervisor stays alive and is the parent, so `as_sibling`'s motivation (a create process
that exits) does not exist for us. But fact 1 is the one to carry forward: **the
intermediate→init clone must use `CLONE_PARENT`**, so that when we eventually add the
intermediate for rootless (§3.4's `[R]` MED caveat), init's parent remains the supervisor and
we never depend on reparenting at all. Two kernel constraints to design around, both pinned
in Part 1.6: `CLONE_PARENT` is `EINVAL` from a namespace init, and `clone3` rejects
`CLONE_PARENT` with a non-zero `exit_signal`. D2's §3.4 "two process boundaries" conclusion
survives; its §10.2 sentence about *"wants the init's parent to be main, not the
intermediate"* should be replaced by *"`CLONE_PARENT` on the intermediate→init clone is
unconditional and is how youki avoids needing a subreaper at all."*

### 8.3 Item 12 — the FIFO pipe capacity behind §2.3 — **CONFIRMED and closed, without touching the shim**

Settled from `pipe(7)` plus two `cat`s under `/proc`, so the fd-on-a-production-shim problem
never arises.

`[F]` HIGH — `pipe(7)`: *"Since Linux 2.6.11, the pipe capacity is 16 pages (i.e., 65,536
bytes in a system with a page size of 4096 bytes)… Since Linux 4.5, the default pipe capacity
is lower than 16 pages when the `pipe-user-pages-soft` limit is exceeded… Since Linux 4.9,
the value of [`/proc/sys/fs/pipe-max-size`] also acts as a ceiling on the default capacity of
a new pipe or newly opened FIFO."*

`[O]` HIGH — `my-droplet`, kernel 6.11.0-29-generic:
`getconf PAGESIZE` → `4096`; `cat /proc/sys/fs/pipe-max-size` → `1048576`;
`cat /proc/sys/fs/pipe-user-pages-soft` → `16384`; `.../pipe-user-pages-hard` → `0`.

⇒ default FIFO capacity on this host is **16 × 4096 = 65,536 bytes**, not clamped
(1 MiB ceiling ≫ 64 KiB), and the soft limit — which `pipe(7)` says applies only to *"a
single **unprivileged** user (i.e., one with neither the `CAP_SYS_RESOURCE` nor the
`CAP_SYS_ADMIN` capability)"* — does not apply to a root-first engine at all.

`[F]` **Upgrade §2.3's `[F]` MED to `[F]` HIGH:** a FIFO/pipe with no reader and a full
64 KiB buffer blocks the writer; the figure is 64 KiB on x86_64 with 4 KiB pages; it is
tunable per-fd with `F_SETPIPE_SZ`; and it is *not* clamped for root. D2's design conclusion
(*"do not put a FIFO between the supervisor and the log file"*) stands, and gains a number:
**a container stalls after 64 KiB of unread output**, which is roughly one second of a
chatty process. Item 12 is closed; remove it from the open list.

---

## Part 9 — §11.1 conmon and §11.8 "which of these problems are ours" — **CONFIRMED, two additions**

### 9.1 §11.1's conmon account — **CONFIRMED against `conmon@v2.2.1`**

Spot-checked every mechanism §7's supervisor is modelled on. All hold at the current release
(D2 cited line numbers from an older tree; the line numbers moved, the facts did not):

| D2 §11.1 claim | Verdict | Pinned location |
|---|---|---|
| double fork, then `setsid()`; podman reaps the middle process | ✔ CONFIRMED | `conmon.c#L99-132` |
| `PR_SET_CHILD_SUBREAPER` right after `setsid()`, *"so we can wait for container process and return its exit code"* | ✔ CONFIRMED | `conmon.c#L131-140` via `utils.c#L88-90` |
| the create child sets `PDEATHSIG(SIGKILL)` and resets `oom_score_adj` — *"We don't want runc to be unkillable"* | ✔ CONFIRMED | `conmon.c#L228-233`, `#L296` |
| the *"current runC API dups the stdio of the calling process over the container's fds. This is actually \*very bad\*"* fork rationale | ✔ CONFIRMED verbatim | `conmon.c#L220-227` |
| `ctl` FIFO opened on **both** ends — *"to prevent getting flood of POLLHUPs when last writer closes"* | ✔ CONFIRMED | `ctrl.c#L260-293` |
| writes the exit status to files, with the inotify rationale | ✔ CONFIRMED | `conmon.c#L515-529` |

`[C]` NEW — **one mechanism D2 missed and it is a real design lesson:** conmon *disables*
its own subreaper flag before forking the exit command —
`conmon@v2.2.1/src/ctr_exit.c#L166-174`, *"We don't want the exit command to be reaped by the
parent conmon as that would prevent double-fork from doing its job. Unfortunately, that also
means that any new subchildren from still running processes could also get lost."* A
subreaper is a mode you enter and leave, not a flag you set once. If our supervisor ever
forks a teardown helper it wants orphaned, the same applies.

`[C]` CONFIRMED — the `sun_path` 108-byte trap (§11.1 item 9) and D2's `[R]` HIGH that it
*"will bite us"*. Measured against the live host's real path shapes: `sun_path` is 108 bytes
**including the terminating NUL**, so 107 usable. A per-container socket at
`/run/docker/runtime-runc/moby/<64-hex>/attach.sock` is **106** characters; under
`/var/lib/mydocker/containers/<64-hex>/` it is **105** — both fit, with one and two bytes of
headroom respectively, and neither survives a prefix, a test directory or a non-default root. `[R]` HIGH — put runtime sockets under a
short `/run/mydocker/<12-hex>/` path, which is the cheap structural fix, rather than the
`O_PATH` + `/proc/self/fd/N` bind trick, which is the expensive one.

### 9.2 §11.8's "which of these are ours" — **CONFIRMED, with one line moved and one added**

The three-bucket split is sound and I could not fault it, with these amendments:

- **Move out of "Ours, because the kernel or the spec forces it":** *"`CLONE_PARENT` (or a
  subreaper) so init doesn't reparent to PID 1"*. In §7.2's shape **neither is forced** — the
  supervisor is the parent. Restate as: *"`CLONE_PARENT` on the intermediate→init clone, the
  day an intermediate exists; `PR_SET_CHILD_SUBREAPER` for `exec` descendants and host-pidns
  containers."* (Parts 2.1 and 5.)
- **Keep, and raise:** *"the memfd self-clone against CVE-2019-5736"* — but delete §11.8's
  qualifier *"the moment we bind-mount a writable host path"*. It is not conditional on bind
  mounts (Part 8.1), and §7.3 currently argues it away.
- **Add to "Not ours":** `rollback.json` (Part 7.3) — nothing in the ecosystem persists one.
- **Add to "Ours only if we want a specific feature":** a `pidfd` per container. Cheap,
  PID-reuse-proof, and on kernel ≥ 6.15 a second exit-code source (Part 5.1). crun already
  does this for `kill`: `crun@1.30.1/src/libcrun/linux.c#L7388-7399`, `libcrun_kill_linux()`
  uses `pidfd_open` with a `kill(2)` fallback on `ENOSYS` — so the PID-reuse defence D2 §6.1
  builds out of `/proc/<pid>/stat` field 22 is, in the live-supervisor case, obsolete.
  **Keep the start-time check for the after-the-supervisor-died case only**, exactly as D2's
  `[R]` MED already concedes.

`[R]` CONFIRMED — §11.8's closing sentence — *"almost every mechanism above the
fork/exec/namespace/cgroup layer exists to solve 'my supervisor is not my client's parent'"*
— survived every check I made. It is the most useful sentence in the document, and Part 2.1,
Part 5 and Part 8.2 are all specific instances of it: conmon's subreaper, youki's
`as_sibling`, runc's `CLONE_PARENT`, and containerd's shim `delete` subcommand are **four
different solutions to the same self-inflicted problem**, and D2's design does not have it.

---

## Part 10 — breadth sweep: sections checked, nothing to correct

Per the contract's breadth-first rule, these were checked at least to the "nothing here
needed correcting" level and are not expanded above.

- **§1.1–§1.3, §1.5** — lifecycle states, the five operations and their MUST/error clauses,
  the `config.json` required/optional split, and the eight explicit non-mandates: all
  verified verbatim against `runtime-spec@v1.3.0/{runtime.md,config.md}`. §1.3's minimal
  legal config (`ociVersion`, `root.path`, `process.cwd`, `process.args[0]`,
  `process.user.{uid,gid}`) is correct. §1.5's `[R]` HIGH — *"OCI compatibility is a
  serialization + lifecycle-semantics contract, not an architecture"* — is CONFIRMED and is
  the correct frame.
- **§2.1–§2.2** — the process table, the shim's fd table, `PPid: 1` + `SID: 912`, the shim
  being outside the container's cgroup. Not re-run (production). The architectural conclusion
  (`[R]` HIGH, *"a supervisor inside the supervised cgroup is a design bug"*) is
  independently CONFIRMED by conmon, which writes `-1000` to its own `oom_score_adj` and is
  placed in a separate `libpod-conmon-<id>.scope`.
- **§3.3** — `nsexec.c`'s three stages and sync alphabet: not re-derived; the *conclusion*
  (all of it is Go scaffolding) is settled by Part 1.4's corrected table.
- **§4** — the parent-side ordering. The one claim I checked and can confirm from a second
  source: crun also applies the cgroup at clone time rather than after, using
  `clone3(CLONE_INTO_CGROUP)` with a `clone()` fallback —
  `crun@1.30.1/src/libcrun/linux.c#L6576-6596`. **This also dates §10.3's table row**, which
  says crun's mechanism is *"`clone` in `linux.c`"*: at 1.30.1 crun is `clone3` +
  `CLONE_INTO_CGROUP` first. D2's §4 `[R]` rule 2(b) preference for `CLONE_INTO_CGROUP` is
  therefore what the C implementation already does. ✔
- **§5.1, §5.5, §5.6** — detach mechanisms; `pid_namespaces(7)` signal semantics for a
  namespace init (only `SIGKILL`/`SIGSTOP` bypass the "must have a handler" rule); the
  13-entry failure catalogue. All sound. Catalogue entries 1 and 3 need the qualifications in
  Parts 2.1 and 5; entry 12 (FIFO `O_NONBLOCK`) needs Part 2.5's addition (the real hazard is
  not `EAGAIN`, it is blocking forever when init is already dead).
- **§6.1–§6.5** — runc's `state.json` fields and their justifications; the atomic
  temp+`rename` write; the two-tier `/run` vs `/var/lib` split; `boot_id` as the reboot
  marker. All CONFIRMED, including the `[C]` HIGH that **runc keeps no exit code** — true at
  `v1.5.2`. D2's `[R]` HIGH that this is *"the single most important thing to fix in our
  design"* is correct and is reinforced by Part 5.2.
- **§7.1, §7.3–§7.6** — component-by-component need analysis; one static binary; the CLI
  never in the ancestry; the cheap-vs-expensive table; the phasing. Sound. §7.3's
  CVE-2019-5736 paragraph is the exception (Part 8.1).
- **§8** — the fork-safety argument. `fork(2)` and `signal-safety(7)` say exactly what D2
  quotes; the *"the process that calls `clone()` never has more than one thread, ever"* rule
  is the correct conclusion and is independently why this cluster's whole Go story exists
  (Part 1.1). Not expanded; V7 owns the crate-level consequences.
- **§11.2–§11.7** — not re-verified per the brief. One note: §11.2's CRI-O finding (hundreds
  of production containers on atomic-rename `state.json` files, no database) is the strongest
  single justification in D2 for the no-daemon design, and nothing I found contradicts it.

---

## Corrections that change a recommendation

1. **§7.2 step 3→6, the `flock` lifetime** → `flock` is released only when **all** inherited
   duplicates close (`flock(2)`), so the CLI's lock is inherited by the forked supervisor and
   held for the container's whole life → **open the lock file `O_CLOEXEC`** (or `close()` it
   explicitly in a forked-not-exec'd child). Invalidates §7.2's *"release lock, exit 0"* as
   written.

2. **§7.2 `mydocker start` = "open `exec.fifo` O_RDONLY, read one byte. That's it"** → if init
   died between `create` and `start`, no writer will ever appear and `start` blocks forever
   **while holding the per-container lock**, wedging `kill`/`delete` too. runc has guarded
   this since 2019 (`runc@8541d9cf3d8b`) → **open `O_RDONLY|O_NONBLOCK` and `poll()` the FIFO
   together with a `pidfd_open(init_pid)`**. Shifts §7.2 and §5.6 entry 12.

3. **§7.2 supervisor line "(optional) openpty"** → runc allocates the pty in the **container
   init**, from the **container's `/dev/pts/ptmx`** (`runc@v1.5.2/libcontainer/standard_init_linux.go#L96-106`,
   `console_linux.go#L22-42`, `docs/terminals.md`) → **the supervisor must never call
   `openpty()`**; it receives the master over a socket. Also avoids the `setsid()` +
   `TIOCSCTTY` collision below. Invalidates §5.2's protocol sketch and §7.2's diagram line.

4. **§5.1 `[R]` MED "setsid() in the supervisor"** → correct for signals, but `setsid()`
   leaves the supervisor a session leader **with no controlling terminal**, i.e. the exact
   state in which the next tty it opens without `O_NOCTTY` becomes its ctty, after which the
   container's `TIOCSCTTY` fails `EPERM` (`credentials(7)`, `TIOCSCTTY(2const)`) → **any pty
   fd the supervisor touches must be opened `O_NOCTTY`.** Shifts §5.2's `[R]`.

5. **§5.4 `[R]` HIGH "set `PR_SET_CHILD_SUBREAPER` … exit codes are recoverable"** → in §7.2's
   shape the supervisor is init's direct parent, so the flag does nothing for the container's
   exit code; and `kernel/exit.c#L634-655` shows a subreaper **never adopts across a PID
   namespace**, so it does nothing for zombies inside the container either → **keep the flag,
   but for `exec` descendants and host-pidns containers only.** Invalidates §5.4's and §5.6
   entry 1's stated reasons and §11.8's "kernel forces it" placement.

6. **§14 item 10 "target 1.2.0 because runc 1.2.5 on our host implements it"** → the bundles
   on that host declare **1.2.1** (moby v28.3.1 vendors runtime-spec v1.2.1) while runc says
   1.2.0 and **nothing validates it** — `runc@v1.5.2/libcontainer/specconv/spec_linux.go#L622`
   overwrites `config.Version` with its own constant; crun hardcodes `"1.0.0"` → **target
   1.3.0, accept any SemVer major 1, never reject on version.** Replaces item 10's `[R]` MED.

7. **§10.1 "crun never re-execs itself … no C constructor trick" / §7.3 "design so we never
   re-exec into the container … eliminates the entire CVE class"** → crun **does** have
   `__attribute__((constructor)) libcrun_rexec()`
   (`crun@1.30.1/src/libcrun/linux.c#L7329-7337`), as do runc and youki, for CVE-2019-5736;
   not re-execing into the container does **not** eliminate that class → **seal
   `/proc/self/exe` into a memfd on line one of `main()`, unconditionally.** Invalidates
   §7.3's `[R]` HIGH and removes §11.8's bind-mount qualifier.

8. **§9.3's `[R]` MED `rollback.json` / §14 item 17** → neither runc nor crun persists an
   acquisition list; crun's failed `create` literally calls its own forced `delete`
   (`crun@1.30.1/src/libcrun/container.c#L2830-2833`, `#L3004`, `#L3034-3037`) → **drop
   `rollback.json` from phase 1; make `delete --force` idempotent and derivable from
   `state.json` + the copied `config.json`.** Resolves item 17. Simultaneously raises §9.4's
   *"delete and create's rollback are the same code"* from recommendation to confirmed
   practice.

9. **§6.1's state-dir permission pairing** → `0711` dir + `0622` FIFO is only safe because the
   **root** is `0700` (`runc@v1.5.2/libcontainer/factory_linux.go#L45`; `[O]` verified on
   `my-droplet`) → **create the state root `0700` explicitly**; without it any local user who
   knows an id can start a created container. Completes §6.1's `[C]` HIGH.

10. **§9.4's `[F]` HIGH "`delete` MUST error unless `stopped`"** → true for every released
    revision, but `runtime-spec@main` (2026-10-16) now permits `delete` on `created`, either
    erroring **or** terminating-then-deleting → **implement `delete` on `created` as
    kill-reap-delete**, not a hard error. Shifts §9.4 and the `delete` row of §1.2.

11. **§14.1 item 1** → CONFIRMED and stronger than claimed (one changed line in the whole
    150-line `runtime.md`), but D2 missed that **`config.md` also changed `prestart`'s
    description**, so §1.4's hook table is already v1.3.0 while §1.1's step 9 is v1.2.0 →
    **rewrite §1.1 step 9 and declare one revision for the whole document.**

12. **§10.2's `as_sibling` reading** → the intermediate→init clone is `CLONE_PARENT`
    **unconditionally** (`youki@v0.7.0/.../container_intermediate_process.rs#L203-214`);
    `as_sibling` (default **false**) only moves the *intermediate*, so by default init's
    parent is youki main, not a sibling of it → restate §10.2 and carry forward only the
    mechanism: **`CLONE_PARENT` on the intermediate→init clone**, subject to the two kernel
    constraints at `kernel/fork.c#L2162-2164` and `#L3043-3045`. Resolves item 18.

13. **§2.3's `[F]` MED pipe capacity** → settled: **65,536 bytes** (16 × 4 KiB) default, not
    clamped on `my-droplet` (`pipe-max-size` = 1 MiB), and `pipe-user-pages-soft` exempts
    `CAP_SYS_RESOURCE` holders → upgrade to `[F]` HIGH and close item 12. Shifts nothing but
    puts a number on §5.3's log-writer argument: **a container stalls after 64 KiB of unread
    output.**

14. **§5.4's `[R]` MED pidfd + `waitid(P_PIDFD)`** → `waitid` is child-only (`wait(2)`), so the
    pair buys a parent nothing; but **`ioctl(pidfd, PIDFD_GET_INFO)` with `PIDFD_INFO_EXIT`
    returns `exit_code` since Linux 6.15** (`torvalds/linux@v6.15` vs `@v6.14`,
    `include/uapi/linux/pidfd.h`) → **take the pidfd via `clone3(CLONE_PIDFD)`** for liveness,
    `poll`-ability and, on ≥ 6.15, a second exit-code source. Does not relax the disk write.

15. **§5.3's moby citation** → `api/pkg/stdcopy/stdcopy.go` **404s at `v28.3.1`**; the file is
    `moby/moby@v28.3.1/pkg/stdcopy/stdcopy.go#L16-31`. The content is correct. The
    document's only recorded full URL has already drifted — exactly as §15 predicted.

---

## Still unverifiable, and the exact test

1. **D2 §5.4's subreaper experiment, re-run in our actual topology** (supervisor is init's
   *direct* parent, container in its own pidns, `exec`'d process via `setns`) → local
   destroyable VM: a 40-line C/Rust program; assert the supervisor collects the `exec`
   grandchild's status with the flag and host PID 1 gets it without, and that a
   container-internal grandchild goes to container PID 1 **either way**. Settles Part 2.1 and
   Part 5 empirically rather than from `kernel/exit.c`.

2. **`PIDFD_INFO_EXIT` behaviour for a non-child** → a host with kernel ≥ 6.15 (not
   `my-droplet`, which is 6.11): `pidfd_open(pid)` on an unrelated process, wait for its
   death, then `ioctl(fd, PIDFD_GET_INFO, &info)` with `mask = PIDFD_INFO_EXIT`; assert
   `info.mask & PIDFD_INFO_EXIT` and that `info.exit_code` matches. Decides whether `mydocker
   ps` can report a truthful exit code for a container whose *reaping* parent was not us.

3. **The `flock`-inherited-across-fork defect (correction 1)** → local VM:
   `open(lock, O_RDWR)`; `flock(LOCK_EX)`; `fork()`; parent `close()`s and exits; second
   process tries `flock(LOCK_EX|LOCK_NB)` → expect `EWOULDBLOCK`. Repeat with `O_CLOEXEC` and
   `fork+execve` → expect success. Two minutes; proves the one-flag fix.

4. **The `setsid()` + `TIOCSCTTY` collision (correction 4)** → local VM: `setsid()`; open a
   pty slave **without** `O_NOCTTY`; `fork`; child `setsid(); ioctl(TIOCSCTTY, 0)` → expect
   `EPERM`. Repeat with `O_NOCTTY` → expect success.

5. **`F_GETPIPE_SZ` on a live shim FIFO fd** → still not runnable on `my-droplet`
   (needs a process holding that fd, and it is production). **No longer needed** — Part 8.3
   settles the question from `pipe(7)` + `/proc/sys/fs/pipe-*`. If anyone wants the direct
   reading anyway: on a *local* VM, `docker run -d nginx` then
   `python3 -c "import fcntl,os; print(fcntl.fcntl(os.open('/proc/<shim>/fd/12', os.O_RDONLY), 1032))"`.

6. **`CLONE_INTO_CGROUP` exercised (item 13) and `cgroup.kill` semantics (item 14)** →
   unchanged from D2: the first `create` on the dev VM, and
   `Documentation/admin-guide/cgroup-v2.rst` respectively. Out of this cluster's scope (D5).

7. **Whether `runc --version`'s reported `spec:` is ever checked by any caller** → grep
   containerd, CRI-O and Podman for parsing of `runc --version` output. Would decide whether
   correction 6's "never reject on version" has any interop cost at all. Not run: three large
   Go trees, and the answer does not change our own behaviour.
