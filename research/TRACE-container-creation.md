# TRACE — `mydocker run alpine /bin/sh`, annotated at the syscall level

**Status:** synthesis artifact. This is the corrected creation sequence for the final `RND.md`.
It supersedes D1 §10.1 and D5 §20 wherever they differ, and it is the document later
implementation work should be checked against.

**Authority rule.** The `D*` domain files were **not edited after verification**. Where a `V*`
file corrects, refutes or reclassifies a `D*` claim, **the V verdict is what appears here** and
the D claim is recorded only as the thing that was wrong. Every load-bearing line cites the file
and section it comes from.

**Tagging** (corpus convention): `[F]` fact — kernel/spec/man-page behaviour · `[C]` convention —
what a shipping runtime does · `[R]` recommendation — our choice · `[O]` observation — measured on
a named host. Confidence `HIGH` / `MED` / `LOW`. Facts and recommendations are never mixed in one
sentence.

**Empirical host.** `host-a` (`HOST-A`), Ubuntu 24.10, kernel `6.11.0-29-generic`,
production. Every `[O]` marked *this session* was taken read-only; nothing was created, started,
stopped or written.

---

## 0. Five actors, not two

The brief's sequence reads as one linear program. It is not. Five distinct execution contexts
appear in the trace, and the single most common way to get this wrong is to put a step in the
wrong one.

| Tag | Who | Lifetime | Why it is separate |
|---|---|---|---|
| `L` | the `mydocker run` CLI process | the user's foreground | owns the terminal, parses args, prints the exit status |
| `X` | the image/pull process | until the rootfs exists | `[R]` HIGH — **must not be `S`.** An HTTP client stack spawns threads; `S` must never have had a second thread (V7 §8.2, V2 §8.1) |
| `S` | the supervisor | outlives the container | calls `clone3`, holds the pidfd, plumbs the netns, reaps, unwinds |
| `I` | the intermediate | milliseconds | **userns path only.** Exists solely so someone outside the new user namespace can write its `uid_map` (D1 §3.3, D5 §9.1) |
| `C` | the container child | between `clone3` and `execve` | everything that requires *being inside* the new namespaces |

`[R]` HIGH — for the MVP, `L` and `S` are the same process and the same binary
(D2 §7.3, CONFIRMED V2 §7.3). `X` is a separate process from day one, and that is structural, not
stylistic: V7 §8.2 records that D3's HTTP client can silently make its process multithreaded, and
D3 offers that same process as a supervisor candidate. `[R]` HIGH — **structural beats
disciplined** (D2 §8.3, endorsed V2 §8.2 as "the strongest recommendation in this section").

---

## 1. The corrected sequence — compact table

`Root?` = requires privilege in the **initial** user namespace. `MVP?` = needed for
`mydocker run alpine /bin/sh` to work at all; otherwise the ladder stage that introduces it.

### 1.1 Host phase — before `clone3`

| # | Step | Who | Primitive | Root? | MVP? | Ordering forced by |
|---|---|---|---|---|---|---|
| 1 | Parse CLI, build the runtime config | `L` | — | no | yes | — |
| 2 | Resolve image ref → manifest → layer digests; pull | `X` | HTTPS | no | yes | must precede 3 |
| 3 | Extract layers into the content store | `X` | `openat2` + `*at` | no | yes | must precede 4 |
| 4 | Assemble `lowerdir` list, `upperdir`, `workdir` | `S` | `mkdir` | no | yes | kernel: `workdir` must be an empty dir on the **same fs** as `upperdir` |
| 5 | Mount the overlay → `merged/` | `S` *(or `C`, §3.5)* | `mount(2)` / `fsconfig` | yes | yes | must precede 12 if done on the host |
| 6 | Create + lock the state dir; status `creating` | `S` | `mkdir`, `flock` | no | yes | convention |
| 7 | Create the cgroup, write limits, read back, open `O_DIRECTORY` dirfd | `S` | `mkdir`, `write`, `openat` | yes | yes | **kernel** — limits must exist before the process does (D5 §20 rule 7) |
| 8 | Allocate IP; create host-side network objects | `S` | rtnetlink | yes | no — stage 3a | must precede 16 |
| 9 | Create `exec.fifo` (`0622`), sync `socketpair`, error pipe, console `socketpair` | `S` | `mkfifo`, `socketpair`, `pipe2` | no | yes | must precede 12 |
| 10 | **Assert `Threads == 1`** | `S` | read `/proc/self/status` | no | **yes** | **kernel + glibc** — see §2.3 |
| 11 | Pre-build **everything** the child will touch: argv/envp arrays, every path `CString`, capability bitsets, the seccomp `Vec<sock_filter>`, mount option strings | `S` | — | no | **yes** | see §2.4 — nothing may allocate after 12 |
| 12 | **`clone3`** — `CLONE_NEWNS\|NEWPID\|NEWNET\|NEWUTS\|NEWIPC\|NEWCGROUP` + `CLONE_PIDFD` + `CLONE_INTO_CGROUP` | `S` | `clone3(2)` | yes | yes | **kernel** — namespaces are created *by* this call |

### 1.2 Parent phase — after `clone3`, concurrent with the child

| # | Step | Who | Primitive | Root? | MVP? | Ordering forced by |
|---|---|---|---|---|---|---|
| 13 | Record the pidfd; close the child ends of every pipe | `S` | — | no | yes | convention |
| 14 | *userns only:* write `setgroups=deny`, then `uid_map`, then `gid_map`; release `I` | `S` | `write` to `/proc/<pid>/*` | no | no — rootless stage | **kernel** — D5 §9.1 |
| 15 | Verify cgroup membership; fallback: parent writes `cgroup.procs` **before** releasing the child | `S` | `write` | yes | yes | **kernel** — before the child can `fork` |
| 16 | Plumb the netns: `setns` in, netlink, `setns` back | `S` | `setns(2)`, rtnetlink | yes | `lo` up only | **kernel** — must finish before 36/37 |
| 17 | Run `createRuntime` hooks | `S` | `fork`/`exec` | yes | no — OCI stage | spec: after namespaces exist, before the user process runs |
| 18 | Await child `ready`; write `state.json`; reply `run` | `S` | `read`/`write` on the sync socket | no | yes | convention |
| 19 | `start`: `open(exec.fifo, O_RDONLY)`, `read` 1 byte | `S` | `open`, `read` | no | yes | **kernel** — the FIFO open is the rendezvous |
| 20 | Supervise: poll the pidfd, forward signals, reap, report exit | `S` | `pidfd_open`/`poll`/`waitid` | no | yes | — |

### 1.3 Child phase — between `clone3` and `execve`

Every row here is **impossible in the parent**. §2.1 says why, per row.

| # | Step | Who | Primitive | Root? | MVP? | Ordering forced by |
|---|---|---|---|---|---|---|
| 21 | Make `/` **not `MS_SHARED`** | `C` | `mount(NULL,"/",NULL,MS_REC\|MS_SLAVE,NULL)` | yes | **yes** | **kernel** — `pivot_root` refuses a shared `/` |
| 22 | `MS_PRIVATE`, **non-recursive**, on the **parent mount of `new_root`** (walk up on `EINVAL`) | `C` | `mount(2)` | yes | yes | **kernel** — the precondition is on the parent mount |
| 23 | Ensure `new_root` is a mount point — recursive self-bind (or the in-child overlay mount, §3.5) | `C` | `mount(MS_BIND\|MS_REC)` | yes | yes | **kernel** — `pivot_root` `EINVAL` otherwise |
| 24 | *(optional)* `unshare(CLONE_NEWCGROUP)` here instead of at step 12 | `C` | `unshare(2)` | yes | no — hardening | **kernel** — the cgroup you are in *at that moment* becomes the ns root |
| 25 | Mount `/proc`, `/dev` (tmpfs), `/dev/pts`, `/dev/shm`, `/dev/mqueue`, `/sys` (ro), `/sys/fs/cgroup` (ro) into the rootfs | `C` | `mount(2)` ×7 | yes | yes | **kernel** — procfs needs the new pidns, sysfs the new netns, mqueue the new ipcns |
| 26 | Create device nodes — `mknod`, or bind from the host `/dev` | `C` | `mknod(2)` / `mount(2)` | **yes** (`CAP_MKNOD` is initial-userns only) | yes | must follow 25 (`/dev` tmpfs must exist) |
| 27 | User bind mounts / volumes — two passes for flags | `C` | `mount(2)` ×2 | yes | no — volumes stage | `MS_BIND` ignores flags on pass 1 |
| 28 | **`pivot_root`** — `O_PATH` on `/`, `fchdir(new)`, `pivot_root(".",".")`, `fchdir(old)`, `MS_REC\|MS_SLAVE` old, `umount2(".",MNT_DETACH)`, `chdir("/")` | `C` | `pivot_root(2)`, `umount2(2)` | yes | **yes** | **kernel** — six preconditions, §3.8 |
| 29 | Re-open `/dev/null` for any fd still pointing at the detached tree | `C` | `open(2)` | no | no — hardening | must follow 28 |
| 30 | `maskedPaths`, `readonlyPaths`, ro-remount of `/dev` — **re-specifying `nosuid/nodev/noexec` read back via `statfs`** | `C` | `mount(2)` | yes | no — hardening | **kernel** — must follow 25 and 26 |
| 31 | `sethostname`, `setdomainname` | `C` | `sethostname(2)` | `CAP_SYS_ADMIN` in the owning userns | yes | **kernel** — without `CLONE_NEWUTS` this renames the **host** |
| 32 | Allocate the pty **from the container's `/dev/ptmx`**; send the master up the console socket via `SCM_RIGHTS`; `dup` the peer onto 0/1/2 | `C` | `openat`, `ioctl`, `sendmsg` | no | yes (for `-it`) | **must follow 28** — V2 §5.2 |
| 33 | `setsid()`; `ioctl(TIOCSCTTY)` | `C` | `setsid(2)`, `ioctl(2)` | no | yes (for `-it`) | **kernel** — `TIOCSCTTY` `EPERM` without session leadership |
| 34 | Resolve `process.cwd` under the new root; `chdir`; verify `st_dev` | `C` | `openat2`, `fchdir` | no | yes | must follow 28 |
| 35 | LSM label — write `exec <profile>` to `/proc/self/attr/exec` | `C` | `write` on a pre-opened fd | varies | no — LSM stage | **kernel** — must precede `execve` |
| 36 | **seccomp — branch A** (only when `noNewPrivileges` is **false**) | `C` | `seccomp(2)` | needs `CAP_SYS_ADMIN` | no — security stage | **kernel** — must precede 37, which removes `CAP_SYS_ADMIN` |
| 37 | **Capabilities, in this order:** `PR_CAPBSET_DROP` ×N → `capset(P/E/I)` → `PR_SET_KEEPCAPS` → `setgroups`/`setgid`/`setuid` (**raw syscalls**) → `capset` again → `PR_CAP_AMBIENT_RAISE` → `PR_CLR_KEEPCAPS` | `C` | `prctl(2)`, `capset(2)`, raw `SYS_setres*id` | needs `CAP_SETPCAP` | yes (partially) | **kernel** — §3.13; getting this backwards is silent |
| 38 | `PR_SET_NO_NEW_PRIVS` | `C` | `prctl(2)` | no | no — security stage | **kernel** — must precede 40 |
| 39 | `close_range(3, ~0U, 0)` | `C` | `close_range(2)` | no | **yes** | must follow every step that opens an fd |
| 40 | Signal `ready` on the sync socket; then `open(exec.fifo, O_WRONLY)` — **blocks until `start`** — write 1 byte, close the `O_PATH` fd | `C` | `openat`, `write` | no | yes | **kernel** — FIFO open blocks until a reader opens |
| 41 | **seccomp — branch B** (only when `noNewPrivileges` is **true**) | `C` | `seccomp(2)` | no | no — security stage | `[C]` — as late as possible, so the profile can be minimal |
| 42 | **`execve`** | `C` | `execve(2)` | no | **yes** | — |

---

## 2. The parent/child split, made explicit

### 2.1 What is impossible in the parent, and why

`[F]` HIGH — four independent reasons, none of which has a workaround (D1 §10.2, no V1 objection).

**(a) The parent is not in the namespaces.** Steps 21–31 mount into a mount namespace `S` is not a
member of. This is not a permission problem — `S` holds every capability and still cannot do it,
because `mount(2)` acts on the caller's mount namespace and `S`'s is the host's. The same argument
covers `sethostname` (UTS ns) and every mount whose superblock is namespace-derived: procfs is tied
to the pidns of the mounter, sysfs to the netns, mqueue to the ipcns (D1 §6.1/§6.5/§6.6, CONFIRMED
and **promoted from single-host `[O]` to `[F]`** by V1 §D9).

The one exception worth knowing: `[F]` HIGH — since Linux 5.8, `setns(2)` on a **pidfd** moves the
caller into *many* namespaces atomically (V1 §C2, REFUTING D1 §3.1's "each call changes just one").
So a parent *could* join and mount. It must not, because joining the mount namespace requires
`CAP_SYS_CHROOT` **and** `CAP_SYS_ADMIN`, refuses a caller sharing `CLONE_FS` state, and — decisively
— would move `S` itself out of the host, leaving nobody to supervise. `[R]` HIGH — the pidfd form of
`setns` belongs on the **`exec` path**, not the create path: `mydocker exec` joining six namespaces
is **one** atomic `setns`, not six `open`+`setns` pairs, which removes a partially-joined failure
state that has no recovery (V1 §C2).

**(b) The child cannot grant itself privilege.** `uid_map`/`gid_map` must be written by a process in
the target's user namespace **or its parent** (D5 §9.1 `[F]` HIGH, from `user_namespaces(7)`). A
process cannot write its own. This is the entire reason `I` exists.

**(c) The parent must survive.** Someone has to poll the pidfd, report the exit status, tear down the
cgroup and release the IP. `[F]` HIGH — if PID 1 of the new pidns dies, every process in that
namespace is `SIGKILL`ed and a subsequent `fork` into it returns `ENOMEM` (D1 §7.4, CONFIRMED V1 §E4).

**(d) Some setup needs privilege the child is about to destroy.** Step 37's `PR_CAPBSET_DROP` is
**irreversible**. Everything that needs `CAP_NET_ADMIN`, `CAP_SYS_ADMIN` or `CAP_MKNOD` must be
finished before it.

### 2.2 What is impossible in the *child*

`[F]` HIGH — writing `uid_map` (above). `[F]` HIGH — `CLONE_INTO_CGROUP` placement, because it is an
argument to the `clone3` the parent makes. `[C]` HIGH — netns plumbing *can* be done from the child
in principle, but `CAP_NET_ADMIN` is needed in the **host** netns for the veth's host end, and runc
puts the veth work in the parent (`createNetworkInterfaces()` in `process_linux.go`) and only `lo`
in the child (`setupNetwork`/`setupRoute` in `standard_init_linux.go`) — V1 §C6 rule 8.

**A correction that matters for latency.** D1 §3.5 rule 8 presents "netns plumbing must precede the
child's mounts" as a runc-sourced constraint. **It is not** (V1 §C6 rule 8, V1 §I3). In runc,
`createNetworkInterfaces()` (parent) and `prepareRootfs()` (child) run **concurrently**, rendezvousing
only at `procReady`/`procRun`. The genuine kernel-imposed constraint is narrower:

> `[F]` HIGH — netns plumbing must complete **before the child drops its bounding set (37) and
> installs seccomp (36/41)**, because the netlink calls need the capabilities and must be permitted
> by the profile.

`[R]` MED — keep step 16 where the table puts it; a serial engine is simpler and the ordering is
harmless. **But label it `[R]`, not `[F]`** — steps 16 and 21–27 are independent, and that matters the
day anyone wants to overlap them for start latency (V1 §I3).

### 2.3 The `Threads == 1` invariant — and why `clone3` makes it load-bearing

D2 §8.1 claimed `[F]` HIGH that `clone(2)` with namespace flags is *"subject to exactly the same
constraint as `fork` — it's the same kernel path"*. **CORRECTED** (V2 §8.1), and the correction makes
the conclusion **stronger**:

`[F]` HIGH — the *kernel* path is the same; the *libc* path is not, and the libc path is where the
protection lives. glibc `posix/fork.c`, when the process is multithreaded, takes the NSS lock, the
libio list lock and the malloc arena locks before cloning, and releases them in the child. A raw
`clone3` — which is what step 12 specifies, and what youki does (`libc::syscall(SYS_clone3, …)`) —
runs **none** of that.

> `[F]` HIGH — **the child of a raw `clone3` from a multithreaded process can deadlock in the
> allocator. The child of `fork()` normally cannot.** The moment you choose `clone3` for
> `CLONE_INTO_CGROUP` and namespace flags, you give up glibc's atfork protection (V2 §8.1).

`[R]` HIGH — so step 10 is not belt-and-braces. `Threads == 1` in `S` is the **only** thing standing
between the design and a nondeterministic hang. A reader who knows glibc's atfork behaviour and reads
D2 §8.1 as written will conclude the hazard is overstated and reach for a thread pool; V2 flags exactly
that reader.

`[R]` HIGH — the corollary V7 §8.1 found and calls *"the highest-risk unstated assumption across the
corpus"*: D4 says netns work needs **a dedicated OS thread** (D4 §2.3, §2.6(4), §12.2 — handed to D2
twice); D2 §8.3 says the cloning process must **never have had a second thread**. Both are `[R]` HIGH
and they are incompatible in one process. **The resolution falls out of combining them rather than
compromising either:** D4's requirement is explicitly conditional on *"a multithreaded Rust program"*.
In a single-threaded process the only thread *is* the process, so step 16 is:

```
host_ns = open("/proc/self/ns/net", O_RDONLY|O_CLOEXEC)   // before anything
setns(container_netns_fd, CLONE_NEWNET)
socket(AF_NETLINK, …)                                      // now bound to the container netns
…netlink work…
setns(host_ns, CLONE_NEWNET)                               // restore
```

No thread. The netlink socket stays bound to the netns it was created in — the property D4 wanted the
thread for — and it outlives the `setns` back. `[R]` HIGH — the restore must be on **every** exit path
including the error path: a `Drop` guard, plus `panic = "abort"` in `S` so a panic cannot skip it. And
nothing between the two `setns` calls may do host-netns I/O.

### 2.4 The no-allocation rule — necessary, and *not sufficient*

D1 §11.2 and §10.1 step 7 give the rule as "every `CString` for path, argv and envp must be built
before the clone". `[F]` HIGH — that framing is **MISCLASSIFIED** (V1 §A10/§H4): POSIX scopes
async-signal-safety to forks from *multithreaded* processes. With `Threads == 1` satisfied, no other
thread holds a malloc arena lock. **The `Threads == 1` assert and the no-allocation rule are
alternatives, not a stack** — and a reader who cannot achieve `Threads == 1` must understand that the
no-allocation rule does not save them, because with real threads the *whole* async-signal-safety
surface applies.

`[F]` HIGH — two of the four post-clone prohibitions survive **regardless** of thread count, and one
of them for a reason D1 got wrong:
- **`println!`/`eprintln!`** — not because of the lock (single-threaded: no contention) but because of
  the **inherited unflushed buffer**, which duplicates the parent's pending output at the child's exit.
  D1 observed exactly this and attributed it to the lock. Correct the reason; keep the rule.
- **panic/unwind** — unwinds through a stack whose destructors belong to the parent's logical program,
  and the default hook writes to that same inherited buffer.

`[F]` HIGH — the **`setxid` rule is the most important one in D1 §11.2 and is CONFIRMED** (V1 §H4):
glibc's `fork()` wrapper resets its thread bookkeeping in the child; a raw `clone()` does not, so the
child inherits a thread list describing threads that do not exist and glibc's `setuid` broadcast waits
forever. **Step 37 must use raw `syscall(SYS_setresuid)` / `SYS_setresgid` — absolute.**

Now the part the corpus nearly missed. D2 §12.1.3's `[R]` HIGH fix is *"the child's setup functions
take `&CStr` in their signatures and nothing else"*. **CORRECTED — necessary but not sufficient**
(V1 §H4 and V7 §4.3, arrived at independently):

1. `[F]` HIGH — **rustix 1.1.4** (`src/path/arg.rs`) copies `&str`/`&Path`/`&OsStr` into a stack buffer
   of `SMALL_PATH_BUFFER_SIZE` and falls through to `with_c_str_slow_path`, which under
   `feature = "alloc"` is literally `f(&CString::new(bytes)…)` — **a heap allocation on the path to the
   syscall**. **nix 0.31.3** has the identical `with_nix_path_allocating` fallback. It is
   *length-dependent*, so it passes every short-path test and fails on a real
   `/var/lib/mydocker/overlay2/<64-hex>/merged/…`. Both crates' `impl … for CStr` is the zero-copy path.
2. `[F]` HIGH — **the seccomp program.** `seccompiler-0.5.0` `src/backend/bpf.rs:9`:
   `pub type BpfProgram = Vec<sock_filter>`. Compiling a filter allocates. **The fix is clean and must
   be written down as a rule:** the same file defines `pub type BpfProgramRef<'a> = &'a [sock_filter]`,
   and `apply_filter` takes it, builds `sock_fprog` **on the stack**, and allocates nothing. So —
   *compile the filter in `S` before the clone, pass the child a `&[sock_filter]`* (V7 §4.3).
3. `[F]` HIGH — **argv/envp.** An array of pointers is not a `&CStr`. Pass a pre-built
   `&[*const c_char]`.
4. `[F]` HIGH — **`std::fs` in the child.** D5 §18.5 has the child writing `/proc/self/attr/*` and
   sysctls with `std::fs::write`, which opens a `File` and allocates. **The child does `openat`/`write`
   on pre-opened fds, never `std::fs`.**

`[R]` HIGH — the enforcement mechanism that covers all four, and the only one that does not depend on
auditing every transitive call: a `#[global_allocator]` shim whose `alloc` writes one byte to the error
pipe and `_exit`s once a static `IN_CHILD` flag is set. ~20 lines. It converts four nondeterministic
deadlocks into a deterministic, loud failure at the exact call site (V7 §4.3).

---

## 3. Step by step

### 3.1 Steps 1–3 — CLI, image resolution, layer extraction (`L`, `X`)

**Who.** `L` parses; `X` pulls and extracts. `[R]` HIGH — `X` is a **separate process** (§0).

**Root?** No. The store lives under a directory the engine owns.

**MVP?** Yes — but `alpine` is one layer, which conveniently hides most of §3.2's hazards.

**Failure cases.** Registry auth, digest mismatch, disk full — all loud and all in `X`, which is why
putting them there is free.

`[R]` HIGH — **the extraction safety property is a three-part invariant, not one call.** D3 §6 frames
`openat2(RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` as *"the whole answer"*. **REFUTED** (V4 §V4-3.2):

- `[F]` HIGH — **`RESOLVE_IN_ROOT` clamps silently and never reports the attack.** `../../etc/passwd`
  becomes `<root>/etc/passwd` and is *created there*. Containment holds; image correctness does not.
  A hostile layer overwrites `<root>/etc/passwd` by naming an entry `../../etc/passwd`, and nothing
  errors. **`RESOLVE_BENEATH` returns `EXDEV`** instead — measured on the host by V4.
  `[R]` HIGH — use `RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV`, and reject `..`
  **lexically** as well.
- `[F]` HIGH — **`RESOLVE_NO_XDEV` is missing from D3's recommendation** and is needed: measured, a
  bind/tmpfs mount planted inside the extraction root is crossed silently without it (`st_dev` 71 vs
  root 40), and D3 §7.4's "everything on one filesystem" invariant then breaks with no signal.
- `[F]` HIGH — **the guarantee is destroyed by mixing in one non-`openat2` call.** There is no
  `openat2` form of `mkdirat`, `symlinkat`, `linkat`, `mknodat`, `unlinkat`, `fchownat`, `fchmodat`,
  `utimensat` or `setxattr`. V4 measured the escape: resolve the parent with `openat2`, then use a
  plain `openat` with a two-component remainder → `/etc/hosts`, **escaped**. In particular
  `fchmodat` on Linux has no working `AT_SYMLINK_NOFOLLOW`, so a chmod applied to a name an earlier
  entry turned into a symlink *is literally RUSTSEC-2026-0067*.

> `[R]` HIGH — the safety is `openat2(RESOLVE_BENEATH|…)` **plus** strictly single-component
> basenames **plus** unlink-before-create. Drop any one and the hole reopens. Assert
> `!name.contains('/')` in the `SafeRoot` API's **type**, not in a comment (V4 §V4-3.2).

### 3.2 Steps 4–5 — the overlay (`S`)

**Who.** `S`, on the host mount namespace — with a real alternative in §3.5.

**Primitive.** `mount("overlay", merged, "overlay", 0, "lowerdir=…:…,upperdir=…,workdir=…")`, or
`fsopen`/`fsconfig`/`fsmount`/`move_mount`.

**Root?** Yes for the `mount(2)` path outside a user namespace. Rootless needs kernel ≥ 5.11 and
`userxattr`, else `fuse-overlayfs` (D5 §16.1).

**MVP?** Yes.

**The failure that must be called out because it is silent.** D3 §8.5/§15.2 claims an over-length
option string fails with `ELOOP`. **REFUTED** (V4 §V4-2.1/§V4-2.3):

> `[F]` HIGH — `mount(2)`'s `data` argument is **silently truncated at 4095 bytes by
> `copy_mount_options()` and parsed as written**. The overlay mounts. Layers past the cut are simply
> **missing**, and there is **no error**. V4 reproduced it repeatedly on the host; `ELOOP` is the
> minority case, not the characteristic errno.

`[F]` HIGH — worse, V4 §V4-2.4 records a case D3 does not have at all: **a read-write overlay silently
becomes read-only** when the truncation lands such that `upperdir`/`workdir` are lost.

`[R]` HIGH — the escape and its exact ceilings (V4 §V4-1.1, all measured):
- `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", <one path>)`, **one path per call**, escapes the limit
  because `fsconfig` never calls `copy_mount_options()`.
- **Kernel floor v6.7**, not v6.8 — `fs/overlayfs/params.c` at tag v6.7 already has
  `fsparam_string("lowerdir+", …)`; v6.6 has none of it. The kernel's own `overlayfs.rst` says v6.8
  and is one release conservative (V4 §V4-1.3). **CORRECTED** from D3's `[F]` HIGH.
- **Hard cap 500 layers** — `OVL_MAX_STACK` in `fs/overlayfs/params.h`; the 501st `lowerdir+` returns
  `EINVAL`. D3 §15.5 lists the ceiling as an open question after testing 500; it landed *exactly on
  the constant*. **§15.5 is closed.**
- **Each `lowerdir+` value is capped at 255 bytes** — `strndup_user(_value, 256)` in `fs/fsopen.c`.
- **The trap:** the *non-append* `fsconfig(SET_STRING, "lowerdir", …)` is capped at **255 bytes too**
  — 16× worse than `mount(2)`. "Use the new mount API" is **not** the escape hatch; "use `lowerdir+`,
  one path per call" is. A fallback reaching for `fsopen`+`lowerdir` on 6.5/6.6 fails at 255 bytes.
- `[O]` MED — the resulting `/proc/self/mountinfo` line for a 500-layer mount is **15,085 bytes**, and
  the options render as repeated `lowerdir+=` keys. Anything parsing `/proc/mounts` for `lowerdir=`
  will not find it.

`[R]` HIGH — so: **feature-probe `lowerdir+`, do not version-gate it** (D5 §6.2, called by V6 §I.2
*"the strongest recommendation in the document"*), and on kernels below 6.7 **assert the assembled
option string is < 4096 bytes and refuse** rather than let the kernel truncate.

`[F]` HIGH — other errors here: `EBUSY` on a reused `upperdir`/`workdir` (but V4 §V4-5.3 **REFUTES**
this as a *guarantee* — do not rely on the kernel to catch a double-use); `[F]` HIGH — a read-only
overlay needs **≥ 2** lower layers (V4 §V4-5.4, a hidden dependency in D3 §11.4 step 6), which
single-layer images hit immediately.

**Ordering.** Must precede step 12 if done on the host, because the child inherits the host mount
table at clone time.

### 3.3 Steps 6–7 — state directory and cgroup (`S`)

**Who.** `S`. **Root?** Yes for a cgroup under `/sys/fs/cgroup`; rootless is a *found* path (§4).

**MVP?** Yes for the directory and the `mkdir`; limits are the resource stage.

**Ordering — kernel-forced.** `[F]` HIGH — **the cgroup and its limits must exist before the process
does** (D5 §20 rule 7). Writing limits after placing the process is a window in which the container is
unconstrained. `CLONE_INTO_CGROUP` makes even the *placement* window vanish.

`[F]` HIGH — the **no-internal-process rule**: a cgroup may not simultaneously have member processes
and enable controllers for its children (D5 §1.3, CONFIRMED V6 §C.1/§C.3 — *"the document's careful
reading is right"*). This decides the layout: `<root>/` enables controllers in `subtree_control`,
`<root>/<id>/` holds the processes. `[F]` MED — the errno is `EBUSY` in both directions
(V6 §C.2: **UNSUPPORTED as stated, but correct** — asserted without a citation, and it checks out).

`[F]` HIGH — V6 §C.4 confirms the consequence D5 §1.3 draws: **a later per-container `exec` sub-cgroup
would require moving a live PID 1.** Decide the layout now; it is expensive to change.

`[F]` HIGH — **read the limits back and report declared-vs-effective.** V6 §D.2 CONFIRMED and
re-measured D5 §2.4's finding that a live container's `pids.max = 19147` came from **systemd's
`DefaultTasksMax`, not Docker**. What you wrote is not necessarily what is in force.

**Failure cases.** `EACCES` on `mkdir` (no delegation) — the rootless wall, §4. A controller absent
from `cgroup.controllers` because the *parent* never enabled it. `[F]` HIGH — `rmdir` on teardown
succeeds while the cgroup is still *dying*; freed is not the same as removed (D5 §4.7, CONFIRMED and
strengthened by V6 §K.2).

### 3.4 Steps 9–11 — the fds, the assert, and the pre-build (`S`)

**Who.** `S`. **Root?** No. **MVP?** All three, yes.

`[F]` HIGH — `exec.fifo` is `mkfifo(name, 0622)` **followed by an explicit `chmod(0622)`** — the chmod
is a umask defence, and mode `rw--w--w-` is deliberate: the owner reads, **everyone writes**, because
the container's init may be an unprivileged uid inside a userns (D2 §3.2; CONFIRMED verbatim against
runc `main` by V2 §3.2, including line numbers).

`[F]` HIGH — the fd handed to the child is **`O_PATH`**, because a real `O_WRONLY` open would itself
block *in the parent*. The child re-opens it through `/proc/self/fd/N`.

**Step 10 and 11 are covered in §2.3 and §2.4.** They are listed as separate steps and not folded into
step 12 deliberately: they are the two steps most likely to be dropped as ceremony, and each of them is
load-bearing.

### 3.5 The genuine design fork: where the overlay is mounted

`[R]` MED, and the corpus is **split for a good reason**.

- **Host-side (step 5, the table's default).** Matches Docker/containerd: the snapshotter mounts, then
  runc receives an already-assembled bundle. `S` can `stat` the rootfs before starting. Teardown is
  `S`'s job and can leak.
- **Child-side (a `C` step between 21 and 23).** D2 §9.2 `[R]`: *"do as little as possible in the host
  mount namespace, and everything else inside the new one"* — which V2 §9.2 calls **"the single best
  idea in §9"**, because `[F]` HIGH (`mount_namespaces(7)`) **every mount in a mount namespace is torn
  down when the last member exits**. Failure atomicity for free, no unwind code, no leak. It also
  subsumes step 23: an overlay mounted at `merged/` *is* a mount point, so the recursive self-bind
  becomes unnecessary.

`[R]` MED — **prefer child-side**, and note the cost honestly: the mount option string must be
pre-built (§2.4), and anything that wants to inspect the rootfs from the host must go through
`/proc/<pid>/root`. The corpus does not settle this; both are defensible and D3 assumes the first while
D2 §9.2 argues for the second.

### 3.6 Step 12 — `clone3` (`S`)

**Who.** `S`, and only `S`.

**Primitive.**
```c
struct clone_args a = {
  .flags = CLONE_NEWNS|CLONE_NEWPID|CLONE_NEWNET|CLONE_NEWUTS|CLONE_NEWIPC|CLONE_NEWCGROUP
         | CLONE_PIDFD | CLONE_INTO_CGROUP,
  .pidfd  = (__u64)&pidfd,
  .cgroup = (__u64)cgroup_dirfd,
  .exit_signal = SIGCHLD,
};
syscall(SYS_clone3, &a, sizeof a);
```

**Root?** Yes, without a user namespace: `CAP_SYS_ADMIN` in the current userns for every `CLONE_NEW*`
flag except `CLONE_NEWUSER`.

**MVP?** Yes.

**This is where the brief's sequence is most wrong.** `[F]` HIGH — "clone/create process" then
"namespaces" is backwards: **namespaces are created *by* the clone**, as flags in the same syscall
(D1 §10, no V1 objection). There is no separate "configure namespaces" step. Only the user-namespace
path needs a separate `unshare`, and that is for a different reason (§3.7).

`[F]` HIGH — `CLONE_NEWPID` in the clone puts the child in the new namespace **as PID 1**. The "extra
fork for PID namespaces" the brief inherits from folklore is an `unshare`/`setns` artefact, not a
`clone` one — `pid_namespaces(7)`, verbatim: *"The first process created in a new namespace (i.e., the
process created using `clone(2)` with the `CLONE_NEWPID` flag …) has the PID 1"* (D1 §3.3, CONFIRMED
V1 §C3).

`[F]` HIGH — `CLONE_INTO_CGROUP` requires **kernel ≥ 5.7**, `clone3` itself ≥ 5.3, and
`CLONE_INTO_CGROUP` is **cgroup-v2 only** (V1 §C9, V2 §4 rule 2). `[O]` HIGH — `syscall(435,0,0)`
returns `EINVAL` (22), not `ENOSYS` (38), on the host, which proves the syscall number is
implemented (D2 §4, CONFIRMED V2 §4 as a valid availability inference).

`[F]` HIGH — **`CLONE_INTO_CGROUP` is load-bearing for correctness, not hardening.** D1 §9 tiers it
"strongly recommended"; V1 §F3 upgrades it: between `clone` and a `cgroup.procs` write the child sits
in the **parent's** cgroup and can fork, spike CPU, or allocate past the memory limit. That is not a
nicety, it is a limit that does not hold. runc pays for the missing atomicity with a synchronisation
protocol, and says so in-tree — *"Do this before syncing with child so that no children can escape the
cgroup"* (verified verbatim at `process_linux.go:822`, V2 §3.4).

`[R]` HIGH — the pre-5.7 fallback is **the parent writes the child's pid to `cgroup.procs` before
releasing the child from the sync pipe**, which is what runc does. D2 §4 rule 2(a) offers "child writes
its own pid as its first act"; V1 §I2 flags this as **D2's invention** — no runtime does it — and it
carries an unstated precondition: the cgroupfs must still be reachable by path from the child, which is
true only before `pivot_root` and the `MNT_DETACH`. **Use the parent-write fallback**, which needs no
cgroupfs inside the container at all.

**Failure cases.** `ENOSYS` → fall back to legacy `clone` (and lose `CLONE_INTO_CGROUP`). `EPERM`
without `CAP_SYS_ADMIN`. `ENOSPC` on a namespace `ulimit`; `[F]` MED — `EUSERS` is scoped to nesting
depth, not the per-user count (V1 §D1b, **CORRECTED** scope). `EBADF`/`EOPNOTSUPP` on the cgroup fd.
`[F]` HIGH — `clone3` returns `EINVAL` if `CLONE_PARENT` is set together with a non-zero `exit_signal`,
whereas legacy `clone` silently ignores the signal bits (`clone3_args_valid()`; CONFIRMED V1 §C9) —
a good catch of D1's and it bites the three-stage userns topology.

**Combinations the kernel rejects that D1 §3.6 misses** (V1 §C8, all from `clone(2)`/`setns(2)`):
- `CLONE_NEWUSER` + `CLONE_PARENT` → **rejected**. This lands directly on the userns topology: the
  user namespace must come from a separate `unshare`, **never** from the clone flags.
- `CLONE_PARENT` is forbidden to **any** namespace init, including one in a non-initial pidns. Matters
  if the engine ever runs as PID 1.
- `CLONE_NEWNS` + `CLONE_FS` in the same `clone()` → not permitted.
- `unshare(CLONE_NEWUSER)` from a chroot → `EPERM` since Linux 3.9.
- `setns(CLONE_NEWTIME)` multithreaded → **`EUSERS`**, not `EINVAL` — the *second* namespace operation
  with a single-threaded requirement (V1 §C1).

`[O]` HIGH, **this session** — the live nginx/python container on the host has distinct
`cgroup/ipc/mnt/net/pid/uts` namespaces and shares `user` (`4026531837`) and `time` (`4026531834`) with
the host. **Six of eight**, exactly as D1 §0.1 measured and V1 §E7 re-measured. That is the MVP target.

### 3.7 Steps 12a–14 — the user-namespace variant (`I` + `S`)

**MVP? No.** This is the rootless stage. It is in the trace because its *shape* constrains the MVP's
process structure, and retrofitting it is expensive (D5 §17.2).

**Who and why three processes.** `[F]` HIGH — a process cannot write its own `uid_map`; the writer must
be in the target's userns or its parent (D5 §9.1). So:

```
S  : clone(I)                                  (no CLONE_NEWUSER in the flags — see §3.6)
I  : unshare(CLONE_NEWUSER) alone, then block on the sync pipe
S  : write "deny" > /proc/<I>/setgroups
     write <map>  > /proc/<I>/uid_map
     write <map>  > /proc/<I>/gid_map
     release the pipe
I  : unshare(the rest), then clone(C); exit
```

`[F]` HIGH — the ordering inside `S`'s three writes is **kernel-forced**: `setgroups` must be `deny`
**before** `gid_map` is writable by an unprivileged writer. The reason is not arbitrary — without it a
user could create a userns, `setgroups(2)` to **drop** a supplementary group, and thereby gain access
to a file whose mode denies the group but permits others (`rwx---rwx`). Group membership can *subtract*
permission in Unix (D5 §9.1).

`[F]` HIGH — each map file may be written **exactly once**; a second write is `EPERM`. Up to **340**
lines (5 before Linux 4.15). A multi-line map requires `CAP_SETUID` in the **parent** userns; the
unprivileged path is a single line mapping the writer's own euid (D5 §9.1).

`[F]` HIGH — **the timing is a correctness issue, not a convenience.** Capabilities gained from
`CLONE_NEWUSER` do **not survive `execve`** unless uid 0 is mapped: a non-setuid exec by a nonzero euid
with an empty ambient set gives `P'(permitted) = F(inheritable) & P(inheritable)` and
`P'(effective) = 0`, so `CapEff` goes to zero (D1 §1.6, derivation **CONFIRMED as sound** by V1 §C7).
So step 14 must land before step 42.

`[F]` HIGH — `unshare(CLONE_NEWUSER)` is the **only** namespace flag a multithreaded process cannot
unshare (`ksys_unshare()` adds `CLONE_THREAD|CLONE_FS` for `CLONE_NEWUSER` only). `[F]` HIGH, **new
finding** — `unshare(2)`'s own man page says *"`CLONE_NEWPID` automatically implies `CLONE_THREAD`"*,
which is **wrong**: D1 §3.4's own measured table (`CLONE_NEWPID → rc=0` at `Threads: 4`) proves it, and
D1 did not notice (V1 §C5). Do not trust that paragraph.

`[F]` HIGH — **do not probe `/proc/<pid>/ns/pid_for_children` to confirm `unshare(CLONE_NEWPID)`
succeeded.** D1 §1.2 says the link *changes*; **CORRECTED** (V1 §C4) — it goes **empty**, and `readlink`
returns `ENOENT`, until the first child is created. D1's transcript appears to contradict this only
because its `readlink` ran as a forked child, which is precisely the event that populates the link.

### 3.8 Steps 13–20 — the parent side (`S`)

**Step 13 — record the pidfd.** `[R]` HIGH — use pidfds, not PIDs, for every subsequent signal and
wait (D1 §8.2). PID reuse across a container's lifetime is a real signal-delivery hazard.

**Step 13 also, in runc — and this one can be dropped.** runc records `/proc/<pid>/fd/{0,1,2}` targets
into `state.json` as `external_descriptors`. D2 §4 and §6.1 assert `[C]` HIGH that this is *"how `runc
exec` wires a new process's stdio"*, and D2 §6.4/§7.5 build on it. **REFUTED** (V2 §4/§6.1):
`externalDescriptors()` has exactly two consumers in runc `main` — the state serializer and
`criu_linux.go:489`, which marshals them into CRIU's `descriptors.json`. There is **no reader on the
`exec` path**; `runc exec` wires stdio from the **caller's** fds. runc's own comment at the capture site
says *"…we won't know **at checkpoint time** which file descriptor to look up."* D2 puts CRIU out of
scope, so `external_fds` in the §6.4 state model has **no consumer at all**. `[R]` HIGH — **delete it.**
A free simplification the corpus argued itself into carrying.

Note also that D2 §4 lists `waitForChildExit` before `getPipeFds`; **CORRECTED** (V1 §I1, V2 §4) —
current runc records the fds *first*, and D2's own stated rationale argues for runc's order, not D2's.

**Step 15 — cgroup verification.** Covered in §3.6.

**Step 16 — netns plumbing.** Covered in §2.2 and §2.3. `[F]` HIGH — the kernel constraint is only
"before 36/37", not "before the child's mounts".

**Step 17 — `createRuntime` hooks.** Not MVP. `[F]` HIGH — `createRuntime` is the right hook and the
only one: it runs after the namespaces exist, before `pivot_root`, and **in the runtime's namespace**,
so it can reach `/proc/<pid>/ns/net` (D4 §11, CONFIRMED V5 §F4). `[C]` HIGH — the hooks fire **after**
the cgroup is applied, so a `createRuntime` hook runs under the container's cgroup limits even though
it runs in the runtime's namespaces. Non-obvious, and the source of "my CNI plugin got OOM-killed"
reports (D2 §4).

`[R]` HIGH — **build networking in; do not make it a hook** (D4 §11, CONFIRMED and raised to HIGH by
V5 §F4). Three spec properties decide it: `createRuntime` failure is fatal (good); **`poststop` failure
is *specified* to be ignored** (so every leak in §6 becomes mandated behaviour); and **a hook has no
return channel** — a hook that allocates `172.18.0.5` cannot tell the runtime what it allocated.
*"An allocation that cannot be reported is not an interface, it is a side effect."*

**Step 18 — write the state file when the child says ready.** `[C]` HIGH, and `[R]` HIGH — carry an
explicit `status` field written at each transition rather than runc's trick of inferring
`created`-vs-`running` from the existence of `exec.fifo` (D2 §3.2; the runc mechanism CONFIRMED
verbatim by V2 §3.2, the preference for an explicit field is ours).

**Step 20 — supervision.** `[F]` HIGH — `PR_SET_CHILD_SUBREAPER` on `S` buys reaping of orphans whose
dying parent was at the **host** pid-namespace level. That covers the container init itself. See §7
for what it emphatically does **not** cover.

### 3.9 Steps 21–23 — mount propagation and the `pivot_root` preconditions (`C`)

**Who.** `C`. Impossible in the parent (§2.1a).

**Root?** Yes — `CAP_SYS_ADMIN`, or `CAP_SYS_ADMIN` in the owning userns for a userns container.

**MVP?** Yes, all three. Skipping step 21 is the most expensive single omission in the trace.

**Step 21 — what is actually required.** D1 §9.3 tiers `MS_REC|MS_SLAVE` on `/` as **required**.
**MISCLASSIFIED** (V1 §A11/§F2), and D1's own §13 open question 8 already said the choice was
unsettled:

> `[F]` HIGH — **what the kernel requires is only that `/` (and `new_root`'s parent mount) not be
> `MS_SHARED`.** `MS_SLAVE`, `MS_PRIVATE` and `MS_UNBINDABLE` all satisfy it.

**This is genuinely unsettled between mature runtimes, and the corpus does not settle it.**
- `[C]` HIGH — runc uses `MS_REC|MS_SLAVE` (`prepareRoot`, verified verbatim at v1.5.1 by V1 §D5);
  youki matches.
- `[C]` HIGH — **crun defaults to `MS_REC|MS_PRIVATE`**, which D1 itself records in §4.2 and §5.3.
- `[F]` HIGH — **`pivot_root(2)`'s own EXAMPLES program uses `MS_REC|MS_PRIVATE`**, with the comment
  *"Ensure that 'new_root' and its parent mount don't have shared propagation"*. The canonical man-page
  example takes the option D1's table forbids (V1 §F2).
- `[C]` MED — D1's stated rationale for slave (host→container propagation keeps volume lifecycle
  working; runc cites devicemapper races) is **unverified**: V1 could not find the devicemapper
  rationale in v1.5.1's `prepareRoot` at all. It is either in the history or gone.

`[R]` MED — **follow runc: `MS_REC|MS_SLAVE`.** The tradeoff is explicit and should be stated rather
than hidden: slave admits host→container propagation, which volume lifecycle wants and which is *also*
a one-way behaviour channel into the container. Do not invent a third answer. Record it as a choice,
not a requirement.

**Why any of it.** `[O]` HIGH (D1 §4.2, host) — a mount created inside a child mount namespace on a
mount point whose propagation is `shared` **appears in the parent namespace and survives the child
namespace's death**. Without the propagation change, every container's `/proc`, `/dev`, `/dev/pts`,
`/dev/shm`, `/sys` and overlay rootfs lands permanently in the host mount table — one growing
`/proc/mounts` per container start — and `umount` events propagate too, so a container can unmount
things on the host.

`[O]` HIGH — **a trap that will mislead anyone experimenting at a shell:** `util-linux 2.40.2`'s
`unshare -m` **silently defaults to `--propagation private`**. D1's first `pivot_root`-with-shared-`/`
test therefore "passed" when it should have failed. Only `unshare -m --propagation unchanged`, nested
inside a protective private namespace, reproduces real kernel behaviour (D1 §4.2).

**Step 22 — the row names the wrong mount.** `[F]` HIGH — **CORRECTED** (V1 §F1): D1 §9.3 says
"non-recursive `MS_PRIVATE` on the rootfs mount"; the kernel precondition is on the **parent mount of
`new_root`**, which is what runc's `rootfsParentMountPrivate` loop actually achieves by walking up on
`EINVAL`.

**Step 23.** `[F]` HIGH — `pivot_root(2)`: *"`new_root` must be a path to a mount point"*. Reproduced
as an `EINVAL` by V1 §B2 T1. A recursive self-bind satisfies it; so does an overlay mounted there
in-child (§3.5).

### 3.10 Steps 24–27 — the mount set (`C`)

**Who.** `C`, necessarily — procfs is tied to the mounter's pidns, sysfs to the netns, mqueue to the
ipcns. `[F]` HIGH, and V1 §D9 **promotes these from single-host `[O]` to `[F]`**.

**Step 24 — the cgroup namespace, and an open question.** `[F]` HIGH — `cgroup_namespaces(7)`:
*"When a process creates a new cgroup namespace … **its current cgroups directories become the cgroup
root directories of the new namespace**."* So **cgroup placement must precede the cgroup namespace**.
D1 §3.5 rule 5 cited only a youki comment; V1 §C6 **upgrades it to `[F]`** — it is kernel behaviour,
not runtime convention.

> **Open question, not settled by the corpus.** Step 12 puts `CLONE_INTO_CGROUP` and `CLONE_NEWCGROUP`
> in the *same* `clone3`. Whether the kernel resolves placement before capturing the namespace root
> within one call is not established anywhere in D1–D5 or V1–V7. `[R]` MED — until it is, either
> (a) `unshare(CLONE_NEWCGROUP)` in `C` **after** placement is observable, i.e. step 24; or
> (b) verify. The experiment is one `clone3` on a non-production host followed by
> `cat /proc/self/cgroup` in the child: `0::/` means the namespace root is the destination cgroup,
> anything else means it is the parent's. `[O]` — this **cannot** be settled by reading the live
> host: `/proc/<pid>/cgroup` is rendered relative to the **reader's** cgroup namespace, so reading a
> container's from the host correctly shows the full
> `/system.slice/docker-<id>.scope` path regardless (confirmed this session).

`[F]` HIGH — **`CLONE_NEWCGROUP` is over-tiered anyway.** D1 §9 calls it "hardening"; V1 §F3 is
generous-to-blunt: it hides a path that is *also* derivable from `/proc/self/mountinfo` unless you fix
that up too, and its stated justification ("tells an attacker where to write") is work the **read-only
cgroupfs mount already does**. Closer to Docker convenience. Not worth arguing about; worth not
spending effort on.

**Step 25 — the set, and the tier corrections.** From D1 §6.9, with V1 §F1's reclassifications applied:

| Mount | fstype | Corrected tier | Breakage without it |
|---|---|---|---|
| `/` (self-bound / overlay) | image fs | **required** (kernel) | `pivot_root` `EINVAL` |
| `/proc` | proc | **strongly recommended** — *not* required | D1 §9 says required; **MISCLASSIFIED** (V1 §F1) — D1's own §6.1 shows `id`, `python3`, `openssl`, `df` all work with no `/proc`. That is exactly "container works, real software breaks" |
| `/dev` (tmpfs) | tmpfs | **required in practice** | `/dev/null` writes silently create a growing regular file |
| `/dev/null,zero,full,random,urandom,tty` | nodes | **required in practice** | as above; `/dev/tty` for `sudo`, `ssh`, `gpg` |
| `/dev/pts` | devpts | **required** for this MVP | `openpty()` → "out of pty devices"; no `-it`. V1 §F3: since step 33 depends on it, for `mydocker run alpine /bin/sh` it is simply required |
| `/dev/shm` | tmpfs | strongly recommended | `shm_open`, `sem_open`, Python multiprocessing, PostgreSQL, Chromium |
| `/sys` (ro) | sysfs | strongly recommended | `lsblk`/`lscpu`/`lspci`/udev/D-Bus |
| `/sys/fs/cgroup` (ro bind) | cgroup2 | convention | JVM/.NET/Node mis-size heaps against host RAM |
| `/dev/mqueue` | mqueue | convention | POSIX mqueues only |

`[F]` HIGH — `nosuid,nodev,noexec` on `/proc` and `ro,nosuid,nodev,noexec` on `/sys` (CONFIRMED
verbatim in live mountinfo, V6 §F.4). `[F]` HIGH — devpts `newinstance` has been a **no-op since
v4.7** (`devpts: Make each mount of devpts an independent filesystem`, Eric W. Biederman, 2016-06-05);
D1 §13 open question 3 is **SETTLED** (V1 §D11).

**Step 26 — device nodes, and a decision this forces.** `[F]` HIGH — `CAP_MKNOD` gates char/block
special files only; `mknod`/`mknodat` for regular files, FIFOs and UNIX sockets need **no capability**
(`vfs_mknod` checks the cap only for `S_IFCHR`/`S_IFBLK`) — so `mkfifo` and socket creation keep
working (V6 §J.2). `[O]` HIGH — a live container's `/dev` is `rw,nosuid` and **not `nodev`**, and the
container is in the initial userns, so `CAP_MKNOD` is fully functional there (V6 §J.2, re-measured).

`[R]` HIGH — **drop `CAP_MKNOD`, and treat that as required rather than optional.** V6 §J.2
**reclassifies** D5 §19.2's "hardening (deviation)": because the design also declines the eBPF device
controller, dropping `CAP_MKNOD` is *the* load-bearing compensation for having no device cgroup. If
someone later restores it for compatibility, the device story collapses **silently**. Gate `--cap-add
MKNOD` the way `--cap-add SYS_ADMIN` is gated. The cost V6 adds that D5 misses: images that create
nodes at runtime — `/dev/fuse`, `/dev/loop*`, `/dev/net/tun`, Docker-in-Docker — and with no eBPF
controller the only answer is `--device`.

**Failure cases.** `[F]` HIGH — rootless: `mknod` **always** fails `EPERM`, even with a full effective
capability set inside a userns; device nodes must be **bind-mounted** instead (D5 §16.1, D1 §6.2).
This is the "quiet" class: `CapEff` shows 41 caps and `mknod` still returns `EPERM`.

**Step 27 — user bind mounts.** `[F]` HIGH — `MS_BIND` **ignores** the flag arguments on the first
pass; a second `MS_REMOUNT|MS_BIND` call is needed to set `nosuid`/`nodev`/`noexec`/`ro`. Recursive
read-only needs `mount_setattr(AT_RECURSIVE)` (≥ 5.12).

### 3.11 Step 28 — `pivot_root` (`C`)

**Who.** `C`. **Root?** Yes. **MVP?** Yes.

**The sequence, verbatim from runc v1.5.1 `pivotRoot()` — CONFIRMED unchanged by V1 §B5:**
```
oldroot = open("/",       O_DIRECTORY|O_RDONLY|O_PATH)
newroot = open(rootfs,    O_DIRECTORY|O_RDONLY|O_PATH)
fchdir(newroot)
pivot_root(".", ".")                       // legal and idiomatic; pivot_root(2) NOTES documents it
fchdir(oldroot)
mount("", ".", "", MS_SLAVE|MS_REC, "")    // detach propagation before unmounting
umount2(".", MNT_DETACH)
chdir("/")
```

**Six preconditions, not five.** D1 §5.2 says "all five preconditions, measured … every documented
precondition reproduces exactly". **REFUTED** (V1 §A3/§B2). The man page lists **six**; D1's own §5.1
block quote contains all six; §5.2 tests four and never tests *"`new_root` and `put_old` must be
directories"* (`ENOTDIR`), *"must not be on the same mount as the current root"* (`EBUSY`), or *"the
current root directory must be a mount point"* (`EINVAL`).

**Worse: three of D1's four tests were confounded.** `[F]` HIGH — in the kernel's
`SYSCALL_DEFINE2(pivot_root, …)` the shared-propagation test
(`IS_MNT_SHARED(old_mnt) || IS_MNT_SHARED(ex_parent) || IS_MNT_SHARED(root_parent)`) is evaluated
**before** the `EBUSY` loop test and before the `path_mounted()` tests. D1 ran C1, C4 and C5 while `/`
was still `shared:513` — so every one failed at the *same* check and their `EINVAL`s carry no
information about the precondition each claimed to demonstrate.

`[O]` HIGH — V1's re-run, host 6.11.0-29, in a private mount namespace, scratch under `/tmp`, host
mount table verified unchanged (0 leaks):

```
                                    / private        / shared
T1 new_root not a mount point   ->  EINVAL           EINVAL
T2 new_root == "/"              ->  EBUSY   <----    EINVAL   <---- confound
T3 put_old not under new_root   ->  EINVAL           EINVAL
T4 put_old is a regular file    ->  ENOTDIR          ENOTDIR
T5 fully valid pivot_root       ->  0 (OK)           EINVAL   <---- the only informative row
```

> `[F]` HIGH — **`new_root == "/"` returns `EBUSY`, not `EINVAL`.** D1 §5.2 C5 is **REFUTED**
> (V1 §A4). `pivot_root(2)` ERRORS documents it: *"`EBUSY` `new_root` or `put_old` is on the current
> root mount. (This error covers the pathological case where `new_root` is `"/"`.)"*

`[F]` HIGH — **CORRECTED** (V1 §B4): D1 §10.1 step 17's "5 documented `EINVAL` preconditions" is wrong
twice over. `pivot_root(2)` ERRORS has **six** `EINVAL` entries, **plus `EBUSY`, `ENOTDIR` and
`EPERM`**. An error-handling path written to D1's table will mis-report `EBUSY` and `ENOTDIR` — and V1
hit `ENOTDIR` on the first try.

**Every one of these is required, and the tier survives its own definition** (V1 §F1):
- `[F]` HIGH — `umount2(put_old, MNT_DETACH)` is **required**: an accessible `put_old` is a one-line
  escape.
- `[F]` HIGH — `chdir("/")` afterwards is **required**: the kernel does **not** move the caller's cwd.
  V1 §B3 independently reproduced this — after a valid `pivot_root` the cwd was still `/put/root`,
  reachable under `put_old`. This is the single most useful observation in D1 §5.

**Why not `chroot`.** `[F]` HIGH — `chroot(2)` says it itself: *"it is **not intended to be used for
any kind of security purpose**"*, and gives the escape recipe in its own man page. `[F]` HIGH —
**CORRECTED** (V1 §B6): D1 says the escape *"requires `CAP_SYS_CHROOT`"*. That is true only of the
final `chroot(".")`. **The load-bearing half — hold an `O_DIRECTORY` fd across the `chroot`, `fchdir`
it, read the entire host filesystem by relative path — requires no capability at all.** So dropping
`CAP_SYS_CHROOT` from the bounding set does **not** mitigate it. A reader could easily conclude
otherwise.

`[F]` HIGH — the one legitimate `MS_MOVE`+`chroot` case: *"the rootfs (initial ramfs) cannot be
`pivot_root()`ed"* (`pivot_root(2)` NOTES; CONFIRMED V1 §B7). `[R]` HIGH — **MVP: `pivot_root` only.
Do not implement the fallback** — a half-right masking step is worse than an absent one.

### 3.12 Steps 29–31 — post-pivot repair, masking, hostname (`C`)

**Step 29 — re-open `/dev/null`.** `[C]` MED — runc's `reOpenDevNull`. Any fd still pointing at the old
root's `/dev/null` now references a detached tree. Not MVP; cheap.

**Step 30 — masking. Three mechanisms, all CONFIRMED and re-measured on live containers** (V6 §F.1):
- masked **directory** → read-only **tmpfs** over it (fresh superblocks `0:103/104/105` in live
  mountinfo);
- masked **file** → **bind of the container's own `/dev/null`** (source device `0:94` is independently
  confirmed to be the container's `/dev` tmpfs). **So `/dev` must be populated before masking runs** —
  D5's derived requirement, and V6 calls it *"correct and load-bearing"*;
- **readonly path** → self-bind from the same procfs superblock + remount `ro`.

`[F]` HIGH — **skip `ENOENT` and only `ENOENT`.** CONFIRMED against runc's `maskPath` (V6 §F.3). `[O]`
HIGH — 4 of Docker's 12 masked paths do not exist on 6.11 (`/proc/asound`, `/proc/timer_stats`,
`/proc/sched_debug`, `/sys/devices/virtual/powercap`), and moby's *dynamic*
`/sys/devices/system/cpu/cpu0/thermal_throttle` is **also absent** on this virtio host — so the
ENOENT-skip is less optional than D5 argues. (D5's companion count of "7 appear in live mountinfo" is
**CORRECTED to 8**; 12 − 4 = 8, V6 §F.2.)

`[R]` MED — **copy runc's implementation shape, not D5's description.** runc does **not** `stat` the
path to decide file-vs-directory: it tries the `/dev/null` bind unconditionally and uses **`ENOTDIR`**
as the discriminator for "this is a directory, use a tmpfs". One syscall, no type check, no branch.
D5's "masked directory → tmpfs; masked file → /dev/null" describes the outcome and implies a
stat-then-branch design (V6 §F.3).

> `[F]` HIGH — **the requirement D5 omits entirely, and it is a net security regression if missed**
> (V6 §F.4): the read-only remount must **re-specify the existing `MS_NOSUID|MS_NODEV|MS_NOEXEC`
> flags, read back via `statfs(2)`**. `MS_REMOUNT|MS_BIND` sets the per-mount flags to exactly what is
> passed; flags not passed are **cleared**. A naive `MS_REMOUNT|MS_BIND|MS_RDONLY` on `/proc/sys` makes
> it read-only **and simultaneously clears `nosuid`, `nodev` and `noexec`**.

`[R]` MED — D5 recommends `mount_setattr(AT_RECURSIVE)` for the ro-remount on ≥ 5.12, because a
non-recursive read-only bind leaves submounts writable. The principle is right, **but runc does not do
this** — it binds recursively (`MS_REC`) and remounts read-only **non-recursively**. `[O]` — label our
choice a **deliberate deviation**, so it does not get "simplified" back to runc's behaviour in review
(V6 §F.4).

**Step 31 — hostname.** `[F]` HIGH — without `CLONE_NEWUTS` this renames the **host**. Needs
`CAP_SYS_ADMIN` in the owning userns.

### 3.13 Steps 32–34 — the terminal, and the correction that changes the design (`C`)

**Who — and this is the one the corpus got backwards.** D2 §5.2 gives `[C]` HIGH:
```
runc:   openpty()  -> (master, slave)                      <-- on the host, before the container
runc:   dup slave onto container init's 0/1/2
runc:   connect(path) ; sendmsg(SCM_RIGHTS, [master])
```
**REFUTED** (V2 §5.2). runc's parent only **dials the socket and hands the connected fd down to the
child**. The pty is allocated **by `runc init`, inside the container, after `pivot_root`**, and the
master is sent **up** the socket from there. `libcontainer/init_linux.go:370–416`, with runc's own
comment:

> *"setupConsole sets up the console from inside the container, and sends the master pty fd to the
> config.Pipe (using cmsg). This is done to ensure that consoles are scoped to a container properly
> (see runc#814 …). **This has to be run \*after\* we've pivoted to the new rootfs** …"*

`[F]` HIGH — **why it matters concretely:** the pty must come from the *container's* `/dev/ptmx`, i.e.
its own `devpts` instance, mounted inside its own mount namespace, with the container's `gid=`/`mode=`
options. Allocating it on the host and passing the slave in is runc#814 and its family: it leaks a host
resource across the isolation boundary and gets the ownership model wrong.

`[R]` HIGH — **D2 §7.2's process diagram puts `(optional) openpty` in the supervisor, before `clone()`,
on the host — precisely the arrangement runc abandoned.** If `-it` is implemented from that diagram it
will be built the wrong way round (V2 §5.2). The correct shape:

```
S: socketpair(AF_UNIX, SOCK_SEQPACKET)   -> one end to C at clone time
C: (after pivot_root) open("/dev/ptmx")  -> master, peer
C: sendmsg(SCM_RIGHTS, master)           -> up to S
C: dup2(peer, 0/1/2)
C: setsid() ; ioctl(TIOCSCTTY)
```

**Step 33.** `[F]` HIGH — `TIOCSCTTY` returns `EPERM` without session leadership, so `setsid()` must
precede it; `setsid()` itself returns `EPERM` if the caller is already a process-group leader
(CONFIRMED V2 §5.1).

`[F]` HIGH — `[R]` HIGH — **never give a container a file descriptor that refers to the invoking
terminal.** `TIOCSTI` injects into the host shell's input queue; and detached pass-through hangs the
container if `stdout`/`stderr` is a pipe (runc `docs/terminals.md`, CONFIRMED verbatim V2 §5.2/§5.6).

**Step 34 — the working directory.** `[R]` HIGH — resolve `process.cwd` with
`openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV)`, `fchdir`, then **verify `st_dev`**
(D5 §11.4; the flag set corrected per §3.1). `[F]` HIGH — this is the CVE-2024-21626 class: a leaked or
mis-resolved directory reference at `WORKDIR` time is a host-filesystem handle inside the container.
`[R]` MED — D1 §10.1 step 22 suggests trying `chdir` optimistically and retrying after the uid switch
on `EPERM`, since a workdir may only be enterable as the target uid. Keep that, but the resolution
must still go through the safe path.

### 3.14 Steps 36–37 — seccomp branch A, then capabilities (`C`)

**This is the single most consequential error in the corpus, and it is silent.**

D5 §7.7's numbered listing has:
```
 3. setgid(gid) ; setuid(uid)
 4. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)
```
and D5 §20's consolidated sequence copies it (C10 `setuid` before C11 `PR_CAPBSET_DROP`). **CORRECTED**
(V6 §B.4). Four lines below its own listing, D5 §7.7 says the *opposite* in prose — *"Follow runc:
bounding drop → capset → uid change with KEEPCAPS → capset again → ambient"* — and the prose is right.

> `[F]` HIGH — **`PR_CAPBSET_DROP` requires `CAP_SETPCAP` in the *effective* set.** After `setuid`
> away from 0, `CAP_SETPCAP` is gone from E, so **every** `PR_CAPBSET_DROP` fails `EPERM` and the
> bounding set is never dropped. If the return value is ignored, **the container runs with the full
> 41-capability bounding set** and nothing says so.

`[F]` HIGH — **and the bug survives `PR_SET_KEEPCAPS`.** `capabilities(7)`'s uid-change rules are two
separate bullets: the first clears P/E/A when *all* uids leave 0; the second says *"If the effective
user ID is changed from 0 to nonzero, then **all capabilities are cleared from the effective set**"* —
unconditionally. `PR_SET_KEEPCAPS` preserves only **permitted**. So even with `KEEPCAPS` set,
`CAP_SETPCAP` is out of E after the uid change. **A `capset()` restoring E from P must sit between
them.** D5 §7.5's ambient sequence includes that `capset()`; §7.7's listing and §20's C9–C13 do not
(V6 §B.4). D5 §20's C9 guard `[only if uid change + caps wanted]` makes it worse: with no caps wanted,
`KEEPCAPS` is skipped, `setuid` clears `CAP_SETPCAP`, and every drop fails.

**The corrected order — step 37 in full:**
```
1. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)   // CAP_SETPCAP still in E. Irreversible.
2. capset(): set P, E, I to the wanted sets
3. prctl(PR_SET_KEEPCAPS, 1)                            // only if a uid change follows
4. setgroups(list) ; setgid(gid) ; setuid(uid)          // RAW syscalls — §2.4
5. capset(): restore E from P                           // E was cleared unconditionally by (4)
6. for cap in ambient: prctl(PR_CAP_AMBIENT_RAISE, cap) // requires the cap in BOTH P and I
7. prctl(PR_CLR_KEEPCAPS)
```

Each ordering here is **kernel-forced**, verified against `capabilities(7)` and `prctl(2)` (V6 §B.4),
and the whole sequence matches runc's `finalizeNamespace()`: `ApplyBoundingSet()` → `SetKeepCaps()` →
`setupUser()` → `ClearKeepCaps()` → `ApplyCaps()` (CONFIRMED at v1.5.1, V1 §C6 rule 10).

`[F]` HIGH — **`setgroups` before `setgid`**, and `setgroups(0, NULL)` must be called **even when the
group list is empty**: omitting it leaves the container process in the **host's** supplementary groups
(D5 §7.7).

`[F]` HIGH — **the bounding set is the security boundary; P/E are convenience** (D5 §7.2, CONFIRMED
V6 §B.1). `[F]` HIGH — **root containers get their whole bounding set back on every `execve`**
(CONFIRMED with a precision fix, V6 §B.3), which is exactly why the bounding drop is the durable one.
`[O]` HIGH, **this session** — the live container's `CapBnd == CapEff == 0x00000000a80425fb`, the 14-cap
Docker default; independently decoded and re-measured by V6 §B.2.

**Step 36 — why seccomp has two positions, and which one Docker actually uses.**

`[F]` HIGH — `seccomp(2)`: *"either the calling thread must have the `CAP_SYS_ADMIN` capability in its
user namespace, or the thread must already have the `no_new_privs` bit set … Otherwise …
**`EACCES`**"* (CONFIRMED verbatim, V1 §C6 rule 9). So:
- **branch A** (`noNewPrivileges` false): the filter needs `CAP_SYS_ADMIN`, so it must be installed
  **before** step 37 drops it;
- **branch B** (`noNewPrivileges` true): no capability needed, so it goes **last**, and the profile can
  be minimal.

`[C]` HIGH — runc has **two** seccomp call sites, guarded `!l.config.NoNewPrivileges` and
`l.config.NoNewPrivileges` (CONFIRMED at v1.5.1, V1 §C6 rule 11). Two branches, not one order — which
is D1 §10's correction #5 to the brief, and it holds.

`[O]` HIGH, **this session** — the live container reports `NoNewPrivs: 0`, `Seccomp: 2`
(`SECCOMP_MODE_FILTER`), `Seccomp_filters: 1`, `CapEff: a80425fb`. **So Docker's default takes branch
A**: the filter was installed while `CAP_SYS_ADMIN` was still effective, and the caps were dropped
afterwards. The MVP inherits whichever branch the `noNewPrivileges` default picks — and D5 §7.4
proposes turning NNP **on** by default, which moves us to branch B. V6 §J.1 adjudicates that deviation:
**adopt, with a corrected rationale — the benefit is REAL but OVERSTATED and the cost is INCOMPLETE.**

**Seccomp facts that change what we write:**
- `[F]` HIGH — **`SECCOMP_FILTER_FLAG_TSYNC`**: pass it. With `Threads == 1` it is a no-op, but without
  it in a multithreaded process the filter applies to **one thread only** — a security control that
  silently does not exist (D5 §8.6).
- `[F]` HIGH — **`execve` and `execveat` must be in the allowlist**, or the container's entrypoint dies
  before it starts with an `EPERM` from a syscall the user never made (D5 §8.6).
- `[F]` HIGH — **denials for probe-and-fallback syscalls must return `ENOSYS`, not `EPERM`.** glibc
  ≥ 2.34 tries `clone3` first and falls back to `clone` **only on `ENOSYS`**; `EPERM` propagates and
  breaks `fork`/`pthread_create`. Docker's profile encodes exactly this
  (`{"names":["clone3"],"action":"SCMP_ACT_ERRNO","errnoRet":38}`). The known set as of glibc 2.4x is
  `clone3`, `faccessat2`, `close_range`, `openat2`, `statx`, `getrandom`, `membarrier`, `rseq`.
  **A blanket `EPERM` for these is a live incident generator** (D5 §8.5, CONFIRMED verbatim V6 §A.3).
- `[F]` HIGH — **`ptrace`, `process_vm_readv` and `process_vm_writev` are ALLOWED by Docker's default
  profile**, not denied. D5 §8.5 lists them as denied-by-capability; **REFUTED** (V6 §A.2): the profile
  has `{"names":["process_vm_readv","process_vm_writev","ptrace"],"action":"SCMP_ACT_ALLOW",
  "includes":{"minKernel":"4.8"}}`, resolved against the **host** kernel, and under seccomp's
  most-severe-result-wins the union of ALLOW rules governs. Consequence: a container process **can**
  `ptrace` another process of the same uid inside the same container, defeating in-container privilege
  separation. `[R]` MED — V6 recommends **our** profile deny them by default and re-allow under
  `--cap-add SYS_PTRACE`; we are not carrying moby's compatibility debt.
- `[R]` HIGH — **do not use `SECCOMP_RET_USER_NOTIF` in the MVP.** The notification carries the
  syscall's *raw arguments*, so a supervisor that dereferences a pointer via `/proc/<pid>/mem` has
  exactly the TOCTOU the design exists to prevent; `SECCOMP_IOCTL_NOTIF_ID_VALID` is not optional
  (D5 §8.7).

### 3.15 Steps 38–42 — NNP, fd hygiene, the rendezvous, `execve` (`C`)

**Step 38 — `PR_SET_NO_NEW_PRIVS`.** Needs no privilege; irreversible; must precede branch B.

**Step 39 — `close_range(3, ~0U, 0)`.** `[F]` HIGH — a leaked host fd into the container is a
documented escape class (CVE-2024-21626). `[R]` HIGH — V1 §F3 flags that D1 lists this only in §11.2 as
an `[R]` and **not in the §9 classification table at all**, and argues it belongs at least at hardening
tier and arguably **required** — *"it is one syscall"*. This trace tiers it **MVP-required**.

`[F]` HIGH — the ordering is ours, not the kernel's, and it is tight on both sides: earlier and we
close fds we still need; later is impossible (D5 §20 rule 6). Note the interaction with step 40: the
`exec.fifo` fd must survive this, so either it is opened after the sweep or excluded from it.
`[C]` HIGH — runc uses `CLOSE_RANGE_CLOEXEC` semantics and closes the `O_PATH` fifo fd **explicitly
before `execve`**, with the comment *"the kernel resets dumpable in the wrong order"* — a CVE-2016-9962
regression guard (D2 §3.2, CONFIRMED V2 §3.2).

**Step 40 — the rendezvous.** See §4.

**Step 42 — `execve`.** `[F]` HIGH — the failure worth naming because its errno lies: **`ENOENT` from
`execve` most often means the binary's *interpreter/loader* is missing from the image**, not the binary
itself. A statically-linked `alpine` `/bin/sh` sidesteps it; a glibc image with no `ld-linux-x86-64.so.2`
does not. Also `EACCES` (not executable, or `noexec` on the mount) and `ENOEXEC`.

`[F]` HIGH — `[R]` HIGH — **the memfd self-seal is mandatory, not optional.** D2 §7.3 claims `[R]` HIGH
that designing so we *"never re-exec into the container at all … eliminates the entire CVE class rather
than mitigating it"*. **REFUTED** (V2 §7.3), twice over:
1. **The mechanism does not require a runtime re-exec.** CVE-2019-5736 works because the runtime's
   `/proc/self/exe` is the host binary and the *container's entrypoint* can be a symlink to
   `/proc/self/exe`. If the user program resolves to `/proc/self/exe`, `C` becomes `mydocker`-the-binary
   running as the container process; anything inside then opens `/proc/<pid>/exe` and writes to it.
   **The exposure is created by `execve` of an attacker-controlled path, which every runtime does by
   definition.**
2. **All three shipping runtimes disagree in code.** crun has *exactly* the architecture D2 §7.3
   recommends and still ships `__attribute__((constructor)) libcrun_rexec` +
   `cloned_binary.c` (13,929 bytes) — which also **REFUTES** D2 §10.1's *"crun never re-execs itself …
   no memfd-clone, no C constructor trick"* (V2 §10.1; the grep was scoped to `container.c`, the code is
   in `linux.c`). youki calls `pentacle::ensure_sealed()` as the **first statement of `main()`** — which
   **REFUTES** D2 §11.8's *"youki appears not to have this"* (V2 §11.8). runc's is in
   `libcontainer/exeseal/`.

> `[R]` HIGH — seal the binary at the top of `main()`, in **every** binary that can reach an `execve`
> into a container. Do it early; retrofitting means auditing every exec path.

---

## 4. Synchronisation — three channels, and what deadlocks

There are **three** parent/child channels in this trace, and conflating them is how the deadlocks
happen.

| Channel | Direction | Lifetime | Purpose |
|---|---|---|---|
| **sync socket** — `socketpair(AF_UNIX, SOCK_SEQPACKET)` | both | create only | staged handshake: `ready` ↔ `run`, map requests, the child's structured error |
| **error pipe** | `C` → `S` | create only | a fixed-layout failure record when the child cannot continue |
| **`exec.fifo`** | `C` → `S` | create **to** start | the `create`/`start` rendezvous — the only one that spans two CLI invocations |

### 4.1 Why the create/start split exists at all

`[F]` HIGH — **it is mandated.** `create` *"MUST apply all config.json properties except `process`"*;
`start` *"MUST run the user-specified program"*. A conforming runtime cannot fuse them (D2 §3.1).

`[F]` HIGH — the *purpose* of the gap is the **hook window**: `prestart`, `createRuntime` and
`createContainer` all fire between "namespaces exist" and "user process runs". Without the gap there is
no moment at which an external tool can put a veth into the container's netns *knowing the netns exists*
and *knowing the user process has not started*. **That is the whole reason CNI works.**

`[C]` HIGH — the second, non-spec reason is worth as much: it separates *"the sandbox could not be
built"* from *"the user program failed"*. `create` non-zero vs `start` succeeding and the container
exiting 127 are different UX paths, and higher layers depend on the distinction (D2 §3.1).

### 4.2 The FIFO rendezvous — the mechanism, and why it is the right one

`[O]` HIGH, host 6.11.0-29 (D2 §3.2, CONFIRMED V2 §3.2): a thread calling
`os.open("/tmp/f2", O_WRONLY)` was still blocked in `open()` at t=1.000 s; a blocking `O_RDONLY` open at
t=1.000 s returned immediately, and **both** completed at t=1.001 s.

> `[F]` HIGH — **`open(O_WRONLY)` on a FIFO blocks until a reader opens. That is the whole
> synchronisation primitive.** The container's init is parked in `open()`, not in `read()` or `wait()`.
> It holds no lock, consumes no CPU, and needs no message protocol.

`[R]` HIGH — **adopt it verbatim.** Ten lines, kernel-provided, survives the create-side process
exiting, works across a userns boundary, costs one inode. The alternatives are worse: a socketpair dies
when the create-side process exits; `SIGSTOP`/`SIGCONT` is racy and visible to the container; an
eventfd cannot be reopened by path.

`[R]` HIGH — **in Rust, open the read side blocking** (`O_RDONLY`, no `O_NONBLOCK`) and `read()`
blocking. Go's `os.File` gives a *logically* blocking `Read` on a `O_NONBLOCK` FIFO via the netpoller;
Rust does not. `[O]` MED — D2's first attempt with `O_RDONLY|O_NONBLOCK` returned `EAGAIN` because the
write raced.

**A note on the deciding argument, because the corpus over-claimed it.** D2 §10.3 picks the FIFO over
youki's `notify.sock` on the grounds that *"the `sun_path` limit is a real, path-length-dependent trap
that we would hit with 64-hex ids"*. **REFUTED** (V2 §10.3) — arithmetic:
`/var/lib/mydocker/containers/<64 hex>/attach.sock` is **105 bytes + NUL = 106**, against a 108-byte
`sun_path`. It fits. And youki demonstrates a two-line workaround (`chdir(parent)` → `bind("notify.sock")`
→ `chdir(back)`) that removes path length as a consideration at any depth. `[R]` MED — **keep the FIFO**,
for the reasons D2 listed *second* and which survive: mode `0622` works across a userns with no socket
permission subtleties, `start` needs only `open`+`read`, and no fd has to survive the create/start gap.
`[R]` — and keep the socket path short anyway: 2 bytes of headroom is genuinely thin.

### 4.3 The staged handshake, and the four deadlocks

The parent's loop (runc's `parseSync`, D2 §4, CONFIRMED V1 §I1 with the step-8/9 transposition fixed):

```
S: clone3 -> C
S: close the child ends of every pipe          <-- deadlock 1 if skipped
S: [record pidfd] [cgroup fallback write] [netns plumbing]
S: loop reading the sync socket:
     mapRequest  -> write uid_map/gid_map, reply           (userns only)
     hooks       -> run prestart + createRuntime here
     ready       -> write state.json, reply "run"          <-- deadlock 2 if never sent
S: shutdown(SHUT_WR); assert we saw "ready"                <-- deadlock 3 if not asserted
--- create returns; the child is parked in open(exec.fifo, O_WRONLY) ---
S: (on `start`) open(exec.fifo, O_RDONLY); read 1 byte
C: open() returns; write 1 byte; close the O_PATH fd; execve
```

**The four ways to hang, and each one's fix:**

1. `[F]` HIGH — **the parent does not close the child ends of the pipes.** Every `read` that should see
   EOF when the child dies blocks forever instead, because the parent itself still holds a writer.
   runc's `comm.closeChild()` is step 2 for this reason. **Fix:** close them immediately after `clone3`,
   before anything else.
2. `[F]` HIGH — **the child dies before sending `ready`.** The parent blocks in `read` on the sync
   socket. **Fix:** the child's error pipe must be `CLOEXEC` and *not* held by the parent, so its EOF
   is a reliable death signal; and the parent must select over both the sync socket and the pidfd.
3. `[C]` HIGH — **the parent does not assert it saw `ready`.** A child that exits cleanly without
   signalling leaves the parent believing the container was created. runc's explicit
   `assert seenProcReady` exists for this.
4. `[F]` HIGH — **`start` is never called.** The child sits in `open(exec.fifo, O_WRONLY)` **forever**.
   This is by design, and it is exactly what makes `created` a real state — but it means an orphaned
   `create` leaks a live process, a cgroup and a netns until something reaps it. `[R]` HIGH — the
   `delete --force` path must survive it (D2 §9.4, CONFIRMED V2 §9.4).

`[F]` HIGH — the child's error channel is a **fixed-layout struct written with a single `write()`**:
crun uses `struct sync_socket_message_s { int type; int error_value; char message[512]; }`, D2 proposes
`#[repr(C)] { kind: u32, errno: i32, msg: [u8; 512] }` — the same shape, CONFIRMED and independently
corroborated (V2 §9.3). It has to be fixed-layout because §2.4 forbids formatting machinery in the
child.

`[R]` HIGH — **ban `tracing` (and all logging macros) in the child**; this is convergent practice, not
taste: runc uses `_LIBCONTAINER_LOGPIPE`, crun redirects its whole log handler into the sync socket
(`crun_set_output_handler(log_write_to_sync_socket, …)`), conmon always creates a stderr pipe
*"because that way we can capture runc stderr messages before the tty is created"* (CONFIRMED V2 §8.2).

### 4.4 Ordering of steps 40 and 41

`[C]` HIGH — the FIFO write comes **before** the branch-B seccomp install, so that the filter can be
minimal: everything after it runs under the profile, and `open`/`write`/`close` on the fifo would
otherwise have to be permitted. This is D1 §10.1's order (steps 31 → 32 → 33) and V1 raised no
objection. Tagged `[C]`, not `[F]`: the reverse order also works, it just costs three more allowed
syscalls.

---

## 5. What this looks like rootless

**Not MVP.** The project is fixed root-first (D5 §17). This subsection exists because the *shape* of the
rootless path constrains decisions made now, and V6/D5 agree on which retrofits are expensive.

### 5.1 Where the sequence diverges

| Step | Rootful | Rootless | Loud or quiet? |
|---|---|---|---|
| 5 overlay | `mount(2)` as root | kernel ≥ 5.11 + `userxattr`; else `fuse-overlayfs` | **loud, if we check** |
| 5 overlay upper | anywhere | **never on NFS** — overlayfs refuses NFS as upper | loud |
| 7 cgroup | `mkdir` under `/sys/fs/cgroup` | the path is **found, not chosen** — read `/proc/self/cgroup`, use that subtree | `EACCES` if there is no systemd user session |
| 7 controllers | all | often only `memory`+`pids` — `cpu` is frequently not delegated to `user@.service` | **quiet** unless we probe |
| 12 clone | `CAP_SYS_ADMIN` | **userns first**: it grants `CAP_SYS_ADMIN` *inside*, which is what the other flags need | — |
| 12a–14 | skipped | **required** — `I` + the map writes | — |
| 16 netns | veth + bridge | **impossible** — needs `CAP_NET_ADMIN` in the **host** netns. Userspace TCP/IP (`slirp4netns`, `pasta`) | loud |
| 26 `mknod` | works | **always `EPERM`**, even with a full effective cap set in the userns. Bind-mount device nodes instead | **quiet — the worst one** |
| 36/41 seccomp | needs `CAP_SYS_ADMIN` or NNP | **works** — the `no_new_privs` path needs no capability | — |
| 37 capabilities | real | work **inside** the userns, useless outside | **quiet, and the most misunderstood** |

`[F]` HIGH, all rows from D5 §16.1. `[F]` HIGH — the quiet rows are the dangerous ones: *"`CapEff`
shows 41 caps and `mknod` still `EPERM`"*.

`[F]` HIGH — the ordering rule that changes: **`unshare(CLONE_NEWUSER)` must come first and alone.**
`user_namespaces(7)`, verbatim: *"Starting in Linux 3.8, unprivileged processes can create user
namespaces, and the other types of namespaces can be created with just the `CAP_SYS_ADMIN` capability in
the caller's user namespace."* (CONFIRMED V1 §C6 rule 1.) `[C]` HIGH — and it must be a *separate*
`unshare` from the rest, which V1 §C6 rule 2 confirms is correctly tagged `[C]` (runc's stated reason;
the underlying SELinux/mqueue bug was not verified). `[F]` HIGH — it must also be a separate
`unshare` because **`CLONE_NEWUSER` cannot be combined with `CLONE_PARENT`** in a `clone` (V1 §C8), and
the three-stage topology needs `CLONE_PARENT`.

`[F]` HIGH — `unshare(CLONE_NEWUSER)` from a chroot returns `EPERM` since Linux 3.9. `[F]` HIGH —
Ubuntu 23.10+ with `apparmor_restrict_unprivileged_userns=1` **strips capabilities inside the new
userns**; it looks like a kernel bug and is LSM policy (D5 §16.1).

### 5.2 The design rule to adopt now

`[R]` HIGH — **every rootless degradation must be detected at container-create time and reported once,
by name, before anything is created** (D5 §16.1). The alternative is discovering it at `chown` on file
4,000 of a layer, or at `bind()` in the entrypoint — errors that are about the wrong subsystem.

`[R]` HIGH — the probe is the *same* "where am I and what may I write" probe the rootful path already
needs for cgroups (D5 §2.6, §16.2). **Write it once, use it in both modes.** Concretely the cheapest
rootless-readiness step available, and it costs the MVP nothing.

---

## 6. The failure-unwind order

`[F]` HIGH — failure atomicity is **spec-mandated**, not merely good engineering. The OCI runtime spec:
*"generating an error MUST leave the state of the environment as if the operation were never
attempted."* D2 §9 argues the whole case on engineering grounds and never invokes this; V2 §9.1 says it
should, *"it changes §9 from good practice to conformance."*

### 6.1 The order, and the two places it is not simply "reverse"

```
 0. read the cgroup's OOM counter                      <-- BEFORE anything is killed
 1. terminate(): SIGKILL the child (or cgroup.kill), then wait/reap
 2. destroy the cgroup (rmdir)                          <-- ONLY possible after 1
 3. child-side mounts                                   <-- free: the mount ns dies with the child
 4. host-side mounts (the overlay, if mounted on the host per §3.5)
 5. delete every network rule / conntrack entry naming the container's IP
 6. flush conntrack for that address
 7. RELEASE THE IP                                      <-- LAST. This is a security property.
 8. delete the state directory and the exec.fifo
```

**Correction 1 — `terminate()` before `Destroy()`.** D2 §4 states the failure path as
*"`Shutdown()` → `p.wait()` → deferred `manager.Destroy()` (remove cgroup dirs) → `intelRdtManager.Destroy()`
→ `p.terminate()` (SIGKILL + wait)"*. **CORRECTED, and copying it produces a bug** (V2 §4). runc's actual
deferred block, `process_linux.go:788–820`, in execution order, is `OOMKillCount()` → **`p.terminate()`**
→ `p.manager.Destroy()` → `intelRdtManager.Destroy()`, with runc's own comment:

> *"Terminate the process to ensure we can remove cgroups."*

`[F]` HIGH — **a cgroup directory with a live process in it cannot be `rmdir`ed.** D2 §9.1 independently
identifies this (*"An orphan cgroup dir cannot be removed while any process is in it"*), so §4's list
contradicts §9.1 and **would leak the cgroup**.

`[F]` HIGH — `rmdir` succeeding is not the same as the cgroup being freed; **dying cgroups** persist
(D5 §4.7, CONFIRMED and strengthened V6 §K.2). Do not treat a successful `rmdir` as proof the memory
was returned.

`[R]` HIGH — use **`cgroup.kill`** for step 1 where available: it is *"the only race-free container
stop"* (D5 §4.2, CONFIRMED V6 §E.1). `[F]` HIGH — **do not `SIGKILL` PID 1 as the escalation — kill the
cgroup** (D5 §4.4, CONFIRMED and endorsed V6 §E.5). `[F]` HIGH — walking `cgroup.procs` instead is racy
(V6 §E.4). And `[F]` HIGH — `exec` children are in the cgroup but **not in the process tree**
(V6 §E.3, *"subtler than stated"*), which is precisely why the cgroup is the right unit.

**Correction 2 — the IP must be released last, and this is not housekeeping.** D2 §9.1's severity table
says *"a leaked IP from the pool is a slow exhaustion; a leaked rule pointing at a dead IP is a latent
misroute."* **CORRECTED** (V2 §9, Adjudication 1) — that is true but records the *wrong* risk:

> `[F]` HIGH — the hazard is not the leak, it is the **inversion**. An IP **released while something
> still references it** — a DNAT/forward rule, a conntrack entry, an ARP/neighbour entry, a published
> port mapping — means the next container gets that address and **inherits the previous container's
> traffic**. That is a cross-tenant data path, i.e. a security bug, not a housekeeping one.
>
> **Release order is a security property.** V2 adjudicates in favour of the networking agent's framing:
> a severity table indexed by "which resource leaked" structurally cannot express a constraint about
> *relative ordering between two resources*.

`[R]` HIGH — annotate `Cleanup::ReleaseIp` as **order-critical**, and add a **conntrack flush for the
address between the rule removal and the release** — deleting an nftables rule does **not** tear down
flows already in conntrack (V2 §9). A strict reverse unwind of the cleanup list produces the right order
for free; what is missing is the *statement* that the adjacency is load-bearing, so nobody later
"optimises" the release to happen early — a very natural thing to do, since releasing first shortens
the window in which the pool is short.

### 6.2 Read the OOM counter before you kill

`[R]` HIGH — runc checks `manager.OOMKillCount()` **before** terminating, so it can annotate the error
`"container init was OOM-killed (memory limit too low?)"` instead of reporting a mystery. CONFIRMED
verbatim, including the string and the comment explaining that the count must be read first because
systemd may remove the cgroup (V2 §4). D2 calls this *"the best small idea in the section"* and V2
agrees. **Copy it.** "Container creation failed" with no reason is the single most common unhelpful
container error, and this turns it into a diagnosis for nearly free.

### 6.3 What the unwind does *not* have to do

`[F]` HIGH — **every mount made inside the container's mount namespace is torn down when the last member
exits** (`mount_namespaces(7)`). Steps 21–30 need **no unwind code at all**. `[R]` HIGH — this is the
argument for §3.5's child-side overlay: *"do as little as possible in the host mount namespace, and
everything else inside the new one"* (D2 §9.2; V2 §9.2 — *"the single best idea in §9"*).

### 6.4 Rollback lists must be idempotent

`[R]` MED — if a persisted rollback list is ever built, note the distinction V2 draws from netavark's
`src/firewall/state.rs` (354 lines, verified): netavark persists **desired state to be re-applied**, not
an undo log. A **replay** list is naturally idempotent — re-adding an existing nftables rule is
harmless. A **rollback** list is not: `ReleaseIp(addr)` executed twice hands the same address to two
containers, and `Umount(path)` twice can unmount someone else's mount at the same path. So a persisted
undo log needs per-entry completion marking, **or** every cleanup variant must be idempotent by
construction (release only if still owned by this container id; unmount only if the mount id matches).
`[R]` — D2's stated cheap alternative — *"on `rm`, best-effort try every possible cleanup and report what
failed"* — **is** the idempotent-by-construction version, and is the right phase-1 answer for that
reason rather than the "80% for 20%" reason D2 gives (V2 §9, Adjudication 2).

---

## 7. A structural gap this trace exposes: there is no reaper inside the container

D2 §5.4 claims `[R]` HIGH that `PR_SET_CHILD_SUBREAPER` on the supervisor gives us *"…and `mydocker
exec`'d processes whose intermediate parent died still get reaped instead of accumulating zombies in the
container."* **REFUTED** (V2 §5.4).

`[F]` HIGH — **the kernel forbids cross-namespace reparenting.** containerd documents it in the two
sentences immediately after the quote D2 §2.3 stops at (`docs/runtime-v2.md:583–588`):

> *"…prevents any cross-namespace reparenting in kernel. Assume that container is in X-namespace and P
> in root-namespace setns into X-namespace. P forks child C. The child C forks a grandchild G and exits.
> The G will be reparented to X instead of P's reaper. If the PID namespace is different from shim
> process, **the container init process should cleanup any orphaned reparented processes created by
> setns process (exec operation)**."*

`[O]` HIGH, host 6.11.0-29 (V2 §5.4, own short-lived processes only, nothing touched):

```
SUPERVISOR pid=3120122 prctl(PR_SET_CHILD_SUBREAPER,1)->0
G host pid = 3120124
G PPid after C exited: 3120122          <- container init DID reparent to the host subreaper
SUPERVISOR reaped = [(3120123,0), (3120127,0), (3120128,0), (3120124,0)]
G reaped         = [(2,0), (3,9), (5,11)]
```

Two results, both load-bearing:
1. **CONFIRMED** — the container init reparented to the host-side subreaper when its creator exited.
   That is exactly the shim ↔ `runc create` ↔ container-init relationship D2 §2.1 *infers*, now
   demonstrated directly.
2. **REFUTED** — the orphan created *inside* the pidns by the `setns`ed process (ns-local pid 5, status
   11) was reaped by **G, the container's PID 1** — not by the supervisor. So was the plain
   in-namespace orphan.

> `[F]` HIGH — **`PR_SET_CHILD_SUBREAPER` on our supervisor buys exactly nothing for zombies inside the
> container.** The container's own PID 1 is the only reaper that can ever get them.

**The design consequence, which the corpus does not draw.** A container whose PID 1 is `nginx`, `bash`,
or any application that does not reap **will accumulate zombies from `mydocker exec` orphans**, and no
supervisor-side flag can fix it. `/bin/sh` — the MVP's own entrypoint — does reap its own children, but
it is not a general answer.

`[R]` HIGH — Docker's answer is `--init`: a tiny reaping PID 1 (`docker-init 0.19.0` is installed on the
host and D2's own version block records it). **D2 never mentions `--init` or the problem it solves.**
It is ~30 lines, it is the only fix, and it belongs on the "cheap later" list — but it must be on a list,
because as of now **the design has no in-container reaper at all** (V2 §5.4).

`[F]` HIGH — related, and it changes the kill path: **`SIGTERM` to a handler-less PID 1 is silently
discarded.** A PID-namespace init only receives signals for which it has installed a handler; the kernel
suppresses the default action (D1 §7.1, CONFIRMED verbatim V1 §E1/§E2; D5 §4.1, CONFIRMED V6 §E.2).
`[F]` HIGH — the mechanism is `force_sig_info_to_task()`, **not** `sig_task_ignored()`'s `force` logic
as D1 §7.1 guessed — **CORRECTED**, and D1 §13 open question 4 is **SETTLED** (V1 §E2).

---

## 8. Where this sequence differs from the brief's proposal, and why

The brief proposed:

> CLI → resolve image → construct rootfs → create runtime state → clone/create process → namespaces →
> UID/GID configuration → cgroups → mount namespace → mount rootfs → `pivot_root` → /proc + required
> mounts → capabilities → seccomp/security → network namespace → environment → `execve` → container
> process

| # | The brief says | Corrected | Why |
|---|---|---|---|
| 1 | "clone/create process" **then** "namespaces" | **one step** | `[F]` **Namespaces are created *by* the clone**, as flags in the same syscall. There is no separate configure step (D1 §10) |
| 2 | "UID/GID configuration" after the clone, in the child | **a parent step**, and the reason `I` exists | `[F]` A process cannot write its own `uid_map`. It must land before `execve` or the child's caps are cleared (D5 §9.1, V1 §C7) |
| 3 | "cgroups" after the clone | **before** the clone — `CLONE_INTO_CGROUP` in the clone itself | `[F]` The child is unconstrained until placed, and can fork/spike/allocate in that window. Also, cgroup **placement** must precede the cgroup **namespace** (V1 §C6 rule 5, upgraded to `[F]`) |
| 4 | "network namespace" **after** seccomp | `CLONE_NEWNET` at clone time; **plumbing** is a parent step that must finish before the cap drop and seccomp | `[F]` A filter that forbids netlink, or a dropped `CAP_NET_ADMIN`, makes the plumbing impossible. **But** the constraint is only "before 36/37" — it need **not** precede the child's mounts, which D1 §3.5 rule 8 wrongly implied (V1 §I3) |
| 5 | "capabilities → seccomp" | **two branches, not one order** | `[F]` With NNP, seccomp goes last; without it, seccomp must precede the bounding drop because installing a filter still needs `CAP_SYS_ADMIN` (`EACCES` otherwise). `[O]` Docker's default is the **without-NNP** branch |
| 6 | "construct rootfs" as one step | split: pull (`X`, separate process) / extract (`X`) / overlay mount (`S` **or** `C`) | `[R]` The puller's HTTP stack spawns threads and would destroy `S`'s `Threads == 1` invariant (V7 §8.2) |
| 7 | (absent) | **assert `Threads == 1` before `clone3`** | `[F]` Raw `clone3` skips glibc's atfork handlers entirely. This is the only thing preventing an allocator deadlock (V2 §8.1) |
| 8 | (absent) | **pre-build argv/envp/paths/the seccomp `Vec`** | `[F]` Both `nix` and `rustix` re-allocate on long paths; `seccompiler` returns an owned `Vec` (V1 §H4, V7 §4.3) |
| 9 | (absent) | **`close_range` before `execve`** | `[F]` A leaked host fd is a documented escape class, CVE-2024-21626. One syscall (V1 §F3) |
| 10 | (absent) | **mount propagation on `/` before anything else in the child** | `[F]` Omit it and every container mount leaks to the host permanently, and `pivot_root` fails (D1 §4.2) |
| 11 | "environment" as a late step | **folded into step 11**, pre-clone | `envp` is an argument to `execve`; it cannot be "set up" in the child without allocating |
| 12 | (implied on the host) | **the pty is allocated in the container, after `pivot_root`** | `[F]` D2 §5.2/§7.2 has the arrangement runc **abandoned** (runc#814) — V2 §5.2 |

**Two tier corrections that change what "MVP" means** (V1 §F1): `CLONE_NEWPID` and a fresh `/proc` are
tiered **required** by D1 §9 and are **MISCLASSIFIED** against D1's own definition — `docker run
--pid=host` is a shipping mode, and D1's own §6.1 measurement shows `id`, `python3`, `openssl` and `df`
all working in a pivoted rootfs with **no** `/proc`. They are "strongly recommended / required for
isolation", which is a different engineering claim.

---

## 9. What is genuinely unsettled

Stated as unsettled rather than silently decided, per the corpus convention.

1. **`MS_REC|MS_SLAVE` vs `MS_REC|MS_PRIVATE` for `/`** (D1 open question 8; V1 §J item 3). runc and
   youki ship slave; **crun ships private**; `pivot_root(2)`'s own EXAMPLES program uses private. V1's
   only contribution is one more data point for private. D1 §9.3 promoted slave to "required", which it
   is not. **This trace picks slave `[R]` MED and records the tradeoff.**
2. **`CLONE_INTO_CGROUP` + `CLONE_NEWCGROUP` in one `clone3`** — whether placement is resolved before
   the namespace root is captured. Not addressed anywhere in D1–D5 or V1–V7. §3.10 gives the experiment
   and the safe alternative. **Cannot be settled from the production host** (`/proc/<pid>/cgroup` is
   rendered relative to the reader's cgroup namespace).
3. **`CLONE_INTO_CGROUP` end to end** (D1 open question 1). Needs a cgroup created on a non-production
   host. D1 declined; V1 declined; so does this document.
4. **Host-side vs child-side overlay mount** (§3.5). D3 assumes host-side; D2 §9.2 argues for
   child-side and V2 endorses that argument as the best idea in §9. Both defensible; the corpus never
   reconciles them.
5. **runc's devicemapper rationale for slave over private.** Not present in v1.5.1's `prepareRoot`.
   `git log -S devicemapper libcontainer/rootfs_linux.go` would settle it (V1 §J item 4).
6. **Time namespace on RHEL 8 / kernel 4.18 backports** (D1 open question 7) — needs `old-host`, which
   is **NXDOMAIN**. Every ROCKY-dependent claim in D1/D5 remains **UNSUPPORTED — pending host**.
7. **`--init` / the in-container reaper** (§7). The problem is now established; the design decision —
   ship a reaper PID 1, require the user's entrypoint to reap, or accept zombies from `exec` — is not
   made anywhere in the corpus.

---

## 10. The one-paragraph summary

`mydocker run alpine /bin/sh` is not a linear program. It is a **five-context** program in which the
host phase builds everything the container will need *before* it exists (because the child may not
allocate, and because the parent is the only one who can write a `uid_map` or place a process in a
cgroup atomically), a single `clone3` creates all six namespaces at once, the parent and child then run
**concurrently** against a staged handshake, and the child performs — in a strictly kernel-forced order —
propagation, mounts, `pivot_root`, the terminal, capabilities, seccomp and `execve`, with the container
parked in a FIFO `open()` in between so that `start` is a separate, observable event. The two places
where getting the order wrong fails **silently** are the capability sequence (`setuid` before
`PR_CAPBSET_DROP` leaves the full 41-cap bounding set) and the overlay option string (over 4095 bytes is
truncated and mounted with layers missing). Both are one line to get right and neither reports an error.
