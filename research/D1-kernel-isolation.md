# D1 — Kernel isolation mechanics & the container creation sequence

Research reference. Claim tags: `[F]` fact about Linux (primary source), `[C]` convention
(what a runtime chose), `[R]` recommendation (ours, with tradeoff), `[O]` empirically
verified (host + kernel + command). Confidence `HIGH`/`MED`/`LOW`.

## 0. Method and empirical baseline

Two hosts. Every `[O]` claim is scoped to exactly one of them — never generalised to `[F]`.

| Tag | Host | Kernel | Notes |
|---|---|---|---|
| **DROPLET** | `my-droplet` = `AIKYNETIX-PROD-NY`, Ubuntu 24.10 | `6.11.0-29-generic` x86_64 | root, cgroup v2 unified, docker+containerd+runc, AppArmor on. **Live production.** All experiments confined to private mount namespaces with scratch under `/tmp`; every one verified non-leaking afterwards. |
| **ROCKY** | `uc-arcc` = `arcc2`, Rocky Linux 8.8 | `4.18.0-425.3.1.el8` x86_64 | no root, cgroup v1, unprivileged userns works, NFS home. Old-kernel + rootless baseline. |

Local man-pages package on DROPLET is `manpages-dev 6.8-2` — relevant because §3.6 documents a
place where those man pages are **wrong** about a 6.11 kernel.

`[O]` DROPLET 6.11.0-29 — `ls /proc/self/ns/` lists all 10 entries: `cgroup ipc mnt net pid
pid_for_children time time_for_children user uts`. Same 10 on ROCKY 4.18.0-425 — RHEL 8
backported the time namespace, although its `util-linux 2.32.1` has no `unshare -T` to drive it.
Confidence HIGH.

### 0.1 What Docker actually creates — measured, not assumed

`[O]` DROPLET 6.11.0-29. `lsns`, and `readlink /proc/82271/ns/*` on a live `redis:7-alpine`
container (host PID 82271) versus `/proc/1/ns/*`:

| ns | container | host pid 1 | isolated? |
|---|---|---|---|
| mnt | `4026532358` | `4026531841` | **yes** |
| uts | `4026532359` | `4026531838` | **yes** |
| ipc | `4026532360` | `4026531839` | **yes** |
| pid | `4026532361` | `4026531836` | **yes** |
| cgroup | `4026532362` | `4026531835` | **yes** |
| net | `4026532363` | `4026531840` | **yes** |
| **user** | `4026531837` | `4026531837` | **NO — shared with host** |
| **time** | `4026531834` | `4026531834` | **NO — shared with host** |

`[C]` HIGH — Docker's default is **6 of 8 namespaces**. No user namespace (that is
`--userns-remap`, opt-in) and no time namespace. `lsns -t time` on DROPLET shows a single time
namespace holding all 159 processes on the box. Anyone claiming "Docker isolates with
namespaces" is describing 6 of them.

`[R]` HIGH — MVP creates the same 6. `user` is D5's rootless track; `time` is a
Docker-convenience-tier extra nobody asks for. Tradeoff: skipping `user` means container root
*is* host root modulo capabilities/seccomp/LSM — the container is one kernel bug from a full
escape. That is Docker's actual default posture, so we are not doing worse than the incumbent,
but it must be a stated decision, not an accident.

---

## 1. The eight namespaces

For each: what it virtualizes, what it explicitly does **not**, what breaks without it, version.

### 1.1 Mount — `CLONE_NEWNS` (Linux 2.4.19)

**Virtualizes:** the list of mount points — one `struct mnt_namespace` holding a tree of
`struct mount`. `[F]` HIGH `namespaces(7)`: "Mount points".

**Does NOT:** isolate the *filesystem contents*, the superblocks, block devices, the page cache,
inode/dentry caches, or the root directory of the calling process. A new mount namespace starts
as a **copy of the caller's mount list** — every host mount is still visible and, absent
propagation changes, still shared. It does not change your `/`. It gives you no process, network,
or resource isolation.

**Breaks without it:** every mount you make for the container lands in the host mount table and
persists after the container dies (see §4.2 for the measured leak). `pivot_root` would repoint
the *host's* root. There is no container without this namespace — it is the one that makes a
private rootfs possible at all.

**Ordering note `[F]` HIGH:** `unshare(CLONE_NEWNS)` moves the caller immediately; no fork needed.
`[O]` DROPLET 6.11 — in-process `libc.unshare(CLONE_NEWNS)` in a 4-thread Python process returned
0 (see §3.4 table).

### 1.2 PID — `CLONE_NEWPID` (Linux 2.6.24)

**Virtualizes:** the PID number space. A process has one PID per namespace in its ancestor chain;
`getpid()` returns the innermost. `[F]` HIGH `pid_namespaces(7)`.

**Does NOT:** hide processes by itself. It does not remount `/proc`; it does not stop you from
seeing every host process through an inherited procfs mount; it does not isolate mounts,
networking, IPC, users, or resources. It also does not restrict `/proc/<hostpid>/` access if a
host procfs is reachable.

**Breaks without it (measured):**

`[O]` DROPLET 6.11.0-29 —
```
unshare -m --propagation private  sh -c 'mount -t proc proc /proc; ls /proc | grep -cE "^[0-9]+$"'   -> 163
unshare -mpf --propagation private sh -c 'mount -t proc proc /proc; ls /proc | grep -cE "^[0-9]+$"'  ->   3
unshare -mpf --propagation private sh -c '                        ls /proc | grep -cE "^[0-9]+$"'    -> 164
```
Confidence HIGH. Read the third line carefully: **a new PID namespace with the inherited `/proc`
still mounted shows all 164 host processes.** PID isolation and a fresh procfs are *both*
required; either alone gives nothing. The inherited procfs instance is pinned to the PID
namespace it was mounted in.

**Hard constraints `[F]` HIGH:**
- `unshare(CLONE_NEWPID)` does **not** move the caller. `pid_namespaces(7)`: "calls to `unshare(2)`
  with the `CLONE_NEWPID` flag cause children subsequently created by the caller to be placed in a
  different PID namespace from the caller." The caller's `/proc/self/ns/pid_for_children` changes;
  `ns/pid` does not.
- `clone(CLONE_NEWPID)` **does** put the child in the new namespace as PID 1 (§3.3 — this is the
  correction to the project spec's premise).
- Only once per process: `unshare(2)` ERRORS — "EINVAL `CLONE_NEWPID` was specified in flags, but
  the process has previously called `unshare()` with the `CLONE_NEWPID` flag."
- Cannot combine with `CLONE_THREAD`. `[F]` HIGH `pid_namespaces(7)`.
- Nesting depth capped at 32 since Linux 3.7; exceeding it gives `ENOSPC`. `[F]` HIGH.

### 1.3 Network — `CLONE_NEWNET` (Linux 2.6.24, "completed" 2.6.29)

**Virtualizes:** network devices, IPv4/IPv6 stacks, routing tables, firewall rules, the
`/proc/net` and `/sys/class/net` trees, port number space, UNIX **abstract** socket namespace.
`[F]` HIGH `namespaces(7)`, `network_namespaces(7)`.

**Does NOT:** give the container any connectivity. A fresh netns contains exactly one device —
`lo`, and it is **down**. It does not isolate filesystem-backed UNIX sockets (those follow the
mount namespace), does not isolate anything else, and does not create veth pairs, bridges, or NAT
for you. Cross-namespace plumbing is entirely userspace's job (D4).

**Breaks without it:** the container shares the host's stack — it can bind host ports, reach
host-only loopback services, see and manipulate host interfaces given `CAP_NET_ADMIN`, and sniff
with `CAP_NET_RAW`. Note Docker's default capability set **includes** `CAP_NET_RAW` (§7.2).

**MVP note `[R]` MED:** an empty netns with only `lo` (brought up) is a legitimate MVP endpoint —
`mydocker run alpine /bin/sh` works fine with no external connectivity. Deferring veth/bridge to
D4 does not block the runtime core. Tradeoff: no DNS, no package installs inside the container,
so it looks broken to a user even though the isolation is correct.

### 1.4 UTS — `CLONE_NEWUTS` (Linux 2.6.19)

**Virtualizes:** exactly two strings — the hostname (`sethostname(2)`) and the NIS domain name
(`setdomainname(2)`). `[F]` HIGH `uts_namespaces(7)`. Also the corresponding fields of
`uname(2)`.

**Does NOT:** virtualize the kernel release/version fields of `uname(2)` (the container always
sees the host kernel), and nothing else at all. It is the cheapest namespace on the list.

**Breaks without it:** `hostname` inside the container renames the **host**. Purely cosmetic for
most workloads, actively destructive if a container's entrypoint sets its hostname.

`[O]` DROPLET 6.11.0-29 — `unshare -u sh -c 'hostname d1-test; hostname'` printed `d1-test` while
the host's `hostname` remained `AIKYNETIX-PROD-NY`. Confidence HIGH. Note `-u` alone, no `--fork`:
`unshare(CLONE_NEWUTS)` moves the caller immediately.

### 1.5 IPC — `CLONE_NEWIPC` (Linux 2.6.19)

**Virtualizes:** System V IPC objects (message queues, semaphore sets, shared memory segments)
and POSIX message queues. `[F]` HIGH `ipc_namespaces(7)`. Also the associated
`/proc/sys/fs/mqueue/*`, `/proc/sys/kernel/{shmmax,shmall,msgmax,...}` and `/proc/sysvipc/*`.

**Does NOT:** cover POSIX **shared memory** (`shm_open(3)`) or POSIX **named semaphores**
(`sem_open(3)`) — glibc implements both as files under `/dev/shm`, so those are isolated by the
*mount* namespace, not this one. Does not isolate signals, pipes, UNIX sockets, or futexes.

**Breaks without it:** containers can read/write each other's and the host's SysV segments, and
exhaust the global SysV limits. Also: the IPC namespace is what makes `mount -t mqueue` legal in
a user namespace (§6.6).

`[O]` DROPLET 6.11.0-29 — created a SysV shm segment with key `0x4d1c0de` inside
`unshare -i python3 -c 'libc.shmget(...)'`; `ipcs -m | grep -c 4d1c0de` returned `1` inside the
namespace and `0` on the host after it died. Confidence HIGH.

### 1.6 User — `CLONE_NEWUSER` (complete in Linux 3.8)

**Virtualizes:** UID/GID number spaces, the capability model, and the ownership of all other
namespaces. Every non-user namespace has an *owning* user namespace; capabilities are evaluated
against it.

**Does NOT:** map anything by itself. A fresh user namespace has an empty `uid_map`/`gid_map` and
every unmapped ID renders as the overflow ID (65534). It does not grant privilege over resources
that are not namespaced — `CAP_SYS_TIME`, `CAP_SYS_MODULE`, and critically **`CAP_MKNOD`** are
useless outside the initial user namespace. `[F]` HIGH `user_namespaces(7)`: "there are many
privileged operations that affect resources that are not associated with any namespace type, for
example, changing the system (i.e., calendar) time (governed by CAP_SYS_TIME), loading a kernel
module (governed by CAP_SYS_MODULE), and creating a device (governed by CAP_MKNOD). Only a
process with privileges in the initial user namespace can perform such operations."

**Two measurements that decide our design:**

`[O]` ROCKY 4.18.0-425, unprivileged uid 780140869 — immediately after
`libc.unshare(CLONE_NEWUSER)` with **no execve**:
```
after  uid=65534 euid=65534
after  CapPrm: 000001ffffffffff
after  CapEff: 000001ffffffffff
after  CapBnd: 000001ffffffffff
uid_map=[]        setgroups= allow
```
Full 41-capability set in the new namespace, exactly as `unshare(2)` documents ("the caller
obtains a full set of capabilities in the new namespace"). Confidence HIGH.

`[O]` DROPLET 6.11.0-29 — the same namespace **after an execve**:
`setpriv --reuid=1000 --regid=1000 --clear-groups unshare -U sh -c 'grep -E "^(Uid|CapEff|CapBnd)" /proc/self/status'`
→ `Uid: 65534`, **`CapEff: 0000000000000000`**, `CapBnd: 000001ffffffffff`. Confidence HIGH.

`[F]` HIGH (derived, `capabilities(7)` execve rules) — **the capability set gained from
`CLONE_NEWUSER` does not survive `execve` unless uid 0 is mapped in the new namespace.** The exec
of a non-setuid file by a non-root euid with an empty ambient set clears permitted and effective.
This is the load-bearing reason `uid_map` must be written by the parent *before* the child execs,
and it is why the parent/child split exists at all for the rootless case.

**Breaks without it:** container uid 0 is host uid 0. Any container escape is instantly root on
the host, and file ownership on bind mounts is unmediated.

**Hard constraints `[F]` HIGH:**
- `unshare(CLONE_NEWUSER)` **requires a single-threaded process**. `unshare(2)`: "CLONE_NEWUSER
  requires that the calling process is not threaded; specifying CLONE_NEWUSER automatically
  implies CLONE_THREAD" plus "EINVAL CLONE_THREAD, CLONE_SIGHAND, or CLONE_VM was specified in
  flags, and the caller is multithreaded." Verified in §3.4 — it is the *only* namespace flag
  with this restriction.
- `setns(fd, CLONE_NEWUSER)` into your own current user namespace → `EINVAL`.
  `[O]` DROPLET 6.11.0-29, `libc.setns(open("/proc/1/ns/user"), CLONE_NEWUSER)` → `EINVAL`. HIGH.
- Writing `gid_map` requires `"deny"` be written to `/proc/<pid>/setgroups` **first** (unless the
  writer holds `CAP_SETGID` in the parent ns). `[F]` HIGH `user_namespaces(7)`: "use of the
  `setgroups(2)` system call must first be denied by writing 'deny' to the
  `/proc/pid/setgroups` file ... before writing to `gid_map`", and once `gid_map` is written the
  `setgroups` file can no longer be set to deny (`EPERM`). CVE-2014-8989 is the reason.
- Multi-line maps require `CAP_SETUID`/`CAP_SETGID` in the parent namespace; hence the
  `newuidmap`/`newgidmap` setuid helpers and `/etc/subuid`. `[F]` HIGH `user_namespaces(7)`.
  (ROCKY has **no** `/etc/subuid` entries — single-ID maps only there.)
- Ubuntu ships an AppArmor brake on this: `[O]` DROPLET 6.11.0-29,
  `cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns` → `1`. Relevant to D5's rootless
  track; irrelevant to our root-first MVP.

### 1.7 Cgroup — `CLONE_NEWCGROUP` (Linux 4.6)

**Virtualizes:** the cgroup *root directory* as seen through `/proc/<pid>/cgroup` and through a
`cgroup2` filesystem mounted inside the namespace. The process's cgroup at creation time becomes
the apparent root.

**Does NOT:** limit anything, delegate anything, or prevent escape to a parent cgroup. It is a
**path-hiding** mechanism, full stop. All actual resource control is `cgroup.*` file writes (D5).

`[O]` DROPLET 6.11.0-29:
```
host  view: 0::/user.slice/user-0.slice/session-30644.scope
cgns  view: 0::/                                    (unshare -C)
container(redis), read from the HOST: 0::/system.slice/docker-db69f93....scope
```
Confidence HIGH. Note the third line: `/proc/<pid>/cgroup` is rendered **relative to the
reader's** cgroup namespace, not the target's. From the host you see the absolute path; from
inside, `/`. This matters for our engine's introspection code — do not expect a container's own
view when reading its cgroup file from the daemon.

**Breaks without it:** the container reads its full host cgroup path — an information leak that
also tells an attacker where to write if they get `CAP_SYS_ADMIN` and a writable cgroupfs. Also,
`mount -t cgroup2` inside would expose the whole host hierarchy rather than a subtree.

**Constraint `[F]` HIGH `user_namespaces(7)`:** "Holding `CAP_SYS_ADMIN` within the user namespace
that owns a process's cgroup namespace allows (since Linux 4.6) that process to mount the cgroup
version 2 filesystem" — same shape as the proc/sysfs/mqueue rules in §6.

### 1.8 Time — `CLONE_NEWTIME` (Linux 5.6)

**Virtualizes:** exactly two clocks — `CLOCK_MONOTONIC` (and `_COARSE`/`_RAW`) and
`CLOCK_BOOTTIME` (and `_ALARM`), via per-namespace offsets in `/proc/<pid>/timens_offsets`.
Affects `clock_gettime(2)`, `nanosleep(2)`, `clock_nanosleep(2)`, `timer_settime(2)`,
`timerfd_settime(2)`, `/proc/uptime`. `[F]` HIGH `time_namespaces(7)`.

**Does NOT:** virtualize `CLOCK_REALTIME`. You cannot give a container a different wall-clock
date. That is the single most common misconception about this namespace and it kills most of the
use cases people imagine for it.

**Breaks without it:** essentially nothing for normal workloads. `/proc/uptime` and monotonic
clocks show host values. Useful only for testing time-dependent code and for checkpoint/restore
(CRIU is the actual driver).

**Special semantics `[F]` HIGH `time_namespaces(7)`:** "This call creates a new time namespace but
does not place the calling process in the new namespace. Instead, the calling process's
subsequently created children are placed in the new namespace." — the offsets must be settable
before any process is inside.

`[O]` DROPLET 6.11.0-29 —
```
before: ns/time=time:[4026531834]  ns/time_for_children=time:[4026531834]
unshare(CLONE_NEWTIME) -> OK
after : ns/time=time:[4026531834]  ns/time_for_children=time:[4026532570]
```
Confidence HIGH. Caller did not move; only `time_for_children` changed. Same split-link pattern
as `pid_for_children`.

**Documentation contradiction — high-value finding.** `time_namespaces(7)` as shipped in
man-pages 6.8 states: *"Currently, the only way to create a time namespace is by calling
`unshare(2)` with the `CLONE_NEWTIME` flag."* That is **false on 6.11**.

`[O]` DROPLET 6.11.0-29 — `clone3()` with `cl_args.flags = CLONE_NEWTIME (0x80)`:
```
parent time ns: time:[4026531834]
   child time=time:[4026532570]  tfc=time:[4026532570]
```
The child is in a genuinely new time namespace, and unlike the `unshare` path it is placed there
directly. Confidence HIGH. Separately, `clone(2)`'s man page does not mention `CLONE_NEWTIME` at
all. The reason legacy `clone(2)` *cannot* carry it: `CLONE_NEWTIME == 0x80` lies inside the
legacy `CSIGNAL` mask (`0xff`). Verified — `[O]` DROPLET 6.11.0-29, `libc.clone(fn, stack, 0x80,
NULL)` returned a normal child in the **parent's** time namespace, having interpreted `0x80` as
`exit_signal = 16` (`SIGSTKFLT`); the subsequent `waitpid` then failed `ECHILD`. Confidence HIGH.
So: `CLONE_NEWTIME` is reachable via `unshare(2)` **and `clone3(2)`**, and via `clone3` only
because clone3 moved flags to a 64-bit field. `[R]` low priority for us, but the flag-space
lesson generalises: prefer `clone3` for any new flag.

---

## 2. Namespace availability by kernel version

`[F]` HIGH — `namespaces(7)` version column, cross-checked against both hosts.

| Namespace | Flag | Since | ROCKY 4.18 | DROPLET 6.11 |
|---|---|---|---|---|
| Mount | `CLONE_NEWNS` `0x00020000` | 2.4.19 | yes | yes |
| UTS | `CLONE_NEWUTS` `0x04000000` | 2.6.19 | yes | yes |
| IPC | `CLONE_NEWIPC` `0x08000000` | 2.6.19 | yes | yes |
| PID | `CLONE_NEWPID` `0x20000000` | 2.6.24 | yes | yes |
| Network | `CLONE_NEWNET` `0x40000000` | 2.6.24 / 2.6.29 | yes | yes |
| User | `CLONE_NEWUSER` `0x10000000` | 2.6.23 / **3.8** | yes | yes |
| Cgroup | `CLONE_NEWCGROUP` `0x02000000` | 4.6 | yes | yes |
| Time | `CLONE_NEWTIME` `0x00000080` | 5.6 | **yes (RHEL backport)** | yes |

`[O]` — the Time row for ROCKY is an observation, not a version claim: `/proc/self/ns/time` and
`time_for_children` both exist on 4.18.0-425.3.1.el8. Its `util-linux 2.32.1` predates
`unshare -T`, so the flag must be driven by syscall there. Confidence MED (the links exist;
I did not exercise the offsets on that host).

Per-namespace count limits, `[O]` DROPLET 6.11.0-29,
`cat /proc/sys/user/max_*_namespaces` → all eight at `63826`. Exceeding gives `ENOSPC`
(`EUSERS` on 3.11–4.8). `[F]` HIGH `unshare(2)`.

### 2.1 Syscall availability — measured, not inferred

`[O]` ctypes probe via raw `syscall()`, distinguishing `ENOSYS` (absent) from any other errno
(present):

| syscall | nr | mainline since | ROCKY 4.18 | DROPLET 6.11 |
|---|---|---|---|---|
| `pivot_root` | 155 | 2.3.41 | present | present |
| `open_tree` | 428 | 5.2 | **present (backport)** | present |
| `move_mount` | 429 | 5.2 | **present (backport)** | present |
| `fsopen` | 430 | 5.2 | **present (backport)** | present |
| `fsconfig` | 431 | 5.2 | present | present |
| `fsmount` | 432 | 5.2 | **present (backport)** | present |
| `fspick` | 433 | 5.2 | present | present |
| `clone3` | 435 | 5.3 | **ENOSYS** | present |
| `close_range` | 436 | 5.9 | **present (backport)** | present |
| `openat2` | 437 | 5.6 | **present (backport)** | present |
| `mount_setattr` | 442 | 5.12 | **ENOSYS** | present |
| `statmount` | 457 | 6.8 | ENOSYS | present |
| `listmount` | 458 | 6.8 | ENOSYS | present |

Confidence HIGH for both columns. The RHEL-8 backport pattern is a genuine trap: **you cannot
infer syscall availability from `uname -r` on enterprise kernels.** `[R]` HIGH — runtime-detect
every optional syscall by calling it and checking for `ENOSYS`; never gate on a parsed kernel
version. Cost is one probe per feature at startup; benefit is not being wrong on RHEL, SLES, and
every vendor kernel.

Full new-mount-API round trip works on DROPLET, `[O]` 6.11.0-29, inside
`unshare -m --propagation private`:
`fsopen("tmpfs",0)`→fd 3, `fsconfig(fd,FSCONFIG_CMD_CREATE,...)`→0,
`fsmount(fd,0,0)`→fd 4 (detached mount), `move_mount(4,"",AT_FDCWD,"/tmp/d1_newapi",
MOVE_MOUNT_F_EMPTY_PATH)`→0. Host mount table unchanged (`grep -c d1_newapi
/proc/self/mountinfo` → 0). Confidence HIGH.

---

## 3. Process-creation primitives

### 3.1 The three calls, and who they affect

`[F]` HIGH `namespaces(7)`, `clone(2)`, `unshare(2)`, `setns(2)`.

| Call | Affects | New ns? | Fork needed? |
|---|---|---|---|
| `clone(2)` / `clone3(2)` with `CLONE_NEW*` | the **child** | creates | no — the child is born inside |
| `unshare(2)` with `CLONE_NEW*` | the **caller** | creates | only for PID and TIME |
| `setns(2)` with an ns fd | the **calling thread** | joins existing | only for PID and TIME |

Two flags are special in the same way: `CLONE_NEWPID` and `CLONE_NEWTIME` via `unshare`/`setns`
affect *subsequently created children only*, which is exactly why `/proc/<pid>/ns/` exposes
`pid_for_children` and `time_for_children` as separate links. `[F]` HIGH.

`setns(2)` is **per-thread**, not per-process — the man page's title is "reassociate *thread*
with a namespace" and "each call to `setns()` changes just one of the caller's namespace
memberships." `[F]` HIGH. Consequence for any threaded language runtime: an `unshare`/`setns` on
one thread leaves the rest of the process behind, and there is no way to fix that up afterwards.

`clone2` is Itanium-only (`clone2(2)` exists on ia64 for the "stack grows up" ABI). `[F]` MED —
irrelevant to x86_64/aarch64; ignore.

### 3.2 `clone` vs `clone3`

`[F]` HIGH `clone(2)`. Legacy `clone(2)` packs flags and the child's termination signal into one
`int`: the low byte (`CSIGNAL = 0xff`) is the exit signal, so the flag space is 24 usable bits and
is **exhausted**. `clone3(2)` (Linux 5.3) takes `struct clone_args` with a 64-bit `flags` and
separate `exit_signal`, and adds `set_tid`, `set_tid_size`, `cgroup`, and extensible-struct
versioning.

`[O]` DROPLET 6.11.0-29 — the kernel accepts `clone3` with `size` = 64, 80, and 88; each returned
a live child pid. `CLONE_PIDFD` populated a pidfd; child exit status propagated (`status 768`,
`>>8 == 3`). Confidence HIGH.

**`CLONE_INTO_CGROUP` (Linux 5.7).** `[F]` HIGH `clone(2)`: "allows the child process to be
created in a different version 2 cgroup ... the caller specifies `CLONE_INTO_CGROUP` in
`cl_args.flags` and passes a file descriptor that refers to a version 2 cgroup in the
`cl_args.cgroup` field. (This file descriptor can be obtained by opening a cgroup v2 directory
using either the `O_RDONLY` or the `O_PATH` flag.) Note that all of the usual restrictions
(described in `cgroups(7)`) on placing a process into a version 2 cgroup apply." cgroup **v2
only**.

`[O]` DROPLET 6.11.0-29 — `clone3(flags=CLONE_INTO_CGROUP, cgroup=99999)` returned **`EBADF`**,
not `EINVAL`. The flag is recognised and validated; only the fd was bogus. Confidence HIGH. (I
deliberately did not create a scratch cgroup on a production host, so I verified flag support
without exercising a successful placement — see Open questions.)

Why it matters: without it you `clone`, then write the child's pid into `cgroup.procs`. Between
those two events the child is in the **parent's** cgroup and unconstrained — it can fork, spike
CPU, or allocate past the intended memory limit. `CLONE_INTO_CGROUP` closes that window
atomically. `[R]` MED for MVP: use it when `clone3` is available, keep the
`clone`+`cgroup.procs`-write path as fallback for pre-5.7 and cgroup v1. Tradeoff: two code paths
forever, but the fallback is 3 lines and you need it for ROCKY-class kernels anyway (`clone3` is
`ENOSYS` there).

`[R]` HIGH — `clone3` first, fall back to legacy `clone` **only on `ENOSYS`**. `[C]` youki does
exactly this. One documented sharp edge worth stealing from youki: with `CLONE_PARENT`, `clone3`
returns `EINVAL` if `exit_signal` is set, whereas legacy `clone` silently ignores the signal bits.

### 3.3 The "extra fork for `CLONE_NEWPID`" — the spec's premise is wrong

The project spec asks "why `CLONE_NEWPID` requires an extra fork before the new namespace's PID 1
exists." **It does not, if you use `clone`.**

`[O]` DROPLET 6.11.0-29 — `clone3(flags = CLONE_PIDFD|CLONE_NEWPID)`:
```
[child]  getpid()=1   <- PID 1 directly, no extra fork needed
[parent] child host-pid=2762871 pidfd=3
[parent] child ns: pid:[4026532570]   our ns: pid:[4026531836]
```
Confidence HIGH. The child of `clone(CLONE_NEWPID)` **is** PID 1 of the new namespace.

The extra fork is required only for `unshare`/`setns`, and the kernel's reason is explicit —
`[F]` HIGH `setns(2)`: "Reassociating the calling thread with a PID namespace changes only the PID
namespace that subsequently created child processes of the caller will be placed in; it does not
change the PID namespace of the caller itself." Changing a live process's own `getpid()` would
break every program that cached it.

`[O]` DROPLET 6.11.0-29 — the failure mode, demonstrated. `unshare -p sh -c 'echo my_pid=$$;
readlink /proc/self/ns/pid; readlink /proc/self/ns/pid_for_children'`:
```
my_pid=2749866                 <- host-scale PID: sh is still in the host pidns
pid:[4026532570]               <- the forked `readlink` child landed in the NEW ns as its PID 1
sh: 1: Cannot fork             <- ...then exited, destroying the namespace
```
Confidence HIGH. This one command shows three things at once: `unshare(CLONE_NEWPID)` leaves the
caller behind; the first forked child becomes PID 1; and when that PID 1 exits the namespace
becomes permanently unusable.

The errno for that last part, isolated:

`[O]` DROPLET 6.11.0-29 — inside `unshare -p python3`, fork a child (it is PID 1), reap it, fork
again:
```
child1 getpid= 1
child1 reaped
second fork FAILED errno=12 (ENOMEM) Cannot allocate memory
```
Confidence HIGH. Matches `[F]` HIGH `pid_namespaces(7)`: "a subsequent `fork(2)` into this PID
namespace fail with the error `ENOMEM`; it is not possible to create a new process in a PID
namespace whose 'init' process has terminated."

**So why do runc and youki fork twice anyway?** Not for `CLONE_NEWPID`. `[C]` HIGH — the real
reasons, from runc's `libcontainer/nsenter/nsexec.c` and youki's
`container_intermediate_process.rs`:
1. **The user-namespace map handshake.** The process that unshares the userns cannot write its own
   `uid_map` (it has no privilege in the parent ns, and rootless needs multi-line maps written by
   a helper). Some *other* process must write `/proc/<pid>/uid_map` for it. So: parent stays
   privileged and outside; an intermediate unshares the userns and asks the parent to write its
   maps; only then does it create the real init.
2. **They use `unshare`, not `clone`, for the userns** — deliberately, and for the userns *alone*
   before the rest. runc's in-tree comment says unsharing everything in one call leaves namespace
   objects "owned incorrectly," specifically that the SELinux label of the internal kern-mount
   backing mqueue is wrong if the UTS namespace is cloned before the user namespace is mapped.
   Once you are on the `unshare` path for the userns, `CLONE_NEWPID` in the same process then
   *does* need the extra fork.
3. `CLONE_PARENT` on both clones, so the eventual init is reparented to the real runtime process
   and can be `wait()`ed directly rather than being adopted by host PID 1 when the intermediate
   exits.

`[R]` HIGH — **for a root-first MVP with no user namespace, one `clone3` is enough.** Parent →
`clone3(CLONE_NEWNS|NEWPID|NEWNET|NEWUTS|NEWIPC|NEWCGROUP|CLONE_PIDFD|CLONE_INTO_CGROUP)` → child
is PID 1, does its own mount/rootfs/caps setup, execs. Two processes, not three. Tradeoff: the
moment we add user namespaces (D5's rootless track) we must grow the intermediate process and the
map handshake, and that is a structural change to the process topology, not a flag. Design the
sync-pipe protocol now so an intermediate stage can be inserted later without reworking it.

### 3.4 Which flags are rejected multithreaded — measured

This is the single most important constraint for a Rust implementation, so I measured it per-flag
rather than trusting the docs. In-process `libc.unshare(flag)` from the main thread of a Python
process with 3 extra live threads (`Threads: 4` in `/proc/self/status`):

`[O]` DROPLET 6.11.0-29:

| flag | threads | result |
|---|---|---|
| `CLONE_NEWNS` | 4 | **OK** |
| `CLONE_NEWUSER` | 4 | **EINVAL** |
| `CLONE_NEWPID` | 4 | **OK** |
| `CLONE_NEWNET` | 4 | **OK** |
| `CLONE_NEWUTS` | 4 | **OK** |
| `CLONE_NEWTIME` | 4 | **OK** |
| `CLONE_NEWCGROUP` | 4 | **OK** |

Confidence HIGH. All seven succeed single-threaded too. **`CLONE_NEWUSER` is the only namespace
flag that a multithreaded process cannot unshare.** Matches `[F]` HIGH `unshare(2)`: "CLONE_NEWUSER
requires that the calling process is not threaded; specifying CLONE_NEWUSER automatically implies
CLONE_THREAD" + "EINVAL CLONE_THREAD, CLONE_SIGHAND, or CLONE_VM was specified in flags, and the
caller is multithreaded."

Same for joining: `[F]` HIGH `setns(2)` — "EINVAL The caller is multithreaded and tried to join a
new user namespace" and "EINVAL The caller shares filesystem (`CLONE_FS`) state ... and tried to
join a new user namespace."

**Why runc is written partly in C.** `[C]` HIGH — `libcontainer/nsenter/nsenter.go` declares
`void __attribute__((constructor)) init(void) { nsexec(); }`, so `nsexec()` runs from the ELF
`.init_array` **before the Go runtime starts any threads**. The Go runtime is unconditionally
multithreaded (sysmon plus Ms), so by the time Go's `main` runs, `unshare(CLONE_NEWUSER)` and
`setns(..., CLONE_NEWUSER)` are already permanently unavailable, and `unshare(CLONE_NEWNS)` from a
goroutine would move only whichever OS thread the goroutine happened to occupy. Even after
nsexec, runc pins itself: `runtime.GOMAXPROCS(1); runtime.LockOSThread()`.

`[R]` HIGH — **Rust's advantage here is real but conditional.** A plain `std` Rust binary is
single-threaded, so we need no C constructor and no re-exec trampoline. The advantage evaporates
the instant anything spawns a thread before the clone: a tokio runtime, `rayon`, jemalloc's
background threads, `tracing-appender`'s non-blocking writer, or any dependency's lazy
initialisation. Make it an enforced invariant, not a hope: assert
`/proc/self/status:Threads == 1` immediately before the container clone and abort loudly
otherwise. Cost: one file read. Benefit: converts a class of silent, environment-dependent
`EINVAL`s and post-fork deadlocks into one obvious startup failure.

### 3.5 Ordering constraints, consolidated

`[F]`/`[O]` HIGH unless noted:

1. **User namespace first.** Everything else then becomes creatable with `CAP_SYS_ADMIN` in the
   *new* userns rather than requiring real root. `[F]` HIGH `user_namespaces(7)`: "Starting in
   Linux 3.8, unprivileged processes can create user namespaces, and the other types of
   namespaces can be created with just the `CAP_SYS_ADMIN` capability in the caller's user
   namespace."
2. **Unshare the userns alone, then the rest.** `[C]` HIGH runc, for the mqueue/SELinux-label
   reason in §3.3.
3. **`uid_map`/`gid_map` before the child's `execve`**, else the child's capabilities are cleared
   (§1.6). And `setgroups`→`deny` before `gid_map`, never after.
4. **cgroup assignment before the container's init can fork.** Otherwise the first descendants
   escape the limits. `CLONE_INTO_CGROUP` or an immediate `cgroup.procs` write.
5. **cgroup namespace after cgroup placement** — the cgroup you are in at unshare time *becomes*
   the namespace root, so placing yourself afterwards would root the namespace in the wrong place.
   `[C]` HIGH youki's `apply_cgroups` comment states exactly this.
6. **Mount namespace before any container mount**, and `MS_REC|MS_SLAVE`-or-`PRIVATE` on `/`
   before the first mount (§4.2).
7. **All mounts before `pivot_root`'s old root is detached** — after `umount2(put_old,
   MNT_DETACH)` nothing outside the new rootfs is reachable by path (§5.4, measured).
8. **Network namespace created early, plumbed from the parent, configured before caps drop.**
   `[C]` HIGH runc calls `setupNetwork`/`setupRoute` before `prepareRootfs`.
9. **`PR_SET_NO_NEW_PRIVS` before `seccomp(SECCOMP_SET_MODE_FILTER)`** unless still holding
   `CAP_SYS_ADMIN`. `[F]` HIGH `seccomp(2)`: "either the calling thread must have the
   `CAP_SYS_ADMIN` capability in its user namespace, or the thread must already have the
   `no_new_privs` bit set ... Otherwise ... fails and returns `EACCES`."
10. **Capability bounding set dropped before the uid change; remaining sets applied after.**
    `PR_SET_KEEPCAPS` around the `setuid` so permitted caps survive it. `[C]` HIGH runc
    `finalizeNamespace`.
11. **seccomp last** — one syscall before `execve` when NNP is set, so the smallest possible
    number of syscalls must be allowed by the profile. `[C]` HIGH runc/crun/youki all branch on
    `noNewPrivileges` for exactly this.

### 3.6 Combinations the kernel rejects

`[F]` HIGH unless marked:
- `CLONE_NEWPID` + `CLONE_THREAD` — rejected (`pid_namespaces(7)`).
- `CLONE_NEWUSER` + multithreaded caller (`unshare`) — `EINVAL`. `[O]` verified §3.4.
- `CLONE_NEWUSER` + `CLONE_FS`-sharing caller (`setns`) — `EINVAL`.
- `setns` into your own current user namespace — `EINVAL`. `[O]` verified §1.6.
- Second `unshare(CLONE_NEWPID)` in the same process — `EINVAL`.
- `CLONE_NEWTIME` via legacy `clone(2)` — silently misread as `exit_signal`. `[O]` verified §1.8.
- `CLONE_NEWUSER` when the caller's own uid/gid are not mapped in its current userns — `EPERM`
  (`unshare(2)`: "CLONE_NEWUSER requires that the user ID and group ID of the calling process are
  mapped").
- `pivot_root` with a shared `/` — `EINVAL`. `[O]` verified §5.2.
- `mount -t proc` inside a userns without also owning a new PID namespace — `EPERM`.
  `[O]` verified §6.1.

---

## 4. Mount namespace mechanics

### 4.1 Propagation types

`[F]` HIGH `mount_namespaces(7)`, quoted:

- **`MS_SHARED`** — "This mount shares events with members of a peer group. `mount(2)` and
  `umount(2)` events immediately under this mount will propagate to the other mounts that are
  members of the peer group ... Conversely, `mount(2)` and `umount(2)` events that take place
  under peer mounts will propagate to this mount." Bidirectional.
- **`MS_PRIVATE`** — "This mount is private; it does not have a peer group. `mount(2)` and
  `umount(2)` events do not propagate into or out of this mount."
- **`MS_SLAVE`** — "`mount(2)` and `umount(2)` events propagate into this mount from a (master)
  shared peer group. `mount(2)` and `umount(2)` events under this mount do not propagate to any
  peer." One-way inbound.
- **`MS_UNBINDABLE`** — "like a private mount, and in addition this mount can't be bind mounted."
  Also: "When a recursive bind mount ... is performed on a directory subtree, any bind mounts
  within the subtree are automatically pruned."

Two subtleties that bite:
- `[F]` HIGH — "a mount's propagation type determines whether `mount(2)` and `umount(2)` of mounts
  **immediately under** the mount are propagated. Thus, the propagation type does not affect
  propagation of events for grandchildren." Hence `MS_REC` on the propagation change.
- `[F]` HIGH — members join a peer group when "the mount is replicated during the creation of a
  new mount namespace" or "a new bind mount is created from the mount." So a fresh mount namespace
  **inherits peer-group membership**, which is precisely why unsharing alone protects nothing.

### 4.2 Why runtimes change `/`'s propagation — measured leak

`[O]` DROPLET 6.11.0-29 — the host's root is shared, which is the default systemd sets:
```
findmnt -no TARGET,PROPAGATION /     ->  / shared
                             /tmp    ->  /tmp shared
                             /proc   ->  /proc shared
                             /sys    ->  /sys shared
                             /dev    ->  /dev shared
```
Confidence HIGH.

Then, fully sandboxed (outer namespace made `private` so the production host could not be
touched; a scratch subtree made deliberately `shared` inside it; an inner namespace created with
`--propagation unchanged`):
```
[outer] / propagation: shared
outer propagation:  /tmp/d1probe/shared  shared
                    /tmp/d1probe/private private
INNER leak mounts: 2
OUTER leaked mounts after inner exited:
  /tmp/d1probe/shared/sub
OUTER leaked count: 1
=== HOST leak check (MUST be 0) === 0
```
`[O]` DROPLET 6.11.0-29, confidence HIGH.

**What this proves:** a mount created inside a child mount namespace, on a mount point whose
propagation is `shared`, appears in the parent namespace **and survives the child namespace's
death**. The mount on the `private` mount point did not propagate. So without a propagation change:

- every container's `/proc`, `/dev`, `/dev/pts`, `/dev/shm`, `/sys`, and overlay rootfs mount
  lands in the host mount table;
- they stay there after the container exits — an unbounded leak, one growing `/proc/mounts` per
  container start, and eventually a host that is slow to enumerate mounts and impossible to
  reason about;
- `umount` events propagate too, so a container unmounting something can unmount it on the host.

**The flag runtimes actually use is `MS_REC|MS_SLAVE`, not `MS_REC|MS_PRIVATE`.** This corrects
the project spec's assumption. `[C]` HIGH — runc `libcontainer/rootfs_linux.go`:
```go
func prepareRoot(config *configs.Config) error {
    flag := unix.MS_SLAVE | unix.MS_REC              // rslave, NOT rprivate
    if config.RootPropagation != 0 { flag = config.RootPropagation }
    if err := mount("", "/", "", uintptr(flag), ""); err != nil { return err }
    if err := rootfsParentMountPrivate(config.Rootfs); err != nil { return err }
    return mount(config.Rootfs, config.Rootfs, "bind", unix.MS_BIND|unix.MS_REC, "")
}
```
So: `MS_REC|MS_SLAVE` on `/` (overridable by the OCI `linux.rootfsPropagation`), then
**non-recursive `MS_PRIVATE` on the rootfs's own mount** (walking up parents while `EINVAL`), then
a recursive self-bind of the rootfs so it is guaranteed to be a mount point for `pivot_root`.
youki's `rootfs.rs` matches (`MS_REC|MS_SLAVE` default). crun defaults to `MS_REC|MS_PRIVATE`
instead — the runtimes genuinely disagree here.

**Why slave rather than private:** slave still blocks outbound propagation (which is the leak we
care about) while *keeping* inbound propagation, so host mount changes — a newly attached volume,
a device-mapper teardown — still reach the container's view. runc's source cites races with
devicemapper where a fully private container held a stale reference to a mount the host thought
was gone. `[C]` MED on the specific devicemapper history (runc's own comment; I did not verify
the bug report).

`[R]` HIGH — **use `MS_REC|MS_SLAVE` on `/`, plus non-recursive `MS_PRIVATE` on the rootfs mount
itself, plus a recursive self-bind of the rootfs.** Honour `linux.rootfsPropagation` when we get
to OCI. Tradeoff versus `rprivate`: slave admits host→container propagation, which is a small
information/behaviour channel but is what every mainstream runtime ships and what volume
lifecycle management needs. Do not invent a third answer.

Trap worth recording, because it will mislead anyone experimenting at a shell:

`[O]` DROPLET 6.11.0-29 — `util-linux 2.40.2`'s `unshare -m` **silently defaults to
`--propagation private`**, i.e. it does `MS_REC|MS_PRIVATE` on `/` for you. My first
`pivot_root`-with-shared-`/` test therefore "passed" when it should have failed, and only
`unshare -m --propagation unchanged` (nested inside a protective private namespace) reproduced the
real kernel behaviour. Confidence HIGH. Anyone learning these mechanics with `unshare(1)` is
being shielded from the exact failure they need to see.

### 4.3 `MS_BIND`, `MS_REC`, and the remount dance

`[F]` HIGH `mount(2)`:
- `MS_BIND` attaches an existing subtree at a second location. Without `MS_REC` it binds the
  single mount only; with `MS_REC` it replicates the whole submount tree.
- **Mount options are not applied by a bind.** `mount(2)`: with `MS_BIND`, "the `mountflags` and
  `data` arguments are ignored" apart from a documented small set. To make a bind read-only or
  `nosuid` you need a second call with `MS_BIND|MS_REMOUNT` and the desired flags. `[C]` HIGH runc
  does exactly this two-pass bind.
- Since Linux 5.12, `mount_setattr(2)` sets attributes **recursively** in one call
  (`AT_RECURSIVE`), which the `MS_REMOUNT` trick cannot do — the classic bug being a recursive
  read-only bind where nested submounts silently stay writable. `[F]` HIGH.

`[R]` MED — MVP uses `mount(2)` + the two-pass bind because it works on every kernel including
ROCKY. Add `mount_setattr` when we need recursive-read-only correctness, gated on an `ENOSYS`
probe. Tradeoff: the two-pass path is subtly wrong for recursive read-only binds; that is
acceptable only until we support user-supplied `-v ...:ro` volumes.

### 4.4 `MS_NOSUID` / `MS_NODEV` / `MS_NOEXEC` — where they actually go

`[O]` DROPLET 6.11.0-29 — read from a live `redis:7-alpine` container's
`/proc/82271/mountinfo`. This is Docker's real, shipped mount set:

| target | fstype | flags observed |
|---|---|---|
| `/` | overlay | `rw,relatime` — **none of the three** |
| `/proc` | proc | `rw,nosuid,nodev,noexec,relatime` |
| `/dev` | tmpfs | `rw,nosuid` + `size=65536k,mode=755` — **no `nodev`, no `noexec`** |
| `/dev/pts` | devpts | `rw,nosuid,noexec,relatime,gid=5,mode=620,ptmxmode=666` |
| `/dev/shm` | tmpfs | `rw,nosuid,nodev,noexec,relatime,size=65536k` |
| `/dev/mqueue` | mqueue | `rw,nosuid,nodev,noexec,relatime` |
| `/sys` | sysfs | `ro,nosuid,nodev,noexec,relatime` |
| `/sys/fs/cgroup` | cgroup2 | `ro,nosuid,nodev,noexec,relatime` (bind of the container's own subtree) |

Confidence HIGH. The two instructive asymmetries:
- `/dev` **must not** be `nodev` — that is where the device nodes live; `nodev` would make them
  inert. It is also not `noexec`, which is a real (small) attack surface Docker accepts.
- `/` is **not** `nosuid`. Setuid binaries in the image work. That is deliberate — images ship
  working `sudo`, `ping`, `passwd`. `--security-opt no-new-privileges` is the opt-in brake.

`[R]` HIGH — copy this table verbatim for the MVP. It is the OCI default mount set and matching it
is free compatibility. `[R]` MED — consider `nosuid` on `/` as a non-default hardening flag; it
breaks a visible minority of images, so it cannot be the default.

### 4.5 The new mount API — worth using?

`[F]` HIGH. `fsopen(2)`/`fsconfig(2)`/`fsmount(2)`/`move_mount(2)`/`open_tree(2)`/`fspick(2)`
(Linux 5.2) plus `mount_setattr(2)` (5.12) decompose `mount(2)` into:
1. `fsopen(fstype)` → a filesystem-configuration fd;
2. `fsconfig(fd, FSCONFIG_SET_STRING|SET_FLAG, key, value, 0)` — one option at a time, **with
   per-option error reporting** instead of one opaque `EINVAL` for a whole option string;
3. `fsconfig(fd, FSCONFIG_CMD_CREATE, ...)` → instantiate the superblock;
4. `fsmount(fd, flags, attr)` → an fd for a **detached mount** that exists but is attached
   nowhere;
5. `move_mount(fd, "", target_dirfd, name, MOVE_MOUNT_F_EMPTY_PATH)` → attach it.

`open_tree(dirfd, path, OPEN_TREE_CLONE|AT_RECURSIVE)` produces a detached clone of an existing
subtree as an fd, which is the fd-passing primitive.

Real advantages for a container runtime, not just aesthetics:
- **A mount can be fully constructed and configured before it is visible anywhere.** No window in
  which a half-configured mount is reachable.
- **Everything is fd-based**, so the whole class of symlink-exchange / TOCTOU attacks on mount
  target paths inside an attacker-controlled rootfs disappears. This is the family that produced
  CVE-2019-14271-style and runc `/proc` masking bugs.
- Mount fds can be **passed over a UNIX socket with `SCM_RIGHTS`**, so a privileged parent can
  build mounts and hand them to an unprivileged child. `[C]` HIGH — crun does this pervasively
  (`prepare_and_send_mounts` / `receive_mounts`); runc has a narrower on-demand version
  (`procMountPlease` / `procMountFd`).
- crun goes further: `open_tree(rootfs_fd, "", OPEN_TREE_NAMESPACE|AT_RECURSIVE)` then
  `setns(tree_fd, CLONE_NEWNS)` builds the container's entire mount namespace out-of-band and
  enters it — **no `unshare(CLONE_NEWNS)` and no `pivot_root` at all**. `[C]` HIGH.
  `OPEN_TREE_NAMESPACE` is very new; treat it as runtime-detected only. `[F]` LOW on its exact
  first kernel version — I did not read the upstream commit. What would settle it: `git log`
  on `fs/namespace.c` for the `OPEN_TREE_NAMESPACE` introduction, or the flag's appearance in
  `open_tree(2)`.

`[O]` DROPLET 6.11.0-29 — full round trip verified working (§2.1). `[O]` ROCKY 4.18.0-425 —
`fsopen`/`fsmount`/`move_mount`/`open_tree` all **present** (backported), `mount_setattr`
`ENOSYS`.

`[R]` MED — **MVP: `mount(2)`. Design for the new API; do not adopt it yet.** Reasoning: the
learning goal is the classic mechanism, `mount(2)` is one call versus five, and every kernel has
it. But structure the mount layer around *(source, target, fstype, flags, options)* descriptors
resolved against **directory fds rather than strings**, so switching the backend later is a
backend swap, not a rewrite. Tradeoff stated plainly: staying on `mount(2)` keeps the
symlink-TOCTOU class alive, which is fine for a single-user dev tool and not fine the day we
accept untrusted images. Revisit before any multi-tenant use.

---

## 5. `pivot_root` versus `chroot`

### 5.1 The preconditions, verbatim

`[F]` HIGH `pivot_root(2)`, quoted in full because every clause is load-bearing:

> `pivot_root()` changes the root mount in the mount namespace of the calling process. More
> precisely, it moves the root mount to the directory `put_old` and makes `new_root` the new root
> mount. The calling process must have the `CAP_SYS_ADMIN` capability in the user namespace that
> owns the caller's mount namespace.
>
> `pivot_root()` changes the root directory and the current working directory of each process or
> thread in the same mount namespace to `new_root` if they point to the old root directory. On the
> other hand, **`pivot_root()` does not change the caller's current working directory** (unless it
> is on the old root directory), and thus it should be followed by a `chdir("/")` call.
>
> The following restrictions apply:
> - `new_root` and `put_old` must be directories.
> - `new_root` and `put_old` must not be on the same mount as the current root.
> - `put_old` must be at or underneath `new_root`.
> - `new_root` must be a path to a mount point, but can't be `"/"`. A path that is not already a
>   mount point can be converted into one by bind mounting the path onto itself.
> - **The propagation type of the parent mount of `new_root` and the parent mount of the current
>   root directory must not be `MS_SHARED`;** similarly, if `put_old` is an existing mount point,
>   its propagation type must not be `MS_SHARED`. These restrictions ensure that `pivot_root()`
>   never propagates any changes to another mount namespace.
> - The current root directory must be a mount point.

### 5.2 All five preconditions, measured

`[O]` DROPLET 6.11.0-29, via `libc.syscall(155, ...)`, run inside a nested sandbox: an outer
mount namespace made `private` (so the production host was unreachable), with `/` inside it
deliberately re-marked `shared` via `mount --make-rshared /`, then an inner namespace created with
`--propagation unchanged`.

```
root propagation in THIS ns: ['shared:513']

C1: new_root is not a mount point            -> FAIL EINVAL
C2: bind new_root onto itself                -> OK
    new_root propagation now: ['shared:558']
C3: pivot_root while / is SHARED             -> FAIL EINVAL
C4: put_old NOT under new_root (/tmp)        -> FAIL EINVAL
C5: new_root == current root (/)             -> FAIL EINVAL
C6: make / rprivate                          -> OK
    / propagation now: ['private']  new_root: ['private']
C7: pivot_root now that / is private         -> OK
    cwd after pivot_root: /oldroot/root
    new / listing: ['oldroot', 'put']
HOST verification: 0 leaked mounts, / still shared
```
Confidence HIGH. Every documented precondition reproduces exactly, including the one most people
never hit because `unshare(1)` hides it (C3).

Note **C7's `cwd after pivot_root: /oldroot/root`** — the caller's cwd was *not* moved; it stayed
on the old root, which is now reachable under `put_old`. This is the man page's "should be followed
by a `chdir("/")`", observed. Forgetting it leaves your process with a working directory in the
old root, which is both a functional bug and an escape vector.

### 5.3 The runc-style sequence

`[C]` HIGH — runc `rootfs_linux.go`, matched almost line-for-line by youki's
`syscall/linux.rs::pivot_rootfs`:

```go
oldroot, _ := open("/", O_DIRECTORY|O_RDONLY|O_PATH)   // fd on the old root, held
fchdir(rootfd)                                          // cwd = new root
pivot_root(".", ".")                                    // both args "."
fchdir(oldroot)                                         // back to the old root via fd
mount("", ".", "", MS_SLAVE|MS_REC, "")                 // old root: rslave
umount2(".", MNT_DETACH)                                // detach it
chdir("/")                                              // land in the new root
```

Three non-obvious things:
- **`pivot_root(".", ".")` with both arguments the same** is legal and idiomatic. The old root ends
  up *stacked on top of* the new root: `/` is the new root while the caller's cwd still refers to
  the old one. This avoids having to create a `put_old` directory inside the container image —
  otherwise every image would need a `/.pivot_root` to exist. `[C]` HIGH (runc credits LXC).
- The `O_PATH` fd on the old root is opened **before** the pivot precisely so the process can
  `fchdir` back to a root that no longer has any pathname.
- The old root gets **`MS_SLAVE|MS_REC`, not `MS_PRIVATE`** — same rationale as §4.2. crun uses
  `MS_REC|MS_PRIVATE` here and loops `umount2(".", MNT_DETACH)` until `EINVAL`. `MNT_DETACH` (lazy
  unmount) is mandatory rather than optional: you are unmounting your own current directory, and a
  synchronous `umount` would fail `EBUSY`.

### 5.4 What happens to the old root — measured, and the ordering lesson

`[O]` DROPLET 6.11.0-29 — after a successful `pivot_root` into a scratch rootfs, `/oldroot`
contained the entire host filesystem (`os.path.exists("/oldroot/etc/hostname")` → `True`). After
`umount2("/oldroot", MNT_DETACH)`: `os.listdir("/oldroot")` → `[]`, and the interpreter then
**died** trying to open `/proc/self/mountinfo`, followed by a cascading
`ModuleNotFoundError: No module named 're'` from the traceback handler. Confidence HIGH.

That crash is the most useful thing in this whole section. The Python binary and its already-loaded
code kept running from open fds and mapped pages, but **every new path lookup failed**, because the
interpreter and its stdlib lived in the old root. Two rules follow:

`[R]` HIGH — **every mount the container needs must exist inside the new rootfs before the old root
is detached**, and the process must `execve` a binary *from within the new rootfs*. Anything the
child still needs by path — `/proc`, the interpreter, shared libraries, the seccomp profile file —
is gone the instant `MNT_DETACH` lands. If you need a host resource after that point, hold it as an
**fd** across the pivot, never as a path. `[C]` HIGH — this is exactly why crun keeps a private
`fsopen("proc")` handle alive across the pivot: so `/proc/self/fd/...` tricks keep working after
the old root is gone and before the new `/proc` is mounted.

`[R]` HIGH — detach the old root, do not merely leave it mounted. An accessible `put_old` is a
full, trivial escape: `chdir("/put_old")` and you are on the host filesystem.

### 5.5 Why `chroot` is escapable — demonstrated

`[F]` HIGH `chroot(2)`: "This call changes an ingredient in the pathname resolution process and
does nothing else. ... This call does not change the current working directory, so that after the
call '.' can be outside the tree rooted at '/'. ... **This call does not close open file
descriptors, and such file descriptors may allow access to files outside the chroot tree.**"

`[O]` DROPLET 6.11.0-29 — the classic escape, executed against the live production host inside a
private mount namespace:
```
inside chroot, / listing: []
can we see host /etc/passwd? False
ESCAPED. real / listing: ['bin','boot','dev','etc','home','lib','lib64','lost+found','media','mnt','opt','proc','root','run']
host /etc/hostname readable: AIKYNETIX-PROD-NY
```
Confidence HIGH. The sequence is five calls: `open("/outside", O_DIRECTORY)` **before** the
chroot; `chroot("/jail")`; `fchdir(fd)` — cwd is now outside the root; `chdir("..")` ×64 — the
kernel's `..` handling stops at the *real* root, not the chroot root, once cwd is already outside;
`chroot(".")`. Requires `CAP_SYS_CHROOT`, which container root has, and Docker's default bounding
set includes `cap_sys_chroot` (§7.2).

So `chroot` fails for containers on two counts: it does not move the mount-namespace root (so the
old tree is still mounted and reachable), and it leaves `..` traversal from an outside cwd
unconstrained. `pivot_root` + `MNT_DETACH` fixes both by making the old root have **no pathname at
all**.

### 5.6 Is `MS_MOVE` + `chroot` ever legitimate?

`[C]` MED — yes, as a narrow fallback, and runc implements it as `msMoveRoot` behind
`--no-pivot`. The sequence:
1. Find every *full* (`mnt_root == "/"`) `proc` and `sysfs` mount outside the target rootfs and
   `MS_SLAVE|MS_REC` + `MNT_DETACH` each — or, if unprivileged, cover each with a tmpfs.
2. `mount(rootfs, "/", MS_MOVE)`.
3. `chroot(".")`, `chdir("/")`.

Step 1 is the whole point and is easy to omit: under `chroot` the host procfs remains reachable,
and a reachable host procfs lets the container mount a fresh procfs and re-derive host paths.

The real motivation is initramfs. `[F]` HIGH `pivot_root(2)` NOTES — `pivot_root` cannot be used
on an initramfs (`rootfs`, a `ramfs`, cannot be moved), so early boot uses
`switch_root`-style `MS_MOVE`+`chroot`. Some sandboxed/nested environments hit the same
restriction.

`[R]` HIGH — **MVP: `pivot_root` only. Do not implement the fallback.** Reasoning: our stated dev
target is a destroyable VM with a modern kernel, `pivot_root` works there, and the fallback's
masking step is the kind of security-relevant code that is worse to have half-right than to not
have. Add it only if a real target platform rejects `pivot_root`. If we ever do: the masking in
step 1 is not optional.

---

## 6. Minimum viable rootfs mount set

Measured by building a scratch rootfs from read-only binds of the host's `/bin`, `/sbin`, `/usr`,
`/lib`, `/lib64`, doing a real `pivot_root` into it, and then observing what fails.

### 6.1 `/proc` — **REQUIRED**

`[F]` HIGH `pid_namespaces(7)`: "a new procfs instance can be mounted directly over `/proc`. From a
shell, the command to mount `/proc` is: `$ mount -t proc proc /proc`". Mount: `proc` on `/proc`,
flags `nosuid,nodev,noexec`.

`[O]` DROPLET 6.11.0-29 — inside a pivoted rootfs with **no** `/proc`:
```
[ls /proc]             : (empty dir)
[ps aux]               : Error, do this: mount -t proc proc /proc
[top -bn1]             : Error, do this: mount -t proc proc /proc
[free -m]              : free: Memory information file /proc/meminfo does not exist
[mount]                : mount: failed to read mtab: No such file or directory
[readlink /proc/self]  : FAILS
[df -h /]              : works (statfs) but device column shows "-"
[id]                   : works
[python3 -c pass]      : works
[python3 os.urandom(8)]: works  (getrandom(2), not /dev/urandom)
[openssl rand -hex 4]  : works
```
After `mount -t proc proc /proc`: `ps aux` → 164 lines, `readlink /proc/self` → a pid, `free` →
real numbers. Confidence HIGH.

So concretely: **every process-listing and memory-reporting tool breaks** (`ps`, `top`, `free`,
`mount`, `pgrep`, `killall`, anything using libprocps), plus any runtime that reads `/proc/self/*`
— the JVM (`/proc/self/maps`, cgroup-aware heap sizing), the Go runtime's `GOMAXPROCS` detection,
glibc's `sysconf(_SC_NPROCESSORS_ONLN)` fast path, systemd, and any language's
`os.cpu_count()`-equivalent. Notably `getrandom(2)` means crypto does **not** need `/dev/urandom`
on a modern kernel.

**Hard constraint — mounting procfs in a user namespace requires a new PID namespace.**
`[F]` HIGH `user_namespaces(7)`: "Holding `CAP_SYS_ADMIN` within the user namespace that owns a
process's PID namespace allows (since Linux 3.8) that process to mount `/proc` filesystems."

`[O]` ROCKY 4.18.0-425, unprivileged:
```
A: unshare -U -m       --map-root-user  -> mount -t proc: permission denied
B: unshare -U -m -p -f --map-root-user  -> mount proc: OK, visible pids: 4
C: same, over the real /proc            -> OK, visible pids: 4, self pid: 1
```
Confidence HIGH. Case A holds `CapEff: 000001ffffffffff` and still cannot mount procfs, because the
PID namespace it is in is owned by the *initial* user namespace.

### 6.2 `/dev` — **REQUIRED**, and `mknod` vs bind is the key decision

Convention: `tmpfs` on `/dev`, `nosuid,mode=755,size=65536k`, populated with the standard nodes.
`[O]` DROPLET matches (§4.4). `[C]` HIGH — runc's default is
`tmpfs nosuid,strictatime,mode=755,size=65536k`.

Standard nodes `[C]` HIGH (runc/OCI default): `/dev/null` (1,3), `/dev/zero` (1,5),
`/dev/full` (1,7), `/dev/random` (1,8), `/dev/urandom` (1,9), `/dev/tty` (5,0). Plus symlinks
`/dev/fd → /proc/self/fd`, `/dev/stdin|stdout|stderr → /proc/self/fd/{0,1,2}`, and `/dev/ptmx →
pts/ptmx`. Note the symlinks depend on `/proc` being mounted — another ordering coupling.

**The failure mode without `/dev/null` is the nastiest one in this document.**

`[O]` DROPLET 6.11.0-29 — in a pivoted rootfs whose `/dev` was an empty directory:
```
[echo x > /dev/null] : OK-wait-really
[ls /dev]            : null
```
Confidence HIGH. `echo x > /dev/null` **succeeded** — by creating a regular file named `null`. It
does not error; it silently accumulates. Every `2>/dev/null` in every script in the image becomes
a disk-filling append. This fails silently at build time and shows up as a full filesystem weeks
later.

**`mknod` vs bind-mounting host nodes:**
- **Root:** `mknod(2)` works. `[C]` HIGH — runc's `createDevices`.
- **Rootless:** `mknod` is impossible. `[F]` HIGH `user_namespaces(7)` — `CAP_MKNOD` is one of the
  capabilities that "only a process with privileges in the initial user namespace can" use.
  `[O]` ROCKY 4.18.0-425, inside `unshare -U -m --map-root-user` holding
  `CapEff: 000001ffffffffff`: `mknod /tmp/.../nul c 1 3` → **`Operation not permitted`**.
  Confidence HIGH. The fallback is a read-only **bind mount of each host device node**, which is
  what runc's `bind_devices` path and youki's `bind_devices == userns_present` do.
- `[R]` HIGH — implement `mknod` for the root-first MVP; structure the device setup as a list of
  `(path, type, major, minor, mode)` descriptors with two backends (`mknod` / bind), so D5's
  rootless track is a backend swap. Tradeoff: bind-mounting is strictly more mounts and more
  mountinfo noise, but it is the only option without initial-userns privilege.

### 6.3 `/dev/pts` (devpts) — **REQUIRED for any interactive container**

Convention: `devpts` on `/dev/pts`, `nosuid,noexec,gid=5,mode=620,ptmxmode=666`. `[O]` DROPLET
matches exactly.

`gid=5` is the `tty` group — `mode=620` plus `gid=5` is what makes a freshly allocated pty slave
`crw--w----  root tty`, which is what `write`/`wall` permission checks expect. `ptmxmode=666`
makes the instance's own `ptmx` node openable by unprivileged users. `[F]` HIGH `pts(4)`.

**`newinstance` is dead — do not pass it.** `[O]` DROPLET 6.11.0-29:
```
mount -t devpts -o newinstance,ptmxmode=0666,gid=5,mode=620 devpts /tmp/d1pts   -> OK
findmnt -no OPTIONS  ->  rw,relatime,gid=5,mode=620,ptmxmode=666
                                    ^^ "newinstance" absent from the resulting options
mount -t devpts -o gid=5,mode=620,ptmxmode=666 devpts /tmp/d1pts                -> OK
findmnt -no OPTIONS  ->  rw,relatime,gid=5,mode=620,ptmxmode=666   (identical)
```
Confidence HIGH. The option is accepted and silently ignored: modern kernels always create a new
devpts instance per mount. `[F]` MED — this follows the removal of
`CONFIG_DEVPTS_MULTIPLE_INSTANCES` (Linux 4.7 era), after which multi-instance became
unconditional. I did not read the commit; what would settle it is `git log --oneline
fs/devpts/inode.c` around v4.7. Note `[C]` runc **still** passes `newinstance` in its default
spec — harmless, and a good example of convention outliving its cause.

`[O]` DROPLET 6.11.0-29 — with `/dev` replaced by a tmpfs containing only `null`, i.e. **no**
`/dev/pts`: `pty.openpty()` → `OSError: out of pty devices`. Confidence HIGH. So: no `docker
run -it`, no `docker exec -it`, no `script`, no `expect`, no `ssh` server, no terminal-based
installer, no `sudo` password prompt.

`[R]` HIGH — required as soon as we support `-it`, which for `mydocker run alpine /bin/sh` is
immediately. Not optional in practice.

### 6.4 `/dev/shm` — **STRONGLY RECOMMENDED**

Convention: `tmpfs`, `nosuid,nodev,noexec,mode=1777,size=65536k`. `[O]` DROPLET shows exactly
`rw,nosuid,nodev,noexec,relatime,size=65536k` — Docker's well-known 64 MB default, the one people
override with `--shm-size` when Chrome or PostgreSQL falls over.

`[O]` DROPLET 6.11.0-29 — with no `/dev/shm` present:
```
[shm_open]              : FAIL: FileNotFoundError [Errno 2] No such file or directory: '/psm_5dbc7e04'
[multiprocessing.Queue] : FAIL: FileNotFoundError [Errno 2]
[mp.Lock / sem_open]    : FAIL: FileNotFoundError [Errno 2]
```
Confidence HIGH. Concretely broken: POSIX shared memory (`shm_open(3)`), POSIX **named
semaphores** (`sem_open(3)` — glibc backs these with `/dev/shm/sem.*` files), Python
`multiprocessing` in its entirety, PostgreSQL, Chromium/Electron, PyTorch DataLoader workers, and
anything using `boost::interprocess`.

Note this is a **mount** namespace concern, not the IPC namespace's — §1.5.

### 6.5 `/dev/mqueue` — **CONVENTION**

Convention: `mqueue`, `nosuid,nodev,noexec`. Needed only for POSIX message queues
(`mq_open(3)`), which almost nothing uses. `[O]` DROPLET has it, because runc's default spec has
it. Nothing in a normal image breaks without it.

`[O]` ROCKY 4.18.0-425 — mounting it in a user namespace requires a **new IPC namespace**:
```
unshare -U -m -i --map-root-user  -> mqueue (with new ipcns): OK
unshare -U -m    --map-root-user  -> permission denied
```
Confidence HIGH. Same ownership pattern as procfs/PID.

### 6.6 `/sys` — **STRONGLY RECOMMENDED**, read-only

Convention: `sysfs`, `ro,nosuid,nodev,noexec`. `[O]` DROPLET matches.

Breaks without it: `ip link` mostly works (netlink, not sysfs) but interface *attributes* do not;
`lsblk`, `lscpu`, `lspci`, `udev`, D-Bus/systemd, block-device discovery, and hwmon all fail.
Cgroup-aware runtimes that probe `/sys/fs/cgroup` also need it mounted first.

`[O]` ROCKY 4.18.0-425 — mounting sysfs in a user namespace requires a **new network namespace**:
```
unshare -U -m -n --map-root-user  -> sysfs (with new netns): OK
unshare -U -m    --map-root-user  -> permission denied
```
Confidence HIGH. This one surprised me and is not obvious from `user_namespaces(7)`'s bullet list
— sysfs exposes the netns's network devices, so the kernel requires the netns be owned by the
mounting userns. Consequence: **a rootless container cannot mount a fresh sysfs while sharing the
host network namespace**; runc's `ToRootless()` handles it by rewriting `/sys` to a read-only
recursive bind of the host `/sys`. `[C]` HIGH.

### 6.7 `/sys/fs/cgroup` — **CONVENTION** (bordering on required for modern images)

`[O]` DROPLET 6.11.0-29 — Docker's actual mount is **not** a fresh cgroup2 mount:
```
499 498 0:29 /system.slice/docker-db69f93....scope /sys/fs/cgroup \
    ro,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw,nsdelegate,memory_recursiveprot
```
Confidence HIGH. Device `0:29` and a non-`/` root field mean this is a **bind of the host cgroup2
subtree at the container's own cgroup path**, mounted read-only. The cgroup namespace (§1.7) then
makes that path *appear* as `/` inside.

Breaks without it: cgroup-aware runtimes mis-size themselves — the JVM's
`UseContainerSupport`/`MaxRAMPercentage`, .NET's GC heap limits, Node's
`--max-old-space-size` heuristics, `nproc`, systemd-in-container. They fall back to host totals
and get OOM-killed.

`[R]` MED — MVP can ship without it and containers will run; add it before anyone runs a JVM.
Read-only, always: a writable cgroupfs plus `CAP_SYS_ADMIN` is a container escape.

### 6.8 `/proc` and `/sys` masking — SECURITY HARDENING

`[O]` DROPLET 6.11.0-29 — the live container also carries, and this is pure hardening:
- read-only re-binds of proc subtrees: `/proc/bus`, `/proc/fs`, `/proc/irq`, `/proc/sys`,
  `/proc/sysrq-trigger` (each `ro,nosuid,nodev,noexec`);
- empty read-only **tmpfs** over `/proc/acpi`, `/proc/scsi`, `/sys/firmware` (hide the whole
  directory);
- binds of **`/dev/null`** over `/proc/kcore`, `/proc/keys`, `/proc/latency_stats`,
  `/proc/timer_list`, `/proc/interrupts` (device `0:77` = the container's `/dev` tmpfs, path
  `/null`).

Confidence HIGH. Two distinct masking techniques: tmpfs-over-directory, `/dev/null`-over-file.
`/proc/kcore` is the important one — it is a readable image of physical memory.

`[R]` MED — `maskedPaths`/`readonlyPaths` are OCI spec fields and cheap to implement once the
mount layer exists. Not MVP-blocking; must exist before untrusted images. Note the ordering
dependency: masking `/proc/*` requires `/proc` mounted first, and `/dev/null`-binds require `/dev`
populated first.

### 6.9 Summary table

| Mount | fstype | Tier | Concrete breakage without it |
|---|---|---|---|
| `/` (rootfs, self-bound) | image fs | **required** | `pivot_root` fails `EINVAL` |
| `/proc` | proc | **required** | `ps`/`top`/`free`/`mount` fail; JVM, Go, systemd misbehave; `/proc/self/*` gone |
| `/dev` (tmpfs) | tmpfs | **required** | `/dev/null` writes silently create a growing regular file |
| `/dev/null,zero,full,random,urandom,tty` | nodes | **required** | as above; `/dev/tty` needed by `sudo`, `ssh`, `gpg` |
| `/dev/pts` | devpts | **required in practice** | `openpty()` → "out of pty devices"; no `-it`, no `exec -it` |
| `/dev/shm` | tmpfs | strongly recommended | `shm_open`, `sem_open`, Python multiprocessing, PostgreSQL, Chromium |
| `/sys` (ro) | sysfs | strongly recommended | `lsblk`/`lscpu`/`lspci`/udev/D-Bus fail |
| `/sys/fs/cgroup` (ro bind) | cgroup2 | convention | JVM/.NET/Node mis-size heaps against host RAM |
| `/dev/mqueue` | mqueue | convention | only POSIX mqueues |
| `/dev/fd`, `/dev/std*` symlinks | symlink | convention | some scripts; needs `/proc` first |
| proc/sys masking | bind/tmpfs | hardening | `/proc/kcore` = physical memory readable |

---

## 7. Container init / PID 1 kernel semantics

### 7.1 Signal disposition — the asymmetry that decides our kill path

`[F]` HIGH `pid_namespaces(7)`, quoted:

> Only signals for which the "init" process has established a signal handler can be sent to the
> "init" process by other members of the PID namespace. This restriction applies even to
> privileged processes, and prevents other members of the PID namespace from accidentally killing
> the "init" process.
>
> Likewise, a process in an ancestor namespace can — subject to the usual permission checks — send
> signals to the "init" process of a child PID namespace only if the "init" process has
> established a handler for that signal.
>
> `SIGKILL` or `SIGSTOP` are treated exceptionally: these signals are forcibly delivered when sent
> **from an ancestor PID namespace**. Neither of these signals can be caught by the "init"
> process.

Read the emphasis: the `SIGKILL`/`SIGSTOP` exception is scoped to *ancestor* senders. Measured
from both directions:

**(a) From inside the namespace — even `SIGKILL`, even as root, is discarded.**

`[O]` DROPLET 6.11.0-29 — PID 1 forks a child; the child (uid 0) calls `kill(1, SIGKILL)`:
```
pid1 running, waiting
  child(pid 2) uid 0 sending SIGKILL to pid 1
  kill(1,SIGKILL) returned success
  child: pid1 still alive? ppid= 1
PID1 SURVIVED SIGKILL from an in-namespace child
  rc=0
```
Confidence HIGH. Also, PID 1 signalling *itself*: `unshare -pf /bin/sh -c 'kill -KILL $$; echo
survived'` printed `survived`, rc=0. And `kill -STOP $$` → `resumed-somehow`, rc=0. And
`kill -SEGV $$` → rc **0**, not 139.

**(b) From an ancestor namespace — `SIGTERM` discarded, `SIGKILL` delivered.**

`[O]` DROPLET 6.11.0-29 — `unshare -pf sleep 60` backgrounded, then from the host shell:
```
sent SIGTERM rc=0
RESULT: sleep(PID1) SURVIVED SIGTERM from ancestor namespace
RESULT: SIGKILL from ancestor ns DID kill PID1        (wrapper rc=137)
```
Confidence HIGH.

**(c) A *forced* signal — a genuine hardware fault — kills PID 1 regardless.**

`[O]` DROPLET 6.11.0-29 — PID 1 performs a real null dereference (`ctypes.string_at(0)`):
```
pid 1 -> genuine null deref now
  rc=139   (128+11: forced SIGSEGV DID kill pid1)
```
Confidence HIGH. Contrast with `kill -SEGV $$` above, which was discarded. The distinction is
*generated* (via `kill(2)`) versus *forced* (synchronous fault / kernel-originated): the kernel's
"ignore for namespace init" check is bypassed for forced signals. `[F]` MED on the mechanism
naming — this matches `sig_task_ignored()`'s `SIGNAL_UNKILLABLE` + `force` logic in
`kernel/signal.c`, but I inferred that from behaviour rather than reading the source. What would
settle it: read `sig_task_ignored()` and the `force` argument's derivation in
`__send_signal_locked()`.

**Consequences for our engine — this is the most actionable finding in the document:**

`[R]` HIGH — **`mydocker stop` must always come from the host namespace, and must always have a
`SIGKILL` fallback on a timeout.** A `SIGTERM` to a container PID 1 that has not installed a
handler is silently dropped; the syscall returns 0 and nothing happens. This is precisely why
`docker stop` is documented as "SIGTERM, then SIGKILL after 10 seconds," and why stopping a
container whose entrypoint is `sleep`, or a shell script that never traps, always takes the full
grace period. Do not interpret a successful `kill(2)`/`pidfd_send_signal(2)` as delivery.

`[R]` HIGH — **the engine must never rely on in-container self-termination.** A container cannot
kill its own PID 1 even as root. Conversely this is a small security *benefit*: a compromised
process inside cannot terminate the namespace's init to escape supervision.

`[R]` MED — this asymmetry is a strong argument for a real init as PID 1 (§7.5): an init that
installs handlers for the full signal set makes `SIGTERM`, `SIGHUP`, `SIGUSR1` etc. actually
deliverable, restoring the semantics users expect.

### 7.2 Default capability set of container PID 1 — measured

`[O]` DROPLET 6.11.0-29 — `/proc/82271/status` of the live `redis:7-alpine` container, plus
`capsh --decode`:
```
Uid: 999  Gid: 1000  Groups: 1000
CapInh: 0000000000000000     CapPrm: 0000000000000000     CapEff: 0000000000000000
CapBnd: 00000000a80425fb     CapAmb: 0000000000000000
NoNewPrivs: 0                Seccomp: 2 (SECCOMP_MODE_FILTER)   Seccomp_filters: 1
```
`CapBnd = 0xa80425fb` decodes to Docker's 14: `cap_chown, cap_dac_override, cap_fowner,
cap_fsetid, cap_kill, cap_setgid, cap_setuid, cap_setpcap, cap_net_bind_service, cap_net_raw,
cap_sys_chroot, cap_mknod, cap_audit_write, cap_setfcap`. Host root's `CapEff` is
`0x1ffffffffff` (41), so Docker drops 27. Confidence HIGH.

Two notable observations: **`NoNewPrivs: 0`** — Docker does *not* set no-new-privs by default (it
is `--security-opt no-new-privileges`), yet a seccomp filter is loaded, which means runc installed
it while still holding `CAP_SYS_ADMIN` (the `!NoNewPrivileges` branch, §3.5 rule 9). And this
container's effective/permitted sets are empty because its entrypoint runs as uid 999 — the
bounding set is what actually constrains it. Capability *policy* is D5's; recorded here because it
is measured PID-1 state.

### 7.3 Orphan reparenting

`[F]` HIGH `pid_namespaces(7)`: "When a child process becomes orphaned, it is reparented to the
'init' process in the PID namespace of its parent (unless one of the nearer ancestors of the
parent employed `prctl(2)` `PR_SET_CHILD_SUBREAPER`...)."

`[O]` DROPLET 6.11.0-29 — PID 1 forks a middle process which forks a grandchild and exits
immediately:
```
pid1 = 1
  middle pid 2 exiting immediately (orphaning grandchild)
  grandchild ppid after parent died = 1
  pid1 reaped 3 status 0
  pid1: no more children (all reaped)
```
Confidence HIGH. Reparenting lands on the **namespace's** PID 1, not host PID 1 — this is what
keeps a container's process tree contained.

### 7.4 Zombie reaping, PID 1 exit, exit-status propagation

**Reaping is PID 1's obligation.** Orphans reparent to it (§7.3); if it never `wait()`s, they
accumulate as zombies holding PID-table slots until the namespace's pid limit is hit. A shell or
application as PID 1 typically only reaps *its own* children, so anything reparented to it leaks.
This is the entire reason `tini`/`dumb-init` exist.

**PID 1 exit terminates the whole namespace.** `[F]` HIGH `pid_namespaces(7)`: "If the 'init'
process of a PID namespace terminates, the kernel terminates all of the processes in the namespace
via a `SIGKILL` signal."

`[O]` DROPLET 6.11.0-29 — PID 1 forks a child that writes a marker file, sleeps 25 s, then appends
`SURVIVED_FULL_SLEEP`; PID 1 exits after 0.4 s with code 7:
```
pid1 exiting now
  unshare rc=7 elapsed=1s
  marker file: [start]        <- the append never happened
```
Confidence HIGH. The command returned in 1 s, not 25 s, and the marker never got its second write:
the child was `SIGKILL`ed. Nothing survives its namespace's init.

`[R]` HIGH — this is a **useful cleanup guarantee**, and we should lean on it rather than fight it.
Killing container PID 1 from the host is a complete, kernel-enforced teardown of the process tree.
No `cgroup.kill` sweep is needed for correctness (still do it for defence in depth, since the
cgroup can outlive in edge cases and D5 owns that).

**Exit status propagates normally through `waitpid`.**

`[O]` DROPLET 6.11.0-29:
```
unshare -pf --mount-proc /bin/sh -c 'exit 42'   -> observed exit=42
clone3(CLONE_PIDFD|CLONE_NEWPID) child _exit(3) -> parent waitpid status=768, >>8 == 3
pidfd_send_signal(pidfd, SIGKILL)               -> waitpid status=0x9, signalled, signo=9
```
Confidence HIGH. Standard encoding — exit code in bits 8–15, terminating signal in bits 0–6. Note
that `unshare -pf --mount-proc /bin/sh -c 'kill -SEGV $$'` yielded **0**, not 139, because the
generated `SIGSEGV` was discarded for PID 1 (§7.1) and the shell then exited normally. Our
"container was killed by signal N" reporting must not assume a container that ought to have died
by signal actually did.

### 7.5 Do we need a separate init/pause process?

Three candidate designs.

**A. User's command is PID 1 directly.** `clone3` → child does setup → `execve("/bin/sh")`.
- Simplest possible: two processes total, no extra binary to ship or find inside the rootfs.
- Costs, all consequences of §7.1/§7.4: `SIGTERM` from `mydocker stop` is silently dropped unless
  the user's program happens to handle it (so every stop takes the full grace period, then
  `SIGKILL`); reparented grandchildren are never reaped, so zombies accumulate; the exact
  semantics differ per image, which makes bug reports irreproducible.
- `[C]` This is what `docker run` gives you without `--init`, and it is a well-known wart.

**B. Our own init as PID 1, user command as its child.** Init installs handlers for the full
signal set, forwards them to the child's process group, reaps everything, exits with the child's
status.
- Fixes all of A's problems: `SIGTERM` actually delivered, no zombies, uniform semantics.
- Costs: the init binary must be reachable *inside* the rootfs after `pivot_root` (§5.4) — so
  either bind-mount a static binary in, or `execve("/proc/self/exe")` before the pivot and rely on
  the already-mapped image. Signal forwarding is genuinely fiddly (process groups, `SIGCHLD`
  races, correct exit-status translation). Adds a process to every container.
- `[C]` This is `docker run --init` (which injects `tini`), and it is what Kubernetes users end up
  enabling.

**C. A pause/holder process that holds namespaces open, separate from the workload.**
- Lets namespaces outlive the workload — needed for pod-style shared-namespace groups and for
  `mydocker exec` into a stopped-but-existing container.
- Costs: a whole extra lifecycle concept. Not needed for `run`.
- `[C]` This is Kubernetes' `pause` container. Not a Docker-single-container concept.

`[R]` HIGH — **MVP: design A, but make it a swappable strategy behind one interface, and document
the two known consequences in the CLI's `stop` output.** Reasoning: A is the shortest path to a
working `mydocker run alpine /bin/sh` and it exercises every kernel mechanism we are trying to
learn; the PID-1 semantics we would be papering over are exactly the material of §7.1. Zombie
accumulation is a non-issue for interactive shells and short-lived commands, which is the MVP's
whole use case.

Explicit tradeoff: the day we support long-running services, design A produces slow stops and
zombie leaks and we *will* have to add B. Cheap insurance now: keep the exec of the user command
behind a single call site so an init can be interposed without restructuring, and never assume in
the supervision code that PID 1 handles signals.

`[R]` MED — do **not** build C. It is a multi-container-pod feature and speculative for a
Docker-like single-container engine.

---

## 8. Signals and the terminal — kernel constraints

D2 owns who forwards what; this is the kernel surface they must build on.

### 8.1 Two independent delivery paths

**Path 1 — the engine signals the container process directly.** Requires the engine to be in an
**ancestor** PID namespace (it is) and to know the container's *host-namespace* PID. Constrained
exactly as §7.1: `SIGKILL`/`SIGSTOP` always land; everything else needs a handler in PID 1.

**Path 2 — the terminal generates signals.** If the container has a controlling terminal, the tty
driver turns `^C`/`^Z`/`^\` into `SIGINT`/`SIGTSTP`/`SIGQUIT` delivered to the tty's **foreground
process group**, with no involvement from the engine at all. `[F]` HIGH `termios(3)`, `tty(4)`.
This path bypasses the PID-1 restriction entirely, because the signal is delivered to a process
group rather than aimed at init from outside — and it is why `^C` in `docker run -it alpine sh`
feels instant while `docker stop` on the same container takes 10 seconds.

`[R]` HIGH — for interactive containers, allocate a pty and let path 2 do the work; reserve path 1
for `mydocker stop`/`kill`. They are not interchangeable and each is wrong for the other's job.

### 8.2 Use pidfds, not PIDs

`[F]` HIGH — `pidfd_send_signal(2)` (Linux 5.1) signals a process via a stable fd, immune to
PID reuse. A pidfd is obtained race-free at creation with `clone3(CLONE_PIDFD)`, or from
`pidfd_open(2)`. A pidfd also becomes **readable** when the process exits, so it can be polled
alongside everything else in an event loop.

`[O]` DROPLET 6.11.0-29 — both verified:
```
child host pid=2776108 pidfd=3  child ns=pid:[4026532570]
pidfd_send_signal(SIGTERM) rc=0
child alive after SIGTERM (no handler): True     <- discarded, as for any ns-init
pidfd_send_signal(SIGKILL) rc=0
waitpid status=0x9 signalled=True signo=9
...
pidfd became readable after 0.30s -> child exited; events=[(4, 1)]
exit code=9
```
Confidence HIGH. Note the first result again: **`pidfd_send_signal` returned 0 for a signal that
was discarded.** The success return means "accepted for delivery", never "delivered".

`[R]` HIGH — take the pidfd from `clone3(CLONE_PIDFD)` at creation and use it for every subsequent
signal and for exit notification. Tradeoff: 5.1+ only, so keep a `kill(2)`-by-pid fallback for
ROCKY-class kernels; but on our stated dev target the pidfd path removes an entire class of
"signalled the wrong process after PID reuse" bug that is essentially impossible to test for.

### 8.3 Controlling-terminal mechanics

`[F]` HIGH `setsid(2)`, `tty_ioctl(4)`. `setsid()` makes the caller a session leader **and
detaches it from any controlling terminal**; only a session leader with no controlling terminal may
acquire one via `ioctl(fd, TIOCSCTTY, 0)` on a tty fd.

`[O]` DROPLET 6.11.0-29:
```
before setsid: /dev/tty ENXIO
after setsid: sid=2776193 pgid=2776193
after setsid: /dev/tty ENXIO  <- controlling terminal dropped
TIOCSCTTY on new pty slave: OK -> this pty is now the ctty
foreground pgrp of that pty = 2776193 (our pgid=2776193)

TIOCSCTTY without setsid: EPERM  <- must be a session LEADER first
```
Confidence HIGH. (`/dev/tty` was already `ENXIO` before `setsid` because the probe ran under
non-interactive ssh — the point stands: after `setsid` it is definitively detached, and
`TIOCSCTTY` fails `EPERM` without leadership.)

So the required child sequence for an interactive container is: `setsid()` → open the pty **slave**
from the container's own devpts instance → `ioctl(TIOCSCTTY)` → `dup2` the slave onto fds 0/1/2 →
`execve`. `[C]` HIGH — runc's `setupConsole` + `system.Setctty()` does exactly this, and it must
happen after `/dev/pts` is mounted (§6.3) and before `execve`.

`[R]` HIGH — the engine holds the pty **master** in the host namespace and proxies bytes to/from
the user's terminal, which it puts in raw mode. Window-size changes must be forwarded explicitly:
`SIGWINCH` on the CLI → `ioctl(master, TIOCSWINSZ)` → the kernel delivers `SIGWINCH` to the
container's foreground group. Nothing propagates automatically.

---

## 9. Classification table

The consolidated answer to the session's most important question. **Required** = no container
without it. **Strongly recommended** = container "works" but real software breaks. **Hardening** =
raises the cost of an escape. **Docker convenience** = compatibility/UX, no isolation value.

### 9.1 Namespaces

| Mechanism | Tier | Why |
|---|---|---|
| `CLONE_NEWNS` (mount) | **required** | Without it there is no private rootfs; `pivot_root` would repoint the host |
| `CLONE_NEWPID` | **required** | Process-visibility isolation; also enables procfs mount in a userns |
| fresh `/proc` mount | **required** | Measured: pidns without it shows all 164 host PIDs (§1.2) |
| `CLONE_NEWNET` | **required** for isolation | Otherwise container binds host ports, sniffs host traffic |
| `CLONE_NEWIPC` | strongly recommended | Prevents SysV IPC crosstalk + limit exhaustion; enables mqueue in userns |
| `CLONE_NEWUTS` | strongly recommended | Cheap; without it container `hostname` renames the host |
| `CLONE_NEWCGROUP` | hardening | Path hiding only; controls nothing |
| `CLONE_NEWUSER` | hardening (the big one) | Only thing making container-root ≠ host-root; **not** Docker's default |
| `CLONE_NEWTIME` | Docker convenience | Docker doesn't use it; `CLOCK_REALTIME` not virtualized anyway |

### 9.2 Process creation

| Mechanism | Tier | Why |
|---|---|---|
| `clone`/`clone3` with ns flags | **required** | The only way to be born inside namespaces |
| extra fork for `unshare(CLONE_NEWPID)` | **required** *on that path only* | Not needed with `clone` (§3.3) |
| `CLONE_PIDFD` | strongly recommended | Race-free signalling + pollable exit |
| `CLONE_INTO_CGROUP` | strongly recommended | Closes the unconstrained window between clone and `cgroup.procs` write |
| `clone3` over legacy `clone` | strongly recommended | 64-bit flags; only route to `CLONE_NEWTIME`; extensible |
| intermediate (3rd) process | **required** *iff* userns | Someone outside must write `uid_map` before the child execs |
| `CLONE_PARENT` | strongly recommended (with 3 stages) | Init reparents to the runtime, not host PID 1 |
| single-threaded-before-clone invariant | **required** | `unshare(CLONE_NEWUSER)` → `EINVAL` multithreaded (§3.4) |

### 9.3 Mount mechanics

| Mechanism | Tier | Why |
|---|---|---|
| `MS_REC\|MS_SLAVE` on `/` | **required** | Measured: otherwise every container mount leaks to the host permanently (§4.2) |
| non-recursive `MS_PRIVATE` on rootfs mount | **required** | `pivot_root` precondition |
| recursive self-bind of rootfs | **required** | Makes rootfs a mount point — `pivot_root` precondition (C1/C2, §5.2) |
| `pivot_root` | **required** | `chroot` is escapable — demonstrated (§5.5) |
| `umount2(put_old, MNT_DETACH)` | **required** | An accessible `put_old` is a one-line escape |
| `chdir("/")` after `pivot_root` | **required** | Kernel does not move the caller's cwd (§5.2 C7) |
| `nosuid,nodev,noexec` on `/proc`,`/sys`,`/dev/shm`,`/dev/mqueue` | hardening | OCI/Docker default (§4.4) |
| `nosuid` on `/dev` (not `nodev`/`noexec`) | hardening | `nodev` would break the device nodes |
| proc/sys masking (`kcore` etc.) | hardening | `/proc/kcore` = readable physical memory |
| `mount_setattr(AT_RECURSIVE)` | hardening | Only correct way to do recursive read-only binds |
| `fsopen`/`fsmount`/`move_mount`/`open_tree` | hardening | Eliminates mount-path TOCTOU; not MVP |
| `MS_MOVE`+`chroot` fallback | Docker convenience | Only for initramfs-like environments; `--no-pivot` |

### 9.4 Rootfs contents

| Mechanism | Tier | Why |
|---|---|---|
| `/proc` | **required** | `ps`/`top`/`free`/`mount` + every `/proc/self` consumer (§6.1) |
| `/dev` tmpfs + core nodes | **required** | `/dev/null` absence silently creates a growing file (§6.2) |
| `/dev/pts` | **required in practice** | `openpty` → "out of pty devices"; no `-it` at all (§6.3) |
| `/dev/shm` | strongly recommended | `shm_open`/`sem_open`/multiprocessing/PostgreSQL/Chromium (§6.4) |
| `/sys` ro | strongly recommended | `lsblk`/`lscpu`/udev/D-Bus (§6.6) |
| `/sys/fs/cgroup` ro | Docker convenience → needed for JVM | Heap sizing against host RAM → OOM (§6.7) |
| `/dev/mqueue` | Docker convenience | POSIX mqueues only (§6.5) |
| `/dev/fd`,`/dev/std*` symlinks | Docker convenience | Some scripts; requires `/proc` first |

### 9.5 PID 1 / signals

| Mechanism | Tier | Why |
|---|---|---|
| signal from host (ancestor) ns | **required** | In-namespace `SIGKILL` to PID 1 is discarded (§7.1) |
| `SIGKILL` fallback after grace timeout | **required** | `SIGTERM` to a handler-less PID 1 is silently dropped |
| rely on PID-1-exit ⇒ namespace `SIGKILL` | strongly recommended | Kernel-guaranteed complete teardown (§7.4) |
| `pidfd_send_signal` + pidfd poll | strongly recommended | No PID-reuse race; pollable exit (§8.2) |
| separate init (reaping/forwarding) | strongly recommended, **not** MVP | Fixes zombies + slow stops; `docker run --init` (§7.5) |
| `setsid` + `TIOCSCTTY` on pty slave | **required** for `-it` | `TIOCSCTTY` → `EPERM` without session leadership (§8.3) |
| `SIGWINCH` → `TIOCSWINSZ` forwarding | Docker convenience | Resize only; nothing propagates automatically |
| pause/holder process | Docker convenience (pod-only) | Not a single-container concept |

---

## 10. The corrected creation sequence

The project spec proposed:

> CLI → resolve image → construct rootfs → create runtime state → clone/create process →
> namespaces → UID/GID configuration → cgroups → mount namespace → mount rootfs → pivot_root →
> /proc + required mounts → capabilities → seccomp → network namespace → environment → execve

**Five things are wrong with that order:**

1. **"clone/create process" then "namespaces" is backwards.** Namespaces are created *by* the
   clone (flags in the same syscall), not configured afterwards. Only the userns-first split
   needs a separate `unshare`.
2. **"UID/GID configuration" after the clone in the child is impossible.** The child cannot write
   its own `uid_map` — that is the whole reason an intermediate process and a sync pipe exist
   (§1.6, §3.3). It is a **parent** action, and it must complete before the child's `execve`.
3. **"cgroups" after the clone is a race.** The child is unconstrained until it is placed. Use
   `CLONE_INTO_CGROUP` in the clone itself, or have the parent write `cgroup.procs` before
   releasing the child. Also, cgroup placement must precede the cgroup *namespace* (§3.5 rule 5).
4. **"network namespace" after seccomp is far too late.** `CLONE_NEWNET` happens at clone time; the
   *plumbing* (veth move, address, routes) is a parent action that must finish before the child
   drops capabilities, and certainly before a seccomp filter forbids the netlink calls.
5. **"capabilities → seccomp" is only half the constraint.** Seccomp's position depends on
   `noNewPrivileges`: with NNP set, seccomp goes *last*, after caps; without it, seccomp must go
   *before* the bounding-set drop because installing a filter still needs `CAP_SYS_ADMIN`
   (§3.5 rule 9). Two branches, not one order.

### 10.1 Corrected sequence

`P` = parent/engine. `C` = child, after clone and before `execve`. `I` = intermediate (only when a
user namespace is involved).

| # | Step | Who | Primitive | Root? | MVP? | Failure cases |
|---|---|---|---|---|---|---|
| 1 | Parse CLI, resolve config | P | — | no | yes | — |
| 2 | Resolve image, materialise rootfs | P | overlayfs mount (D3) | yes (non-userns overlay) | yes | image missing; overlay unsupported on fs; `EBUSY` on reused upperdir |
| 3 | Write runtime state dir | P | `mkdir`, `open(O_CLOEXEC)` | no | yes | stale state from a crashed run |
| 4 | **Create cgroup, get its dirfd** | P | `mkdir` in cgroup2 + `open(O_DIRECTORY)` | yes | yes | no delegation; controllers not enabled in `subtree_control`; no-internal-process rule |
| 5 | Create sync socketpair + pidfd slot | P | `socketpair(SOCK_SEQPACKET)` | no | yes | fd exhaustion |
| 6 | **Assert `Threads == 1`** | P | read `/proc/self/status` | no | **yes** | any dependency spawned a thread ⇒ later `EINVAL`/deadlock (§3.4) |
| 7 | Pre-allocate argv/envp/path `CString`s, all mount descriptors | P | — | no | **yes** | must happen pre-clone: no allocation is safe post-clone (§11.2) |
| 8 | **`clone3`**: `NEWNS\|NEWPID\|NEWNET\|NEWUTS\|NEWIPC\|NEWCGROUP` + `CLONE_PIDFD` + `CLONE_INTO_CGROUP` | P | `clone3(2)` | yes | yes | `ENOSYS` (pre-5.3) ⇒ fall back to `clone`; `EPERM` w/o `CAP_SYS_ADMIN`; `ENOSPC` on ns limits; `EBADF`/`EOPNOTSUPP` on the cgroup fd |
| 8a | *userns variant:* `clone` → I `unshare(CLONE_NEWUSER)` alone → ask P for maps → `unshare(rest)` → `clone` init | P+I | `unshare`, sync pipe | no | **no** (D5) | `EINVAL` if I is threaded; caps lost at exec if maps late |
| 8b | *userns variant:* write `setgroups=deny`, then `uid_map`, then `gid_map` | **P** | `write` to `/proc/<pid>/*` | no | **no** (D5) | order inverted ⇒ `EPERM`; multi-line map w/o `CAP_SETUID` ⇒ `EPERM`; target non-dumpable ⇒ `EACCES` |
| 9 | Verify cgroup membership (fallback: write `cgroup.procs`) | P | `write` | yes | yes | child already forked ⇒ descendants escape limits |
| 10 | Plumb the netns (veth move, addr, routes, `lo` up) | P | rtnetlink (D4) | yes | partial — `lo` up only | must precede C's cap drop and seccomp |
| 11 | `MS_REC\|MS_SLAVE` on `/` | C | `mount(2)` | yes (or CAP_SYS_ADMIN in owning userns) | **yes** | omitted ⇒ **every container mount leaks to the host permanently** (§4.2) |
| 12 | Non-recursive `MS_PRIVATE` on the rootfs mount (walk up parents on `EINVAL`) | C | `mount(2)` | yes | yes | `pivot_root` later fails `EINVAL` |
| 13 | `MS_BIND\|MS_REC` rootfs onto itself | C | `mount(2)` | yes | yes | omitted ⇒ `pivot_root` `EINVAL` (not a mount point) |
| 14 | Mount `/proc`, `/dev`(tmpfs), `/dev/pts`, `/dev/shm`, `/dev/mqueue`, `/sys`, `/sys/fs/cgroup` **into the rootfs** | C | `mount(2)` | yes | yes | procfs needs the new pidns (§6.1); sysfs needs the new netns (§6.6); mqueue needs the new ipcns (§6.5) |
| 15 | `mknod` core device nodes (or bind them); create `/dev/fd`,`/dev/std*` symlinks | C | `mknod(2)`, `symlink(2)` | **yes** — `CAP_MKNOD` is initial-userns only | yes | rootless ⇒ `EPERM`, must bind instead (§6.2) |
| 16 | User bind mounts / volumes (two-pass for options) | C | `mount(2)` ×2 | yes | later | bind ignores flags on the first pass; recursive-ro needs `mount_setattr` |
| 17 | **`pivot_root`**: hold `O_PATH` on `/`, `fchdir(new)`, `pivot_root(".",".")`, `fchdir(old)`, `MS_REC\|MS_SLAVE` old, `umount2(".",MNT_DETACH)`, `chdir("/")` | C | `pivot_root(2)`, `umount2(2)` | yes | **yes** | 5 documented `EINVAL` preconditions (§5.2); forgetting `chdir("/")` leaves cwd outside the root |
| 18 | Re-open `/dev/null` for any fd pointing at the old root's null | C | `open(2)` | no | later | runc's `reOpenDevNull` — stale fds into a detached tree |
| 19 | `readonlyPaths`, `maskedPaths`, ro-remount of `/dev` and tmpfs | C | `mount(2)` | yes | later (hardening) | must follow step 14 |
| 20 | `sethostname`, `setdomainname` | C | `sethostname(2)` | CAP_SYS_ADMIN in owning userns | yes | without `CLONE_NEWUTS` this renames the **host** |
| 21 | `setsid()`; open pty slave; `TIOCSCTTY`; `dup2` 0/1/2 | C | `setsid(2)`, `ioctl(2)` | no | yes (for `-it`) | `TIOCSCTTY` `EPERM` without session leadership (§8.3); needs `/dev/pts` from step 14 |
| 22 | `chdir(workdir)` (optimistic; retry after the uid switch on `EPERM`) | C | `chdir(2)` | no | yes | workdir may only be enterable as the target uid |
| 23 | `PR_SET_NO_NEW_PRIVS` (if requested) | C | `prctl(2)` | no | later | must precede the late seccomp branch |
| 24 | AppArmor / SELinux label (D5) | C | `/proc/self/attr/*` | varies | no | write must precede `execve` |
| 25 | **seccomp — branch A** (only if **not** NNP) | C | `seccomp(2)` | needs `CAP_SYS_ADMIN` | later (D5) | after the cap drop and without NNP ⇒ `EACCES` |
| 26 | Drop capability **bounding** set | C | `PR_CAPBSET_DROP` | needs CAP_SETPCAP | later (D5) | irreversible; dropping too early breaks steps 14–20 |
| 27 | `PR_SET_KEEPCAPS`, `setgroups`, `setgid`, `setuid`, `PR_CLR_KEEPCAPS` | C | raw `syscall(SYS_setres*id)` | yes | yes | **glibc's `setuid` broadcasts to all threads (setxid) — unsafe post-clone; use raw syscalls** (§11.2) |
| 28 | Apply remaining cap sets (permitted/effective/inheritable/ambient) | C | `capset(2)`, `PR_CAP_AMBIENT` | — | later (D5) | must follow the uid change or the caps are cleared by it |
| 29 | Set up environment | C | pre-built `envp` for `execve` | no | yes | must be pre-allocated (step 7) |
| 30 | `close_range(3, ~0, CLOSE_RANGE_CLOEXEC)` | C | `close_range(2)` | no | yes | leaked host fds = escape (CVE-2024-21626); nothing may touch fds after |
| 31 | Signal readiness; wait for `start` | C | write to sync pipe | no | yes | parent must not `execve` the workload before plumbing is done |
| 32 | **`seccomp` — branch B** (only if NNP) | C | `seccomp(2)` | no | later (D5) | as late as possible: every later syscall must be in the profile |
| 33 | **`execve`** | C | `execve(2)` | no | **yes** | `ENOENT` (binary or its **interpreter/loader** missing from the image); `EACCES`; `ENOEXEC` |

### 10.2 Why the parent/child split exists at all

Four independent reasons, none of which can be worked around:

1. **Namespace membership is decided at clone time and mostly cannot be changed afterwards.** The
   child must do everything that requires being *inside* the new namespaces (steps 11–20): the
   parent literally cannot mount into a mount namespace it is not in.
2. **The child cannot grant itself privilege.** `uid_map`/`gid_map` must be written by a process
   outside the new user namespace (§1.6), and it must land before the child's `execve` or the
   child's capabilities are cleared. That is a hard parent obligation.
3. **The parent must survive to supervise.** Someone has to `waitpid`/poll the pidfd, report the
   exit status, and clean up the cgroup and network. If the workload were the only process, there
   would be nobody to observe it.
4. **Some setup needs privilege the child is about to drop.** Moving a veth into the netns,
   writing cgroup files, opening host paths — all easier or only possible from the parent, and all
   must complete before step 26 drops the bounding set.

**What must happen in the child, after `clone`, before `execve` — the irreducible list:** steps
11–14 (propagation + rootfs bind + all mounts), 15 (device nodes), 17 (`pivot_root` + detach), 20
(hostname), 21 (session/tty), 23–28 (NNP, LSM, seccomp, caps, uid), 30 (fd hygiene), 32–33
(seccomp-late, exec). Everything on that list either requires being inside the new namespaces or
must not be visible to the workload once it starts.

---

## 11. Rust: crates and the post-clone safety constraint

### 11.1 Crates

Health, from the crates.io and GitHub APIs (fetched 2026-08-23). All `[F]` HIGH.

| crate | latest | date | 90-day dl | MSRV | open iss+PR | last commit |
|---|---|---|---|---|---|---|
| `libc` | 0.2.189 | 2026-07-21 | 332M | 1.65 | 172 | 2026-08-20 |
| `rustix` | 1.1.4 | 2026-02-22 | 237M | 1.63 | 142 | 2026-08-19 |
| `linux-raw-sys` | 0.12.1 | 2025-12-23 | 231M | 1.63 | 18 | 2026-03-30 |
| `nix` | 0.31.3 | 2026-05-11 | 164M | 1.69 | **347** | 2026-05-19 |
| `caps` | 0.5.6 | 2025-10-17 | 7.5M | 1.63 | 7 | 2025-10-17 |
| `oci-spec` | 0.10.0 | 2026-05-27 | 4.1M | — | 24 | 2026-07-02 |
| `cgroups-rs` | 0.5.1 | 2026-07-14 | 964K | — | 21 | 2026-08-20 |
| `syscalls` | 0.8.1 | 2026-02-02 | 282K | — | 11 | 2026-04-23 |
| `sc` | 0.2.7 | **2022-03-24** | 203K | — | — | stale |

**`nix` 0.31.3.** Thin 1:1 syscall wrappers, hides nothing. Has `unshare`, `setns` (as
`sched::setns`), `mount`/`umount`/`umount2`, `unistd::pivot_root` (itself a raw
`syscall(SYS_pivot_root)`), signals, `waitpid`, `sethostname`. `[F]` HIGH — **no `clone3`**, and
`nix::mount` on Linux has *only* `mount`/`umount`/`umount2`: none of the new mount API.

**`nix::sched::clone` should not be used.** `[F]` HIGH — signature is
`unsafe fn clone(cb: Box<dyn FnMut() -> isize>, stack: &mut [u8], flags, signal) -> Result<Pid>`.
It passes `&mut cb` — a pointer to a `Box` living in `nix::clone`'s **own stack frame** — to the
child. With `CLONE_VM` the parent returns, that frame is popped, and the child dereferences freed
stack. This is nix issue **#360, open since 2016-04-19** ("clone() is broken and unsafe with
CLONE_VM flag ... will unconditionally segfault when passed CLONE_VM") and **#2803** (2026-06-20,
"Stack use-after-free and data race in `sched::clone` under `CloneFlags::CLONE_VM`"). Separately,
`stack: &mut [u8]` means you hand it heap or parent-stack memory with **no guard page**, so
overflow silently corrupts adjacent memory rather than faulting. A ten-year-old open soundness bug
in the one primitive we care most about is a sufficient reason to write our own. `[R]` HIGH.

**`rustix` 1.1.4.** Same abstraction level, plus I/O-safety types, plus the property that matters
here: `[F]` HIGH its **`linux_raw` backend is the default** on x86-64/aarch64/riscv64/etc. and
issues syscalls via inline asm with **no libc, no PLT, no `errno` function call, no locks, no
allocation** — the closest thing Rust has to a guaranteed async-signal-safe syscall wrapper.
Errors are a `u16` `Errno` newtype, which does not allocate (unlike `std::io::Error`).

`[F]` HIGH — rustix has **the best new-mount-API coverage of any crate**: `fsopen`, `fsmount`,
`fsconfig_set_flag/string/binary/path/fd`, `fsconfig_create`, `move_mount`, `open_tree`, plus
classic `mount`/`mount_bind`/`mount_change`/`mount_move`/`mount_remount`/`unmount`. Also
`openat2`. `pivot_root` lives in `rustix::process`; namespaces live in `rustix::thread` under
rustix-specific names (`move_into_link_name_space` = `setns`, `unshare`, `unshare_unsafe`).
Missing: **`mount_setattr`** (PRs #975/#1002/#1630 open) and **`close_range`**. Do not build on
`rustix::runtime` — it is `doc(hidden)` and its own docs say the API is unstable and "cannot be
used in a process which also has a libc present."

**raw `libc` 0.2.189.** `[F]` HIGH, verified against the crate source. **Declared as functions:**
`unshare`, **`setns`**, `clone` (the variadic glibc wrapper), `umount2`, `mount`, `prctl`,
`sethostname`, `setdomainname`, `setgroups`, `waitid`, `syscall`. `close_range` is declared
**gnu-only** — on `*-linux-musl` it needs a raw syscall. (This corrects a MED claim in an earlier
draft: `setns` *is* declared.)

**Not declared — need `syscall(SYS_*)`:** `clone3`, `pivot_root`, `fsopen`, `fsmount`, `fsconfig`,
`move_mount`, `open_tree`, `mount_setattr`, `openat2`, `capget`, `capset`. All have `SYS_*`
constants, so each wrapper is one `unsafe` line. `[F]` HIGH — `man clone(2)` states outright:
"glibc provides no wrapper for `clone3()`, necessitating the use of `syscall(2)`."

`[F]` HIGH — the *types* come free from `libc`: `struct clone_args` (per-arch), `struct open_how`,
and a UAPI module carrying `struct mount_attr`, all `MOUNT_ATTR_*` (including `_IDMAP`),
`OPEN_TREE_{CLONE,NAMESPACE,CLOEXEC}`, `MOVE_MOUNT_*`, `FSOPEN_CLOEXEC`, `FSMOUNT_CLOEXEC`,
`enum fsconfig_command`, `CLOSE_RANGE_CLOEXEC`. Note `libc` declares **no `CAP_*` constants at
all**.

**`caps` 0.5.6.** Covers all five sets including **ambient** (`PR_CAP_AMBIENT`) and **bounding**
(`PR_CAPBSET_DROP`/`_READ`), base sets via raw `capget`/`capset`. `[F]` HIGH — **does not cover
`PR_SET_NO_NEW_PRIVS`** (zero hits in the crate). It also allocates: `read()` returns a
`HashSet<Capability>` and errors are `CapsError(String)`, so it is unusable post-clone.
Single maintainer, feature-frozen, capability table hardcoded to `CAP_CHECKPOINT_RESTORE = 40`.
`[R]` MED — write ~60 lines of `capget`/`capset`/`PR_CAPBSET_DROP`/`PR_CAP_AMBIENT` ourselves; the
v3 header/data layout is trivial and this is exactly the mechanic we want visible. Use `caps` only
for its name↔bit table if we want it.

**`cgroups-rs` 0.5.1** — `[R]` HIGH **do not use.** Now kata-containers-maintained and it also
drives the systemd cgroup driver over D-Bus, dragging `zbus 5`, `strum`, `uuid`, `tracing`, and
*two* versions of `nix` into the graph. cgroup v2 is `mkdir` plus `std::fs::write` to
`cgroup.subtree_control`, `cgroup.procs`, `memory.max`. Its value is v1 compatibility and systemd
delegation, neither of which we want. (D5's call ultimately, but the dependency cost is ours.)

**`oci-spec` 0.10.0** (youki-dev) — `[R]` HIGH, the right dependency once we do OCI compat, with
`features = ["runtime"]`. Hand-writing `config.json`'s full schema is typing, not learning, and it
hides no kernel mechanics — it is a serde schema.

**Others.** `syscalls` 0.8.1 gives typed per-arch `Sysno` enums with inline-asm `syscall!` macros,
`no_std`-capable — an attractive alternative to `libc::syscall` for the raw calls, though we still
need `libc` for the struct layouts. `linux-raw-sys` 0.12.1 is bindgen'd UAPI types (rustix's type
layer) if we want the structs without libc. `sc` is stale (2022) — avoid. `procfs` 0.18.0 is fine
for a `state` subcommand but does hide `/proc/self/mountinfo`'s field order, which is the one file
we must understand to get propagation right — parse that one by hand. **A `close_range` crate does
not exist.**

`[R]` HIGH — **recommended stack:**
```
libc      # SYS_* numbers, clone_args/mount_attr/open_how layouts, syscall()
nix       # unshare, setns, mount/umount2, pivot_root, signals, waitpid, sethostname
rustix    # new mount API, openat2  (adopt later per §4.5; linux_raw backend)
oci-spec  # later, features = ["runtime"]
+ ~200 lines of our own: clone3, mount_setattr, close_range, capget/capset, prctl(NO_NEW_PRIVS)
NOT: nix::sched::clone (unsound, §11.1), cgroups-rs (D-Bus), caps (allocates), sc (stale)
```
Tradeoff: mixing `nix` and `rustix` means two error types. That is a real annoyance and the
mitigation is one `From` impl. It buys us the only complete new-mount-API coverage plus the
`linux_raw` no-libc property, and it lets us skip the one nix primitive that is actually unsound.

### 11.2 What is safe between `clone`/`fork` and `execve`

This is the correctness landmine D2 is waiting on. The kernel-level answer:

`[F]` HIGH — POSIX and `fork(2)`: after a `fork` in a **multithreaded** process, the child may
call only **async-signal-safe** functions until it `execve`s. The child gets one thread — the one
that called `fork` — but inherits the *entire* address space, including every mutex in whatever
state it was in. If another thread held a lock at fork time, that lock is now held forever by a
thread that does not exist.

**The allocator is the specific danger.** glibc's `malloc` uses internal arena locks. If another
thread was inside `malloc` when the clone happened, the child's first allocation deadlocks —
silently, unreproducibly, under load only. Rust's default global allocator (`std::alloc::System`)
is libc `malloc` on glibc targets, so **Rust inherits this hazard exactly**. `[F]` MED on the
Rust-specific mapping — it follows from `System` delegating to `malloc`, but I did not read the
`std` source to confirm for every target; what would settle it is
`library/std/src/sys/alloc/unix.rs`.

**Therefore, post-clone and pre-`execve`, do not:**
- allocate — no `String`, `Vec`, `format!`, `to_owned()`, `CString::new`, `Box`, or any collection
  growth. This is the rule that forces step 7 of §10.1: **every `CString` for path, argv, and
  envp must be built before the clone.**
- `println!`/`eprintln!`/`dbg!` — `std::io::Stdout` is a `ReentrantLock` over a `LineWriter`
  buffer. Both the lock and the inherited buffer are hazards; the buffer one is observable (below).
- panic or unwind — the panic machinery allocates and formats.
- use `std::process::Command`, any logging framework, or anything that might lazily initialise.
- call **glibc's** `setuid`/`setgid`/`setgroups`. `[C]` HIGH — glibc implements the `setxid`
  family with a broadcast: signal every registered thread and wait for acknowledgement. In a
  cloned child whose libc thread bookkeeping is inherited but whose threads do not exist, that
  hangs. youki bypasses it with raw syscalls and an explicit comment: *"This is safe because at
  this point we have only one thread in the process"* around
  `libc::syscall(libc::SYS_setresuid, ...)`. `[R]` HIGH — do the same. This is step 27 of §10.1.

**The inherited-stdio-buffer hazard, observed live.** `[O]` DROPLET 6.11.0-29 — my `clone3` probe
script's output appeared **twice**: the parent had buffered `print` output that had not been
flushed at clone time; the children inherited the buffer and flushed it at their own exit. The
entire "=== clone3 struct sizes ===" block was duplicated verbatim in the transcript. Confidence
HIGH. This is exactly the class of bug the rule above prevents, and it is worth noting that it
manifests as *wrong output*, not a crash — the failure mode is silent corruption.

**Is a Rust binary multithreaded by default?** `[R]`/`[F]` MED — a plain `std` Rust binary starts
single-threaded; `std` itself spawns no background threads. But the guarantee is about the whole
process, not our code: a `tokio` runtime, `rayon`'s global pool, jemalloc's background threads,
`tracing-appender`'s non-blocking writer, or any dependency's `lazy_static` that starts a thread
all break it, and a transitive dependency can do this without our noticing. Hence §3.4's
recommendation: assert `Threads == 1` from `/proc/self/status` immediately before the clone.

**Why Rust is genuinely better positioned than Go here, and when that stops being true.** Go's
runtime is *unconditionally* multithreaded before `main` — so runc can never do
`unshare(CLONE_NEWUSER)` from Go, which is why `nsexec.c` runs as an ELF constructor before the Go
runtime initialises (§3.4). Rust has no such runtime, so a single-threaded Rust binary can do the
whole namespace dance in-process with no C trampoline and no re-exec. **That advantage is a
property of our dependency graph, not of the language**, and it disappears the moment we add an
async runtime to the process that performs the clone. `[R]` HIGH — keep the container-creation
path in a process (or a pre-runtime phase) that provably has no threads. If the engine needs
async for its API surface, that argues for a separate helper binary or a strict
"clone before starting the runtime" ordering — which is essentially the architectural reason
runtimes re-exec `/proc/self/exe`.

**Why some runtimes re-exec a helper instead of doing the work in the child.** `[C]` HIGH — three
reasons: (1) it resets the thread situation, giving a guaranteed single-threaded process; (2) it
gives the child a fresh, fully-initialised language runtime, so normal code (allocation, logging,
error handling) becomes safe again; (3) `exeseal`/memfd copies of the binary defend against
CVE-2019-5736, where a container overwrote the runtime's own binary via `/proc/self/exe`. The cost
is a full process spawn per container and a serialisation format for the config (runc uses netlink
TLV for the pre-Go part and JSON for the Go part). `[R]` MED — for our MVP, doing the setup
directly in the cloned child is simpler and safe **because we can guarantee single-threadedness**;
adopt the re-exec pattern only if we take on an async runtime or find ourselves needing real
allocation in the child.

**fd hygiene.** `[F]` HIGH — `close_range(2)` (Linux 5.9) closes a whole fd range in one call, and
with `CLOSE_RANGE_CLOEXEC` marks them cloexec instead of closing, which is safer when other code
may still hold Rust `File` objects whose `Drop` would then close an unrelated fd. `[O]` present on
both hosts, including ROCKY 4.18 via backport (§2.1). Rust's `std` sets `O_CLOEXEC` on files it
opens, so the main leak risk is fds we create with raw syscalls or inherit. `[R]` HIGH — do the
`close_range` sweep as the last action before `execve` (step 30) and touch no `File` afterwards;
leaked host fds into a container are a documented escape class (CVE-2024-21626).

`posix_spawn` is irrelevant to us — it cannot create namespaces. `[F]` HIGH.

---

## 12. Handoff

**To D2 (runtime architecture / OCI / daemon-shim):**
- The corrected creation sequence and the parent/child split rationale are §10. The
  must-happen-in-the-child list is §10.2.
- **The kernel-level answer you were waiting on is §11.2.** Short version: no allocation, no
  `println!`, no panic, no glibc `setuid` between clone and `execve`; pre-build every `CString`;
  assert `Threads == 1` before cloning. Rust's advantage over Go here is real but is a property of
  our dependency graph, not the language — an async runtime in the cloning process destroys it.
- Signal forwarding architecture is yours; the kernel constraints are §8. The one that will shape
  your design: **`SIGTERM` to a handler-less container PID 1 is silently discarded even from the
  host**, and `pidfd_send_signal` returns 0 anyway. `stop` needs a timeout and a `SIGKILL`.
- Use `clone3(CLONE_PIDFD)` and keep the pidfd as the container handle: race-free signalling plus
  pollable exit (§8.2).
- The init/PID-1 decision (§7.5) is a lifecycle-design choice with a kernel-semantics basis; my
  recommendation is design A for MVP behind a swappable interface.

**To D3 (images / layers / overlayfs):**
- The rootfs must be a **mount point** for `pivot_root` — if the overlay mount is the rootfs that
  is satisfied, but if you hand us a plain directory we must recursively bind it onto itself
  (§10.1 step 13).
- Overlayfs in a user namespace requires Linux **5.11+** (`[F]` HIGH `user_namespaces(7)`); ROCKY
  4.18 needs `fuse-overlayfs`. Relevant to D5's rootless track, not our MVP.
- `[O]` DROPLET: Docker's live container rootfs mounts overlay with `nouserxattr` — worth knowing
  when you compare our overlay options against Docker's.

**To D4 (networking):**
- `CLONE_NEWNET` gives you exactly one device, `lo`, and it is **down**. Everything else is
  userspace's job.
- Netns plumbing must be done by the **parent** and must complete before the child drops
  capabilities and installs seccomp (§10.1 step 10).
- Non-obvious constraint you will hit: **mounting sysfs in a user namespace requires the netns to
  be owned by that userns** — `[O]` ROCKY 4.18, `unshare -U -m` alone → permission denied,
  `unshare -U -m -n` → OK (§6.6). runc's rootless mode works around it by bind-mounting host
  `/sys` read-only.
- `[R]` an empty netns with `lo` up is a legitimate MVP target; it makes the container look
  network-broken but the isolation is correct.

**To D5 (cgroups / caps / seccomp / LSM / rootless):**
- Measured Docker defaults on a live container: 14-capability bounding set
  `0xa80425fb`, `Seccomp: 2` with 1 filter, and **`NoNewPrivs: 0`** (§7.2). That last one means
  runc took the "install seccomp while still holding `CAP_SYS_ADMIN`" branch.
- The seccomp/NNP ordering is a **kernel** constraint, not a preference: `seccomp(2)` returns
  `EACCES` without either `CAP_SYS_ADMIN` or `no_new_privs`. Two code branches are mandatory
  (§3.5 rule 9).
- Rootless facts I verified you will need: `CAP_MKNOD` is useless outside the initial userns
  (`[O]` ROCKY — `mknod` `EPERM` with a full 41-cap `CapEff`), so `/dev` must be bind-mounted;
  procfs needs a new pidns; mqueue needs a new ipcns; sysfs needs a new netns.
- **Capabilities from `CLONE_NEWUSER` do not survive `execve` unless uid 0 is mapped** (§1.6,
  measured both ways). This makes `uid_map` timing a correctness issue, not a convenience.
- `CLONE_INTO_CGROUP` is cgroup-v2 only and is validated by the kernel on 6.11 (`EBADF`, not
  `EINVAL`, for a bad fd). It removes the unconstrained window between clone and `cgroup.procs`.
- `[O]` DROPLET: `apparmor_restrict_unprivileged_userns = 1` on Ubuntu 24.10 — will bite the
  rootless track on Ubuntu specifically.
- ROCKY is cgroup **v1** with no delegation, so it is a bad proxy for our cgroup-v2 target.

---

## 13. Open questions

1. **`CLONE_INTO_CGROUP` end-to-end.** I verified the flag is recognised and validated (`EBADF`
   for a bad fd) but did **not** perform a successful placement, because that needs creating a
   cgroup on a production host. Unresolved: the exact errno when the target violates the
   no-internal-process rule, and whether the child is billed to the target cgroup from its very
   first instruction. Settles it: on the dev VM, `mkdir /sys/fs/cgroup/test`, open it `O_DIRECTORY`,
   `clone3(CLONE_INTO_CGROUP)`, read `cgroup.procs`.
2. **`OPEN_TREE_NAMESPACE`'s first kernel version.** crun uses it to build a whole mount namespace
   out-of-band and `setns` into it, skipping `pivot_root` entirely. I could not confirm the
   upstream version from a primary source. Settles it: `git log -S OPEN_TREE_NAMESPACE
   fs/namespace.c`, or the flag appearing in `open_tree(2)`.
3. **`newinstance` no-op — exact commit.** Observed ignored on 6.11 (§6.3); attributed to the
   `CONFIG_DEVPTS_MULTIPLE_INSTANCES` removal around 4.7 but not verified. Settles it:
   `git log --oneline fs/devpts/inode.c` near v4.7.
4. **Forced-vs-generated signal mechanism.** Behaviour confirmed both ways (§7.1); the
   `SIGNAL_UNKILLABLE`/`force` explanation is inferred from behaviour, not read. Settles it:
   `sig_task_ignored()` in `kernel/signal.c` and how `force` is derived in
   `__send_signal_locked()`.
5. **Rust `std::alloc::System` → glibc `malloc` for every target.** The fork-deadlock consequence
   is what makes §11.2's no-allocation rule non-negotiable, and I reasoned it rather than reading
   `std`. Settles it: `library/std/src/sys/alloc/unix.rs`. Also unresolved: whether musl's
   allocator has the same fork hazard (it has different locking) — relevant if we ever ship a
   static musl build.
6. **`libc` crate's exact declared-vs-missing syscall list.** §11.1 is MED because it varies by
   version and target. Settles it: grep the pinned `libc` source.
7. **Time namespace on RHEL 8.** `/proc/self/ns/time` exists on 4.18.0-425 but I never exercised
   `timens_offsets` there (no `unshare -T` in util-linux 2.32.1). Whether it is fully functional or
   a partial backport is unknown, and irrelevant unless we target RHEL 8.
8. **Whether `MS_REC|MS_SLAVE` or `MS_REC|MS_PRIVATE` is right for `/`.** runc and youki use
   slave; crun uses private. I have runc's stated devicemapper-race reason but did not verify the
   underlying bug. We should follow runc/youki (the majority, and the more conservative choice for
   volume lifecycle), but this is a genuine disagreement among mature runtimes rather than a
   settled fact.
9. **Delegated crate assessment (§11.1) had not returned at write time.** The `nix`/`rustix`/
   `caps`/`libc` maintenance-health and coverage comparison should be merged into §11.1 from that
   research.

---

## 14. Sources

Primary — man pages, read locally on DROPLET (`manpages`/`manpages-dev` **6.8-2**, Ubuntu 24.10),
cross-checked against man7.org:
- `pid_namespaces(7)` — PID-1 signal rules, init-termination `SIGKILL`, `fork`→`ENOMEM`, orphan
  reparenting, the `mount -t proc proc /proc` recommendation, nesting limit 32, `CLONE_THREAD`
  incompatibility
- `mount_namespaces(7)` — the four propagation types verbatim, peer-group formation, "Restrictions
  on mount namespaces" [1]–[4] (less-privileged namespaces, shared→slave reduction, locked mounts)
- `pivot_root(2)` — all six restrictions verbatim, the "does not change the caller's cwd" clause
- `unshare(2)` — `CLONE_NEWUSER` single-threaded requirement, per-flag `EINVAL`/`ENOSPC`/`EPERM`
  table, the "previously called unshare with CLONE_NEWPID" restriction
- `setns(2)` — per-*thread* semantics, userns `EINVAL` conditions, PID/time "children only" rule
- `user_namespaces(7)` — capability semantics in a userns, the filesystem-mount permission list
  (proc/sys/devpts/tmpfs/ramfs/mqueue/bpf/overlayfs + versions), the
  `CAP_SYS_TIME`/`CAP_SYS_MODULE`/`CAP_MKNOD` initial-userns-only rule, `setgroups`-before-`gid_map`
  ordering, single-line-map restriction
- `time_namespaces(7)` — the two virtualized clocks, the "only way to create ... is `unshare`"
  claim (**contradicted on 6.11**, §1.8), `time_for_children` semantics
- `cgroup_namespaces(7)`, `namespaces(7)` (namespace-type/flag/version table), `clone(2)`
  (`CLONE_INTO_CGROUP` verbatim; **no** `CLONE_NEWTIME` entry), `mount(2)`, `capabilities(7)`,
  `chroot(2)`, `seccomp(2)`, `setsid(2)`, `pts(4)`
- https://man7.org/linux/man-pages/man7/pid_namespaces.7.html
- https://man7.org/linux/man-pages/man2/unshare.2.html
- https://man7.org/linux/man-pages/man2/setns.2.html
- https://man7.org/linux/man-pages/man2/seccomp.2.html
- https://man7.org/linux/man-pages/man7/user_namespaces.7.html
- https://man7.org/linux/man-pages/man2/open_tree.2.html

Runtime source (pinned versions, read as raw files):
- runc v1.5.1 — `libcontainer/nsenter/nsexec.c`, `nsenter.go`, `standard_init_linux.go`,
  `init_linux.go`, `rootfs_linux.go`, `process_linux.go`, `sync.go`, `container_linux.go`,
  `specconv/example.go`
  (https://raw.githubusercontent.com/opencontainers/runc/v1.5.1/...)
- crun 1.29.1 — `src/libcrun/linux.c`, `src/libcrun/container.c`
  (https://raw.githubusercontent.com/containers/crun/1.29.1/...)
- youki v0.7.0 — `crates/libcontainer/src/process/{fork.rs,container_main_process.rs,
  container_intermediate_process.rs,init/process.rs}`, `src/namespaces.rs`, `src/syscall/linux.rs`,
  `src/rootfs/rootfs.rs`, `crates/youki/src/main.rs`
  (https://raw.githubusercontent.com/youki-dev/youki/v0.7.0/...)

Empirical (all `[O]` claims):
- **DROPLET** `my-droplet` / `AIKYNETIX-PROD-NY`, Ubuntu 24.10, `6.11.0-29-generic`, x86_64.
  Every mutating probe confined to a private mount namespace with scratch under `/tmp`; host mount
  table verified unchanged after each; all scratch removed. No container, iptables, systemd, or
  `:80`/`:443` state touched.
- **ROCKY** `uc-arcc` / `arcc2`, Rocky Linux 8.8, `4.18.0-425.3.1.el8`, x86_64, unprivileged.
  All writes under `/tmp`, removed afterwards.
