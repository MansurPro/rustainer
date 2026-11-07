# V1 — adversarial verification of D1 (kernel isolation), §1–§11

Verifier: V1. Target: `research/D1-kernel-isolation.md` (1960 lines, written 2026-09-04).
Date: 2026-10-23. Contract: `research/VERIFIER-CONTRACT.md`.

## Method and evidence base

**Primary sources read, pinned.** Linux kernel source at tag **v6.11** (the DROPLET's kernel),
fetched as raw files and read directly — `torvalds/linux@v6.11/` + path + line:
`fs/namespace.c` (`clone_mnt` L1285, `do_umount` L1917, `pivot_root` L4301),
`fs/pnode.c` (`change_mnt_propagation`), `fs/super.c` (`mount_capable` L690),
`fs/proc/root.c` (`proc_init_fs_context` L252, `proc_root_lookup` L323, `subset=pid` L101),
`kernel/fork.c` (`clone3_args_valid` L3025, `copy_clone_args_from_user` L2928, CLONE_PARENT
L2162, SIGNAL_UNKILLABLE L2558), `kernel/cgroup/namespace.c` (`copy_cgroup_ns` L75),
`kernel/signal.c` (`sig_task_ignored` L82, `send_signal_locked` L1215,
`force_sig_info_to_task` L1326), `kernel/exit.c` (`find_new_reaper` L625),
`kernel/pid_namespace.c` (`zap_pid_ns_processes` L170), `kernel/user_namespace.c`
(`verify_root_map` L881, `new_idmap_permitted` L1163), `kernel/nsproxy.c`,
plus `torvalds/linux@v4.18/kernel/fork.c` for the RHEL-8-era comparison.
plus `v6.11/include/uapi/linux/{sched.h,mount.h}`, `fs/{sysfs/mount.c,devpts/inode.c}`,
`ipc/mqueue.c`, `Documentation/admin-guide/cgroup-v2.rst`.
man-pages from man7.org, fetched 2026-10-23 (**man-pages 6.19**, 2026-06-05 — note D1 read the
DROPLET's `manpages-dev 6.8-2`): `clone(2)`, `setns(2)`, `pivot_root(2)`, `close_range(2)`,
`chroot(2)`, `pid_namespaces(7)`, `mount_namespaces(7)`, `user_namespaces(7)`,
`cgroup_namespaces(7)`, `signal-safety(7)`, `proc(5)`.
glibc at tag **glibc-2.40** (the DROPLET's): `posix/fork.c`, `malloc/arena.c`,
`nptl/nptl_setxid.c`. Runtimes at pinned tags: **runc v1.5.1**
(`libcontainer/{rootfs_linux.go,process_linux.go,nsenter/nsexec.c,configs/validate/validator.go}`),
**crun 1.29.1** (`src/libcrun/{linux.c,linux.h,utils.c,container.c}`).
Docker: `docs.docker.com/engine/release-notes/29/` and `/engine/security/userns-remap/`.

**Empirical, read-only, both contract hosts.** DROPLET (`my-droplet`, 6.11.0-29, live
production — every probe confined to a throwaway private mount+PID namespace with scratch
dirs under `/tmp`, removed afterwards, host mount table verified unchanged):
`docker version`/`docker info`; a `subset=pid` procfs probe; a second-procfs-instance probe;
a `clone3(CLONE_PARENT)` combination probe; `close_range` flag probes.
ROCKY (`uc-arcc`, 4.18.0-425): `close_range` flag probes. Nothing was started, stopped,
written outside `/tmp`, or left behind. Local machine is macOS — no kernel claim verified here.

**Out of scope by assignment:** everything about the Rust substrate (crates, `nix` vs `rustix`,
allocator behaviour) is V7's; I touch §11 only where a *kernel or libc* fact is at stake, and I
take V7's `rustix::io::close_range` finding as given.

---

## Part 1 — §9.3 row 1: `MS_REC|MS_SLAVE` on `/`, tier and justification — **CORRECTED** (tier survives, reasoning does not)

D1 §9.3 classifies `MS_REC|MS_SLAVE` on `/` as **required**, justified as
*"Measured: otherwise every container mount leaks to the host permanently (§4.2)"*.

**Tier: CONFIRMED required. Justification: CORRECTED — it is not the stated failure mode, and
the stated failure mode is mostly prevented by a *different* row of the same table.**

### 1.1 The §4.2 experiment does not support the §9.3 claim

§4.2 measured a mount made *directly on a `shared` mount point* propagating to the parent
namespace and surviving the child's death. That is correct and well-founded. It is not what
steps 11–14 of §10.1 do. In the runc-shaped sequence D1 itself recommends, the container's
own mounts (`/proc`, `/dev`, `/dev/pts`, `/dev/shm`, `/sys`) are created **under the rootfs
self-bind**, and that self-bind's propagation is decided by step 12 (non-recursive
`MS_PRIVATE` on the rootfs's parent mount), not by step 11.

`[F]` HIGH — `torvalds/linux@v6.11/fs/namespace.c#L1320-L1334` (`clone_mnt`): a plain bind
copies the *source* mount's peer-group membership — `if ((flag & CL_MAKE_SHARED) ||
IS_MNT_SHARED(old)) list_add(&mnt->mnt_share, &old->mnt_share); if (IS_MNT_SLAVE(old))
list_add(&mnt->mnt_slave, &old->mnt_slave); mnt->mnt_master = old->mnt_master;`. A bind of a
**private** mount is private and belongs to no peer group; mounts created under a private
parent therefore propagate nowhere. So with steps 12+13 done and step 11 omitted, the
container's own mount set would still not leak. D1's §9.3 attribution of the leak to step 11
is wrong.

### 1.2 What step 11 actually buys — and it is worse than a leak

`[F]` HIGH — `torvalds/linux@v6.11/fs/namespace.c#L1914-L1917`, `do_umount()`:

```c
	if (flags & MNT_DETACH) {
		if (mnt->mnt.mnt_flags & MNT_ONRB ||
		    !list_empty(&mnt->mnt_list))
			umount_tree(mnt, UMOUNT_PROPAGATE);
```

**`umount2(..., MNT_DETACH)` propagates.** `umount_tree()` with `UMOUNT_PROPAGATE` calls
`propagate_umount()` on the gathered subtree. Step 17 of §10.1 detaches the **old root** —
i.e. a lazy umount of a tree that, if `/` was left `shared`, is still in the host's peer
group. The first-order risk of skipping step 11 is therefore not "`/proc/mounts` grows on the
host"; it is **umount events from the container's teardown propagating into the host mount
namespace**. That is a destructive, not a cosmetic, failure.

The second-order reason is the `pivot_root` precondition, and it is **not** the one D1 implies
(§5.2's C3 test is read in D1 as "`/` must not be shared"). The kernel checks three mounts —
`torvalds/linux@v6.11/fs/namespace.c#L4338`:

```c
	if (IS_MNT_SHARED(old_mnt) ||          /* the mount holding put_old, itself */
	    IS_MNT_SHARED(ex_parent) ||        /* parent of new_root */
	    IS_MNT_SHARED(root_parent))        /* parent of the current root */
		goto out4;                     /* -EINVAL */
```

With the `pivot_root(".", ".")` idiom, `old_mnt == new_mnt` = the rootfs self-bind, and
`ex_parent` = the mount holding the rootfs path. Both are neutralised by **step 12 alone**.
`root_parent` is the parent of the current root mount — on a normal booted system that is the
hidden initramfs `rootfs` mount that the real root was pivoted onto at boot, which is private;
it is *not* `/` itself. So: **`pivot_root` does not actually require step 11 if step 12 is
done first.** D1's §5.2 C3 result ("`pivot_root` while `/` is SHARED → EINVAL") was produced by
`ex_parent`/`old_mnt` being shared because the self-bind was taken from a shared `/`, which is
exactly the `IS_MNT_SHARED(ex_parent)` clause, not a "`/` must be private" rule.

One refinement, from `mount_namespaces(7)`: "What happens if the mount itself is unmounted is
determined by the propagation type that is in effect for **the parent** of the mount." After
`pivot_root(".", ".")` the old root's parent is the new (private) root, so the old root's *own*
detach does not propagate; its **descendants'** do, because their parents are still peers of the
host's mounts. That is exactly why the §5.3 sequence re-applies `MS_REC|MS_SLAVE` **to the old
root** immediately before `umount2(".", MNT_DETACH)` — a second, independent defence that D1
records but does not connect to this risk.

**So what is left that genuinely requires step 11?** The strongest case is **bind-mounted
volumes** (§10.1 step 16). A bind taken from a *shared* host mount is itself **shared**
(`clone_mnt`, `IS_MNT_SHARED(old)` → `list_add(&mnt->mnt_share, &old->mnt_share)`), regardless of
how private its destination parent is. So without step 11, `-v /host/x:/ctr/x` hands the
container a shared mount through which its own `mount`/`umount` calls propagate straight back to
the host. With `/` made `rslave` first, the source is a slave, the bind is a slave of the same
master, and outbound propagation is blocked while inbound still works.

`[R]` **Keep `MS_REC|MS_SLAVE` on `/` as step 11, required** — but record the real reasons:
(1) bind-mounted volumes inherit the *source's* peer-group membership, so without it every
volume is a two-way channel; (2) any mount we make *outside* the rootfs subtree leaks; (3) it is
defence-in-depth for the old-root detach, alongside the §5.3 old-root `rslave`; (4) it is the
knob the OCI `linux.rootfsPropagation` field maps onto, so it has to exist.
Note that for an MVP with **no volumes and no mounts outside the rootfs**, step 11 is strictly
redundant given steps 12/13 and §5.3 — it is cheap insurance, not a load-bearing wall.
And record that step 12 is **not** redundant decoration: it is what actually makes the
container's own mount set non-propagating, and it must run **before** step 13's self-bind,
because `clone_mnt` copies propagation from the source at bind time.

### 1.3 `MS_SLAVE` versus `MS_PRIVATE` — **DISPUTED, and D1's reasoning is too coarse**

D1 §4.2: "slave still blocks outbound propagation ... while *keeping* inbound propagation, so
host mount changes ... still reach the container's view", citing a runc devicemapper race
(`[C]` MED, unverified). D1 open question 8 leaves it open.

What the kernel actually gives you, `clone_mnt` L1320-L1334 again:

| source mount state | bind/clone of it becomes | inbound propagation to the copy? |
|---|---|---|
| shared | shared (same peer group) | yes, and outbound too |
| **slave** (after `MS_REC\|MS_SLAVE` on `/`) | **slave of the same master** | **yes, inbound only** |
| **private** (after `MS_REC\|MS_PRIVATE`) | **private** | **no** |

So the slave-vs-private choice is **decided at every later bind**, not once on `/`. Concretely:

- The container's **own** `/`, `/proc`, `/dev`… are private either way, because step 12 makes
  the rootfs's parent private before the self-bind. The slave setting on `/` does nothing for
  them.
- The difference is entirely about **user bind mounts / volumes** (§10.1 step 16). With
  `rslave`, a host volume bound into the container is a slave of the host's peer group, so a
  later host-side `mount`/`umount` under that volume shows up inside the container. With
  `rprivate` it does not, and the container keeps a stale view of a subtree the host has torn
  down — which is precisely the class of bug runc's comment describes.

**Verdict: DISPUTED → resolved in favour of `MS_REC|MS_SLAVE`, but for the volume-lifecycle
reason only, and the choice is only observable once we implement `-v`.** For an MVP with no
bind mounts, `rslave` and `rprivate` are behaviourally identical after step 12, so this is not
an MVP decision at all. D1's framing ("the runtimes genuinely disagree", open question 8)
overstates the stakes for us. The cheap correct answer: default `MS_REC|MS_SLAVE`, make it one
configurable value (OCI `linux.rootfsPropagation`), stop thinking about it.

One inbound-propagation hazard D1 does not mention, worth a line in the spec: `rslave` leaves
the container's view of *everything still mounted under the old root* subscribed to host mount
events until step 17 detaches it. That window is short and ends at `MNT_DETACH`; it is a reason
not to defer the pivot, not a reason to pick private.

---

## Part 2 — §5 `pivot_root` preconditions — **CONFIRMED**, with three additions

D1 §5.1 quotes `pivot_root(2)` verbatim; the quote matches the 2026-10-23 man7 text exactly.
Checked clause by clause against `torvalds/linux@v6.11/fs/namespace.c#L4301-L4395`:

| D1's stated precondition | kernel line | verdict |
|---|---|---|
| `CAP_SYS_ADMIN` in the userns owning the caller's **mount** ns | `may_mount()` → `ns_capable(mnt_ns->user_ns, CAP_SYS_ADMIN)`, L4308, `-EPERM` | CONFIRMED |
| `new_root`/`put_old` must be directories | `LOOKUP_DIRECTORY` in both `user_path_at`, L4311/L4316 | CONFIRMED (`-ENOTDIR`) |
| `new_root` must be a mount point, can't be `/` | `!path_mounted(&new)` → EINVAL; `new_mnt == root_mnt` → **`-EBUSY`** | CORRECTED (errno) |
| `put_old` at or under `new_root` | `is_path_reachable(old_mnt, old.dentry, &new)`, L4367 | CONFIRMED |
| no shared propagation | the three `IS_MNT_SHARED` tests, L4338 (see Part 1.2 for *which* three) | CONFIRMED, refined |
| current root must be a mount point | `!path_mounted(&root)` | CONFIRMED |

**Addition 1 — the errno for "new_root is the current root" is `EBUSY`, not `EINVAL`.**
`if (new_mnt == root_mnt || old_mnt == root_mnt) goto out4;` is preceded by `error = -EBUSY`.
D1 §5.2's C5 line records "`new_root == current root (/)` → FAIL EINVAL". Either the probe hit
a different clause first (it would: with `new_root == "/"`, `!path_mounted` and the
`is_path_reachable` tests are reached only after the shared tests) or the transcription is
wrong. Low impact, but our error mapping should not promise `EINVAL` here.

**Addition 2 — two preconditions D1 does not list at all**, both of which an attacker-supplied
rootfs can trigger: `if (new_mnt->mnt.mnt_flags & MNT_LOCKED) goto out4;` (EINVAL) and
`if (d_unlinked(new.dentry)) goto out4;` (**`-ENOENT`** — the new root directory was unlinked
between lookup and pivot). Also `!check_mnt(root_mnt) || !check_mnt(new_mnt)`: both mounts must
belong to the **caller's** mount namespace, which is why the pivot must be done by the child,
not by the parent on the child's behalf.

**Addition 3 — `MNT_DETACH` leaves exactly one thing observable, and it is the escape.**
`[F]` HIGH. After `umount2(put_old, MNT_DETACH)` the mounts leave the namespace's mount tree
(they vanish from `/proc/self/mountinfo`) but the `struct mount`s and their superblocks live
until the last reference drops. **Any fd held across the pivot keeps its object reachable**, and
an `O_PATH`/`O_DIRECTORY` fd on the old root is a complete escape: `openat(oldroot_fd, "etc/…")`
still resolves, because path resolution from a dirfd does not consult the mount tree for the
starting point. This is the mechanism of CVE-2024-21626 and it is why §10.1 step 30 exists.
D1 states the positive half ("hold it as an fd, never as a path") but never states the negative:
**the `oldroot` and `rootfd` `O_PATH` descriptors from the §5.3 sequence are themselves escape
capabilities and must be closed before `execve`, not merely marked cloexec at step 30.** Marking
cloexec is sufficient *if nothing between step 17 and step 33 execs or leaks them*, which is an
invariant no one will remember in a year. Close them explicitly at the end of step 17.

**§5.5 `chroot` escape — CONFIRMED as mechanism, not folklore.** `chroot(2)`: "This call does
not change the current working directory... This call does not close open file descriptors, and
such file descriptors may allow access to files outside the chroot tree." The `fchdir(fd)` +
`chdir("..")×N` + `chroot(".")` sequence D1 measured is the documented consequence, not lore;
the `..` behaviour is `follow_dotdot()` stopping at the *process's* root only when the cwd is
inside it. CONFIRMED.

---

## Part 3 — §9.1 namespace tiers, and §0.1's "6 of 8" — **two CORRECTED rows**

### 3.1 `CLONE_NEWTIME` = "Docker convenience — Docker doesn't use it" — **CORRECTED (upstream moved)**

`[C]` HIGH, 2026-10-23 — **Docker Engine 29.5.0 (released 2026-05-14) enables a private time
namespace for every container by default.** Release notes, *New*: "Enable private time namespace
for containers by default on supported kernels"; *Bug fixes and enhancements*: "Add
`time-namespaces` feature flag to disable time-namespaces."
(https://docs.docker.com/engine/release-notes/29/ — latest listed 29.9.2, 2026-10-20.)

So D1 §0.1's headline — *"Docker's default is 6 of 8 namespaces"* — and §9.1's `CLONE_NEWTIME`
row are both stale. On current Docker it is **7 of 8**. D1's measurement was correct for the
version it measured (DROPLET still runs **28.3.1**, confirmed `[O]` 2026-10-23
`docker version` → `28.3.1 api=1.51`), so this is drift, not an error. The MVP consequence is
nil — a time namespace with zero offsets changes no observable behaviour — but the *claim*
"nobody asks for it" is no longer safe to repeat, and the compatibility story changes if Docker
is also setting a boottime offset (unknown; see "Still unverifiable").

### 3.2 `CLONE_NEWUSER` = "hardening, not Docker's default" — **CONFIRMED on fact, DISPUTED on reasoning**

**Fact: CONFIRMED, still true in Docker 29.** `[C]` HIGH — `--userns-remap` remains an opt-in
*daemon-wide* flag (docs.docker.com/engine/security/userns-remap: "You can start `dockerd` with
the `--userns-remap` flag", per-container escape hatch `--userns=host`), and the 29.x notes
treat it as a conditional path throughout ("when userns-remap is enabled"), including 29.0.0's
"the containerd image store is temporarily unavailable when userns remapping is enabled" —
i.e. enabling it still costs you the new default image store. `[O]` DROPLET 2026-10-23,
`docker info --format '{{json .SecurityOptions}}'` → `["name=apparmor","name=seccomp,profile=builtin","name=cgroupns"]`
— no `name=userns`. Nothing moved here.

**Reasoning: DISPUTED.** D1 §0.1 defends the omission with *"That is Docker's actual default
posture, so we are not doing worse than the incumbent."* That is false at the MVP milestone D1
itself defines. Docker's no-userns default is survivable **because it is paired with** a
14-capability bounding set, a loaded seccomp filter, and an AppArmor profile — all three
measured by D1 itself in §7.2. D1's §10.1 marks capability drop (step 26), seccomp (25/32) and
LSM labelling (24) as "later (D5)" and **not MVP**. An MVP with six namespaces, full root
capabilities, no seccomp and no LSM is not "Docker's posture"; it is `docker run --privileged`
minus the device cgroup. The honest statement for the spec is: *the MVP is a root-equivalent
sandbox and must be labelled as such until D5 lands; the first hardening increment that buys
the most is the capability bounding-set drop, not the user namespace.*

### 3.3 `CLONE_NEWCGROUP` = "hardening — path hiding only; controls nothing" — **CORRECTED**

Two things it does that are not path hiding:

1. **It is what makes a correctly-rooted `/sys/fs/cgroup` possible inside the container.**
   `[F]` HIGH `cgroup_namespaces(7)`: "we must remount the ... cgroup filesystem from the new
   shell (i.e., perform the mount from a process that is in the new cgroup namespace)". §6.7
   therefore *depends* on §9.1's "hardening" row — a tier inversion.
2. **With the `nsdelegate` mount option it is a real migration boundary.** `[F]` HIGH
   `torvalds/linux@v6.11/Documentation/admin-guide/cgroup-v2.rst` ("Delegation Containment"):
   "For delegations to namespaces, containment is achieved by requiring that both the source and
   destination cgroups are reachable from the namespace of the process which is attempting the
   migration. If either is not reachable, the migration is rejected with `-ENOENT`." The
   DROPLET's live cgroup2 is mounted `nsdelegate` (D1's own §6.7 transcript). So a container
   with a cgroup namespace **cannot** write pids into cgroups outside its namespace root even
   with a writable cgroupfs. D1 §1.7's "prevent escape to a parent cgroup … path-hiding
   mechanism, full stop" is wrong on a `nsdelegate` host.

Tier should read **strongly recommended** (prerequisite for `/sys/fs/cgroup`, plus a real
containment property), not "hardening".

### 3.4 The rest of §9.1 — **CONFIRMED**

`CLONE_NEWNS`/`CLONE_NEWPID`/fresh-`/proc` as required, `CLONE_NEWNET` required for isolation,
`CLONE_NEWIPC`/`CLONE_NEWUTS` strongly recommended: all check out against `namespaces(7)` and
against the §1 subsections. Nothing to correct.

---

## Part 4 — §10.1's 33 steps: ordering by kernel rule vs. convention — **one step cannot work where D1 puts it**

### 4.1 **Step 8 is wrong: `CLONE_NEWCGROUP` and `CLONE_INTO_CGROUP` in the same `clone3` do not compose** — CORRECTED, HIGH

`[F]` HIGH — `torvalds/linux@v6.11/kernel/cgroup/namespace.c#L75`, `copy_cgroup_ns()`:

```c
	/* It is not safe to take cgroup_mutex here */
	spin_lock_irq(&css_set_lock);
	cset = task_css_set(current);      /* <-- the CALLER's cgroup, i.e. the parent's */
	get_css_set(cset);
	spin_unlock_irq(&css_set_lock);
	...
	new_ns->root_cset = cset;
```

The new cgroup namespace is rooted at **`current`'s** css_set — the *cloning parent's* cgroup —
evaluated inside `copy_namespaces()`, independent of `cgroup_can_fork()`/`CLONE_INTO_CGROUP`,
which attach the *child* (`p`). For an ordinary `clone` the two are the same cgroup, so nobody
notices. The moment you add `CLONE_INTO_CGROUP` they diverge, and the container's cgroup
namespace root becomes the **engine's** cgroup, one or more levels above the container's.

Observable consequences, all bad: `/proc/self/cgroup` inside the container reads
`0::/<container-leaf>` instead of `0::/`; a `mount -t cgroup2` inside exposes the engine's whole
subtree — **including sibling containers' cgroups** — instead of just its own; and with
`nsdelegate`, the containment boundary is drawn around the engine's cgroup, not the container's.
This directly contradicts D1's **own** §3.5 rule 5 ("cgroup namespace after cgroup placement").
`cgroup_namespaces(7)`'s wording ("its current cgroups directories become the cgroup root
directories") reads as if the child's cgroup is used, which is exactly the trap.

`[C]` HIGH — runc does it in the only order that works:
`opencontainers/runc@v1.5.1/libcontainer/process_linux.go#L822-L825`, with the comment *"Do this
before syncing with child so that no children can escape the cgroup"*, calls
`p.manager.Apply(p.pid())` **before** writing the bootstrap data that makes `nsexec` unshare
anything; `nsexec.c` then does `try_unshare(config.cloneflags, "remaining namespaces")`
(L1107) — cgroup namespace included — from a process *already in the target cgroup*.

**Corrected step 8 / new step 8c.** Keep `CLONE_INTO_CGROUP` in the `clone3` (it is the only way
to close the unconstrained window — D1 §3.2 is right about that), but **drop `CLONE_NEWCGROUP`
from the clone flags** and have the child `unshare(CLONE_NEWCGROUP)` as its first action after
the clone, before step 14 mounts `/sys/fs/cgroup`. The child is in the target cgroup from its
first instruction and holds `CAP_SYS_ADMIN`, so the unshare is one syscall and cannot fail for
privilege reasons. The fallback path (legacy `clone` + parent writes `cgroup.procs`) must
additionally **wait for the parent's write to land** before unsharing — that is a second use for
the sync socketpair of step 5.

### 4.2 Which §10.1 orderings are kernel-enforced and which are convention

| Constraint | Status | Basis |
|---|---|---|
| namespaces created *by* the clone, not after | **kernel** | `clone(2)`; only `unshare`/`setns` paths differ |
| cgroup placement before the child can fork | **kernel-ish** (race, not error) | nothing stops the child forking first; `CLONE_INTO_CGROUP` makes it atomic |
| **cgroup placement before cgroup-ns creation** | **kernel** | `copy_cgroup_ns` uses `current`'s cset (§4.1) |
| propagation change on `/` before any mount | **kernel for the pivot, policy otherwise** | three `IS_MNT_SHARED` tests, `fs/namespace.c#L4338` (see Part 1) |
| rootfs-parent `MS_PRIVATE` **before** the self-bind | **kernel** | `clone_mnt` copies propagation from the source at bind time (`fs/namespace.c#L1320`) |
| `/proc` mounted by a process already in the new **PID** ns | **kernel** | `proc_init_fs_context` pins `task_active_pid_ns(current)` (`fs/proc/root.c#L252`) |
| sysfs needs the new **net** ns, mqueue the new **ipc** ns — *in a userns only* | **kernel** | `mount_capable` → `ns_capable(fc->user_ns, …)` (`fs/super.c#L690`) |
| all mounts before `umount2(put_old, MNT_DETACH)` | **kernel** | nothing outside the new root has a pathname afterwards |
| `chdir("/")` after `pivot_root` | **kernel** | `pivot_root(2)`; kernel only moves *other* tasks' cwd |
| `/dev/pts` mounted before `setsid`+`TIOCSCTTY` | **kernel** | you cannot open a slave from an unmounted devpts |
| `/dev` populated before `/dev/null`-masking of `/proc/*` | **kernel** | the bind source must exist |
| `/proc` mounted before `/dev/fd`→`/proc/self/fd` is *usable* | **kernel at use time, not at creation** | `symlink(2)` does not resolve its target; D1 §6.2's "symlinks depend on /proc" is only true at dereference |
| `PR_SET_NO_NEW_PRIVS` before seccomp **unless** `CAP_SYS_ADMIN` | **kernel** | `seccomp(2)` `EACCES` |
| bounding-set drop before the uid change | **convention** | the drop is irreversible either way; the real rule is "after everything that needs the cap" |
| `PR_SET_KEEPCAPS` around `setuid` | **kernel** | `capabilities(7)`: uid 0→non-0 clears permitted/effective without it |
| remaining cap sets applied **after** the uid change | **kernel** | same clearing rule |
| seccomp last (NNP branch) | **convention, strongly motivated** | every syscall after it must be in the profile |
| netns plumbing before the child drops caps | **convention** (it is a different process) | the parent's caps are unaffected by the child's drop; the real constraint is "before the workload runs" |
| `close_range` sweep last before `execve` | **convention + CVE** | nothing kernel-enforced; discipline |

Three D1 claims in that table that are **overstated as kernel rules**:
- §3.5 rule 8 / step 10, "netns plumbing … must finish before the child drops capabilities":
  the plumbing is done by the **parent**, whose capabilities the child's drop cannot affect.
  The true constraint is that it must finish before the child `execve`s the workload — which is
  what the step-31 sync point is for. Harmless, but do not encode the wrong reason.
- §3.5 rule 10, "bounding set dropped before the uid change": convention. Dropping the bounding
  set does not touch permitted/effective; the ordering that *is* enforced is KEEPCAPS around
  the uid change and `capset` after it.
- §10.1 step 19 (`maskedPaths`) "must follow step 14": true, and additionally **must precede any
  `execve`** — but note masking is per-superblock, not per-path (Part 5.3), so a container that
  can mount a second procfs walks around all of it.

### 4.3 `uid_map`/`gid_map`: D1 says the child "cannot" write its own — **CORRECTED**

D1 §10's defect list, item 2: *"'UID/GID configuration' after the clone in the child is
impossible. The child cannot write its own `uid_map` — that is the whole reason an intermediate
process and a sync pipe exist."* §3.3 reason 1 repeats it.

`[F]` HIGH — `torvalds/linux@v6.11/kernel/user_namespace.c#L1163` (`new_idmap_permitted`):

```c
	if ((new_map->nr_extents == 1) && (new_map->extent[0].count == 1) &&
	    uid_eq(ns->owner, cred->euid)) {
		u32 id = new_map->extent[0].lower_first;
		if (cap_setid == CAP_SETUID) {
			kuid_t uid = make_kuid(ns->parent, id);
			if (uid_eq(uid, cred->euid))
				return true;     /* self-map of own euid: allowed */
```

A process that has just `unshare(CLONE_NEWUSER)`d **can** write its own `uid_map`, provided the
map is a **single extent of length 1 mapping its own euid** — this is exactly what
`unshare --map-root-user` and `podman unshare` do, and D1's §6.1/§6.2 ROCKY probes used that
path. What it cannot do is write a **multi-extent / length>1** map: that needs
`ns_capable(ns->parent, CAP_SETUID) && file_ns_capable(file, ns->parent, CAP_SETUID)`, i.e. a
privileged helper (`newuidmap`) in the parent namespace.

So the intermediate process exists for **`/etc/subuid`-style multi-ID maps**, not for id-mapping
as such. A rootless container with a *single* uid mapped needs no third process. That changes
D5's rootless design space: there is a legitimate intermediate rung between "root MVP" and "full
subuid rootless" that costs no new process topology.

Two further writer rules D1 does not state, both of which produce confusing errnos:
- the writer must hold `CAP_SYS_ADMIN` **in the target user namespace**, checked against the
  credentials of the process that **opened** the file (`file_ns_capable(file, map_ns,
  CAP_SYS_ADMIN)`, L977) — so an fd opened before the unshare is useless;
- mapping **parent uid 0** additionally requires `CAP_SETFCAP` in the parent (`verify_root_map`,
  L881; for the self-map case, the *unsharing* process must have had it). A map that works as
  real root fails for a semi-privileged helper, with `EPERM` and no explanation.

`uid_map`/`gid_map` are also **write-once**, the whole write must be a single `write(2)` of less
than one page at offset 0, and the line limit is **5 up to Linux 4.14, 340 since Linux 4.16**
(`user_namespaces(7)`).

---

## Part 5 — §6 minimum mount set: `/proc` and the rest — **CONFIRMED with mechanism; two corrections**

### 5.1 The actual rule for mounting `proc` (D1's open question in §6.1, settled)

`[F]` HIGH, two independent kernel facts, neither stated in D1:

1. **Which PID namespace the instance shows is decided by the mounting process, not by the mount
   point.** `torvalds/linux@v6.11/fs/proc/root.c#L252`:
   `ctx->pid_ns = get_pid_ns(task_active_pid_ns(current)); fc->user_ns = get_user_ns(ctx->pid_ns->user_ns);`
   There is **no way to ask for a different PID namespace** — procfs always binds to the caller's
   active one. Consequences: (a) the `/proc` mount of §10.1 step 14 **must** be performed by the
   child `C`; if the parent pre-mounted `proc` into the rootfs the container would see host PIDs,
   which is D1 §1.2's "164 processes" failure wearing a different hat; (b) crun's trick of
   holding an `fsopen("proc")` handle across the pivot only works if the `fsopen` was issued from
   inside the target PID namespace.
2. **The permission check is `ns_capable(pid_ns->user_ns, CAP_SYS_ADMIN)`**, via
   `mount_capable()` (`torvalds/linux@v6.11/fs/super.c#L690`:
   `if (!(fc->fs_type->fs_flags & FS_USERNS_MOUNT)) return capable(CAP_SYS_ADMIN); else return ns_capable(fc->user_ns, CAP_SYS_ADMIN);`),
   and procfs sets `fc->user_ns` to the owner of the PID namespace. So: **a new PID namespace is
   not required to mount procfs — what is required is that the PID namespace you are in is owned
   by a user namespace in which you hold `CAP_SYS_ADMIN`.** For our root-first MVP (initial user
   namespace, real root) that is satisfied unconditionally. D1 §6.1's heading
   ("mounting procfs in a user namespace requires a new PID namespace") is true only inside a
   user namespace and only because the *initial* PID namespace is owned by the *initial* user
   namespace; it is not a general rule, and §9.1's "`CLONE_NEWPID` … also enables procfs mount in
   a userns" should carry that scope.

The same shape, now source-backed, settles two D1 `[O]`-only claims and upgrades them to `[F]`:

| filesystem | `fc->user_ns` is taken from | source |
|---|---|---|
| `proc` | the owner of the caller's **PID** namespace | `fs/proc/root.c#L252` |
| `sysfs` | the owner of the caller's **net** namespace, plus an explicit `kobj_ns_current_may_mount(KOBJ_NS_TYPE_NET)` → `-EPERM` | `fs/sysfs/mount.c#L~40` |
| `mqueue` | the owner of the caller's **IPC** namespace | `ipc/mqueue.c` `mqueue_init_fs_context` |
| `devpts`, `tmpfs` | the mount namespace's owner (plain `FS_USERNS_MOUNT`) | `fs/devpts/inode.c#L503` |

D1 §6.6's "This one surprised me and is not obvious from `user_namespaces(7)`'s bullet list" is
correct **and the man page is wrong**: `user_namespaces(7)` lists `/sys` and `mqueue` under the
*mount*-namespace clause, and only calls out `/proc` (PID ns) and cgroup2 (cgroup ns) separately.
That is a documentation bug worth the same treatment D1 gave the `time_namespaces(7)` one.

### 5.2 `hidepid=`, `subset=pid`, `gid=` — **measured; partially redundant with masking, not adoptable**

`[O]` DROPLET 6.11.0-29, 2026-10-23, inside a throwaway `unshare -m --propagation private
--fork --pid` with scratch dirs under `/tmp` (host `mountinfo` verified clean afterwards, dirs
removed):

```
mount -t proc -o subset=pid proc /tmp/d1v1-proc
ENTRIES(subset=pid): 1 3 4 self thread-self
  present: self, thread-self
  ABSENT : meminfo cpuinfo stat uptime loadavg kcore sys mounts net version cmdline sysrq-trigger
  readlink self -> 5 ; /proc/self/status and /proc/self/stat fully readable
```

- **`subset=pid` does make most of `maskedPaths`/`readonlyPaths` redundant**: `/proc/kcore`,
  `/proc/keys`, `/proc/latency_stats`, `/proc/timer_list`, `/proc/sched_debug`,
  `/proc/sysrq-trigger`, `/proc/acpi`, `/proc/scsi`, `/proc/bus`, `/proc/fs`, `/proc/irq` and
  `/proc/sys` are *all* non-pid top-level entries and all disappear. One mount option replaces
  about fifteen bind mounts.
- **It is not adoptable as a default**, because it also removes `/proc/meminfo`, `/proc/cpuinfo`,
  `/proc/stat`, `/proc/uptime`, `/proc/loadavg`, `/proc/mounts`, `/proc/net`, `/proc/version`,
  `/proc/cmdline` and all of `/proc/sys`. `free`, `top`, `uptime`, `nproc`'s fallback, every
  sysctl, and the host-vs-container memory heuristics of the JVM/.NET/Node break. `/proc/self/*`
  survives intact, so the `/proc/self/maps`, `/proc/self/cgroup` and `/proc/self/status` consumers
  D1 §6.1 lists keep working — the damage is confined to the *global* files.
  `[R]` offer it as an opt-in hardening mode (`--proc-subset=pid`), never as the default.
- **`hidepid=` is close to worthless for us.** It hides other processes' `/proc/<pid>` from users
  who do not own them; inside a private PID namespace there are almost no other processes, and
  container root (uid 0, `CAP_SYS_PTRACE`-less but owner-matching) sees its own tree anyway. It
  is a shared-`/proc` mitigation, i.e. for the `--pid=host` case we do not support.
- **`gid=`** only names a group exempt from `hidepid`; it has no masking value. Note it is
  interpreted in the **initial** user namespace on display (`from_kgid_munged(&init_user_ns,…)`),
  so do not expect it to mean the same thing under a userns.
- **`max=`** (devpts) is the one undocumented-in-D1 option with real value: it caps ptys per
  instance, a cheap DoS bound. Nothing in D1 or in Docker's default spec sets it.

### 5.3 The masking caveat D1 §6.8 does not state — **masking is per-superblock**

`[O]` DROPLET 6.11.0-29, same run: mounting a **second, plain** procfs inside the same namespace
succeeded and produced a *different superblock* (`0:62` with `subset=pid` vs `0:67` plain), with
`/proc/kcore` present in the second. `[F]` HIGH — procfs uses `get_tree_nodev()`
(`fs/proc/root.c`), i.e. a fresh superblock per mount; `maskedPaths` are bind mounts over paths
*in one instance*. Therefore: **`maskedPaths`/`readonlyPaths` (and `subset=pid`) are not a
boundary against a container that can still call `mount(2)`.** They are only meaningful in
combination with dropping `CAP_SYS_ADMIN` (or seccomp-blocking `mount`). D1 §6.8 presents them
as hardening with no such precondition; D5 must state the dependency.

### 5.4 Rest of the mount set — **CONFIRMED**

- `/dev` tmpfs `nosuid,mode=755,size=65536k`, the six device nodes, the `/dev/fd`,`/dev/std*`
  symlinks, `/dev/shm` `mode=1777`, `/dev/mqueue`, `/sys` ro: all match the OCI/runc defaults and
  D1's live-container measurement. The `/dev`-must-not-be-`nodev` and `/`-is-not-`nosuid`
  asymmetries are real and correctly explained.
- **`newinstance` is confirmed dead, from source.** `torvalds/linux@v6.11/fs/devpts/inode.c`:
  `case Opt_newinstance: break;` — parsed, ignored — and `devpts_show_options()` never emits it.
  D1 open question 3 is settled on behaviour; the exact removal commit remains unchecked and does
  not matter.
- One ordering nuance in §6.2: "the symlinks depend on `/proc` being mounted — another ordering
  coupling" is **not** a creation-time constraint. `symlink(2)` does not resolve its target, so
  `/dev/fd → /proc/self/fd` can be created before `/proc` exists; only *dereferencing* needs it.
  Harmless, but it is not an ordering edge in step 14/15.

---

## Part 6 — §7 PID 1 semantics — **CONFIRMED, mechanism now read from source (D1 open question 4 settled)**

### 6.1 Exactly which signals a namespace init ignores

D1 §7.1 quotes `pid_namespaces(7)` correctly and measures the behaviour correctly in all three
directions. The implementation rule is sharper than the man page and is worth having in the spec,
because it decides what our `stop`/`kill` path may assume.
`[F]` HIGH — `torvalds/linux@v6.11/kernel/signal.c#L82` (`sig_task_ignored`) and `#L1215`
(`send_signal_locked`):

```c
	if (unlikely(t->signal->flags & SIGNAL_UNKILLABLE) &&
	    handler == SIG_DFL && !(force && sig_kernel_only(sig)))
		return true;                         /* ignored */
```
```c
	if (info == SEND_SIG_NOINFO)      force = !task_pid_nr_ns(current, task_active_pid_ns(t));
	else if (info == SEND_SIG_PRIV)   force = true;
	else if (has_si_pid_and_uid(info)) {
		force = (info->si_code == SI_KERNEL);
		if (!task_pid_nr_ns(current, task_active_pid_ns(t))) { info->si_pid = 0; force = true; }
	}
```

So, for a process that is PID 1 of any PID namespace (`SIGNAL_UNKILLABLE`, set in
`kernel/fork.c#L2556-L2558` for `is_child_reaper(pid)`):

| sender | signal disposition | result |
|---|---|---|
| anyone (incl. in-namespace) | a handler is installed | **delivered** — this is why an init process works at all |
| in-namespace, or a *descendant* namespace | `SIG_DFL`, any signal incl. `SIGKILL` | **discarded** (`task_pid_nr_ns` is non-zero, so `force` stays false) |
| **ancestor / unrelated** namespace | `SIG_DFL`, `SIGKILL` or `SIGSTOP` | **delivered** |
| ancestor namespace | `SIG_DFL`, anything else (`SIGTERM`…) | **discarded** |
| **the kernel itself** (`SEND_SIG_PRIV`, `si_code == SI_KERNEL`) | `SIG_DFL`, `SIGKILL`/`SIGSTOP` | **delivered** |

The "descendant namespace" row is not in D1 and is not obvious: a process in a *nested*
container cannot kill its ancestor's init either, because it still has a PID in that namespace.

**The kernel-originated row matters for D5 and corrects a D1 recommendation.** The cgroup OOM
killer and `cgroup.kill` both signal with `SEND_SIG_PRIV`, so **they do kill container PID 1**.
D1 §7.4 says "No `cgroup.kill` sweep is needed for correctness (still do it for defence in
depth)" — true, but the stronger and more useful statement is that `cgroup.kill` is a *complete
alternative* teardown that works even when PID 1 is a process we cannot name (pid reuse,
`CLONE_NEWPID` nesting), and it is the only thing that kills a container whose init has somehow
blocked in uninterruptible state. (`is_global_init` is the one exception in the opposite
direction: the *host's* PID 1 is unkillable even by a forced `SIGKILL`.)

**D1 open question 4 — settled.** The generated-vs-forced distinction D1 inferred from behaviour
is implemented in **`force_sig_info_to_task()`**, `kernel/signal.c#L1326`, not in
`sig_task_ignored` alone:

```c
	if (action->sa.sa_handler == SIG_DFL &&
	    (!t->ptrace || (handler == HANDLER_EXIT)))
		t->signal->flags &= ~SIGNAL_UNKILLABLE;
```

A synchronous fault (`force_sig_info`, e.g. the real null dereference D1 measured) **clears the
`SIGNAL_UNKILLABLE` flag outright** before sending, which is why a faulting init dies while
`kill -SEGV $$` is dropped. Note the ptrace exception: a *traced* init keeps the flag, so a
debugger does not make init killable by accident.

### 6.2 PID 1 exit — the guarantee is stronger than D1 states

`[F]` HIGH — `torvalds/linux@v6.11/kernel/pid_namespace.c#L170` (`zap_pid_ns_processes`):
`disable_pid_allocation(pid_ns)` first (this is the `ENOMEM`-on-later-`fork` D1 measured), then
`group_send_sig_info(SIGKILL, SEND_SIG_PRIV, task, PIDTYPE_MAX)` to **every pid in the
namespace** — which includes processes in *descendant* PID namespaces — and then the exiting
init **blocks until every other pid in the namespace is gone**:

```c
	for (;;) {
		set_current_state(TASK_INTERRUPTIBLE);
		if (pid_ns->pid_allocated == init_pids)
			break;
		schedule();
	}
```

Therefore: **when the engine's `waitpid`/pidfd on the container's init reports exit, the PID
namespace is already empty.** No polling, no "did the children die" check, no race with a
subsequent cgroup rmdir. D1 §7.4's `[R]` HIGH ("useful cleanup guarantee") is CONFIRMED and can
be stated much more strongly than it is. The one thing that does *not* disappear is the namespace
*object*: it lives as long as anything holds a reference (an nsfs fd, a bind-mounted
`/proc/<pid>/ns/pid`, a `setns`ed process in a descendant). That is a leak our state tracking can
cause and the kernel will not clean up.

### 6.3 Orphan reparenting and `PR_SET_CHILD_SUBREAPER` — **CONFIRMED, with the boundary rule D1 omits**

`[F]` HIGH — `torvalds/linux@v6.11/kernel/exit.c#L625` (`find_new_reaper`): the search order is
(1) **another live thread in the dying parent's own thread group**; (2) the nearest ancestor with
`is_child_subreaper`, walking `real_parent` **only while `task_pid(reaper)->level == ns_level`**;
(3) the PID namespace's `child_reaper`, i.e. that namespace's PID 1.

Two consequences, neither in D1:
- **A subreaper can never adopt across a PID-namespace boundary.** Setting
  `PR_SET_CHILD_SUBREAPER` on our engine does **nothing** for orphans inside a container; they go
  to container PID 1 regardless. The flag is only useful for the `--pid=host`-shaped case where a
  container has no PID namespace — there it makes the engine, not host PID 1, the reaper.
- Multithreaded parents reparent to a sibling thread first, so "orphans go to init" is only true
  when the whole thread group is gone.

### 6.4 Do we need an in-container init? — D1's design A is right; the reasons need tightening

Everything D1 §7.5 lists is confirmed. Precision to add, because it changes how the CLI should
behave rather than whether we build an init:

- The kernel delivers a signal to a namespace init **iff a handler is installed** (6.1). An init
  process is therefore not a workaround — it is the *only* mechanism that makes `SIGTERM`,
  `SIGHUP`, `SIGUSR1` deliverable to a container at all.
- Without an init, **`mydocker stop` on the common `sh -c`/`sleep` entrypoint always costs the
  full grace period** and then kills. That is not a corner case; it is the default experience.
- Without an init, **exit-status reporting is unreliable in a specific way**: a process that
  "should" have died by signal may exit 0 instead (D1's `kill -SEGV $$` → rc 0). Our
  `state`/`inspect` output must not claim `ExitCode`/`Signal` fidelity we do not have.
- Zombie accumulation is genuinely a non-issue for the MVP's interactive/short-lived use case,
  as D1 says.
- An init is **not** needed for teardown (6.2) or for cgroup accounting. Those are the two
  reasons people usually give and they are both wrong.

---

## Part 7 — §11.2 the post-`clone`/pre-`execve` window — **CONFIRMED, and the reason is sharper than D1 gives**

(V7 owns the crate questions. This part is only the kernel/libc facts. I take V7's finding that
`rustix::io::close_range` does not exist as settled and do not revisit it.)

### 7.1 The malloc deadlock: **CONFIRMED, but the hazard is specific to a *raw* `clone`, not to "fork"**

`[F]` HIGH — `bminor/glibc@glibc-2.40/posix/fork.c#L60-L109`. glibc's `fork()` is **not** a thin
wrapper. When `multiple_threads` is set it does, in order: `_IO_list_lock()`,
`__malloc_fork_lock_parent()` (which takes `list_lock` and then **every arena mutex**,
`malloc/arena.c#L176-L185`), then `_Fork()`; and in the child it calls
`__malloc_fork_unlock_child()`, `fresetlockfiles()`, `_IO_list_resetlock()`,
`__rtld_lock_initialize(GL(dl_load_lock))`, `__rtld_lock_initialize(GL(dl_load_tls_lock))`,
`reclaim_stacks()`, and the registered `pthread_atfork` child handlers.

**None of that runs for `syscall(SYS_clone3, …)`.** So D1's rule is right and its basis is
understated: the hazard is not "malloc has locks" (glibc repairs those for `fork()` users); it is
that **we are deliberately bypassing glibc's repair path**. Four inherited-lock hazards, not one:

| inherited state | what breaks | D1 mentions? |
|---|---|---|
| arena mutexes + `list_lock` | first `malloc` deadlocks | yes |
| libio list lock + per-`FILE` recursive locks | first `printf`/`fwrite` deadlocks | partly ("ReentrantLock") |
| **`GL(dl_load_lock)` / `dl_load_tls_lock`** | **any lazily-bound PLT symbol, `dlopen`, or first TLS access in a shared object can deadlock** | **no** |
| **`GL(dl_stack_used)` / `dl_stack_cache_lock`** (thread list) | `setxid` family, `pthread_*` | partly |

The `dl_load_lock` row is the one with teeth for us: a first call into a lazily-bound libc
function from the cloned child can take it. Mitigations are standard (`-Wl,-z,now` full RELRO, or
issuing raw syscalls so no PLT is involved) — that part is V7's.

The man-page statement of the same thing, which is the cleanest citation available:
`signal-safety(7)` (man-pages **6.19**, 2026-06-05) lists **`_Fork(3)` as async-signal-safe and
does not list `fork(2)` at all.** A raw `clone3` child is in exactly `_Fork`'s position: AS-safe
functions only, and nobody has repaired anything on your behalf.

### 7.2 The glibc `setxid` broadcast: **CONFIRMED, and it is conditional on the parent being multithreaded**

`[F]` HIGH — `bminor/glibc@glibc-2.40/nptl/nptl_setxid.c#L175-L245`. `__nptl_setxid()` takes
`GL(dl_stack_cache_lock)`, walks `GL(dl_stack_used)` and `GL(dl_stack_user)` calling
`setxid_mark_thread()` on every thread but itself, signals each with `SIGSETXID`, and then
**futex-waits on `cmdp->cntr` until every signalled thread acknowledges**, looping until no
thread is signalled. `setxid_mark_thread()` itself futex-waits when a thread entry is in the
"being cloned" state. In a raw-cloned child, `reclaim_stacks()` never ran, so that list still
names the parent's threads.

Two refinements to D1:
- **It only hangs if the parent had other threads at clone time.** Single-threaded parent ⇒ the
  list contains only `self` ⇒ `setuid` is a plain syscall. So D1's §3.4 `Threads == 1` assertion
  *already* neutralises this hazard; the raw-syscall rule is belt-and-braces. Keep both — the
  raw syscall costs nothing and the assertion can be defeated by a dependency — but do not
  describe the hang as unconditional.
- `getuid`-style tgkill failures are harmless (`setxid_signal_thread` ignores `ESRCH`); the hang
  comes from `setxid_mark_thread`'s futex and from `dl_stack_cache_lock` itself being inherited
  locked.

### 7.3 Inherited stdio buffers: **CONFIRMED**

D1's live observation (duplicated transcript blocks) is the textbook consequence: the child
inherits the parent's unflushed `FILE` buffers in its copied address space and flushes them at
its own `exit`. Primary-source corroboration that this is libio state, not an artefact:
`posix/fork.c` has to call `fresetlockfiles()` and `_IO_list_resetlock()` in the child precisely
because that state is shared. Rules that follow: flush/`fflush(NULL)` **before** the clone, and
leave the child via `execve` or `_exit`, never `exit`. D1's "no `println!` after the clone" covers
the write side; the **flush-before-clone** half is missing from §10.1 — it belongs in step 7.

### 7.4 `close_range(CLOSE_RANGE_CLOEXEC)`: **CORRECTED — the flag is Linux 5.11, not 5.9**

`[F]` HIGH — `close_range(2)`: "**CLOSE_RANGE_CLOEXEC (since Linux 5.11)** — Set the close-on-exec
flag on the specified file descriptors, rather than immediately closing them." The syscall itself
is 5.9. D1 §11.2 and §10.1 step 30 fold the two together ("`close_range(2)` (Linux 5.9) … and
with `CLOSE_RANGE_CLOEXEC` …"), and §2.1's probe tested only the **syscall**, not the flag.
This matters because **D5 §7.3 sets the hard kernel floor at 5.10** — on a 5.9/5.10 kernel the
syscall exists and the flag returns `EINVAL`, so the step-30 code as written fails on the stated
floor. Probe the flag, not the syscall.

`[O]` 2026-10-23, raw `syscall(436, 1000000, 1000001, flags)` on both hosts (empty fd range, no
side effects); `flags=8` used as a control to prove the kernel validates flags:

| host | kernel | `flags=0` | `UNSHARE(2)` | `CLOEXEC(4)` | `8` (bogus) |
|---|---|---|---|---|---|
| DROPLET | 6.11.0-29 | 0 | 0 | **0** | EINVAL |
| ROCKY | 4.18.0-425 | 0 | 0 | **0** | EINVAL |

So the RHEL-8 backport carries the 5.11 flag as well — another data point for D1 §2.1's "you
cannot infer syscall availability from `uname -r`", and for "you cannot infer *flag* availability
from syscall availability" either.

One ordering note from `close_range(2)`'s own rationale, which supports D1's step order: the
`CLOSE_RANGE_CLOEXEC` form exists so descriptors "can be marked before the `seccomp(2)` profile
is set up, and the profile can control access to `close_range()` without affecting the calling
process". Step 30 before step 32 is therefore correct and deliberate, not incidental.

---

## Part 8 — §2 (D1) versus §7 (D5): the kernel floor — **no factual contradiction; a scope conflict that costs D1 three fallback paths**

D1 §2/§2.1 never states a minimum kernel. It states a **policy** — `[R]` HIGH "runtime-detect
every optional syscall by calling it and checking for `ENOSYS`; never gate on a parsed kernel
version" — and then keeps fallbacks alive explicitly *for ROCKY-class 4.18 kernels*. D5 §7.3
states a **floor** — `[R]` HIGH hard 5.10, recommended 5.15, "**Explicitly do not support
4.18/RHEL 8**". Checked against each other:

- **The facts agree.** Every version in D5 §7.1 that overlaps D1 (`CLONE_PIDFD` 5.2,
  `pidfd_open` 5.3, `clone3` 5.3, `CLONE_INTO_CGROUP` 5.7, new mount API 5.2, `close_range` 5.9,
  idmapped mounts 5.12, cgroup ns 4.6) matches `namespaces(7)`/`clone(2)`/`close_range(2)`.
  D5's RHEL-8 claim is independently corroborated by D1 §2.1's own probe table: `clone3`
  **ENOSYS** on 4.18, everything else backported. The policy claims agree too — D5 §7.2's "never
  gate on `uname -r`" is D1 §2.1's `[R]` restated.
- **The scope conflicts, and D5 is right.** D1 justifies three fallbacks by ROCKY: the
  `clone`+`cgroup.procs` path "for pre-5.7 and cgroup v1 … you need it for ROCKY-class kernels
  anyway" (§3.2), the `kill(2)`-by-pid fallback "for ROCKY-class kernels" (§8.2), and
  `mount(2)`+two-pass-bind "because it works on every kernel including ROCKY" (§4.3). **At D5's
  floor every one of those justifications evaporates**: `clone3` (5.3), `CLONE_PIDFD` (5.2),
  `pidfd_send_signal` (5.1) and `CLONE_INTO_CGROUP` (5.7) are all below 5.10, and
  `mount_setattr` (5.12) is below 5.15. The legacy-`clone` path D1 calls "two code paths forever"
  should simply **not exist**.
- **D5's architectural consequence is confirmed.** A `clone3`-based design does exclude RHEL 8 —
  `clone3` is `ENOSYS` there (D1 §2.1, `[O]` HIGH), and `CLONE_INTO_CGROUP` and `CLONE_NEWTIME`
  are reachable *only* through `clone3` (D1 §1.8, confirmed: `CLONE_NEWTIME == 0x80` collides
  with legacy `clone`'s `CSIGNAL` mask). Supporting 4.18 is not "add a fallback", it is "keep a
  second process-creation architecture".

**Adjudication: adopt D5's floor (hard 5.10, documented/tested 5.15), and delete D1's pre-5.3
fallbacks.** Keep the `ENOSYS` probe policy — it is cheap and still needed *above* the floor, for
`mount_setattr` (5.12), `CLOSE_RANGE_CLOEXEC` (5.11, see Part 7.4), `statmount`/`listmount`
(6.8) and `OPEN_TREE_NAMESPACE`. At a 5.10 hard floor, `CLOSE_RANGE_CLOEXEC` is the *only* thing
in D1's MVP path that needs a probe-and-fallback; at 5.15 nothing does. That is a strong argument
for documenting 5.15 and being done with it.

One consequence for ROCKY as a research host: it remains useful as the **degradation probe** the
contract calls it, but D1 should stop deriving `[R]`s from it. Its value is "what breaks when a
feature is missing", not "what we must support".

---

## Part 9 — §3 process-creation primitives — **CONFIRMED, with one man-page error and two additions**

### 9.1 `clone_args` layout and size versioning — CONFIRMED from source

`[F]` HIGH — `torvalds/linux@v6.11/include/uapi/linux/sched.h#L107-L109` and
`kernel/fork.c#L2928-L2995` (`copy_clone_args_from_user`): `CLONE_ARGS_SIZE_VER0 = 64`
(`flags, pidfd, child_tid, parent_tid, exit_signal, stack, stack_size, tls`),
`VER1 = 80` (`+set_tid, set_tid_size`), `VER2 = 88` (`+cgroup`). Rules:
`usize > PAGE_SIZE` → `E2BIG`; `usize < 64` → `EINVAL`; `copy_struct_from_user` → `E2BIG` if any
byte beyond the kernel's known size is **nonzero** (forward compatibility is "zero means
default"); `exit_signal` must fit `CSIGNAL` and be a valid signal; `CLONE_INTO_CGROUP` requires
`usize >= 88` **and** `cgroup <= INT_MAX`. D1 §3.2's measured "accepts 64, 80 and 88" is exactly
the three published versions. Pass `sizeof(struct clone_args)` from a pinned UAPI definition and
let the kernel zero-extend; never pass a hand-rolled size.

### 9.2 `CLONE_PIDFD` — CONFIRMED, plus a stale restriction

`[F]` HIGH — `kernel/fork.c#L2176-L2183`: `CLONE_PIDFD | CLONE_DETACHED` → `EINVAL`;
`#L2758`: `CLONE_PIDFD | CLONE_PARENT_SETTID` → `EINVAL` (legacy `clone` only — they share the
`parent_tid` slot). **`CLONE_PIDFD | CLONE_THREAD` is now legal** (`#L2403`:
`int flags = (clone_flags & CLONE_THREAD) ? PIDFD_THREAD : 0;`), which contradicts the older
documented restriction; irrelevant to us but a sign that pidfd rules have moved recently. D1's
`[R]` (take the pidfd from `clone3` at creation, use it for every signal and for exit
notification) is sound, and its warning that a successful `pidfd_send_signal` means "accepted",
never "delivered", is exactly right for the Part 6.1 reasons.

### 9.3 `CLONE_PARENT` — **the man page is wrong about one combination (measured)**

- `[F]` HIGH — a **namespace init cannot use it**: `kernel/fork.c#L2162`,
  `if ((clone_flags & CLONE_PARENT) && current->signal->flags & SIGNAL_UNKILLABLE) return ERR_PTR(-EINVAL);`
  with the comment "prevent global and container-inits from creating siblings". So an engine that
  is itself PID 1 of a container (a plausible "engine inside a container" test setup) cannot use
  `CLONE_PARENT` at all.
- `[F]` HIGH — `clone3` rejects `CLONE_PARENT`/`CLONE_THREAD` together with a nonzero
  `exit_signal` (`clone3_args_valid`, `kernel/fork.c#L3043`). This confirms the youki sharp edge
  D1 §3.2 cites, from source.
- **`[F]` CORRECTED — `clone(2)`'s ERRORS section claims "EINVAL: `CLONE_NEWPID` and one (or
  both) of `CLONE_THREAD` or `CLONE_PARENT` were specified". For `CLONE_PARENT` that is false on
  6.11.** `[O]` DROPLET 6.11.0-29, 2026-10-23, `clone3()` with `exit_signal = 0`:
  `CLONE_NEWPID` → pid 3748077; `CLONE_NEWPID|CLONE_PARENT` → **pid 3748078, no error**;
  `CLONE_PARENT` → pid 3748079. No such check exists in `v6.11/kernel/fork.c`, nor in
  `v4.18/kernel/fork.c` (only the `SIGNAL_UNKILLABLE` one). runc's own comment corroborates a
  *historical* restriction — `nsexec.c` L1103-L1105: "there were some old kernel versions where
  `clone(CLONE_PARENT | CLONE_NEWPID)` was broken, so we'll just do it the long way anyway".
  The `CLONE_THREAD` half of that man-page sentence is still enforced
  (`kernel/fork.c#L2170-L2173`).
  Practical effect: D1 §9.2 tiers `CLONE_PARENT` as "strongly recommended (with 3 stages)". It is
  not required for the single-`clone3` MVP (the init is already the engine's direct child) and it
  is *available* if we ever want it, contrary to the documentation.

### 9.4 `unshare(2)` — CONFIRMED, with the mechanism behind the multithread rule

`[F]` HIGH — `kernel/fork.c#L3268-L3271` (`ksys_unshare`): "If unsharing a user namespace must
also unshare the thread group and unshare the filesystem root and working directories" — i.e.
`CLONE_NEWUSER` implies `CLONE_THREAD|CLONE_FS` — and `check_unshare_flags` (`#L3195`) then
rejects `CLONE_THREAD` when `!thread_group_empty(current)` → **`EINVAL`**. That is the whole
mechanism behind D1 §3.4's measured table, and it also means `unshare(CLONE_NEWUSER)` silently
**unshares your `fs_struct`** (root/cwd stop being shared), which D1 does not mention.
`CLONE_NEWPID`/`CLONE_NEWTIME` affecting only subsequently-created children: CONFIRMED
(`pid_for_children`/`time_for_children`). Unsharing the user namespace **together with** the
others in one call is legal and the new namespaces are owned by the new user namespace
(`kernel/nsproxy.c`: the capability check uses `new_cred->user_ns`); runc's "userns alone, then
the rest" is therefore `[C]` convention — which D1 already tags correctly.

### 9.5 `setns(2)` — **CORRECTED: D1's "one namespace per call" is the nsfs-only rule**

`[F]` HIGH — `setns(2)`, "fd is a PID file descriptor": "**Since Linux 5.8**, fd may refer to a
PID file descriptor obtained from `pidfd_open(2)` or `clone(2)`. In this usage, `setns()`
**atomically** moves the calling thread into one or more of the same namespaces as the thread
referred to by fd. The nstype argument is a bit mask specified by ORing together one or more of
the `CLONE_NEW*` namespace constants." D1 §3.1 quotes only the nsfs form ("each call to
`setns()` changes just one of the caller's namespace memberships") and presents it as the rule.

This matters for `mydocker exec`, which is the next thing after `run`: with a pidfd for the
container init, joining is **one** syscall instead of seven, and the atomicity removes an
ordering hazard (with nsfs fds you must join the user namespace first, and a partially-joined
process is observable). `nstype == 0` with a pidfd is `EINVAL`. The per-**thread** nature of
`setns` and the multithreaded/`CLONE_FS` restrictions for user namespaces are unchanged and
D1 states them correctly.

---

## Part 10 — shorter verdicts: §1, §4, §6.7, §8

### 10.1 §4.5 — **`OPEN_TREE_NAMESPACE` does not exist. A `[C]` HIGH claim collapses.**

D1 §4.5: *"crun goes further: `open_tree(rootfs_fd, "", OPEN_TREE_NAMESPACE|AT_RECURSIVE)` then
`setns(tree_fd, CLONE_NEWNS)` builds the container's entire mount namespace out-of-band and
enters it — **no `unshare(CLONE_NEWNS)` and no `pivot_root` at all**. `[C]` HIGH."

`[F]` HIGH — **CORRECTED.** `torvalds/linux@v6.11/include/uapi/linux/mount.h#L64-L65` defines
exactly two `open_tree` flags: `OPEN_TREE_CLONE` (1) and `OPEN_TREE_CLOEXEC`. Same in
`v6.17`. And `SYSCALL_DEFINE3(open_tree, …)` in `fs/namespace.c` rejects everything else:

```c
	if (flags & ~(AT_EMPTY_PATH | AT_NO_AUTOMOUNT | AT_RECURSIVE |
		      AT_SYMLINK_NOFOLLOW | OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC))
		return -EINVAL;
	if ((flags & (AT_RECURSIVE | OPEN_TREE_CLONE)) == AT_RECURSIVE)
		return -EINVAL;
```

So the quoted call would fail `EINVAL` twice over (unknown flag; `AT_RECURSIVE` without
`OPEN_TREE_CLONE`). `[C]` HIGH — in **crun 1.29.1**, `OPEN_TREE_NAMESPACE` appears **once**, as a
`#ifndef OPEN_TREE_NAMESPACE / #define OPEN_TREE_NAMESPACE 2` fallback in `src/libcrun/linux.c`,
and is **never used**: zero occurrences in `linux.h`, `utils.c`, `container.c`, and every actual
`open_tree` call site in `linux.c` (L375, L3116, and the `mount_setattr` paths at L1258/L2136)
uses `OPEN_TREE_CLONE`. crun uses `open_tree` + `mount_setattr` to apply mount attributes
atomically — which is real and worth stealing — but it does **not** build mount namespaces
out-of-band, and it does `pivot_root` like everyone else.

D1 open question 2 ("`OPEN_TREE_NAMESPACE`'s first kernel version") is answered: **there isn't
one**. Anything in the design resting on "we could skip `pivot_root` entirely later" must be
deleted. The rest of §4.5 (the five-step `fsopen`/`fsconfig`/`fsmount`/`move_mount` decomposition,
the fd-passing via `SCM_RIGHTS`, the TOCTOU argument, `[R]` MED "MVP: `mount(2)`, design for the
new API") survives intact.

### 10.2 §4.2 — a second propagation step is missing from §10.1

`[C]` HIGH — `opencontainers/runc@v1.5.1/libcontainer/rootfs_linux.go` (`prepareRootfs`, after
the pivot): a non-`MS_PRIVATE` `RootPropagation` from the OCI config is applied **again, after
`pivot_root`**, with the comment: *"This must be done after pivot_root/chroot because the mount
propagation flag is applied to the current root ('/'), and not to the old rootfs before it
becomes '/'. Applying the flag in prepareRoot would affect the host mount namespace if the
container's root mount is shared."* D1's `[R]` "Honour `linux.rootfsPropagation` when we get to
OCI" is therefore not a one-value substitution in step 11 — an `rshared` request needs a
**second** `mount("", "/", …)` after step 17. Add it as step 17a when OCI support lands.
D1's quote of `prepareRoot` itself matches v1.5.1 verbatim: CONFIRMED.

### 10.3 §6.7 — **Docker's `/sys/fs/cgroup` is not a bind mount** — CORRECTED

D1 §6.7 and the §4.4 table read the live container's mountinfo line
`499 498 0:29 /system.slice/docker-….scope /sys/fs/cgroup ro,… - cgroup2 cgroup rw,nsdelegate,…`
as "a **bind** of the host cgroup2 subtree at the container's own cgroup path".

`[O]` DROPLET 6.11.0-29, 2026-10-23 — the host's own line is
`34 25 0:29 / /sys/fs/cgroup rw,… - cgroup2 **cgroup2** rw,nsdelegate,memory_recursiveprot`.
A bind inherits `mnt_devname` from its source (`clone_mnt(old->mnt_devname)`), so a bind of the
host mount would read `cgroup2`, not `cgroup`. The container's `cgroup` devname is the source
string runc passes to a **fresh `mount -t cgroup2`**. The non-`/` root field is exactly what
`cgroup_namespaces(7)` documents for a cgroup filesystem mounted *inside a cgroup namespace* —
its worked example shows the same shape (`155 145 0:32 /.. /sys/fs/cgroup/freezer`) — because
cgroup2 reuses one superblock (hence the shared `0:29`) and sets the mount's root dentry to the
namespace's cgroup directory. `[C]` runc confirms: `mountCgroupV2` does a real `cgroup2` mount
and falls back to a bind of `c.cgroup2Path` **only** on `EPERM`/`EBUSY`, i.e. userns-without-
cgroupns.

`[R]` **CORRECTED implementation for §6.7:** create the cgroup namespace (after cgroup placement
— Part 4.1), then from inside the child simply
`mount("cgroup", "<rootfs>/sys/fs/cgroup", "cgroup2", MS_RDONLY|MS_NOSUID|MS_NODEV|MS_NOEXEC, NULL)`.
No path computation, no bind, no host-subtree reference to get wrong. The bind form is the
*rootless* fallback, not the normal path.

### 10.4 §1, §4.1, §4.3, §4.4, §8 — **nothing needed correcting**

- §1.1–§1.5, §1.8: checked against `namespaces(7)`, `pid_namespaces(7)`, `uts_namespaces(7)`,
  `ipc_namespaces(7)`, `time_namespaces(7)`. Accurate, including the `CLONE_NEWTIME == 0x80`
  collision with legacy `clone`'s `CSIGNAL` mask, which `clone3_args_valid`'s
  `kargs->flags & (CLONE_DETACHED | (CSIGNAL & (~CLONE_NEWTIME)))` test confirms from the other
  direction — `CLONE_NEWTIME` is explicitly carved **out** of the forbidden CSIGNAL bits for
  `clone3`.
- §1.6's load-bearing claim (capabilities from `CLONE_NEWUSER` do not survive `execve` unless
  uid 0 is mapped) is correct and the mechanism is `capabilities(7)`'s root rule: for a
  non-setuid, non-fcap file the file sets are empty, so `P'(permitted) = P'(ambient) = 0`,
  **unless** the euid is 0, in which case the file's inheritable and permitted sets are treated
  as all-ones. CONFIRMED.
- §4.1's four propagation definitions are verbatim-correct against `mount_namespaces(7)`, as is
  the "immediately under" subtlety. The one adjacent sentence D1 omits is the one used in Part 1:
  "What happens if the mount itself is unmounted is determined by the propagation type that is in
  effect for the parent of the mount."
- §4.3 (`MS_BIND` ignores flags; two-pass remount; `mount_setattr(AT_RECURSIVE)` since 5.12) and
  §4.4 (the measured Docker flag table) are correct.
- §8 (two delivery paths; `pidfd_send_signal`; `setsid` → `TIOCSCTTY` → `dup2` → `execve`;
  `SIGWINCH` → `TIOCSWINSZ` is not automatic) is correct against `setsid(2)`, `tty_ioctl(4)` and
  `termios(3)`. The one thing worth adding to the spec: the terminal path works **because the
  signal goes to a process group**, so it is unaffected by the PID-1 rules in Part 6.1 — D1 says
  this and it is right.

---

## Corrections that change a recommendation

1. **§10.1 step 8** put `CLONE_NEWCGROUP` and `CLONE_INTO_CGROUP` in one `clone3` → `copy_cgroup_ns()` roots the new cgroup namespace at **`current`'s** (the parent's) cgroup (`v6.11/kernel/cgroup/namespace.c#L75`), so the container would see the engine's cgroup subtree, siblings included → **invalidates step 8 as written and shifts §3.3's "one `clone3` is enough" `[R]` HIGH**: drop `CLONE_NEWCGROUP` from the clone and `unshare(CLONE_NEWCGROUP)` in the child after placement, before mounting `/sys/fs/cgroup`.
2. **§9.3/§4.2** justified `MS_REC|MS_SLAVE` on `/` as "otherwise every container mount leaks to the host permanently" → container mounts under the rootfs are already non-propagating once step 12 makes the rootfs's parent private; the real exposure is **bind-mounted volumes inheriting the source's peer group**, plus umount propagation at teardown → **shifts §4.2's `[R]` HIGH reasoning and demotes open question 8 to a non-issue for a volume-less MVP** (`rslave` and `rprivate` are then behaviourally identical).
3. **§11.2 / step 30** treated `CLOSE_RANGE_CLOEXEC` as part of `close_range` (5.9) → the **flag is Linux 5.11**; §2.1's probe tested the syscall only → **shifts the `[R]` HIGH fd-hygiene step**: probe the flag, or adopt D5's 5.15 floor where it is moot.
4. **§10 defect 2 / §3.3 reason 1** said the child "cannot write its own `uid_map`" → it **can**, for a single-extent length-1 self-map of its own euid (`new_idmap_permitted`, `v6.11/kernel/user_namespace.c#L1163`); only multi-extent/subuid maps need a privileged writer → **shifts the `[R]` HIGH claim that userns support forces a 3-process topology**: single-ID rootless needs no intermediate.
5. **§6.7/§4.4** read Docker's `/sys/fs/cgroup` as a bind of the host cgroup2 subtree → it is a **fresh `mount -t cgroup2` performed inside the cgroup namespace** (host devname is `cgroup2`, container's is `cgroup`; `cgroup_namespaces(7)` worked example) → **shifts the `[R]` MED implementation** to "create cgroupns, then plain `cgroup2` mount"; the bind is the rootless fallback only.
6. **§9.1/§0.1** "Docker doesn't use the time namespace", "Docker's default is 6 of 8" → **Docker Engine 29.5.0 (2026-05-14) enables a private time namespace by default**, with a `time-namespaces` feature flag → **shifts §0.1's `[R]` HIGH framing**; the MVP decision is unchanged, the claim is not repeatable.
7. **§9.1/§1.7** `CLONE_NEWCGROUP` as "hardening — path hiding only, controls nothing" → with `nsdelegate` (on by default on the DROPLET) the cgroupns root is a **migration boundary** (`cgroup-v2.rst`, Delegation Containment: cross-boundary migration → `-ENOENT`), and it is a **prerequisite for §6.7** → **tier shifts to strongly recommended**.
8. **§0.1** "skipping the user namespace is Docker's actual default posture, so we are not doing worse than the incumbent" → Docker's default is survivable only because of its capability drop + seccomp + AppArmor, all of which §10.1 defers to D5 → **shifts the stated MVP posture**: the MVP is root-equivalent and must be labelled so; the cheapest first hardening is the bounding-set drop, not the userns.
9. **§3.2/§8.2/§4.3** kept legacy-`clone`, `kill(2)`-by-pid and `mount(2)`-only fallbacks "for ROCKY-class kernels" → D5 §7.3's floor (hard 5.10, documented 5.15) puts `clone3`, `CLONE_PIDFD`, `pidfd_send_signal`, `CLONE_INTO_CGROUP` and `mount_setattr` all below the floor → **deletes three `[R]`-level fallback paths**; keep only the `ENOSYS` probe policy for above-floor features.
10. **§3.1** "each call to `setns()` changes just one namespace" → that is the nsfs form; **since Linux 5.8 a pidfd + `CLONE_NEW*` bitmask joins several atomically** (`setns(2)`) → **shifts the future `mydocker exec` design** from a 7-call ordered sequence to one call.
11. **§4.5** cited crun building a whole mount namespace out-of-band via `open_tree(…, OPEN_TREE_NAMESPACE|AT_RECURSIVE)` + `setns`, "no `pivot_root` at all", `[C]` HIGH → **`OPEN_TREE_NAMESPACE` is not in the kernel UAPI (v6.11, v6.17) and `open_tree` rejects it**; crun 1.29.1 defines it in an unused `#ifndef` → **removes that escape hatch from the mount-layer design** and answers open question 2.
12. **§6.8** presented `maskedPaths`/`readonlyPaths` as hardening → masking is **per-superblock** and a container that can still `mount` gets a clean `/proc` (measured: second instance, `0:67`, `kcore` present) → **shifts the `[R]` MED**: masking only counts when paired with dropping `CAP_SYS_ADMIN` or seccomp-blocking `mount`.
13. **§10.1 step 7** ("pre-allocate everything") omits the output side → **add "flush all stdio before the clone"**; the inherited-buffer duplication D1 observed is prevented by flushing, not by not printing afterwards.
14. **§5.3/§10.1 step 17** holds `O_PATH` fds on the old root and the rootfs → those fds **are** the escape (CVE-2024-21626 class) and survive `MNT_DETACH` → **close them explicitly at the end of step 17** rather than relying on step 30's cloexec marking.
15. **§5.1** lists the `pivot_root` preconditions but omits `MNT_LOCKED` (EINVAL), `d_unlinked(new_root)` (**ENOENT**), and that both mounts must be in the **caller's** mount namespace; and `new_root == current root` is **EBUSY**, not EINVAL → **shifts our errno mapping**, not the design.

## Still unverifiable, and the exact test

- `CLONE_INTO_CGROUP` + `CLONE_NEWCGROUP` end-to-end (Part 4.1 is from source, not measured; also D1 open question 1) → **dev VM, not the droplet**: `mkdir /sys/fs/cgroup/v1test`, `fd=open(…, O_DIRECTORY)`, `clone3(flags=CLONE_NEWCGROUP|CLONE_INTO_CGROUP, cgroup=fd)`, child prints `/proc/self/cgroup` — expect `0::/v1test` (confirms the defect) rather than `0::/`.
- Whether a volume bind from a shared host mount really lands **shared** inside the container (Part 1.3 is derived from `clone_mnt`) → **dev VM**: `mount --make-rshared /srv; unshare -m --propagation unchanged sh -c 'mount --bind /srv /mnt/ctr/srv; findmnt -no PROPAGATION /mnt/ctr/srv'` → expect `shared`.
- Whether omitting *both* the `/` `rslave` and the §5.3 old-root `rslave` lets `umount2(put_old, MNT_DETACH)` remove **host** mounts → **dev VM only, destructive**: shared `/`, `pivot_root(".",".")`, skip the old-root propagation change, detach, then compare host `/proc/self/mountinfo` before/after.
- Whether Docker 29.5+ also sets `timens_offsets` (does a container's `/proc/uptime` start at 0?) → **a host running Docker ≥ 29.5**: `docker run --rm alpine cat /proc/uptime` versus the host's, and `cat /proc/$(docker inspect -f '{{.State.Pid}}' <c>)/timens_offsets`.
- `CLONE_PIDFD` availability on RHEL 8 / 4.18 (only matters if the 5.10 floor is ever reversed) → **`uc-arcc`**: `syscall(SYS_clone, CLONE_PIDFD|SIGCHLD, 0, &pidfd, 0, 0)` and check for `EINVAL` versus a valid fd.
- Exact kernel version at which `clone(CLONE_NEWPID|CLONE_PARENT)` stopped being rejected (behaviourally allowed on 6.11 and absent from v4.18 source; runc's comment implies it was once broken) → `git log -S 'CLONE_PARENT' -- kernel/fork.c` in a kernel clone, bisecting pre-4.18 tags.
- `subset=pid` / `hidepid=` support on 4.18 (upstream 5.8; RHEL backport status unknown) → **`uc-arcc`**: `unshare -Umrpf sh -c 'mkdir -p /tmp/p && mount -t proc -o subset=pid proc /tmp/p && ls /tmp/p'` → `EINVAL`/"unsupported subset option" means absent.
- The `newinstance` no-op commit (behaviour settled from v6.11 source; D1 open question 3) → `git log --oneline -S newinstance -- fs/devpts/inode.c` around v4.7.
