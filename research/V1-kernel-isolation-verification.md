# V1 — Adversarial verification of D1 (Kernel isolation) + D2 §4 ordering

Verifier: independent agent. Source under review: `D1-kernel-isolation.md` (1960 lines, 216 tagged
claims) and `D2-runtime-architecture.md` §4 (creation-sequence ordering constraints).
This is a **separate verdict document**; no source file was edited.

Method: primary sources only — man-pages 6.8-2 text read on HOST-A (`host-a`,
`HOST-A`, Ubuntu 24.10, `6.11.0-29-generic`), UAPI headers on the same host, kernel
source read upstream, upstream crate docs, and fresh read-only/sandboxed observations on HOST-A.
`old-host` (ROCKY) is **unreachable** (`old-host.example.edu` → NXDOMAIN); every claim that depends on it is
marked **UNSUPPORTED — pending host** with the command that would settle it.

Verdicts: CONFIRMED / CORRECTED / REFUTED / OUTDATED / UNSUPPORTED / MISCLASSIFIED.

---

## Running log

## A. Headline verdicts (read this first)

| # | D1 §  | Claim | Verdict |
|---|---|---|---|
| A1 | §3.1 | `setns(2)` needs a fork for **TIME** as well as PID | **REFUTED** |
| A2 | §3.1 | "each call to `setns()` changes just one of the caller's namespace memberships" | **OUTDATED** (Linux 5.8 pidfd form) |
| A3 | §5.2 | "All **five** preconditions, measured … every documented precondition reproduces exactly" | **REFUTED** (six restrictions; four tested; three confounded) |
| A4 | §5.2 | C5 `new_root == "/"` → `EINVAL` | **REFUTED** (it is `EBUSY`) |
| A5 | §2 | Version table sourced to "`namespaces(7)` version column" | **REFUTED** (no such column in man-pages 6.8-2) |
| A6 | §1.2 | `unshare(CLONE_NEWPID)`: "the caller's `pid_for_children` **changes**" | **CORRECTED** (it goes *empty*) |
| A7 | §7.1 | Forced-signal mechanism = `sig_task_ignored()`'s `force` logic | **CORRECTED** (it is `force_sig_info_to_task()`) — Open Q4 settled |
| A8 | §11.1 | "write ~60 lines of `capget`/`capset` ourselves" | **CORRECTED — obsolete.** rustix ships it (adjudication in §H) |
| A9 | §11.2 | The no-allocation rule set is sufficient | **CORRECTED** — insufficient; both `nix` and `rustix` re-allocate |
| A10 | §11.2 | "no allocation post-clone" as a derived `[F]` | **MISCLASSIFIED** (POSIX scopes it to *multithreaded* forks) |
| A11 | §9.3 | `MS_REC\|MS_SLAVE` on `/` = **required** | **MISCLASSIFIED** (the requirement is "not shared"; slave vs private is a choice) |
| A12 | §9.1/§9.4 | `/proc`, `CLONE_NEWPID`, `CLONE_NEWNET` = **required** | **MISCLASSIFIED** against D1's own tier definition |
| A13 | §4.5 | `CVE-2019-14271`-style symlink-TOCTOU | **CORRECTED** (wrong CVE) |
| A14 | §5.5 | The `chroot` escape "requires `CAP_SYS_CHROOT`" | **CORRECTED** (the fd half needs no capability) |
| A15 | §4.5 | `[C]` HIGH crun / `[F]` LOW on `OPEN_TREE_NAMESPACE` | **MISCLASSIFIED confidence** — both now settled (§G) |
| A16 | §3.4 | (unreported) `unshare(2)`'s own `CLONE_NEWPID` paragraph is **wrong**, and D1's data proves it | **new finding** |

Everything else load-bearing that I checked came back CONFIRMED, and several `[O]`/`MED` claims are now
**strictly stronger** than D1 left them (§F).

---

## B. §5 `pivot_root` — the section that does not survive contact

### B1. §5.1 — the verbatim quote — **CONFIRMED**

Read `man 2 pivot_root` on HOST-A (man-pages 6.8-2, page dated 2024-05-02). D1's block quote of the
DESCRIPTION and of all six restrictions is **verbatim accurate**, including the "does not change the
caller's current working directory … should be followed by a `chdir("/")`" clause. No paraphrase drift.

### B2. §5.2 heading and conclusion — **REFUTED**

Two defects.

**(a) "All five preconditions."** The man page lists **six** restrictions. D1's own block quote in §5.1
contains all six. §5.2 then tests four of them (C1 mount-point, C3 shared, C4 put_old-location,
C5 new_root=="/"), and never tests:
- "`new_root` and `put_old` must be **directories**" (errno `ENOTDIR`);
- "`new_root` and `put_old` must not be on the same mount as the current root" (errno `EBUSY`);
- "The current root directory must be a mount point" (errno `EINVAL`).

**(b) "Every documented precondition reproduces exactly" is not supported by the transcript.** C1, C4
and C5 were all executed **before** C6 (`make / rprivate`) — i.e. while the namespace's `/` was still
`shared:513`, the state that C3 had just shown causes an unconditional `EINVAL`. In the kernel's
`SYSCALL_DEFINE2(pivot_root, …)` the shared-propagation test
(`IS_MNT_SHARED(old_mnt) || IS_MNT_SHARED(ex_parent) || IS_MNT_SHARED(root_parent)`) is evaluated
**before** the `EBUSY` loop test and before the `path_mounted()` tests. So under a shared `/`, every
one of those calls fails at the same check and their `EINVAL`s carry no information about the
precondition each was supposed to demonstrate.

**Measured, HOST-A 6.11.0-29, `unshare -m` sandbox, scratch under `/tmp`, host mount table verified
unchanged (0 leaks) and scratch removed:** the same four probes run twice, once with `/` `MS_REC|MS_PRIVATE`
and once with `/` `MS_REC|MS_SHARED` (nested inside a protective private namespace):

```
                                    / private        / shared
T1 new_root not a mount point   ->  EINVAL           EINVAL
T2 new_root == "/"              ->  EBUSY   <----    EINVAL   <---- confound
T3 put_old not under new_root   ->  EINVAL           EINVAL
T4 put_old is a regular file    ->  ENOTDIR          ENOTDIR
T5 fully valid pivot_root       ->  0 (OK)           EINVAL   <---- C3, the only informative one
```

**T2 is the proof.** With `/` private the kernel returns **`EBUSY`** for `new_root == "/"`, exactly as
`pivot_root(2)` ERRORS documents ("`EBUSY` `new_root` or `put_old` is on the current root mount. (This
error covers the pathological case where `new_root` is `"/"`.)"). D1 reported `EINVAL` for C5 — which is
the signature of the shared-root check firing first, not of the precondition it claims to have measured.

**Verdicts.** §5.2 heading: **CORRECTED** (six, not five). §5.2 C5: **REFUTED**. §5.2 C1 and C4:
**UNSUPPORTED as run** — both happen to be correct (I confirmed them under a private `/` above), but the
transcript does not establish them. §5.2 C3 and C7: **CONFIRMED**.

### B3. §5.2 C7 "cwd after pivot_root" — **CONFIRMED, independently reproduced**

My T5 landed with `cwd = /put/root` and a new `/` listing of `['afile','put']` — the caller's cwd was not
moved and remained reachable under `put_old`, exactly as D1's `/oldroot/root` shows and exactly as
`pivot_root(2)` states. This is the single most useful observation in D1 §5 and it holds.

### B4. §10.1 step 17 — "5 documented `EINVAL` preconditions" — **CORRECTED**

`pivot_root(2)` ERRORS has **six** `EINVAL` entries (not a mount point; `put_old` not under `new_root`;
current root not a mount point; current root on rootfs; `new_root`/its parent `MS_SHARED`; `put_old`
`MS_SHARED`), **plus** `EBUSY`, `ENOTDIR` and `EPERM`. An error-handling path written to D1's table will
mis-report `EBUSY` and `ENOTDIR`. I hit `ENOTDIR` in T4 above on the first try.

### B5. §5.3 the runc-style sequence — **CONFIRMED verbatim**

Read runc **v1.5.1** `libcontainer/rootfs_linux.go` (the pinned version D1 cites; the tag exists).
`pivotRoot()` is exactly D1's seven steps: `open("/", O_DIRECTORY|O_RDONLY|O_PATH)` → `fchdir(rootfd)`
→ `pivot_root(".",".")` → `fchdir(oldroot)` → `mount("", ".", "", MS_SLAVE|MS_REC, "")` →
`unmount(".", MNT_DETACH)` → `chdir("/")`. `pivot_root(2)`'s own NOTES section carries the
`pivot_root(".",".")` idiom and explains the stacking, so D1's "legal and idiomatic" is primary-sourced,
not just runc-sourced.

### B6. §5.5 "`chroot` is escapable" — **CONFIRMED**, with one **CORRECTED** detail

`chroot(2)` says all of it, verbatim: "This call changes an ingredient in the pathname resolution process
and does nothing else… it is **not intended to be used for any kind of security purpose**… This call does
not change the current working directory, so that after the call '.' can be outside the tree rooted at
'/'. In particular, the superuser can escape from a "chroot jail" by doing: `mkdir foo; chroot foo; cd ..`
… This call does not close open file descriptors, and such file descriptors may allow access to files
outside the chroot tree."

**CORRECTED:** D1 says the escape "Requires `CAP_SYS_CHROOT`, which container root has". That is true only
of the *final* `chroot(".")` that re-roots the process. The load-bearing half — hold an `O_DIRECTORY` fd
across the `chroot`, `fchdir` it, then read the entire host filesystem by relative path — **requires no
capability at all**. This matters operationally: a reader could conclude that dropping `CAP_SYS_CHROOT`
from the bounding set mitigates the escape. It does not. Full host-filesystem read/write is already
available from the preserved fd; `chroot(".")` only makes it convenient.

### B7. §5.6 `MS_MOVE` + `chroot` / initramfs — **CONFIRMED**

`pivot_root(2)` NOTES: "The rootfs (initial ramfs) cannot be `pivot_root()`ed. The recommended method …
is to delete everything in rootfs, overmount rootfs with the new root … see `switch_root(8)`." And the
ERRORS list carries the matching "`EINVAL` The current root is on the rootfs (initial ramfs) mount".
D1's `[R]` HIGH "MVP: `pivot_root` only, do not implement the fallback" stands, and the reasoning
("the masking step is worse half-right than absent") is sound.

---

## C. §3 process-creation primitives, and the ordering constraints in §3.5 / §10

### C1. §3.1 table row for `setns(2)` + TIME — **REFUTED**

D1's table says `setns(2)` needs a fork "only for PID and TIME", and the following paragraph generalises:
"Two flags are special in the same way: `CLONE_NEWPID` and `CLONE_NEWTIME` via `unshare`/`setns` affect
*subsequently created children only*."

That is right for `unshare(2)` and **wrong for `setns(2)`**. `time_namespaces(7)`, in the very paragraph
D1 quotes from, says the opposite: "(A process can use a file descriptor opened on this symbolic link in
a call to `setns(2)` in order to **move into the namespace**.)"

**Measured, HOST-A 6.11.0-29:**
```
after unshare(CLONE_NEWTIME):
  ns/time             = time:[4026531834]     <- caller not moved (as D1 says)
  ns/time_for_children= time:[4026532570]
setns(open("/proc/self/ns/time_for_children"), CLONE_NEWTIME) -> rc=0
  ns/time             = time:[4026532570]     <- CALLER MOVED
  ns/time_for_children= time:[4026532570]
```
Kernel mechanism: `timens_install()` (`kernel/time/namespace.c`) assigns **both** `nsproxy->time_ns` and
`nsproxy->time_ns_for_children`. Only PID is children-only on the `setns` path — because, as
`setns(2)` says, changing a live process's own `getpid()` would break callers.

**Bonus gap for §3.6.** `timens_install()` also begins `if (!current_is_single_threaded()) return -EUSERS;`.
So `setns` into a time namespace is the **second** namespace operation with a single-threaded requirement,
and it reports **`EUSERS`**, not `EINVAL`. §3.4's "`CLONE_NEWUSER` is the only namespace flag that a
multithreaded process cannot unshare" is correct as written (it is scoped to `unshare`), but §3.6's
"Combinations the kernel rejects" is missing this row.

### C2. §3.1 "each call to `setns()` changes just one" — **OUTDATED (Linux 5.8)**

`setns(2)` scopes that sentence to the `/proc/pid/ns/` form. It then documents a second form:

> **fd is a PID file descriptor** — Since Linux 5.8, `fd` may refer to a PID file descriptor obtained from
> `pidfd_open(2)` or `clone(2)`. In this usage, `setns()` **atomically** moves the calling thread into one
> or more of the same namespaces as the thread referred to by `fd`. The `nstype` argument is a bit mask …
> `setns(fd, CLONE_NEWUSER | CLONE_NEWNET | CLONE_NEWUTS);`

This is not a footnote for us: `mydocker exec` joining a container's six namespaces is **one** `setns` on
the pidfd D1 already recommends keeping (§8.2), not six `open`+`setns` pairs — and it is atomic, which
removes a partially-joined failure state that the six-call version has and cannot recover from. D1's §3.1
table and §8.2 never mention the pidfd form of `setns`. **Recommend adding it.**

Also missing from §3.1 and relevant to `exec`: `setns(2)` requires **`CAP_SYS_CHROOT` *and* `CAP_SYS_ADMIN`**
to join a mount namespace, and refuses if the caller shares `CLONE_FS` state.

### C3. §3.3 "the spec's premise is wrong" — **CONFIRMED**

`clone(CLONE_NEWPID)` puts the child in the new namespace as PID 1; the extra fork is an `unshare`/`setns`
artefact. Primary source, `pid_namespaces(7)`: "The first process created in a new namespace (i.e., the
process created using `clone(2)` with the `CLONE_NEWPID` flag, **or the first child created by a process
after a call to `unshare(2)`** using the `CLONE_NEWPID` flag) has the PID 1". D1's `setns(2)` quote for the
"why" is verbatim accurate. The `ENOMEM`-after-init-death quote is verbatim accurate
("a subsequent `fork(2)` into this PID namespace fail with the error `ENOMEM`"). The 32-deep nesting cap
since 3.7 is verbatim accurate. The "only once per process" `EINVAL` is verbatim accurate.

### C4. §1.2 "the caller's `pid_for_children` **changes**" — **CORRECTED**

It does not change to point at the new namespace; it becomes **empty**. `pid_namespaces(7)`: "A process may
call `unshare(2)` with the `CLONE_NEWPID` flag only once. After it has performed this operation, its
`/proc/pid/ns/pid_for_children` symbolic link will be **empty until the first child is created** in the
namespace."

**Measured, HOST-A 6.11.0-29** — in-process `libc.unshare(CLONE_NEWPID)`, then immediately:
```
ns/pid              = pid:[4026531836]      (unchanged, as D1 says)
ns/pid_for_children -> FileNotFoundError [Errno 2]   <- link is EMPTY, readlink fails
```
D1's §3.3 transcript appears to contradict this only because the `readlink` there ran *as a forked child*,
which is precisely the event that populates the link. Implementation consequence: **do not probe
`pid_for_children` to confirm an `unshare(CLONE_NEWPID)` succeeded** — it will `ENOENT` on a healthy path.

### C5. §3.4 — the measurement is right and it **refutes `unshare(2)`'s own text** (unreported by D1)

`unshare(2)` states, in the `CLONE_NEWPID` paragraph: "**`CLONE_NEWPID` automatically implies `CLONE_THREAD`
as well.**" If true, `unshare(CLONE_NEWPID)` from a multithreaded process would fail — `check_unshare_flags()`
rejects `CLONE_THREAD` unless `thread_group_empty(current)`.

**Measured, HOST-A 6.11.0-29**, `Threads: 4` in `/proc/self/status`, in-process `libc.unshare()`:
```
CLONE_NEWPID   -> rc=0      CLONE_NEWUSER -> rc=-1 EINVAL
CLONE_NEWNS    -> rc=0      CLONE_NEWTIME -> rc=0      CLONE_NEWUTS -> rc=0
```
Reproduces D1's §3.4 table exactly. Kernel `ksys_unshare()` adds `CLONE_THREAD` for **`CLONE_NEWUSER` only**:
```c
if (unshare_flags & CLONE_NEWUSER)
        unshare_flags |= CLONE_THREAD | CLONE_FS;
```
So D1 collected the data that proves a *second* man-page defect, right next to the `time_namespaces(7)` one
it does report as a "high-value finding", and did not notice. **New finding; worth adding to §1.8's
documentation-contradiction list.** §3.4's own conclusion is **CONFIRMED**.

### C6. §3.5 ordering rules — rule by rule

| Rule | Verdict | Evidence |
|---|---|---|
| 1. userns first | **CONFIRMED** | `user_namespaces(7)` verbatim: "Starting in Linux 3.8, unprivileged processes can create user namespaces, and the other types of namespaces can be created with just the `CAP_SYS_ADMIN` capability in the caller's user namespace." |
| 2. unshare userns alone, then the rest | **CONFIRMED as `[C]`** (runc's stated reason; I did not verify the SELinux/mqueue bug itself). Correctly tagged `[C]`. |
| 3. `uid_map`/`gid_map` before `execve`; `setgroups`→`deny` before `gid_map` | **CONFIRMED** | `user_namespaces(7)` verbatim; and the capability-loss-at-exec derivation is sound (see C7). |
| 4. cgroup assignment before init can fork | **CONFIRMED** | Matches runc v1.5.1's in-tree comment, verified verbatim: *"Do this before syncing with child so that no children can escape the cgroup"*. |
| 5. cgroup **namespace** after cgroup **placement** | **CONFIRMED — and upgraded to `[F]`** | D1 cites only youki's comment. `cgroup_namespaces(7)` states it as kernel behaviour: "When a process creates a new cgroup namespace using `clone(2)` or `unshare(2)` with the `CLONE_NEWCGROUP` flag, **its current cgroups directories become the cgroup root directories of the new namespace**." This is a kernel fact, not a runtime convention. |
| 6. mount ns before any container mount; propagation change first | **CONFIRMED** | `mount_namespaces(7)`; see §D. |
| 7. all mounts before `put_old` detach | **CONFIRMED** | Follows necessarily; D1's Python-dies-mid-run observation is the right demonstration. |
| 8. netns "created early, plumbed from the parent, configured before caps drop" | **CORRECTED (citation)** | The ordering is right, the citation supports a different claim. runc v1.5.1's `setupNetwork`/`setupRoute` are in `standard_init_linux.go` — **child-side**, and they configure `lo`, not veth. The parent-side veth work is `createNetworkInterfaces()` in `process_linux.go`, which runs *concurrently* with the child's `prepareRootfs` and is gated only by the `procReady`/`procRun` rendezvous. So "netns plumbing must precede the child's mounts" is D1's own serialization, not runc's and not the kernel's. What *is* kernel-imposed is only: plumbing must finish before the child drops caps / installs seccomp. |
| 9. `PR_SET_NO_NEW_PRIVS` before `seccomp(SECCOMP_SET_MODE_FILTER)` unless holding `CAP_SYS_ADMIN` | **CONFIRMED verbatim** | `seccomp(2)`: "…either the calling thread must have the `CAP_SYS_ADMIN` capability in its user namespace, or the thread must already have the `no_new_privs` bit set… Otherwise, the `SECCOMP_SET_MODE_FILTER` operation fails and returns **`EACCES`** in errno." |
| 10. bounding set dropped before uid change; `PR_SET_KEEPCAPS` around it; remaining sets after | **CONFIRMED** | runc v1.5.1 `finalizeNamespace()`: `ApplyBoundingSet()` → `SetKeepCaps()` → `setupUser()` → `ClearKeepCaps()` → `ApplyCaps()`. Exactly D1's order. |
| 11. seccomp last when NNP | **CONFIRMED** | runc v1.5.1 `Init()` has **two** seccomp call sites, guarded `!l.config.NoNewPrivileges` (before `finalizeNamespace`) and `l.config.NoNewPrivileges` (after). Two branches, as §10 rule 5 says. |

### C7. §1.6 "capabilities from `CLONE_NEWUSER` do not survive `execve`" — **CONFIRMED (derivation sound)**

The `[O]` HOST-A half is a re-runnable one-liner and the `capabilities(7)` execve rules give the reason:
a non-setuid exec by a nonzero euid with an empty ambient set produces `P'(permitted) = F(inheritable) & P(inheritable)`
and `P'(effective) = 0`, so `CapEff` goes to zero. Load-bearing conclusion — "`uid_map` must be written by the
parent *before* the child execs" — stands. The ROCKY half of the pair is **UNSUPPORTED — pending host**;
settles it: `ssh old-host "python3 -c 'import ctypes;ctypes.CDLL(\"libc.so.6\").unshare(0x10000000)' ; grep -E \"^(Uid|CapEff)\" /proc/self/status"`.

### C8. §3.6 "combinations the kernel rejects" — **CONFIRMED, but incomplete in ways that bite**

Every listed row is correct. Missing rows, all from `clone(2)`/`setns(2)` on the same host:
- **`CLONE_NEWUSER` + `CLONE_PARENT` → rejected.** `clone(2)`: "This flag can't be specified in conjunction
  with `CLONE_THREAD` or **`CLONE_PARENT`**." This lands directly on §9.2's row
  "`CLONE_PARENT` | strongly recommended (with 3 stages)" — in the three-stage userns topology the userns
  must therefore come from a separate `unshare`, never from the clone flags. D1 gets the topology right in
  §3.3 for a different reason and never states the hard constraint.
- **`CLONE_PARENT` is forbidden to any namespace init.** `clone(2)`: "The `CLONE_PARENT` flag can't be used
  in clone calls by the global init process … **and init processes in other PID namespaces**." Matters if our
  engine ever runs as PID 1 (a container-in-container dev setup, or a minimal VM).
- **`CLONE_NEWNS` + `CLONE_FS` in the same `clone()` → not permitted** (`clone(2)`).
- **`unshare(CLONE_NEWUSER)` from a chroot → `EPERM`** since Linux 3.9 (`unshare(2)`).
- **`setns(CLONE_NEWTIME)` multithreaded → `EUSERS`** (C1 above).

### C9. §3.2 `clone` vs `clone3` — **CONFIRMED**

- `CSIGNAL = 0x000000ff` and `CLONE_INTO_CGROUP = 0x200000000ULL` verified in
  `/usr/include/linux/sched.h` on HOST-A. Flag space bits 8–31 are all assigned → "exhausted" is accurate.
- "with `CLONE_PARENT`, `clone3` returns `EINVAL` if `exit_signal` is set, whereas legacy `clone` silently
  ignores the signal bits" — **CONFIRMED** by `clone3_args_valid()`:
  `if ((kargs->flags & (CLONE_THREAD | CLONE_PARENT)) && kargs->exit_signal) return false;`. Legacy `clone`
  has no such gate. Good catch by D1; keep it.
- `CLONE_INTO_CGROUP` quote from `clone(2)` — **CONFIRMED verbatim**, cgroup-v2-only — **CONFIRMED**.
- The `EBADF`-not-`EINVAL` probe is a valid feature-detection inference. **CONFIRMED as an `[O]`.**

---

## D. §1, §2, §4, §6 — facts, versions and mount mechanics

### D1a. §2's version table — numbers **CONFIRMED**, sourcing **REFUTED**

D1 heads the table `[F]` HIGH — "`namespaces(7)` version column". **There is no version column in
`namespaces(7)`.** As shipped in `manpages 6.8-2` — the exact package D1 says it read on HOST-A — the table
is four columns: `Namespace | Flag | Page | Isolates`. The version data has to come from `clone(2)`'s and
`unshare(2)`'s per-flag headers, and from the individual `*_namespaces(7)` pages. Verdict on the sourcing
line: **REFUTED**. Verdict on the numbers themselves: **CONFIRMED**, spot-checked against `clone(2)` on the
same host:

| Row | D1 | `clone(2)` on HOST-A | |
|---|---|---|---|
| Mount `CLONE_NEWNS` | 2.4.19 | "(since Linux 2.4.19)" | ✓ |
| UTS `CLONE_NEWUTS` | 2.6.19 | "(since Linux 2.6.19)" | ✓ |
| IPC `CLONE_NEWIPC` | 2.6.19 | "(since Linux 2.6.19)" | ✓ |
| PID `CLONE_NEWPID` | 2.6.24 | "(since Linux 2.6.24)" | ✓ |
| Network `CLONE_NEWNET` | 2.6.24 / 2.6.29 | "(since Linux 2.6.24)… (The implementation of this flag was completed only by about Linux 2.6.29.)" | ✓ |
| User `CLONE_NEWUSER` | 2.6.23 / **3.8** | "first became meaningful for clone() in Linux 2.6.23, the current clone() semantics were merged in Linux 3.5, and the final pieces … merged in Linux 3.8" | ✓ |
| Cgroup `CLONE_NEWCGROUP` | 4.6 | "(since Linux 4.6)" | ✓ |
| Time `CLONE_NEWTIME` | 5.6 | **absent from `clone(2)`**; `unshare(2)` says "(since Linux 5.6)" | ✓ (and D1 correctly notes the absence) |

One extra worth recording that D1's table hides: **`unshare(2)` support for `CLONE_NEWPID` is "(since Linux
3.8)"**, six years after the namespace itself. Anyone reading "PID: since 2.6.24" and reaching for `unshare`
on an ancient kernel gets `EINVAL`.

All eight **flag hex values CONFIRMED** against `/usr/include/linux/sched.h` on HOST-A (`NEWNS 0x00020000`,
`NEWCGROUP 0x02000000`, `NEWUTS 0x04000000`, `NEWIPC 0x08000000`, `NEWUSER 0x10000000`, `NEWPID 0x20000000`,
`NEWNET 0x40000000`, `NEWTIME 0x00000080`).

`max_*_namespaces` = **63826 on all eight** — **CONFIRMED**, re-read.

### D1b. §2's `ENOSPC` / `EUSERS` note — **CORRECTED (scope)**

D1 writes it as if it applies to all eight per-type limits. `unshare(2)`'s `EUSERS` history is specific to the
**nested user-namespace** limit: "`ENOSPC` (since Linux 4.9; beforehand `EUSERS`) `CLONE_NEWUSER` was specified
in flags, and the call would cause the limit on the number of **nested user namespaces** to be exceeded …
From Linux 3.11 to Linux 4.8, the error diagnosed in this case was `EUSERS`." The per-type `/proc/sys/user`
count limits are a separate `ENOSPC` entry, "(since Linux 4.9)", with no `EUSERS` predecessor.

### D1c. §2.1 syscall numbers — **CONFIRMED**

All thirteen verified against `/usr/include/x86_64-linux-gnu/asm/unistd_64.h` on HOST-A: `pivot_root 155`,
`open_tree 428`, `move_mount 429`, `fsopen 430`, `fsconfig 431`, `fsmount 432`, `fspick 433`, `clone3 435`,
`close_range 436`, `openat2 437`, `mount_setattr 442`, `statmount 457`, `listmount 458`. Mainline-since
column matches `clone(2)`/`unshare(2)`/feature history. The ROCKY column is **UNSUPPORTED — pending host**;
settles it: `ssh old-host "python3 - <<'X'
import ctypes;l=ctypes.CDLL('libc.so.6',use_errno=True)
for n in (428,429,430,431,432,433,435,436,437,442,457,458):
    l.syscall(ctypes.c_long(n)); print(n, ctypes.get_errno())
X"` and reading which return 38 (`ENOSYS`).

`[R]` HIGH "runtime-detect every optional syscall … never gate on a parsed kernel version" — **CONFIRMED as
sound**, and independently reinforced: `OPEN_TREE_NAMESPACE` (§G) is exactly the case where a version parse
would mislead, and crun implements precisely this probe-don't-parse pattern.

### D2a. §1.8 time namespace via `clone3` — **CONFIRMED, independently reproduced, mechanism now named**

The man-page defect is real. `time_namespaces(7)` on HOST-A (man-pages 6.8-2) still reads: "Currently, the
only way to create a time namespace is by calling `unshare(2)` with the `CLONE_NEWTIME` flag."

**Measured, HOST-A 6.11.0-29**, `clone3(cl_args.flags = CLONE_NEWTIME, size=88)`:
```
parent  ns/time              = time:[4026531834]
child   ns/time              = time:[4026532570]   <- new namespace, placed directly
child   ns/time_for_children = time:[4026532570]
```
**Kernel mechanism** (D1 did not supply one). `create_new_namespaces()` in `kernel/nsproxy.c` sets only
`time_ns_for_children` from `copy_time_ns()`, leaving `time_ns` at the parent's value — which is why `unshare`
does not move the caller. `copy_namespaces()` then runs, for a fork/clone:
```c
if ((flags & CLONE_VM) == 0)
        timens_on_fork(new_ns, tsk);
```
and `timens_on_fork()` (`kernel/time/namespace.c`) promotes `time_ns_for_children` into `time_ns` and calls
`timens_commit()`. **Caveat D1 missed and that a reader will trip on:** this is gated on `CLONE_VM` being
clear. A `clone3(CLONE_NEWTIME | CLONE_VM)` child stays in the *parent's* time namespace with only
`time_for_children` changed — the `unshare` behaviour. Worth stating explicitly given §11.1 spends a page on
the `CLONE_VM` hazard in `nix::sched::clone`.

The `CLONE_NEWTIME == 0x80` inside `CSIGNAL` explanation and the "legacy `clone` misreads it as
`exit_signal = 16`" observation are both **CONFIRMED** (header value verified; `CSIGNAL = 0xff` verified).

### D2b. §1.6 "`CAP_SYS_TIME` … useless outside the initial user namespace" — **CORRECTED**

The quoted `user_namespaces(7)` sentence is verbatim accurate, but it predates time namespaces and D1
repeats it without the exception. `time_namespaces(7)`: "In order to write to the `timens_offsets` file, a
process must have the **`CAP_SYS_TIME` capability in the user namespace that owns the time namespace**."
Since Linux 5.6, `CAP_SYS_TIME` is therefore *not* uniformly initial-userns-only. Low practical impact for
the MVP (which skips both namespaces), but the sentence is quoted in the D5 handoff as a rootless fact.
`CAP_SYS_MODULE` and `CAP_MKNOD` are unaffected — those two stand.

### D3. §1.7 cgroup namespace — **CONFIRMED verbatim**

`cgroup_namespaces(7)`: "When a process creates a new cgroup namespace … its current cgroups directories
become the cgroup root directories of the new namespace" and "When reading the cgroup memberships of a
'target' process from `/proc/pid/cgroup`, the pathname shown in the third field of each record will be
**relative to the reading process's root directory**". Both of D1's claims — including the non-obvious
reader-relative one, which is the one that would silently break our daemon's introspection — are exact.
The man page's own worked example (`sh2# cat /proc/1/cgroup` → `7:freezer:/..`) confirms D1's third
transcript line. Also **CONFIRMED**: the "path-hiding, controls nothing" characterisation.

### D4. §4.1 propagation quotes — **CONFIRMED verbatim**

`MS_SHARED`, `MS_PRIVATE`, `MS_SLAVE` all match `mount_namespaces(7)` word for word (D1's ellipsis in the
`MS_SHARED` quote elides only "Propagation here means that the same `mount(2)` or `umount(2)` will
automatically occur under all of the other mounts in the peer group", which does not change the meaning).
The two "subtleties that bite" — propagation applies to mounts *immediately under*, not grandchildren; and a
new mount namespace **inherits peer-group membership** — are both in the page and are the correct pair to
have pulled out. The man page's own `MS_SHARED and MS_PRIVATE example` reproduces D1's §4.2 leak from the
opposite direction (a submount under `/mntS` created in `sh2`'s namespace appears in `sh1`'s), which makes
D1 §4.2's measured leak **CONFIRMED by primary source as well as by observation**.

### D5. §4.2 runc `prepareRoot` — **CONFIRMED verbatim**, one **UNSUPPORTED** rider

The Go quoted in §4.2 is byte-accurate against runc **v1.5.1** `libcontainer/rootfs_linux.go`. So is the
description of `rootfsParentMountPrivate()`, which loops `mount("", path, "", MS_PRIVATE, "")` and walks
`filepath.Dir(path)` on `EINVAL` until `/`. Note the mechanism precisely, because §9.3's label is loose: this
makes **the nearest enclosing mount** of the rootfs private, which is what `pivot_root(2)`'s precondition
actually names ("the propagation type of the **parent mount of `new_root`** … must not be `MS_SHARED`"). The
subsequent self-bind (step 13) then makes that private mount the parent of the new `new_root` mount — so the
12→13 ordering is load-bearing and D1 has it in the right order.

**UNSUPPORTED:** "runc's source cites races with devicemapper where a fully private container held a stale
reference to a mount the host thought was gone." D1 tags this `[C]` MED and says it did not verify the bug
report. I also could not find that comment in v1.5.1's `prepareRoot`/`rootfs_linux.go`. Treat the
slave-vs-private *rationale* as unsourced; the *fact* that runc and youki default slave and crun defaults
private is confirmed (and is D1's own Open Question 8).

### D6. §4.3 `MS_BIND` / remount dance — **CONFIRMED**

`mount(2)` does state that bind mounts ignore the filesystem type and data, and that changing mount options
on a bind requires a second `MS_BIND|MS_REMOUNT` call; `mount_setattr(2)` with `AT_RECURSIVE` is the only
one-call recursive form. The recursive-read-only-bind bug D1 flags (nested submounts silently stay writable)
is the well-known one and the `[R]` MED ("acceptable only until we support `-v …:ro`") is correctly scoped.

### D7. §4.4 the Docker mount table — **CONFIRMED, re-measured on the same container**

Re-read `/proc/82271/mountinfo` (container `12d16acf9cc0` / `aikserver-redis-1`, still running):
```
/               overlay   rw,relatime                                    <- none of the three
/proc           proc      rw,nosuid,nodev,noexec,relatime
/dev            tmpfs     rw,nosuid                                       <- no nodev, no noexec
/dev/pts        devpts    rw,nosuid,noexec,relatime
/dev/shm        tmpfs     rw,nosuid,nodev,noexec,relatime
/dev/mqueue     mqueue    rw,nosuid,nodev,noexec,relatime
/sys            sysfs     ro,nosuid,nodev,noexec,relatime
/sys/fs/cgroup  cgroup2   ro,nosuid,nodev,noexec,relatime
```
All eight rows match D1 exactly, including both "instructive asymmetries". **CONFIRMED.**

### D8. §6.8 masking — **CONFIRMED, re-measured**

`/proc/{bus,fs,irq,sys,sysrq-trigger}` are `ro,nosuid,nodev,noexec` proc re-binds; `/proc/acpi` is a
`tmpfs ro,relatime`; `/proc/{kcore,keys,latency_stats,timer_list,interrupts}` are binds off the container's
own `/dev` tmpfs (they report fstype `tmpfs rw,nosuid`, which is the `/dev` superblock — consistent with a
`/dev/null` bind). Two masking techniques, exactly as D1 describes. **CONFIRMED.**

### D9. §6.1 / §6.5 / §6.6 — **CONFIRMED and PROMOTED from single-host `[O]` to `[F]`**

These three were `[O]` **ROCKY 4.18** only, and ROCKY is unreachable. I reproduced all three on HOST-A
6.11.0-29 as root inside `unshare -U -m --map-root-user --propagation private` (scratch `/tmp/v1m`, removed;
host mount table verified 0 leaks):

| fstype | `-U -m` | `-U -m -p -f` | `-U -m -n` | `-U -m -i` |
|---|---|---|---|---|
| `proc`   | EPERM | **OK** | EPERM | EPERM |
| `sysfs`  | EPERM | EPERM | **OK** | EPERM |
| `mqueue` | EPERM | EPERM | EPERM | **OK** |
| `devpts` | OK | OK | OK | OK |
| `tmpfs`  | OK | OK | OK | OK |

Identical to D1's ROCKY results on a kernel seven major versions newer. This is now a two-kernel,
two-distro result and the pattern is a documented kernel rule for procfs
(`user_namespaces(7)`: "Holding `CAP_SYS_ADMIN` within the user namespace that owns a process's PID namespace
allows (since Linux 3.8) that process to mount `/proc` filesystems") — so **procfs→pidns is `[F]`, not `[O]`.**

For **sysfs→netns**, D1 says it "is not obvious from `user_namespaces(7)`'s bullet list" and leaves it as a
surprising observation. It is a documented kernel rule too, just in the source rather than the man page:
`sysfs_init_fs_context()` calls `kobj_ns_current_may_mount(KOBJ_NS_TYPE_NET)` → `net_ns_current_may_mount()`
→ `ns_capable(current->nsproxy->net_ns->user_ns, CAP_SYS_ADMIN)`. You need `CAP_SYS_ADMIN` **in the user
namespace that owns your current net namespace**; a fresh userns sharing the host netns does not have it.
That makes D1's D4 handoff item a hard `[F]`, and it also means the rule is stated exactly backwards from
how one would guess: it is not "sysfs needs a netns", it is "sysfs needs your netns to be *yours*".

### D10. §6.2 — `/dev` and the `/dev/null` failure mode — **CONFIRMED**

`echo x > /dev/null` creating a regular file in an empty `/dev` is a correct and genuinely nasty observation,
and the OCI/runc default node list (`null 1,3`, `zero 1,5`, `full 1,7`, `random 1,8`, `urandom 1,9`,
`tty 5,0`) matches the spec's default devices. `CAP_MKNOD` initial-userns-only is verbatim from
`user_namespaces(7)`. The ROCKY `mknod → EPERM` observation is **UNSUPPORTED — pending host**; settles it:
`ssh old-host "unshare -U -m --map-root-user sh -c 'mknod /tmp/nul c 1 3; echo rc=\$?'"`.

### D11. §6.3 devpts `newinstance` — **CONFIRMED, and Open Question 3 SETTLED**

D1 tags the cause `[F]` MED and says "I did not read the commit; what would settle it is
`git log --oneline fs/devpts/inode.c` around v4.7." Done, against `git.kernel.org` with `h=v4.7`:

> **2016-06-05 — `devpts: Make each mount of devpts an independent filesystem.` — Eric W. Biederman —
> `fs/devpts/inode.c` (+55 / −136)**, reachable from tag **v4.7**.

That is the commit, it is in v4.7, and the option D1 names (`CONFIG_DEVPTS_MULTIPLE_INSTANCES`) went with it.
**Upgrade §6.3's `[F]` MED to `[F]` HIGH.** The observation that `newinstance` is accepted and silently
dropped from the resulting option string on 6.11 is therefore explained, and the note that runc still passes
it is a fair "convention outliving its cause".

---

## E. §7 — PID 1 semantics, checked line by line against `pid_namespaces(7)`

I read the whole of `man 7 pid_namespaces` on HOST-A and diffed it against §7. Result: **the quotations are
verbatim and the conclusions follow**, with one mechanism correction and two omissions.

### E1. §7.1 signal-disposition quote — **CONFIRMED verbatim**

Man page, exactly: "Only signals for which the 'init' process has established a signal handler can be sent
to the 'init' process by other members of the PID namespace. This restriction applies even to privileged
processes, and prevents other members of the PID namespace from accidentally killing the 'init' process." …
"Likewise, a process in an ancestor namespace can—subject to the usual permission checks described in
`kill(2)`—send signals to the 'init' process of a child PID namespace only if the 'init' process has
established a handler for that signal. **`SIGKILL` or `SIGSTOP` are treated exceptionally: these signals are
forcibly delivered when sent from an ancestor PID namespace.** Neither of these signals can be caught by the
'init' process."

D1's reading of the emphasis — that the `SIGKILL`/`SIGSTOP` exception is scoped to **ancestor** senders — is
correct and is the right thing to have pulled out. Measurements (a) and (b) are consistent with the text.
D1 also omits one sentence worth having: *"(Within the handler, the `siginfo_t` `si_pid` field described in
`sigaction(2)` will be zero.)"* — relevant the moment we ship the design-B init of §7.5, because a forwarding
init cannot identify the sender.

### E2. §7.1(c) forced vs generated signals — behaviour **CONFIRMED**, mechanism **CORRECTED** (Open Q4 settled)

D1 tags the mechanism `[F]` MED: "this matches `sig_task_ignored()`'s `SIGNAL_UNKILLABLE` + `force` logic in
`kernel/signal.c`, but I inferred that from behaviour rather than reading the source."

Read (`kernel/signal.c`, v6.11). **The inference is wrong in a way that matters.** `sig_task_ignored()` is:
```c
	if (unlikely(t->signal->flags & SIGNAL_UNKILLABLE) &&
	    handler == SIG_DFL && !(force && sig_kernel_only(sig)))
		return true;
```
The `force` escape hatch is **gated on `sig_kernel_only(sig)`**, i.e. `SIGKILL`/`SIGSTOP` only. A forced
`SIGSEGV` is *not* `sig_kernel_only`, so this clause would still ignore it — it cannot be the explanation for
D1's `rc=139`.

The real mechanism is one function earlier, in `force_sig_info_to_task()`:
```c
	if (action->sa.sa_handler == SIG_DFL &&
	    (!t->ptrace || (handler == HANDLER_EXIT)))
		t->signal->flags &= ~SIGNAL_UNKILLABLE;
```
A forced signal whose target has `SIG_DFL` **clears `SIGNAL_UNKILLABLE` outright**, so by the time
`sig_task_ignored()` runs the task is no longer unkillable at all. This **settles D1's Open Question 4**, and
it should be `[F]` HIGH, not `[F]` MED.

Two consequences D1 does not draw, both engine-relevant:
1. "Namespace init is unkillable" is **not a durable property** — one synchronous fault with a default
   disposition retracts it permanently for that process.
2. The protection is bypassed only for *forced* (synchronous/kernel-originated) signals. A container's own
   `kill(1, SIGSEGV)` stays discarded, which is exactly why D1's §7.4 note — that `unshare -pf sh -c
   'kill -SEGV $$'` exits **0**, not 139 — is correct and worth keeping in the CLI's exit-status handling.

### E3. §7.3 orphan reparenting — **CONFIRMED verbatim, with an elided qualifier**

The quote is exact. D1's gloss "Reparenting lands on the **namespace's** PID 1" drops the man page's own
follow-up, which is the interesting half: *"Note that because of the `setns(2)` and `unshare(2)` semantics
described above, this may be the 'init' process in the PID namespace that is the **parent** of the child's
PID namespace, rather than the 'init' process in the child's own PID namespace."* For the `clone3`-based
MVP topology D1 recommends, the simple reading holds; for the `unshare` topology (which D1 keeps as the
userns path, §10.1 step 8a) it does not, and orphans can reparent *outside* the container. Worth a sentence.
The `PR_SET_CHILD_SUBREAPER` exception is correctly noted.

### E4. §7.4 PID-1 exit, `ENOMEM`, exit-status propagation — **CONFIRMED**

- "If the 'init' process of a PID namespace terminates, the kernel terminates all of the processes in the
  namespace via a `SIGKILL` signal." — verbatim.
- "a subsequent `fork(2)` into this PID namespace fail with the error `ENOMEM`" — verbatim; D1's measured
  `errno=12` matches.
- Exit-status encoding (code in bits 8–15, signal in bits 0–6) — standard, and `status 768 >> 8 == 3` checks out.
- `[R]` HIGH "lean on PID-1-exit as a kernel-enforced teardown" — **CONFIRMED as sound**, and correctly
  hedged with "still do the `cgroup.kill` sweep for defence in depth".

### E5. §7.1 → §7.5 recommendations — **CONFIRMED as sound, correctly tagged `[R]`**

"`mydocker stop` must always come from the host namespace and must always have a `SIGKILL` fallback" and
"never rely on in-container self-termination" both follow directly from the quoted text. The §8.2 note that
`pidfd_send_signal` **returns 0 for a discarded signal** is the single most operationally useful line in §7–§8
and is correct: the return value means "accepted for delivery", never "delivered". Design A vs B vs C is a
`[R]`, honestly labelled, with the costs stated. No objection.

### E6. §7.2 measured PID-1 state — **CONFIRMED, re-measured byte for byte**

Re-read `/proc/82271/status` on the same live container:
```
Uid: 999  Gid: 1000  Groups: 1000
CapInh: 0 CapPrm: 0 CapEff: 0  CapBnd: 00000000a80425fb  CapAmb: 0
NoNewPrivs: 0   Seccomp: 2   Seccomp_filters: 1
```
Identical. `0xa80425fb` = the 14 Docker keeps. The inference — that `NoNewPrivs: 0` **plus** a loaded filter
means runc took the `!NoNewPrivileges` branch and installed seccomp while still holding `CAP_SYS_ADMIN` — is
sound and is confirmed by runc v1.5.1 having exactly those two guarded call sites (see C6 rule 11).

### E7. §0.1 "Docker's default is 6 of 8 namespaces" — **CONFIRMED, re-measured**

Same container, same host, today: `mnt/uts/ipc/pid/cgroup/net` all differ from `/proc/1/ns/*`; `user` and
`time` are byte-identical to host PID 1 (`user:[4026531837]`, `time:[4026531834]`). `lsns -t time -t user`
shows a single namespace of each holding all 164 processes on the box. **CONFIRMED.** The `[R]` that follows —
MVP creates the same six, and skipping `user` is a stated decision rather than an accident — is honest and
correctly tagged.

---

## F. §9 — attacking the classification table

D1 defines the tiers itself: **required** = "no container without it"; **strongly recommended** = "container
'works' but real software breaks"; **hardening**; **Docker convenience**. I hold the table to that definition.

### F1. The "required" row does not survive its own definition — **MISCLASSIFIED**

| Row | D1 tier | Verdict | Why |
|---|---|---|---|
| `CLONE_NEWNS` | required | **CONFIRMED required** | There is no `--mount=host` in any runtime. Without it `pivot_root` repoints the host and every mount leaks. Genuinely irreducible. |
| recursive self-bind of rootfs | required | **CONFIRMED required** | `pivot_root(2)`: "`new_root` must be a path to a mount point". Reproduced as T1→EINVAL above. |
| `pivot_root` | required | **CONFIRMED required** *for isolation* | `chroot` is escapable (B6). But note runc ships `--no-pivot` and D1's own §5.6 documents when it is necessary, so "no container without it" is false in the literal sense; "no *safe* container without it" is true. |
| `umount2(put_old, MNT_DETACH)` | required | **CONFIRMED required** | An accessible `put_old` is a one-line escape. |
| `chdir("/")` after `pivot_root` | required | **CONFIRMED required** | Kernel does not move the caller's cwd — reproduced (B3). |
| `CLONE_NEWPID` | **required** | **MISCLASSIFIED** | `docker run --pid=host` is a shipping, supported mode. A container without a PID namespace runs. Correct tier by D1's own definition: **strongly recommended / required for isolation**. |
| fresh `/proc` mount | **required** | **MISCLASSIFIED** | D1's own §6.1 measurement shows `id`, `python3`, `openssl`, `df` all work in a pivoted rootfs with **no** `/proc`. This is precisely "container works, real software breaks" — the strongly-recommended tier. Note D1 hedges `/dev/pts` as "required **in practice**" and `/proc` deserves the same wording, not a stronger one. |
| `CLONE_NEWNET` | **required for isolation** | **CONFIRMED as hedged** | The "for isolation" qualifier already does the work; `--network=host` is shipping. Fine as written. |
| `MS_REC\|MS_SLAVE` on `/` | **required** | **MISCLASSIFIED — see F2** | |
| non-recursive `MS_PRIVATE` on rootfs mount | **required** | **CORRECTED wording** | The kernel precondition is on the **parent mount of `new_root`** (D5 above), which is what runc's loop actually achieves. As written the row names the wrong mount. |
| `clone`/`clone3` with ns flags | required | **CONFIRMED** | |
| single-threaded-before-clone invariant | required | **CONFIRMED** — but only *because* the design takes the `unshare(CLONE_NEWUSER)` path (§10.1 step 8a). For the pure-`clone3` MVP with no userns, nothing in §3.4's measured table fails multithreaded. It is required for the D5 rootless track and it is cheap insurance for the MVP; it is not a kernel requirement of the MVP as specified. |
| intermediate 3rd process **iff** userns | required-iff | **CONFIRMED** | |
| signal from host (ancestor) ns | required | **CONFIRMED** | |
| `SIGKILL` fallback after grace timeout | required | **CONFIRMED** | |
| `setsid` + `TIOCSCTTY` **for `-it`** | required-for | **CONFIRMED** | Correctly scoped. |

### F2. `MS_REC|MS_SLAVE` on `/` as "required" — **MISCLASSIFIED**, and D1 knows it

What the kernel requires is only that **`/` (and `new_root`'s parent) not be `MS_SHARED`**. `MS_SLAVE`,
`MS_PRIVATE` and `MS_UNBINDABLE` all satisfy it. The evidence against "slave is required":
- **crun defaults to `MS_REC|MS_PRIVATE`** — D1 says so itself in §4.2 and §5.3.
- **`pivot_root(2)`'s own EXAMPLES program** uses `mount(NULL, "/", NULL, MS_REC | MS_PRIVATE, NULL)`, with
  the comment "Ensure that 'new_root' and its parent mount don't have shared propagation (which would cause
  `pivot_root()` to return an error)". The canonical man-page example takes the option D1's table forbids.
- **D1's own Open Question 8** says "this is a genuine disagreement among mature runtimes rather than a
  settled fact."
- **D1's own stated rationale for slave is `[C]` MED and unverified** (D5 above).

So the table promotes to "required" a choice its own body marks as unsettled. Correct classification:
*"`/` must not be `MS_SHARED`"* is **required**; *"`MS_REC|MS_SLAVE` rather than `MS_REC|MS_PRIVATE`"* is a
**recommendation** with a real tradeoff (slave keeps host→container propagation, which volume lifecycle
wants and which is also a one-way information/behaviour channel into the container). Keeping the `[R]` HIGH
in §4.2 is fine; the §9.3 row is not.

### F3. Anything in the lower rows that is actually load-bearing?

Three, in my judgement:

1. **`CLONE_INTO_CGROUP` — listed "strongly recommended", is load-bearing for correctness.** D1's own §3.2
   explains why: between `clone` and the `cgroup.procs` write, the child sits in the **parent's** cgroup and
   can fork, spike CPU, or allocate past the memory limit. That is not a hardening nicety, it is a limit that
   does not hold. runc's in-tree comment agrees — *"Do this before syncing with child so that no children can
   escape the cgroup"* — which is runc paying for the missing atomicity with a synchronisation protocol.
   Tier: **required for correct resource limits**, with the fallback (parent writes `cgroup.procs` *before*
   releasing the child from the sync pipe) as the pre-5.7 equivalent.
2. **`close_range` sweep before `execve` — listed only in §11.2 and as `[R]`, not in §9 at all.** A leaked host
   fd into the container is a documented escape class (D1 cites CVE-2024-21626 correctly). This belongs in the
   §9.5 table at least at hardening tier, and arguably required — it is one syscall.
3. **`/dev/pts` — "required in practice"** is right and I would leave it, but note that D1's §10.1 step 21
   makes `setsid()`+`TIOCSCTTY` depend on it, so for the stated MVP goal (`mydocker run alpine /bin/sh`) it is
   simply required.

And one in the other direction: **`CLONE_NEWCGROUP` is listed "hardening"** and that is generous. It hides a
path. The information it hides (`/system.slice/docker-<id>.scope`) is also derivable from `/proc/self/mountinfo`
unless you also fix that up. It is closer to **Docker convenience** with a small information-leak benefit.
Not worth arguing about, but the row's justification ("also tells an attacker where to write if they get
`CAP_SYS_ADMIN` and a writable cgroupfs") is doing work that the read-only cgroupfs mount already does.

### F4. §9.2 `CLONE_PARENT` row — **incomplete in a way that will produce an `EINVAL`**

"`CLONE_PARENT` | strongly recommended (with 3 stages)". The three-stage topology is exactly the userns
topology, and `clone(2)` says `CLONE_NEWUSER` **cannot** be combined with `CLONE_PARENT`. The row needs the
rider: the userns must come from a separate `unshare`, never from the same clone flags — which is what runc
does and what §3.3 reason 2 describes, but the two facts are never connected.

---

## G. §13's open questions — three settled, one reframed

| # | D1's open question | Status now |
|---|---|---|
| 2 | `OPEN_TREE_NAMESPACE`'s first kernel version | **SETTLED — see G1** |
| 3 | `newinstance` no-op — exact commit | **SETTLED** — `devpts: Make each mount of devpts an independent filesystem.`, Eric W. Biederman, 2016-06-05, in **v4.7** (D11) |
| 4 | Forced-vs-generated signal mechanism | **SETTLED — and D1's guess was wrong** (E2) |
| 5 | Rust `std::alloc::System` → glibc `malloc` | **Reframed — the question is the wrong one** (H4) |
| 1, 7 | `CLONE_INTO_CGROUP` end-to-end; time ns on RHEL 8 | still open; both need a host D1 correctly declined to use |
| 8 | slave vs private for `/` | still open, and §9.3 should not have pre-empted it (F2) |

### G1. `OPEN_TREE_NAMESPACE` — settled, and §4.5's confidence tags were incoherent

D1 pairs `[C]` **HIGH** ("crun goes further: `open_tree(rootfs_fd, "", OPEN_TREE_NAMESPACE|AT_RECURSIVE)` then
`setns(tree_fd, CLONE_NEWNS)` … no `unshare(CLONE_NEWNS)` and no `pivot_root` at all") with `[F]` **LOW** on
whether the flag exists in any kernel. You cannot be HIGH-confident that a runtime ships a code path while
being unable to establish that the flag it depends on exists. **MISCLASSIFIED confidence pairing.**

Both halves now settled, and — to D1's credit — **the `[C]` claim is CONFIRMED**:

- **crun 1.29.1** (the exact pinned version D1 cites) `src/libcrun/linux.c` carries
  `#ifndef OPEN_TREE_NAMESPACE / # define OPEN_TREE_NAMESPACE 2 / #endif`, the function
  `maybe_open_tree_namespace()` calling `syscall_open_tree(rootfs_fd, "", OPEN_TREE_NAMESPACE | OPEN_TREE_CLOEXEC | …)`,
  a gate `can_use_open_tree_namespace()`, and an in-tree comment describing the fallback
  *"this is retried after `unshare(CLONE_NEWNS)`"*. Exactly as D1 describes.
- **The flag is `OPEN_TREE_NAMESPACE = 2`**, submitted by Christian Brauner on **2025-12-29**
  (`mount: add OPEN_TREE_NAMESPACE`, LWN 1052262), landing in **Linux 7.0**.
- **It is not on our reference kernel.** `/usr/include/linux/mount.h` on HOST-A (6.11) defines only
  `OPEN_TREE_CLONE 1` and `OPEN_TREE_CLOEXEC O_CLOEXEC`. `open_tree(2)` has no man page on the host at all.

Two corrections follow. (a) §4.5's "`OPEN_TREE_NAMESPACE` is very new; treat it as runtime-detected only"
understates it — it is *newer than every kernel the project has access to*, and crun ships it precisely
behind a runtime probe with a local `#define`, which is the pattern to copy. (b) §9.3 files
`fsopen/fsmount/move_mount/open_tree` under "hardening" with no version caveat; the specific
pivot_root-elimination trick needs **7.0**, while the rest of the new mount API needs 5.2. Those are not the
same availability story and the row conflates them.

D1's `[R]` MED — "MVP: `mount(2)`. Design for the new API; do not adopt it yet", with mount descriptors
resolved against **dirfds rather than strings** — is **CONFIRMED as the right call**, and the dirfd advice is
the part that actually buys the TOCTOU resistance later.

### G2. §4.5's CVE citation — **CORRECTED**

"This is the family that produced CVE-2019-14271-style and runc `/proc` masking bugs."

**CVE-2019-14271** is not in that family. It is `docker cp`: the `docker-tar` helper, executing inside the
container's mount namespace, loads `libnss_*.so` from the **container's** filesystem — a dynamic-library
injection bug, not a symlink race on a mount target path. The mount-target symlink-exchange family D1 is
describing is:
- **CVE-2021-30465** — runc, symlink-exchange race on volume mount targets, letting a mount escape the
  container rootfs. This is the canonical member and the one the fd-based mount API actually prevents.
- **CVE-2019-16884** — runc, `/proc` masking bypass via a malicious volume mounted over `/proc`, defeating
  the AppArmor profile. This is the "runc `/proc` masking bug" D1 gestures at, so half the sentence is right.

The **argument** is sound and the new mount API does defeat this class; only the citation is wrong. Fix the
CVE number — it is the kind of thing that gets copied into a design doc and then into a commit message.

---

## H. §11 — Rust substrate, and the `capget`/`capset` adjudication

### H1. §11.1's crate table — **CONFIRMED, every cell**

Re-fetched from crates.io today. `libc 0.2.189`, `rustix 1.1.4`, `linux-raw-sys 0.12.1`, `nix 0.31.3`,
`caps 0.5.6` (2025-10-17), `oci-spec 0.10.0`, `cgroups-rs 0.5.1`, and `sc` stale — all match, including the
dates. `nix` issue **#360** ("clone() is broken and unsafe with CLONE_VM flag", **open**, created
**2016-04-19**) and **#2803** ("… Stack use-after-free and data race in `sched::clone` under
`CloneFlags::CLONE_VM`", **open**, created **2026-06-20**) both verified against the GitHub API — titles,
states and dates exactly as D1 states. This table is in good shape.

`libc` declarations — **CONFIRMED against the crate source**: `unshare` and `setns` are declared in
`src/unix/linux_like/linux/mod.rs`; `close_range` is declared only in `.../linux/gnu/mod.rs`, i.e. **gnu-only**,
exactly as D1 says; and there is **no `pub fn capget`/`capset`** anywhere, which `capget(2)` itself explains:
"Note: glibc provides no wrappers for these system calls, necessitating the use of `syscall(2)`." Likewise
`pivot_root(2)`: "glibc provides no wrapper for `pivot_root()`". D1's declared/not-declared split is right.

### H2. `nix::sched::clone` — conclusion **CONFIRMED**, leading argument **CORRECTED**

D1 leads with the `CLONE_VM` soundness bug and concludes `[R]` HIGH "write our own". The conclusion is right.
The leading argument does not apply to us: **our clone does not set `CLONE_VM`** (§10.1 step 8 lists
`NEWNS|NEWPID|NEWNET|NEWUTS|NEWIPC|NEWCGROUP|PIDFD|INTO_CGROUP` and nothing else). Without `CLONE_VM` the
child gets a copy-on-write snapshot of the parent's address space, so the `&mut cb`-into-a-popped-frame
hazard of #360/#2803 does not arise. The two reasons that **do** apply, and that should lead instead:
1. **`nix` has no `clone3`** — so it cannot carry `CLONE_INTO_CGROUP`, `CLONE_PIDFD` in its modern form, or
   any future flag. That alone forecloses §10.1 step 8.
2. **`stack: &mut [u8]` has no guard page** — still true without `CLONE_VM`, since the child does run on the
   caller-supplied stack. Overflow silently corrupts adjacent memory instead of faulting.

### H3. THE ADJUDICATION — `capget`/`capset` and the "~60 lines" — **the later agent is RIGHT**

D1 §11.1 assesses `caps 0.5.6`, correctly finds it allocates (`read()` → `HashSet<Capability>`, errors are
`CapsError(String)`) and correctly finds it lacks `PR_SET_NO_NEW_PRIVS`, then concludes `[R]` MED: *"write
~60 lines of `capget`/`capset`/`PR_CAPBSET_DROP`/`PR_CAP_AMBIENT` ourselves"*, and lists in the recommended
stack *"+ ~200 lines of our own: clone3, mount_setattr, close_range, capget/capset, prctl(NO_NEW_PRIVS)"*.

**Verified against rustix 1.1.4's own docs — `rustix::thread` ships the entire surface:**

| D1 says we must hand-write | rustix 1.1.4 |
|---|---|
| `capget` | `capabilities()` — documented as `capget(_LINUX_CAPABILITY_VERSION_3, pid)` |
| `capset` | `set_capabilities()` — `capset(_LINUX_CAPABILITY_VERSION_3, pid, effective, permitted, inheritable)` |
| `PR_CAPBSET_READ` / `PR_CAPBSET_DROP` | `capability_is_in_bounding_set()` / `remove_capability_from_bounding_set()` |
| `PR_CAP_AMBIENT_{IS_SET,RAISE,LOWER,CLEAR_ALL}` | `capability_is_in_ambient_set()` / `configure_capability_in_ambient_set()` / `clear_ambient_capability_set()` |
| `PR_{GET,SET}_KEEPCAPS` (§10.1 step 27) | `get_keep_capabilities()` / `set_keep_capabilities()` |
| `prctl(PR_SET_NO_NEW_PRIVS)` (§10.1 step 23) | `no_new_privs()` / `set_no_new_privs()` |
| the v3 header/data layout | `CapabilitySets` (documented as `__user_cap_data_struct`), `CapabilitySet` bitflags |
| — | bonus: `capabilities_secure_bits()` / `set_capabilities_secure_bits()` (`SECBIT_*`) |

**So: D1's `[R]` MED is obsolete, and the later agent's characterisation is accurate — D1 assessed only the
`caps` crate and never checked rustix's capability coverage**, despite recommending rustix in the same
paragraph list and praising its `linux_raw` backend two paragraphs earlier. D1's enumeration of rustix
("`fsopen`, `fsmount`, … `openat2`. `pivot_root` lives in `rustix::process`; namespaces live in
`rustix::thread` … Missing: `mount_setattr` … and `close_range`") reads as exhaustive and is not; the
"Missing:" list is however **CONFIRMED correct** — neither `mount_setattr` nor `close_range` is in 1.1.4.

Three points in D1's partial defence, which I record because they are real:
1. **`caps` genuinely is unusable post-clone** — that finding stands and is well made.
2. **rustix's capability API is alloc-free**, which is the property D1 actually cares about: `CapabilitySets`
   is a plain bitflags struct and none of these functions take a path argument (so none can hit the
   allocating slow path of H4). It is *strictly better* than hand-rolling on D1's own criteria.
3. **`Capability` (the enum) is deprecated in 1.1.4** in favour of `CapabilitySet` bitflags, and all of this
   is behind rustix's `thread` feature. Neither is an obstacle.

**Corrected recommendation:** drop `capget`/`capset` and `prctl(NO_NEW_PRIVS)` from the hand-written list.
The stack line becomes *"+ ~120 lines of our own: `clone3`, `mount_setattr`, `close_range`"*. If the team still
wants to hand-write capabilities **for the stated pedagogical reason** ("this is exactly the mechanic we want
visible"), that is a legitimate `[R]` — but it must be argued on those grounds, not as D1 argues it, on a
crate-inadequacy premise that is false.

### H4. §11.2's post-clone rules — **the rules are necessary, and as stated they are NOT sufficient**

**The gap.** §10.1 step 7 says "Pre-allocate argv/envp/path `CString`s, all mount descriptors … must happen
pre-clone: no allocation is safe post-clone". §11.2 repeats it: "every `CString` for path, argv, and envp must
be built before the clone." That rule governs *our* code and stops at the crate boundary — and the crates D1
recommends re-do the conversion themselves:

- **rustix 1.1.4** (`src/path/arg.rs`): `Arg::into_with_c_str` for `&str`/`&Path`/`&OsStr` copies into a
  stack buffer of `SMALL_PATH_BUFFER_SIZE` and falls through to `with_c_str_slow_path`, which under
  `#[cfg(feature = "alloc")]` is literally `f(&CString::new(bytes).map_err(…)?)` — **a heap allocation on the
  path to the syscall.**
- **nix 0.31.3** (`src/lib.rs`): `impl NixPath for [u8]` uses a stack array and then
  `return with_nix_path_allocating(self, f);` for anything longer. `str`, `OsStr`, `Path` and `PathBuf` all
  funnel through it. **Same hazard.**

Both crates' `impl … for CStr` is the zero-copy path.

So a container whose rootfs sits under a long path — a deep `/var/lib/mydocker/overlay2/<64-hex>/merged/...`,
a long bind-mount target from `-v`, a user-supplied `WORKDIR` — silently crosses into the allocator on a
mount call *after* the clone, in the one window §11.2 is trying to protect. It is length-dependent, so it
passes every short-path test and fails on real image paths.

**Strengthened rule, which §11.2 should carry instead of its current one:**
> Post-clone, pass **only `&CStr`** to `nix`/`rustix`. Never `&str`, `&Path`, `&OsStr`, `String`, `PathBuf`.
> Pre-build every `CString` before the clone *and* keep them as `CStr` all the way to the syscall.
> Enforce it: the child's setup functions should take `&CStr` in their signatures, so the type system
> forbids the allocating path rather than a comment discouraging it.

**And the framing is MISCLASSIFIED.** D1's own `[F]` line scopes it correctly — "after a `fork` in a
**multithreaded** process, the child may call only async-signal-safe functions" — and then the "Therefore,
post-clone and pre-`execve`, do not:" list drops the qualifier and presents an absolute prohibition as a
kernel/POSIX fact. In a process that satisfies §3.4's `Threads == 1` invariant, no other thread can hold a
malloc arena lock, glibc's `setxid` broadcast has nobody to broadcast to, and `std::io::Stdout`'s
`ReentrantLock` cannot be held by anyone else. **The `Threads == 1` assert and the no-allocation rule are
alternatives, not a stack** — and D1 recommends both without saying so, which reads as though allocation is
unsafe even in the single-threaded case. That matters because a reader who cannot achieve `Threads == 1`
(an async runtime in the process) will believe the no-allocation rule saves them. It does not: with real
threads, the *whole* async-signal-safety surface applies, not just allocation.

Two of the four rules survive **regardless of thread count** and should be labelled as such:
- **`println!`/`eprintln!`** — not because of the lock (single-threaded: no contention) but because of the
  **inherited unflushed buffer**, which duplicates output at the child's exit. D1 observed exactly this and
  reported it, but attributes the prohibition to the lock. Correct the reason; keep the rule.
- **panic/unwind** — a panic in the cloned child unwinds through a stack whose destructors belong to the
  parent's logical program, and the default hook writes to that same inherited buffer. Rule stands.

The **glibc `setxid` rule is CONFIRMED and is the most important one in §11.2** — and D1 scopes it exactly
right, to `clone` rather than `fork`: glibc's `fork()` wrapper resets its thread bookkeeping in the child, a
raw `clone()` does not, so the child inherits a thread list describing threads that do not exist and the
`setxid` broadcast waits forever. youki's raw-`SYS_setresuid` workaround and its in-tree comment are the
right precedent. **Keep §10.1 step 27's "use raw syscalls" as an absolute.**

### H5. §11.2 open question 5 — **reframed**

D1 asks whether `std::alloc::System` maps to glibc `malloc` on every target, and marks the no-allocation rule
as depending on the answer. Given H4, that is the wrong question: the rule's necessity is decided by whether
the process is multithreaded at clone time, not by which allocator is underneath, and the *unfixed* gap is at
the crate boundary rather than in `std`. The question worth carrying forward instead: **"does any dependency
in the cloning process spawn a thread?"** — which §3.4's `/proc/self/status:Threads` assert already answers at
runtime, and which a `cargo tree` review answers at build time.

---

## I. D2 §4 — the creation-sequence ordering constraints

Scope as assigned: §4 only (lines 396–444).

### I1. runc's parent-side order — **CONFIRMED, with two nits**

Checked against runc **v1.5.1** `libcontainer/process_linux.go`, `initProcess.start()`. D2's 14-step listing
is substantially accurate, and the load-bearing citation is **verbatim**:

> `manager.Apply(p.pid())` carries the comment *"Do this before syncing with child so that no children can
> escape the cgroup"*.

Nits, neither of which changes a conclusion:
- **Steps 8 and 9 are transposed.** In v1.5.1 the order is `getChildPid()` → **`getPipeFds()`** →
  **`waitForChildExit()`**; D2 lists `waitForChildExit` before `getPipeFds`. D2's own commentary ("record
  `/proc/<pid>/fd/{0,1,2}` targets **BEFORE** the container can move them") is the reason the real order is
  the safer one, so the argument survives its own listing error.
- **Two steps are missing** between `createNetworkInterfaces()` and the `parseSync` loop: `setupNetworkDevices()`
  and a `WriteJSON` of the config to the init process. Immaterial to the ordering claims.

### I2. D2 §4's distilled rules — verdicts

| Rule | Verdict |
|---|---|
| 1. Create the cgroup **before** forking anything that will hold the container | **CONFIRMED** — matches runc's step 3 and D1 §10.1 step 4. No conflict. |
| 2. Put the child in the cgroup before it forks; `CLONE_INTO_CGROUP` is strictly better | **CONFIRMED**, with a caveat below |
| 3. Record the pre-container identity of fds 0/1/2 at create time | **CONFIRMED** — `external_descriptors` in `state.json`; the mechanism is real and the reasoning is right |
| 4. Write the state file when the child says "ready" | **CONFIRMED** — runc writes `state.json` in the `procReady` handler |
| 5. On failure: kill, reap, unwind in reverse, read the cgroup OOM counter before reporting | **CONFIRMED** — and the `[R]` HIGH to copy the OOM-counter trick is the best small idea in the section |

**Caveat on rule 2(a).** D2 offers "child writes its own pid to `cgroup.procs` as its first act" as the
non-`clone3` fallback. It works — cgroup v2 resolves a written pid in the **writer's** PID namespace, and `0`
means "the calling process" — but it carries two unstated preconditions that will bite:
1. The cgroupfs must still be **reachable by path** from the child. That is true only *before* `pivot_root`
   and `umount2(put_old, MNT_DETACH)` (D1 §5.4), i.e. it constrains where in D1's §10.1 sequence the write
   can sit — between steps 8 and 11, or via a dirfd held across the pivot.
2. **No runtime actually does this.** runc has the *parent* write the child's pid (`manager.Apply(p.pid())`),
   which is why the "before syncing with child" comment exists at all. D2's own step 3 shows the parent doing
   it. The child-writes-its-own-pid variant is D2's invention and should be marked as such or dropped in
   favour of the parent-write fallback, which needs no cgroupfs inside the container.

**`[F]` MED "`CLONE_INTO_CGROUP` requires kernel ≥ 5.7" — CONFIRMED** (`clone(2)`). **`[O]` "`syscall(435,0,0)`
returned `EINVAL` (22), not `ENOSYS` (38)" — CONFIRMED as a valid availability inference**, and it is the same
probe-don't-parse discipline D1 §2.1 recommends.

### I3. D2 §4 vs D1 §10.1 — one divergence, and it is D1's

D1 §10.1 orders **step 10 (parent plumbs the netns) before step 11 (child's propagation change and mounts)**,
and §3.5 rule 8 presents this as a runc-sourced constraint. It is not one. In runc, `createNetworkInterfaces()`
(parent) and `prepareRootfs()` (child) run **concurrently**, rendezvousing only at `procReady`/`procRun`.
The genuine kernel-imposed constraint is narrower and D1 states it correctly elsewhere: netns plumbing must
complete **before the child drops its bounding set and installs seccomp**, because the netlink calls need the
capabilities and must be permitted by the profile.

Recommendation: keep D1's §10.1 row 10 where it is (a serial engine is simpler and the ordering is harmless),
but relabel it `[R]` rather than `[F]`/`[C]`, and note that steps 10 and 11–16 are independent — which matters
the day anyone wants to overlap them for start latency.

### I4. Not contradicted

D2 §4's cgroup-before-fork rule and D1 §3.5 rule 5 (cgroup **namespace** after cgroup **placement**) are
complementary, not conflicting: placement is a parent action at/around clone time; the cgroup namespace is
unshared later, by the child, and the cgroup it is in *at that moment* becomes the namespace root
(`cgroup_namespaces(7)`, C6 rule 5). Both documents get this right, from different directions.

---

## J. Claims examined, and what is left open

**Counted.** ~100 of D1's 216 tagged claims examined in detail — every `[F]` in §§1–5, 7, 11 that carries a
quotation or a version number; every ordering rule in §3.5 and §10.1; the whole of §9; the load-bearing `[O]`
transcripts in §§1.2, 1.8, 3.3, 3.4, 4.2, 4.4, 5.2, 6.1–6.8, 7.1–7.4; and §11.1's crate table cell by cell.
Plus D2 §4 in full.

- **CONFIRMED: 71.** Of these, 9 are now *stronger* than D1 left them — independently reproduced, given a
  primary source D1 lacked, or promoted from `[O]`/MED to `[F]`/HIGH (D2a, D9, D11, C6-rule-5, E2, G1, H1,
  B3, C5).
- **CORRECTED: 12** — A6, A7, A8, A9, A13, A14, D1b, D2b, B2(a), B4, C6-rule-8, I1.
- **REFUTED: 4** — A1, A3, A4, A5.
- **OUTDATED: 1** — A2 (`setns` "just one namespace", superseded by the Linux 5.8 pidfd form).
- **MISCLASSIFIED: 5** — A10, A11, A12, A15, F4.
- **UNSUPPORTED: 7** — the ROCKY-only observations that I could not reproduce (§1.6 41-cap set, §2/§2.1's
  ROCKY columns, §6.2 `mknod` `EPERM`, §2's time-ns backport row), plus §4.2's unverified devicemapper
  rationale, plus §5.2's C1/C4 as *run*.
- **New finding not in D1: 1** — `unshare(2)`'s `CLONE_NEWPID` paragraph is wrong, and D1's own §3.4 data
  proves it (C5).

**Could not settle.**
1. **Everything ROCKY.** `old-host.example.edu` → NXDOMAIN. Three of the four load-bearing ROCKY claims (§6.1, §6.5,
   §6.6) I replaced with equivalent HOST-A measurements, which is strictly better evidence; the remaining
   ones need the host. Exact commands given inline (C7, D1c, D10).
2. **`CLONE_INTO_CGROUP` end-to-end** (D1 Open Q1). Needs a cgroup created on a non-production host. D1 was
   right to decline; so am I.
3. **Whether `MS_REC|MS_SLAVE` or `MS_REC|MS_PRIVATE` is right for `/`** (D1 Open Q8). Genuinely unsettled
   between mature runtimes; my only contribution is that `pivot_root(2)`'s own EXAMPLES program uses
   `MS_REC|MS_PRIVATE`, which is one more data point for private and one more reason the §9.3 row should not
   say "required" (F2).
4. **runc's devicemapper rationale for slave over private.** Not present in v1.5.1's `prepareRoot`; either it
   moved, or it is in the history rather than the tree. `git log -S devicemapper libcontainer/rootfs_linux.go`
   would settle it.
5. **Time namespace on RHEL 8** (D1 Open Q7) — needs ROCKY.

**Sources used.** `man-pages 6.8-2` on HOST-A (`pivot_root(2)`, `chroot(2)`, `capget(2)`, `clone(2)`,
`unshare(2)`, `setns(2)`, `seccomp(2)`, `mount(2)`, `namespaces(7)`, `pid_namespaces(7)`,
`mount_namespaces(7)`, `user_namespaces(7)`, `cgroup_namespaces(7)`, `time_namespaces(7)`);
`/usr/include/linux/sched.h`, `/usr/include/linux/mount.h`, `/usr/include/x86_64-linux-gnu/asm/unistd_64.h`
on HOST-A; Linux v6.11 `kernel/signal.c`, `kernel/nsproxy.c`, `kernel/time/namespace.c`, `kernel/fork.c`;
`git.kernel.org` log for `fs/devpts/inode.c` at `v4.7`; LWN 1052262; runc **v1.5.1**
`libcontainer/{rootfs_linux.go, standard_init_linux.go, process_linux.go}`; crun **1.29.1**
`src/libcrun/linux.c`; crates.io and GitHub APIs for `nix`/`rustix`/`libc`/`caps`/`oci-spec`/`cgroups-rs`
and nix issues #360/#2803; `docs.rs/rustix/1.1.4`.

**Empirical hygiene.** All HOST-A experiments ran inside `unshare -m --propagation private` with scratch
under `/tmp`; `grep -c … /proc/self/mountinfo` returned **0** after every one; `/` remained `shared`; all
scratch files and directories removed. No container, systemd unit, iptables/nft rule, network interface, or
`:80`/`:443` state was created, modified, or stopped. `docker` was used read-only (`ps`, `inspect -f`).
