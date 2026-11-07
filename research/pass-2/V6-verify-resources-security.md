# V6 — adversarial verification of D5 §0–§18 (resource control & security boundary)

**Verifier:** V6. **Date:** 2026-10-23. **Target:** `research/D5-resources-security.md`
(3828 lines, written 2026-09-04), scope **§0–§18**. §19 (Rust crates) belongs to V7;
§17–§18's networking half is V5's. My side of those is stated where it arises and not
pursued further.

## Method and evidence base

Breadth pass over every section §0–§18 (enough to say whether anything moved), then depth
on the ten adjudication targets. Falsification-first: for each target I went to the primary
source and tried to break the claim, not to find a quote that matches it.

**Primary sources read (pinned, recoverable):**

| source | pin | used for |
|---|---|---|
| `Documentation/admin-guide/cgroup-v2.rst` | `torvalds/linux@v6.17` (+ bisected v4.19…v6.11 for feature-arrival dates) | §1–§7 |
| `kernel/cgroup/cgroup.c`, `kernel/cgroup/freezer.c`, `kernel/cgroup/pids.c`, `kernel/signal.c` | `torvalds/linux@v6.17`, diffed against v6.12/v6.13/v6.14 | `cgroup.kill` mechanism, freeze×SIGKILL |
| `kernel/seccomp.c`, `include/uapi/linux/bpf_common.h`, `arch/x86/Kconfig` | `torvalds/linux@v6.17` | §9 limits, x32 |
| `capabilities(7)`, `prctl(2)`, `user_namespaces(7)`, `pid_namespaces(7)`, `seccomp(2)` | man7.org, 2026-10-23 | §5, §8, §9, §10 |
| runc `libcontainer/{init_linux.go,cgroups/utils.go,cgroups/fs2/*,cgroups/systemd/v2.go,capabilities/capabilities.go}` | `opencontainers/runc@v1.2.5` (the version live on `host-a`) | §2–§3, §8.5, §8.7 |
| moby `oci/caps/defaults.go`, `profiles/seccomp/default.json`, `daemon/daemon_unix.go` | `moby/moby@v28.3.1` (the version live on `host-a`) + spot-checks at `docker-v29.9.2` | §8.2, §9.6, §2.4 |
| systemd `src/core/manager.c`, `src/core/cgroup.c`, `src/basic/limits-util.c`, `man/systemd-system.conf.xml` | `systemd/systemd@v257` | §2.4 `DefaultTasksMax`, shares→weight |

**Empirical, read-only:** `ssh host-a` (`HOST-A`, Ubuntu 24.10, 6.11.0-29,
systemd 256) — `cat` of `/proc/sys/kernel/{pid_max,threads-max}`, `/sys/kernel/cgroup/{delegate,features}`,
`ls /sys/fs/cgroup`, `systemctl --version`. Nothing written, nothing started, no container touched.
`old-host` not re-probed — D5's `old-host` observations are internally consistent and I had no
falsifying hypothesis that needed a new probe there; where one is needed I say so in
*Still unverifiable*.

**Upstream drift since 2026-09-04 (7 weeks):** runc is now **v1.5.2** (2026-09-25) with
**v1.6.0-rc.1** (2026-10-06); moby is now **docker-v29.9.2** (2026-10-20). D5's `[C]` claims
are pinned to what runs on `host-a` (Docker 28.3.1 / runc 1.2.5), which is legitimate as
an `[O]`, but several of D5's *source path* citations are to a tree layout that does not exist
at that pin (see Part D). Nothing in the drift falsifies a `[C]` I checked.

**Verdict counts:** CONFIRMED 44 · CORRECTED 24 · DISPUTED 5 · WEAK-EVIDENCE 3 ·
UNVERIFIABLE-HERE 8. Eighteen of the corrections change or shift a recommendation and are
listed at the end. Two of D5's own five "run this on the local VM" experiments (§1.7-E1,
§1.7-E2) and one `[F] MED` open question (§13.3) turned out to be answerable from kernel
source and are closed here.

---

## Part A — §5 stop/kill semantics and `cgroup.kill` (target 1)

D5's own §21 admits `cgroup-v2.rst` was never read for this section. It has now been read.
The **conclusions survive**; three of the stated **mechanisms and version numbers do not**.

### A.1 `cgroup.kill` exact semantics — CONFIRMED, with three additions D5 omits

`cgroup-v2.rst` §"Core Interface Files" (`torvalds/linux@v6.17`, line 1030), verbatim:

> `cgroup.kill` — A write-only single value file which exists in non-root cgroups. The only
> allowed value is "1". Writing "1" to the file causes the cgroup and all descendant cgroups
> to be killed. … Killing a cgroup tree will deal with concurrent forks appropriately and is
> protected against migrations. **In a threaded cgroup, writing this file fails with
> EOPNOTSUPP** as killing cgroups is a process directed operation, i.e. it affects the whole
> thread-group.

- **Recursive: yes.** `cgroup_kill()` walks `cgroup_for_each_live_descendant_pre()`
  (`torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L4107-L4116`). CONFIRMED.
- **Version: 5.14.** Bisected the documentation: absent at `v5.13`, present at `v5.14`.
  D5's "Linux 5.14" is CONFIRMED (not a search-derived guess any more).
- **Only `1` is accepted** — any other integer returns `-ERANGE`, not `-EINVAL`
  (`cgroup.c#L4129-L4130`). D5 does not say this; an implementation that writes `"1\n"` is
  fine (`strstrip`), one that writes `"true"` gets `-EINVAL` from `kstrtoint`.
- **`EOPNOTSUPP` on a threaded cgroup** (`cgroup.c#L4141-L4142`). D5 omits this entirely.
  It is inert for us only because §1.4 decides to skip threaded cgroups — but our
  `kill_all()` must still map `EOPNOTSUPP` to a comprehensible error rather than retrying.
- **Kernel threads are skipped** and **tasks already carrying a pending fatal signal are
  skipped** (`cgroup.c#L4094-L4100`). So `cgroup.kill` is idempotent and cheap to re-issue.

### A.2 D5's stated *mechanism* for fork-safety — **CORRECTED**

D5 §5.2: *"no process can fork out from under it, because the kernel holds the cgroup's lock
while walking."*

That is not how it works, and the real mechanism matters because it tells you what the
guarantee actually covers.

`__cgroup_kill()` (`torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L4081-L4105`):

1. increments `cgrp->kill_seq` under `css_set_lock`;
2. *then* iterates `css_task_iter` and `send_sig(SIGKILL, task, 0)` each task.

The fork side closes the race separately: `cgroup_can_fork()` snapshots
`kargs->kill_seq = cgrp->kill_seq` (`cgroup.c#L6581-L6584`), and `cgroup_post_fork()`
compares it against the current value and kills the brand-new child if it changed
(`cgroup.c#L6820`: `kill = kargs->kill_seq != cgrp_kill_seq;`). No lock is held across the
walk; a fork that *starts* before the seq bump and *completes* after it is killed by the
post-fork check.

Version note that matters for a 5.15 floor: this was a **`CGRP_KILL` flag bit** until
**v6.14**, when it was replaced by `kill_seq` (`CGRP_KILL` present in v6.12/v6.13
`cgroup.c`, absent in v6.14; `kill_seq` absent in v6.13, present in v6.14). The flag version
had a race with `CLONE_INTO_CGROUP`. **Practical consequence:** on a 5.15–6.13 kernel
`cgroup.kill` is correct for ordinary forks but had a known hole against a child cloned
*directly into* the cgroup. Our floor is 5.15 and we want `CLONE_INTO_CGROUP` (§1.6) — so on
pre-6.14 kernels the combination of `CLONE_INTO_CGROUP` + concurrent `cgroup.kill` is the
one case where the kernel, not our code, is racy. Not a reason to avoid either; a reason not
to claim "atomic".

Verdict: **CORRECTED** (conclusion holds, mechanism restated, version caveat added).

### A.3 `cgroup.kill` vs the walk-`cgroup.procs`-and-SIGKILL loop — CONFIRMED, sharpened

D5's three race modes are all real and all documented:

1. **fork between read and kill** — the loop is probabilistic; `cgroup.kill` is not.
2. **PID reuse** — `cgroup-v2.rst` on `cgroup.procs` (line 879ff), verbatim: *"The PIDs are
   not ordered and the same PID may show up more than once if the process got moved to
   another cgroup and then back **or the PID got recycled while reading**."* The kernel
   documentation states the recycle hazard on the read side itself; D5 inferred it. CONFIRMED
   and now primary-sourced.
3. D5 is right that `pidfd_open(2)` only shrinks the window rather than closing it: you still
   resolve a *number* to a process. The only race-free handle is one obtained at creation
   (`CLONE_PIDFD`), which is exactly §5.5's recommendation.

**The decisive argument D5 does not make:** with `--pid=host` or any shared PID namespace,
the walk loop signals **host PIDs by number**. A recycled PID there is not "an unrelated
process in the container", it is an unrelated *host* process, killed by a daemon running as
root. `cgroup.kill` cannot do that by construction because it never names a PID. This is a
stronger reason to require `cgroup.kill` than the fork race.

### A.4 `cgroup.freeze` version — **CORRECTED** (internal contradiction in D5)

D5 §5.3 says `cgroup.freeze` landed in **5.2**; D5 §5.2's pseudocode comments it
`# >= 4.19`. Documentation bisect: absent at `v5.1`, present at `v5.2`. **5.2 is correct;
the `4.19` in the §5.2 code block is wrong** and would, if copied into a probe table, make
the engine attempt a freeze on 4.19–5.1 kernels and get `ENOENT`.

### A.5 Freeze → kill → **thaw** — **CORRECTED**, and the thaw is the wrong move

D5 §5.2 marks the freezer/`SIGKILL` interaction `[F] MED` — *"I did not test the ordering"* —
and its fallback writes `cgroup.freeze = 0` after the kill loop. Settled from source, no test
needed:

- `cgroup-v2.rst` on `cgroup.freeze`: *"Processes in the frozen cgroup can be killed by a
  fatal signal."*
- Mechanism: `get_signal()` reaches the `fatal:` label and calls `cgroup_leave_frozen(true)`
  **unconditionally** before taking the fatal action
  (`torvalds/linux@v6.17/kernel/signal.c#L2997-L3000`). `cgroup_leave_frozen(true)` clears
  `current->frozen` and decrements the frozen count regardless of `CGRP_FREEZE`
  (`kernel/cgroup/freezer.c#L128-L146`).

So a `SIGKILL` delivered to a frozen task *takes effect while the cgroup is still frozen*.
The thaw is not "belt-and-braces" — **it is harmful**: thawing re-enables `fork()` for any
task the walk missed (which is precisely the case the freeze was there to prevent), reopening
the race in the window between the last `kill()` and the last exit. Also note
`cgroup_post_fork()` propagates `JOBCTL_TRAP_FREEZE` to children born into a frozen cgroup
(`cgroup.c#L6796-L6804`), so staying frozen is self-reinforcing.

**Corrected fallback:**

```
write("cgroup.freeze", "1")            # >= 5.2, NOT 4.19
poll cgroup.events until frozen == 1   # freeze is asynchronous
loop:
    pids = read("cgroup.procs")
    if pids empty: break
    for pid in pids: kill(pid, SIGKILL)   # frozen tasks still die
poll cgroup.events until populated == 0
rmdir                                   # never thaw; the cgroup is going away
```
The loop-until-empty is still needed because a task mid-`fork()` when the freeze landed may
complete it (the kernel doc says freezing "may take some time" and explicitly allows
`fork()` to race the freeze).

### A.6 The stop path: `SIGTERM` → grace → `SIGKILL`, and PID 1's signal disposition — CONFIRMED

`pid_namespaces(7)` is unambiguous and D5 reports it correctly:

> Only signals for which the "init" process has established a signal handler can be sent to
> the "init" process by other members of the PID namespace. … Likewise, a process in an
> ancestor namespace can … send signals to the "init" process of a child PID namespace only
> if the "init" process has established a handler for that signal. … **SIGKILL or SIGSTOP are
> treated exceptionally: these signals are forcibly delivered when sent from an ancestor PID
> namespace.**

So `SIGTERM` from the host to a container PID 1 with no `SIGTERM` handler is **discarded by
the kernel, not ignored by the process** — `kill(2)` returns 0 and nothing happens. CONFIRMED.
This, not "sh doesn't forward", is the *first* reason `docker stop` burns the full 10 s; D5
correctly gives both but presents the forwarding story first, which is the weaker of the two.

One refinement on D5's `sh -c` claim: both `dash` and `bash` exec-optimise
`sh -c "<single simple command>"`, so the *common* `CMD nginx -g 'daemon off;'` shell form
does end up with `nginx` as PID 1 and does stop cleanly. The failure is specific to shell
forms the shell cannot exec away (`a && b`, redirections, `$VAR` expansion that forces a
subshell). D5's "every stop is a SIGKILL" overstates it. **WEAK-EVIDENCE** on the universality
of the claim, CONFIRMED on the mechanism.

### A.7 Which primitive is correct for us: **kill the cgroup, with PID 1 as the polite opener**

D5 §5.4's answer survives and I would state it more strongly:

- `SIGTERM` to **PID 1 only** is the *graceful* request and must stay PID-1-directed: sending
  the stop signal to every task in the cgroup is what `docker kill --signal` deliberately does
  not do, and broadcasting `SIGTERM` to a worker pool whose supervisor is mid-drain produces
  worse outcomes than the 10-second wait.
- **Every forcible step goes through the cgroup.** `cgroup.kill` when present; freeze+walk
  otherwise (A.5). Never `kill(pid1, SIGKILL)` as the escalation — in a shared PID namespace
  that kills exactly one process.
- The wait predicate is `cgroup.events:populated`, which is `poll(2)`-able and, per the doc,
  reflects the whole subtree. D5's step-4 reasoning is sound.

D5's own hedge in §5.4 ("return as soon as PID 1 exits *and* schedule cgroup cleanup
asynchronously") is the right resolution and should be promoted from a trailing `[R]` to
*the* design: **exit status comes from PID 1's pidfd; teardown completion comes from
`populated`.** They are two different events and conflating them is where both Docker's
semantics and D5's step 4 get awkward.

---

## Part B — §4 OOM detection and §3 limit mapping (target 2)

### B.1 `memory.events` semantics — CONFIRMED verbatim, with one trap D5 misses

D5's six-row table is a verbatim transcription of `cgroup-v2.rst` §"Memory Interface Files"
(`torvalds/linux@v6.17`, lines 1450–1497). I checked every row against the source. All six
are **CONFIRMED**, including the two that carry the weight:

- `max` — *"The number of times the cgroup's memory usage was about to go over the max
  boundary. If direct reclaim fails to bring it down, the cgroup goes to OOM state."* Not a kill.
- `oom` — *"The number of time the cgroup's memory usage was reached the limit and allocation
  was about to fail. **This event is not raised if the OOM killer is not considered as an
  option**, e.g. for failed high-order allocations or if caller asked to not retry attempts."*
  Not a kill.
- `oom_kill` — *"The number of processes belonging to this cgroup killed by **any kind of**
  OOM killer."*
- `oom_group_kill` — *"The number of times a **group OOM** has occurred."* Note the unit
  difference D5's table reproduces but does not flag: `oom_kill` counts **processes**,
  `oom_group_kill` counts **events**. With `memory.oom.group=1` a single group kill of 40
  processes shows `oom_kill +40, oom_group_kill +1`.

**The trap D5 misses, and it is exactly the "lying in `ps`" failure mode the brief worries
about:** *"any kind of OOM killer"* **includes the global/system OOM killer**. A container
that never came near its own `memory.max` but was chosen as the victim under host-wide memory
pressure gets `oom_kill +1` in its `memory.events`. Reporting that as "this container exceeded
its memory limit" is wrong and sends the user to tune the wrong knob.

**Corrected detection rule** (replaces D5 §4.2's "`oom_kill_after > oom_kill_before` ⇒
OOM-killed"):

```
oom_killed            = Δoom_kill > 0
hit_own_memory_limit  = Δoom_kill > 0 AND (Δmax > 0 OR Δoom > 0)   # cgroup-local OOM path
# else: killed by the system OOM killer under host pressure — report differently
```
`max`/`oom` increment only on the memcg OOM path, so their delta is what separates "you hit
your limit" from "the host ran out and picked you". Both deltas come from the same single
read of `memory.events`, so this costs nothing. Verdict: **CORRECTED**.

### B.2 `memory.events` vs `memory.events.local` for a nested hierarchy — CONFIRMED, plus a mount-option caveat

`cgroup-v2.rst`: *"all fields in this file are hierarchical … For the local events at the
cgroup level see memory.events.local."* D5's reading is right: for "was **my container**
OOM-killed", read the recursive `memory.events` **on the container's own cgroup** — it already
aggregates the §1.3 leaf children. `.local` answers "which sub-cgroup".

**Caveat D5 omits:** the `memory_localevents` **mount option** (documented at
`cgroup-v2.rst` line 197) makes `memory.events` local-only *system-wide* — *"This option is
system wide and can only be set on mount or modified through remount from the init
namespace."* It is listed as available in `/sys/kernel/cgroup/features` on
`[O] host-a` (verified today: `nsdelegate favordynmods memory_localevents
memory_recursiveprot memory_hugetlb_accounting pids_localevents`), i.e. the kernel supports
it; whether it is *enabled* is a property of how the admin mounted cgroup2. On a host that
set it, D5's algorithm under-reports a kill that happened in a child cgroup.

Because we read `memory.events` on the container's own cgroup and (per §1.3) place the
workload in a child, this is a live hazard for us, not a theoretical one. **Fix: read both
`memory.events` and `memory.events.local` at the container cgroup and at the leaf, or simply
read `memory.events` at the leaf where the processes actually are.** The second is cheaper
and correct under either mount option. Verdict: **CORRECTED** (reasoning gap, not a wrong fact).

### B.3 Exit status 128+9 — CONFIRMED

`137 = 128 + SIGKILL` is produced identically by the OOM killer, `docker kill`, our own
`cgroup.kill`, and any `CAP_KILL` holder. D5's rule ("`ContainerState.oom_killed` sourced from
the counter, never inferred from 137") is correct and matches Docker, whose `State.OOMKilled`
comes from the cgroup event stream. One addition: with `memory.oom.group=1` the *whole*
container dies at 137 including PID 1, so the exit code is the same and the counter is the
only discriminator — a second reason B.1's rule must be counter-based.

### B.4 `memory.oom.group` version — **CORRECTED** (internal contradiction)

D5 §3.1 says `memory.oom.group` is **"(5.10+)"**. D5 §7.1's table says **4.19 `[F] MED`**.
Documentation bisect: `memory.oom.group` is present in `cgroup-v2.rst` at **`v4.19`**.
The table is right; §3.1 is wrong. This matters because D5 §7.3 uses the feature list to
justify a 5.10/5.15 floor; `memory.oom.group` is not one of the features that argues for it.

Also worth promoting out of D5's `[R] MED`: `memory.oom.group` is in
`/sys/kernel/cgroup/delegate` (verified on `host-a`), i.e. the kernel explicitly
considers it a file a **delegatee** may write. That is a hint about intent — it is meant to
be set by whoever owns the workload, which for us is the engine.

### B.5 `--memory` → `memory.max`, `memory.high`, and whether Docker sets it — CONFIRMED

- `memory.max` is *"Memory usage hard limit … the OOM killer is invoked in the cgroup"*;
  `memory.high` is *"Memory usage throttle limit … Going over the high limit **never invokes
  the OOM killer**"*. D5's distinction is verbatim-correct. CONFIRMED.
- **Docker sets no `memory.high`.** moby v28.3.1 `daemon/daemon_unix.go` builds
  `specs.LinuxMemory` from `Memory`, `MemoryReservation`, `MemorySwap`, `MemorySwappiness`,
  `KernelMemory`, `DisableOOMKiller` — there is no `MemoryHigh` field in the OCI runtime spec
  that Docker populates. runc writes `memory.high` only from `linux.resources.unified`
  (the `unified` passthrough map). So D5's `[C]` "Docker exposes no `memory.high` equivalent
  at all" is CONFIRMED, with the nuance that `--cgroup-conf memory.high=...` reaches it via
  the unified passthrough on a cgroup-v2 host.
- D5's `[R]` (expose `memory.high`, never default it) survives unchanged. Two additions worth
  carrying into the spec: `memory.high` opened `O_NONBLOCK` skips the synchronous reclaim
  (documented at `cgroup-v2.rst` line 1346) — relevant if the engine ever adjusts limits on
  a live container from its event loop, because the plain write **blocks the writer** doing
  reclaim on the target's behalf. The same is true of `memory.max` (line 1375), which means
  *shrinking* a container's memory limit can block our daemon thread arbitrarily long, or get
  the **writing thread** OOM-killed (`cgroup-v2.rst` line 3367: *"the new limit is met — or
  the task writing to memory.max is killed"*). **That is a real daemon-liveness hazard and
  D5 does not mention it.** Our `set()` on a live cgroup must use `O_NONBLOCK` or a dedicated
  thread.

### B.6 `--memory-swap` → `memory.swap.max` and the v1/v2 semantic difference — CONFIRMED

v1 `memory.memsw.limit_in_bytes` = memory **+** swap; v2 `memory.swap.max` = **swap alone**
(`cgroup-v2.rst` line 1819: *"Swap usage hard limit"*). runc performs the subtraction in
`cgroups.ConvertMemorySwapToCgroupV2Value()`
(`opencontainers/runc@v1.2.5/libcontainer/cgroups/utils.go#L432-L457`), and the function is
worth copying wholesale because its five special cases are all real:

| input | result |
|---|---|
| `memory == -1 && swap == 0` | `-1` (unlimited memory ⇒ unlimited swap) |
| `swap == -1` or `swap == 0` | pass through unchanged |
| `memory == -1` | pass swap through unchanged |
| `memory == 0` | **error** — "unable to set swap limit without memory limit" |
| `swap < memory` | **error** — "memory+swap limit should be >= memory limit" |
| otherwise | `swap - memory` |

D5 cites this as living in `fs2/memory.go`; it is actually in `cgroups/utils.go` and only
*called* from `fs2/memory.go#setMemory`. Minor, but the brief asked for recoverable citations.
Also note `fs2/memory.go` has a sixth case D5 omits: when `memory == swap` the subtraction
yields 0, `numToStr(0)` returns `""`, and runc explicitly rewrites it to `"0"` because
*"never write empty string to `memory.swap.max`, it means set to 0"* — i.e. the "equal values
mean no swap" Docker idiom. An implementation that writes the empty string gets a silently
different outcome. Verdict: **CONFIRMED** with a citation fix and one added case.

### B.7 `--oom-kill-disable` on cgroup v2 — **CORRECTED**, and D5's `[R]` gets *stronger*

D5 §3.1 `[C]`: *"Docker returns an error for `--oom-kill-disable` on cgroup v2 hosts (with
`--memory`) rather than silently ignoring it."*

**False.** moby v28.3.1:
- `pkg/sysinfo/cgroup2_linux.go#L85` sets `info.OomKillDisable = false` unconditionally on
  cgroup v2.
- `daemon/daemon_unix.go#L470-L477`: when `OomKillDisable` is requested and unsupported,
  Docker appends the **warning** *"Your kernel does not support OomKillDisable.
  OomKillDisable discarded."* and sets `resources.OomKillDisable = nil`. There is no error
  return on this path.

So Docker does exactly the accept-and-ignore that D5 holds up as the thing Docker avoids.
This does not weaken D5's `[R]` ("reject the flag with a message naming v2") — it removes its
false support and replaces it with a better argument: **Docker's behaviour here is an instance
of the §6.3 anti-pattern ("a requested limit silently not applied") and we should deliberately
diverge.** Promote this to the §6.3 rule's worked example.

### B.8 `--cpus` → `cpu.max` — CONFIRMED

`cpu.max` is `"$MAX $PERIOD"` in microseconds, default `"max 100000"`
(`cgroup-v2.rst` line 1172). `--cpus=1.5` ⇒ `"150000 100000"`. runc's `fs2/cpu.go#setCpu`
defaults the period to `100000` when unset, with a source comment citing the same doc.
CONFIRMED. D5's `cpuset.cpus` vs `cpuset.cpus.effective` observation is also correct and is
the right read-side warning.

### B.9 `--cpu-shares` → `cpu.weight`: the conversion formula — **DISPUTED / CORRECTED**

D5 §3.2 `[F] HIGH`: *"`[C]` **runc/systemd** use the mapping
`weight = 1 + ((shares - 2) * 9999) / 262142`, so `shares=1024 → weight≈39`."*

**runc and systemd use *different* formulas, and they disagree by a factor of 2.6 at the
default.** Both verified in source:

| implementation | formula | `shares=1024` → | `shares=2` → | `shares=262144` → |
|---|---|---|---|---|
| **runc** `cgroups.ConvertCPUSharesToCgroupV2Value` (`runc@v1.2.5/libcontainer/cgroups/utils.go#L421-L426`) | `1 + ((shares-2) * 9999) / 262142` | **39** | 1 | 10000 |
| **systemd** `cgroup_cpu_shares_to_weight` (`systemd@v257/src/core/cgroup.c#L1596-L1599`) | `CLAMP(shares * 100 / 1024, 1, 10000)` | **100** | 1 | 10000 |

systemd's is the linear "keep the default fixed" map (`CGROUP_CPU_SHARES_DEFAULT`=1024 ↦
`CGROUP_WEIGHT_DEFAULT`=100); runc's is the linear "keep the *endpoints* fixed" map. They
agree only at the extremes.

**Which one actually runs for a Docker container?** runc's — even with the systemd cgroup
driver, which is Docker's default on cgroup v2 (`[O] host-a`: `docker info` reports
`CgroupDriver=systemd`). The conversion happens in runc's **spec conversion**, not in its
cgroup driver: `specconv/spec_linux.go#L830-L835` sets
`c.Resources.CpuWeight = cgroups.ConvertCPUSharesToCgroupV2Value(CpuShares)` as soon as the
OCI `linux.resources.cpu.shares` field is seen. The systemd driver then sends the already-
converted value as the **`CPUWeight`** D-Bus property
(`runc@v1.2.5/libcontainer/cgroups/systemd/v2.go`), never `CPUShares`, so systemd's own
conversion is never reached. systemd's formula only applies to `.service`/`.scope` units that
literally set `CPUShares=` in a unit file.

**Consequence for us (this changes a recommendation):** D5's `[R]` "accept `--cpu-shares` for
compatibility, convert with that formula" is right about *which* formula to use — runc's is
the Docker-compatible one — but D5's reasoning ("runc/systemd use…") is wrong and would lead
someone implementing the systemd driver later to introduce a silent 2.6× behaviour change
between drivers. **State the formula as "runc's, deliberately, for Docker compatibility" and
hard-code it in `Resources`-to-`cpu.weight` translation, above the driver trait — never in
the driver.** That is a concrete reason D5 §2.3's trait boundary ("everything above this
trait speaks in *our* `Resources` type") must include the shares→weight conversion on the
*upper* side of the seam.

Range check: v1 `cpu.shares` is **2–262144** (`MIN_SHARES = 1UL<<1`, `MAX_SHARES = 1UL<<18`
in the fair scheduler), default 1024. v2 `cpu.weight` is **1–10000**, default 100
(`cgroup-v2.rst` line 1142: *"For non idle groups (cpu.idle = 0), the weight is in the range
[1, 10000]"*). D5's ranges are CONFIRMED. (A weight read of `0` means `cpu.idle = 1`, not an
error — worth knowing for the read side.)

### B.10 `--pids-limit` → `pids.max` — CONFIRMED

Number or `max`; `fork`/`clone` fail with `EAGAIN` when hit; `pids.events:max` increments.
The controller counts **tasks (threads)**, not processes. All CONFIRMED against
`cgroup-v2.rst` §"PID Interface Files". One correction to D5's `[O]`: the doc also notes
*"it is possible to have `pids.current > pids.max`"* (by setting the limit below the current
count, or by migrating tasks in), so a monitor must not assume the invariant.

Also: `pids.events` is hierarchical and `pids.events.local` exists; on `host-a`
`/sys/kernel/cgroup/features` lists `pids_localevents`, the analogue of `memory_localevents`.
Same caveat as B.2.

### B.11 cpuset — CONFIRMED with a version correction

`cpuset.cpus`/`cpuset.mems` passthrough and the subset-of-parent's-effective-set rule are
correct. But D5 §7.1 dates `cpuset.cpus.partition` to **"5.17 `[F] LOW`"**; it is documented
in `cgroup-v2.rst` from **`v5.0`**. (What arrived later is the `isolated` partition *type*
and the `root invalid` states — the section grows from 1 to 5 mentions between `v6.1` and
`v6.2`.) D5's "don't care" stance is unaffected; the fact is wrong.

---

## Part C — §2 delegation, systemd, and the `DefaultTasksMax` claim (target 3)

### C.1 The no-internal-process rule — CONFIRMED, and D5 under-states the root exception

`cgroup-v2.rst` §"No Internal Process Constraint" (`torvalds/linux@v6.17` line 501), verbatim:

> Non-root cgroups can distribute domain resources to their children only when they don't have
> any processes of their own. In other words, **only domain cgroups which don't contain any
> processes can have domain controllers enabled in their "cgroup.subtree_control" files.** …
> The root cgroup is exempt from this restriction. … Note that the restriction doesn't get in
> the way if there is **no enabled controller** in the cgroup's "cgroup.subtree_control".

Three precise points, each of which D5 §1.3 gets right but which are worth nailing because
the whole §1.3 layout rests on them:

1. What is forbidden is **not** "a cgroup with processes and children". It is **enabling a
   domain controller in `subtree_control` while the cgroup holds processes** (and,
   symmetrically, moving a process into a cgroup that has controllers enabled in
   `subtree_control`). A cgroup with processes *and* children is legal as long as
   `subtree_control` is empty.
2. The root cgroup is exempt because it must hold kernel threads and unattributable
   consumption.
3. The ordering the doc mandates: *"the cgroup must create children and transfer all its
   processes to the children **before** enabling controllers in its `cgroup.subtree_control`."*
   Our `create()` therefore has a mandatory order — `mkdir leaf` → move/clone processes into
   the leaf → only then enable controllers in the intermediate's `subtree_control`. Doing it
   the other way round returns `EBUSY`.

### C.2 `cgroup.subtree_control` — CONFIRMED

*"A controller can be enabled only if the parent has the controller enabled and a controller
can't be disabled if one or more children have it enabled"* (`cgroup-v2.rst` line ~495). So
`cgroup.controllers` (read-only: what is available here) is populated from the parent's
`cgroup.subtree_control` (read-write: what is handed down). D5's §1.2 framing is correct.

### C.3 What "delegation" means concretely — **CORRECTED** (the file list is five, not three)

D5 §2.1: *"grant that user write access to the subtree root directory **and** to these three
files in it — `cgroup.procs`, `cgroup.threads`, `cgroup.subtree_control`. **Nothing else.**"*

The three-file `chown` recipe is correct for delegation method 1 and is what the doc's prose
lists. But **"nothing else" is wrong as a statement about the kernel's delegation set.** The
kernel publishes the authoritative list at `/sys/kernel/cgroup/delegate`, and the doc points
at it: *"the kernel rejects writes to all files on a namespace root from inside the cgroup
namespace, **except for those files listed in `/sys/kernel/cgroup/delegate`** (including
`cgroup.procs`, `cgroup.threads`, `cgroup.subtree_control`, **etc.**)"*.

`[O] host-a HIGH` (verified today, `cat /sys/kernel/cgroup/delegate`, 6.11.0-29):

```
cgroup.procs
cgroup.threads
cgroup.subtree_control
memory.oom.group
memory.reclaim
```

Five files, not three. Two consequences:

- For **method 2 (cgroup namespace + `nsdelegate`)**, a container at its cgroupns root may
  write `memory.oom.group` and `memory.reclaim` on its own cgroup even though they are
  "resource control interface files". That is deliberate — both are properties of *how the
  workload is managed*, not of how the parent's resources are split.
- **Our engine must read `/sys/kernel/cgroup/delegate` rather than hard-coding a list.** It
  has grown twice (`memory.oom.group` and `memory.reclaim` were added after the original
  three) and will grow again. Hard-coding three means we under-`chown` on a future kernel and
  rootless workloads lose capability silently.

Also CONFIRMED, and this is the half D5 gets exactly right and is the important half: the
file that must **not** be writable by the delegatee is the **common ancestor's
`cgroup.procs`**. `cgroup-v2.rst` §"Delegation Containment": a non-root-euid migration
requires write access to *both* the destination `cgroup.procs` *and* the `cgroup.procs` of
the **common ancestor of source and destination**; otherwise `-EACCES`. That single rule is
what makes a delegated subtree a one-way container. Verdict for §2.1 overall: **CORRECTED**
(scope of "nothing else"), reasoning sound.

### C.4 systemd `Delegate=yes`, `user@.service`, and the rootless controller problem — CONFIRMED and sharpened

- **`user@.service`.** `systemd@v257/units/user@.service.in`, verbatim:
  `Delegate=pids memory cpu`, `DelegateSubgroup=init.scope`, `TasksMax=infinity`.
  D5's "`Delegate=pids memory cpu` (varies by version/distro)" is **CONFIRMED exactly** at
  v257. Note what is *absent*: **`io` and `cpuset` are not delegated to a user manager.**
  That is the structural reason rootless Podman can do `--memory`, `--cpus` and `--pids-limit`
  but cannot do `--device-read-bps` or `--cpuset-cpus`. D5 says rootless gets `--memory` but
  does not name the two that are permanently out of reach; it should, because our §6.3
  "requested-limit-that-cannot-be-applied is a hard error" rule will fire on exactly these two
  for every rootless user.
- `[O] host-a HIGH` (verified today) — the availability problem is live on a stock
  Ubuntu 24.10:
  ```
  /sys/fs/cgroup/user.slice/cgroup.controllers     : cpuset cpu io memory hugetlb pids rdma misc
  /sys/fs/cgroup/user.slice/cgroup.subtree_control : cpu memory hugetlb pids rdma misc
  ```
  `io` and `cpuset` are **available** at `user.slice` but **not enabled for its children**.
  A rootless engine under `user-<uid>.slice` therefore finds no `io.max` and no `cpuset.cpus`
  file at all — not a permission error, a **missing file**. Our probe must distinguish
  "controller not enabled in my parent's `subtree_control`" (fixable by the admin, name the
  fix) from "no writable cgroup at all" (not fixable). D5's §6.3 tier table has only the
  second case.
- **`user-<uid>.slice` gets `TasksMax=33%`**, from systemd's shipped drop-in
  `systemd@v257/units/user-.slice.d/10-defaults.conf`. So the "free fork-bomb ceiling" is
  *different* for rootless (33%) and rootful (15%) containers on the same host. D5 treats
  19147 as if it were a host constant.

### C.5 The `DefaultTasksMax = 19147` claim — **CORRECTED** (right number, wrong rule)

D5 §2.4 `[O]`: *"`kernel.threads-max` on this host is `127653`, and `127653 × 0.15 =
19147.95`. That is systemd's `DefaultTasksMax=15%` (**of `kernel.threads-max`**)."*

The number is right; the **rule is wrong**, and the wrong rule gives a wrong answer on most
hosts. The actual definition, from two independent primary sources:

- `systemd@v257/man/systemd-system.conf.xml`, verbatim: *"Defaults to 15% of the minimum of
  `kernel.pid_max=`, `kernel.threads-max=` **and root cgroup `pids.max`**. … For example,
  with the default `kernel.pid_max=`, `DefaultTasksMax=` defaults to **4915**."*
- `systemd@v257/src/core/manager.c#L120`: `#define DEFAULT_TASKS_MAX ((CGroupTasksMax) { 15U, 100U })`,
  resolved through `system_tasks_max()` in `src/basic/limits-util.c#L113-L160`, which takes
  `min(procfs_get_threads_max(), procfs_get_pid_max() - 1, root cgroup pids.max)` — the
  `-1` because *"PIDs assigned will be kept below this value"*.

`[O] host-a HIGH` (verified today): `kernel.pid_max = 4194304`,
`kernel.threads-max = 127653`, `/sys/fs/cgroup/pids.max` **does not exist** (no root
`pids.max` on a v2 unified root). So `min = 127653`, `× 15 / 100 = 19147`. D5's arithmetic
survives *on this host only because `pid_max` happens to be the modern 4194304*. On a host
with the historical `pid_max = 32768` the same formula yields **4915** — the number systemd's
own man page quotes. A 4× difference in the thing D5 calls "Docker's fork-bomb protection".

Also verified: `/etc/systemd/system.conf` carries `#DefaultTasksMax=15%` (commented), so this
host is on the built-in default and an admin may change it to anything including `infinity`.

**The other half of the `[O]`: does Docker set a pids limit by default? — CONFIRMED, no.**
`moby@v28.3.1/daemon/daemon_unix.go#L125-L136`:

```go
func getPidsLimit(config containertypes.Resources) *specs.LinuxPids {
    if config.PidsLimit == nil { return nil }       // <- user said nothing ⇒ nothing in the spec
    if *config.PidsLimit <= 0 { return &specs.LinuxPids{Limit: -1} }
    return &specs.LinuxPids{Limit: *config.PidsLimit}
}
```
`PidsLimit` is `*int64` and `nil` by default, so the OCI spec carries no `linux.resources.pids`
and runc writes nothing to `pids.max`. Unchanged at `moby@docker-v29.9.2` (same function,
same semantics). CONFIRMED.

**Net effect on D5 `[R]` "set a default `pids.max` ourselves":** the recommendation is
**strengthened**, not weakened. D5's argument was "systemd backstops Docker at 19147, we
won't have that". The true argument is sharper: *the backstop is not a constant — it is
`15% × min(pid_max−1, threads-max, root pids.max)` for system units, `33%` of the same for
user slices, zero for a bare `fs` driver outside systemd's management, and whatever the admin
configured. A fork-bomb ceiling that varies by a factor of 10 across hosts is not a ceiling.*
Pick our own number, document it, and make it a function of nothing.

One refinement on D5's proposed default: 2048 tasks is tight for a JVM (D5 says so) but it is
also tight for anything using a Go runtime with `GOMAXPROCS` on a 64-core box plus cgo
threads. A defensible alternative that keeps the "our number, not an accident" property is
`min(4096, 15% × threads-max)` — same order as systemd's for small hosts, capped so a
256-core machine does not hand each container 100k tasks.

### C.6 `fs` vs `systemd` driver, and the `Delegate=yes` transient scope — CONFIRMED

D5 §2.4's transient unit file artefact is the strongest evidence in the document and I could
not falsify it. The reading is right: `Delegate=yes` + only `*Accounting=yes` means runc asks
systemd for a *subtree*, then writes the control files itself. Two confirmations from source:

- `runc@v1.2.5/libcontainer/cgroups/systemd/v2.go#genV2ResourcesProperties` emits a small,
  fixed property set (`MemoryMax`, `CPUWeight`, `CPUQuotaPerSecUSec`, `TasksMax`,
  `AllowedCPUs`, `AllowedMemoryNodes`, device properties) and the `fs2` manager runs
  afterwards for everything else — D5's `[F] MED` inference in §2.3 is **upgraded to
  CONFIRMED**.
- The source carries an explicit comment about the device-BPF duplication that is §15's
  finding: *"This is of questionable correctness because we insert our own devices eBPF
  program later. Two programs with identical rules aren't the end of the world … it's unclear
  if systemd removes all eBPF programs attached when doing SetUnitProperties"*
  (`systemd/v2.go#L212-L216`). See Part G.

D5's `[R]` (fs driver first, into our own top-level subtree, `CgroupManager` trait from day
one) is sound and I have nothing to add except B.9's finding: the shares→weight conversion
belongs **above** the trait, not inside a driver.

---

## Part D — §8 capabilities, and the §8.5 ordering (target 5)

### D.1 The five sets and the `execve` transformation — CONFIRMED verbatim

D5's table and the five-line transformation are a correct transcription of `capabilities(7)`.
I re-derived the root special case from the man page rather than accepting D5's paraphrase,
because §0 item 10 ("the bounding set *is* the capability policy") rests entirely on it:

> (1) If the **real or effective** user ID of the process is 0 (root), then the file
> inheritable and permitted sets are ignored; instead they are notionally considered to be all
> ones. (2) If the **effective** user ID of the process is 0 (root) **or** the file effective
> bit is in fact enabled, then the file effective bit is notionally defined to be one.
> … the calculation … simplifies to:
> `P'(permitted) = P(inheritable) | P(bounding)` ; `P'(effective) = P'(permitted)`

So the exact simplification is `P(inheritable) | P(bounding)`, not D5's `P(bounding)`. They
coincide only because Docker (and we) leave `CapInh = 0`. **If anyone ever sets a non-empty
inheritable set, the bounding set stops being the whole policy** — and `capset(2)`'s rules
permit inheritable to retain a capability that is *not* in bounding (the constraint is
"subset of existing inheritable ∪ bounding", so an already-held inheritable bit survives a
later bounding drop). That is a small hole in §0 item 10 as stated, and the fix is one line in
our implementation: **clear inheritable before dropping bounding, and assert it stayed clear.**
Verdict: CONFIRMED with that caveat.

### D.2 Docker's default 14-capability set — CONFIRMED, and unchanged at the latest release

`moby@v28.3.1/oci/caps/defaults.go#DefaultCapabilities()` — exactly D5's 14, including
`CAP_NET_RAW`. `moby@docker-v29.9.2/daemon/pkg/oci/caps/defaults.go` — byte-for-byte the same
list. **`CAP_NET_RAW` has not been dropped** as of 2026-10-20, so D5's `[C]` has not drifted.
`0xa80425fb` as the fingerprint of "default Docker container" is correct.

**Citation defect worth flagging** (the contract calls this out explicitly): D5 cites
`moby/daemon/pkg/oci/caps/defaults.go`. That path **does not exist** at the version D5's own
`[O]` says is running (`v28.3.1`, where it is `oci/caps/defaults.go`); it exists only from the
`docker-v29.x` tree reorganisation. Likewise D5 cites the seccomp profile as
`moby/daemon/pkg/oci/fixtures/default.json` — at `v28.3.1` it is `profiles/seccomp/default.json`,
and at `v29.9.2` the real profile has moved out into a separate module, vendored at
`vendor/github.com/moby/profiles/seccomp/default.json` (the `daemon/pkg/oci/fixtures/` copies
are **test fixtures**, not the shipped profile). D5's `[C]` citations were taken from a moving
branch. The facts survive; the citations need re-pinning before anyone uses them.

`[C] MED` on runc's own `runc spec` default (3 caps) — **CONFIRMED**:
`runc@v1.2.5/libcontainer/specconv/example.go#L33-L49` has exactly `CAP_AUDIT_WRITE`,
`CAP_KILL`, `CAP_NET_BIND_SERVICE` in bounding/permitted/effective, and `NoNewPrivileges: true`.
Unchanged at `runc@v1.5.2`. Upgrade D5's `MED` to **HIGH**. Note the second half of that
example that D5 does not mention and that supports §8.4: **runc's own default spec sets
`NoNewPrivileges: true`.** Docker's `NoNewPrivs: 0` is a Docker policy choice layered on top
of a runtime whose own default is the opposite. That is a better argument for D5's `[R]`
("set `no_new_privs` by default") than the one D5 gives.

### D.3 §8.5's ordering — **CORRECTED. The sequence as written cannot work.**

This is the most consequential finding in my scope. D5 §8.5 prescribes:

```
 1. prctl(PR_SET_KEEPCAPS, 1)
 2. setgroups
 3. setresgid
 4. setresuid                      <-- euid 0 -> nonzero here
 5. prctl(PR_SET_KEEPCAPS, 0)
 6. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)   # "needs CAP_SETPCAP"
 7. capset()
 8. PR_CAP_AMBIENT_RAISE
 9. PR_SET_NO_NEW_PRIVS
10. seccomp
11. execve
```

**Step 6 returns `EPERM` for any container whose `USER` is not root.** Two primary sources,
both unambiguous:

- `PR_CAPBSET_DROP(2const)`, ERRORS: *"**EPERM** The caller does not have the `CAP_SETPCAP`
  capability."* (`capable()` in the kernel tests the **effective** set.)
- `capabilities(7)`, §"Effect of user ID changes on capabilities": *"If the effective user ID
  is changed from 0 to nonzero, then all capabilities are cleared from the **effective** set."*
  And, in the `SECBIT_KEEP_CAPS` description, in terms: *"**Note that even with the
  SECBIT_KEEP_CAPS flag set, the effective capabilities of a thread are cleared when it
  switches its effective UID to a nonzero value.**"*

`PR_SET_KEEPCAPS` preserves **permitted**, not **effective**. After step 4, `CapEff = 0`, so
`CAP_SETPCAP` is not effective, so every `PR_CAPBSET_DROP` in step 6 fails. The failure mode is
nasty: for a `USER root` container the sequence works (euid stays 0, effective is preserved),
so this passes every smoke test and fails *only* for non-root containers — the ones where
getting capabilities right matters most. And the failure is `EPERM` on a prctl that a naive
implementation logs and continues past, leaving the container with the **host's full bounding
set**.

**runc does it the other way round, with the comment spelling out why**
(`runc@v1.2.5/libcontainer/init_linux.go#L326-L362`, `finalizeNamespace()`):

```go
w, err := capabilities.New(caps)
// drop capabilities in bounding set before changing user
if err := w.ApplyBoundingSet(); err != nil { ... }
// preserve existing capabilities while we change users
if err := system.SetKeepCaps(); err != nil { ... }
if err := setupUser(config); err != nil { ... }      // setgroups/setgid/setuid
if err := system.ClearKeepCaps(); err != nil { ... }
if err := w.ApplyCaps(); err != nil { ... }          // capset: prm/eff/inh/ambient
```

**Corrected sequence for our container init:**

```
 1. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)   # euid is still 0; CAP_SETPCAP effective
 2. capset(): inheritable = {}                           # D.1: close the inheritable hole first
 3. prctl(PR_SET_KEEPCAPS, 1)                            # preserve PERMITTED across the uid change
 4. setgroups(supplementary)                             # CAP_SETGID
 5. setresgid(gid,gid,gid)
 6. setresuid(uid,uid,uid)                               # euid 0->nonzero: EFFECTIVE is wiped here
 7. prctl(PR_SET_KEEPCAPS, 0)
 8. capset(): permitted = effective = wanted; inheritable = ambient_wanted
 9. for cap in ambient_wanted: prctl(PR_CAP_AMBIENT_RAISE, cap)
10. prctl(PR_SET_NO_NEW_PRIVS, 1)
11. seccomp filter install
12. execve(entrypoint)
```

Why each move: **1 before 6** because `PR_CAPBSET_DROP` needs effective `CAP_SETPCAP`.
**8 after 7** because `capset` restores effective from the permitted set that `KEEPCAPS`
preserved — this is the step that un-does the kernel's effective-set wipe, and it is legal
without any capability (`capabilities(7)`: the new permitted must be a subset of the existing
permitted, the new effective a subset of the new permitted). **9 after 8** because of the
ambient invariant, verbatim: *"no capability can ever be ambient if it is not both permitted
and inheritable"* — so step 8 must put the ambient candidates in inheritable, not leave it
empty. D5's step 7 says `inheritable = wanted`, which raises everything into inheritable
unnecessarily; raise only what is actually going ambient (usually nothing).

D5's own §8.5 bullet "**6 before 7** — `capset` clamps to bounding" is also imprecise: per
`capabilities(7)` only the **inheritable** set is clamped by bounding; the new **permitted**
set is clamped by the *existing permitted*, which bounding does not touch. The bullet's
conclusion (drop bounding first) is right for a different reason — it has to be first because
of the `CAP_SETPCAP` requirement.

D5 is right that bounding drops are **irreversible**: `capabilities(7)` §"Capability bounding
set" — once dropped it cannot be restored, and it masks `F(permitted)` in the `execve`
transformation, so no file capability can re-introduce it in this process or any descendant.
CONFIRMED.

`SECBIT_NOROOT` / `SECBIT_KEEP_CAPS` — CONFIRMED as D5 describes them (`SECBIT_NOROOT`:
*"the kernel does not grant capabilities when a set-user-ID-root program is executed, or when
a process with an effective or real UID of 0 calls execve(2)"*; `SECBIT_KEEP_CAPS` is the
securebit form of `PR_SET_KEEPCAPS` and *"is always cleared on an execve(2)"*). D5's `[R]`
"skip securebits, `no_new_privs` + bounding covers it" is **DISPUTED, mildly**: `SECBIT_NOROOT`
covers something `no_new_privs` does not — it disables the *root* special case of D.1, which
is what makes uid-0-in-a-container equal to `P(bounding)`. With `SECBIT_NOROOT` set, a uid-0
container process that `execve`s an ordinary binary gets **no** capabilities at all rather
than the whole bounding set. That is a genuinely stronger posture for the `USER root` case
and it is one `prctl`. Not a correction — D5's tradeoff is defensible, Docker does not set it
and images would break — but the reasoning ("covers the same ground") is wrong and should say
"covers *less* ground, deliberately, for compatibility".

### D.4 §8.7's runc sequence and the "two branches" question — CONFIRMED, and it is where D1 and D5 actually differ

D5 §8.7's 21-step list matches `runc@v1.2.5/libcontainer/standard_init_linux.go` as far as I
checked it, including both seccomp branches and the two source comments quoted verbatim.
CONFIRMED.

The real D1-vs-D5 disagreement here is not about numbers of BPF branches (see Part E) but
about **how many code branches the seccomp install needs**: D1 §10.1/§12 says *"Two code
branches are mandatory"* (steps 25 and 32 of its table); D5 §8.7/§9.5 says *"we only need path
15b"*. **D5 is right and D1 is right, about different things.** The kernel constraint is
`seccomp(2)`: *"either the calling thread must have the `CAP_SYS_ADMIN` capability in its user
namespace, or the thread must already have the `no_new_privs` bit set"*, else `EACCES`
(CONFIRMED, man page + `kernel/seccomp.c#L686-L690`). That mandates two branches **only if
`no_new_privs` can be off**. D5 makes it unconditionally on, which deletes the other branch —
but only if the engine genuinely refuses to honour `--security-opt allow-new-privileges` for
the *seccomp* install, which D5 §8.4 offers as an escape hatch. **Adjudication: the two
branches collapse to one if and only if we treat `no_new_privs=off` as "install seccomp
early, while privileged" rather than as a separate ordering.** Say so explicitly in the spec;
as written, D5's §8.4 escape hatch quietly reintroduces D1's second branch.

### D.5 §8.3's "which of the 14 are dangerous" — DISPUTED on one item

The analysis is sound and I would not change the recommendation (ship Docker's 14 as the
default, 7 as `--security-profile=strict`). One item is mis-weighted:

- **`CAP_SETFCAP` is rated "low risk; drop anyway".** It is the highest-leverage of the four
  D5 is ambivalent about, because with `CAP_SETFCAP` a container process can write
  `security.capability` on a binary in its own writable layer — and that xattr **survives
  into a `docker commit`/image export**, so the capability escapes the container's lifetime
  into any future container started from that image, where the bounding set may be different.
  `no_new_privs` neutralises it for the *current* process tree only. Rate it the same as
  `CAP_MKNOD`: drop by default.

`CAP_DAC_OVERRIDE`, `CAP_NET_RAW`, `CAP_MKNOD` and `CAP_SETPCAP` analyses: CONFIRMED, no
changes. The `ping_group_range` observation (`[O] host-a`: host `1 0`, Docker sets
`0 2147483647` inside) is correct and is the right replacement mechanism for `CAP_NET_RAW`.

### D.6 §8.6 `CAP_SYS_ADMIN` and the per-capability table — CONFIRMED

`capabilities(7)` does contain the verbatim *"Don't choose CAP_SYS_ADMIN if you can possibly
avoid it!"*. The three escape routes (mount, unshare+mount, `/proc/sys` writes →
`kernel.core_pattern` pipe) are each real; see Part G for the `--privileged` adjudication of
which one is *the* vector. The `[O] old-host` in-userns observation (`CapEff = CapBnd =
0x1ffffffffff`, all 41 caps, yet `CAP_SYS_MODULE` cannot load a module because `init_module`
checks against the **initial** user namespace) is correct and is the single most useful
teaching example in §8.

---

## Part E — §9 seccomp, and the branch-count adjudication (target 4)

### E.1 The count, settled from the pinned profile

Counted from `moby@v28.3.1/profiles/seccomp/default.json` — the exact version running on
`host-a` — and cross-checked against `moby@docker-v29.9.2`
(`vendor/github.com/moby/profiles/seccomp/default.json`):

| quantity | v28.3.1 | v29.9.2 |
|---|---|---|
| rule groups (`syscalls[]` entries) | 31 | 37 |
| **unique syscall names mentioned** | **426** | **426** |
| names in the single unconditional `SCMP_ACT_ALLOW` group | 361 | 361 |
| `SCMP_ACT_ERRNO` rule groups | **1** | 1 |
| arch entries in `archMap` | 8 | 9 (adds LoongArch64) |
| **effective allowlist for a *default* container (14 caps, amd64)** | **370** | 370 |
| arg-filtered rule groups reachable by a default container | 7 (`socket`×1, `personality`×5, `clone`×1) | 7 |

The syscall name set is **identical** between the two releases — no drift in seven weeks.

**D5's "allowlist of 384 syscall names" is CORRECTED to 426 mentioned / 370 effective.**
The distinction matters for us: the number that constrains the generated BPF is the number of
*rules emitted for the container at hand* (370 on amd64 for a default cap set), not the
profile's total vocabulary.

### E.2 Does the count matter? `BPF_MAXINSNS` — **the answer is no, and D5 and D1 are both worrying about the wrong limit**

Verified limits:

- **`BPF_MAXINSNS = 4096`**, per filter. `include/uapi/linux/bpf_common.h#L53-L54`
  (`torvalds/linux@v6.17`); enforced at `kernel/seccomp.c#L680-L681`
  (`if (fprog->len == 0 || fprog->len > BPF_MAXINSNS) return ERR_PTR(-EINVAL)`);
  documented in `seccomp(2)` ERRORS: *"the filter program was zero or exceeded BPF_MAXINSNS
  (4096)"*.
- **`MAX_INSNS_PER_PATH = (1 << 18) / sizeof(struct sock_filter)` = 32768 instructions**
  across *all stacked filters*, with a **+4 instruction penalty per previously installed
  filter** (`kernel/seccomp.c#L237-L238`, `#L912-L919`), returning `ENOMEM`.
- **`struct sock_filter`'s `jt`/`jf` are `__u8`** (`include/uapi/linux/filter.h#L24-L29`),
  so the maximum forward jump in one instruction is **255**.

Order-of-magnitude for a realistic filter: a flat chain of `jeq <nr>, ALLOW` costs ~1
instruction per allowed syscall plus a shared `ret`. Native-arch-only ⇒ ~370 + arch check +
ENOSYS stub + ~30 for the seven arg blocks ≈ **~410 instructions**. Emitting all three x86
ABIs as Docker's `archMap` does ⇒ ~**1200**. Both are comfortably inside 4096, and libseccomp
compresses further with ordered jump chains.

**So `BPF_MAXINSNS` is not the binding constraint for a Docker-equivalent profile and no
design decision should be justified by it.** The two limits that *are* live:

1. **The 255-instruction jump offset.** runc's own ENOSYS patcher has two code paths for
   exactly this (`runc@v1.2.5/libcontainer/seccomp/patchbpf/enosys_linux.go#L379`,
   `#L449`: `if baseJumpEnosys+1 <= 255 { ... } else { /* long jump via skip chain */ }`).
   D5 mentions this in one clause of §9.6 and then does not carry it into §19.1's
   "hand-build vs `seccompiler`" argument, where it is the strongest point: **jump arithmetic
   with an 8-bit offset and a long-jump fallback is the part of hand-building that will
   actually bite us**, not the opcode encoding.
2. **`MAX_INSNS_PER_PATH` when filters stack.** Our filter + a nested runtime's filter +
   the workload's own (systemd's `SystemCallFilter=`, a Go program's `unix.Seccomp*`) each
   add their length + 4. Three 1200-instruction filters is 3.6k — fine; ten is not. Relevant
   only for docker-in-docker, but it is the limit that produces `ENOMEM` from `seccomp(2)`
   and nobody expects that errno.

Verdict: **CORRECTED** (count), and the dispute is resolved by observing that the limit both
files gesture at is not the one that binds.

### E.3 Install mechanics — CONFIRMED

- `seccomp(SECCOMP_SET_MODE_FILTER, flags, &prog)` since **3.17**; `prctl(PR_SET_SECCOMP,
  SECCOMP_MODE_FILTER, …)` since 3.5. Only `seccomp(2)` takes flags. CONFIRMED.
- The `CAP_SYS_ADMIN`-or-`no_new_privs` precondition and `EACCES`: CONFIRMED verbatim in
  `seccomp(2)` and in `kernel/seccomp.c#L686-L690` (*"Installing a seccomp filter requires
  that the task has CAP_SYS_ADMIN in its namespace or be running with no_new_privs. This
  avoids scenarios where unprivileged tasks can affect the behavior of privileged children."*).
  D5's stated *reason* (a filter could make a set-UID binary misbehave) matches the kernel
  comment's intent. CONFIRMED.
- **`TSYNC`**: CONFIRMED, with a refinement D5 gets slightly wrong. Plain
  `SECCOMP_FILTER_FLAG_TSYNC` on failure returns **the thread ID of the offending thread**
  (a positive value), not an errno; `ESRCH` is returned only when
  **`SECCOMP_FILTER_FLAG_TSYNC_ESRCH` (since Linux 5.7)** is also set, which exists precisely
  because the positive-return convention is incompatible with `SECCOMP_FILTER_FLAG_NEW_LISTENER`
  (which also returns a positive value — an fd). D5 says "fails (`ESRCH`, or `EBUSY` for older
  semantics)"; `EBUSY` is not the historical TSYNC error. Minor **CORRECTED**.
- **Do we need TSYNC in a single-threaded child? No.** CONFIRMED, and D5's inference to the
  D2 design constraint ("the process that installs seccomp, drops caps and `execve`s must not
  be multithreaded") is the right one and is independently forced by D1 §11.2's
  post-`clone` async-signal-safety rules.
- Filters are inherited across `fork`/`clone`, preserved across `execve`, cannot be removed,
  and the most restrictive result wins. CONFIRMED.

### E.4 The per-architecture problem — CONFIRMED, and still live

`seccomp(2)` states it almost exactly as D5 does:

> x86-64 ABI and the x32 ABI both use `AUDIT_ARCH_X86_64` as `arch`, and they run on the same
> processors. Instead, the `__X32_SYSCALL_BIT` is used on the system call number to tell the
> two ABIs apart. … **A list of system calls to be denied based on `nr` that does not also
> contain `nr` values with `__X32_SYSCALL_BIT` set can be bypassed by a malicious program that
> sets `__X32_SYSCALL_BIT`.**

**Is it still live in 2026?** Yes, conditionally: `CONFIG_X86_X32_ABI` is still a Kconfig
option in `torvalds/linux@v6.17` (`arch/x86/Kconfig#L3139`). It is off in most distro
kernels, in which case an x32 `nr` returns `ENOSYS` from the kernel and the bypass is inert —
but that is a *host configuration* property, not something we can assume. **A filter must
still handle it**, because we do not control which kernel our engine runs on and the failure
mode (silent total bypass of the filter) is the worst possible one. CONFIRMED, `[F] HIGH`
stands, with the caveat that D5 should have said "live wherever `CONFIG_X86_X32_ABI=y`".

`seccomp(2)` adds a second x32 trap D5 omits and that matters for the "deny unknown arch"
design: *"the x32 ABI uses syscall numbers in the ranges 512-547 **as well as** the
corresponding non-x32 syscalls ORed with `__X32_SYSCALL_BIT`. For example, `nr == 521` and
`nr == (101 | __X32_SYSCALL_BIT)` would result in invocations of `ptrace(2)` with potentially
confused x32-vs-x86_64 semantics."* So x32 has **two** encodings for the same call. D5's
recommendation (native arch only; deny every other `arch` value and any `nr` with bit 30 set)
handles both, which is a point in its favour that D5 does not make.

D5's `archMap` transcription is **CORRECTED** in one respect: it lists 7 entries; the real
file has **8** at v28.3.1 (it omits `SCMP_ARCH_RISCV64`) and 9 at v29.9.2 (adds
`SCMP_ARCH_LOONGARCH64`).

The `[R]` ("emit native arch only, deny the rest, make it a profile field") survives and I
agree with the weighting.

### E.5 The actions — CONFIRMED

The eight actions and their decreasing-precedence order are as D5 states, with
`SECCOMP_RET_KILL_PROCESS` and `SECCOMP_RET_LOG` at 4.14 and `SECCOMP_RET_USER_NOTIF` at 5.0.
The `[R]` (default `ERRNO`, never `KILL_*`, offer `LOG` as an audit mode) is well-reasoned and
I have no falsification. One `[C]` correction below (E.6) changes the stated Docker default.

### E.6 §9.6 "what Docker's default profile blocks" — **three concrete `[C] HIGH` errors**

**(a) "`defaultAction: SCMP_ACT_ERRNO` with no explicit `defaultErrnoRet`" — CORRECTED.**
The profile carries `"defaultErrnoRet": 1` explicitly (`EPERM`) at both v28.3.1 and v29.9.2.
Inconsequential for behaviour, but D5 presents it as an inference that libseccomp resolves,
and it is not an inference.

**(b) "There is not a single non-`ALLOW` rule in the profile" — FALSE.** There is exactly one,
and it is the most interesting rule in the file:

```json
{ "names": ["clone3"], "action": "SCMP_ACT_ERRNO", "errnoRet": 38,
  "excludes": { "caps": ["CAP_SYS_ADMIN"] } }
```
`errnoRet: 38` is **`ENOSYS`**. So **D5's §9.6 claim that "`clone3` does not appear in
Docker's profile at all" is also false** — `clone3` appears **twice**: allowed when the
container has `CAP_SYS_ADMIN`, and explicitly `ENOSYS` when it does not. The runc `patchbpf`
ENOSYS-above-max stub is a *separate, additional* mechanism for syscalls **newer than
anything the profile mentions**; it is not what handles `clone3`. D5 conflates the two and
draws its §0 item 13 conclusion from the wrong one.

The conclusion itself ("unknown/newer syscalls must return `ENOSYS`, not `EPERM`") is
**CONFIRMED and if anything strengthened** — it is important enough that Docker encodes it by
hand for the one syscall it knows about *and* runc patches the BPF for the rest. But the
argument in §19.1 ("`libseccomp` cannot express the `ENOSYS`-above-max rule … **This alone
decides it**") is weaker than stated: libseccomp *can* express a per-syscall `ENOSYS` rule
(Docker's profile does exactly that for `clone3`); what it cannot express is the open-ended
"anything above the highest number I know about". That is a real gap, but it is one rule, not
the decisive argument D5 makes of it. **DISPUTED** — flagging for V7, since §19 is theirs.

runc's own FIXME adds a caveat D5 should carry, because D5 §12.4 recommends `close_range`:
> *"This implementation can in principle cause issues with syscalls like `close_range(2)`
> which were added out-of-order in the syscall table between kernel releases."*
(`patchbpf/enosys_linux.go#L294-L304`.) An ENOSYS-above-max rule computed from *our* profile
can accidentally ENOSYS a syscall that exists, if the profile omits a low-numbered newer call.
Our implementation must compute the threshold from the **kernel's** syscall table, not from
the profile's maximum.

**(c) The `clone` namespace-flag mask — CORRECTED, and the derived `[R] MED` is false.**
D5 quotes `"value": 2080505856` = `0x7C020000` and concludes: *"`CLONE_NEWCGROUP`
(`0x02000000`) is **deliberately not** in the mask. Creating a cgroup namespace is harmless …
so Docker permits it unprivileged."*

The actual value in the profile is **`2114060288` = `0x7E020000`**, and it has been that
value since at least `moby@v20.10.24` (checked v20.10.24, v23.0.0, v24.0.0, v25.0.0, v28.3.1,
v29.9.2 — all identical). Decoded:

```
0x7E020000 = CLONE_NEWNS     0x00020000
           | CLONE_NEWCGROUP 0x02000000   <-- present, contra D5
           | CLONE_NEWUTS    0x04000000
           | CLONE_NEWIPC    0x08000000
           | CLONE_NEWUSER   0x10000000
           | CLONE_NEWPID    0x20000000
           | CLONE_NEWNET    0x40000000
```
Seven flags, not six. **Docker denies unprivileged `clone(CLONE_NEWCGROUP)`.** D5's
"Docker permits it because it is harmless" is an invented rationale for a bit that is not
there. The surviving half of D5's point is the useful one: *write the mask as a named
constant expression, not a magic number* — this correction is exactly what a magic number
costs.

**Is masking `clone`'s namespace flags expressible in classic BPF at all, given 32-bit
registers and a 64-bit argument?** Yes — **CONFIRMED, and for a specific reason**: every
`CLONE_NEW*` bit lives in the **low 32 bits** of `flags`, so a single
`BPF_LD|BPF_W|BPF_ABS` of the low word of `seccomp_data.args[0]` plus `BPF_ALU|BPF_AND` plus
`BPF_JMP|BPF_JEQ` suffices. The register-width problem does bite for arguments whose
interesting bits are in the high word (none here), and a correct implementation must still
load the **low** word at the endianness-correct offset — on big-endian the two 32-bit halves
of each `__u64` are swapped and `kernel/seccomp.c` says so explicitly: *"Endianness is
explicitly ignored and left for BPF program authors to manage"* (`#L241-L243`).

**The sharper point D5 half-makes and should make outright:** `clone3`'s flags are **not**
filterable at all. `clone3` takes a *pointer* to `struct clone_args`, and §9.1 establishes
that a cBPF filter cannot dereference pointers. So "block namespace creation" is expressible
for `clone` and structurally **inexpressible** for `clone3`. That — not kernel-version
fallback etiquette — is the real reason `clone3` has to be denied wholesale rather than
argument-filtered, and it is the reason Docker's single non-ALLOW rule exists.

### E.7 Which blocked syscalls matter — CONFIRMED

Verified against the pinned profile (membership = allowed somewhere; absence = denied by the
default `ERRNO`):

| syscall | in profile? | gate |
|---|---|---|
| `keyctl`, `add_key`, `request_key` | **absent** | denied outright — D5's §9.7 item 3 is right |
| `userfaultfd` | **absent** | denied outright (a top-tier kernel-exploit race primitive) |
| `io_uring_setup` (and `io_uring_enter`/`_register`) | **absent** | denied outright — D5's §9.7 item 3 CONFIRMED |
| `ptrace`, `process_vm_readv/writev`, `kcmp` | present | gated on `CAP_SYS_PTRACE` |
| `unshare`, `setns`, `mount`, `umount2`, `bpf`, `perf_event_open`, `mount_setattr`, `open_tree`, `move_mount`, `fsopen`/`fsconfig`/`fsmount`/`fspick` | present | gated on `CAP_SYS_ADMIN` |
| `open_by_handle_at` | present | gated on `CAP_DAC_READ_SEARCH` |
| `execve`, `execveat`, `close_range`, `read`, `write`, `statx` | present | unconditional |

Note the new-mount-API calls (`fsopen`/`fsmount`/`fsconfig`/`fspick`/`open_tree`/`move_mount`/
`mount_setattr`) are all in the `CAP_SYS_ADMIN` bucket. D1 §4.5 wants to use that API; if our
*default profile* mirrors Docker's, a container cannot use it, which is correct, but **our own
init must install the filter after it is done mounting** — already true in the corrected
ordering (D.3 step 11), worth stating because it is a failure that only shows up with
`--security-opt seccomp=` custom profiles.

D5's §9.6 belt-and-braces observation — gating in seccomp syscalls the kernel already gates
on a capability, as defence against a kernel capability-check bug, with CVE-2022-0492 as the
worked example — is the best idea in §9 and survives verification intact (see Part F.2 for
why the CVE-2022-0492 link is exactly right).

---

## Part F — §10 user namespaces (target 6)

### F.1 `uid_map`/`gid_map` write rules — CONFIRMED, with one omission that bites us as root

All six of D5's rules check out against `user_namespaces(7)`:

1. **Write-once**, `EPERM` on a second write. CONFIRMED verbatim.
2. **Position**: *"The writing process must either be in the user namespace of the process
   `pid` or be in the parent user namespace of the process `pid`."* CONFIRMED.
3. **Capability**: CONFIRMED but **conflated**. The man page has *two* distinct capability
   rules and D5 merges them: `CAP_SETUID`/`CAP_SETGID` **in the user namespace of `pid`** is a
   prerequisite for writing *at all*; `CAP_SETUID`/`CAP_SETGID` **in the parent user
   namespace** is what lifts the single-line restriction. Separating them matters because the
   first is what a fresh `CLONE_NEWUSER` child has by construction and the second is what our
   root daemon has.
4. **Unprivileged single-line rule**: CONFIRMED verbatim, including the two clauses D5 omits —
   *"The writing process must have the same effective user ID as the process that created the
   user namespace"*, and the `setgroups`-deny precondition for `gid_map`.
5. **Line limit**: CONFIRMED — 5 lines up to 4.14, **340 since 4.16**, and still 340 in the
   current man page. D5 omits two hard implementation constraints stated in the same bullet:
   *"the number of bytes written to the file must be **less than the system page size**, and
   the write must be performed **at the start of the file** (`lseek(2)` and `pwrite(2)` can't
   be used to write to nonzero offsets)."* So the map must go out in **one `write(2)` at
   offset 0, under 4096 bytes** — which, at ~30 bytes a line, caps a *practical* map at
   roughly 130 lines, well under 340. Our builder must enforce the byte limit, not the line
   limit.
6. **`setgroups` deny before `gid_map`**: CONFIRMED, including the one-way transition
   (*"a process cannot transition from a state where `setgroups(2)` is allowed to a state
   where `setgroups(2)` is denied"*) and D5's negative-permission rationale, which is the man
   page's own.

**The omission that matters for a root-first engine — a rule added in Linux 5.12 that D5
does not mention at all:**

> If updating `/proc/pid/uid_map` to create a mapping that maps **UID 0 in the parent
> namespace**, then one of the following must be true: (a) if the writing process is in the
> parent user namespace, then it must have the **`CAP_SETFCAP`** capability in that user
> namespace; or (b) if the writing process is in the child user namespace, then the process
> that created the user namespace must have had `CAP_SETFCAP` when the namespace was created.
> This rule has been in place since Linux 5.12.

We are root-first, and §10.4's `[R]` is *"support a user namespace with a mapping from the
start, even in root mode"*. Any such mapping that includes host uid 0 — an identity map, or
`0 0 1` — therefore requires **`CAP_SETFCAP` in the daemon**. Three places this will bite:

- A hardened daemon that drops `CAP_SETFCAP` from its own set (plausible: §8.3 recommends
  dropping it from containers, and the obvious generalisation is to drop it from ourselves).
- Our engine running **inside** a Docker container in CI: Docker's default 14 *does* include
  `CAP_SETFCAP`, so this happens to work — but under `--cap-drop=SETFCAP` or a
  Kubernetes `securityContext` that drops it, userns creation with a root mapping fails.
- The errno is `EPERM` on a `write(2)` to `uid_map`, identical to the write-once error and to
  the unprivileged-multi-line error. **Our error path must disambiguate these three by
  checking our own `CAP_SETFCAP` and the map's shape before reporting.**

Verdict: **CORRECTED** (material omission in a `[F] HIGH` rule list).

### F.2 Nested depth and the other kernel ceilings — CONFIRMED, and D5 does not mention them

`user_namespaces(7)`: *"The kernel imposes (since Linux 3.11) a limit of **32 nested levels**
of user namespaces."* Exceeding it fails `clone`/`unshare` with `EUSERS`. D5 does not state
the depth limit even though §18 contemplates nested containers. Also unmentioned and worth a
probe entry: `/proc/sys/user/max_user_namespaces` (per-userns quota, commonly 0 on hardened
hosts, which turns every rootless operation into `EPERM` with no explanation) and the
Debian/Ubuntu `kernel.unprivileged_userns_clone` knob D5 does mention in §11.5.

### F.3 `/etc/subuid`, `newuidmap`, and what `old-host` can actually map — CONFIRMED

The `[O]` matrix is internally consistent and matches the man page rules line by line. Two
observations I want to upgrade or qualify:

- **The `getcap` finding (`newuidmap` carries `cap_setuid=ep`, no setuid bit) + the
  `no_new_privs` interaction is correct and is the best catch in §10.** `prctl(2)`'s
  `PR_SET_NO_NEW_PRIVS` ignores file capabilities exactly as it ignores set-UID; a rootless
  engine that sets `no_new_privs` on itself and then `execve`s `newuidmap` gets an
  unprivileged helper and an `EPERM` with no diagnostic. **Promote this from `[R] MED` to
  `[R] HIGH`** — it is a hard ordering constraint, not a preference.
- **What `old-host` can map, definitively:** with no `/etc/subuid` entry for `sataromk`, the
  *only* legal map is the unprivileged single-line one, i.e. `0 900000000 1` in `uid_map` and
  `0 709200513 1` in `gid_map` after `setgroups=deny`. `newuidmap` cannot help — its own
  `/etc/subuid` check rejects any range, as D5's `[O]` shows. **So `old-host` can run exactly
  one uid inside a container and nothing else.** D5 states this; I confirm it follows
  directly from rule 4(b) without needing the empirical result.
- D5's `MED` puzzle (`unshare -Ur chown 0:0 file` → `EINVAL` while `chown 0` → `0`) has a
  straightforward explanation that does not need an `strace`: `unshare -Ur` writes
  `gid_map` only after writing `setgroups=deny`, and `util-linux`'s `-r` maps **one** gid.
  `chown 0:0` on `old-host` targets gid 0 → maps to the user's primary gid 709200513 → should
  work. The `EINVAL` therefore points at `chown` being called with a gid the namespace cannot
  translate, which happens if `unshare -Ur` on util-linux 2.32 (RHEL 8) wrote only `uid_map`
  and left `gid_map` empty — an **unmapped gid 0** then yields `EINVAL` exactly as an unmapped
  uid does. **WEAK-EVIDENCE**; the settling command is in the final section and is a `cat`,
  not an `strace`.

### F.4 What root-inside-a-userns does *not* have — CONFIRMED, and the §10.4 `[R]` is the right call

The `[O] host-a` evidence (same `user:[4026531837]` inode as host PID 1; `uid_map` =
`0 0 4294967295`) is decisive and I have nothing to add except to name the mechanism behind
D5's §8.6 `old-host` contrast: a capability held in a non-initial user namespace is checked by
`ns_capable(ns, CAP)` against the namespace that **owns the object**. `init_module`,
`reboot`, `settimeofday`, `bpf(BPF_PROG_LOAD)` for most program types, and writes to
non-namespaced `/proc/sys` entries all check `capable()` (the initial namespace), so a userns
root holds the bit and the check still fails. Conversely `mount` of a `FS_USERNS_MOUNT`
filesystem, `setns` into an owned namespace, and `chown` within the mapped range all check
`ns_capable()` and succeed. D5's `[R]` ("our error messages must never say 'you have
CAP_SYS_ADMIN' without saying which namespace") is correct and cheap.

**`chown` to an unmapped id → `EINVAL`, CONFIRMED and explained:** the kernel translates the
requested kuid/kgid through the namespace's map in `chown_common` → `make_kuid()`, which
returns `INVALID_UID` for an unmapped value, and the syscall rejects it as an invalid
*argument*, not as a permission failure. D5's observation and its consequence for D3's tar
extraction are both right, and the error-message interception D5 proposes is exactly the
right mitigation.

### F.5 §10.5 idmapped mounts — CONFIRMED

`mount_setattr(2)` + `MOUNT_ATTR_IDMAP` + `userns_fd` since **5.12**. The architectural
argument (extract once, idmap per container, instead of `chown -R` per user) is right and is
the strongest reason in D5 to put the floor above 5.12. The per-filesystem support figures
are marked `[F] MED` and I did not re-derive them; D3/V3 should.

---

## Part G — §12 escape classes, and seven weeks of drift (target 7)

### G.1 CVE-2019-5736 — CONFIRMED, with a mechanism correction

D5's chain is right in outcome and slightly wrong in step 2/3. The precise shape: the
attacker does **not** need a writable fd to `/proc/self/exe` from inside. The exploit is that
the *host-privileged* process (`runc init`, running with the host runc binary as its
`/proc/self/exe`) is made to `execve` a container-controlled file, and the container then
`open("/proc/<that pid>/exe", O_WRONLY)` — the write fails with `ETXTBSY` while it executes,
so the exploit holds the fd and retries after exit. The leaked privilege is "a process whose
`/proc/self/exe` is the host binary executed attacker code", not "a descriptor was passed in".
The fix D5 describes (sealed `memfd` re-exec) is correct and is what runc does.

D5's `[R]` 2 — *do not re-exec at all; a single-threaded Rust pre-exec child deletes the class*
— is the most valuable recommendation in §12 and it survives. Worth strengthening: the reason
runc must re-exec is `nsenter`'s C constructor, which exists because Go cannot `unshare(CLONE_NEWUSER)`
on a live multi-threaded runtime. D1 §11.2 reaches the same conclusion from the
async-signal-safety side. **Two independent files arriving at "single-threaded pre-exec child"
is the strongest cross-file agreement in the corpus and should be lifted into the spec as a
constraint, not a preference.**

### G.2 CVE-2022-0492 — CONFIRMED, including why cgroup v2 is immune

- **Mechanism**: `cgroup_release_agent_write()` checked only file write permission, not
  `CAP_SYS_ADMIN` **in the initial user namespace**. CONFIRMED.
- **Why no capability is needed in the initial userns**: because step 2 of the chain
  (`unshare(CLONE_NEWUSER|CLONE_NEWNS)`) manufactures a full capability set *in a new*
  namespace, and the buggy check accepted that. The capability the attacker "has" is not a
  host capability at all — this is exactly the §8.6 `old-host` observation weaponised.
- **Is cgroup v2 immune? Yes, structurally.** `release_agent` and `notify_on_release` **do
  not exist** in cgroup v2. `torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L330`, in the
  "Deprecated v1 core features" comment block: *"'release_agent' and 'notify_on_release' are
  removed. Replacement notification mechanism will be implemented."* The `release_agent_path`
  field survives only on `struct cgroup_root` for **cgroup1** mounts (`cgroup1_release_agent`
  at `#L2106`). So this is not "v2 happens not to have the bug" — **the entire upcall
  mechanism was deleted from v2 by design**, which is a genuinely good argument for D5's
  §6.3 "drop v1 entirely" beyond the engineering-cost one.
- **The mitigation matrix** (*"Containers running with AppArmor, SELinux or Seccomp are
  protected"*) is CONFIRMED and is the evidence for §9.6's belt-and-braces idea.

### G.3 CVE-2024-21626 — CONFIRMED

Advisory GHSA-xr7r-f8xq-vfvv, affected `1.0.0-rc93`–`1.1.11`, fixed **1.1.12**. The four
variants and the four-part fix are quoted accurately. The three requirements D5 derives
(`O_CLOEXEC` at creation, `close_range` before `execve`, resolve `cwd` *and* `args[0]` inside
the rootfs with `openat2(RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)`) are the right ones and are
the most directly actionable items in my scope. No correction.

### G.4 **Drift: five escape CVEs D5 does not cover, three of them landing in D5's own §11/§12 territory**

D5 was written 2026-09-04 and §12 stops at CVE-2024-21626. Checking the four upstream
advisory feeds (`opencontainers/runc`, `containers/crun`, `containerd/containerd`,
`moby/moby`) turned up the following, **all published before or during the seven-week drift
window and none mentioned in D5**. The three runc ones predate D5 by ten months, so this is a
research gap, not just drift.

| CVE | project | published | what it is | our structural defence |
|---|---|---|---|---|
| **CVE-2025-31133** | runc ≤1.2.7/1.3.2 | 2025-11-05, **high** | **`maskedPaths` itself is the vector.** runc masks *files* by bind-mounting **the container's own `/dev/null`** over them, and never verified the source is a real `/dev/null` inode. Replace it with a symlink and runc bind-mounts an arbitrary source — e.g. a **writable `/proc/sys/kernel/core_pattern`** ⇒ host root via coredump upcall, or `/proc/sysrq-trigger` ⇒ host panic. Exploitable from a plain `docker buildx build` via shared mounts between parallel containers. | **`fstat` the mask source fd and require `S_ISCHR` with `st_rdev == makedev(1,3)`; mount from the *fd*, never from a rootfs path.** Better: mask files with a bind of a `/dev/null` we opened **before** `pivot_root`, from the host's devtmpfs, held as an fd. |
| **CVE-2025-52881** | runc ≤1.2.7/1.3.2 | 2025-11-05, **high** | **All of runc's writes to `/proc` can be redirected** by a racing container with shared mounts (symlink in a tmpfs). Generalises CVE-2019-19921 past its "is it a procfs file" fix by redirecting to a *real but harmless* procfs file (`/proc/self/sched`) — so LSM labels silently do not apply. Worse, the sysctl writer becomes a semi-arbitrary write gadget: redirect a sysctl write to `/proc/sys/kernel/core_pattern` ⇒ host escape; even the literal string `docker-default` written to `/proc/sysrq-trigger` crashes the host. | **Every write under `/proc` must go to an fd opened with `openat2(RESOLVE_NO_SYMLINKS\|RESOLVE_NO_MAGICLINKS)` and verified with `fstatfs` == `PROC_SUPER_MAGIC` *and* `fstat` against the expected inode**, then `write(fd, …)` — never `write` by path. This is a *stronger* rule than D5 §12.5's, which covers rootfs paths but not `/proc` paths. |
| **CVE-2025-52565** | runc ≤1.2.7/1.3.2 | 2025-11-05, **high** | Same class, different target: the `/dev/pts/$n` → `/dev/console` bind is done **before** `maskedPaths` and `readonlyPaths` are applied, and the destination is not verified, so the attacker gets a writable `/proc/sysrq-trigger` or `core_pattern`. **The root cause is an ordering bug.** | **Apply `maskedPaths`/`readonlyPaths` before any bind whose destination comes from the rootfs**, and verify every bind destination by fd. This is an ordering constraint D1 §10.1's sequence table does not currently encode. |
| **CVE-2026-88264 / CVE-2026-88265** | crun ≤1.25 | **2026-09-24** (inside the drift window) | Rootfs-controlled `/dev/console` and `/dev/null` **symlinks** followed by crun during device setup and stdio reopening ⇒ root-owned file creation outside the rootfs / taking ownership of a host file bind-mounted in. Both advisories note the mitigating condition: *"Default configurations … mount a fresh tmpfs on `/dev`, so they are not affected."* | **Always mount a fresh `tmpfs` on `/dev` before creating any device node or opening any `/dev/*` path.** D1 §6.2 already requires this; this pair of CVEs is the evidence that it is a *security* requirement, not a tidiness one. |
| **CVE-2026-41567 / CVE-2026-42306** | moby ≤28.x | 2026-05-18, **high** | `PUT /containers/{id}/archive` executes a container binary on the host; and a race in `docker cp` redirects a bind mount to a host path. **This is CVE-2019-14271's class recurring seven years later.** | D5 §12.3's rule already covers it: *implement `cp` as an fd-based operation from outside the container, never by entering the container's filesystem view.* The recurrence is the argument for making that a hard architectural rule rather than a preference. |

(Also in-window but out of my scope: containerd CVE-2026-95837 (2026-09-01, **critical**,
CRI checkpoint-restore bypasses the destination security context) and two containerd DoS
advisories from 2026-10-13. Checkpoint/restore is not in our design; the DoS pair is a D3/V3
image-parsing concern.)

**The single structural conclusion, which D5 §12 does not draw and should:** every one of
CVE-2025-31133, CVE-2025-52881, CVE-2025-52565, CVE-2026-41579 and the two crun 2026 CVEs is
the **same bug**, and it is not D5 §12.5's bug. D5 §12.5 says "never build a path by string
concatenation of rootfs + container path". These CVEs are about the runtime using a
**rootfs-controlled path as the *source* or *destination* of its own mounts and writes** —
`/dev/null`, `/dev/pts/N`, `/proc/self/attr/*`, `/proc/sys/*`. The generalised rule is one
line longer than D5's and covers both:

> **The runtime never names a path that the container can influence, for any operation,
> including its own `/proc` writes and its own masking mounts. Every such operation goes
> through an fd obtained before the container could interfere, or through
> `openat2(RESOLVE_IN_ROOT|RESOLVE_NO_SYMLINKS|RESOLVE_NO_MAGICLINKS)`, and is verified by
> `fstat`/`fstatfs` against the expected object before use.**

That rule, plus "mask before you bind", plus "fresh tmpfs on `/dev` first", is what a
from-scratch engine gets for free and what runc and crun have been retrofitting for six years.

---

## Part H — §11 `/proc` masking policy, §14 `--privileged`, §15 devices (target 8)

*(V1 owns the `/proc` **mount mechanics**; this is the masking **policy** side only.)*

### H.1 The masked and readonly lists — CONFIRMED against moby and runc

`moby@v28.3.1/oci/defaults.go#defaultLinuxMaskedPaths()` matches D5's list exactly:
`/proc/asound`, `/proc/acpi`, `/proc/interrupts`, `/proc/kcore`, `/proc/keys`,
`/proc/latency_stats`, `/proc/timer_list`, `/proc/timer_stats`, `/proc/sched_debug`,
`/proc/scsi`, `/sys/firmware`, `/sys/devices/virtual/powercap`, plus
`/sys/devices/system/cpu/cpu<N>/thermal_throttle` for each **possible** CPU that exists.
`ReadonlyPaths` = `/proc/bus`, `/proc/fs`, `/proc/irq`, `/proc/sys`, `/proc/sysrq-trigger`.
The two advisory links D5 attributes are in the source as comments
(`GHSA-6fw5-f8r9-fgfm` for `/proc/interrupts` + thermal_throttle, `GHSA-jq35-85cj-fj4p` for
powercap). CONFIRMED.

**Divergence D5 does not note:** `runc@v1.2.5/libcontainer/specconv/example.go#L104-L122`
masks only **10** paths — it has **no `/proc/interrupts`, no powercap, no thermal_throttle**.
So "the runtime-spec/runc default" and "the Docker default" differ, and the three Docker adds
are exactly the side-channel ones. If we emit a `runc spec`-compatible default we are
side-channel-exposed; if we emit Docker's we are not. **Take Docker's.**

### H.2 `maskedPaths` is now itself an escape vector — see G.4

The single most important update to §11.2: **runc CVE-2025-31133 turned the masking
mechanism into the escape.** D5 describes file-masking as *"`mount --bind /dev/null <path>`
— chosen over deleting because programs that `stat()` the path do not break"* and presents it
as unambiguously safer. It is safer *only if the source really is `/dev/null`*. Our
implementation must hold an fd to a verified character device `1:3` obtained before
`pivot_root` and bind from `/proc/self/fd/<n>`, never from the rootfs path.

Also from CVE-2025-52565: **masking must be applied before any bind whose destination is a
rootfs path** — in runc the `/dev/console` bind ran first and could be redirected onto a
to-be-masked path, which then never got masked.

### H.3 `hidepid` / `subset=pid` — CORRECTED (an omission)

D5 §11.4 covers `hidepid=` and concludes "low value for us inside a private PID namespace".
That conclusion is right but the section **omits `subset=pid`** (procfs mount option, Linux
**5.8**), which is the one that would actually change our masking story: `mount -t proc -o
subset=pid` hides **all non-process files** in `/proc` — `/proc/kcore`, `/proc/keys`,
`/proc/sched_debug`, `/proc/timer_list`, `/proc/sys`, the lot — in a single mount option,
with no bind mounts at all.

That is strictly stronger than the 12-path masked list and it is *one flag*. The tradeoff is
real and is why nobody defaults it: `/proc/meminfo`, `/proc/cpuinfo`, `/proc/stat`,
`/proc/loadavg`, `/proc/mounts`, `/proc/self/mountinfo` and `/proc/sys` all disappear, which
breaks essentially every runtime's container-awareness and every `ps`/`top`/`free`. **But it
is exactly right for a hardened profile**, and it is the kind of thing a from-scratch engine
can offer as `--proc=minimal` where Docker cannot retrofit it. D5 should have evaluated it;
it belongs next to `hidepid` in §11.4. Verdict: **CORRECTED** (omission, not an error).

### H.4 `--privileged`, precisely — CONFIRMED, with the vector identified

D5's eleven-row table is accurate. The `[R]` that `--privileged` is "a root shell on the host
with extra steps" is correct and the `core_pattern` demonstration is the real mechanism.

**Which of the eleven is *the* escape vector?** Not the capability set on its own, and not
seccomp-unconfined on its own. It is the **conjunction of `CAP_SYS_ADMIN` with a writable
`/proc/sys`** (i.e. the `readonlyPaths` row going empty). Evidence that this is the right
identification, independent of D5's reasoning: **three of the five 2025–2026 runc/crun CVEs in
G.4 are escapes by *non-privileged* containers, and every one of them works by obtaining a
writable `/proc/sys/kernel/core_pattern` or `/proc/sysrq-trigger`.** The attackers are not
after capabilities; they are after that one file. So:

- `--privileged` hands over the vector directly.
- The masking/readonly policy is therefore not "hardening", it is **the control** — it is the
  thing the CVE authors spend their effort defeating.
- Corollary for our design: `readonlyPaths` on `/proc/sys` deserves a second, independent
  enforcement. The cheapest is **`CAP_SYS_ADMIN` not in the bounding set** (already true by
  default) and the second cheapest is **seccomp denying `mount`/`umount2`/`move_mount`/
  `fsmount`/`open_tree`/`mount_setattr` without `CAP_SYS_ADMIN`** (already in Docker's
  profile, E.7). Both are already our defaults; the point is to **document them as the
  enforcement of a single property — "the container cannot obtain a writable `/proc/sys`" —
  rather than as three unrelated hardening items.**

D5's `[R]` 2 (warn on every use, naming what was disabled, and suggest the narrower flag) is
right and I would add: **print the list of masked paths that were un-masked**, because that is
the row users never think about.

### H.5 §15.1's `[F] LOW` `/dev/full` puzzle — **SETTLED**, and the answer is bigger than the question

D5 could not explain why `/dev/full` (1:7) works when Docker's device-cgroup list does not
mention it, and guessed *"possibly the 1:7 rule comes from runc's own default set"*. That
guess is **correct**, and the file it comes from contains something far more important.

`runc@v1.2.5/libcontainer/specconv/spec_linux.go#L185-L201`, the comment above
`AllowedDevices`, verbatim:

> *This behaviour is at the very least "questionable" (if not outright wrong) according to the
> runtime-spec. Yes, we have to include certain devices other than the ones the user
> specifies, but several devices listed here are not part of the spec (**including "mknod for
> any device"?!**). In addition, these rules are **appended to the user-provided set which
> means that users cannot disable this behaviour**. … unfortunately I'm too scared to change
> this now because who knows how many people depend on this (incorrect and arguably insecure)
> behaviour.*

And the list itself:

```
c *:*    m     ALLOW     <-- mknod for ANY char device
b *:*    m     ALLOW     <-- mknod for ANY block device
c 1:3    rwm   ALLOW     /dev/null
c 1:8    rwm   ALLOW     /dev/random
c 1:7    rwm   ALLOW     /dev/full     <-- answers D5's question
c 5:0    rwm   ALLOW     /dev/tty
c 1:5    rwm   ALLOW     /dev/zero
c 1:9    rwm   ALLOW     /dev/urandom
c 136:*  rwm   ALLOW     /dev/pts/*
c 5:2    rwm   ALLOW     /dev/ptmx
c 10:200 rwm   ALLOW     /dev/net/tun   (removed in 1.2.0-rc1, restored after regressions, PR #3468)
```

### H.6 Does the device controller gate `mknod`? — **CONFIRMED that it does not, in practice**

The brief asks me to verify "unprivileged `mknod c 0 0` succeeds" from the device-cgroup
angle. The answer from runc's source is unambiguous:

- The cgroup-v2 device controller is a **`BPF_PROG_TYPE_CGROUP_DEVICE`** program, not a
  control file (CONFIRMED — there is no `devices.*` file in v2; `bpf(BPF_PROG_ATTACH,
  BPF_CGROUP_DEVICE, …)`, landed 4.15). It receives
  `struct bpf_cgroup_dev_ctx { access_type = (access << 16) | dev_type, major, minor }`
  (`torvalds/linux@v6.17/kernel/bpf/cgroup.c#L1737-L1755`) and the `access` bits include a
  distinct **`m` (mknod)** bit alongside `r` and `w`.
- runc's unconditional `c *:* m ALLOW` / `b *:* m ALLOW` rules mean the program **returns 1
  for every `mknod`**, of any major/minor, char or block. So inside a default Docker
  container, `mknod /tmp/x c 0 0` (or `b 253 0`) is permitted by the device controller and
  succeeds, gated only by `CAP_MKNOD` (in Docker's default 14) and by `MS_NODEV` on the
  target filesystem.
- **And it cannot be turned off** — the rules are appended *after* the user's, so
  `--device-cgroup-rule 'c *:* m'` with `Allow: false` does not remove them.

This refines D5 §15.3's `[R]`: *"The device cgroup blocks this today — so it takes two
failures"*. Precisely: the device cgroup **does not block the `mknod`**; it blocks the
subsequent `open()` for read/write, because `b *:*` is allowed only for `m`, not `r`/`w`. So
the two failures are (1) the `open`, blocked by the BPF program, and (2) `MS_NODEV`, which
blocks the `open` independently. D5's conclusion holds; the step it names is wrong.

**Design consequence for us, and it is a real compatibility fork:** if we implement the device
program from the OCI spec faithfully — deny-all, then the spec's default device list — then
`mknod` of an arbitrary device is **denied**, and we diverge from Docker/runc behaviour that
"people depend on". Three options, and we should pick deliberately rather than by accident:
(a) copy runc's `c *:* m` / `b *:* m` for compatibility and rely on `CAP_MKNOD` + `nodev`;
(b) deny `mknod` by default and provide `--device-cgroup-rule` to re-allow;
(c) deny `mknod` *and* drop `CAP_MKNOD` from the default set (which D5 §8.3 already
recommends), which makes the question moot. **(c) is the coherent choice** — it is the one
place where D5's §8.3 recommendation and its §15 analysis actually reinforce each other, and
D5 does not connect them.

### H.7 Two `cgroup_device` programs per scope, intersected — **CONFIRMED and broader than D5 says**

`[O] host-a` reports two `cgroup_device` programs attached to each Docker scope with
`BPF_F_ALLOW_MULTI` — runc's (unnamed, tag `ab4bc4523b7fe6b4`, identical across all five
containers) and systemd's (`sd_devices`). The AND semantics are CONFIRMED from the kernel:

`bpf_prog_run_array_cg()` (`torvalds/linux@v6.17/kernel/bpf/cgroup.c#L60-L93`) iterates the
**whole effective array** and does `if (!func_ret && ...) run_ctx.retval = -EPERM;` — so a
single `0` from any program denies, and there is no short-circuit. Access is the
**intersection**.

**Broader than D5 states:** the array is `cgrp->bpf.effective[atype]`, which under
`BPF_F_ALLOW_MULTI` includes programs attached at **every ancestor cgroup**, not only the two
at the leaf. So on a host where an admin has attached a `cgroup_device` program at
`system.slice` or at the root, our policy is intersected with that too, invisibly.

The design consequence D5 draws is right and is the strongest argument in §2.3: **we cannot
replace systemd's program, only add to it**, so a container under a systemd-managed scope can
have a device *denied* for a reason our own rules do not explain. A cgroup under our own
top-level directory has no systemd program above it. One addition D5 does not make: our
`info`/diagnostics should **enumerate the attached `cgroup_device` programs on the container's
cgroup and its ancestors** (readable via `bpf(BPF_PROG_QUERY)`), because "your `--device` was
accepted and the device still does not work" is otherwise undiagnosable.

Also worth recording from `runc@v1.2.5/libcontainer/cgroups/systemd/v2.go#L212-L216`: runc's
own source flags this as *"of questionable correctness because we insert our own devices eBPF
program later … it's unclear if systemd removes all eBPF programs attached when doing
SetUnitProperties"*. So this is a known-uncomfortable area upstream, not a settled design.

### H.8 `mknod` vs bind-mount, and `MS_NODEV` — CONFIRMED

The recommendation to **bind-mount device nodes unconditionally** rather than branching on
`CAP_MKNOD` is right and is strengthened by G.4: the crun 2026 CVEs are both in the
device-*creation* path. Bind-mounting from a host fd obtained before `pivot_root` avoids the
`/dev`-symlink class entirely. D5's `nodev`/`nosuid`/`noexec` matrix matches the live
observations and the recommendation (`nodev` everywhere except `/dev` and `/dev/pts`,
`noexec` on `/dev` with an opt-out) is sound.

One addition: both crun advisories name the mitigating condition explicitly — *"Default
configurations generated by Docker, Podman, containerd, CRI-O, and `crun spec` mount a fresh
`tmpfs` on `/dev`, so they are not affected."* **"Fresh tmpfs on `/dev` before any device
setup" is therefore a security requirement with CVE evidence**, not a convention. It belongs
in §15 as `[F]`, not only in D1 §6.2 as a mount-list entry.

---

## Part I — §6 detection and §7 the kernel floor (target 9)

*(V1 owns the D1 §2 namespace-availability cross-check; V4 owns the overlayfs-in-userns
three-way dispute. Neither is re-litigated here. What follows is whether §7's table is
internally consistent and what single number falls out of it.)*

### I.1 Detection — CONFIRMED

Filesystem magics verified against `torvalds/linux@v6.17/include/uapi/linux/magic.h`:
`CGROUP2_SUPER_MAGIC = 0x63677270`, `TMPFS_MAGIC = 0x01021994`,
`CGROUP_SUPER_MAGIC = 0x27e0eb`. D5's table is correct. The algorithm matches runc's
`IsCgroup2UnifiedMode()` / `IsCgroup2HybridMode()`
(`runc@v1.2.5/libcontainer/cgroups/utils.go#L33-L67`), which `statfs("/sys/fs/cgroup")` and
`statfs("/sys/fs/cgroup/unified")` respectively and compare `st.Type` to
`unix.CGROUP2_SUPER_MAGIC`. CONFIRMED, and worth noting runc treats a `statfs` **failure**
specially: `os.IsNotExist(err) && RunningInUserNS()` is downgraded to a debug log, i.e. runc
explicitly expects `/sys/fs/cgroup` to be absent in a rootless context. Our probe should do
the same rather than treating absence as an error.

**D5's `[R]` "do NOT detect by parsing `/proc/self/cgroup`" is correct and the `0::/` trap is
real.** The `0::` line is the unified-hierarchy line and the kernel emits it whenever
`CONFIG_CGROUP` is on, mounted or not. One addition: the same `0::` line is the **right**
source for *which cgroup we are in* once you have established by `statfs` that v2 is mounted
— so the rule is "never use it for *detection*, always use it for *location*", which is a
sharper statement than "do not parse it".

**The no-writable-cgroup case is handled correctly** by §6.3's tier table, with one gap named
in C.4: the table has "no writable cgroup at all" but not "writable cgroup, controller absent
from the parent's `subtree_control`", which is the *common* rootless case on a stock systemd
host (verified live: `io` and `cpuset` missing from `user.slice`'s `subtree_control` on
`host-a`). Those need different messages — one is unfixable, the other is a one-line
admin fix.

### I.2 The feature-floor table, row by row on the rows that gate the design

| feature | D5 says | verified | verdict |
|---|---|---|---|
| `cgroup.kill` | 5.14 | **5.14** (doc absent v5.13, present v5.14) | CONFIRMED |
| `cgroup.freeze` | 5.2 (§7.1, §5.3) / **4.19 (§5.2 code block)** | **5.2** (doc absent v5.1, present v5.2) | **CORRECTED** — internal contradiction, 4.19 is wrong |
| `clone3` | 5.3 | 5.3 | CONFIRMED |
| `CLONE_INTO_CGROUP` | 5.7 | 5.7 | CONFIRMED |
| `pidfd_open` | 5.3 | 5.3 | CONFIRMED |
| `pidfd_send_signal` | 5.1 | 5.1 | CONFIRMED |
| `CLONE_PIDFD` | 5.2 | 5.2 | CONFIRMED |
| `close_range` | 5.9 | 5.9 (`CLOSE_RANGE_CLOEXEC` 5.11) | CONFIRMED |
| PSI | 4.20 | 4.20 | CONFIRMED |
| `openat2` | 5.6 | 5.6 | CONFIRMED |
| `mount_setattr` + `MOUNT_ATTR_IDMAP` | 5.12 | 5.12 | CONFIRMED |
| unprivileged overlayfs + `userxattr` | 5.11 | 5.11 upstream | CONFIRMED (V4 owns the RHEL-backport dispute) |
| `BPF_PROG_TYPE_CGROUP_DEVICE` | 4.15 | **4.15** (absent in `uapi/linux/bpf.h` at v4.14, present at v4.15) | CONFIRMED |
| `memory.oom.group` | 4.19 (§7.1) / **5.10 (§3.1)** | **4.19** (doc present at v4.19) | **CORRECTED** — internal contradiction |
| `memory.events.local` | **5.13 `[F] MED`** | **5.3** — doc absent v5.2 / present v5.3, and `memory_events_local_show()` first appears in `mm/memcontrol.c` at v5.3 | **CORRECTED** — ten releases early |
| `cpuset.cpus.partition` | **5.17 `[F] LOW`** | **5.0** (doc present at v5.0; `isolated` type and the extra states arrive at 6.2) | **CORRECTED** |
| `memory.peak`, `memory.reclaim` | 5.19 `[F] MED` | **5.19** (doc absent v5.18, present v5.19) | CONFIRMED, upgrade to HIGH |
| `cgroup.pressure` | 6.1 `[F] MED` | **6.1** (doc absent v6.0, present v6.1) | CONFIRMED, upgrade to HIGH |
| `pids.peak` | 6.1 `[F] MED` | **6.1** — present in `kernel/cgroup/pids.c` at v6.1 (the *documentation* only catches up at v6.11, which is why this looked uncertain) | CONFIRMED, upgrade to HIGH |
| `cpu.max.burst` | 5.14 | 5.14 | CONFIRMED |
| `memory_recursiveprot` | 5.7 | **5.7** (doc absent v5.6, present v5.7) | CONFIRMED |

### I.3 Is the table internally consistent, and what is the single minimum number?

**After the three corrections, yes — and the corrections *lower* two of the justifications
without lowering the floor.**

- D5 §7.3 lists `memory.events.local` among what **5.15 adds over 5.10**. It does not:
  it is 5.3, so a 5.10 kernel already has it. One fewer reason for 5.15.
- D5 §3.1's "`memory.oom.group` (5.10+)" would have made `memory.oom.group` a 5.10-floor
  feature. It is 4.19. One fewer reason for 5.10.
- `cpuset.cpus.partition` at 5.0 instead of 5.17 does not move anything, because D5 says
  "don't care" about it.

**What actually survives as the argument for each floor:**

- **5.10 (hard floor):** `openat2` (5.6) — removes ~300 lines of `filepath-securejoin`;
  `close_range` (5.9); `CLONE_PIDFD` (5.2) and `cgroup.freeze` (5.2); `CLONE_INTO_CGROUP`
  (5.7); `CAP_BPF`/`CAP_PERFMON` split (5.8). Plus `MS_NOSYMFOLLOW` (5.10) which D5 mentions
  only in §12.5.
- **5.15 (documented floor):** adds exactly **two** things that change architecture rather
  than the feature list — **`cgroup.kill` (5.14)** and **idmapped mounts (5.12)** — plus
  unprivileged overlayfs + `userxattr` (5.11) and `cpu.max.burst` (5.14). D5's framing of
  those two as the architecture-changing pair is **correct and survives verification.**
- **6.1:** telemetry only (`memory.peak`/`memory.reclaim` 5.19, `cgroup.pressure`/`pids.peak`
  6.1).

**The single resulting minimum number is 5.15**, and D5 §0 item 7 ("Kernel floor: 5.15
supported, 6.x for development, not RHEL 8") is the statement to carry forward. §7.3's
"hard floor 5.10" is a *second*, weaker tier and should be labelled "untested, unsupported"
rather than "hard floor" — the phrase invites someone to treat 5.10 as the target.

**One addition that strengthens the 6.14 side of the argument** (A.2): `cgroup.kill`'s
fork-race handling was rewritten from a `CGRP_KILL` flag to a `kill_seq` counter in **6.14**
to fix a race with `CLONE_INTO_CGROUP`. We want both features. On 5.15–6.13 they interact
racily. That is not a reason to raise the floor, but it is a reason our `kill_all()` must
re-check `cgroup.events:populated` and re-issue `cgroup.kill` rather than assuming one write
is sufficient — which A.5's corrected loop already does.

### I.4 §7.2's "never version-sniff, probe" — CONFIRMED and it is the most valuable `[R]` in §7

The `old-host` evidence (4.18 with `openat2`, `faccessat2`, `pidfd_send_signal`,
`cap_last_cap=40` and overlayfs-in-userns, but no `clone3`, no `pidfd_open`, no PSI) is
exactly the counterexample that kills version-gating, and the probe recipe is right.
Two additions to the probe list:

- **Landlock** probes with `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`,
  which returns the **ABI version number** — a model worth copying: the kernel hands you a
  capability level, not a version string. `ENOSYS` means unsupported, `EOPNOTSUPP` means
  compiled in but disabled at boot. See J.3.
- **cgroup delegation set** should be read from `/sys/kernel/cgroup/delegate` and
  **available mount options** from `/sys/kernel/cgroup/features` rather than hard-coded
  (C.3). Both files are themselves the kernel's own probe interface for this subsystem and
  D5 §7.2's probe list does not mention either.

---

## Part J — §13 LSMs and Landlock, §16 isolation taxonomy, §17–§18 breadth (target 10)

### J.1 §13.3's `[F] MED` on `no_new_privs` and LSM transitions — **SETTLED from source; no VM test needed**

D5 marks this `MED` and proposes settling it by experiment. It is settled by reading
`torvalds/linux@v6.17/security/apparmor/domain.c#L978-L992`:

```c
/* Policy has specified a domain transitions. If no_new_privs and
 * confined ensure the transition is to confinement that is subset
 * of the confinement when the task entered no new privs.
 *
 * NOTE: Domain transitions from unconfined and to stacked
 * subsets are allowed even when no_new_privs is set because this
 * always results in a further reduction of permissions.
 */
if ((bprm->unsafe & LSM_UNSAFE_NO_NEW_PRIVS) &&
    !unconfined(label) &&
    !aa_label_is_unconfined_subset(new, ctx->nnp)) {
        error = -EPERM;
        info = "no new privs";
```

D5's reading is **correct**: a transition that *reduces* confinement is permitted under
`no_new_privs`. Specifically, `!unconfined(label)` short-circuits the check entirely when the
task was **unconfined** at the moment `no_new_privs` was set — which is our case, since our
daemon and its pre-exec child will be unconfined. Upgrade `[F] MED` → **CONFIRMED**, and
remove the open question from §21.

**The non-obvious corollary D5 does not draw, and it is a deployment hazard:** `ctx->nnp`
records the label *at the time `no_new_privs` was detected*. If our daemon is itself confined
— e.g. shipped with a systemd unit carrying an AppArmor profile, which is increasingly normal
on Ubuntu — then our container init is confined at `no_new_privs` time, and the transition to
the container profile is permitted **only if it is a subset of the daemon's profile**. A
daemon profile that is narrower in any dimension than the container profile makes the
container transition fail with `EPERM` and the audit reason `"no new privs"`. **If we ship an
AppArmor profile for our own daemon, it must be a strict superset of the container profile,
or we must apply the container profile before setting `no_new_privs`** (which is what runc
does — §8.7 steps 6 and 9).

### J.2 §13.1/§13.2 AppArmor and SELinux — CONFIRMED, no corrections

The `docker-default` deny list, the four things it buys that caps+seccomp cannot, SELinux
type enforcement + MCS, and the `:z`/`:Z` semantics all check out. The `[O] host-a MED`
note (profile not on disk in `/etc/apparmor.d/`, generated from a Go template at daemon
start) is correct — moby ships `profiles/apparmor/template.go` and loads it via
`apparmor_parser` without writing a permanent file. Upgrade to HIGH.

D5 §13.1's point 1 is the one worth carrying: **`deny mount` is path-and-operation based,
which seccomp structurally cannot express** (§9.1, no pointer dereference). It is the control
that independently defeated CVE-2022-0492, and — per G.4 — it is also what would have
contained CVE-2025-31133 and CVE-2025-52565, both of which are runc being tricked into
mounting. The staged plan (stage 0 now: two `Option<String>` fields and two writes to
`/proc/self/attr/*`) is correct and cheap.

### J.3 **Landlock — D5 omits it entirely, and it belongs in our design where the other two do not**

§13 is titled "LSMs: AppArmor and SELinux" and Landlock is absent from the whole document.
That is a gap, because Landlock has a property neither of the other two has and that matches
our stated trajectory (root-first, **rootless-later**).

Verified facts, `Documentation/userspace-api/landlock.rst` (`torvalds/linux@v6.17`) and
`landlock_restrict_self(2)`:

- **Since Linux 5.13.** It is a **stackable** LSM: it composes with AppArmor/SELinux rather
  than competing for the single "major LSM" slot, so enabling it never conflicts with the
  host's existing policy.
- **It needs no privilege and no host configuration.** `landlock_restrict_self(2)` fails with
  `EPERM` only if *"the calling thread is not running with `no_new_privs`, **or** it doesn't
  have `CAP_SYS_ADMIN` in its user namespace"*. We set `no_new_privs` by default (§8.4), so
  the precondition is already met. **No profile has to be loaded by root into the kernel
  (unlike AppArmor) and no policy package has to be installed (unlike SELinux).**
- **It is self-restriction and it is inherited.** A domain applies to the calling thread and
  all its future children, and can only ever be narrowed (max 64 composed rulesets, `E2BIG`
  beyond that). That is precisely the shape of "our container init restricts itself, then
  `execve`s the workload".
- **Capability levels are probed, not version-sniffed:**
  `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` returns the ABI number
  (`ENOSYS` = unsupported, `EOPNOTSUPP` = disabled at boot). The documented ABI progression:
  v1 filesystem access rights; v2 `LANDLOCK_ACCESS_FS_REFER` (rename/link across
  directories); v3 `FS_TRUNCATE`; v4 `NET_BIND_TCP`/`NET_CONNECT_TCP`; v5 `FS_IOCTL_DEV`;
  v6 `LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET` and `LANDLOCK_SCOPE_SIGNAL`; v7 audit-logging
  control.

**Where it fits, honestly:**

| | AppArmor | SELinux | Landlock |
|---|---|---|---|
| needs root to install policy | **yes** (`apparmor_parser`, `CAP_MAC_ADMIN`) | **yes** (policy package) | **no** |
| works rootless | **no** — D5 §17's own table says "fails quietly (`unconfined`)" | no | **yes** |
| works on any distro without host config | no (Ubuntu/Debian) | no (RHEL) | **yes** |
| can deny `mount` | **yes** | yes | **no** |
| can confine by filesystem hierarchy | yes | yes (by type) | **yes** |
| cross-container ptrace/signal confinement | yes (`peer=`) | yes (MCS) | partial (v6 `SCOPE_SIGNAL`, same-domain only) |
| per-container policy without a policy compiler | no | no | **yes** — it is a `struct` and three syscalls |

So: **Landlock does not replace `docker-default`** — it cannot deny `mount`, which is the
single most valuable thing the AppArmor profile does. But it is the **only** one of the three
that a rootless engine can use at all, and the only one that needs nothing installed on the
host. §17's table currently has a row reading *"AppArmor profile transition — fails for
unprivileged users — quietly (`unconfined`)"* with no alternative offered; Landlock is the
alternative, and it closes the one *quiet* failure in that table.

`[R]` for the spec: add a third `Option` beside `apparmor_profile` and `selinux_label` —
a Landlock ruleset derived from the container's mount list (allow the rootfs hierarchy, deny
everything else), probed by ABI version, applied immediately before `no_new_privs` + seccomp.
It is three syscalls and no dependency. Cost of deferring it is low (it is a leaf, not a
seam), so this is a **`[R] MED`, not urgent** — but its *absence from §13* should be recorded
as a gap rather than a decision.

### J.4 §16 — the distinction is drawn on mechanism, but the table contradicts §12.6 and §16.3

The conceptual claim — *"process isolation is a subset of what you need and gives no security
guarantee whatsoever"* — is correct, is drawn on mechanism rather than vibes, and the §16.2
worked example is a good one. It is also now **independently validated**: three of the five
2025–2026 CVEs in G.4 are escapes by containers that were fully namespace-isolated, and every
one of them terminates at `/proc/sys/kernel/core_pattern` — the exact file §16.2 names.

**But the §16.1 assignment of mechanisms to the two columns contradicts D5's own later
sections**, three times:

- **PID namespace** is in the process-isolation column. §12.6 argues at length that *"a
  private PID namespace is a security control and not just a cosmetic one"* because it is what
  keeps `/proc/<host-pid>/root` out of reach; and §16.3 #8 rates `--pid=host` as
  "equivalent to `--privileged` in effect".
- **Network namespace** is in the process-isolation column. §16.3 #9 says sharing it reaches
  the engine's own API socket on `127.0.0.1` and the cloud metadata endpoint — a
  confidentiality and privilege failure, not a mistake-avoidance one.
- **Mount namespace** is in the process-isolation column, yet every masking control in §11 is
  implemented *as mounts inside it*, and §16.3 #2 (`MS_PRIVATE`) is a security item.
- **cgroups** appear only as "cgroup accounting" under process isolation, yet §16.3 #12 is a
  denial-of-service item, and availability is a security property.

The fix is one sentence and it makes the section stronger rather than weaker:

> **Namespaces are *necessary but not sufficient*. Each namespace is a precondition for one
> or more security controls (mount ns → masking and read-only binds; PID ns → `/proc` magic
> links are unreachable; net ns → host-local services are unreachable), and none of them is a
> control by itself. The security controls are the ones that still hold when the container is
> actively attacking: user namespace, bounding set, seccomp, LSM, `/proc`-`/sys` policy,
> device program, no leaked fds, and resource limits.**

Verdict: **DISPUTED** — the conclusion is right, the taxonomy as tabulated is not, and a
future spec that copies the table will mis-rank `--pid=host` and `--net=host`.

§16.3's twenty-row shortcuts table: I checked the rows that fall in my scope (#3, #4, #5, #6,
#7, #11, #12, #13, #16, #17, #18, #19, #20) and found no errors. The closing `[R]` ("five
lines of code cover the five cheapest items") is accurate and is the best summary sentence in
the document.

### J.5 §17–§18 breadth — nothing moved, two notes

*(V5 is adjudicating the networking half of §17–§18; the rows below are the resource/security
half only, which is mine.)*

§17's degradation table is internally consistent with §2.2, §10.2, §15.2 and §15.3, and
§17.1's identification of "cgroup limits without delegation" as *the* quiet failure is
correct — C.4's live `[O]` adds a second, more common instance of the same failure (controller
absent from the parent's `subtree_control` on a stock systemd host, where the file does not
exist and a naive `write` gets `ENOENT`). Two notes:

1. **The `ip_unprivileged_port_start = 0` recommendation has a scope bug.** D5 §17 observes
   Docker sets it inside the container and concludes *"we can then drop
   `CAP_NET_BIND_SERVICE` from the default set — a small, free reduction."* The sysctl is
   **per-network-namespace**, so this holds only for containers with their own netns. With
   `--net=host` the container sees the host's value and a dropped `CAP_NET_BIND_SERVICE`
   becomes a real regression. Also note §8.3's proposed 7-capability "strict" set still
   *includes* `CAP_NET_BIND_SERVICE`, so §8.3 and §17 disagree about the same capability.
   **Make the drop conditional on a private netns, and reconcile the two sections.**
2. **§18.1's six expensive retrofits** are correctly ranked and I can add primary-source
   weight to #1 and #6: #1 is forced by `capabilities(7)`'s `execve` transformation (D.1) and
   now also by the **5.12 `CAP_SETFCAP` rule** (F.1), which adds a second reason the *parent*
   must write the map; #6 is forced by `seccomp(2)`'s TSYNC semantics (E.3) *and* by
   `landlock_restrict_self(2)`, whose man page notes it propagates `no_new_privs` to sibling
   threads — i.e. a third subsystem that behaves differently in a threaded process. §18.3's
   `Privilege` seam is the right abstraction and I have no correction to it.

---

## Part K — breadth sweep of §1, §3.4–§3.5, §4.1–§4.5, and two of D5's own open experiments settled

### K.1 §1 — nothing needed correcting, and §1.7's E1 and E2 are now answered from source

§1.1, §1.2, §1.4 (`cgroup.type`, threaded cgroups), §1.5 (`cgroup.procs` vs `cgroup.threads`)
all check out against `cgroup-v2.rst`. The `[R]` to skip threaded cgroups is right (the OCI
spec has no field for it). The `[R]` that `cgroup.procs` is never authoritative for "is the
container gone" is right and is reinforced by the verbatim kernel text quoted in A.3.

**D5 §1.7 lists five experiments "for the local VM". Two of them do not need a VM:**

**E1 — the no-internal-process errno.** D5 marks it `[O] LOW` ("not verified… `LOW` on the
exact errno"). Settled from `torvalds/linux@v6.17/kernel/cgroup/cgroup.c`:

- Migrating a process **into** a cgroup whose own `subtree_control` is non-empty:
  `#L2758-L2760`, comment *"apply no-internal-process constraint"*,
  `if (dst_cgrp->subtree_control) return -EBUSY;` → **`EBUSY`**.
- Enabling a controller in `subtree_control` of a cgroup that **has tasks**:
  `#L3453-L3458`, comment *"Controllers can't be enabled for a cgroup with tasks to avoid
  child cgroups competing against tasks"*, `if (cgroup_has_tasks(cgrp)) return -EBUSY;` →
  **`EBUSY`**.

D5's prediction ("expect `EBUSY` both ways") is **correct**; upgrade from `[O] LOW` to
`[F] HIGH`. Two additions: a **third** `EBUSY` exists that D5 does not list — *disabling* a
controller that a live child has enabled (`#L3524-L3527`); and the threaded exemption D5
states in §1.3 item 3 is visible as the early return immediately above the constraint
(`#L2754-L2756`: `if (cgroup_can_be_thread_root(dst_cgrp) || cgroup_is_threaded(dst_cgrp))
return 0;`).

**E2 — `CLONE_INTO_CGROUP` constraints. — one half CONFIRMED, one half CORRECTED.**
`cgroup_css_set_fork()` (`cgroup.c#L6596-L6650`) calls **the same**
`cgroup_attach_permissions()` that the `cgroup.procs` write path calls, with the kernel's own
comment explaining the design:

> *Spawning a task directly into a cgroup works by passing a file descriptor to the target
> cgroup **directory**. This can even be an **`O_PATH`** file descriptor. But it can **never
> be a `cgroup.procs` file descriptor**. This was done on purpose so spawning into a cgroup
> could be conceptualized as an atomic `fd = openat(dfd_cgroup, "cgroup.procs", ...);
> write(fd, <child-pid>, ...)` sequence … This allows us to always use the **caller's**
> credentials.*

- D5's *"It does not let you spawn into a cgroup that itself has `subtree_control` set"* —
  **CONFIRMED**, same `EBUSY`, same code path.
- D5's *"`CLONE_INTO_CGROUP` also **relaxes** the no-internal-process rule for the target's
  parent … because the child never transits the parent"* — **CORRECTED. There is no
  relaxation.** The constraint has never applied to the destination's *parent*; it applies
  only to the destination's own `subtree_control`. Writing a pid into a leaf whose parent has
  controllers enabled is legal via the ordinary path too — that *is* the §1.3 layout. The
  imagined asymmetry would, if believed, make `CLONE_INTO_CGROUP` look like a prerequisite
  for the recommended layout. It is not. D5's `[R] MED` (optional optimisation, pipe-
  synchronised path is the only correctness-bearing one) is right, and now rests on firmer
  ground.
- Three implementation details D5 omits and that the kernel states: the fd must be a
  **directory** fd (`O_PATH` acceptable, never `cgroup.procs`); a dead target gives
  **`ENODEV`**; and the permission check uses the **caller's** credentials, not the child's —
  which matters for a rootless engine spawning into a delegated subtree.

E3, E4 and E5 remain. **E5 is answered in A.5** (freeze×`SIGKILL` settled from `signal.c` and
`freezer.c`); only E3 and E4 genuinely need a VM, and both are behaviour-observation rather
than correctness questions.

### K.2 §3.4 IO, §3.5 hugetlb/rdma/misc — nothing needed correcting

`io.max` line format, `io.weight`'s `default N` form, and the "`io.weight` is inert without
`bfq`/`io.cost`/`io.latency` while `io.max` works regardless because blk-throttle sits above
the scheduler" point all check out. The `[R]` (implement `io.max`, skip `io.weight`, warn when
the scheduler is not `bfq`) is sound. §3.5's "skip all three, but round-trip
`hugepageLimits` through the spec losslessly" is the right call.

One addition relevant to C.4: **`io` is not in the controllers systemd delegates to
`user@.service`**, and on `host-a` it is absent from `user.slice`'s `subtree_control`.
So `io.max` is permanently unavailable rootless on a stock systemd host, which should be a
named row in §17's degradation table rather than folded into "cgroup limits".

### K.3 §4.1, §4.3, §4.4, §4.5 — nothing needed correcting

- **§4.1** `memory.current` includes page cache (`cgroup-v2.rst#L1276`: *"The total amount of
  memory currently being used by the cgroup and its descendants"*); the `docker stats`
  subtraction is CONFIRMED verbatim from `docker/cli@v28.3.1/cli/command/container/stats_helpers.go#L227-L249`,
  including the cgroup-v1 (`total_inactive_file`) vs cgroup-v2 (`inactive_file`) split and the
  comment that Docker ≤19.03 used `cache` instead. The `[R]` to report both numbers is right.
  `memory.peak` resettability from 6.8 — CONFIRMED from the doc's *"or the most recent reset
  for that FD"* wording, which is present at v6.17 and absent at 5.19; D5's "do not reset it,
  other tools share it" is good advice.
- **§4.3** `cpu.stat`'s throttling counters only populate when `cpu.max` is set — consistent
  with the live `[O]` and with the kernel only accounting CFS bandwidth when a quota exists.
  The `[R]` to expose `throttled_usec / (nr_periods × period)` as first-class is the single
  best telemetry recommendation in the document.
- **§4.4** PSI at 4.20, two lines (`some`/`full`), `cgroup.pressure` at 6.1 (CONFIRMED, I.2).
  The `old-host` absence of `/proc/pressure/*` on RHEL 8 is consistent with `CONFIG_PSI` not
  being set there. `[R]` gate on file existence, never depend on it — correct.
- **§4.5** `cgroup.events:populated` as the race-free liveness predicate, `cgroup.stat`'s
  `nr_dying_descendants` as the explanation for `rmdir` → `EBUSY`, `pids.peak` (6.1, I.2),
  `memory.reclaim` (5.19, I.2). All CONFIRMED. The doc at v6.17 adds
  `nr_dying_subsys_<cgroup_subsys>` which is a finer-grained version of the same signal.

### K.4 Cross-file notes handed to other verifiers (not adjudicated here)

- **D1 §11.1 says "NOT … `caps` (allocates)"; D5 §19.2 says `[R] MED` "use it".** A direct
  D1↔D5 contradiction about the same crate, in §19, which is **V7's**. My §8.5 finding (D.3)
  is relevant to whichever way it is resolved: the thing the crate cannot get right is the
  *ordering*, and D5's stated ordering is wrong, so "use `caps` and own the ordering" is only
  safe once the ordering in §8.5 is fixed.
- **V5 (networking)**: §17's `ip_unprivileged_port_start = 0` observation and the proposed
  drop of `CAP_NET_BIND_SERVICE` is netns-scoped — see J.5 item 1.
- **V4 (overlayfs-in-userns)**: untouched here. §7.2's `old-host` RHEL-backport `[F] MED` is
  theirs.
- **V1 (`/proc` mount mechanics, namespace tables)**: I took only the `/proc` *masking policy*
  side (Part H) and the §7 floor's internal consistency (Part I), not D1 §2's table.

---

## Corrections that change a recommendation

1. **§8.5 capability ordering.** D5 drops the bounding set *after* `setresuid` → every
   `prctl(PR_CAPBSET_DROP)` returns `EPERM` for a non-root container user, because `capabilities(7)`
   clears the **effective** set on a euid 0→nonzero transition and `PR_SET_KEEPCAPS` preserves
   only **permitted**. → **Drop bounding first, while euid is still 0** (runc's order,
   `init_linux.go#L336`), then `KEEPCAPS`→ids→`capset`→ambient. Invalidates the §8.5 sequence
   verbatim and the §19.2 claim that `caps` + "our ordering" is sufficient.
2. **§3.2 `--cpu-shares` conversion.** "runc/systemd use `1 + ((shares-2)*9999)/262142`" —
   systemd does **not**; it uses `CLAMP(shares*100/1024, 1, 10000)` (`shares=1024 → 100` vs
   runc's `39`). runc converts in *spec conversion* and sends `CPUWeight`, so systemd's
   formula never runs for containers. → **Put the (runc) conversion above the `CgroupManager`
   trait**, never in a driver; shifts §2.3's trait boundary.
3. **§9.6 `clone` namespace mask.** The real value is `2114060288` = `0x7E020000`, which
   **includes `CLONE_NEWCGROUP`**, unchanged since Docker 20.10. D5's `0x7C020000` and its
   `[R] MED` "Docker deliberately permits unprivileged cgroup-ns creation because it is
   harmless" are both false. → Invalidates that `[R]`; reinforces D5's own "write the mask as
   a named constant expression".
4. **§9.6 `clone3`.** It **is** in Docker's profile, with an explicit
   `SCMP_ACT_ERRNO errnoRet: 38 (ENOSYS)` rule when `CAP_SYS_ADMIN` is absent — so §9.6's
   "not a single non-ALLOW rule" and "clone3 does not appear at all" are both wrong. → The
   §0-13 / §19.1 argument ("libseccomp cannot express the ENOSYS rule, **this alone decides
   it**") weakens: libseccomp *can* express a per-syscall ENOSYS; only the open-ended
   above-max rule needs the patch. Shifts, not invalidates, the `seccompiler`-over-`libseccomp`
   recommendation (V7's section).
5. **§2.4 `DefaultTasksMax`.** The rule is **15% of `min(kernel.threads-max, kernel.pid_max − 1,
   root cgroup pids.max)`**, not 15% of `threads-max`; and **user slices get 33%**, not 15%.
   The 19147 on `host-a` is a coincidence of `pid_max = 4194304`; the documented default
   elsewhere is **4915**. → **Strengthens** the `[R]` "set our own `pids.max`": the systemd
   backstop varies ~10× across hosts and is admin-configurable.
6. **§3.1 `--oom-kill-disable`.** Docker does **not** error on cgroup v2; it emits a warning
   and discards (`moby@v28.3.1/daemon/daemon_unix.go#L470-L477`). → The `[R]` to reject the
   flag stands but must stop citing Docker as precedent; it is an instance of the §6.3
   anti-pattern Docker *commits*.
7. **§4.2 OOM detection.** `oom_kill` counts kills by **"any kind of OOM killer"**, including
   the global one, so `Δoom_kill > 0` alone mislabels a host-pressure kill as "exceeded your
   memory limit". → Add `Δmax > 0 || Δoom > 0` to distinguish; one extra read of the same file.
   Changes what `ContainerState.oom_killed` may claim.
8. **§5.2 kill fallback.** `cgroup.freeze` is **5.2**, not the `4.19` in D5's own pseudocode;
   and the trailing `write("cgroup.freeze","0")` is **harmful** — a frozen task takes `SIGKILL`
   and dies (`signal.c#L2997-L3000`), so thawing only re-enables `fork()` for anything the walk
   missed. → Corrected loop in A.5.
9. **§5.2 `cgroup.kill` mechanism.** Not "the kernel holds the cgroup's lock while walking" but
   a `kill_seq` bump checked in `cgroup_post_fork` (a `CGRP_KILL` flag before **6.14**). →
   On 5.15–6.13, `cgroup.kill` racing `CLONE_INTO_CGROUP` is a known kernel hole; `kill_all()`
   must re-check `populated` and re-issue rather than assume one write suffices.
10. **§10.1 `uid_map`.** Missing the **Linux 5.12 rule**: mapping **UID 0 of the parent
    namespace** requires **`CAP_SETFCAP`** in the parent userns. → A root-first engine that
    drops `CAP_SETFCAP` from itself, or runs inside a container without it, cannot create a
    userns with a root mapping — and the errno is an indistinguishable `EPERM`. Also: the map
    must be **one `write(2)` at offset 0, under one page**, which caps a practical map near
    ~130 lines, not 340.
11. **§15.1/§15.3 the device controller does not gate `mknod`.** runc's `AllowedDevices`
    unconditionally appends `c *:* m` and `b *:* m` ALLOW, *after* the user's rules, so users
    cannot disable it (source comment: *"incorrect and arguably insecure … too scared to change
    this"*). → `mknod` of any device succeeds in a default container; the blocked step is the
    subsequent `open`. Forces a deliberate choice: copy runc's rules for compatibility, or
    deny `mknod` **and** drop `CAP_MKNOD` (§8.3 already recommends the latter — connect them).
    Also settles §15.1's `[F] LOW` `/dev/full` question: 1:7 comes from runc's list.
12. **§11.2 masking is itself an escape vector (CVE-2025-31133, CVE-2025-52565) and all `/proc`
    writes are redirectable (CVE-2025-52881)** — three high-severity runc CVEs from 2025-11-05
    that D5 does not cover. → §12.5's rule must widen from "never concatenate rootfs paths" to
    **"the runtime never names a path the container can influence, for any operation, including
    its own `/proc` writes and its own masking mounts"**, plus **"apply masking before any bind
    whose destination comes from the rootfs"** and **"fresh `tmpfs` on `/dev` before any device
    setup"** (the latter now has CVE evidence from crun CVE-2026-88264/88265, 2026-09-24).
13. **§7.1 three version errors.** `memory.events.local` is **5.3** (not 5.13),
    `cpuset.cpus.partition` is **5.0** (not 5.17), and §3.1's `memory.oom.group` "(5.10+)" is
    **4.19**. → Removes one of §7.3's stated reasons for the 5.15 floor (the floor still holds
    on `cgroup.kill` + idmapped mounts alone). The single resulting minimum is **5.15**;
    §7.3's "hard floor 5.10" should be relabelled "untested, unsupported".
14. **§2.1 the delegation file set is five, not three.** `/sys/kernel/cgroup/delegate` on 6.11
    reads `cgroup.procs cgroup.threads cgroup.subtree_control memory.oom.group memory.reclaim`.
    → **Read that file; do not hard-code the list.** It has grown twice.
15. **§6.3's tier table is missing the common rootless case.** `[O] host-a`: `user.slice`'s
    `cgroup.controllers` has `io` and `cpuset` but its `cgroup.subtree_control` does not — so
    rootless sees a **missing file**, not `EACCES`. → Add a tier "controller not enabled in the
    parent's `subtree_control`" with a different, admin-actionable message.
16. **§1.6 `CLONE_INTO_CGROUP` relaxes nothing.** The no-internal-process constraint has never
    applied to the destination's *parent*. → Removes an imagined reason to treat
    `CLONE_INTO_CGROUP` as a prerequisite for the §1.3 layout; D5's "optional optimisation" `[R]`
    is correct and now better grounded.
17. **§16.1's taxonomy table contradicts §12.6 and §16.3.** PID / mount / network namespaces are
    listed as *process* isolation while D5 itself argues each is a security control. → Restate
    as "namespaces are necessary preconditions, never sufficient"; otherwise a spec built from
    the table mis-ranks `--pid=host` and `--net=host`.
18. **§13 omits Landlock.** 5.13+, stackable, needs **no privilege and no host configuration**
    (`landlock_restrict_self(2)` requires only `no_new_privs`, which we set by default). It
    cannot deny `mount`, so it does not replace `docker-default` — but it is the **only** LSM a
    rootless engine can use, and it closes the one *quiet* failure in §17's table
    ("AppArmor transition → `unconfined`"). → Add a third `Option` beside `apparmor_profile`
    and `selinux_label`.

## Still unverifiable, and the exact test

1. **`memory.max` vs `memory.high` behaviour under real allocation pressure (D5 §1.7-E3).** →
   Local VM, root, cgroup v2: `mkdir /sys/fs/cgroup/t; echo 100M > /sys/fs/cgroup/t/memory.max;
   (echo $BASHPID > /sys/fs/cgroup/t/cgroup.procs; exec python3 -c "b=bytearray(200*1024*1024)")`
   then `cat /sys/fs/cgroup/t/memory.events`. Repeat with `memory.high=100M, memory.max=max`.
2. **`cpu.max` throttling counters (D5 §1.7-E4).** → Local VM:
   `echo "50000 100000" > /sys/fs/cgroup/t/cpu.max`, run a busy loop in it, watch
   `nr_throttled`/`throttled_usec` in `cpu.stat`.
3. **Whether `CONFIG_X86_X32_ABI` is set on our target kernels (E.4).** → `host-a`:
   `zcat /proc/config.gz | grep X86_X32` (read-only, under `/proc`), or
   `grep X86_X32 /boot/config-$(uname -r)`. Decides whether the `__X32_SYSCALL_BIT` bypass is
   live on that host or inert.
4. **The exact BPF instruction count of a Docker-equivalent filter (E.2).** → Local VM with
   libseccomp tools: build the profile with `scmp_bpf_disasm` / a 20-line Go program calling
   `libseccomp.ExportBPF`, and count `len(fprog)` for (a) native-arch-only and (b) the full
   `archMap`. My estimate is ~410 and ~1200; the point stands either way (both ≪ 4096), but the
   number should be measured before anyone writes "we must keep the filter small" in a spec.
5. **`seccomp(2)` → `EACCES` without `CAP_SYS_ADMIN` and without `no_new_privs` (D5 §9.2's
   inconclusive probe).** → Local VM, 5-line C program: `seccomp(SECCOMP_SET_MODE_FILTER, 0,
   &prog)` as an unprivileged user with and without `prctl(PR_SET_NO_NEW_PRIVS,1,0,0,0)`;
   expect `EACCES` then `0`.
6. **D5 §10.3's `chown 0:0` → `EINVAL` puzzle (F.3).** → `old-host`, no `strace` needed:
   `unshare -Ur sh -c 'cat /proc/self/uid_map /proc/self/gid_map /proc/self/setgroups'`.
   If `gid_map` is empty, the `EINVAL` is an unmapped gid 0 and the mystery is closed.
7. **Whether our own daemon's AppArmor confinement would block the container profile
   transition under `no_new_privs` (J.1 corollary).** → Any AppArmor host:
   run the daemon under a unit with `AppArmorProfile=`, then check
   `/proc/<container-pid1>/attr/current` and `journalctl -k | grep 'no new privs'`.
8. **Whether a `cgroup_device` BPF program attached at an ancestor cgroup silently narrows our
   policy (H.7).** → `host-a`, read-only:
   `bpftool cgroup tree /sys/fs/cgroup | grep -B2 cgroup_device` — enumerates every attachment
   point, not just the leaf. (`bpftool cgroup show` on the leaf, which D5 ran, does not show
   ancestors' programs.)
