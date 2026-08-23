# Plan — Rust Container Engine R&D (`RND.md`)

## Context

`/Users/mansur/D/projects/docker-projects/docker-rust` is empty. The long-term goal is a
Docker-like container engine written in Rust, owning the runtime core rather than
shelling out to runc/crun/containerd. This session is **research only**: no code, no
`SPEC.md`, no task list. The single deliverable is `RND.md` — source-grounded,
adversarially verified, and structured so a fresh specification session can orient by
loading only its priority index.

Per the project spec, this plan is the Phase 1 output: the five research domains, why
each earns its own agent, and the questions each must answer. Research starts only after
approval.

Host is macOS, so **no claim can be verified by running it locally**. Everything must be
grounded in kernel docs, OCI specs, and runtime source; empirical checks get recorded in
`RND.md` as reproducible commands for a Linux box, not as results.

Privilege decision (user): **root first, rootless later.**

---

## Environment reality — probed, not assumed

Two hosts were probed read-only. Neither is the right primary dev target, but together they
cover the interesting extremes.

### `host-a` → `HOST-A` — modern, root, **and in production**

| Property | Value |
|---|---|
| Distro / kernel | Ubuntu 24.10, **6.11.0-29** — `clone3`, `openat2`, idmapped mounts, `CLONE_INTO_CGROUP`, seccomp notify all present |
| Privilege | **root**, sudo OK |
| cgroups | **v2 unified**, `cgroup2fs`, controllers available *and* delegated: `cpuset cpu io memory hugetlb pids rdma misc` |
| Namespaces | userns enabled; no `/etc/subuid` entries (irrelevant for root mode, needed later for rootless) |
| Overlay | native `overlay`, root — no fuse-overlayfs needed; ext4, 146 G free, 15 G RAM, 4 vCPU |
| Firewall | `iptables v1.8.10 (nf_tables)` + `nft 1.1.0` — i.e. the iptables-nft translation layer, exactly the D4 question |
| LSM | **AppArmor enabled** — Docker's default profile is in play |
| On-box refs | `docker` + `containerd` + `runc` + `slirp4netns`, all active |
| **Live traffic** | `docker-proxy` listening on **:80 and :443** (v4 + v6); docker and containerd `active` |

**This host serves a production site.** Per the user's instruction, nothing here gets
touched. That is not just a courtesy — Docker owns the `docker0` bridge and the
`DOCKER`/`DOCKER-USER` nftables chains, so the networking stages of our engine are precisely
the ones that could take :80/:443 down.

Its real value is as a **read-only observation lab on a modern kernel**, which upgrades the
research from doc-only to doc-plus-evidence. Already learned from the probe alone: this
Docker install uses **both** userland `docker-proxy` processes *and* kernel DNAT, and the
distro's `iptables` is really nftables underneath — two claims D4 would otherwise have had
to take on trust.

Rules for using it, to be honored by every agent:

- **Allowed:** reads of `/proc`, `/sys`, `/etc`; `nft list ruleset`, `iptables-save -t nat`;
  read-only `docker inspect` / `ps` / `images`; inspecting containerd shim processes and
  `/sys/fs/cgroup/system.slice/docker-*` values; overlay2 on-disk layout under
  `/var/lib/docker`; `unshare`-based namespace experiments confined to a private mount
  namespace with scratch in `/tmp`.
- **Forbidden:** any `iptables`/`nft`/`ip link`/bridge mutation; starting, stopping or
  removing containers or images; `systemctl` anything; edits to `/etc` that services read
  (including adding `/etc/subuid` entries); anything binding or rerouting :80/:443.

### `old-host` → `OLD-HOST`, a Slurm HPC login node

| Property | Value | Consequence |
|---|---|---|
| Distro / kernel | Rocky Linux 8.8, **4.18.0-425** | No cgroup v2, no idmapped mounts (5.12), no time ns (5.6), `clone3` availability uncertain under RHEL backports |
| Root | **None** — `sudo` needs a password we don't have | Root-first work is impossible here |
| cgroups | **v1 legacy only** (no unified, no hybrid); our shell sits in `user.slice/…/session.scope`, no delegation | `--memory` / `--cpus` / `--pids-limit` are **untestable** on this host; Slurm owns the hierarchy |
| User namespaces | Enabled (`max_user_namespaces` ≈ 1e6); `unshare -Ur` → uid 0 | Namespace work *is* testable unprivileged |
| PID + mount ns | `unshare -Urpf --mount-proc` → PID 1 inside | Isolation stages testable |
| Network ns | `unshare -Urn` → loopback only | veth/bridge/NAT need `CAP_NET_ADMIN` in the host netns → **not testable**; slirp4netns/pasta only |
| OverlayFS | `overlay` present, and **mount inside a userns succeeds** on local ext4 (`/tmp`, 106G free) — RHEL 8 backport; `fuse-overlayfs` also installed | Layered-fs stages testable unprivileged, but **only on local disk** |
| `/etc/subuid`,`/etc/subgid` | **No entries** | Single-UID mapping only; multi-UID images (`nobody`, `www-data`) will break |
| `$HOME` | NFS (`441T` shared) | Overlay upperdir and runtime state must live on local `/tmp` or `/var/tmp`, not `$HOME` |
| SELinux | Disabled | No labeling friction here — but not representative of a real distro |
| Toolchain | No `rustc`/`cargo` | rustup into `$HOME`, with `CARGO_TARGET_DIR` on local disk |
| Reference impls on-box | `podman`, `apptainer`, `singularity`, `runc`, `fuse-overlayfs`, `newuidmap` | Free comparison targets for `strace`-level behavior study |

### Conclusion — three targets, distinct jobs

`old-host` cannot host the chosen root-first path (no root, cgroup v1, no delegation, NFS
home). `host-a` technically could, but it is production and off-limits for mutation. So:

- **Primary dev/test target — a local Linux VM on macOS (Lima or UTM):** modern kernel,
  cgroup v2, root, local disk, and *destroyable*. Every ladder success test is written
  against this, and all destructive stages — mounts, `pivot_root`, cgroup writes, and above
  all bridges/veth/NAT — happen only here. Standing up this VM is a prerequisite for the
  implementation session, not for this research session.
- **`host-a` — read-only observation lab.** Ground truth for how a modern kernel and a
  real Docker install actually behave: nftables rulesets, docker-proxy vs DNAT, overlay2
  layout, shim process trees, cgroup v2 values under load. Never a place to run our engine's
  networking.
- **`old-host` — degradation probe.** *Rootless + kernel 4.18 + cgroup v1 + no delegation +
  no subuid + NFS.* It answers "does our engine fail honestly on an old shared machine?",
  which is a stronger portability test than any VM can give.

This makes two things real research questions rather than assumptions: the **minimum
kernel floor** we target, and how the engine **detects and degrades** on cgroup v1 /
no-delegation / no-subuid hosts.

---

## The five research domains

Chosen to minimize overlap while covering every required question. Rust-crate research is
deliberately **cross-cutting** (each agent reports candidates in its own area) with D2
owning the core substrate decision — one agent cannot credibly evaluate 15 crate
categories, and crate judgement is only trustworthy when made while reading the code that
uses them.

### D1 — Kernel isolation mechanics & the container creation sequence

**Why its own agent:** This is the only domain where being wrong is fatal to the whole
project. Everything above it (architecture, images, networking) is a policy choice; the
syscall sequence is not. Namespace creation has hard ordering constraints and
non-obvious semantics (a namespace you create is not the one you join; `CLONE_NEWPID`
does not move the caller; `pivot_root` fails on shared propagation) that no amount of
later refactoring fixes.

Must answer:
- Semantics of all 8 namespaces (mnt, pid, net, uts, ipc, user, cgroup, time): what each
  virtualizes, what it explicitly does *not*, and what breaks if omitted.
- `clone`/`clone3` vs `unshare` vs `setns`: which flags apply to caller vs child, which
  require a fork to take effect, which cannot be combined, ordering constraints
  (user namespace first → unprivileged everything else), and why `CLONE_NEWPID` needs an
  extra fork.
- Mount namespace mechanics: propagation types (`MS_PRIVATE`/`MS_SLAVE`/shared), why
  runtimes recursively make `/` private, `MS_BIND`/`MS_REC`, `pivot_root` vs `chroot`
  (including the exact `pivot_root` preconditions and why `chroot` is escapable).
- The minimum viable rootfs mount set: `/proc`, `/sys`, `/dev` (tmpfs + device nodes vs
  bind), `devpts` (`newinstance`/`ptmxmode`), `/dev/shm`, `/dev/mqueue`, and which are
  required vs convention.
- Container init (PID 1) kernel semantics: default signal dispositions inside a PID
  namespace, orphan reparenting, zombie reaping, what happens when PID 1 dies, and how
  exit status reaches the outside.
- Classification of every mechanism as: **required for a minimal container** / **strongly
  recommended** / **security hardening** / **Docker convenience**.
- The corrected, ordered creation trace for `mydocker run alpine /bin/sh` — per step:
  actor, kernel API, privilege required, MVP or not, failure modes.

### D2 — Runtime architecture, reference implementations & Rust substrate

**Why its own agent:** Daemon-vs-daemonless, shim-or-no-shim, and sync-vs-async internals
are the decisions that cause rewrites. They can only be judged by understanding *why*
runc splits `create`/`start`, why containerd needs a shim at all, and why youki's Rust
implementation makes the choices it does — architecture study and crate study are the same
reading session.

Must answer:
- OCI Runtime Spec: lifecycle state machine, `create`/`start`/`kill`/`delete`/`state`,
  `config.json` structure, hooks, what the spec mandates vs leaves to implementations.
- The real chain: `docker` CLI → dockerd → containerd → containerd-shim → runc → kernel.
  What each boundary buys (daemon restarts without killing containers, reparenting,
  console/FIFO ownership, exit-status collection) and which of those reasons apply to us.
- Why runc uses a two-stage `nsexec`/`runc init` design, and how crun (C, no runtime
  threads) and youki (Rust) differ — does a Rust engine inherit Go's constraints?
- Comparison with a purpose, not a table: Docker, Podman (conmon, fork/exec), containerd,
  CRI-O, runc, crun, youki, gVisor, Firecracker/Kata — where each draws its trust and
  process boundaries.
- **Apptainer/Singularity** as a fourth architecture family, added because it is the model
  that actually runs in the user's HPC environment: no daemon, no persistent state, no
  cgroups, single SIF image file, and a deliberate "container as a process you exec, not a
  service you manage" stance. Which of its simplifications are principled and which are
  HPC-specific?
- Detached execution mechanics: double-fork, `setsid`, console socket + SCM_RIGHTS pty
  passing, attach/logs, signal forwarding, exit-code propagation without a parent.
- Persistent state: what runc keeps in `/run/runc/<id>/state.json`, what containerd keeps
  in boltdb, what `mydocker ps` after a reboot actually needs.
- Recommendation: simplest technically sound architecture for us (single static binary?
  runtime + separate lifecycle store? shim later?), and what it must not preclude.
- Rust substrate core: `nix` vs `rustix` vs raw `libc` (what each hides, maintenance
  health, Linux coverage, `unsafe` surface), `oci-spec-rs`, `caps`, `clap`, sync vs `tokio`
  internals, and the fork/exec safety rules (async-signal-safety, no allocation between
  `clone` and `execve`, thread-hostile syscalls) that constrain the design.

### D3 — OCI images, registries & layered filesystems

**Why its own agent:** Content-addressed storage layout and the image→rootfs path are
schema decisions that are painful to migrate later, and the three OCI specs
(Image/Distribution/Runtime) are routinely conflated. This agent also owns the highest
concentration of security-relevant parsing code we will write.

Must answer:
- Exactly what each spec controls, and precise definitions of: image, bundle,
  `config.json`, rootfs, manifest, index/manifest list, layer, blob, digest, registry,
  runtime.
- Full `docker pull alpine` trace: name resolution and defaults
  (`docker.io/library/alpine:latest`), `/v2/` challenge, Docker Hub token auth
  (`WWW-Authenticate` realm/service/scope), manifest fetch with `Accept` media types,
  index → platform selection → manifest → config → layer blobs, digest verification.
- Layer format: tar + gzip/zstd, `+gzip` media types, diff_id vs layer digest (compressed
  vs uncompressed hashing) and why both exist.
- Whiteouts: `.wh.<name>` and opaque `.wh..wh..opq`, and how they map onto OverlayFS's own
  char-device/xattr representation.
- Extraction hazards to design against: path traversal, symlink escape, hardlinks, device
  nodes, setuid bits, xattrs, uid/gid ownership, absolute paths, `..` entries.
- Storage: content-addressable layout (`blobs/sha256/...`), refcounting/GC, dedup across
  images, what "overlay2" actually lays out on disk, containerd's snapshotter model.
- OverlayFS mechanics: `lowerdir`/`upperdir`/`workdir`, lowerdir stacking order and limits,
  `trusted.overlay.*` xattrs, `metacopy`, `redirect_dir`, `userxattr` for rootless, and
  where fuse-overlayfs is still needed.
- Recommended first implementation and the correct progression from plain extracted
  directory → immutable layers → overlay + writable layer.
- Target OCI conformance level for MVP / intermediate / mature.

### D4 — Networking & host integration

**Why its own agent:** Networking is the domain most easily copied wrongly from tutorials,
it is the one that touches host state that outlives our process (bridges, iptables rules,
netns mounts), and its cleanup-on-crash story shapes the lifecycle design.

Must answer:
- Network namespace lifecycle: creation, nsfs, the `/var/run/netns` bind-mount convention,
  persisting a netns without a process, entering with `setns`, and cleanup.
- veth pair creation and configuration over rtnetlink (what `ip link` actually sends),
  moving one end into a netns, addressing, routes, MTU.
- Bridge model: `docker0`-style bridge, why the default is bridge+NAT and not routed,
  ARP/hairpin behavior, `net.ipv4.ip_forward`, `bridge-nf-call-iptables`.
- Real trace of `docker run -p 8080:80 nginx`: DNAT in `nat/PREROUTING` + `DOCKER` chain,
  `MASQUERADE` on egress, the role of `docker-proxy` (and when it is actually used),
  localhost publishing and `route_localnet`.
- DNS: Docker's embedded resolver at 127.0.0.11, `/etc/resolv.conf` generation,
  container-name resolution, and what the minimum is (copy host resolv.conf).
- Container-to-container reachability on a bridge; iptables vs nftables reality on modern
  distros (`iptables-nft` translation) and which we should target.
- Rootless networking: why veth+bridge needs `CAP_NET_ADMIN` in the *host* netns,
  slirp4netns vs pasta, port forwarding without privileges, performance cost.
- Smallest defensible ladder: host netns → new netns with loopback only → veth + bridge +
  NAT → published ports → DNS.

### D5 — Resource control, security boundary & rootless

**Why its own agent:** cgroup v2 and the security layers are "constraints applied to the
container process" and share one config surface, one privilege model, and one failure
mode: a container that looks isolated but is not. This agent's mandate includes naming the
dangerous shortcuts that educational implementations ship.

Must answer:
- cgroup v2: unified hierarchy, `cgroup.subtree_control`, the no-internal-process rule,
  `cgroup.procs` vs `cgroup.threads`, delegation (and delegation to an unprivileged user),
  and how systemd's ownership of the hierarchy affects a non-systemd-aware runtime.
- Concrete mapping of `--memory` → `memory.max`/`memory.high`/`memory.swap.max`,
  `--cpus` → `cpu.max`, `--pids-limit` → `pids.max`; plus `memory.events`, OOM behavior,
  and `cgroup.kill`/freezer for stop semantics.
- cgroup v1 vs v2 differences that actually matter (per-controller hierarchies, no unified
  OOM/pressure, `cpu.cfs_*` naming, rootless being effectively v2-only) — and whether
  supporting v1 is justified. Concretely: how does a runtime *detect* v1 vs v2 vs hybrid,
  and what is the honest failure message when there is no writable cgroup at all (the
  `old-host` case: v1, no delegation, Slurm-owned)?
- **Kernel feature floor.** Which mechanisms exist at 4.18/RHEL-8 vs require 5.x+:
  `clone3`, `openat2`, cgroup v2 defaults, idmapped mounts, time namespace, unprivileged
  overlay (and which RHEL-8 backports change this — probing showed overlay-in-userns
  working on 4.18), seccomp notify, `pidfd_open`/`pidfd_send_signal`, `CLONE_INTO_CGROUP`.
  Recommend a minimum kernel and name what each version buys.
- Capabilities: the five sets, `CAP_SYS_ADMIN` as the real boundary, Docker's default
  bounding set, `no_new_privs`, why capability drops must happen in the right order
  relative to `execve`.
- seccomp: filter mechanics, `SCMP_ACT_ERRNO` vs `KILL`, notify, architecture/multiplexed
  syscall pitfalls (x32, socketcall), cost of a wrong default policy, and the
  seccomp-vs-userns interaction.
- User namespaces: uid/gid mapping, `/etc/subuid`/`subgid`, `newuidmap`/`newgidmap`,
  `setgroups` deny, `uid_map` write rules, idmapped mounts as the modern alternative to
  `chown`-ing layers.
- `/proc` and `/sys` handling: masked and read-only paths, why a bare `/proc` mount is a
  hole, `/proc/self/exe` reopening (CVE-2019-5736 class), device cgroup / device nodes,
  what `--privileged` actually turns off.
- The line between **process isolation** and **security isolation**, stated operationally,
  plus an explicit list of shortcuts we must not ship silently (writable `/proc`,
  `chroot` instead of `pivot_root`, no propagation change, full caps, no seccomp, host
  netns, host IPC, sharing the host cgroup, root-in-container == root-on-host).
- Rootless privilege model: what works, what silently degrades (cgroup delegation,
  overlay, netns, `--pids-limit`), and whether rootless should be a first-class target or a
  later mode.

---

## Execution after approval

1. **Phase 2 — parallel research.** Five independent agents, one per domain, launched in
   one batch. Each is given: the source-priority order (kernel docs → OCI specs →
   runtime docs → runtime source → standards → quality engineering writing → talks/papers
   → GitHub → forums only for undocumented behavior), the strict
   **Fact / Convention / Recommendation** separation, a requirement that every material
   claim carries a citation, and an instruction to flag its own low-confidence claims.
   Each also reports Rust crates relevant to its own domain.

   Each agent additionally gets the **read-only observation budget** on `host-a`
   (kernel 6.11, root, cgroup v2, live Docker) under the allowed/forbidden lists above, and
   may use `old-host` for old-kernel/rootless checks. Empirical results are labeled as
   observations on a specific host and kernel — never generalized into Facts about Linux.

2. **Phase 3 — adversarial verification.** Aggregate findings, then group them into seven
   claim clusters — kernel isolation, runtime architecture, OCI, storage/images,
   networking, resource control & security, Rust implementation — and launch **fresh**
   agents (none of the original five) tasked with *falsifying* the claims: check against
   primary sources, find outdated information (cgroup v1 assumptions, pre-nftables
   networking, old overlay limits, unmaintained crates), surface source disagreements,
   separate fact from recommendation, and mark every weakly-evidenced claim. Surviving
   claims are labeled with confidence.

   Verifiers get the same read-only host access, and are told to prefer refutation by
   observation where a claim is checkable — a doc-supported claim that the live kernel 6.11
   box contradicts is the most valuable finding this session can produce.

3. **Phase 4 — architecture risk pass.** Classify each rewrite-risk decision as
   `DECIDE NOW` / `DEFER SAFELY` / `EXPERIMENT FIRST` with reasoning: daemon vs
   daemonless, CLI/runtime coupling, state and metadata layout, image store layout,
   namespace abstraction boundaries, cgroup v1/v2 assumption, root-required assumption,
   networking ownership, async vs sync internals, and coupling image handling to execution.
   Two risks are promoted by the environment probe: the **kernel feature floor** (building
   on `clone3`/idmapped mounts silently excludes the user's only real hardware) and the
   **root-required assumption** (root-first is correct for the VM, but hard-coding
   "we are uid 0" instead of routing privilege through one seam is the expensive version of
   that choice).

4. **Deliverable — `RND.md`** at the project root, with the spec's structure: priority
   index (P0/P1/P2, sorted by architectural importance, each entry linking to its
   section), the 22 required sections, the detailed `mydocker run alpine /bin/sh` creation
   trace, and the implementation ladder where every stage states goal, new concept,
   commands supported, explicit non-goals, success test, and risks.

**Verification of the deliverable:** claim-level citations present and resolvable; no
section mixing Fact/Convention/Recommendation; index loadable standalone and sufficient to
route a fresh session; every ladder stage independently testable; verification pass
outcomes recorded (confirmed / corrected / disputed / weak evidence) rather than silently
merged.

**Stop condition:** `RND.md` written. No `SPEC.md`, no tasks, no code.
