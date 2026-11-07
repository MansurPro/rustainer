# D5 — Resource control, security boundary & rootless execution

Research-only. No code. Companion to `D1-kernel-isolation.md` (namespaces, mounts,
`pivot_root`, PID 1), `D2-runtime-architecture.md` (lifecycle, process boundaries),
`D3` (images/layers), `D4-networking.md` (netns plumbing).

Claim tags, matching D1:

- `[F]` **Fact** — what Linux/OCI actually requires. Primary source cited.
- `[C]` **Convention** — what Docker/runc/crun/podman happen to do. Docs or source cited.
- `[R]` **Recommendation** — what our engine should do, with the tradeoff stated.
- `[O]` **Observation** — empirically verified; names host + kernel + exact command.

Confidence: `HIGH` / `MED` / `LOW`. Unsourced ⇒ `LOW`, with what would settle it.

---

## 0. Method and empirical baseline

### 0.1 Hosts

**HOST-A** — `host-a`, hostname `HOST-A`, Ubuntu 24.10,
`6.11.0-29-generic`, x86_64, 4 CPUs, root.

```
$ ssh host-a 'uname -a'
Linux HOST-A 6.11.0-29-generic #29-Ubuntu SMP PREEMPT_DYNAMIC
  Fri Jun 13 20:29:41 UTC 2025 x86_64 GNU/Linux
```

**This is a production host serving live traffic on :80/:443.** Every probe in this
document is a *read*. Nothing under `/sys/fs/cgroup` was written, no `sysctl -w`, no
container created/started/stopped, no `systemctl`, no netfilter change, no AppArmor
profile change. Where an experiment would have required a write, the experiment is
*described* for the future dev VM instead of run — those are marked
**`[EXPERIMENT — dev VM]`** and collected in §21.

Live containers observed (read-only `docker ps`):

```
c087abc73246 webapp-web-1      ghcr.io/example-org/backend:v2.25.1
4a820bcb9df9 webapp-nginx-1    nginx:stable
7bfda1b6dda9 webapp-frontend-1 ghcr.io/example-org/frontend:latest
ed133d1fe199 webapp-certbot-1  certbot/certbot
12d16acf9cc0 webapp-redis-1    redis:7-alpine
```

**ROCKY** — `old-host` (`OLD-HOST`, the university ARCC Slurm login node),
Rocky Linux 8.8, `4.18.0-425.3.1.el8`, x86_64, **unprivileged**, cgroup v1 legacy only,
no `/etc/subuid` entries, SELinux disabled, NFS home. This is the *degradation
laboratory*: the machine where the honest answer is "we cannot limit resources here".

> **Availability note for this session:** `ssh old-host` failed DNS resolution at the
> start of this run (`ssh: Could not resolve hostname old-host.example.edu`) — the host is
> off-network from this machine right now. Where ROCKY observations appear below and
> are marked `[O] ROCKY (D1)`, they are **inherited from `D1-kernel-isolation.md`
> §12/§13, which probed the same host in the same week** — they are second-hand within
> this project, not re-verified by me. Claims that *needed* fresh ROCKY evidence and
> could not get it are demoted to `LOW` and listed in §23 Open questions with the exact
> command that would settle them. That is the honest state; I have not invented ROCKY
> output.

### 0.2 The HOST-A cgroup v2 baseline

```
$ ssh host-a 'findmnt -t cgroup2 -o TARGET,SOURCE,FSTYPE,OPTIONS'
TARGET         SOURCE  FSTYPE  OPTIONS
/sys/fs/cgroup cgroup2 cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate,memory_recursiveprot

$ ssh host-a 'cat /sys/fs/cgroup/cgroup.controllers'
cpuset cpu io memory hugetlb pids rdma misc

$ ssh host-a 'cat /sys/fs/cgroup/cgroup.subtree_control'
cpuset cpu io memory hugetlb pids rdma misc

$ ssh host-a 'cat /sys/fs/cgroup/cgroup.stat'
nr_descendants 65
nr_dying_descendants 881
```

`[O]` HOST-A, HIGH. Pure cgroup v2 unified ("**cgroup v2 only**", no `tmpfs` at
`/sys/fs/cgroup` with per-controller subdirs, no hybrid). Two mount options matter and
are covered in §1.6 and §3.2: `nsdelegate` and `memory_recursiveprot`.

`nr_dying_descendants 881` is worth noticing on its own: 881 cgroups have been removed
by userspace but still hold charged pages, so the kernel keeps the objects alive. This
is normal, it is not a leak in the ordinary sense, and it is why "delete the cgroup
directory" is not "release the memory" (§4.7).

### 0.3 What "read the source" means here

Runtime source was read as pinned raw files, same convention as D1:

- runc **v1.2.5** — `libcontainer/cgroups/systemd/{v2.go,common.go}`,
  `libcontainer/cgroups/utils.go`, `libcontainer/specconv/spec_linux.go`.
  **v1.2.5 deliberately**: it is the version actually running on HOST-A (`runc --version`),
  so source and observation are the same build.
- moby/moby **v28.3.1** (the version on HOST-A) — `oci/caps/defaults.go`,
  `oci/defaults.go`, `profiles/seccomp/default.json`, `profiles/apparmor/template.go`
- youki **v0.7.0** — `crates/libcgroups/` read as a stat-parsing reference only

Kernel documentation is quoted from `Documentation/admin-guide/cgroup-v2.rst` and from
man-pages **6.8-2** as installed on HOST-A (`cgroups(7)`, `capabilities(7)`,
`user_namespaces(7)`, `seccomp(2)`, `prctl(2)`, `proc(5)`), cross-checked against
man7.org and kernel.org/doc.

---

## 1. The cgroup v2 model, precisely

### 1.1 One hierarchy, one mount, and what that changes

`[F]` HIGH. cgroup v2 is a **single** hierarchy. There is exactly one `cgroup2`
superblock, conventionally at `/sys/fs/cgroup`, and every controller distributes
resources along *that one* tree. In v1 each controller had its own independent tree, so
a process could be in `/sys/fs/cgroup/memory/foo` and `/sys/fs/cgroup/cpu/bar`
simultaneously with no relationship between the two. Source:
`Documentation/admin-guide/cgroup-v2.rst`, "Basic Operations"; `cgroups(7)`.

The engineering consequence is bigger than it sounds. In v1 our cgroup manager is a
*map* — controller name → path — and every operation loops over controllers, each of
which can fail independently, leaving a partially-configured container. In v2 it is a
**single path plus a set of file writes into one directory**. Failure is still per-write,
but the identity of the container's cgroup is one string. Our `Container` state struct
should hold one `cgroup_path: PathBuf` (or better, one `OwnedFd` to the directory —
see §1.7), not a map.

`[O]` HOST-A, HIGH:

```
$ ssh host-a 'cat /proc/2590614/cgroup'
0::/system.slice/docker-c087abc73246….scope
```

The `0::` prefix is the v2 signature: hierarchy ID 0, empty controller list. A v1 line
looks like `12:memory:/…`. This is the cheapest reliable per-process v1/v2 discriminator
and is what §5.4's detection routine uses.

### 1.2 `cgroup.controllers` vs `cgroup.subtree_control` — the enable-for-children model

`[F]` HIGH. `cgroup.controllers` (read-only) lists the controllers **available to this
cgroup**, i.e. the ones its *parent* has enabled for its children.
`cgroup.subtree_control` (read-write) lists the controllers this cgroup has enabled
**for its own children**. A controller can only appear in a child's
`cgroup.controllers` if the parent put it in `cgroup.subtree_control`. Writes are
operator-form: `echo "+memory +pids -io" > cgroup.subtree_control`. Kernel doc:
"Only controllers which are listed in `cgroup.controllers` can be enabled." Multiple
operations in one write are **atomic** — all succeed or none do.

The consequence that catches every first implementation: **a cgroup's own resource
knobs come from its parent's `subtree_control`, not from its own.** Creating
`/sys/fs/cgroup/mydocker/ctr1/` gives you a directory with `cgroup.procs` and nothing
else if `/sys/fs/cgroup/mydocker/cgroup.subtree_control` is empty. There is no
`memory.max` to write; `open()` returns `ENOENT`. So the creation order is:

```
1. mkdir  /sys/fs/cgroup/mydocker
2. write  "+cpu +memory +pids +io" > /sys/fs/cgroup/cgroup.subtree_control   (root)
3. write  "+cpu +memory +pids +io" > /sys/fs/cgroup/mydocker/cgroup.subtree_control
4. mkdir  /sys/fs/cgroup/mydocker/ctr1
5. now    /sys/fs/cgroup/mydocker/ctr1/memory.max exists
```

Step 2 is usually already done for you — `[O]` HOST-A shows root
`cgroup.subtree_control` already carries the full set because systemd enables
everything at boot:

```
$ ssh host-a 'cat /sys/fs/cgroup/cgroup.subtree_control'
cpuset cpu io memory hugetlb pids rdma misc
```

`[R]` HIGH. Do not *assume* it. Read `cgroup.controllers` at each level and enable only
what you need and what is available. Writing `+memory` when `memory` is not in
`cgroup.controllers` returns `ENOENT` — a confusing errno for "controller unavailable",
and one worth translating into a real error message.

`[F]` MED (`cgroup-v2.rst`, "Enabling and Disabling"). You cannot disable a controller
in `subtree_control` while a child still has it enabled in *its* `subtree_control`, and
you cannot enable a controller in `subtree_control` while the cgroup has member
processes (that is the no-internal-process rule below). Both return `EBUSY`.

### 1.3 The no-internal-process rule — exactly what it forbids

`[F]` HIGH. Verbatim from `cgroup-v2.rst`:

> Non-root cgroups can distribute domain resources to their children only when they
> don't have any processes of their own.

And from `cgroups(7)`, the precise form:

> a (nonroot) cgroup can't both (1) have member processes, and (2) distribute resources
> into child cgroups — that is, have a nonempty `cgroup.subtree_control` file.

Read carefully, because the common summary ("processes only in leaves") is **wrong in
one direction that matters**:

- A cgroup with member processes and an **empty** `subtree_control` **may** have child
  cgroups. That is legal. It is not a leaf, and the rule does not forbid it.
- A cgroup with a **non-empty** `subtree_control` may not have member processes.
- The **root** cgroup is exempt entirely — it always has processes (kthreads at minimum)
  and always distributes.

The errno: writing a PID into `cgroup.procs` of a cgroup whose `subtree_control` is
non-empty fails with **`EBUSY`**, and writing `+ctrl` to `subtree_control` of a cgroup
that has member processes also fails with **`EBUSY`**.

`[O]` HOST-A, HIGH — the live layout is exactly this shape:

```
$ ssh host-a 'cat /sys/fs/cgroup/system.slice/cgroup.subtree_control; \
                  wc -l < /sys/fs/cgroup/system.slice/cgroup.procs'
cpuset cpu io memory hugetlb pids rdma misc
0

$ ssh host-a 'CG=/sys/fs/cgroup/system.slice/docker-ee88….scope; \
                  cat $CG/cgroup.subtree_control; cat $CG/cgroup.type'
                     ← (empty)
domain
```

`system.slice` distributes and holds **zero** processes. The container scope holds 12
processes and distributes **nothing**. Textbook compliance. Docker/runc never nest below
the container scope, which is why they never trip the rule.

`[R]` HIGH — **this dictates our layout.** Our tree must be:

```
/sys/fs/cgroup/mydocker/                     subtree_control="+cpu +memory +pids"   0 procs
/sys/fs/cgroup/mydocker/<id>/                subtree_control=""                     container procs
```

`[R]` MED — and it forbids the tempting thing. If we later want a per-container
sub-cgroup for `exec`-ed processes (like Kubernetes' `pod/container` split, or to give
`mydocker exec` its own accounting), the container cgroup must become a distributor,
which means **the container's own processes must move down one level first**. Design the
path helper as `container_cgroup(id).join("init")` from day one if there is any chance
of that; retrofitting it means moving a live PID 1, which is doable but is a migration
you will not want to write later.

### 1.4 `cgroup.type` and threaded cgroups

`[F]` HIGH (`cgroup-v2.rst`, "Threads"). `cgroup.type` reads one of four values:

| value | meaning |
|---|---|
| `domain` | normal cgroup; processes are the unit of assignment |
| `domain threaded` | this cgroup is the **root** of a threaded subtree |
| `domain invalid` | inside a threaded subtree but not yet valid; cannot hold processes or controllers |
| `threaded` | a member of a threaded subtree; threads assignable individually |

Writing `threaded` into `cgroup.type` converts a domain into a threaded cgroup. Kernel
doc: the conversion is "single direction" — **irreversible**. Threaded mode exists so
that *threads of one process* can be split across cgroups, which only makes sense for
the small set of controllers that are thread-aware (`cpu`, `cpuset`, `perf_event`). The
memory controller is process-granular and is **not** available in a threaded subtree.

`[R]` HIGH. **We should never create a threaded cgroup, and we should assert
`cgroup.type == "domain"` on the cgroup we are about to use.** Two concrete reasons:

1. `cgroup.kill` is explicitly **"not supported in threaded cgroups"** (kernel doc). Our
   entire race-free stop story (§4) depends on `cgroup.kill`. Silently landing in a
   threaded subtree — e.g. because a user passed `--cgroup-parent` pointing at
   something a JVM or a systemd `Delegate=` unit made threaded — turns `cgroup.kill`
   into `ENOTSUP` and we fall back to the racy path without noticing.
2. `memory.max` does not exist there at all, so `--memory` would silently do nothing.

`[O]` HOST-A, HIGH: every cgroup I read (`/sys/fs/cgroup/system.slice`, the docker
scopes) reports `domain`. `[R]` The check is one `read_to_string` and one `==`. Do it.

### 1.5 `cgroup.procs` vs `cgroup.threads`

`[F]` HIGH. Writing a PID to `cgroup.procs` migrates the **entire thread group**.
Writing a TID to `cgroup.threads` migrates **one thread**, and is only meaningful inside
a threaded subtree (in a domain cgroup all threads of a process must share the cgroup, so
`cgroup.threads` is read-only-in-effect there for cross-cgroup moves). Reading
`cgroup.procs` yields TGIDs; reading `cgroup.threads` yields TIDs.

`[O]` HOST-A, HIGH — the difference is visible and large:

```
$ ssh host-a 'CG=…docker-ee88….scope; wc -l < $CG/cgroup.procs; wc -l < $CG/cgroup.threads'
12      (cgroup.procs — TGIDs)
72      (cgroup.threads — TIDs)
```

`[R]` HIGH. **Always read `cgroup.procs` for "who is in this container", never
`cgroup.threads`.** Sending SIGKILL to a TID that is not a TGID via `kill(2)` is legal
(it signals the thread group anyway, since `kill()` takes a PID meaning TGID), but you
will do 72 syscalls where 12 suffice, and you will log 60 spurious `ESRCH`s as threads
die under you. The read is also **not atomic** — the file can change between the read
and your signal; see §4.

`[F]` MED (`cgroup-v2.rst`). Reading `cgroup.procs` gives **no ordering guarantee and no
guarantee of no duplicates** — the kernel doc says PIDs "are not ordered" and the same
PID "may appear more than once" if the process moved and was re-added during the read.
Deduplicate.

### 1.6 `nsdelegate` and `memory_recursiveprot` — the two mount options we inherit

`[O]` HOST-A, HIGH:

```
/sys/fs/cgroup cgroup2 rw,nosuid,nodev,noexec,relatime,nsdelegate,memory_recursiveprot
```

`[F]` HIGH. `nsdelegate` makes the kernel treat a **cgroup namespace root as a
delegation boundary automatically** — a process inside a cgroup namespace cannot move
processes across that boundary even if file permissions would allow it. `[F]` HIGH
(`cgroups(7)`, containment rules). This is a second, independent containment mechanism
layered on the file-ownership one.

`[F]` MED. `memory_recursiveprot` changes `memory.low`/`memory.min` semantics so that
protection set on a parent is distributed to descendants without requiring each
descendant to set its own value. Without it, an unset `memory.low` on a child means
"unprotected" even if the parent is protected. This matters if we ever expose
`--memory-reservation` and users nest cgroups; it is systemd's default on modern
distros, so on Ubuntu 24.10 we get it for free but must not *assume* it.

`[O]` HOST-A, HIGH — the kernel advertises exactly what it supports:

```
$ ssh host-a 'cat /sys/kernel/cgroup/features'
nsdelegate
favordynmods
memory_localevents
memory_recursiveprot
memory_hugetlb_accounting
pids_localevents

$ ssh host-a 'cat /sys/kernel/cgroup/delegate'
cgroup.procs
cgroup.threads
cgroup.subtree_control
memory.oom.group
memory.reclaim
```

`[R]` MED. `/sys/kernel/cgroup/features` is a **free, cheap, forward-compatible feature
probe** and we should read it once at startup and cache it. It is not exhaustive (it
does not advertise `cgroup.kill`, for example), but it is the only self-describing
cgroup feature file the kernel offers.

### 1.7 Placing the process: move-in vs `CLONE_INTO_CGROUP`

`[F]` HIGH. The classical placement is: create the process, then write its PID into the
target `cgroup.procs`. Between `clone()` returning and that write landing, **the child
is running in the parent's cgroup with the parent's limits** (i.e. usually none). That
window is short but real, and it is exactly the window in which a malicious or merely
broken image can `fork()` unbounded or allocate.

`[F]` HIGH (`clone(2)`, Linux **5.7**). `clone3()` with `CLONE_INTO_CGROUP` and
`cl_args.cgroup` set to an `O_RDONLY|O_DIRECTORY` fd on the target cgroup causes the
child to be **born** in that cgroup. No window at all.

`[O]` HOST-A (inherited from D1 §12, same host/kernel), MED: on 6.11 the flag is
recognised and validated — a bad fd yields `EBADF`, not `EINVAL`, which proves the
kernel parsed the field rather than rejecting the flag.

`[R]` HIGH — **use it, but keep the move-in path.** Reasoning:

- It closes a real window for free; the cost is one extra `open()` and one struct field.
- It is **cgroup v2 only** (`clone(2)`: "This flag is only available in cgroup v2").
- It needs kernel ≥ 5.7, which is above our floor recommendation (§6) but not
  universally satisfied — RHEL 8's 4.18 does not have it.
- We need the move-in path *anyway*, because `mydocker exec` puts a new process into an
  **existing** container cgroup, and because the systemd driver (§2.4) has systemd
  create the unit and place a PID.

The tradeoff to state plainly: `CLONE_INTO_CGROUP` also **fails the whole `clone3`** if
the target cgroup violates the no-internal-process rule or is `domain invalid`. With
move-in, the clone succeeds and only the placement fails, which is easier to report but
leaves an orphan you must reap. `[R]` Prefer the harder failure — a container that
cannot be constrained should not exist.

`[F]` MED. A **populated** `cgroup.events` transition is the correct way to observe
"this cgroup has processes": `cgroup.events` contains `populated 0|1`, and the file is
**pollable** (`poll(2)`/`epoll` on it returns `POLLPRI` on change). `[O]` HOST-A:
`cgroup.events` reads `populated 1 / frozen 0` for a running container. `[R]` MED —
this is a much better "container exited" signal than polling `cgroup.procs`, and it
composes with the pidfd from D1 §8.2: pidfd tells you PID 1 died, `cgroup.events`
tells you *everything* died. Both are needed; they answer different questions.

---

## 2. Delegation, and the systemd problem

### 2.1 What delegation actually is

`[F]` HIGH (`cgroups(7)`, "Cgroups v2 delegation"). Delegation = handing a subtree to a
less-privileged manager. It is done by `chown`-ing a specific, kernel-published set of
files — **not** the whole directory contents:

| chown to delegatee | effect |
|---|---|
| the directory `dlgt_grp` itself | new cgroups created inside are owned by the delegatee |
| `dlgt_grp/cgroup.procs` | delegatee can move processes into the subtree root |
| `dlgt_grp/cgroup.subtree_control` | delegatee can enable controllers for its children |
| `dlgt_grp/cgroup.threads` | delegatee can manage threaded subtrees |

And the rule that defines the whole security model, verbatim from `cgroups(7)`:

> The delegater should **not** change the ownership of any of the controller interface
> files (e.g. `pids.max`, `memory.high`) in `dlgt_grp`. Those files are used from the
> next level above the delegated subtree in order to distribute resources into the
> subtree.

So: **a delegatee can subdivide its budget but can never raise it.** `memory.max` on the
subtree root is written by the *delegater*; the delegatee may only write `memory.max` on
cgroups it creates *inside*, and the kernel enforces the parent's limit regardless.

`[O]` HOST-A, HIGH — the kernel publishes the authoritative list, and it is **five**
files on 6.11, not the three that older code hardcodes:

```
$ ssh host-a 'cat /sys/kernel/cgroup/delegate'
cgroup.procs
cgroup.threads
cgroup.subtree_control
memory.oom.group
memory.reclaim
```

`[C]` HIGH — runc reads exactly this file and falls back to the three-item list only
when it is absent (Linux < 4.15). runc v1.2.5,
`libcontainer/cgroups/systemd/v2.go:cgroupFilesToChown()`:

```go
const cgroupDelegateFile = "/sys/kernel/cgroup/delegate"
f, err := os.Open(cgroupDelegateFile)
if err != nil {
    return []string{"cgroup.procs", "cgroup.subtree_control", "cgroup.threads"}, nil
}
```

`[R]` HIGH. Copy this exactly. Hardcoding the three-file list is a live bug on ≥ 4.15:
a rootless container that cannot write `memory.oom.group` cannot use `--oom-kill-group`
semantics, and one that cannot write `memory.reclaim` cannot proactively reclaim.

### 2.2 Delegation containment — the rule that bites

`[F]` HIGH (`cgroups(7)`). A non-privileged delegatee may write PID *p* into a
destination `cgroup.procs` only if **all** of:

1. it has write permission on the destination `cgroup.procs`;
2. it has write permission on `cgroup.procs` of the **nearest common ancestor** of the
   source and destination cgroups;
3. (with `nsdelegate`) both source and destination are visible from its cgroup namespace;
4. it has the usual `ptrace`-style permission over the target process.

Condition 2 is the one people miss, and the man page spells out its consequence:

> One consequence of these delegation containment rules is that the unprivileged
> delegatee can't place the first process into the delegated subtree; instead, the
> delegater must place the first process.

`[R]` HIGH — **this is a hard architectural constraint on rootless.** In rootless mode
we do not get to "create our cgroup and move into it". Something privileged (systemd's
`user@.service`, via `Delegate=yes`) must have already put our session's processes
inside the delegated root; we then subdivide *downward* from where we already are.
Practically: in rootless mode our cgroup root is not a path we choose, it is
`/sys/fs/cgroup` + the contents of our own `/proc/self/cgroup`. §16.2.

### 2.3 systemd owns the hierarchy, and it is not sharing

`[F]` HIGH. On any systemd distro, systemd is the **single writer** of the cgroup tree
by policy. Its documented rule (the "single-writer rule",
`systemd.resource-control(5)` and the kernel's own delegation doc) is that a cgroup
subtree has exactly one manager, and outside a `Delegate=yes` unit that manager is
systemd. systemd will happily `rmdir` cgroups it does not know about during a daemon
reload or a `systemctl daemon-reexec`.

`[O]` HOST-A, HIGH — this is why Docker's units look the way they do:

```
$ ssh host-a 'systemctl show docker.service     -p Delegate -p Slice -p TasksMax'
Slice=system.slice   Delegate=yes   TasksMax=infinity
$ ssh host-a 'systemctl show containerd.service -p Delegate -p Slice -p TasksMax'
Slice=system.slice   Delegate=yes   TasksMax=infinity
$ ssh host-a 'systemctl show docker-ee88….scope -p Slice -p Delegate -p TasksMax \
                     -p MemoryMax -p CPUQuotaPerSecUSec'
Slice=system.slice   Delegate=yes   TasksMax=19147
MemoryMax=infinity   CPUQuotaPerSecUSec=infinity
```

Every container gets its **own transient systemd scope unit** with `Delegate=yes`.
Docker did not put the container in a directory it made itself; it asked systemd (over
D-Bus, via runc) to create `docker-<id>.scope` and to put the PID in it.

### 2.4 The finding that decides this section: `TasksMax=19147` came from systemd, not Docker

`[O]` HOST-A, HIGH. None of the three containers I inspected declares any limit:

```
$ ssh host-a 'docker inspect -f "{{.HostConfig.Memory}} {{.HostConfig.NanoCpus}} \
                    {{.HostConfig.PidsLimit}}" c087abc73246'
0 0 <no value>
```

Yet the live cgroup shows:

```
$ ssh host-a 'cat …docker-ee88….scope/pids.max'
19147
$ ssh host-a 'systemctl show --property=DefaultTasksMax'
DefaultTasksMax=19147
$ ssh host-a 'cat /proc/sys/kernel/threads-max'
127653
```

19147 ≈ 15% of `threads-max`, which is systemd's `DefaultTasksMax=15%` default. **The
container has a PID limit that Docker never asked for and does not report.** `docker
inspect` says "no limit"; the kernel says 19147.

`[C]` HIGH — and runc's source confirms it is not runc's doing. runc v1.2.5
`libcontainer/cgroups/systemd/v2.go`:

```go
if r.PidsLimit > 0 || r.PidsLimit == -1 {
    properties = append(properties, newProp("TasksMax", uint64(r.PidsLimit)))
}
```

`PidsLimit == 0` (unset) ⇒ no `TasksMax` property ⇒ systemd applies `DefaultTasksMax`.

`[R]` HIGH — two lessons, both about honesty:

1. **`docker inspect` is not ground truth and neither will ours be.** Our `inspect`
   should report *declared* config and, separately, be able to report *effective*
   values read back from the cgroup files. The gap between them is where support
   tickets live ("why did my app hit a thread limit? I set no limit").
2. If we choose the systemd driver we inherit systemd's policy defaults whether we
   want them or not. If we choose the fs driver we get exactly what we write and
   nothing else. That is a real, load-bearing difference, not an implementation detail.

### 2.5 What runc's `fs` vs `systemd` driver split exists for

`[C]` HIGH. runc ships two cgroup managers:

- **`fs` / `fs2`** — `mkdir` under `/sys/fs/cgroup/<parent>/<name>`, write the interface
  files directly.
- **`systemd`** — talk D-Bus to `org.freedesktop.systemd1.Manager`,
  `StartTransientUnit` a `.scope` (or `.slice`), pass resource limits as **systemd unit
  properties** (`MemoryMax`, `CPUQuota`, `TasksMax`, `AllowedCPUs`, …).

The split exists for exactly one reason: **coexistence with systemd's single-writer
rule.** The `fs` driver creates cgroups systemd does not know about; systemd may delete
them, and `systemctl status` will not show the container's resource usage. The `systemd`
driver makes containers first-class systemd objects.

`[C]` HIGH — **and the systemd driver is a superset, not an alternative.** This is the
single most useful thing I found in runc's source. `UnifiedManager.Set()`:

```go
func (m *UnifiedManager) Set(r *configs.Resources) error {
	properties, err := genV2ResourcesProperties(m.fsMgr.Path(""), r, m.dbus)
	...
	if err := setUnitProperties(m.dbus, getUnitName(m.cgroups), properties...); err != nil { ... }
	return m.fsMgr.Set(r)      // <-- ALSO writes the files directly
}
```

It configures systemd **and then** runs the plain file-writing manager over the same
cgroup, because systemd does not model every cgroup knob (there is no systemd property
for `memory.high` nuances, `cpu.max.burst`, `cpu.idle`, arbitrary `Unified` keys, …).
runc even carries a `Unified map[string]string` escape hatch that is translated into
per-file writes.

`[R]` HIGH — **implement the `fs` driver first, and implement it well, because the
systemd driver is built on top of it.** This is not a "pick one" decision; it is a
"which half do I write first" decision, and the answer is unambiguous. Concretely:

```
trait CgroupManager {
    fn create(&self) -> Result<()>;
    fn apply(&self, pid: Pid) -> Result<()>;
    fn set(&self, res: &Resources) -> Result<()>;
    fn stats(&self) -> Result<Stats>;
    fn kill(&self) -> Result<()>;
    fn destroy(&self) -> Result<()>;
}
```

with `FsManager` complete and `SystemdManager { dbus, fs: FsManager }` delegating
`set()` to D-Bus **plus** `self.fs.set()`. That is runc's shape and it is right.

### 2.6 So: `/sys/fs/cgroup/mydocker/` or D-Bus?

`[R]` HIGH — **for the dev VM and the MVP: `/sys/fs/cgroup/mydocker/`. For anything
that runs on a user's real systemd machine long-term: D-Bus.** The reasoning, stated as
a tradeoff rather than a preference:

Arguments for the fs driver first:

- It is ~200 lines of `std::fs::write` and no dependency. The D-Bus driver needs a D-Bus
  client (`zbus`), a `StartTransientUnit` call with a correctly-typed variant array, unit
  naming rules, and systemd-version feature detection (runc has
  `systemdVersion(cm) >= cpuIdleSupportedVersion` guards). That is a lot of surface for
  a project whose point is to learn the kernel.
- It is the only thing that works on a non-systemd host (Alpine/OpenRC, minimal
  containers-in-containers, an initramfs test rig).
- It is required anyway (§2.5).
- **Learning value**: the whole point of this project is owning the runtime core.
  `MemoryMax=` teaches nothing; `memory.max` teaches everything.

Arguments the fs driver loses on, which we must document rather than pretend away:

- `systemctl daemon-reload` / `daemon-reexec` can garbage-collect our cgroups. In
  practice modern systemd leaves unknown top-level cgroups alone, but this is
  **explicitly not guaranteed** and is the documented reason the systemd driver exists.
  `[C]` MED — the mitigation everyone uses is to create the top-level cgroup with
  `Delegate=yes` on our *own* service unit, so `/sys/fs/cgroup/system.slice/
  mydocker.service/` is a delegated subtree and systemd will not touch inside it.
- No `systemctl status mydocker-<id>` , no `systemd-cgtop` grouping, no journal
  correlation.
- On a host where systemd has **not** enabled a controller in
  `system.slice/cgroup.subtree_control`, the fs driver has to enable it itself, which
  is a write into systemd's territory.

`[R]` HIGH — the pragmatic middle, and my actual recommendation:

> Ship the fs driver. Place our root at
> `/sys/fs/cgroup/system.slice/mydocker.service/` when we are running as a systemd
> service with `Delegate=yes` (detect by reading `/proc/self/cgroup` and checking the
> path ends in `.service`), and at `/sys/fs/cgroup/mydocker/` otherwise. Do **not**
> hardcode `/sys/fs/cgroup/mydocker/`.

That one detection — "where am I already, and is it delegated to me?" — costs ten lines
now and is the same detection rootless needs later (§16.2, §17). It is the cheapest
version of the seam that §17 argues for.

`[O]` HOST-A, HIGH — the shape to imitate is visible: `docker-<id>.scope` lives under
`system.slice`, has `Delegate=yes`, and its `cgroup.subtree_control` is empty because
it is the leaf that holds processes.

---

## 3. Concrete limit mapping: CLI flag → OCI field → cgroup v2 file

### 3.0 The table

`[F]`/`[C]` HIGH unless noted. OCI fields are `linux.resources.*` from the OCI Runtime
Spec v1.2.0 (which HOST-A's runc 1.2.5 reports supporting).

| Docker flag | OCI field | cgroup v2 file | value written |
|---|---|---|---|
| `--memory=100m` | `memory.limit` | `memory.max` | `104857600` |
| `--memory-reservation=64m` | `memory.reservation` | `memory.low` *(see 3.1)* | `67108864` |
| *(no flag)* | `memory.checkBeforeUpdate` | — | runc pre-check only |
| `--memory-swap=200m` | `memory.swap` | `memory.swap.max` | `104857600` = swap−mem |
| `--memory-swappiness=0` | `memory.swappiness` | *(no v2 equivalent)* | **silently dropped** |
| `--oom-kill-disable` | `memory.disableOOMKiller` | *(no v2 equivalent)* | **errors in v2** |
| `--kernel-memory` | `memory.kernel` | *(removed in v2)* | ignored |
| `--cpus=1.5` | `cpu.quota`+`cpu.period` | `cpu.max` | `150000 100000` |
| `--cpu-period`/`--cpu-quota` | same | `cpu.max` | `$quota $period` |
| `--cpu-shares=512` | `cpu.shares` | `cpu.weight` | `20` *(see 3.3)* |
| `--cpuset-cpus=0,2` | `cpu.cpus` | `cpuset.cpus` | `0,2` |
| `--cpuset-mems=0` | `cpu.mems` | `cpuset.mems` | `0` |
| `--pids-limit=100` | `pids.limit` | `pids.max` | `100` |
| `--blkio-weight=500` | `blockIO.weight` | `io.weight` | `4954` *(see 3.6)* |
| `--device-read-bps` | `blockIO.throttleReadBpsDevice` | `io.max` | `8:0 rbps=1048576` |
| `--device-write-iops` | `blockIO.throttleWriteIOPSDevice` | `io.max` | `8:0 wiops=100` |
| `--device-cgroup-rule` | `devices[]` | *(eBPF, §14)* | not a file |

Three of those rows are the interesting ones.

### 3.1 `memory.max` vs `memory.high` — hard kill vs backpressure

`[F]` HIGH. Kernel doc, verbatim:

> **memory.max** — "Memory usage hard limit. This is the main mechanism to limit memory
> usage of a cgroup. If a cgroup's memory usage reaches this limit and can't be reduced,
> the OOM killer is invoked."
>
> **memory.high** — "Memory usage throttle limit. If a cgroup's usage goes over the high
> boundary, the processes of the cgroup are throttled and put under heavy reclaim
> pressure."

Operationally: writing `104857600` to `memory.max` means an allocation that would push
`memory.current` past 100 MiB first triggers direct reclaim, and if reclaim cannot free
enough, **the cgroup OOM killer runs and SIGKILLs a process inside the cgroup**. Writing
the same number to `memory.high` means the same allocation succeeds but the allocating
task is put to sleep proportionally to the overage and reclaim is driven hard — the
process gets *slow*, not *dead*. Above `memory.high` the cgroup can keep growing until
`memory.max`.

`[C]` HIGH — **Docker sets only `memory.max`.** `--memory` maps to `memory.limit` maps to
`memory.max`. There is no Docker flag for `memory.high` at all. runc will write it if the
OCI config's `unified` map contains `"memory.high"`, but nothing in the Docker CLI
produces that.

`[R]` HIGH — **this is the most user-visible resource-control design decision we get to
make, and I recommend deviating from Docker, carefully.** The reasoning:

A runtime that only sets `memory.max` gives users hard kills where they expected
backpressure. The failure mode is: a JVM/Python process with a 100 MiB limit briefly
touches 101 MiB during GC, gets SIGKILL'd, and the user sees exit code 137 with no
explanation. Setting `memory.high` slightly below `memory.max` (the pattern systemd and
Kubernetes-adjacent tooling converged on) converts most of those into a latency spike
plus a countable `memory.events high` increment, which we can *report*.

The tradeoff, stated honestly: `memory.high` throttling is implemented by sleeping the
allocating task, and a workload that lives permanently above `high` becomes pathologically
slow rather than failing — the classic "it's not down, it's just infinitely slow" outage
that is harder to debug than a crash. That is a real argument for Docker's behaviour.

`[R]` My concrete proposal: default to `memory.high = max` (Docker-compatible, no
surprise), expose `--memory-high` as a first-class flag, and **always report the
`memory.events high` counter in `mydocker stats`** so the user can see throttling they
did not ask for. Cheap, honest, no behaviour change by default.

`[F]` HIGH — swap accounting differs from v1 and this is a real compatibility trap.
In v1, `memory.memsw.limit_in_bytes` is **memory + swap combined**. In v2,
`memory.swap.max` is **swap only**. runc does the subtraction, and its source is the
clearest statement of the semantics (runc v1.2.5 `libcontainer/cgroups/utils.go`):

```go
func ConvertMemorySwapToCgroupV2Value(memorySwap, memory int64) (int64, error) {
	switch {
	case memory == -1 && memorySwap == 0:  return -1, nil
	case memorySwap == -1, memorySwap == 0: return memorySwap, nil
	case memory == -1:                      return memorySwap, nil
	case memory == 0:
		return 0, errors.New("unable to set swap limit without memory limit")
	case memorySwap < memory:
		return 0, errors.New("memory+swap limit should be >= memory limit")
	}
	return memorySwap - memory, nil     // <-- v1 semantics preserved at the CLI
}
```

`[R]` HIGH. Adopt this conversion verbatim if we want `--memory-swap` to mean what
Docker users think it means. If we invent our own flag, make it `--swap` meaning
*swap only* and say so — but then we are not Docker-compatible on a flag people
copy-paste. `[R]` I'd keep Docker semantics; the conversion is 10 lines.

`[F]` MED. `--oom-kill-disable` has **no cgroup v2 equivalent**. v1's
`memory.oom_control` could disable the killer (and thereby freeze tasks in the cgroup
indefinitely, a documented footgun). v2 removed it. `[C]` Docker on a v2 host errors out
with "OOM kill disable is not supported". `[R]` HIGH — do not implement the flag; if we
accept it for CLI compatibility, **reject it with a clear error**, do not silently ignore
it. Silent ignoring of a *safety* flag is the worst option.

`[F]` MED. `memory.oom.group` (Linux 4.19) is v2's replacement for a different need:
writing `1` makes the cgroup OOM killer kill **every process in the cgroup as a unit**
rather than picking one victim. `[R]` MED — this is closer to what container users
actually expect ("my container OOM'd"), and it is one file write. Consider it behind
`--oom-kill-group`, default off for Docker compatibility. It is also on the kernel's
delegate list (§2.1), so it works rootless.

### 3.2 The read side: detecting an OOM kill correctly

`[F]` HIGH. `memory.events` has six fields; `[O]` HOST-A, live container:

```
$ ssh host-a 'cat …docker-ee88….scope/memory.events'
low 0
high 0
max 0
oom 0
oom_kill 0
oom_group_kill 0
```

Meanings that matter:

- `max` — count of times usage hit `memory.max` and reclaim was forced. **Not** an OOM.
  A healthy-but-tight container has a large `max` and zero `oom_kill`.
- `oom` — count of times the cgroup went OOM, i.e. reclaim failed. Note this can
  increment **without any process being killed**, when the failing allocation is one the
  kernel can return `-ENOMEM` for instead.
- `oom_kill` — count of processes actually SIGKILL'd by the OOM killer.
- `oom_group_kill` — count of group-kill events (`memory.oom.group=1`).

`[R]` HIGH — **the correct OOM detection is: `oom_kill` increased between container start
and container exit.** Not exit code 137. Exit 137 is `128+SIGKILL` and is produced
identically by `mydocker kill`, by a host admin's `kill -9`, and by the OOM killer. Docker's
`OOMKilled: true` field is derived from the cgroup, not from the exit status, and ours
must be too.

`[F]` MED. `memory.events` is **recursive** by default — it aggregates descendants —
unless the cgroup2 mount carries `memory_localevents`. `memory.events.local` is the
always-non-recursive counterpart. `[O]` HOST-A has `memory_localevents` in
`/sys/kernel/cgroup/features` (supported) but **not** in the mount options (not enabled),
so `memory.events` there is recursive. `[R]` MED — since our container cgroup is a leaf
(§1.3), recursive and local are identical for us today; read `memory.events` and note the
distinction in a comment for the day we nest.

`[F]` MED. `memory.events` is **pollable**: `poll(2)` returns `POLLPRI` when a counter
changes. `[R]` MED — this is the v2 replacement for v1's `cgroup.event_control`/eventfd
OOM notification, and it is a much simpler API. For MVP, reading the counter once at exit
is enough; poll it only if we want live `stats`.

`[O]` HOST-A, HIGH — **I could not observe a live OOM.** Every `memory.events` on the
host reads all-zeros and `dmesg | grep -ci "killed process"` is `0`. Not surprising: no
cgroup on this host has a `memory.max` other than `max`:

```
$ ssh host-a 'find /sys/fs/cgroup -maxdepth 3 -name memory.max -exec sh -c \
    "v=$(cat {}); [ \"$v\" != max ] && echo {} = $v" \;'
                     ← (no output)
```

That is itself worth recording: **on a real production Docker host, nobody set any memory
or CPU limit at all.** Defaults are what actually runs. `[EXPERIMENT — dev VM]` in §21.

`[F]` HIGH. `cpu.stat` carries the throttling counters. `[O]` HOST-A:

```
$ ssh host-a 'cat …docker-ee88….scope/cpu.stat'
usage_usec 27499791293   user_usec 22068199650   system_usec 5431591642
nice_usec 0              core_sched.force_idle_usec 0
nr_periods 0   nr_throttled 0   throttled_usec 0   nr_bursts 0   burst_usec 0
```

`[F]` HIGH — `nr_periods`/`nr_throttled`/`throttled_usec` are **only populated when a
bandwidth limit is set**. `nr_periods 0` here despite 7.6 CPU-hours of `usage_usec`
confirms `cpu.max` is `max`. `[R]` HIGH — `throttled_usec` is the single most valuable
number to surface in `stats`: "your container was ready to run but the quota said no, for
N µs". CFS-throttling-induced tail latency is the #1 mystery of containerised services
and this counter is the answer to it.

`[F]` MED (PSI, Linux 4.20; per-cgroup PSI same release). `memory.pressure`,
`cpu.pressure`, `io.pressure` report `some`/`full` stall percentages over 10/60/300 s.
`[O]` HOST-A:

```
$ ssh host-a 'cat …docker-ee88….scope/cpu.pressure'
some avg10=0.00 avg60=0.00 avg300=0.00 total=1139260235
full avg10=0.00 avg60=0.00 avg300=0.00 total=1127315048
```

`some` = at least one task stalled; `full` = *all* runnable tasks stalled. `[R]` LOW/MED —
PSI is strictly better than "CPU % used" for answering "is this container starved", and
it is free to read. But it is a `stats`-quality feature, not an MVP one. Note the
`cgroup.pressure` file (Linux 6.1) can *disable* PSI accounting per-cgroup for overhead;
irrelevant to us.

### 3.3 `cpu.max` vs `cpu.weight` — the flag most users get wrong

`[F]` HIGH. `cpu.max` is a two-field file: `$MAX $PERIOD`, both in microseconds. `max`
in the first field means unlimited. It is a **hard ceiling**: the cgroup gets at most
`$MAX` µs of CPU time per `$PERIOD` µs of wall time, enforced by the CFS bandwidth
controller, and when the budget is exhausted **every task in the cgroup is throttled
until the next period**, even if the machine is idle.

`cpu.weight` is a **proportional share** in `[1, 10000]`, default 100. It only matters
under contention: with two cgroups at weight 100 and 200 both runnable on one CPU, they
get 1/3 and 2/3. With no contention, a weight-1 cgroup can use the whole machine.

`[O]` HOST-A, HIGH — Docker's default is "no ceiling, default share":

```
$ ssh host-a 'cat …docker-ee88….scope/cpu.max …docker-ee88….scope/cpu.weight'
max 100000
100
```

Note `cpu.max` still carries a period (100000 µs = 100 ms) even when unlimited. That is
the kernel default, not something Docker wrote.

`[C]` HIGH. Docker's `--cpus=N` sets `CpuQuota = N × 100000`, `CpuPeriod = 100000`, so
`--cpus=1.5` → `cpu.max = "150000 100000"`. `--cpu-shares` maps to `cpu.weight` via a
range conversion (v1 shares are `[2, 262144]` default 1024; v2 weight is `[1, 10000]`
default 100), so `--cpu-shares=512` lands near `cpu.weight=20`.

`[R]` HIGH — **implement `--cpus` (→`cpu.max`) first and `--cpu-shares` second, and
document the difference in the help text.** `--cpus` is the one users mean. But the
tradeoff is worth stating because it is counter-intuitive:

> `--cpus=1` on a 4-CPU idle machine makes a single-threaded process run at full speed
> and an 8-threaded process run at ~12.5% each with 100 ms throttle stalls. `cpu.weight`
> would let the 8-threaded process use all 4 CPUs when nobody else wants them. Hard caps
> waste idle capacity; weights do not. Weights cannot give you predictable latency.

`[C]` MED — a real gotcha inherited from systemd, from runc v1.2.5
`libcontainer/cgroups/systemd/common.go:addCpuQuota()`:

```go
// systemd converts CPUQuotaPerSecUSec ... to CPUQuota (integer percentage of CPU)
// internally. This means that if a fractional percent of CPU is indicated by
// Resources.CpuQuota, we need to round up to the nearest 10ms (1% of a second)
cpuQuotaPerSecUSec = uint64(quota*1000000) / period
if cpuQuotaPerSecUSec%10000 != 0 {
    cpuQuotaPerSecUSec = ((cpuQuotaPerSecUSec/10000) + 1) * 10000
}
```

**The systemd driver cannot express a CPU quota finer than 1%, and rounds *up*.**
`--cpus=0.005` becomes `--cpus=0.01`. runc then re-writes the exact value via `fsMgr.Set`
afterwards, which is why the file ends up right anyway (§2.5) — but it also means the
D-Bus and file values transiently disagree. `[R]` One more point for owning the fs path.

`[F]` MED. `cpu.max.burst` (Linux 5.14) allows accumulating unused quota up to a burst
budget, smoothing the throttling of bursty workloads. `[O]` HOST-A: present, value `0`.
`[R]` LOW — genuinely useful for latency-sensitive services, not MVP, one file write when
we want it.

`[F]` MED. `cpu.idle` (Linux 5.15) puts the cgroup in `SCHED_IDLE`, i.e. weight ≈ 0.
`[O]` HOST-A: present, `0`. Nice for "best-effort background container"; not MVP.

### 3.4 `pids.max` — the fork-bomb answer

`[F]` HIGH. `pids.max` limits the number of processes+threads in the cgroup and its
descendants. On exceeding it, `fork()`/`clone()` returns **`EAGAIN`**. `pids.current` is
the live count; `pids.peak` (Linux 6.1) is the high-water mark; `pids.events`
(`max <count>`) counts refusals.

`[O]` HOST-A, HIGH:

```
$ ssh host-a 'cd …docker-ee88….scope; cat pids.max pids.current pids.peak pids.events'
19147
72
343
max 0
```

`[R]` HIGH — **`pids.max` is the single highest-value-per-line-of-code limit we can
implement, and it should be on by default with a non-`max` value.** Reasoning: the fork
bomb is the one resource attack that takes down the *host* rather than the container,
because `kernel.pid_max` is global (4194304 here) and PID exhaustion means the host
cannot fork `sshd`, `systemd`, or our own runtime to clean up. Memory exhaustion, by
contrast, is contained by `memory.max`. CPU exhaustion just makes things slow.

`[C]` Docker's default `--pids-limit` is 0 (unlimited) and it relies on the host's
`TasksMax`. `[O]` HOST-A showed this producing an *accidental* limit of 19147 from
systemd (§2.4). `[R]` I would set an explicit default — a few thousand — and say so in
the docs, on the grounds that "protected by accident, on some distros, by a component we
did not configure" is not a safety property.

### 3.5 `cpuset.cpus` / `cpuset.mems`

`[F]` HIGH. `cpuset.cpus` takes a CPU list (`0-3`, `0,2`). An **empty** value means
"inherit the parent's effective set", which is why `[O]` HOST-A shows:

```
cpuset.cpus:            (empty)
cpuset.cpus.effective: 0-3
```

Always read `cpuset.cpus.effective`, never `cpuset.cpus`, when reporting. `[F]` A cgroup
cannot be given CPUs its parent does not have — `write` returns `EINVAL`. `cpuset.mems`
is the NUMA-node analogue and is a no-op on single-node machines (HOST-A is one node).

`[F]` MED. `cpuset` is the one controller whose v2 behaviour is meaningfully different
from v1 beyond file naming: v2 added `cpuset.cpus.partition` for exclusive
("root partition") CPU assignment, which is how you actually get CPU isolation rather
than affinity. Out of scope for MVP.

`[R]` MED — `--cpuset-cpus` is trivial to implement (one write, one parse) and is the
right tool when someone genuinely needs core pinning. Implement it after `--cpus`. Note
that it interacts with the container's view of `nproc`: the container still sees all host
CPUs via `/proc/cpuinfo` and `sysconf(_SC_NPROCESSORS_ONLN)` regardless — cgroups do not
virtualize CPU count. That is a documented cause of thread-pool oversizing in JVM/Go
workloads and is not something we can fix without an LD_PRELOAD-style shim.

### 3.6 `io.max` and `io.weight`

`[F]` HIGH. `io.max` is per-device throttling; format is one line per device:

```
MAJ:MIN rbps=<bytes/s|max> wbps=<...> riops=<...> wiops=<...>
```

Writing only the keys you want is enough; unspecified keys stay `max`. `io.weight` is
proportional (`default 100` plus per-device overrides) and requires a scheduler that
implements it — **`bfq` or the `io.cost` controller**; with `mq-deadline`/`none` and no
`io.cost` model configured, `io.weight` **does nothing**.

`[O]` HOST-A, HIGH — this is precisely the silent-no-op case:

```
$ ssh host-a 'cat …docker-ee88….scope/io.max'
                     ← (empty: no throttling)
$ ssh host-a 'cat …docker-ee88….scope/io.weight'
default 100
$ ssh host-a 'cat /sys/fs/cgroup/io.cost.qos'
                     ← (empty: no io.cost model configured)
$ ssh host-a 'cat …docker-ee88….scope/io.stat'
253:0 rbytes=12152832 wbytes=2596864 rios=228 wios=273 dbytes=0 dios=0
```

`io.weight` is set to its default and there is no cost model, so on this host writing a
different `io.weight` would be accepted by the kernel and have **no effect on scheduling
whatsoever**. `[R]` HIGH — **do not implement `--blkio-weight`.** It is the single
worst ratio of "user believes something happened" to "something happened" in the whole
resource surface. If we implement any I/O control, implement `io.max` (absolute
throttling, always works, no scheduler dependency) and tell users so.

`[F]` MED. `io.max` needs the device major:minor of the **backing block device**, which
for an overlayfs rootfs is not obvious — you must resolve the upperdir's filesystem to
its device. `[O]` HOST-A's `io.stat` shows `253:0` (the dm/vda device) and `253:16`,
and the container rootfs overlay's upperdir lives on `253:1` per `/proc/<pid>/mountinfo`.
`[R]` The `stat(2)` → `st_dev` → `major()/minor()` path is the way; note that for a
device-mapper or LVM stack the *visible* device is not necessarily the one the throttle
applies at. This is a genuine footgun and a reason `io.max` is a P2 feature.

---

## 4. Stopping and killing a container

### 4.1 "Kill the container" ≠ "kill PID 1"

`[F]` HIGH. Two separate reasons, both from D1's territory but with consequences here:

1. **PID 1 in a PID namespace has no default signal handlers** (`pid_namespaces(7)`).
   `SIGTERM` sent to a container PID 1 with no installed handler is **silently
   discarded** — the kernel does not apply the default terminate action for signals sent
   from an ancestor namespace unless the signal is `SIGKILL`/`SIGSTOP`. D1 §7.1 measured
   this. So `kill(pid1, SIGTERM)` can return `0` and do absolutely nothing.
2. Even when PID 1 dies, processes it spawned that **escaped its process group** — or
   that were re-parented and then double-forked — are cleaned up only because the kernel
   SIGKILLs the whole PID namespace when its init exits. That is a real guarantee
   (`pid_namespaces(7)`), *but it only holds if there is a PID namespace*. Run with
   `--pid=host` and it does not, and the leftovers are yours forever.

There is a third, subtler one: processes that were **moved into the container cgroup but
are not descendants of PID 1** — `mydocker exec` children whose parent already exited —
are in the cgroup and not in the process tree. Walking the process tree misses them.
Walking the cgroup does not.

`[R]` HIGH. **The cgroup is the authoritative membership list of a container, not the
process tree.** Every stop/kill path should be cgroup-rooted.

### 4.2 `cgroup.kill` — the race-free way

`[F]` HIGH (Linux **5.14**, `cgroup-v2.rst`):

> **cgroup.kill** — "Writing '1' to the file causes the cgroup and all descendant
> cgroups to be killed. This means that all processes located in the affected cgroup
> tree will be killed via SIGKILL." … "not supported in threaded cgroups".

The important property is **atomicity with respect to fork**. The kernel walks the
cgroup's task list with the appropriate locks held and sets the group-kill flag, so a
process that forks during the operation cannot produce a survivor: the child is born into
the cgroup and inherits the pending kill. There is no loop, no re-read, no
`/proc` scan, no PID-reuse window.

`[O]` HOST-A, HIGH — the file exists on every container cgroup:

```
$ ssh host-a 'ls …docker-ee88….scope/ | grep -E "^cgroup\.(kill|freeze)$"'
cgroup.freeze
cgroup.kill
```

`[R]` HIGH — **`echo 1 > cgroup.kill` is our `SIGKILL` path, unconditionally, whenever
it exists.** One `write(2)`. It is strictly better than every alternative and it is the
thing that makes a from-scratch runtime *correct* rather than approximately correct.

`[F]` MED — `cgroup.kill` is **write-only-`1`**; writing anything else returns `EINVAL`,
and reading it returns `0` always (it is not a state file). It only sends SIGKILL — there
is no `cgroup.signal` for arbitrary signals. That is the limitation that shapes §4.4.

### 4.3 The fallback: walking `cgroup.procs`, and why it is racy

`[F]` HIGH. Pre-5.14 (and in threaded cgroups) the only option is:

```
loop {
    pids = read(cgroup.procs)
    if pids.is_empty() { break }
    for p in pids { kill(p, SIGKILL) }
}
```

Three races, all real:

1. **Fork race.** A process forks between our `read` and our `kill`; the child is in the
   cgroup but not in our list. Hence the loop. The loop terminates only because SIGKILL
   is unblockable *and* because the parent dies — but a fork bomb inside the cgroup can
   in principle outrun a single-threaded killer. `pids.max` is what actually guarantees
   termination, which is another argument for §3.4.
2. **PID reuse.** Between reading PID *p* and `kill(p, …)`, *p* can exit and the number
   be reused by an unrelated host process. On a host with `pid_max = 4194304` (`[O]`
   HOST-A) this is unlikely but not impossible, and it is a **host-corrupting** bug when
   it happens. Mitigation: `pidfd_open(p)` then `pidfd_send_signal(pidfd, …)` —
   `pidfd_open` fails with `ESRCH` if the process is gone and the fd is bound to the
   *process*, not the number. Linux 5.3 for `pidfd_send_signal` on a `pidfd_open` fd.
3. **Freeze race.** The standard mitigation, which runc and crun both use, is to
   **freeze the cgroup first** (§4.5), so nothing can fork while you enumerate.

`[R]` HIGH — implement the fallback as: `freeze → read procs → pidfd_open each →
pidfd_send_signal(SIGKILL) → thaw → verify empty`. And gate it behind a
`cgroup.kill`-exists check so on any modern kernel it is dead code.

### 4.4 What `docker stop` actually does, and what we should do

`[C]` HIGH. `docker stop` = send `SIGTERM` (or the image's `STOPSIGNAL`) to the container's
**PID 1 only**, wait `--time` seconds (default **10**), then `SIGKILL` **the whole
container**. `docker kill` = send the signal to PID 1 immediately. The asymmetry is
deliberate: the graceful phase targets the app's init so the app can shut down its own
children; the forceful phase targets everything.

`[F]` HIGH — the graceful phase is the one with the kernel trap from §4.1: if the image's
entrypoint is a shell script or a binary without a `SIGTERM` handler, running as PID 1,
**the SIGTERM does nothing at all** and every `docker stop` on that image takes the full
10 seconds. This is why `docker stop` feels slow on so many images, and it is a kernel
behaviour, not a Docker one.

`[R]` HIGH — our `stop`:

```
1. read STOPSIGNAL from image config (default SIGTERM)
2. pidfd_send_signal(container_pid1_fd, sig)          # PID 1 only, ancestor ns
3. poll the pidfd for exit, with timeout = --time (default 10s)
   ALSO poll cgroup.events for populated 0
4. on timeout:  write "1" > cgroup.kill               # everything, atomically
   (fallback: freeze; pidfd-kill each; thaw)
5. wait for cgroup.events populated == 0
6. rmdir the cgroup
```

Two design notes worth stating:

- **Poll the pidfd *and* `cgroup.events`.** The pidfd tells us PID 1 exited; `populated 0`
  tells us the cgroup is empty. They are different events and both matter: an
  `exec`-spawned process can outlive PID 1 only if there is no PID namespace, but
  `rmdir` will fail with `EBUSY` until the cgroup is genuinely empty, so step 5 is not
  optional.
- **Do not `SIGKILL` PID 1 as the escalation.** Kill the cgroup. Killing PID 1 relies on
  the PID-namespace-teardown guarantee, which is real but is a *second* mechanism doing
  the work; `cgroup.kill` does it directly and works with `--pid=host` too.

### 4.5 `cgroup.freeze`

`[F]` HIGH (Linux **5.2**, `cgroup-v2.rst`):

> Writing "1" to the file causes freezing of the cgroup and all descendant cgroups. This
> means that all belonging processes will be stopped and will not run until the cgroup
> will be explicitly unfrozen.

Frozen tasks are in `TASK_FROZEN`/`D`-like state; **fatal signals still work** ("processes
can be killed by fatal signals while frozen"), which is exactly what makes the
freeze-then-kill fallback safe. `cgroup.events` gains a `frozen 0|1` field. `[O]`
HOST-A: `cgroup.events` reads `populated 1 / frozen 0`, and `cgroup.freeze` reads `0`.

`[F]` MED — freezing is **asynchronous**. Writing `1` returns immediately; you must poll
`cgroup.events` for `frozen 1` to know it took effect. A task in an uninterruptible sleep
delays it.

`[C]` `docker pause`/`unpause` is exactly this on v2 (it was the `freezer` controller on
v1). `[R]` MED — implement `pause`/`unpause` as two file writes plus a poll; it is nearly
free and it is a genuinely useful primitive (checkpointing, "stop the bleeding" during
incident response, and the kill fallback above).

### 4.6 `rmdir` and the teardown order

`[F]` HIGH. A cgroup directory is removed with `rmdir(2)` — **not** `unlink`, and its
files are not individually removable. `rmdir` fails with `EBUSY` if the cgroup has member
processes or live child cgroups. So teardown is: kill everything → wait for
`populated 0` → `rmdir` children depth-first → `rmdir` self.

`[R]` HIGH — `std::fs::remove_dir` is the right call and `std::fs::remove_dir_all` is
**wrong**: it tries to unlink the interface files first, which fails, and the error is
confusing. Write the recursive rmdir by hand; it is 15 lines.

### 4.7 Dying cgroups: `rmdir` ≠ freed

`[O]` HOST-A, HIGH:

```
$ ssh host-a 'cat /sys/fs/cgroup/cgroup.stat'
nr_descendants 65
nr_dying_descendants 881
```

`[F]` MED. When a cgroup is `rmdir`'d but still has charged page cache, the kernel keeps
an offline ("dying") cgroup object until those pages are reclaimed. 881 of them here
against 65 live ones. Consequences we should know:

- Memory attributed to a deleted container is **not** returned to the system until
  reclaim pressure forces it out. "I deleted the container and free memory didn't move"
  is expected, not a bug.
- There is a per-hierarchy cap (`cgroup.max.descendants`, default `max`) and historically
  there were kernel bugs where large numbers of dying cgroups caused measurable slowdown.
  `[R]` LOW — worth a `stats` line, not worth engineering around.

---

## 5. cgroup v1 vs v2: what differs, how to detect, and whether to bother

### 5.1 The differences that actually change code

| | v1 | v2 |
|---|---|---|
| hierarchy | one tree **per controller**, mounted separately | **one** tree, one mount |
| container identity | map controller→path | one path |
| memory limit | `memory.limit_in_bytes` | `memory.max` |
| "unlimited" | `9223372036854771712` (a magic near-`INT64_MAX`) | the literal string `max` |
| swap | `memory.memsw.limit_in_bytes` = **mem+swap** | `memory.swap.max` = **swap only** |
| soft limit | `memory.soft_limit_in_bytes` (best-effort, largely ineffective) | `memory.low`/`memory.min` (real protection) |
| throttle-before-kill | none | `memory.high` |
| OOM notify | `memory.oom_control` + `cgroup.event_control` eventfd | `memory.events` (pollable) |
| disable OOM killer | `memory.oom_control` | **removed** |
| cpu cap | `cpu.cfs_quota_us` + `cpu.cfs_period_us` (two files) | `cpu.max` (one file, two fields) |
| cpu share | `cpu.shares` `[2,262144]` default 1024 | `cpu.weight` `[1,10000]` default 100 |
| throttle stats | `cpu.stat` (`nr_throttled`,`throttled_time`) | `cpu.stat` (`nr_throttled`,`throttled_usec`) |
| pids | `pids.max` (same) | `pids.max` |
| freeze | `freezer` controller, `freezer.state` | `cgroup.freeze` (core, no controller) |
| mass kill | none | `cgroup.kill` |
| devices | `devices` controller, `devices.allow`/`deny` | **no controller** — eBPF (§14) |
| blkio | `blkio.*` | `io.max`/`io.weight` |
| PSI | none | `*.pressure` |
| rootless | effectively impossible (see 5.3) | delegation is designed in |

`[F]` HIGH on all rows; sources `cgroups(7)`, `cgroup-v2.rst`,
`Documentation/admin-guide/cgroup-v1/*.rst`.

The two rows that cost the most code are **hierarchy** (a map vs a path, threaded through
every function) and **devices** (a file write vs attaching a BPF program).

### 5.2 Detection — v1 / v2 / hybrid

`[F]` HIGH. Three layouts exist:

- **v2 unified**: `/sys/fs/cgroup` is a `cgroup2` filesystem. `statfs(2)` on it gives
  `f_type == CGROUP2_SUPER_MAGIC (0x63677270)`.
- **v1 legacy**: `/sys/fs/cgroup` is a `tmpfs` with per-controller `cgroup`
  (`0x27e0eb`) mounts under it (`memory/`, `cpu,cpuacct/`, …).
- **hybrid**: `tmpfs` with v1 controller mounts **plus** a `cgroup2` mount at
  `/sys/fs/cgroup/unified` carrying no controllers (systemd's transitional mode).

`[R]` HIGH — the detection routine, in order of reliability:

```
1. statfs("/sys/fs/cgroup")
     CGROUP2_SUPER_MAGIC  -> v2 unified          (done)
     TMPFS_MAGIC          -> v1 or hybrid, continue
2. exists("/sys/fs/cgroup/unified/cgroup.controllers") -> hybrid
   else                                                -> v1 legacy
3. cross-check per process: first line of /proc/self/cgroup
     "0::/..."  -> this process is managed by v2
     "N:ctrl:/" -> v1
```

`[O]` HOST-A, HIGH: `findmnt -t cgroup2` shows exactly one mount at `/sys/fs/cgroup`
and `/proc/2590614/cgroup` is a single `0::` line. Pure v2.

`[O]` ROCKY (D1), MED: `/proc/self/cgroup` carries numbered per-controller lines and
there is no `cgroup2` mount at all. Pure v1 legacy. (Not re-verified this session — host
off-network; see §0.1.)

`[R]` MED — note the third check is about **us**, not the host. It is possible to be in a
container on a v2 host with only a v1 view, or vice versa. The runtime's own
`/proc/self/cgroup` is what determines where we can *write*, which is the question that
matters.

### 5.3 v1's broken rootless semantics

`[F]` HIGH. v1 has no delegation model. There is no `cgroup.subtree_control`, no
`/sys/kernel/cgroup/delegate`, no `nsdelegate`, and no containment rule preventing a
delegatee from moving processes out. The "delegate to a user" story in v1 is
`chown -R` on a subtree, which the kernel docs explicitly warn against
(`cgroup-v2.rst`: "cgroup v1 … delegation was done in an ad-hoc manner … the resulting
setups were fragile and often insecure"). Crucially, v1 lets a delegatee write **its own
`memory.limit_in_bytes`**, i.e. raise its own limit, because the limit file is inside the
subtree it owns. v2's rule that controller interface files stay owned by the delegater
(§2.1) is the fix.

`[F]` MED. In practice, on a v1 host, rootless container runtimes **do not set resource
limits at all**. Podman's documentation states resource limits are unsupported in
rootless mode on cgroup v1.

### 5.4 The `old-host` case: no writable cgroup at all

The concrete situation: cgroup v1 legacy, Slurm owns the hierarchy, the user's session is
inside `user.slice/user-<uid>.slice/session-N.scope` for `pids` and `memory` but at the
**root** for `cpu`, and nothing is delegated. `[O]` ROCKY (D1), MED (not re-verified this
session).

`[F]` HIGH — the failure is a plain `EACCES`/`EPERM` on `mkdir` in the controller
directory, or on `open(…, O_WRONLY)` of the limit file. There is nothing subtle about it;
the difficulty is entirely in what the runtime does next.

`[R]` HIGH — **three-state policy, not a boolean.** This is the design recommendation
this section exists for:

```
enum CgroupSupport {
    Full { root: PathBuf, controllers: HashSet<Controller> },
    Partial { root: PathBuf, controllers: HashSet<Controller> },   // some enabled, some not
    None { reason: String },
}
```

and the rule:

1. **Probe at startup, once**, and record the result in the daemon/runtime state.
2. If the user asked for a limit we cannot enforce (`--memory` with no `memory`
   controller): **hard error, before creating anything.** Refusing to run is the only
   honest response to "limit this to 100 MiB" when we cannot.
3. If the user asked for **no** limits and there is no writable cgroup: **run, and warn
   once, loudly, naming the reason.** `"no writable cgroup: /sys/fs/cgroup/pids is
   cgroup v1 and not delegated to uid 1234; resource limits are unavailable"`. Refusing
   to run at all would make the tool useless on exactly the shared machines where a
   from-scratch container tool is most interesting.
4. **Never silently succeed.** The failure mode to design against is `--memory=100m`
   quietly doing nothing, because the user then ships a workload believing it is bounded.

That asymmetry — hard-fail on requested-but-impossible, warn on not-requested — is the
whole answer to "hard error or documented degradation": **both, selected by whether the
user asked.**

### 5.5 Is v1 support worth it?

`[R]` HIGH — **No for the MVP. Detect it, name it, refuse to pretend.** Reasoning:

*For* v1: it is the only cgroup available on the user's only unprivileged Linux box
(ROCKY, 4.18); RHEL 8 is supported until 2029; some HPC and older-cloud fleets are still
v1.

*Against*, and decisive:

- On that specific box v1 is **not writable anyway** — Slurm owns it, nothing is
  delegated. Implementing the v1 driver would not make `--memory` work on the one host it
  was for. We would write a whole second driver and still print the §5.4 warning.
- v1 doubles the cgroup manager's surface: a controller→path map, per-controller mount
  discovery, per-controller failure handling, `9223372036854771712` vs `max`, a separate
  `freezer` path, a separate `devices` path, no `cgroup.kill`.
- The v2-only features our design leans on — `cgroup.kill` (§4.2), `memory.events`
  (§3.2), `CLONE_INTO_CGROUP` (§1.7), delegation (§2) — have **no v1 equivalent**, so v1
  support is not "the same code with different filenames", it is a second, worse
  architecture.
- Every distro we would realistically target defaults to v2: Fedora ≥ 31, Ubuntu ≥ 21.10,
  Debian ≥ 11, RHEL ≥ 9, Arch, openSUSE. RHEL 8 is the only significant holdout and it
  can be *booted* into v2 with `systemd.unified_cgroup_hierarchy=1`.

`[R]` The honest cost of this decision, stated so the architecture-risk pass can price it:
we lose RHEL 8 / CentOS 8 hosts that cannot be rebooted with a changed kernel cmdline.
That is a real fleet. If we ever need them, the v1 driver is a **leaf** addition behind
the `CgroupManager` trait from §2.5 — expensive but not structural, as long as the trait
exists from the start and no code outside it assumes a single path. **That is the seam
that makes this decision reversible; see §17.**

---

## 6. The kernel feature floor

### 6.1 What landed when

`[F]` unless noted; versions from man pages (`clone(2)`, `seccomp(2)`, `pidfd_open(2)`,
`mount_setattr(2)`, `user_namespaces(7)`), `cgroup-v2.rst`, and the OCI/runc
documentation that gates on them.

| Feature | Kernel | We use it for | Without it |
|---|---:|---|---|
| cgroup v2 unified | 4.5 | everything in §1–§4 | v1 driver (§5.5: we won't) |
| cgroup namespace | 4.6 | hiding host cgroup paths | container sees host paths in `/proc/self/cgroup` |
| `nsdelegate`, `/sys/kernel/cgroup/{delegate,features}` | 4.13 / 4.15 | delegation, feature probe | hardcode the 3-file chown list |
| eBPF device cgroup | 4.15 | §14 device control on v2 | **no device control at all on v2** |
| `memory.oom.group` | 4.19 | kill container as a unit | one victim per OOM |
| PSI (`*.pressure`) | 4.20 | starvation stats | no starvation signal |
| seccomp user-notify | 5.0 | §8.7 (not MVP) | no syscall interposition |
| `pidfd_send_signal` | 5.1 | race-free signalling | PID-reuse race |
| `cgroup.freeze` | 5.2 | pause/unpause, kill fallback | v1 `freezer` only |
| `CLONE_PIDFD`, `open_tree`/`move_mount` | 5.2 | pidfd container handle | PID handle |
| `clone3`, `pidfd_open` | 5.3 | 64-bit clone flags, pidfd | legacy `clone` |
| time namespace, `openat2` | 5.6 | TOCTOU-safe path resolution (§11.5) | manual `O_PATH` walk |
| `CLONE_INTO_CGROUP`, `memory_recursiveprot` | 5.7 | born-in-cgroup (§1.7) | move-in window |
| `CAP_BPF`, `CAP_PERFMON`, `CAP_CHECKPOINT_RESTORE` | 5.8 | finer cap split than `CAP_SYS_ADMIN` | those go back to `CAP_SYS_ADMIN` |
| `close_range` | 5.9 | §11.4 fd hygiene before exec | `/proc/self/fd` loop |
| overlayfs in userns | 5.11 | rootless native overlay | `fuse-overlayfs` |
| `CLOSE_RANGE_CLOEXEC` | 5.11 | mark-all-cloexec in one call | per-fd `fcntl` |
| idmapped mounts (`mount_setattr`) | 5.12 | §9.6 — no `chown` of layers | `chown -R` the whole layer |
| `cgroup.kill`, `cpu.max.burst` | 5.14 | §4.2 race-free kill | freeze+loop |
| `cpu.idle` | 5.15 | best-effort containers | — |
| overlayfs **idmapped** | 5.19 | idmapped rootfs, not just volumes | idmap only on non-overlay mounts |
| `memory.reclaim`, `memory.peak` | 5.19 | proactive reclaim, stats | — |
| `cgroup.pressure`, `pids.peak` | 6.1 | stats | — |

`[C]` MED — distro cgroup-v2-by-default: Fedora 31 (2019-10), Ubuntu 21.10, Debian 11,
RHEL 9, Arch, openSUSE Tumbleweed. RHEL 8 / CentOS 8 / Rocky 8 default to **v1**.

### 6.2 What 4.18 (RHEL 8) actually costs us

`[F]`/`[C]` MED. At 4.18 we lose, in rough order of pain: `cgroup.kill`,
`CLONE_INTO_CGROUP`, `clone3` (so no 64-bit clone flags, no time namespace),
`cgroup.freeze` (v2 form), `pidfd_*`, `close_range`, `openat2`, idmapped mounts,
seccomp user-notify, and cgroup v2 by default. We keep: namespaces, seccomp filters,
capabilities, the v1 controllers, PSI (no — 4.20, so no PSI either), and the eBPF device
cgroup (4.15, so yes).

The interesting exception the plan asked about: **overlayfs mounted inside a user
namespace works on ROCKY at 4.18**, per D1's measurement. `[O]` ROCKY (D1), MED.

`[C]` MED — the explanation is a Red Hat backport, not upstream. Upstream gained
unprivileged overlay in **5.11**; Red Hat carried an out-of-tree patch enabling
overlay-in-userns in RHEL 7.5+/8 specifically so rootless Podman could use `overlay`
instead of `fuse-overlayfs`. Red Hat's own documentation describes native rootless
overlay as arriving with kernel 5.11 (RHEL 8.5 notes) while simultaneously shipping the
capability earlier on 4.18 — i.e. the backport predates the upstream feature it is
documented against.

`[R]` HIGH — **do not build any feature detection on kernel version numbers.** This is
the concrete lesson. A `KernelVersion >= (5,11)` check would conclude "no unprivileged
overlay" on a host where it demonstrably works, and a `>= (5,14)` check for
`cgroup.kill` would be right by accident. Detect by **trying**:

```
cgroup.kill        -> access("<cg>/cgroup.kill", W_OK)
CLONE_INTO_CGROUP  -> attempt clone3; fall back on EINVAL/E2BIG
close_range        -> attempt; fall back on ENOSYS
openat2            -> attempt; fall back on ENOSYS
idmapped mounts    -> attempt mount_setattr; fall back on EINVAL/ENOSYS/EOPNOTSUPP
overlay-in-userns  -> attempt the mount; fall back to fuse-overlayfs
```

Every one of these is a syscall that fails cleanly. Version numbers are a lie on
enterprise kernels — RHEL's 4.18 contains code from 5.x, and conversely a vendor kernel
can *remove* things.

### 6.3 Recommended floor

`[R]` HIGH — **Minimum: Linux 5.10. Target and test on: 6.1+. Assume nothing above 5.10
without a runtime probe.**

Why 5.10 and not something else:

- **5.10 is an LTS** with support to 2026, and is the floor for Debian 11, Amazon Linux 2
  (via the 5.10 kernel), and many embedded/cloud images. It is a line real fleets sit on.
- At 5.10 we have: cgroup v2 (4.5), cgroup ns (4.6), delegation files (4.15), eBPF device
  cgroup (4.15), `memory.oom.group` (4.19), PSI (4.20), `pidfd_*` (5.1–5.3), `clone3`
  (5.3), `cgroup.freeze` (5.2), `CLONE_INTO_CGROUP` (5.7), `openat2` (5.6),
  `close_range` (5.9). **That is every mechanism this document treats as load-bearing
  except `cgroup.kill` (5.14) and idmapped mounts (5.12).**
- Those two are exactly the ones with clean, already-designed fallbacks: freeze+pidfd-kill
  loop (§4.3) and `chown` the layer (§9.6). Neither is a structural change.
- Going lower to 4.19 costs `clone3`, `pidfd`, `openat2`, `close_range`, `CLONE_INTO_CGROUP`
  and `cgroup.freeze` — six mechanisms, each needing a separate fallback path. That is a
  real maintenance tax for a research project.
- Going higher to 5.15/6.1 buys `cgroup.kill`, idmapped mounts, `cpu.idle`,
  `cpu.max.burst`, `memory.peak` — nice, none load-bearing.

What each step buys, stated as the plan asked:

| floor | buys over the previous | costs |
|---|---|---|
| 4.18 (RHEL 8) | — | v1 by default, no clone3/pidfd/openat2/close_range/freeze/kill/idmap/PSI |
| 4.19 | `memory.oom.group` | still no pidfd/clone3 |
| 5.10 **(recommended)** | pidfd, clone3, `CLONE_INTO_CGROUP`, `openat2`, `close_range`, `cgroup.freeze`, PSI | no `cgroup.kill`, no idmapped mounts |
| 5.15 | `cgroup.kill`, idmapped mounts, `cpu.idle` | — |
| 6.1 (LTS) | overlayfs idmap (5.19), `memory.reclaim`, `pids.peak`, `cgroup.pressure` | — |

`[R]` MED — practical form of this: the dev VM should run **6.1 or newer** (so
everything works and we learn the modern APIs), and CI, if it ever exists, should have
one 5.10 target to keep the fallbacks honest. Untested fallback paths are worse than
absent ones.

---

## 7. Capabilities

### 7.1 The five sets

`[F]` HIGH, `capabilities(7)`:

| set | what it is |
|---|---|
| **Permitted** (P) | "a limiting superset for the effective capabilities that the thread may assume" |
| **Effective** (E) | "the set of capabilities used by the kernel to perform permission checks" |
| **Inheritable** (I) | "a set of capabilities preserved across an `execve(2)`" — only in combination with file inheritable bits |
| **Bounding** (B) | "a mechanism that can be used to limit the capabilities that are gained during `execve(2)`" |
| **Ambient** (A) | since 4.3; "preserved across an `execve(2)` of a program that is **not privileged**" |

### 7.2 The `execve` transformation, and why bounding is the durable one

`[F]` HIGH, verbatim from `capabilities(7)` (P = before, P' = after, F = file caps):

```
P'(ambient)     = (file is privileged) ? 0 : P(ambient)
P'(permitted)   = (P(inheritable) & F(inheritable)) |
                  (F(permitted) & P(bounding)) | P'(ambient)
P'(effective)   = F(effective) ? P'(permitted) : P'(ambient)
P'(inheritable) = P(inheritable)     [unchanged]
P'(bounding)    = P(bounding)        [unchanged]
```

Read the second line as the security statement it is: **`P(bounding)` masks
`F(permitted)`.** A setuid-root or file-capability binary inside the container can only
gain capabilities that are still in our bounding set. Drop `CAP_SYS_ADMIN` from bounding
and no binary in the image — not `/bin/ping` with `cap_net_raw+ep`, not a setuid root
`mount` — can ever obtain it, for the lifetime of the process and every descendant.

By contrast, dropping a capability only from P and E is **not durable**: an `execve` of a
file with `F(permitted)` containing that capability re-grants it, because `P(bounding)`
still allows it. `[R]` HIGH — **the bounding set is the security boundary; P/E are
convenience.** Our capability drop must always include `PR_CAPBSET_DROP` for every
capability we do not want, and it must happen before the final `execve`.

`[F]` HIGH — the root special case. When a process with **uid 0** execs a file with no
file capabilities, "the file inheritable and permitted sets are ignored; instead they are
notionally considered to be all ones" and "the file effective bit is notionally defined
to be one". Substituting into the formula: `P'(permitted) = P(bounding)`, `P'(effective)
= P(bounding)`. **A root container's process gets back its entire bounding set on every
`execve`, whatever we set P and E to beforehand.** This is exactly why Docker's numbers
look the way they do.

`[O]` HOST-A, HIGH — measured on the live `webapp-web-1` container:

```
$ ssh host-a 'grep -E "^(Uid|Cap|NoNewPrivs|Seccomp)" /proc/2590614/status'
Uid:  0  0  0  0
CapInh: 0000000000000000
CapPrm: 00000000a80425fb
CapEff: 00000000a80425fb
CapBnd: 00000000a80425fb
CapAmb: 0000000000000000
NoNewPrivs: 0
Seccomp: 2      Seccomp_filters: 1

$ ssh host-a 'capsh --decode=00000000a80425fb'
0x00000000a80425fb=cap_chown,cap_dac_override,cap_fowner,cap_fsetid,cap_kill,
  cap_setgid,cap_setuid,cap_setpcap,cap_net_bind_service,cap_net_raw,
  cap_sys_chroot,cap_mknod,cap_audit_write,cap_setfcap
```

P = E = B, I = A = 0. Fourteen capabilities.

### 7.3 The non-root case — measured, and it is the interesting one

`[O]` HOST-A, HIGH — `webapp-redis-1` runs `USER redis` (uid 999):

```
$ ssh host-a 'grep -E "^(Uid|Gid|Cap)" /proc/82271/status'
Uid:  999  999  999  999
Gid: 1000 1000 1000 1000
CapInh: 0000000000000000
CapPrm: 0000000000000000
CapEff: 0000000000000000
CapBnd: 00000000a80425fb
CapAmb: 0000000000000000
```

**P and E are empty; B is still the full 14.** `[F]` HIGH — the mechanism is
`capabilities(7)`, "Effect of user ID changes on capabilities":

> If one or more of the real, effective, or saved set user IDs was previously 0, and as a
> result of the UID changes all of these IDs have a nonzero value, then all capabilities
> are cleared from the permitted, effective, and ambient capability sets.

The bounding set is not in that list, so it survives. That is the correct outcome and it
is the shape we want: the process has no privilege now, and cannot acquire any beyond the
14 later.

`[R]` HIGH — **two consequences for our ordering:**

1. If the image says `USER 999`, our `setuid` must come **after** we have used any
   capability we still need (e.g. `chown`ing something), because `setuid` away from 0
   nukes P and E irreversibly (unless `PR_SET_KEEPCAPS`).
2. If a non-root container genuinely needs a capability — the canonical case is a
   non-root process binding port 80 — clearing P/E on `setuid` means the capability is
   gone. The fix is **ambient capabilities** (§7.5), not "run as root".

### 7.4 `PR_SET_NO_NEW_PRIVS` — what it actually prevents

`[F]` HIGH (`prctl(2)`, Linux 3.5). With `no_new_privs` set, "`execve` promises not to
grant privileges to do anything that could not have been done without the `execve` call".
Concretely it neutralises, for this process and **all descendants, irreversibly**:

- setuid/setgid bits on executables (the uid change simply does not happen),
- file capabilities (`F(permitted)`/`F(effective)` are ignored),
- LSM transitions that would *raise* privilege (AppArmor profile transitions to a more
  permissive profile are denied).

It cannot be unset. It is inherited across `fork` and `execve`.

`[F]` HIGH — its second role, from `seccomp(2)`: installing a seccomp filter requires
**either** `CAP_SYS_ADMIN` in the caller's user namespace **or** `no_new_privs` already
set; otherwise `seccomp(2)` returns `EACCES`. The reason is that a filter could otherwise
be used to trick a setuid binary (by making a syscall fail in a way its author never
anticipated) into misbehaving with elevated privilege.

`[O]` HOST-A, HIGH — **and Docker does not set it.** `NoNewPrivs: 0` on all three
containers I checked, with `Seccomp: 2` (filter mode) and one filter installed. So runc
took the "install the filter while still holding `CAP_SYS_ADMIN`" branch, exactly as D1
§7.2 predicted.

`[C]` HIGH — Docker sets `no_new_privs` only with `--security-opt no-new-privileges`.
The reason it is not the default is **compatibility**: NNP breaks every image that relies
on a setuid binary inside the container (`sudo`, `su`, `ping` on distros that still ship
it setuid, `passwd`, `newgrp`, Nginx dropping privileges via a setuid helper).

`[R]` HIGH — **our default: `no_new_privs` ON, with a documented `--no-new-privileges=false`
escape hatch.** Reasoning:

- It is a genuine, cheap escalation barrier: with the default 14-capability set a
  container *root* already cannot do much, but a container process that escapes to a
  setuid-root binary can. NNP closes that.
- It makes seccomp installation possible **after** we have dropped capabilities, which is
  the ordering we want anyway (§7.7) and which removes a `CAP_SYS_ADMIN`-shaped branch
  from our code.
- The compatibility cost is real but is *loud*: `sudo` fails with a clear error rather
  than silently doing nothing.

This is a deliberate deviation from Docker and should be listed as such.

### 7.5 Ambient capabilities

`[F]` HIGH (Linux 4.3). Ambient capabilities survive `execve` of an **unprivileged**
(no-setuid, no-file-caps) binary, subject to the invariant that a capability can only be
ambient if it is in **both** P and I. Raising one: `prctl(PR_CAP_AMBIENT,
PR_CAP_AMBIENT_RAISE, cap, 0, 0)`, which fails with `EPERM` if the cap is not in P and I,
or if it has been dropped from B. The ambient set is cleared entirely if the exec'd file
is privileged, and by the uid-change rule in §7.3.

`[R]` HIGH — **needed for exactly one scenario, but it is a common one:** a non-root
container process that must retain a capability across `execve`. `--user 1000` +
`--cap-add NET_BIND_SERVICE` is meaningless without ambient caps, because the `setuid`
clears P/E and the plain formula gives `P'(effective) = P'(ambient) = 0`. The correct
sequence:

```
capset(): put NET_BIND_SERVICE in P and I
prctl(PR_SET_KEEPCAPS, 1)             # survive the uid change
setgid/setgroups/setuid(1000)         # P survives thanks to KEEPCAPS; E is still cleared
capset(): restore E from P
prctl(PR_CAP_AMBIENT_RAISE, NET_BIND_SERVICE)
execve()                              # ambient -> P' and E'
```

`[C]` MED — OCI's `process.capabilities` has all five fields (`bounding`, `effective`,
`inheritable`, `permitted`, `ambient`) precisely so a runtime can express this. runc
implements it in `libcontainer/capabilities`. `[R]` Implement `ambient` from the start —
it is one extra `prctl` loop — but it is fine for the MVP to leave it empty.

### 7.6 Docker's default set, and what is dangerous

`[C]` HIGH — moby v28.3.1 `oci/caps/defaults.go`, verbatim, 14 capabilities:

```
CAP_CHOWN            CAP_DAC_OVERRIDE      CAP_FSETID       CAP_FOWNER
CAP_MKNOD            CAP_NET_RAW           CAP_SETGID       CAP_SETUID
CAP_SETFCAP          CAP_SETPCAP           CAP_NET_BIND_SERVICE
CAP_SYS_CHROOT       CAP_KILL              CAP_AUDIT_WRITE
```

`[O]` HOST-A confirms this is exactly `0xa80425fb` (§7.2). Of ~41 capabilities, 27 are
dropped.

**`CAP_SYS_ADMIN` is effectively root**, and it is worth spelling out *why*, because it is
the single most important thing in this section. `capabilities(7)` lists it as granting,
among ~25 unrelated operations: `mount(2)`, `umount(2)`, `pivot_root(2)`, `setns(2)`,
namespace creation, `bpf(2)`, `perf_event_open(2)`, checkpoint/restore, writing `trusted.*`
and `security.*` xattrs, `keyctl(2)`, `fanotify_init(2)`, overriding `RLIMIT_NPROC`,
device-driver administration, modifying device cgroup rules, `ioprio_set`, forging a PID
in a `SCM_CREDENTIALS` message, and `TIOCSTI`. Any *one* of the first four is a complete
escape:

- `mount(2)`: mount the host's block device, or bind-mount host `/` into the container.
- `pivot_root`/`setns`: re-enter the host namespaces if any handle is reachable.
- writing `security.*` xattrs: rewrite the SELinux/AppArmor label on a file.
- device cgroup rules: grant yourself `/dev/sda` (on v1; on v2 you need `CAP_BPF` too).

It is a grab-bag of unrelated privileges that accumulated because it was the "misc" bucket
— the kernel's own docs acknowledge this. Treating it as "one capability among many" is
the most common security mistake in container tooling.

The rest, ranked by what they buy an attacker:

| capability | in Docker default? | why it matters |
|---|---|---|
| `CAP_SYS_ADMIN` | **no** | see above — equivalent to root on the host |
| `CAP_SYS_MODULE` | no | `init_module` — load a kernel module. Total game over, no mitigation |
| `CAP_SYS_RAWIO` | no | `iopl`/`ioperm`, `/dev/mem` — read/write physical memory |
| `CAP_SYS_PTRACE` | no | `ptrace` any process **in the same PID namespace**; combined with `--pid=host` it is host code injection. Also `pidfd_getfd`, `process_vm_readv` |
| `CAP_DAC_READ_SEARCH` | no | bypass file *read* permission **and** enables `open_by_handle_at` — the Shocker/`CVE-2014-…` class: brute-force a host inode handle and read host files from inside the container |
| `CAP_BPF` (5.8) | no | load BPF programs. Split out of `CAP_SYS_ADMIN` to make this grantable separately; still very powerful (kprobes need `CAP_PERFMON` too) |
| `CAP_PERFMON` (5.8) | no | `perf_event_open` — a long history of LPE bugs |
| `CAP_NET_ADMIN` | no | configure host networking **if netns is shared**; D4 owns the detail, we own the fact that it gates veth creation |
| `CAP_NET_RAW` | **yes** | raw/packet sockets. Enables ARP spoofing and DNS poisoning **of the container network**, and is the reason CIS benchmarks recommend dropping it. Docker keeps it so `ping` works |
| `CAP_SETUID`/`CAP_SETGID` | **yes** | switch to any uid *inside the container*. Harmless without a userns *only because* container uid 0 == host uid 0 already, i.e. it is not adding privilege. **With** a userns it is confined to the mapped range. It also lets a process escape a `--user` restriction back to container-root |
| `CAP_SETPCAP` | **yes** | modify capability sets. Cannot *add* beyond bounding, so it is safe-ish; it exists so an in-container init can drop caps |
| `CAP_MKNOD` | **yes** | create device nodes. Neutered by `MS_NODEV` on the container's mounts and by the device cgroup — and D1 measured that it is a **no-op outside the initial user namespace** anyway |
| `CAP_DAC_OVERRIDE` | **yes** | bypass file permission checks *in the container's filesystem*. Since container root usually owns everything, this is close to a no-op; it is also what makes `--user` less of a boundary than people think |
| `CAP_AUDIT_WRITE` | **yes** | write audit records. Present only so `login`/`su`/`sshd` inside containers work |
| `CAP_CHOWN`,`CAP_FOWNER`,`CAP_FSETID`,`CAP_KILL`,`CAP_SETFCAP`,`CAP_SYS_CHROOT`,`CAP_NET_BIND_SERVICE` | **yes** | ordinary container-local operations |

`[R]` HIGH — **adopt Docker's 14 as the default** (compatibility, and it is a
well-tested set), but:

- make `--cap-drop=ALL --cap-add=…` the documented recommendation,
- consider dropping `CAP_NET_RAW` by default and requiring `--cap-add NET_RAW` for
  `ping`. This is what several hardening guides and some downstreams already do; the cost
  is one confusing error the first time someone pings.
- **Refuse `--cap-add SYS_ADMIN` unless combined with `--privileged` or an explicit
  `--i-know-what-this-does` flag.** It is not a capability, it is a mode.

### 7.7 The correct ordering

`[R]` HIGH — the sequence in the container child, after the mount namespace is set up and
before `execve`. Every step's position is forced by a rule above:

```
 1. prctl(PR_SET_KEEPCAPS, 1)                    # only if we will setuid and need caps after
 2. setgroups(supplementary)                     # needs CAP_SETGID; must precede setgid
 3. setgid(gid) ; setuid(uid)                    # clears P,E,A  (unless KEEPCAPS for P)
 4. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)     # needs CAP_SETPCAP -> before dropping it!
 5. capset(): set P, E, I to the wanted sets
 6. for cap in ambient: prctl(PR_CAP_AMBIENT_RAISE, cap)   # requires cap in P and I
 7. prctl(PR_SET_NO_NEW_PRIVS, 1)
 8. seccomp(SECCOMP_SET_MODE_FILTER, TSYNC, &prog)
 9. execve()
```

The traps, each of which is a real bug someone has shipped:

- **`PR_CAPBSET_DROP` requires `CAP_SETPCAP` in the *effective* set.** If step 5 runs
  before step 4 and drops `CAP_SETPCAP` from E, step 4 fails with `EPERM` and the bounding
  set is left wide open — silently, if you ignore the return value. Drop bounding first.
- **`setgroups` before `setgid`.** After `setgid` away from a privileged gid you may no
  longer have `CAP_SETGID`. And forgetting `setgroups` entirely leaves the container
  process in the *host's* supplementary groups — a real privilege leak, and the reason
  `setgroups(0, NULL)` must be called even when the group list is empty.
- **Order 3 vs 4/5 is genuinely contested.** runc drops capabilities and *then* changes
  uid, using `PR_SET_KEEPCAPS`. Doing it the other way (uid first) is simpler but requires
  KEEPCAPS anyway if any capability is wanted afterwards. `[R]` Follow runc: bounding
  drop → capset → uid change with KEEPCAPS → capset again → ambient.
- **Step 8 must be last-but-one.** Anything after it runs under the filter. If the filter
  denies a syscall our own code makes between 8 and 9, we deadlock or die confusingly.
  See §8.6.
- **`execve` itself must be allowed by the filter** (§8.6).

---

## 8. seccomp

### 8.1 Mechanics

`[F]` HIGH, `seccomp(2)`. A seccomp filter is a **cBPF** program (classic BPF, *not*
eBPF) attached to the thread. On every syscall entry the kernel builds:

```c
struct seccomp_data {
    int   nr;                   /* syscall number */
    __u32 arch;                 /* AUDIT_ARCH_* */
    __u64 instruction_pointer;
    __u64 args[6];
};
```

and runs every installed filter over it. The program returns a 32-bit value: the high 16
bits select the action, the low 16 (`SECCOMP_RET_DATA`) carry data (an errno, or a trace
message). **All filters run; the most severe result wins** — that is the documented
precedence order, and it means filters can only ever be *added*, never relaxed.

`[F]` HIGH — action precedence, decreasing severity:

| action | effect |
|---|---|
| `SECCOMP_RET_KILL_PROCESS` | terminate the whole thread group, core dump |
| `SECCOMP_RET_KILL_THREAD` | terminate only this thread; others keep running |
| `SECCOMP_RET_TRAP` | thread-directed `SIGSYS` |
| `SECCOMP_RET_ERRNO` | syscall not executed; returns `-SECCOMP_RET_DATA` as errno |
| `SECCOMP_RET_USER_NOTIF` | hand off to a userspace supervisor over a notify fd |
| `SECCOMP_RET_TRACE` | notify a `ptrace` tracer |
| `SECCOMP_RET_LOG` | log, then allow |
| `SECCOMP_RET_ALLOW` | allow |

`[R]` HIGH — **use `SECCOMP_RET_ERRNO(EPERM)` as the default action, never
`KILL_PROCESS`.** Reasoning: `ERRNO` is what every real workload can cope with — a
libc that probes for a new syscall (`statx`, `openat2`, `clone3`, `faccessat2`,
`close_range`) and gets `EPERM`/`ENOSYS` falls back to the old one and keeps working. A
`KILL_PROCESS` default turns "your glibc is newer than your container runtime's profile"
into an unexplained SIGSYS crash, which is the single most common seccomp support
question in the wild. `KILL_THREAD` is even worse: it kills one thread of a
multi-threaded process and leaves a wedged program.

`[C]` HIGH — Docker's default profile agrees: `"defaultAction": "SCMP_ACT_ERRNO"`,
`"defaultErrnoRet": 1` (EPERM).

`[R]` MED — one nuance worth stealing from Docker: `clone3` is denied with
**`errnoRet: 38` (`ENOSYS`)**, not EPERM (§8.5). `ENOSYS` means "this kernel does not
have it" and triggers glibc's fallback to `clone`; `EPERM` means "you may not" and
glibc does *not* fall back. Choosing the right errno per syscall is a real design
decision, not a formality.

### 8.2 `prctl` vs `seccomp(2)`

`[F]` HIGH. `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog)` is the legacy interface;
`seccomp(SECCOMP_SET_MODE_FILTER, flags, &prog)` (Linux 3.17) is the modern one and is
the **only** way to pass flags. `[R]` HIGH — use `seccomp(2)`. It is not in glibc, so
call it via `syscall(SYS_seccomp, …)`; `libc::syscall` in Rust.

`[F]` HIGH — the permission rule (§7.4): `EACCES` unless `CAP_SYS_ADMIN` in the caller's
userns **or** `no_new_privs`. Two code branches unless we mandate NNP — which is one more
reason §7.4 recommends mandating it.

### 8.3 The architecture problem

`[F]` HIGH. `seccomp_data.arch` is an `AUDIT_ARCH_*` value, and **syscall numbers are
per-ABI**. On x86_64:

- `AUDIT_ARCH_X86_64` — the 64-bit ABI. `read` = 0, `write` = 1, …
- `AUDIT_ARCH_I386` — the 32-bit compat ABI, reachable from a 64-bit kernel by a 32-bit
  binary or by `int 0x80`. `read` = 3, `write` = 4, … **completely different numbering.**
- **x32** — uses `AUDIT_ARCH_X86_64` as its `arch` value but ORs `__X32_SYSCALL_BIT`
  (`0x40000000`) into `nr`.

The classic bypass, in the man page's own words:

> A list of system calls to be denied based on `nr` that does not also contain `nr`
> values with `__X32_SYSCALL_BIT` set can be bypassed.

Concretely: a denylist that blocks `nr == 101` (`ptrace`) on `AUDIT_ARCH_X86_64` does not
block `nr == 0x40000065`, which the kernel dispatches to the same `ptrace`. And a filter
that only ever checks `AUDIT_ARCH_X86_64` lets a 32-bit binary in the image call anything
via `AUDIT_ARCH_I386`.

`[R]` HIGH — **three mandatory rules, all cheap:**

1. **Allowlist, not denylist.** With `defaultAction = ERRNO`, an unknown `nr` — including
   every x32-bit-set number and every i386 number — is denied by construction. The x32
   bypass simply does not exist for an allowlist. This alone is the strongest argument
   for the allowlist design.
2. **Check `arch` first**, and if it is not one we generated rules for, deny. The
   canonical prologue is `if (arch != AUDIT_ARCH_X86_64) return ERRNO`.
3. If we ever emit a denylist rule, mask `nr` against `~__X32_SYSCALL_BIT` or emit both.

`[C]` HIGH — Docker's profile declares an `archMap` where `SCMP_ARCH_X86_64` has
`subArchitectures: ["SCMP_ARCH_X86", "SCMP_ARCH_X32"]`, i.e. **it generates the full
allowlist three times, once per ABI**, resolving the syscall *names* to the correct
numbers for each. That is what `libseccomp` does for you and it is the main thing
`libseccomp` is for.

`[F]` MED — a related caveat from `seccomp(2)`: **vDSO calls bypass seccomp entirely.**
`clock_gettime`, `gettimeofday`, `time`, `getcpu` are usually served from the vDSO
without entering the kernel, so a filter denying them may or may not take effect. Never
build a security property on filtering those.

### 8.4 Why you cannot dereference pointers, and multiplexed syscalls

`[F]` HIGH. `seccomp_data.args[]` holds the **raw register values**. A cBPF filter runs
in a context where it cannot dereference user memory, deliberately: if it could, the
value it read could be changed by another thread between the filter's read and the
kernel's own read — a **TOCTOU** that would turn every filter into a false sense of
security. So:

- **You cannot filter `open("/etc/shadow")` by path.** You can only see the pointer.
- **You cannot filter `mount()` by source or filesystem type.**
- **You cannot filter `execve()` by program name.**

You *can* filter on scalar arguments: flags, fds, ints, and constants.

`[F]` HIGH — the multiplexed-syscall problem is the same issue one level up. On i386,
`socketcall(2)` multiplexes `socket`, `bind`, `connect`, `accept`, … behind one syscall
number, with the real operation in `args[0]` and the *actual arguments in a userspace
array* — so a filter can distinguish `socket` from `bind` (that is a scalar) but cannot
see their arguments. Same for the older `ipc(2)`. `ioctl(2)` is the modern instance: you
can filter on the `request` number (scalar) but not on the pointed-to structure — so
`TIOCSTI` (the classic "inject characters into the controlling terminal" escape) *is*
filterable, but `SIOCSIFADDR`'s target interface is not.

`[C]` MED — Docker's profile uses exactly this power where it can. `[O]` from
`profiles/seccomp/default.json` v28.3.1:

```json
{"names":["socket"],"action":"SCMP_ACT_ALLOW",
 "args":[{"index":0,"value":40,"op":"SCMP_CMP_NE"}]}
```

`AF_VSOCK` is 40 — **`socket(AF_VSOCK, …)` is denied, every other family allowed**,
purely because the family is a scalar in `args[0]`.

And the `clone` rule, which is the neatest example in the whole profile:

```json
{"names":["clone"],"action":"SCMP_ACT_ALLOW",
 "excludes":{"caps":["CAP_SYS_ADMIN"]},
 "args":[{"index":0,"value":2114060288,"op":"SCMP_CMP_MASKED_EQ"}]}
```

`2114060288 = 0x7E020000 = CLONE_NEWNS|CLONE_NEWCGROUP|CLONE_NEWUTS|CLONE_NEWIPC|
CLONE_NEWUSER|CLONE_NEWPID|CLONE_NEWNET`. The masked-equality compares
`(args[0] & 0x7E020000) == 0` — i.e. **`clone` is allowed only when it requests no new
namespaces.** That is how Docker blocks nested user namespaces without blocking `fork`.

### 8.5 What Docker's default profile actually is

`[C]` HIGH, from moby v28.3.1 `profiles/seccomp/default.json` (13 KB, 31 syscall groups),
analysed directly:

- `defaultAction: SCMP_ACT_ERRNO`, `defaultErrnoRet: 1` (**EPERM**).
- **It is an allowlist**, not a blocklist. 361 syscalls allowed unconditionally; 426
  distinct names allowed across all groups. The common summary "Docker blocks 44
  syscalls" is a description of the *outcome*, not the mechanism, and it is misleading —
  the mechanism denies everything not named.
- Denied **unconditionally**, i.e. even for a container running as root with default caps:

  ```
  keyctl  add_key  request_key         — kernel keyring is shared across containers
  kexec_load  kexec_file_load          — replace the running kernel
  pivot_root                            — escape the mount namespace
  userfaultfd                           — the exploit primitive of choice for kernel heap
                                          grooming / race widening
  swapon  swapoff                       — host-global
  io_uring_setup/enter/register         — large, young, repeatedly-CVE'd attack surface
  create_module get_kernel_syms query_module uselib ustat nfsservctl _sysctl vm86 vm86old
                                        — obsolete, removed, or trivially dangerous
  ```

- Denied **conditionally on capability**. The profile carries `includes.caps` / `excludes.caps`
  annotations, and Docker resolves them against the container's *actual* capability set at
  create time, emitting a flat OCI list. With the default 14 caps this means `mount`,
  `umount2`, `unshare`, `setns`, `bpf`, `perf_event_open`, `ptrace`, `process_vm_readv/writev`,
  `pidfd_getfd`, `init_module`, `syslog`, `open_by_handle_at`, `clock_settime`, `iopl`,
  `ioperm`, `reboot`, `quotactl` and friends are all denied — because their gating
  capabilities (`CAP_SYS_ADMIN`, `CAP_SYS_PTRACE`, `CAP_BPF`, `CAP_PERFMON`,
  `CAP_SYS_MODULE`, `CAP_SYS_RAWIO`, `CAP_SYS_TIME`, `CAP_DAC_READ_SEARCH`, …) are not in
  the default set.

`[R]` HIGH — **the cap↔seccomp correlation is defence in depth and we should copy it.**
The capability check and the seccomp check are enforced by different kernel subsystems;
a bug in one is covered by the other. A `CAP_SYS_ADMIN`-less process that somehow
acquires the capability (a kernel bug, a setuid binary with NNP off) still cannot call
`mount` if the filter says no. Note that this makes the filter **per-container**, derived
from the container's capability set — it is not one static blob.

`[R]` MED — **the cost of a wrong default, both directions:**

- *Too strict*: the workload dies or misbehaves in a way that looks nothing like a
  security control. The historical examples are exact and instructive: Docker's profile
  had to be amended for `clock_adjtime` (chrony), `statx` (new glibc), `faccessat2`
  (glibc 2.33 — this one broke Ubuntu 21.04 containers on old Docker en masse),
  `clone3` (glibc 2.34 — same, and the reason for the ENOSYS trick), `close_range`,
  `openat2`, `membarrier`. **Every glibc release is a potential incident.** Mitigation:
  default `ERRNO`, prefer `ENOSYS` for syscalls that have a fallback, and ship a
  `--seccomp=unconfined` escape hatch.
- *Too loose*: a container-local bug becomes a host compromise. `userfaultfd` and
  `io_uring` are both in the profile's denylist because of specific exploit chains, not
  theory.

`[R]` HIGH — **for the MVP: ship a small, hand-written allowlist and grow it, or ship
Docker's JSON.** I would ship Docker's JSON parsed by our own code, because (a) it is
already correct about the ABI problem and about glibc's probing behaviour, (b) it gives
us Docker compatibility for free, and (c) writing our own allowlist is the exact activity
most likely to produce a mysterious workload failure two months in. The *learning* is in
implementing the BPF assembler and the arch handling, not in curating the syscall list.

### 8.6 Where in the sequence the filter goes

`[F]` HIGH. A filter installed in the child applies to that thread and, with
`SECCOMP_FILTER_FLAG_TSYNC`, to all threads of the process. It is inherited across `fork`
and **across `execve`**. It cannot be removed.

`[R]` HIGH — **install it in the container child, immediately before `execve`, and make
sure `execve` is in the allowlist.** Consequences:

- **`execve` (and `execveat`) must be allowed**, or the container's own entrypoint dies
  before it starts, with `EPERM` from a syscall the user never made. Docker's profile
  allows both. This is a real, easy self-inflicted wound.
- Anything our runtime code does *between* installing the filter and `execve` runs under
  the filter. Keep it to zero syscalls: build the `CString` argv/envp before installing
  the filter (which D1 §11.2 already requires for the no-allocation rule).
- Installing it **before** dropping capabilities means the filter must permit whatever
  the drop needs (`prctl`, `capset`); installing it **after** — which §7.7 does — means
  the filter can be minimal. Install last.

`[F]` HIGH — `SECCOMP_FILTER_FLAG_TSYNC` "synchronize[s] all other threads of the calling
process to the same seccomp filter tree", and fails (returning the offending TID rather
than `-1`) if any thread cannot be synchronised. `[R]` MED — for the container child we
are single-threaded by D1's invariant, so TSYNC is a no-op. Pass it anyway: it costs
nothing and it is correct if that invariant is ever violated. **The failure mode without
TSYNC in a multithreaded process is a filter that applies to one thread only** — i.e. a
security control that silently does not exist.

### 8.7 `SECCOMP_RET_USER_NOTIF`

`[F]` MED (Linux 5.0; `SECCOMP_FILTER_FLAG_NEW_LISTENER`). The filter returns
`USER_NOTIF`, the kernel blocks the caller and posts a notification on an fd that a
supervisor reads; the supervisor can respond with a value/errno, or (5.5+)
`SECCOMP_USER_NOTIF_FLAG_CONTINUE` to let the syscall proceed, or (5.9+) inject an fd
with `SECCOMP_IOCTL_NOTIF_ADDFD`.

`[C]` MED — this is how rootless Podman implements `--mount` of things an unprivileged
user cannot mount, and how `crun`'s `seccomp-notify` plugin handles `mknod`, `chown` and
`setxattr` in rootless containers: the syscall is intercepted, performed by a privileged
helper, and the result handed back.

`[F]` HIGH — **the security caveat is severe and often missed**: the notification carries
the syscall's *raw arguments*, so a supervisor that dereferences a pointer argument via
`/proc/<pid>/mem` has the exact TOCTOU that §8.4 exists to prevent. Doing it safely
requires `SECCOMP_IOCTL_NOTIF_ID_VALID` to confirm the notification is still live, and
even then pointer-following is only sound for immutable data. `[R]` HIGH — **do not use
user-notify in the MVP.** It is the right tool for rootless `mknod`/`chown` much later
(§16), and if we build it, the `ID_VALID` re-check is not optional.

---

## 9. User namespaces and UID/GID mapping

### 9.1 The `uid_map` / `gid_map` write rules

`[F]` HIGH, `user_namespaces(7)`. `/proc/<pid>/uid_map` and `gid_map` contain up to
**340** lines (**5** before Linux 4.15) of `ID-inside-ns  ID-outside-ns  length`. Each
file may be written **exactly once**; a second write returns `EPERM`. Writing requires
all of:

1. `CAP_SETUID` (`CAP_SETGID` for `gid_map`) in the user namespace of the target process;
2. the writer is in the target's userns **or its parent**;
3. every outside-ID being mapped already has a mapping in the parent userns;
4. **either** (a) the write is a *single line* mapping the writer's own effective UID in
   the parent namespace, and the writer's eUID equals that of the process that created
   the namespace; **or** (b) the writer has `CAP_SETUID` in the **parent** userns, in
   which case arbitrary maps are allowed.

Rule 4(a) is "the unprivileged single-mapping rule" and is the entire reason
`unshare -Ur` gives you uid 0 and nothing else.

`[F]` HIGH — the `setgroups` requirement. Before an unprivileged writer may write
`gid_map`, `/proc/<pid>/setgroups` must contain `deny` (write `"deny"` to it; this is
irreversible for that namespace). Why: without it, a user could create a userns, call
`setgroups(2)` to **drop** a supplementary group, and thereby gain access to a file whose
mode denies the group but permits others — e.g. mode `rwx---rwx`. Group membership can
*subtract* permission in Unix, so being able to drop a group is a privilege escalation.
Blocking `setgroups` until a `gid_map` exists closes it.

`[R]` HIGH — the correct order, and it must be done by a process **outside** the new
namespace (D1's three-process structure exists for this):

```
child:  unshare/clone(CLONE_NEWUSER)  ->  blocks on a pipe
parent: write "deny"        > /proc/<child>/setgroups     # unprivileged path only
        write "<map>"       > /proc/<child>/uid_map
        write "<map>"       > /proc/<child>/gid_map
        signal the pipe
child:  continues; now has capabilities in the new userns
```

`[F]` HIGH — D1 measured the consequence of getting the timing wrong: **capabilities
gained from `CLONE_NEWUSER` do not survive `execve` unless uid 0 is mapped.** So
`uid_map` timing is a correctness issue, not a convenience.

### 9.2 Root inside vs root outside

`[F]` HIGH. With mapping `0 100000 65536`, container uid 0 is host uid 100000. A process
with full capabilities *in that namespace* has, on host resources, the authority of uid
100000 — which is none. That is the entire security value: `CAP_SYS_ADMIN` inside the
namespace lets you `mount` a tmpfs in your own mount namespace, and lets you do nothing
at all to a host file.

`[O]` HOST-A, HIGH — and Docker does **not** use it:

```
$ ssh host-a 'cat /proc/2590614/uid_map /proc/2590614/gid_map'
         0          0 4294967295
         0          0 4294967295
$ ssh host-a 'ls -l /proc/2590614/ns/user'
user -> user:[4026531837]      # the host's initial user namespace
```

Identity map over the whole range = no user namespace at all. **Container root *is* host
root on the default Docker configuration.** Every other control in this document —
capabilities, seccomp, AppArmor, masked paths — exists to make that acceptable. That is
the honest framing of what Docker's security model is.

`[O]` HOST-A, HIGH — Ubuntu 24.10 also restricts *unprivileged* userns creation:

```
$ ssh host-a 'cat /proc/sys/kernel/apparmor_restrict_unprivileged_userns'
1
$ ssh host-a 'cat /proc/sys/kernel/unprivileged_userns_clone'
1
$ ssh host-a 'cat /proc/sys/user/max_user_namespaces'
63826
```

`[F]` MED — `apparmor_restrict_unprivileged_userns=1` is an Ubuntu-specific AppArmor
feature (23.10+): an unprivileged process may still *create* a userns, but is stripped of
capabilities inside it unless an AppArmor profile grants `userns create`. **This will
break a rootless track on Ubuntu specifically**, in a way that looks like a
capabilities bug rather than an LSM policy. `[R]` Detect it (read the sysctl) and produce
a message naming AppArmor, not "permission denied".

### 9.3 `/etc/subuid`, `/etc/subgid`, and the setuid helpers

`[F]` HIGH. `/etc/subuid` and `/etc/subgid` are `shadow-utils` files of the form
`user:start:count`, granting a user the right to map that host-UID range into namespaces
they create. The kernel knows nothing about them: they are consulted by
**`newuidmap(1)`/`newgidmap(1)`**, which are `setuid`-root (or file-capability
`cap_setuid=ep` / `cap_setgid=ep`) helpers that perform the privileged `uid_map` write on
the unprivileged caller's behalf, after checking the range against those files.

`[O]` HOST-A, HIGH:

```
$ ssh host-a 'cat /etc/subuid /etc/subgid'
deploy:100000:65536
deploy:100000:65536
```

One user, 65536 UIDs starting at 100000 — the standard `useradd` allocation.

`[O]` ROCKY (D1), MED — **no `/etc/subuid`/`subgid` entries at all**, and the user cannot
add them (no root). `newuidmap`/`newgidmap` are installed but have nothing to authorise.
So the only available mapping is rule 4(a): **a single UID**. `unshare -Ur` works and
gives uid 0 in-namespace; nothing else is mapped.

`[R]` HIGH — **what concretely breaks with a single-UID map**, which is the question the
plan asked and the answer is sharp:

| operation | result |
|---|---|
| extract an image layer containing a file owned by uid 100 | `chown(…, 100, …)` → **`EINVAL`** (not `EPERM`) — uid 100 has no mapping, so it is not a representable identity in this namespace |
| `tar -x` of such a layer as "root" | tar reports the chown failure per file; with `--no-same-owner` it silently makes everything uid 0 |
| a process trying to `setuid(65534)` (`nobody`) | **`EINVAL`** — same reason |
| an image whose entrypoint is `USER nobody` | container fails to start |
| a daemon that drops privilege internally (nginx worker, postgres, redis) | fails at `setuid`, usually fatally |
| files in the image owned by uid 0 | fine — 0 is the one mapped id |

`[F]` MED — the errno being **`EINVAL` rather than `EPERM`** is the operationally useful
detail: `EPERM` would suggest "get more privilege"; `EINVAL` correctly says "that identity
does not exist here". Error messages should say so. *(This specific errno is `MED` rather
than `HIGH` because I could not re-run it on ROCKY this session — see §23.)*

`[R]` HIGH — the consequence for our design: **rootless with a single-UID map is not a
degraded mode, it is a different mode.** Any image with more than one UID in it simply
does not work. The engine should detect the absence of a subuid range at startup and say:
`"rootless: only uid 0 is mappable (no /etc/subuid entry for <user>); images with
multiple UIDs will fail"` — up front, not on the first `chown` error 400 files into a
layer extraction.

### 9.4 Where the UIDs actually collide: layer ownership

`[F]` HIGH. An OCI image layer is a tar with numeric uid/gid. Extracted as root, a file
owned by uid 0 in the tar is owned by host uid 0. In a rootless container with map
`0 → 100000`, that same file must be owned by **host uid 100000** for the container's
uid-0 process to own it.

`[C]` HIGH — the two historical answers:

1. **`chown` the whole layer on extraction** (podman's classic behaviour): correct,
   simple, and costs a full `chown` walk of every layer, per user, plus a second copy of
   the storage if two users share an image. On a large image this is minutes and
   gigabytes.
2. **`user.*` xattr-based shifting** (`userxattr` overlay option): D3 owns the mechanics;
   the privilege-relevant fact is that in a userns, `trusted.*` xattrs are unavailable
   (they need `CAP_SYS_ADMIN` in the *initial* userns), so overlayfs must be told to use
   `user.overlay.*` instead of `trusted.overlay.*` — that is the entire purpose of the
   `userxattr` mount option (Linux 5.11). `[O]` HOST-A's *rootful* Docker overlay mounts
   with **`nouserxattr`**, confirming the option is the rootless-specific one.

### 9.5 Idmapped mounts — the modern answer

`[F]` HIGH (Linux **5.12**, `mount_setattr(2)`). An idmapped mount applies a UID/GID
translation **at the mount, not on disk**. The mechanism:

```
fd = open_tree(AT_FDCWD, "/var/lib/mydocker/layers/abc", OPEN_TREE_CLONE|AT_RECURSIVE)
userns = open("/proc/<pid_with_the_map>/ns/user")     # a userns holding the desired map
mount_setattr(fd, "", AT_EMPTY_PATH|AT_RECURSIVE,
              &(struct mount_attr){ .attr_set = MOUNT_ATTR_IDMAP,
                                    .userns_fd = userns })
move_mount(fd, "", AT_FDCWD, "/target", MOVE_MOUNT_F_EMPTY_PATH)
```

The on-disk file stays uid 0; through *this mount* it appears as uid 100000 (or the
reverse). No copy, no walk, instant, and shareable between users with different maps
because each gets its own mount.

`[F]` MED — filesystem support is per-filesystem and arrived progressively: ext4/xfs/fat
in 5.12, btrfs 5.15, **overlayfs in 5.19**, others later. That last one matters: idmapping
the *container rootfs* (an overlay) needs 5.19, whereas idmapping a *bind-mounted volume*
needs only 5.12.

`[R]` HIGH — **this is the right long-term answer and it is also a good near-term one for
rootful.** Even with root, `--volume /host/data:/data` into a container running as uid
1000 currently requires either `chown`ing the host directory or `:z`-style relabelling.
An idmapped bind mount solves it with zero on-disk change. `[R]` Put it behind a runtime
probe (§6.2), with the fallback being "document that the host directory's ownership must
match".

`[R]` MED — this is one of the **structurally cheap** rootless retrofits, precisely
because it lives entirely in the mount-construction code (D1/D3's territory) and does not
change the privilege model of anything else. Contrast §17.

---

## 10. `/proc`, `/sys`, and masking

### 10.1 Why a plain `mount -t proc proc /proc` is not enough

`[F]` HIGH. Mounting a fresh procfs in a new PID namespace gives correct *process*
isolation — D1 measured that without it the container sees all host PIDs. But procfs is
not only a process listing. It is also:

- **`/proc/sys`** — the entire `sysctl` tree, host-global and writable by root. Writing
  `/proc/sys/kernel/core_pattern` gives host code execution (the pattern can be a
  `|program`, executed by the kernel **on the host** when any process dumps core). This is
  the single most direct "container root → host root" path and it needs no CVE.
- **`/proc/kcore`** — a readable ELF image of **all physical memory**. Reading it dumps
  host kernel memory, other containers' memory, and any secret on the machine.
- **`/proc/sysrq-trigger`** — writing `c` panics the host; `b` reboots it immediately.
- **`/proc/<pid>/*` of host processes** — only if the PID namespace is shared, but
  `/proc/self/mem`, `/proc/self/exe` and `/proc/self/fd` are dangerous even in-container
  (§11).
- **`/proc/irq`, `/proc/bus`, `/proc/fs`** — host hardware and filesystem configuration.

None of these are hidden by a PID namespace. **PID isolation and procfs isolation are
different problems.** Masking is what solves the second one.

### 10.2 `maskedPaths` and `readonlyPaths`

`[C]` HIGH — moby v28.3.1 `oci/defaults.go`:

```go
maskedPaths := []string{
    "/proc/asound", "/proc/acpi", "/proc/interrupts", "/proc/kcore", "/proc/keys",
    "/proc/latency_stats", "/proc/timer_list", "/proc/timer_stats", "/proc/sched_debug",
    "/proc/scsi", "/sys/firmware", "/sys/devices/virtual/powercap",
}   // + dynamically computed CPU thermal-throttle paths
readonlyPaths := []string{
    "/proc/bus", "/proc/fs", "/proc/irq", "/proc/sys", "/proc/sysrq-trigger",
}
```

Why each matters:

| path | what it leaks or allows |
|---|---|
| `/proc/kcore` | all physical memory, readable |
| `/proc/keys` | kernel keyring contents (host-wide) |
| `/proc/timer_list`, `/proc/sched_debug`, `/proc/latency_stats`, `/proc/interrupts` | **KASLR-defeating kernel pointers** and host process names/scheduling info |
| `/proc/acpi` | firmware tables; some are writable and can affect host power state |
| `/proc/scsi` | writable host SCSI control (`/proc/scsi/scsi` can add/remove devices) |
| `/proc/asound` | host sound devices |
| `/sys/firmware` | DMI/EFI tables, EFI variables — `efivarfs` writes can brick a machine |
| `/sys/devices/virtual/powercap` | RAPL energy counters → a **side channel** for AES key recovery (PLATYPUS) |
| `/proc/sys` (ro) | `core_pattern`, `modprobe`, `panic_on_oops`, `kexec_load_disabled`, … |
| `/proc/sysrq-trigger` (ro) | instant host panic/reboot |
| `/proc/bus`, `/proc/fs`, `/proc/irq` (ro) | PCI config, filesystem tunables, IRQ affinity |

### 10.3 How masking is implemented — measured

`[O]` HOST-A, HIGH — from `/proc/2590614/mountinfo` of the live container:

```
657 1026 0:103 /     /proc/acpi        ro,relatime  - tmpfs tmpfs  ro
663 1026 0:104 /     /proc/scsi        ro,relatime  - tmpfs tmpfs  ro
664 1029 0:105 /     /sys/firmware     ro,relatime  - tmpfs tmpfs  ro
658 1026 0:94  /null /proc/interrupts  rw,nosuid    - tmpfs tmpfs  rw,size=65536k,mode=755
659 1026 0:94  /null /proc/kcore       rw,nosuid    - tmpfs tmpfs  rw,size=65536k,mode=755
660 1026 0:94  /null /proc/keys        rw,nosuid    - …
661 1026 0:94  /null /proc/latency_stats …
662 1026 0:94  /null /proc/timer_list  …
598 1026 0:91  /bus  /proc/bus         ro,nosuid,nodev,noexec - proc proc rw
599 1026 0:91  /fs   /proc/fs          ro,…
600 1026 0:91  /irq  /proc/irq         ro,…
621 1026 0:91  /sys  /proc/sys         ro,…
641 1026 0:91  /sysrq-trigger /proc/sysrq-trigger ro,…
```

So the implementation is exactly two rules, and both are visible:

- **Masked *directory*** → bind an empty read-only **`tmpfs`** over it. (Source device
  `0:103`, `0:104`, `0:105` — three fresh tmpfs superblocks.)
- **Masked *file*** → bind **`/dev/null`** over it. Note the source: `0:94 /null` — that
  is the container's **own `/dev` tmpfs**, path `/null`. So `/dev` must be populated
  *before* masking runs.
- **Read-only path** → bind the path onto itself from the same procfs superblock (`0:91`)
  and remount `ro`.

`[O]` HOST-A, HIGH — and a detail that is easy to get wrong: of the 12 masked paths in
the source, only 7 appear in the live mountinfo. The missing ones —
`/proc/asound`, `/proc/timer_stats`, `/proc/sched_debug`, `/sys/devices/virtual/powercap`
— **do not exist on this kernel**:

```
$ ssh host-a 'for p in /proc/asound /proc/sched_debug /proc/timer_stats \
    /sys/devices/virtual/powercap; do printf "%s " $p; [ -e $p ] && echo PRESENT || echo ABSENT; done'
/proc/asound ABSENT   /proc/sched_debug ABSENT
/proc/timer_stats ABSENT   /sys/devices/virtual/powercap ABSENT
```

`[R]` HIGH — **masking must silently skip `ENOENT` and must not skip any other error.**
A blanket "ignore errors" hides a failed mask; a blanket "fail on error" makes the
runtime refuse to start on half the kernels in existence.

`[R]` HIGH — two more implementation requirements that follow from the mechanism:

- The read-only remount must be done with `MS_REMOUNT|MS_BIND|MS_RDONLY` **after** the
  bind, and on ≥ 5.12 preferably with `mount_setattr(AT_RECURSIVE)` (D1 §9.3) — a
  non-recursive read-only bind leaves submounts writable, which for `/proc/sys` would be
  a complete bypass.
- The `nosuid,nodev,noexec` on `/proc` and `ro,nosuid,nodev,noexec` on `/sys` are visible
  in the same mountinfo and are not optional.

### 10.4 `hidepid`

`[F]` MED (`proc(5)`). `mount -o hidepid=2` (or `hidepid=invisible` in the newer
`hidepid=off|noaccess|invisible` naming, Linux 5.8) makes `/proc/<pid>` directories of
other users' processes invisible. `gid=<g>` exempts a group.

`[R]` LOW/MED — **not useful for us.** In a PID namespace the container only sees its own
processes anyway, and inside a container everything usually runs as the same uid, so
`hidepid` hides nothing. It is a host-hardening measure that container tooling
occasionally proposes and that solves a problem the PID namespace already solved. Skip it.

### 10.5 A writable `/sys/fs/cgroup` in the container

`[F]` HIGH — this is the `release_agent` escape and it is worth stating exactly. On
**cgroup v1**, each hierarchy root has a `release_agent` file containing a **host path**
that the kernel executes, **as root in the initial namespaces**, when a cgroup in that
hierarchy becomes empty (triggered per-cgroup by `notify_on_release`). So:

```
mount -t cgroup -o rdma cgroup /tmp/cg      # any v1 controller not already mounted
mkdir /tmp/cg/x ; echo 1 > /tmp/cg/x/notify_on_release
echo "/path/on/host/to/script" > /tmp/cg/release_agent
echo $$ > /tmp/cg/x/cgroup.procs ; exit     # cgroup empties -> host runs the script as root
```

**CVE-2022-0492** is the version of this that needed no `--privileged`: the kernel failed
to verify that the writer of `release_agent` had `CAP_SYS_ADMIN` **in the initial user
namespace**, so an unprivileged user who could create a userns (thereby getting
`CAP_SYS_ADMIN` inside it) and mount a v1 cgroup hierarchy could write it. Fixed by
requiring `CAP_SYS_ADMIN` in the init userns and that the file be opened from the initial
namespaces.

`[F]` HIGH — **cgroup v2 has no `release_agent`.** It was deliberately removed; v2 uses
`cgroup.events`/`populated` notification instead. This is a real, concrete security
argument for v2-only that is worth adding to §5.5.

`[O]` HOST-A, HIGH — Docker mounts the container's cgroup **read-only** and rooted at
the container's own cgroup:

```
1031 1029 0:29 /system.slice/docker-ee88….scope /sys/fs/cgroup \
     ro,nosuid,nodev,noexec,relatime - cgroup2 cgroup rw,nsdelegate,memory_recursiveprot
```

Field 4 (the mount root) is the container's cgroup directory, not `/` — combined with the
cgroup namespace this makes the container see its own cgroup as `/`. And the mount is
`ro`. `[R]` HIGH — copy this exactly: **cgroup namespace + bind of the container's own
cgroup + `ro`**. The `ro` is what stops a container from writing its own `memory.max` back
to `max`; the namespace is what stops it from even naming the host paths.

---

## 11. Container escape classes, and the requirements they impose

### 11.1 CVE-2019-5736 — `/proc/self/exe` re-open

`[F]` HIGH. Mechanism: when runc enters an existing container (`docker exec`) it
`execve`s itself inside the container's namespaces. At that moment the container can open
`/proc/self/exe`, which resolves to the **host's runc binary**, `O_WRONLY`, and overwrite
it with `#!/proc/self/exe` or arbitrary code. The next invocation of runc on the host runs
attacker code **as root, outside the container**. The trick works because the file
descriptor is obtained by the *victim's* own path (`/proc/self/exe` in the container's
`/proc` refers to the host inode) — no host path is needed.

`[C]` HIGH — the fix: runc copies itself into a **`memfd_create`** anonymous file, seals
it with `F_SEAL_WRITE`, and executes *that*, so `/proc/self/exe` inside the container
points at a read-only memfd. (Earlier variants used `O_TMPFILE` in a private tmpfs, then
`memfd`.)

`[R]` HIGH — **the requirement on our code:** *any* helper binary we `execve` inside the
container's namespaces must not be the on-disk host binary. Two acceptable designs:

1. `memfd_create(MFD_CLOEXEC|MFD_ALLOW_SEALING)` → copy our own executable → seal
   `F_SEAL_WRITE|F_SEAL_SHRINK|F_SEAL_GROW` → `fexecve()`. Costs one copy of our binary
   per exec.
2. Structure the runtime so that **nothing of ours ever execs inside the container's
   namespaces** — the child process that joins the namespaces is already a `fork`
   (inheriting our memory image), never an `execve` of the host binary, and it `execve`s
   only the *container's* entrypoint. D2 owns the shape here.

`[R]` Design 2 is cleaner and is available to us because we are designing the process
model from scratch. **Prefer it, and treat "we never `execve` the host runtime binary
inside container namespaces" as an invariant with a comment naming this CVE.** If we ever
need design 1, `memfd` + seal is not optional — an unsealed memfd is writable.

### 11.2 CVE-2022-0492 — cgroup v1 `release_agent`

Covered in §10.5. `[R]` HIGH — requirements: (a) never mount cgroupfs writable in a
container; (b) prefer v2, which has no `release_agent` at all; (c) if we ever support v1,
`CAP_SYS_ADMIN` must be out of the default set (it is) and the seccomp filter must deny
`mount` (it does, via the capability correlation in §8.5). Three independent controls,
any one of which stops it — that is what defence in depth looks like in practice.

### 11.3 CVE-2019-14271 — `docker cp` library injection

`[F]` MED. Mechanism: the `docker-tar` helper `chroot`ed into the **container's**
filesystem and then, being a dynamically-linked Go binary using cgo/nsswitch, loaded
`libnss_*.so` **from the container's `/lib`**. A malicious image ships a hostile
`libnss_files.so`; the helper runs it **as root on the host**, outside any container
confinement.

`[R]` HIGH — the requirement is broader than `docker cp`: **no host-privileged process of
ours may ever resolve a dynamic library, a config file, an interpreter, or a `PATH`
lookup from container-controlled filesystem contents.** Concretely for a Rust engine:

- Statically link, or at minimum never `chroot`/`pivot_root` into container storage in a
  process that still has host privilege and may dlopen anything.
- Beware NSS specifically: any call that touches `getpwnam`/`getaddrinfo` in glibc can
  dlopen. In Rust, `std::net::ToSocketAddrs` and the `users` crate reach it. Under a
  chroot into container storage this is the exact bug. Using musl-static, or simply never
  chrooting a privileged process, avoids it.
- The file-copy operation should be done by **reading through an `O_PATH`/`openat2`
  handle rooted at the container's rootfs**, not by entering it.

### 11.4 CVE-2024-21626 — the leaked-fd / `WORKDIR` class

`[F]` HIGH. Mechanism: runc left an internal file descriptor (to `/sys/fs/cgroup`, and in
variants to the host root) **open across `execve`** into the container. A container image
with `WORKDIR /proc/self/fd/7` (or an OCI `process.cwd` set to it) starts its process with
its **current working directory inside the host filesystem**. From a host-rooted cwd,
`open("../../../etc/shadow")` walks the host. No exploit code needed — just a Dockerfile.

`[R]` HIGH — **this is the most direct requirement on our own code in this entire
document, and it is easy to satisfy:**

1. **Set `O_CLOEXEC` on every fd we ever create.** In Rust, `std::fs::File` and
   `std::net` set it by default; **`libc`/`nix` raw calls do not**. Every `libc::open`,
   `libc::socket`, `libc::pipe`, `libc::eventfd`, `libc::inotify_init` in our code must
   carry the `*_CLOEXEC` flag. `nix` exposes them; use them.
2. **Belt and braces: `close_range(3, ~0U, 0)` immediately before `execve`** in the
   container child (Linux 5.9). One syscall, closes everything above stderr regardless of
   what any dependency leaked. Fallback for < 5.9: iterate `/proc/self/fd` — but note the
   directory fd itself must be excluded and the read must be done into a buffer first.
3. **Validate `process.cwd`.** After `pivot_root`, resolve the requested cwd with
   `openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)` from the rootfs fd. `RESOLVE_NO_MAGICLINKS`
   is what specifically defeats `/proc/self/fd/N`, since those are magic links.
4. **Verify after the fact**: `chdir`, then confirm the cwd's `st_dev` matches the
   rootfs's. Cheap, and catches classes we did not anticipate.

`[R]` The general principle, worth stating once: **`execve` is the security boundary, and
everything that crosses it is part of the container's attack surface** — file descriptors,
environment, cwd, signal dispositions, resource limits, the mount namespace. Enumerate
them explicitly. Anything you did not decide about is a thing you leaked.

### 11.5 Symlink / TOCTOU in mount handling

`[F]` HIGH. The pattern: the runtime is asked to mount a volume at `/data` inside the
container. It computes `<rootfs>/data` and calls `mount()`. But `<rootfs>/data` is a
**symlink the image controls** — pointing at `/proc/sys` or at `../../..`. The mount lands
outside the rootfs. **CVE-2021-30465** is exactly this in runc, made reliable by a race:
two concurrent `runc run`s where one repeatedly swaps a directory for a symlink while the
other resolves the path.

`[R]` HIGH — requirements:

- **Never build a container path by string concatenation and pass it to `mount(2)`.**
- Resolve every in-container destination with `openat2(dirfd=rootfs_fd, …,
  RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS)` (Linux 5.6). This is the
  only *kernel-enforced* answer and it is why `openat2` is on our floor list (§6).
- Fallback for < 5.6: the `filepath-securejoin` algorithm — resolve component by
  component with `O_PATH|O_NOFOLLOW`, re-rooting any absolute symlink at the rootfs — and
  then **operate on the resulting fd** via `/proc/self/fd/<n>`, never on the recomputed
  path. Re-resolving by path after checking it is the race.
- Do the mounts **inside the container's mount namespace after `pivot_root`**, where
  `/data` is genuinely `/data` and there is no host to escape to. This turns a security
  problem into a non-problem for most cases and is the structural fix.

### 11.6 `/proc/self/fd` and magic links generally

`[F]` HIGH. `/proc/<pid>/fd/N`, `/proc/<pid>/exe`, `/proc/<pid>/root`, `/proc/<pid>/cwd`
are **magic links**: `open()`ing one re-opens the underlying file directly, ignoring the
path entirely — and, crucially, **ignoring the mount namespace and any chroot**. That is
why they defeat `chroot` (D1 §5.5), why CVE-2019-5736 works, and why CVE-2024-21626 works.

`[R]` HIGH — three defences, all already listed: `RESOLVE_NO_MAGICLINKS` on every
path resolution that touches container-controlled input; `close_range` before `execve`;
and never leaving a privileged process's `/proc` visible to the container (the container
has its own procfs, so its `/proc/self` refers to itself — which is why a *fresh* procfs
mount, not a bind of the host's, is required).

### 11.7 The summary requirement list

`[R]` HIGH — extracted from the above, these are code-level invariants:

1. Never `execve` the host runtime binary inside container namespaces (or use a sealed memfd).
2. `O_CLOEXEC` on every fd; `close_range` before `execve`; verify.
3. Resolve every container path with `openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS)`, or
   `securejoin` + operate-on-fd.
4. Never mount cgroupfs writable; prefer v2 (no `release_agent`).
5. No host-privileged process resolves libraries/config from container storage.
6. Mask `/proc` and `/sys` before the container's first instruction, skipping only `ENOENT`.
7. `pivot_root`, never `chroot`; `MS_REC|MS_SLAVE` on `/` first (D1).
8. Validate `process.cwd` after `pivot_root`, by fd and by `st_dev`.

---

## 12. LSMs: AppArmor and SELinux

### 12.1 AppArmor — measured, and what it adds

`[O]` HOST-A, HIGH — AppArmor is enabled and every container is confined:

```
$ ssh host-a 'cat /proc/2590614/attr/current'
docker-default (enforce)
$ ssh host-a 'aa-status | head -3'
apparmor module is loaded.  140 profiles are loaded.  43 profiles are in enforce mode.
$ ssh host-a 'aa-status | grep -c docker-default'
23                     # 23 confined processes in that profile
$ ssh host-a 'ls /etc/apparmor.d/docker*'
ls: cannot access '/etc/apparmor.d/docker*': No such file or directory
```

`[C]` HIGH — note the last line: **the profile is not on disk.** dockerd generates it from
a Go template and loads it into the kernel directly via `apparmor_parser`. There is no
file to inspect, which is why the source is the only reference.

`[C]` HIGH — the profile itself, moby v28.3.1 `profiles/apparmor/template.go`, is
remarkably short. The load-bearing rules:

```
profile docker-default flags=(attach_disconnected,mediate_deleted) {
  network, capability, file, umount,          # <- everything allowed by default
  signal (receive) peer=unconfined,
  signal (receive) peer=runc,
  signal (send,receive) peer=docker-default,

  deny @{PROC}/* w,                            # no writes to files directly in /proc
  deny @{PROC}/sys/[^k]** w,                   # /proc/sys writable only under kernel/
  deny @{PROC}/sys/kernel/{?,??,[^s][^h][^m]**} w,   # ... and only shm*
  deny @{PROC}/sysrq-trigger rwklx,
  deny @{PROC}/kcore rwklx,
  deny mount,
  deny /sys/[^f]*/** wklx,   deny /sys/f[^s]*/** wklx,
  deny /sys/fs/[^c]*/** wklx, deny /sys/fs/c[^g]*/** wklx, deny /sys/fs/cg[^r]*/** wklx,
  deny /sys/firmware/** rwklx,
  deny /sys/devices/virtual/powercap/** rwklx,
  deny /sys/kernel/security/** rwklx,
  ptrace (trace,read,tracedby,readby) peer=docker-default,
}
```

`[R]` HIGH — read that carefully, because it tells you what an LSM is *for* here:

- `network, capability, file, umount,` — bare, unqualified: **allow all** network, all
  capability use, all file access, all umounts. The profile is a **denylist on top of
  allow-everything**, not a confinement.
- The value it adds beyond capabilities+seccomp is in exactly three places:
  1. **`deny mount`** — a *second* enforcement of the `CAP_SYS_ADMIN`/seccomp `mount`
     denial, in a different subsystem.
  2. **Path-based denial of `/proc` and `/sys` writes**, which seccomp fundamentally
     **cannot express** (§8.4: you cannot filter `open()` by path). This is the one thing
     only an LSM can do. It is a backstop for a masking bug — if we forgot to mask
     `/proc/sysrq-trigger`, AppArmor still denies it.
  3. **`signal` and `ptrace` peer rules** — a container process may only signal/ptrace
     processes in the *same* profile, so even with `--pid=host` it cannot ptrace host
     daemons.
- The `/sys/fs/cg[^r]*` sequence is a hand-rolled "deny everything under `/sys` except
  `/sys/fs/cgroup`" written as a chain of negated-character-class globs, because AppArmor
  has no "deny except" operator. It is ugly and it is load-bearing.

`[R]` HIGH — **our position on AppArmor: do nothing in the MVP, then load a profile at
stage N.** Reasoning, and the "what happens if we do nothing" answer the plan asked for:

- If we do nothing, containers we start on an AppArmor host run **unconfined**. `[O]`
  This is observable: `/proc/<pid>/attr/current` would read `unconfined`. Our containers
  would be strictly *less* confined than Docker's on the same host, and the difference is
  exactly the three items above.
- Concretely, "do nothing" means: a `/proc` masking bug becomes exploitable rather than
  merely embarrassing, and a container can `ptrace` any host process in its PID namespace.
- The cost of doing something is small but not zero: generate the profile text, shell out
  to `apparmor_parser -Kr`, and set `/proc/self/attr/exec` to the profile name **before**
  `execve` (the write is to `attr/exec`, applied at the next exec — writing `attr/current`
  is a *change_hat*-style immediate transition and is not what we want).
- `[R]` Order of work: **seccomp and capabilities first, masking second, AppArmor third.**
  AppArmor is a backstop for a mistake in the first two; building the backstop before the
  primary control is the wrong order.

`[F]` MED — one detail that would bite us: with `no_new_privs` set (§7.4), AppArmor
profile *transitions* are restricted to profiles that are not more permissive. Loading
`docker-default` at exec while NNP is set works (it is more restrictive than unconfined),
but this interaction has historically produced confusing `EACCES` on exec. Test it
explicitly.

### 12.2 SELinux

`[F]` HIGH. SELinux is **type enforcement**: every process has a domain
(`container_t`), every file a type (`container_file_t`), and policy states which domains
may do what to which types. Container support adds **MCS** (Multi-Category Security): each
container gets a unique category pair, e.g. `system_u:system_r:container_t:s0:c123,c456`,
and the policy says a process may only access files carrying its own categories. That is
what makes container-to-container isolation work even when both are `container_t`.

`[C]` HIGH — this is why `:z` and `:Z` exist on Docker/Podman volume mounts. A host
directory bind-mounted into a container has the host's label (e.g. `home_t`) and
`container_t` may not touch it, so the container gets `Permission denied` on a directory
it plainly owns. `:z` relabels the source to `container_file_t` with **shared** (no)
categories; `:Z` relabels with the container's **private** categories. `:Z` on a directory
two containers share breaks the second one. Both **modify the host filesystem's labels**,
which is why `-v /etc:/host-etc:Z` is a documented way to break your machine.

`[R]` MED — **do nothing for SELinux, and say so.** Reasoning: on an enforcing host our
containers would either run in an unconfined domain (if our binary is unconfined) or fail
in ways we cannot debug without policy expertise. Implementing it means labelling the
rootfs, setting `/proc/self/attr/exec` to a computed context, generating unique MCS
categories and tracking them so they are not reused, and relabelling volumes. That is a
substantial subsystem. `[O]` ROCKY has SELinux disabled and HOST-A is AppArmor, so we
have **no SELinux test host at all** — which is a decisive practical argument.

`[R]` The seam that keeps it cheap later: whatever we do for AppArmor
(`write /proc/self/attr/exec` before `execve`, from a `SecurityLabel` value computed at
create time) is **the same seam SELinux uses** — same file, same timing, different string.
Design it as `Option<SecurityLabel>` written to `attr/exec`, not as an AppArmor-specific
function.

---

## 13. `--privileged` is a bundle, not a switch

`[C]` HIGH. `docker run --privileged` is not one kernel flag. It is a **policy bundle**
that Docker applies at spec-construction time. Precisely, it:

| turns off | mechanism | consequence |
|---|---|---|
| capability restriction | `process.capabilities.*` = **all** capabilities in every set | `CAP_SYS_ADMIN` ⇒ `mount`, `setns`, `pivot_root`, BPF, kernel keyring — root on the host in practice |
| device restriction | device cgroup rule becomes `a *:* rwm` (allow all) **and** the host's `/dev` is bind-mounted in | `/dev/sda`, `/dev/mem`, `/dev/kmsg` all present and usable |
| `/proc` masking | `maskedPaths` = `[]` | `/proc/kcore` readable; **`/proc/sys/kernel/core_pattern` writable** |
| `/proc` read-only paths | `readonlyPaths` = `[]` | `/proc/sysrq-trigger` writable ⇒ instant host reboot |
| seccomp | profile = `unconfined` | `mount`, `keyctl`, `kexec_load`, `userfaultfd`, `io_uring`, everything |
| AppArmor | profile = `unconfined` | the §12.1 backstop gone |
| SELinux | label = `spc_t` (super-privileged container) | type enforcement effectively off |
| `/sys` | mounted **read-write** | `/sys/fs/cgroup` writable, `/sys/kernel/security`, module parameters |

What it does **not** turn off: namespaces. A `--privileged` container still has its own
PID, mount, net, IPC and UTS namespaces. **That is exactly the point of §15**: it is fully
process-isolated and trivially escapable.

`[R]` HIGH — implications for us:

1. **Implement `--privileged` as the *absence* of each control, computed in one place**,
   so that a future control we add is automatically disabled by it and we cannot forget.
   The shape: build a `SecurityPolicy` struct with every field, and have `privileged()`
   be a constructor that returns the permissive value for every field. If a new field is
   added and `privileged()` does not compile, that is the design working.
2. **Never make `--privileged` a partial thing.** "Privileged but with seccomp" is a
   configuration nobody understands. If a user needs one capability, `--cap-add` exists.
3. `[R]` The honest documentation line: *"`--privileged` disables every containment
   mechanism except namespaces. A privileged container is a host process with a private
   view of the filesystem. Do not run untrusted images with it."*

---

## 14. Devices

### 14.1 v1 `devices` controller vs v2 eBPF — a real dependency question

`[F]` HIGH. cgroup v1 had a `devices` controller with `devices.allow` / `devices.deny` /
`devices.list` files: `echo 'c 1:3 rwm' > devices.allow`. **cgroup v2 has no devices
controller at all.** Device access control in v2 is implemented by attaching a
`BPF_PROG_TYPE_CGROUP_DEVICE` eBPF program to the cgroup with
`bpf(BPF_PROG_ATTACH, …, BPF_CGROUP_DEVICE)` (Linux **4.15**). The program receives the
device type/major/minor/access and returns 0 (deny) or 1 (allow).

`[O]` HOST-A, HIGH — this is directly observable and the result is instructive:

```
$ ssh host-a 'bpftool cgroup tree /sys/fs/cgroup/system.slice'
/sys/fs/cgroup/system.slice/docker-c087abc73246….scope
    2294     cgroup_device   multi
    2455     cgroup_device   multi           sd_devices
…
$ ssh host-a 'bpftool prog list | grep -A2 "^2294"'
2294: cgroup_device  tag ab4bc4523b7fe6b4
	loaded_at 2026-08-12T00:46:01+0000  uid 0
	xlated 552B  jited 361B  memlock 4096B
```

**Two** device programs per container, attached with `BPF_F_ALLOW_MULTI`: runc's anonymous
552-byte one and systemd's `sd_devices`. Both must return allow for access to succeed.
runc's own source comments on this ("Two programs with identical rules aren't the end of
the world, but it is a bit concerning") — it is a direct consequence of the systemd driver
setting `DeviceAllow=` properties *and* runc attaching its own program (§2.5). Note the
identical `tag ab4bc4523b7fe6b4` across every container: same rules, same program.

`[R]` HIGH — **this is a genuine dependency decision and here is my recommendation:
do not implement device control in the MVP; make device *access* impossible by
construction instead.** Reasoning:

- Attaching a BPF program means either linking `libbpf` (C dependency, `libbpf-rs`) or
  writing an eBPF assembler and calling `bpf(2)` directly (`aya` does the latter in pure
  Rust). Either is a substantial dependency for a project whose point is kernel
  mechanics, and it buys us *one* control.
- The control it buys is **redundant with three things we already do**: (a) the
  container's `/dev` is a fresh `tmpfs` containing only the nodes we create, (b)
  `CAP_MKNOD` is a no-op outside the initial user namespace and, in a root container,
  is neutralised by `MS_NODEV` on every other mount, and (c) the image's rootfs is an
  overlay we control, and `mknod` on it can be blocked.
- **The gap this leaves, stated honestly**: a *root* container with `CAP_MKNOD` on a
  filesystem *without* `MS_NODEV` can create `/dev/sda` and read the host disk. Our `/dev`
  tmpfs is mounted `rw,nosuid` — `[O]` HOST-A shows Docker does the same, deliberately,
  because `nodev` would break the device nodes we just created. So `/dev` **is** a place
  where `mknod` works. Closing that without the device cgroup requires either dropping
  `CAP_MKNOD` from the default set or the seccomp filter denying `mknod`/`mknodat`.
- `[R]` **So: drop `CAP_MKNOD` from our default capability set** (a deviation from Docker,
  see §7.6) **and deny `mknod`/`mknodat` in seccomp when it is absent.** That gets ~95% of
  the device cgroup's value for zero dependencies. Add the eBPF device controller later,
  when `--device` needs fine-grained rules.

`[C]` MED — worth knowing what Docker's actual default rules are, because they are worse
than people assume. runc v1.2.5 `libcontainer/specconv/spec_linux.go`, with the
maintainer's own comment preserved:

```go
// XXX (cyphar)
// This behaviour is at the very least "questionable" (if not outright wrong)
// according to the runtime-spec. ... several devices listed here are not part of
// the spec (including "mknod for any device"?!). In addition, these rules are
// appended to the user-provided set which means that users *cannot disable this
// behaviour*.
var AllowedDevices = []*devices.Device{
	{Rule: {Type: CharDevice,  Major: Wildcard, Minor: Wildcard, Permissions: "m", Allow: true}},
	{Rule: {Type: BlockDevice, Major: Wildcard, Minor: Wildcard, Permissions: "m", Allow: true}},
	{Path: "/dev/null",   Rule: {c, 1, 3, "rwm", allow}},
	{Path: "/dev/random", Rule: {c, 1, 8, "rwm", allow}},
	…
}
```

**Every runc container is allowed `mknod` of any character or block device, and the user
cannot turn it off.** The device cgroup then denies *access* (`rw`) to the resulting node
— so the node can be created but not opened. That is a strange design, it is documented as
such by its own maintainer, and it is another reason not to treat "Docker does X" as
"X is correct".

### 14.2 The minimum device set

`[C]` HIGH — OCI Runtime Spec "Default Devices" plus Docker's implicit set:

| path | type maj:min | why |
|---|---|---|
| `/dev/null` | c 1:3 | **required.** D1 measured that its absence silently creates a growing regular file |
| `/dev/zero` | c 1:5 | required in practice — `mmap`-based allocators, `dd` |
| `/dev/full` | c 1:7 | ENOSPC-on-write; some test suites |
| `/dev/random` | c 1:8 | entropy |
| `/dev/urandom` | c 1:9 | **required.** Every TLS library, every language runtime seeding a hash map |
| `/dev/tty` | c 5:0 | the process's controlling terminal |
| `/dev/console` | c 5:1 | bind of the pty **slave** when `-it`; not a fresh node |
| `/dev/ptmx` | c 5:2 | **symlink to `pts/ptmx`**, not a node (see below) |
| `/dev/pts/*` | c 136:* | created by the `devpts` mount |
| `/dev/shm` | — | tmpfs, not a device |
| `/dev/fd`, `/dev/stdin|out|err` | — | symlinks into `/proc/self/fd` |

### 14.3 `mknod` vs bind-mount, and `MS_NODEV`

`[F]` HIGH — three ways to put a device node in the container:

1. **`mknod`** in the container's `/dev` tmpfs. Requires `CAP_MKNOD` **in the initial user
   namespace** — D1 measured `EPERM` on ROCKY with a full 41-capability effective set
   inside a userns. So this path is **unavailable to rootless, always**.
2. **Bind-mount the host's node** onto a placeholder file in the container's `/dev`. Needs
   no `CAP_MKNOD`, works in a userns, and is what rootless runtimes do. Costs one extra
   mount per device.
3. **Bind-mount the host's whole `/dev`** — what `--privileged` does. Never otherwise.

`[R]` HIGH — **use bind-mounts (2) for every device, even as root.** It costs 8 extra
mounts and it makes the rootful and rootless code paths *identical*, which is precisely
the kind of cheap seam §17 argues for. runc does exactly this in rootless mode and
`mknod` in rootful mode — two code paths where one would do.

`[F]` HIGH — `/dev/ptmx` must be a **symlink to `pts/ptmx`**, not a `mknod`'d 5:2 node.
A real 5:2 node opens the *host's* ptmx (the legacy single-instance devpts), not the
container's. D1 §6.3 covers the devpts side; the privilege-relevant statement is:
**a `mknod`'d `/dev/ptmx` is a container-to-host tty leak.** Docker uses the symlink;
so must we.

`[F]` HIGH — `MS_NODEV` on a mount makes every device node on it inert (`open` returns
`EACCES`). Our mount flags must be:

- `/dev` — `nosuid` only. **Not `nodev`** (that would disable the devices we just made),
  **not `noexec`** (Docker doesn't; some images put helper scripts there).
- everything else the container mounts — `nosuid,nodev,noexec` where possible.
- **the container rootfs itself** — this is the one people forget. If the rootfs overlay
  is mounted without `nodev`, a device node baked *into the image tar* is live. `[O]`
  HOST-A shows Docker's rootfs overlay is mounted with **neither `nodev` nor `nosuid`**:
  `1000 597 0:60 / / rw,relatime - overlay overlay rw,lowerdir=…,nouserxattr`. Docker
  relies on the device cgroup for this instead. `[R]` **We should mount the rootfs
  `nodev`** — one flag, closes the "malicious image ships `/dev/sda`" hole without any
  eBPF, and reinforces §14.1's recommendation. The compatibility cost is images that ship
  their own device nodes and expect them to work, which is vanishingly rare.
- `nosuid` on the rootfs is a harder call: it breaks legitimate setuid binaries inside the
  image (`sudo`, `ping`). `[R]` Leave the rootfs `suid`-capable and rely on
  `no_new_privs` (§7.4) instead, which achieves the same effect without breaking the file
  metadata.

---

## 15. Process isolation vs security isolation

### 15.1 The distinction, operationally

- **Process isolation** = the container cannot *see* or *name* host resources. Delivered
  by namespaces. Answers "does `ps` show host processes", "does `ls /` show the host
  root", "does `ip addr` show host interfaces".
- **Security isolation** = the container cannot *affect* host resources even when it
  finds a way to name them. Delivered by capabilities, seccomp, LSM, cgroups, masked
  paths, user namespaces, and mount flags.

Namespaces are about **visibility**; the rest are about **authority**. A container can
have perfect visibility isolation and zero authority isolation.

### 15.2 The concrete example

`[R]` HIGH — a container that is fully process-isolated and trivially escapable, in one
command:

```
mydocker run --privileged alpine sh
# inside:
echo '|/bin/sh -c "cat /etc/shadow > /host_tmp/x"' > /proc/sys/kernel/core_pattern
# then crash any process -> the kernel runs that pipe as root ON THE HOST
```

Every namespace is fresh: its own PID 1, its own `/`, its own network stack, its own
hostname. `ps` shows one process. `ip addr` shows `lo` and `eth0@…`. It looks completely
isolated. And `/proc/sys` is writable because `--privileged` cleared `readonlyPaths`, so
`core_pattern` — a **host-global** setting the kernel acts on with full root privilege in
the initial namespaces — is one `echo` away.

A second, even simpler one that needs no `--privileged`:

```
mydocker run -v /var/run/docker.sock:/var/run/docker.sock alpine sh
# inside: talk to the socket, ask the daemon to start a --privileged container
#         with the host / bind-mounted. Done.
```

Namespaces perfect; escape total; no kernel bug involved. **This is the point:
namespaces are not a security boundary on their own, and every escape in §11 is a story
about authority, not visibility.**

### 15.3 The dangerous shortcuts list

`[R]` HIGH. What educational and first-pass container implementations ship, and the
concrete consequence of each. This is the checklist form of everything above.

| shortcut | concrete consequence |
|---|---|
| **`chroot` instead of `pivot_root`** | `chroot` does not change the process's *root* for a process that still holds a directory fd outside it. The classic escape is 6 lines: `mkdir foo; chroot(foo); chdir("../../../../..")` — because `chroot` does not `chdir`, and `..` at the old root is not a no-op. D1 §5.5 demonstrated it. The old root is also still reachable via `/proc/<pid>/root` of any process outside |
| **no `MS_REC\|MS_SLAVE` on `/`** | every mount the container makes propagates to the **host** and stays there after the container dies. D1 measured this. It is a permanent host-state leak and eventually an out-of-mounts failure |
| **`put_old` left mounted after `pivot_root`** | the host root is a directory away. `umount2(put_old, MNT_DETACH)` is mandatory, not tidy-up |
| **all capabilities retained** | `CAP_SYS_ADMIN` ⇒ `mount` ⇒ mount the host disk. `CAP_SYS_MODULE` ⇒ load a module. `CAP_SYS_RAWIO` ⇒ `/dev/mem`. Any one is a full escape (§7.6) |
| **no seccomp** | `keyctl` (shared keyring), `userfaultfd` (kernel-exploit primitive), `io_uring` (large young surface), `kexec_load`, `perf_event_open`. Also: every future kernel syscall is automatically exposed |
| **no `no_new_privs`** | any setuid binary in the image is an escalation path from a `--user` restriction back to container root |
| **writable `/proc/sys`** | `core_pattern` ⇒ host root (§15.2). `modprobe` ⇒ host root. `panic_on_oops`, `kexec_load_disabled` ⇒ host DoS |
| **unmasked `/proc/kcore`** | read all host physical memory: other containers' secrets, host keys, kernel memory |
| **writable `/proc/sysrq-trigger`** | `echo b` reboots the host instantly |
| **writable `/sys/fs/cgroup`** | on v1, `release_agent` ⇒ host root (CVE-2022-0492). On v2, the container raises its own `memory.max` and `pids.max`, defeating every limit |
| **sharing host PID namespace** | `/proc/<host pid>/` visible; with `CAP_SYS_PTRACE`, inject code into any host process. Also `kill -9 1` |
| **sharing host network namespace** | bind any port including ones already bound by host services; sniff all host traffic with `CAP_NET_RAW`; reach `127.0.0.1`-only host services (databases, metadata endpoints, `/var/run` HTTP APIs) |
| **sharing host IPC namespace** | read other processes' SysV shared memory (Postgres, Oracle); exhaust the host's global SysV limits |
| **root-in-container == root-on-host** | this is Docker's *default* (§9.2). Every file the container writes to a bind-mounted host path is owned by host root. A single mount-flag or masking mistake becomes a full compromise instead of a nuisance |
| **no cgroup limits** | a fork bomb exhausts the host's global PID space; the host cannot fork `sshd` or our own runtime to recover. Memory exhaustion invokes the *global* OOM killer, which will pick a victim on the host (§3.4) |
| **fds leaked across `execve`** | CVE-2024-21626 verbatim: `WORKDIR /proc/self/fd/7` starts the container with its cwd in the host filesystem (§11.4) |
| **mounting the host `/`** | no comment needed |
| **mounting the docker/our socket** | the container asks our daemon to start a privileged container. Complete escape with zero exploitation (§15.2) |
| **`/dev` bind-mounted from the host** | `/dev/sda`, `/dev/mem`, `/dev/kmsg` |
| **rootfs mounted without `nodev`** | a device node baked into the image tar is live (§14.3) |
| **error paths that leave state** | a failed `create` that leaves the cgroup, the netns, the mounts or the veth behind. Not an escape, but the thing that makes a machine unusable after 200 failed runs. D2 owns the atomicity design; the cgroup and its `rmdir` are ours |

`[R]` HIGH — the meta-point for the implementation ladder: **each stage of the ladder
should state which of these it has *not* yet fixed.** A stage-3 container that is
namespace-isolated with no seccomp and full capabilities is a legitimate teaching
milestone — as long as the document says, in that stage, "this container is escapable in
the following three ways". The dangerous version is the same code with no such statement.

---

## 16. The rootless privilege model: what works, what degrades, how loudly

### 16.1 The failure-mode table

`[R]` HIGH — the column that matters most is the last one. A **quiet** degradation is a
bug in the tool's honesty, not just a missing feature.

| capability | rootless status | fails how |
|---|---|---|
| namespaces (mnt/pid/uts/ipc/net) | **works** — a userns grants `CAP_SYS_ADMIN` inside it, which is what `unshare` needs | — |
| cgroup limits **with** systemd user session (`user@.service` + `Delegate=yes`) | **works** — `/sys/fs/cgroup/user.slice/user-N.slice/user@N.service/…` is delegated | — |
| cgroup limits **without** a systemd user session (ssh into a non-systemd-login shell, Slurm, a container) | **impossible** | `mkdir` → `EACCES`. §5.4 policy applies |
| cgroup limits on **cgroup v1** | **impossible** | no delegation model at all (§5.3). Podman documents it as unsupported |
| `--pids-limit` / `--memory` in the above | depends entirely on the above | must be **loud** — see §5.4 |
| `chown` in image extraction to a uid outside the map | **fails** | `EINVAL` (§9.3). Common tars silently lose ownership with `--no-same-owner` — *quiet* |
| multi-UID images with no `/etc/subuid` entry | **fails** | at extraction or at `setuid` — *quiet at extraction, loud at runtime* |
| `mknod` | **always fails** | `EPERM` even with a full effective cap set inside a userns (D1 measured on ROCKY). Must bind-mount device nodes (§14.3) |
| overlayfs, kernel ≥ 5.11 | **works** with `userxattr` | — |
| overlayfs, kernel < 5.11 (except RHEL backports) | falls back to `fuse-overlayfs` | slower, different semantics, extra dependency — *loud, if we check* |
| overlay upperdir on **NFS** | **fails** | overlayfs refuses NFS as upper. ROCKY's home is NFS, so scratch must be local `/tmp` — *loud* |
| veth pair / bridge | **impossible** — needs `CAP_NET_ADMIN` in the **host** netns | D4 owns this. The workaround is a userspace TCP/IP stack (`slirp4netns`, `pasta`), with real throughput and ICMP consequences |
| binding ports < 1024 | **fails** unless `net.ipv4.ip_unprivileged_port_start` is lowered or the process has ambient `CAP_NET_BIND_SERVICE` on the **host** | `EACCES` — *loud* |
| `--privileged` | meaningless | all capabilities *inside the userns*, none outside |
| loading a seccomp filter | **works** — `no_new_privs` path needs no capability | — |
| capabilities | **work inside the userns**; useless outside | quiet, and the most misunderstood: `CapEff` shows 41 caps and `mknod` still `EPERM` |
| AppArmor profile load | **fails** — needs `CAP_MAC_ADMIN` in the init userns | — |
| Ubuntu 23.10+ `apparmor_restrict_unprivileged_userns=1` | **capabilities stripped inside the new userns** | looks like a kernel bug; is an LSM policy (§9.2) |

`[R]` HIGH — the design rule extracted from the "how" column: **every rootless
degradation must be detected at container-create time and reported once, by name, before
anything is created.** The alternative — discovering it at `chown` on file 4,000 of a
layer, or at `bind()` in the container's entrypoint — produces error messages that are
about the wrong subsystem.

### 16.2 The rootless cgroup path is *found*, not *chosen*

`[F]` HIGH, from §2.2's containment rules: an unprivileged delegatee cannot place the
first process into a delegated subtree. So in rootless mode our cgroup root is determined
by where our own process already is:

```
1. read /proc/self/cgroup  ->  "0::/user.slice/user-1000.slice/user@1000.service/app.slice/…"
2. our writable root is that path (or an ancestor we own)
3. mkdir a child under it; enable controllers in OUR cgroup's subtree_control
4. before we can enable anything, verify it is in OUR cgroup.controllers —
   which requires the PARENT (systemd's user@.service) to have enabled it
```

`[F]` MED — step 4 is the common real-world failure: on many distros the user slice has
only `memory` and `pids` delegated by default, not `cpu`, because enabling `cpu` for
user sessions has a measured overhead. So a rootless container can get `--memory` and
`--pids-limit` but **not** `--cpus`, on an otherwise perfectly configured host. The fix is
a systemd drop-in (`[Service] Delegate=cpu cpuset io memory pids` on `user@.service`) —
which is a *documentation* deliverable, not a code one.

`[R]` HIGH — this is the same "where am I, and what may I write" probe §2.6 already
recommends for the rootful case. **Write it once, use it in both modes.** That is
concretely the cheapest rootless-readiness step available.

---

## 17. Which rootless retrofits are structurally expensive — and the one seam that matters

The project has fixed **root-first, rootless later**. Not relitigating it; identifying
what that costs and where.

### 17.1 Cheap later (leaf changes)

- **Device nodes**: bind-mount instead of `mknod` (§14.3). Cheap *if we bind-mount from
  the start*, which costs nothing as root. **Do it now.**
- **Idmapped mounts** (§9.5): entirely inside mount construction.
- **`userxattr` on overlay**: one mount option; D3's territory.
- **Seccomp**: identical in both modes (needs only `no_new_privs`).
- **Capability sets**: identical API; the *semantics* differ but the code does not.
- **The cgroup-root probe** (§16.2): ten lines, needed in both modes anyway.

### 17.2 Structurally expensive later

Three, and they are expensive for the same underlying reason.

**1. "Am I root?" scattered through the code.** The cheap-to-write version of every
privileged operation is a direct syscall that happens to work because we are root. The
expensive-to-retrofit version is that same call appearing in forty places, each needing a
different fallback: `mknod`→bind-mount, `chown`→idmap or skip, `mount`→different flags,
`setgroups`→`deny`, `ip link add`→`slirp4netns`. Retrofitting means auditing every
syscall site.

**2. The network model is not a fallback, it is a different implementation.** Rootful
networking is veth+bridge+netfilter (D4). Rootless networking is a **userspace TCP/IP
stack** (`slirp4netns`/`pasta`) with its own process, its own lifecycle, its own
performance profile, and no ICMP. There is no `if rootless {}` branch that turns one into
the other. This is the single most expensive item and it is D4's, not ours — we own only
the fact that `CAP_NET_ADMIN` is the gate.

**3. The UID model.** Root-first means "the uid in the tar is the uid on disk". Rootless
means every uid is shifted by a per-user offset, which touches image extraction, layer
sharing/caching (two users cannot share a `chown`-ed layer), volume ownership, `--user`
handling, and every error message that prints a uid. If uid handling is spread across the
storage code as bare `u32`s, retrofitting is a type-level change through the whole
storage layer.

### 17.3 The single seam

`[R]` HIGH — **one type, constructed once, threaded through everything, and no code
outside it reads `geteuid()`.**

```rust
/// Constructed exactly once at startup. The only place that asks "am I root?".
pub struct Privilege {
    /// None = we are in the initial user namespace (rootful).
    /// Some = we run inside a userns with this mapping.
    idmap: Option<IdMap>,
    /// Where we may create cgroups, and which controllers are usable there.
    cgroup: CgroupSupport,          // Full / Partial / None  (§5.4)
    /// Can we create veth into the host netns?
    net: NetCapability,             // HostVeth / UserspaceStack / None
    /// Can we mknod, or must we bind-mount device nodes?
    devices: DeviceStrategy,        // Mknod / BindMount
    /// Native overlay, overlay+userxattr, or fuse-overlayfs?
    overlay: OverlayStrategy,
}
```

Why this specific shape:

- It is **not** `is_root: bool`. A boolean is the trap: the interesting cases are
  root-but-no-writable-cgroup (a container-in-container), rootless-with-full-delegation
  (a good systemd desktop), and rootless-without-subuid (ROCKY). Each of those has a
  different answer per field. A boolean forces every call site to re-derive the real
  question from the wrong one.
- Each field is a **strategy the caller obeys**, not a fact the caller interprets.
  `devices: BindMount` means the device code bind-mounts; it never asks why.
- It is populated by **probing**, per §6.2 — try the syscall, record the answer — not by
  reading a version number or a uid.

`[R]` HIGH — **what the code should assume about privilege: nothing, except through
this struct.** The concrete rule to enforce in review: *`geteuid()`, `getuid()`, and any
`if root` appear exactly once in the codebase, inside `Privilege::probe()`.* If they
appear anywhere else, the rootless retrofit has already become expensive.

`[R]` MED — and the rootful-only shortcut that is *fine* to take: implement only the
rootful arm of each strategy at first, with the other arm as
`unimplemented!("rootless: see D5 §16")`. The seam exists, the branch exists, the work is
deferred, and the compiler will list every site the day we do it. That is the whole
difference between "deferred" and "expensive".

---

## 18. Rust crates for this domain

D2 §12 already evaluated the substrate (`rustix` vs `nix` vs `libc`, `oci-spec`, the youki
crates) and **explicitly handed D5 two decisions**: the seccomp substrate (D2 §12.11) and
the cgroups crate (D2 §12.8). Making them here, plus the ones D2 did not cover.

### 18.1 cgroups: **no crate. `std::fs` and `rustix`.** `[R]` HIGH

`[F]` HIGH — `cgroups-rs` 0.5.1 (2026-07-14, 1.02M dl/90d, now maintained under
`kata-containers/cgroups-rs`) and youki's `libcgroups` 0.7.0 are both healthy. Neither
should be adopted.

The reasoning is arithmetic. Our entire cgroup v2 surface is:

```
mkdir(path)                                          std::fs::create_dir
write "+cpu +memory +pids" -> cgroup.subtree_control  std::fs::write
write "<pid>"              -> cgroup.procs            std::fs::write
write "104857600"          -> memory.max              std::fs::write
write "150000 100000"      -> cpu.max                 std::fs::write
write "1"                  -> cgroup.kill             std::fs::write
read  memory.events, cpu.stat, memory.current         std::fs::read_to_string + parse
rmdir(path)                                          std::fs::remove_dir
open(path, O_DIRECTORY)  for CLONE_INTO_CGROUP        rustix::fs::open
poll(cgroup.events)                                   rustix::event::poll
```

That is **cgroupfs is just files**, and a crate wrapping it is pure overhead: it adds a
dependency, hides the exact filenames (the thing we are here to learn), and — decisively —
**the two crates' main value is the parts we do not want**: cgroup v1 support (§5.5: no)
and the systemd D-Bus driver (§2.6: later, and then as our own code atop our own fs
driver). `cgroups-rs`'s v1 code is the majority of the crate.

`[R]` The one thing worth copying rather than writing: the **parsers**. `memory.stat` has
~40 fields, `io.stat` is per-device key=value, `cpu.stat` grew fields across kernel
versions. Read `libcgroups`' `v2/stats.rs` before writing ours; do not depend on it.

`[R]` HIGH — **`std::fs::write` semantics are exactly right for cgroupfs and this is not
obvious.** cgroup interface files require the value to be delivered in **one `write(2)`**
— they are not stream files, and a buffered writer that splits `"+cpu +memory"` across two
writes produces two separate (and possibly failing) operations. `std::fs::write` does a
single `write` for a small buffer; `writeln!` on a `BufWriter` does not guarantee it. Use
`std::fs::write`, never `File::create` + `write!`. Also: **do not append a trailing
newline blindly** — most files accept it, `cgroup.procs` accepts it, but the habit hides
which files actually care.

### 18.2 seccomp: **`seccompiler`, or hand-built `sock_filter`.** `[R]` MED→HIGH

D2 §12.11 recommended `seccompiler` at MED and handed us the call. Confirming it, with
the additional facts I checked:

`[F]` HIGH — `seccompiler` 0.5.0, rust-vmm (Firecracker's), 5.75M dl/90d, dependencies
`libc` + optional `serde`/`serde_json`. **Pure Rust, no C.** `SeccompAction` is
`{Allow, Errno(u32), KillThread, KillProcess, Log, Trace(u32), Trap}`.

`[F]` HIGH — **`seccompiler` has no `Notify` action**, i.e. no `SECCOMP_RET_USER_NOTIF`
and no `SECCOMP_FILTER_FLAG_NEW_LISTENER`. §8.7 says we do not need user-notify in the
MVP, so this is not disqualifying — but it *is* the thing that would force a switch later
if we ever do rootless `mknod`-via-supervisor. Note it now.

`[F]` HIGH — `libseccomp` 0.4.0 is C FFI to `libseccomp.so`. `[O]` HOST-A (via D2 §12.11):
runc is the **only dynamically-linked binary in the docker→containerd→shim→runc chain**,
and `ldd /usr/bin/runc` shows `libseccomp.so.2`. Adopting `libseccomp` recreates exactly
that wart, against D2 §7.3's static-single-binary recommendation.

`[R]` The tradeoff, stated: `libseccomp` gives multi-architecture syscall-name→number
translation and the `scmp_arch` machinery for free, and the OCI `linuxSeccomp` schema maps
onto its API nearly 1:1. With `seccompiler` (which targets x86_64 and aarch64) **we own
the name→number table**, which for x86_64-first is a generated table and for multi-arch is
real work. Given the project is x86_64-first and the *learning* is in the ABI handling
(§8.3), owning it is the point, not the cost.

`[R]` HIGH — **and consider going one step further: build the `sock_filter` array
directly.** The filter for an allowlist is genuinely simple — validate `arch`, load `nr`,
a binary search or a jump table over allowed numbers, return `ALLOW`/`ERRNO`. Writing it
teaches the mechanism `seccompiler` hides, is ~200 lines, and removes the last dependency
in this domain. `[R]` My concrete suggestion: **write it by hand for the MVP allowlist;
keep `seccompiler` as the escape hatch if argument-comparison rules (§8.4's `socket`/`clone`
masked-equality) get fiddly.** Those two rules are the only ones in Docker's profile that
need real expression compilation.

### 18.3 capabilities: **`rustix`. Not `caps`.** `[R]` HIGH — and D2's open question is settled

D2 §12.3 recommends `rustix` over the `caps` crate but flagged LOW confidence on whether
`rustix::thread::CapabilitySet` carries every constant we need. **Settled:**

`[O]` HIGH — `docs.rs/rustix/1.1.4/rustix/thread/struct.CapabilitySet.html`, enumerated:
all **41** capabilities `CAP_CHOWN` (0) … `CAP_CHECKPOINT_RESTORE` (40) are present as
associated constants, explicitly including `CAP_BPF`, `CAP_PERFMON` and
`CAP_CHECKPOINT_RESTORE` — the three D2 was unsure about. The non-deprecated bitflags API
is complete against `CAP_LAST_CAP` on 6.11.

So the full §7.7 sequence is available from `rustix::thread` with no additional crate:
`set_keep_capabilities`, `capabilities`/`set_capabilities` (`capget`/`capset`),
`remove_capability_from_bounding_set`, `configure_capability_in_ambient_set`,
`set_no_new_privs`, `set_capabilities_secure_bits`. `[R]` Adopt. The only thing `caps`
still offers is its **name↔bit string table** for `--cap-add CAP_NET_RAW` parsing; copy or
generate that table, do not take the dependency.

### 18.4 BPF (for v2 device control): **not needed — and that is the recommendation** `[R]` HIGH

§14.1 argues against implementing the eBPF device controller at all. If that changes:

`[F]` HIGH — `libbpf-rs` 0.27.1 (2026-09-03, 1.47M dl/90d) wraps `libbpf-sys`, i.e. **C**;
it brings a C build dependency and breaks the static-single-binary property. `aya` 0.14.0
(2026-06-24, 1.39M dl/90d) is **pure Rust**, issues `bpf(2)` directly, and needs no
libbpf, no BTF and no C toolchain.

`[R]` If we ever need it: **`aya`**, for the same reason `seccompiler` beats `libseccomp`.
But note the program we need is ~15 eBPF instructions per rule — runc's is 552 bytes
xlated (`[O]` HOST-A §14.1) — and `bpf(BPF_PROG_LOAD)` + `bpf(BPF_PROG_ATTACH)` is two
syscalls. Hand-writing it is entirely feasible and is the same argument as §18.2. **The
real recommendation stands: don't need it (§14.1).**

### 18.5 AppArmor / SELinux: **no crate exists that is worth using** `[R]` MED

`[F]` MED — there is no maintained, widely-used Rust crate for either. The operations we
need are trivially small anyway:

- **AppArmor**: write the profile name to `/proc/self/attr/apparmor/exec` (or
  `/proc/self/attr/exec` on older kernels) in the form `"exec <profile>"`, before
  `execve`. Loading the profile is `apparmor_parser -Kr <file>` — a subprocess, which is
  acceptable because it runs in the *host* context at container-create time, not in the
  child (and so does not violate §11.3).
- **SELinux**: write the context string to `/proc/self/attr/exec`. Same file, different
  string (§12.2).

`[R]` Both are `std::fs::write` on a `/proc` file. There is no abstraction to buy. The
hard part is *generating* the policy, and no crate does that either.

### 18.6 Summary

| need | verdict | why |
|---|---|---|
| cgroup v2 | **`std::fs` + `rustix`** | cgroupfs is files; crates' value is v1 + systemd, which we don't want |
| cgroup stats parsing | write it; **read** `libcgroups/v2/stats.rs` first | the field lists are the only hard part |
| systemd cgroup driver (later) | `zbus` | only if/when §2.6's later half happens |
| seccomp | **hand-built `sock_filter`**, `seccompiler` as fallback | pure Rust, static binary, and the ABI handling is the lesson |
| capabilities | **`rustix::thread`** | complete 41-constant `CapabilitySet`; deletes the `caps` dep |
| `no_new_privs`, `KEEPCAPS`, securebits | **`rustix::thread`** | already there |
| eBPF device cgroup | **don't** (§14.1); `aya` if forced | pure Rust, no C toolchain |
| AppArmor / SELinux | **`std::fs::write`** | one `/proc` file write each |
| capability name parsing | copy a table | not a dependency |

---

## 19. Classification table

Same tiering as D1 §9. **Required** = no container without it. **Strongly recommended** =
container "works" but is escapable or unbounded. **Hardening** = raises the cost of an
escape. **Docker convenience** = compatibility/UX, no isolation value.

### 19.1 Resource control

| Mechanism | Tier | Why |
|---|---|---|
| a writable cgroup at all | **required** for any limit | Without it every `--memory`/`--cpus` is a lie (§5.4) |
| `pids.max` | **required** | Only limit whose absence takes down the **host** (§3.4) |
| `memory.max` | **required** | The limit users mean by "container" |
| `cgroup.procs` placement (or `CLONE_INTO_CGROUP`) | **required** | A limit on an empty cgroup constrains nothing |
| `cgroup.kill` | **required** *(with the freeze+pidfd fallback)* | Race-free stop; kill-PID-1 is not equivalent (§4.1) |
| no-internal-process-rule-compliant layout | **required** | `EBUSY` on `cgroup.procs` otherwise (§1.3) |
| `cgroup.type == domain` assertion | strongly recommended | `cgroup.kill` and `memory.*` silently absent in threaded (§1.4) |
| `cpu.max` | strongly recommended | `--cpus`; the flag users actually pass |
| `memory.events` read at exit | strongly recommended | Only correct OOM detection; exit 137 is ambiguous (§3.2) |
| `cgroup.events` poll (`populated`) | strongly recommended | "Everything died", complements the pidfd |
| `cgroup.freeze` | strongly recommended | `pause`, and the kill fallback |
| recursive `rmdir` teardown | strongly recommended | `remove_dir_all` fails confusingly (§4.6) |
| `memory.high` | hardening / UX | Backpressure instead of hard kill (§3.1) |
| `memory.oom.group` | hardening | "container OOM'd" instead of "a process died" |
| `cpuset.cpus` | Docker convenience | Real use case, rarely needed |
| `cpu.weight` | Docker convenience | Only bites under contention; users misread it |
| `io.max` | Docker convenience | Works, but needs the right major:minor (§3.6) |
| `io.weight` | **anti-feature** | Silent no-op without bfq/io.cost — measured on HOST-A |
| PSI (`*.pressure`) | Docker convenience | Great stats, zero isolation value |
| cgroup **v1** driver | **skip** | §5.5 — a second, worse architecture |

### 19.2 Security boundary

| Mechanism | Tier | Why |
|---|---|---|
| bounding-set drop | **required** | The only durable capability drop (§7.2) |
| `pivot_root` + detach `put_old` | **required** (D1) | `chroot` is escapable |
| fresh `/proc` + masked paths | **required** | `core_pattern` = host root; `kcore` = all RAM (§10) |
| `/proc/sys` read-only | **required** | Same |
| `/sys/fs/cgroup` read-only + cgroupns | **required** | Otherwise the container edits its own limits (§10.5) |
| `O_CLOEXEC` everywhere + `close_range` before `execve` | **required** | CVE-2024-21626 is a `WORKDIR` line (§11.4) |
| `openat2(RESOLVE_BENEATH\|NO_MAGICLINKS)` for container paths | **required** | CVE-2021-30465 class (§11.5) |
| never `execve` our own host binary in container ns | **required** | CVE-2019-5736 (§11.1) |
| seccomp allowlist, `defaultAction=ERRNO` | **required** | `keyctl`/`userfaultfd`/`io_uring`/`kexec` (§8.5) |
| `arch` check as filter prologue | **required** | x32/i386 bypass (§8.3) |
| `execve` allowed in the filter | **required** | Otherwise the container never starts |
| `TSYNC` flag | strongly recommended | Free; a filter on one thread is not a control |
| `PR_SET_NO_NEW_PRIVS` | strongly recommended *(we default ON — Docker does not)* | Neutralises setuid + file caps (§7.4) |
| device nodes by **bind-mount**, not `mknod` | strongly recommended | Identical rootful/rootless path (§14.3) |
| rootfs mounted `nodev` | strongly recommended | Image-embedded device nodes (§14.3) |
| `/dev/ptmx` as a **symlink** | **required** for `-it` | A `mknod`'d 5:2 is a host tty leak |
| drop `CAP_MKNOD` from defaults | hardening *(deviation)* | Buys ~95% of the device cgroup for free (§14.1) |
| drop `CAP_NET_RAW` from defaults | hardening *(deviation)* | ARP/DNS spoofing on the container net |
| ambient capabilities | Docker convenience | Only for non-root + `--cap-add` (§7.5) |
| eBPF device cgroup | Docker convenience *(defer)* | Redundant with the four items above (§14.1) |
| AppArmor profile | hardening | Path-based `/proc` denial that seccomp **cannot** express (§12.1) |
| SELinux | **skip** | No test host; large subsystem (§12.2) |
| user namespace | hardening (the big one) | The only thing making container-root ≠ host-root — **not** Docker's default |
| `hidepid` | **skip** | Solves a problem the PID namespace already solved (§10.4) |

---

## 20. The consolidated security sequence

D1 §10 gives the corrected creation sequence. This is the D5-owned portion, expanded, with
the parent/child split marked. `P` = the runtime process (host privilege), `C` = the
container child.

```
 P1. Privilege::probe()                       once at startup (§17.3)
 P2. detect cgroup layout (v1/v2/hybrid)      §5.2 ; hard-fail per §5.4 if limits requested
 P3. mkdir  <root>/<id>                       §1.2 ordering: parent subtree_control first
 P4. write  limits: memory.max, memory.high, cpu.max, pids.max, cpuset.*
     -- verify by reading back; report declared-vs-effective (§2.4)
 P5. open   <root>/<id>  O_RDONLY|O_DIRECTORY  -> cgroup_fd for CLONE_INTO_CGROUP
 P6. build  the seccomp program                 (allocation happens HERE, not in C)
 P7. build  argv/envp CStrings, capability bitsets, all paths      (D1 §11.2)
 P8. clone3(CLONE_INTO_CGROUP|CLONE_PIDFD|namespaces...)
 P9. [if userns] write setgroups=deny, uid_map, gid_map    §9.1 ; then release C
     ------------------------------------------------------------------
 C1. mount propagation: MS_REC|MS_SLAVE on /                 (D1)
 C2. rootfs bind, pivot_root, umount2(put_old, MNT_DETACH)   (D1)
 C3. mount /proc (fresh), /dev tmpfs, /dev/pts, /dev/shm, /dev/mqueue, /sys ro
 C4. bind-mount device nodes into /dev ; /dev/ptmx symlink   §14.3
 C5. bind-mount the container's own cgroup at /sys/fs/cgroup, READ-ONLY   §10.5
 C6. maskedPaths: tmpfs over dirs, bind /dev/null over files (skip ENOENT only)  §10.3
 C7. readonlyPaths: bind self + remount ro (recursive)       §10.3
 C8. resolve process.cwd with RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS ; chdir ; verify st_dev  §11.4
 C9. prctl(PR_SET_KEEPCAPS, 1)     [only if uid change + caps wanted]
C10. setgroups() ; setgid() ; setuid()                        §7.7
C11. PR_CAPBSET_DROP for every unwanted cap  (needs CAP_SETPCAP in E — do this FIRST)
C12. capset() P/E/I
C13. PR_CAP_AMBIENT_RAISE for ambient caps
C14. [LSM] write "exec <profile>" > /proc/self/attr/exec      §12
C15. prctl(PR_SET_NO_NEW_PRIVS, 1)                            §7.4
C16. close_range(3, ~0u, 0)                                   §11.4
C17. seccomp(SECCOMP_SET_MODE_FILTER, TSYNC, &prog)           §8.6
C18. execve()
```

The five orderings that are **kernel-forced**, not preference:

1. `PR_CAPBSET_DROP` before `capset` drops `CAP_SETPCAP` (C11 before C12).
2. `setgroups` before `setgid` (C10 internal order).
3. `PR_SET_KEEPCAPS` before the uid change (C9 before C10).
4. Ambient raise requires the cap already in P **and** I (C13 after C12).
5. seccomp last: `no_new_privs` (C15) must precede it, or hold `CAP_SYS_ADMIN` — and
   everything after it runs under the filter (C17 immediately before C18).

Two more that are ours, not the kernel's, and matter as much:

6. **`close_range` after every mount and before `execve`** (C16). Earlier and we close fds
   we still need; later is impossible.
7. **The cgroup is written before the process exists** (P4 before P8). Writing limits after
   placing the process is a window, and `CLONE_INTO_CGROUP` makes even the placement
   window vanish.

---

## 21. Experiments for the dev VM

Everything here needs a cgroup write, a container start, or a `sysctl -w`, so none of it
could run on HOST-A. Each is small and each settles a specific claim in this document.

1. **`memory.max` OOM behaviour.** `mkdir /sys/fs/cgroup/t`, `echo "+memory" >
   /sys/fs/cgroup/cgroup.subtree_control`, `echo 104857600 > t/memory.max`, put a
   `stress-ng --vm 1 --vm-bytes 200M` in it. Confirm: `memory.events` `oom` and `oom_kill`
   increment; the victim's wait status is `SIGKILL`; `dmesg` shows the cgroup OOM line.
   Then repeat with `memory.high=100M, memory.max=max` and confirm the process **survives**
   and `memory.events high` climbs instead. *Settles §3.1 and §3.2.*
2. **`memory.oom.group=1`** with two processes in the cgroup: confirm both die, and
   `oom_group_kill` increments. *Settles §3.1's last claim.*
3. **No-internal-process rule errnos.** Put a PID in `t/cgroup.procs`, then try
   `echo "+cpu" > t/cgroup.subtree_control` → expect `EBUSY`. Reverse order likewise.
   *Settles §1.3's errno claim (currently inferred).*
4. **`CLONE_INTO_CGROUP` end to end.** D1 §13 Q1 left this open. `clone3` with a valid
   cgroup fd; read `cgroup.procs` in the child's first instruction. Then repeat with a
   target that violates the no-internal-process rule and record the errno.
5. **`cgroup.kill` vs a fork bomb.** Run `:(){ :|:& };:` inside a cgroup with
   `pids.max=100`, then `echo 1 > cgroup.kill`. Time it; confirm `cgroup.procs` is empty
   and no survivors. Compare against the read-loop fallback with and without a preceding
   freeze. *Settles §4.2/§4.3.*
6. **`cgroup.kill` in a threaded cgroup** → confirm the errno (`ENOTSUP`?). *Settles §1.4.*
7. **`cpu.max` throttling counters.** `cpu.max = "10000 100000"` (10%), spin a busy loop,
   confirm `nr_periods`/`nr_throttled`/`throttled_usec` become non-zero. *Settles §3.3.*
8. **`io.weight` is a no-op.** With `mq-deadline` and no `io.cost` model, set two cgroups
   to weight 10 and 1000, run concurrent `fio`, measure. Then switch to `bfq` and repeat.
   *Settles §3.6's `[R]` "do not implement `--blkio-weight`".*
9. **`docker stop` on a handler-less PID 1** — start `docker run alpine sleep 1000`, time
   `docker stop`. Expect ~10 s. Then with a shell that traps SIGTERM, expect immediate.
   *Settles §4.4's user-visible claim.*
10. **seccomp `ENOSYS` vs `EPERM` for `clone3`.** Two containers, one denying `clone3` with
    EPERM and one with ENOSYS, both running a glibc ≥ 2.34 binary that forks. Confirm the
    EPERM one breaks and the ENOSYS one falls back. *Settles §8.1's `[R]` MED.*
11. **`no_new_privs` breakage inventory.** Start a container with NNP on and run `sudo`,
    `ping`, `su`, `passwd`. Record exactly which fail and how. *Prices the §7.4 deviation.*
12. **Masking with a nonexistent path** — add `/proc/does_not_exist` to `maskedPaths` and
    confirm the runtime starts. *Settles §10.3's `ENOENT`-skip requirement.*
13. **The `WORKDIR /proc/self/fd/N` test** — build an image with that WORKDIR and start it
    under our runtime; confirm it fails, and that `close_range` is why. *A regression test
    for CVE-2024-21626, and it should live in CI forever.*
14. **`--privileged` + `core_pattern`** in a throwaway VM: confirm §15.2 actually works, so
    the documentation claim is measured rather than repeated.
15. **systemd garbage collection.** Create `/sys/fs/cgroup/mydocker/x` by hand, then
    `systemctl daemon-reload` and `daemon-reexec`; check whether it survives. *Settles the
    §2.6 `[C]` MED risk that is currently the main argument for the D-Bus driver.*
16. **Rootless with a delegated user session.** As a non-root user with a systemd session:
    confirm which controllers appear in `user@N.service/cgroup.controllers` out of the box,
    and whether `cpu` is among them. *Settles §16.2's `[F]` MED.*

---

## 22. Handoff

**To D1 (kernel isolation) — answering its §12 hand-offs to me:**

1. **`NoNewPrivs: 0` explained and overridden.** D1 measured Docker's `NoNewPrivs: 0` and
   correctly inferred runc takes the `CAP_SYS_ADMIN` branch. §7.4 adds *why* Docker does
   it (setuid-binary compatibility) and recommends we **default it ON**, which collapses
   D1 §3.5's "two mandatory code branches" into one. That is a simplification D1's
   sequence can absorb.
2. **`CAP_MKNOD` useless outside the initial userns** (D1's measurement) is now a design
   decision, not just a fact: §14.3 recommends bind-mounting device nodes **in both
   modes**, so the rootful path stops using `mknod` at all.
3. **`CLONE_INTO_CGROUP` is worth using** (§1.7), and D1's open question 1 is item 4 in my
   §21 experiment list.
4. **`apparmor_restrict_unprivileged_userns=1`** — confirmed on HOST-A, and §9.2 explains
   the mechanism (capabilities stripped inside the userns, not creation blocked) and why
   the error will look like a kernel bug.
5. **ROCKY is a bad proxy for cgroup v2** — agreed, and §5.5 turns that into the
   recommendation to skip v1 entirely, with the reversibility argument.
6. **New constraint for D1's sequence:** `close_range(3, ~0u, 0)` must go immediately
   before `execve`, after all mounts (§20 C16). D1 §10 does not currently have it.

**To D2 (runtime architecture):**

- **Both decisions D2 §12 handed me are made.** Seccomp: **hand-built `sock_filter`, with
  `seccompiler` as the fallback** — not `libseccomp`, confirming D2 §12.11's static-binary
  reasoning (§18.2). Cgroups: **no crate at all** (§18.1).
- **D2 §12.3's LOW-confidence open question is settled**: `rustix::thread::CapabilitySet`
  carries all 41 constants including `CAP_BPF`, `CAP_PERFMON`, `CAP_CHECKPOINT_RESTORE`
  (§18.3). The `caps` dependency can be dropped without a caveat.
- **The `CgroupManager` trait shape is a D2-level decision** and §2.5 gives the evidence:
  runc's systemd driver calls `fsMgr.Set()` **in addition to** D-Bus, so the fs driver is a
  *component* of the systemd driver, not an alternative. Build `FsManager` first;
  `SystemdManager { dbus, fs: FsManager }` later.
- **`stop` needs both a pidfd poll and a `cgroup.events` poll** (§4.4). D2's lifecycle
  design should carry two exit signals, not one: "PID 1 exited" ≠ "container empty".
- **`inspect` must be able to report effective-vs-declared** (§2.4) — a real
  `TasksMax=19147` that Docker never asked for and does not report.
- **`Privilege` (§17.3) is a startup-time singleton** and belongs in D2's state model.
  The review rule that makes it work: `geteuid()` appears exactly once in the codebase.

**To D3 (images / layers / overlay):**

- **uid shifting is yours mechanically, mine as a privilege concern.** §9.3: with a
  single-UID map, `chown` to any unmapped uid fails with **`EINVAL`**, not `EPERM`. An
  image with files owned by uid 100 cannot be extracted correctly. Detect the absent
  `/etc/subuid` entry **at startup** and say so, not at file 4,000.
- **`userxattr` (5.11) is required in a userns** because `trusted.*` xattrs need
  `CAP_SYS_ADMIN` in the *initial* userns. `[O]` HOST-A shows rootful Docker mounts
  overlay with `nouserxattr`, confirming it is the rootless-only option.
- **Idmapped mounts (§9.5) are the alternative to `chown`-ing a layer**, and overlayfs
  gained idmap support only in **5.19** (bind mounts: 5.12). If storage is designed around
  "the uid on disk is the uid in the tar", the idmap retrofit is cheap; if it is designed
  around per-user `chown`ed copies, it is not.
- **Mount the container rootfs `nodev`** (§14.3) — a device node in the image tar is
  otherwise live. Docker does not do this; it relies on the device cgroup we are choosing
  not to implement.
- **Tar extraction must not honour setuid bits from untrusted images** if we default
  `no_new_privs` ON — they become inert anyway, which is a nice property to state.

**To D4 (networking):**

- The only thing I own here: **`CAP_NET_ADMIN` in the *host* network namespace is the gate**
  for veth creation, bridge attachment and netfilter. Rootless cannot have it, which is why
  §17.2 classifies the rootless network as a *different implementation*, not a fallback —
  the most expensive rootless retrofit in the project and it is yours.
- `CAP_NET_RAW` is in Docker's default 14 and I recommend **dropping it** (§7.6). That
  breaks `ping` inside containers. If D4 wants `ping` to work by default, say so and we
  keep it; otherwise the flag is `--cap-add NET_RAW`.
- `CAP_NET_BIND_SERVICE` is in the default set, so a **root** container binding :80 works.
  A **non-root** container binding :80 needs the ambient-capability sequence in §7.5.

---

## 23. Open questions

1. **ROCKY was unreachable this entire session** (`ssh old-host` → DNS `NXDOMAIN` for
   `old-host.example.edu`; likely a campus-VPN requirement). Everything marked `[O]` ROCKY here is
   inherited from D1's probe of the same host, not re-verified by me. The claims that
   specifically needed fresh evidence, with the command that settles each:
   - **`chown` errno with a single-UID map** (§9.3, claimed `EINVAL`).
     `unshare -Ur sh -c 'touch /tmp/f && chown 100 /tmp/f'` and read the errno.
   - **`setuid(65534)` errno** in the same namespace. A 5-line C or Rust program.
   - **`useradd`-less `newuidmap` failure text** with no `/etc/subuid` entry.
   - **cgroup v1 write failure errno** for a Slurm-owned hierarchy:
     `mkdir /sys/fs/cgroup/memory/test` → `EACCES` or `EPERM`?
2. **`cgroup.kill` in a threaded cgroup** — I claim `ENOTSUP` from the kernel doc's "not
   supported in threaded cgroups", but I did not find the errno in the source. §21 item 6.
3. **`EBUSY` for both no-internal-process violations** (§1.3) is inferred from the
   documented rule, not read from `kernel/cgroup/cgroup.c`. §21 item 3, or
   `grep -n EBUSY kernel/cgroup/cgroup.c` around `cgroup_attach_permissions` and
   `cgroup_subtree_control_write`.
4. **systemd garbage-collecting fs-driver cgroups** (§2.6) is the main argument for the
   D-Bus driver, and it is `[C]` MED — I have the documented single-writer rule but no
   observation of systemd actually removing an unknown top-level cgroup on modern versions.
   §21 item 15. If it turns out systemd leaves them alone on 256, the fs driver's case
   gets much stronger.
5. **`memory.events.local` first kernel version** — I attribute it to 5.2 alongside the
   `memory_localevents` mount option but did not confirm. Settles it:
   `git log -S memory.events.local mm/memcontrol.c`.
6. **The RHEL overlay-in-userns backport** (§6.2) — I have Red Hat documentation stating
   native rootless overlay arrives with **5.11** (RHEL 8.5 notes) while D1 measured it
   working on RHEL **4.18**. I could not find the RHEL kernel commit. `[C]` MED. Settles
   it: `rpm -q --changelog kernel | grep -i overlay` on ROCKY, or the RHEL kernel source
   for `ovl_own_xattr` / `FS_USERNS_MOUNT` on overlayfs.
7. **Whether `docker inspect` ever reports the effective `TasksMax`** — I checked
   `HostConfig.PidsLimit` (absent) and the cgroup (19147). I did not exhaustively search
   Docker's API for an effective-limits field. `[O]` HIGH on the discrepancy, `[C]` MED on
   "Docker does not report it".
8. **`seccompiler`'s architecture coverage.** I confirmed its action set has no
   `USER_NOTIF`; I did **not** confirm which `TargetArch` values it supports beyond
   x86_64/aarch64, which matters only if we ever want the i386 sub-architecture rules
   Docker generates (§8.3). Settles it: `docs.rs/seccompiler/0.5.0/seccompiler/enum.TargetArch`.
9. **Whether a `memory.high` default would actually help or hurt** (§3.1). The argument
   against — permanent throttling is a worse outage than a crash — is real and I have no
   data. `[R]` MED. Settles it: §21 item 1, plus running a real workload against both.
10. **AppArmor profile transition under `no_new_privs`** (§12.1's last `[F]` MED). If NNP
    and profile loading interact badly, our §7.4 deviation and our §12 plan collide.
    Settles it: on the dev VM with AppArmor, load a docker-default-like profile, set NNP,
    and exec.
11. **The exact set of controllers a default `user@.service` delegates** (§16.2) — I claim
    `memory` and `pids` but not `cpu` on many distros, at `[F]` MED. §21 item 16.
12. **`io.max` and device-mapper/LVM stacks** (§3.6) — whether throttling applies at the
    visible `dm-` device or the underlying one. `[R]` flagged it as a footgun without
    resolving it. Settles it: on the dev VM with an LVM volume, set `io.max` on both
    major:minor pairs and measure which takes effect.

---

## 24. Sources

**Kernel documentation**
- `Documentation/admin-guide/cgroup-v2.rst` (v6.11) — no-internal-process constraint,
  `cgroup.subtree_control` enabling model, delegation model and containment, `cgroup.type`
  and the four thread modes, `cgroup.kill`, `cgroup.freeze`, `memory.max`/`high`/`low`/`min`/
  `swap.max`, `memory.events` field meanings, `cpu.max`/`cpu.weight`/`cpu.stat`
  — https://raw.githubusercontent.com/torvalds/linux/v6.11/Documentation/admin-guide/cgroup-v2.rst
- `Documentation/admin-guide/cgroup-v1/*.rst` — v1 file names and semantics (memsw, cfs
  quota/period, `release_agent`, `notify_on_release`)

**Man pages** (man-pages 6.8-2 as installed on HOST-A, cross-checked against man7.org)
- `cgroups(7)` — v2 delegation to less-privileged users (the exact chown list), delegation
  containment rules including the "delegatee can't place the first process" consequence,
  `/sys/kernel/cgroup/{delegate,features}`, no-internal-processes precise form, v1-vs-v2
  differences — https://man7.org/linux/man-pages/man7/cgroups.7.html
- `capabilities(7)` — the five sets, the `execve` transformation formulas, root/capability-dumb
  binary rules, effect of UID changes, `PR_SET_KEEPCAPS`/`SECBIT_KEEP_CAPS`, ambient
  preconditions, the `CAP_SYS_ADMIN` grant list
  — https://man7.org/linux/man-pages/man7/capabilities.7.html
- `seccomp(2)` — `struct seccomp_data`, the `SECCOMP_RET_*` precedence order,
  `SECCOMP_SET_MODE_FILTER` and the `EACCES`/`no_new_privs`/`CAP_SYS_ADMIN` rule, the
  `SECCOMP_FILTER_FLAG_*` list, the x32 `__X32_SYSCALL_BIT` bypass, the vDSO and
  multiplexed-syscall caveats — https://man7.org/linux/man-pages/man2/seccomp.2.html
- `user_namespaces(7)` — `uid_map`/`gid_map` write rules, the single-mapping rule, the
  `setgroups` deny requirement and its rationale, the 340/5-line limit, capabilities in a
  userns — https://man7.org/linux/man-pages/man7/user_namespaces.7.html
- `prctl(2)` — `PR_SET_NO_NEW_PRIVS`, `PR_CAPBSET_DROP`, `PR_CAP_AMBIENT`, `PR_SET_KEEPCAPS`
- `clone(2)` — `CLONE_INTO_CGROUP` (v2-only, 5.7), `clone3` and `struct clone_args`
- `proc(5)` — `/proc/<pid>/status` capability fields, `hidepid`, magic links
- `mount_setattr(2)` — `MOUNT_ATTR_IDMAP`, `AT_RECURSIVE`
- `openat2(2)` — `RESOLVE_BENEATH`, `RESOLVE_NO_MAGICLINKS`, `RESOLVE_NO_SYMLINKS`
- `pidfd_open(2)`, `pidfd_send_signal(2)`, `close_range(2)`, `memfd_create(2)`

**OCI**
- OCI Runtime Spec v1.2.0 — `linux.resources`, `process.capabilities` (five sets),
  `linux.seccomp`, `linux.namespaces`, `linux.uidMappings`/`gidMappings`,
  `linux.maskedPaths`/`readonlyPaths`, `linux.devices`, "Default Devices"

**Runtime and engine source (pinned, read as raw files)**
- moby/moby **v28.3.1**
  - `oci/caps/defaults.go` — the 14 default capabilities, verbatim
  - `oci/defaults.go` — `maskedPaths`, `readonlyPaths`, default mounts, default devices
  - `profiles/seccomp/default.json` — 13 KB, 31 groups, `defaultAction: SCMP_ACT_ERRNO`,
    `defaultErrnoRet: 1`, 426 allowed names, the `archMap` sub-architecture list, the
    `socket`/`AF_VSOCK`, `clone`-namespace-mask and `clone3`→ENOSYS rules
  - `profiles/apparmor/template.go` — the whole `docker-default` profile, verbatim
- opencontainers/runc **v1.2.5**
  - `libcontainer/cgroups/systemd/v2.go` — `genV2ResourcesProperties`, `UnifiedManager.Apply`,
    `UnifiedManager.Set` (D-Bus **and** `fsMgr.Set`), `cgroupFilesToChown`
  - `libcontainer/cgroups/systemd/common.go` — `addCpuQuota` and the 1%-rounding comment
  - `libcontainer/cgroups/utils.go` — `ConvertMemorySwapToCgroupV2Value`,
    `ConvertBlkIOToIOWeightValue`
  - `libcontainer/specconv/spec_linux.go` — `AllowedDevices` and the maintainer's
    "questionable (if not outright wrong)" comment
- youki **v0.7.0** — `crates/libcgroups` read as a reference for v2 stat parsing (not adopted)

**Crate metadata** (crates.io API and docs.rs, fetched 2026-09-05)
- `seccompiler` 0.5.0 (rust-vmm) — `SeccompAction` variants; no `Notify`
- `libseccomp` 0.4.0 (C FFI), `caps` 0.5.6, `cgroups-rs` 0.5.1 (now `kata-containers/cgroups-rs`),
  `libbpf-rs` 0.27.1, `aya` 0.14.0, `oci-spec` 0.10.0, `rustix` 1.1.4, `nix` 0.31.3
- `docs.rs/rustix/1.1.4/rustix/thread/struct.CapabilitySet.html` — all 41 capability constants

**CVEs / escape research**
- CVE-2019-5736 — runc `/proc/self/exe` overwrite; fixed by sealed `memfd` re-exec
- CVE-2022-0492 — cgroup v1 `release_agent` writable from an unprivileged userns
- CVE-2019-14271 — `docker cp` / `docker-tar` loading `libnss_*` from container storage
- CVE-2024-21626 — runc leaked fd + `process.cwd`/`WORKDIR /proc/self/fd/N`
- CVE-2021-30465 — runc mount-destination symlink race
- PLATYPUS (RAPL side channel) — the reason `/sys/devices/virtual/powercap` is masked

**Empirical**
- **HOST-A** `host-a` / `HOST-A`, Ubuntu 24.10, `6.11.0-29-generic`, x86_64,
  4 CPUs. Docker **28.3.1**, containerd **1.7.27**, runc **1.2.5** (libseccomp 2.5.5, OCI
  spec 1.2.0), cgroup driver **systemd**, cgroup version **2**, storage `overlay2`,
  AppArmor on, systemd **256**. **Every command run was a read.** No cgroup write, no
  `sysctl -w`, no `systemctl`, no container lifecycle operation, no netfilter or AppArmor
  change, nothing touching :80/:443. Commands used: `findmnt`, `cat` of `/sys/fs/cgroup/**`
  and `/proc/<pid>/{status,cgroup,mountinfo,uid_map,gid_map,setgroups,attr/current,ns/*}`,
  `capsh --decode`, `aa-status`, `bpftool cgroup tree` / `prog list`, `systemctl show`,
  `docker ps`/`docker inspect`/`docker info` (read-only), `sysctl` (read).
- **ROCKY** `old-host` / `OLD-HOST`, Rocky 8.8, `4.18.0-425.3.1.el8` — **unreachable this
  session** (DNS `NXDOMAIN`). All ROCKY claims are inherited from
  `D1-kernel-isolation.md` §12–§13 and are marked `[O] ROCKY (D1)`; see §23 item 1.
