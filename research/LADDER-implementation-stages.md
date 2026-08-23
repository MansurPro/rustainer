# LADDER — the incremental implementation ladder

Closing section of `RND.md`. One merged progression across all five domains, not five
concatenated ones. Every stage is small enough to become one or a few features and produces
something independently testable.

**Source precedence.** Where a `V*` verification file refutes or corrects a `D*` claim, the `V`
verdict governs and is what is cited below. The `D` files were not edited after verification, so
a citation to a `D` section without a `V` companion means "checked, nothing overturned it".

**Tags.** `[F]` fact · `[C]` convention · `[R]` recommendation · `[O]` observation, each with
`HIGH`/`MED`/`LOW` confidence. Where a line is my own engineering judgement rather than a
reported finding, it says **judgement** in the sentence.

**Dependency not yet available.** `research/PHASE4-risks-and-reconciliation.md` **does not exist**
as of this writing (`ls` on the research folder, 2026-09-05). The reversibility table in **§D** is
therefore derived directly from `V3 §D.4/§L`, `V4 §V4-6.5`, `D3 §11.5`, `D2 §7.5` and `D5 §17`.
When the risk register lands, §D should be reconciled against it rather than re-derived.

---

## A. Answers up front

The four questions the brief asks, answered before the ladder so nobody has to reconstruct them
from it.

### A1. Which stage is the first a person could actually use?

**L9 — `mydocker run alpine /bin/sh` against a real image pulled from Docker Hub.**

Being honest about the rest: **L0, L1, L2, L4, L5, L6, L7 and L8 have no user-visible payoff at
all.** They are the syscall sequence, the privilege sequence, the limit sequence, the process
model and the byte-fetching pipeline. L3 is the first stage whose output a human recognises (a
shell in a rootfs that is not the host's), but it needs a rootfs directory that the human had to
produce by other means, so it is a demo, not a tool. L9 is the first stage where the input is a
name a person already knows and the output is a working shell.

**judgement** — say this out loud in the project README at every stage before L9, because the
alternative is a stakeholder watching eight stages of "no visible change" and concluding the
project is stalled when it is in fact doing the only part that is hard.

There is a second, weaker sense of "useful" worth naming: **L10 (routed NAT egress + resolver
rewriting) is where the engine stops being a curiosity**, because before it an image cannot
`apk add` / `apt-get` (`D4 §11.2` `[R]` HIGH; `V5 §F2` confirms the payoff and corrects its
cause — see §C3). L9 gets you a shell; L10 gets you a shell you can install things into.

### A2. Which stage crosses from process isolation into security isolation?

**L5.** Not before.

- `[F]` HIGH — process isolation is about *visibility*, security isolation is about *authority*;
  a container can have perfect visibility isolation and zero authority isolation (`D5 §15.1`).
- The concrete demonstration: a fully namespaced container with a writable `/proc/sys` is one
  `echo` into `core_pattern` away from running a command as root on the host (`D5 §15.2`).

So the line is drawn where authority is first actually constrained: **bounding-set drop +
masked/read-only `/proc` + `close_range` (L4), then the seccomp allowlist (L5)**. Both halves are
needed — `V6 §G.2` shows the two remaining layers after L4 share a failure mode, and `D5 §19.2`
tiers the seccomp allowlist itself **required**, not hardening.

**Everything at L4 and below must be documented, in the README and in `--help`, as
"process-isolated, NOT secure".** `D5 §15.3` `[R]` HIGH already demands exactly this: *"each stage
of the ladder should state which of these it has not yet fixed… The dangerous version is the same
code with no such statement."* Each stage below carries that statement in its **Do NOT support
yet** field.

And the ceiling, stated once so no stage over-claims: even after L5 the container's root **is**
host root (`D5 §9.2`, re-measured `V6 §M.6`: `uid_map = 0 0 4294967295`). `V6 §N.7` puts it
plainly — the two capability-drop deviations *"do not change the runtime's security class… Only a
user namespace changes the class."* And a namespace-based engine does not defend against a
hostile kernel-exploiting image at all (`D2 §11.5`, `[R]` "say so in the README"). L5's honest
claim is **"the default `docker run` security posture, minus the user namespace"** — nothing more.

### A3. Which stages are reversible and which lock something in?

Full table in **§D**. The three that genuinely lock in:

1. **L8 — `tar-split` capture, yes or no.** `V3 §L` reranks this as *the* irreversible decision in
   the storage domain: it can only happen **during extraction**, so retrofitting means
   re-extracting every layer in the store. Note this is a change from `D3 §11.5`, which ranked the
   CAS keying first; `V3 §D.4` demotes that, because every content-address key is recomputable
   offline from data already on disk.
2. **L9 — the xattr namespace layers are extracted with** (`trusted.overlay.*` vs `user.overlay.*`).
   `V4 §V4-6.5` adds this as a fourth lock-in `D3 §11.5` missed: a layer extracted with one cannot
   be used under a mount using the other, and switching means re-extracting the whole store.
3. **L7 — who is the container's parent.** `D2 §7.5`: adding a socket to a supervisor is cheap;
   *changing who the container's parent is* is not. Decide at L7 and never revisit.

Two more that are cheap to pre-empt and expensive to retrofit, both one-liners now:

- **L6** — name the container cgroup path helper `container_cgroup(id).join("init")` from the
  first line of cgroup code (`V6 §C.4`, endorsing `D5 §1.3`). Retrofitting an `exec` sub-cgroup
  later requires live-migrating a running container's PID 1.
- **L4** — make "who writes `uid_map`" a **named participant** in the create path even though the
  root-only engine never calls it (`D2 §7.5`). Then rootless is filling in a function; otherwise
  it is a rewrite. `D5 §17.3` calls this the single seam.

### A4. Where does each stage's success test run?

| Target | Reality | Role in this ladder |
|---|---|---|
| **`devvm`** — a local destroyable Linux VM, **does not exist yet** | modern kernel (≥ 6.7 so `lowerdir+` is exercisable), cgroup v2, root, local disk | **Primary. Every success test below runs here unless marked otherwise.** Building it is L0. |
| `host-a` | `HOST-A`, Ubuntu 24.10, 6.11.0-29, live traffic on :80/:443 | **Read-only observation only.** Used below only to *compare against* Docker's live behaviour, never to run a stage's test. |
| `old-host` | Rocky 8.8, kernel 4.18, cgroup v1, no root, no subuid — **UNREACHABLE** (`old-host.example.edu` → NXDOMAIN, re-confirmed independently in `V1`, `V4 §V4-9`, `V5`, `V6 §L`) | **Degradation probe.** Stages that *want* it are marked **[wants old-kernel host]**. None of them is blocked on it. |

`[R]` HIGH — **judgement, but forced by the corpus:** the dev VM must be ≥ 6.7, not 5.10.
The engine's *floor* is 5.10 (`D5 §6.3`), but `lowerdir+` first appears at **v6.7**
(`V4 §V4-1.3`, `V6 §I.3` — both correct the kernel's own docs, which say 6.8). A 5.10 dev VM
cannot exercise the fast path; a 6.7+ dev VM plus the probe from L0 can exercise **both** paths by
forcing the fallback. Build one VM at 6.7+, and force-flag the fallback rather than building two.

Stages wanting the old-kernel host, and exactly what it would settle:
- **L0** — the `ENOSYS` column of the probe table on a 4.18 kernel (`V1 §D1c` gives the command).
- **L3** — `mknod` → `EPERM` inside a user namespace (`V1 §D10`, `V3 §I.4`).
- **L12** — unprivileged overlay at 4.18 via a RHEL backport, and the `userxattr` path
  (`V4 §V4-9`, `V6 §I.2` — this is the motivating case for probe-don't-version-gate and it is
  still unverified).

---

## B. The ladder

Fifteen rungs (L0–L14), plus L15, which is deliberately **not** a rung and says so.
Read the **Trap** field of each before writing its code; every one of them is a mistake the
corpus caught somebody already making.

**Ordering principle for L0–L5: understanding over Docker compatibility.** The first six rungs
buy no Docker-shaped feature at all — no image, no network, no `-p`, no `inspect` field anyone
would recognise. They exist to make the syscall sequence and the privilege sequence correct and
legible while they are still small enough to hold in one head. Compatibility starts at L8 and is
not *claimed* until L14 (§C7).

---

### L0 — Dev VM and the feature probe

**Goal.** A destroyable Linux VM at kernel ≥ 6.7 with cgroup v2 and root, and one subcommand —
`mydocker probe` — that reports which optional kernel features are present *by calling them*, not
by parsing `uname`.

**New kernel/runtime concept.** Runtime feature detection. `[R]` HIGH — **probe features, never
version-gate** (`D1 §2.1`, upgraded by `V1 §D1c` and `V6 §I.2` to *the strongest recommendation in
the corpus*). The probes are cheap and each fails with a distinguishable errno:
`clone3` → `EINVAL` vs `ENOSYS` (`V1 §C9`, `V2 §I2` — both confirm this is a valid availability
inference); `fsopen("overlay")` then `fsconfig(SET_STRING,"lowerdir+",…)` → `EINVAL`/`ENOPARAM`
when < 6.7 (`V6 §I.3`); `close_range`, `openat2`, `mount_setattr`, `pidfd_open`, `CLONE_INTO_CGROUP`,
`cgroup.kill`, `cgroup.freeze`, `io.bfq.weight`/`io.cost.qos`.

**Commands after this stage.** `mydocker probe`. Nothing else.

**Do NOT support yet.** Everything. **This stage produces no container and must not be described
as producing one.**

**Success test (devvm).** `mydocker probe` prints a table; every row matches a hand-checked
expectation for that kernel. Then, on the same VM, boot a second image at 5.10 (or force the
fallback flag) and confirm the `lowerdir+` row flips to absent **without any version string
appearing in the code path** — grep the source for `uname`/version parsing and expect zero hits
outside a diagnostics banner.
**[wants old-kernel host]** — the `ENOSYS` column at 4.18; command in `V1 §D1c`.

**Major risks.** Building probes that *mutate*: an overlay probe that actually mounts, a cgroup
probe that writes into a live hierarchy. Keep every probe to "construct and discard" — `fsopen`
without `fsmount`, `clone3` with an invalid arg — so the probe is safe to run anywhere, including
read-only against `host-a`.

**Verified-claim dependencies.**
- `[R]` HIGH — probe-don't-parse (`V1 §D1c`, `V6 §I.2`, both CONFIRMED and independently reinforced).
- `[F]` HIGH — `lowerdir+` is **v6.7**, not 6.8; the kernel's own documentation is one release
  conservative (`V4 §V4-1.3`, `V6 §I.3`; code present at tag v6.7, absent at v6.6).
- `[F]` HIGH — `OPEN_TREE_NAMESPACE` needs **Linux 7.0** and exists on no kernel this project can
  reach; crun ships it behind exactly this probe pattern with a local `#define` (`V1 §G1`).
- `[F]` MED — feature probes are necessary but **not sufficient**: kernels < 5.4 mis-dispatch x32
  syscall numbers 512–547 in a way no probe can detect (`V6 §A.4`, `§I.2`). Keep a short,
  explicitly-listed set of version checks for known-broken ranges *alongside* the probes.

**Trap.** Writing the probe table as documentation and the code as version checks. `V6 §I.2`'s
motivating case is RHEL 8 shipping overlay-in-userns at 4.18, years before upstream 5.11 — a
version check gets that host wrong in the direction of refusing a feature that works. (That case
is itself `UNSUPPORTED — pending host`; the rule does not depend on it.)

---

### L1 — Run a process

**Goal.** `mydocker run -- /bin/echo hi` forks, execs, waits, and exits with the child's exit
status. No namespaces, no rootfs, no cgroup. The container is the host, with extra steps.

**New kernel/runtime concept.** The process model and exit-status plumbing, plus the invariant
that governs everything after it: **the process that will call `clone()` must never have had a
second thread.**

**Commands after this stage.** `mydocker run -- <host path> [args]`.

**Do NOT support yet.** Isolation of any kind. **This is not a container and must not be called
one.** No images, no rootfs, no limits, no detach, no logs.

**Success test (devvm).** A table-driven test: `/bin/true` → 0; `/bin/false` → 1; `sh -c 'exit 42'`
→ 42; `sh -c 'kill -SEGV $$'` → 139 (128+11). Then the invariant test: a unit test that spawns a
thread before the fork point and asserts the `Threads == 1` check **fails loudly** rather than
proceeding.

**Major risks.** Exit-status encoding. `[F]` HIGH — code in bits 8–15, signal in bits 0–6
(`V1 §E4`, confirmed). Getting this wrong is silent and shows up two stages later as "the
container reports 0 when it crashed".

**Verified-claim dependencies.**
- `[F]` HIGH — a raw `clone3` **forfeits glibc's `atfork` protection**. glibc's `fork()` locks
  NSS, the libio list and the malloc arenas before cloning and resets them in the child; a raw
  `clone3` runs none of it (`V2 §8.1`, CORRECTED, and the correction makes the conclusion
  *stronger* than `D2` stated). So `Threads == 1` is not belt-and-braces; it is the only thing
  standing between the engine and a nondeterministic hang.
- `[F]` MED — the no-allocation-post-clone rule and the `Threads == 1` assert are **alternatives,
  not a stack** (`V1 §H4`, MISCLASSIFIED against `D1 §11.2`). In a single-threaded process no
  other thread can hold a malloc arena lock. Pick the assert as primary and treat allocation
  discipline as defence in depth — but see L4's trap for where the discipline still binds.
- `[R]` HIGH — `rustix` primary, `libc` for constants and layouts (`V7 §1`, CONFIRMED row by row);
  `rustix::thread` already ships `capget`/`capset`/`PR_CAP_AMBIENT_*`/`PR_SET_KEEPCAPS`/
  `no_new_privs` (`V1 §H3`, which makes `D1 §11.1`'s "hand-write ~60 lines" **obsolete**).

**Trap.** Adding a logging framework here. `[R]` HIGH — `tracing` is fine in the CLI and the
supervisor and **banned in the cloned child** (`D2 §12.7`; `V2 §8.2` confirms the convergent
practice across runc, crun and conmon). `tracing-appender`'s `non_blocking` spawns a thread and
would break L1's own invariant (`V7 §13` row 31, CONFIRMED, correctly rated structural). Decide
the child's error channel now: a fixed-size `#[repr(C)]` record over a socketpair, the shape crun
uses verbatim (`V2 §9.3`, CONFIRMED — `struct sync_socket_message_s { int type; int error_value;
char message[512]; }`).

---

### L2 — Namespaces: an isolated process

**Goal.** `clone3` with `CLONE_NEWNS|NEWPID|NEWNET|NEWUTS|NEWIPC|NEWCGROUP`; change `/`'s
propagation; mount a fresh `/proc` **inside the new mount namespace only**; bring `lo` up. The
process still sees the host's filesystem tree.

**New kernel/runtime concept.** Namespace creation at clone time (not afterwards), mount
propagation, and the fact that `procfs` and `sysfs` are gated on *which namespaces you are in*.

**Commands after this stage.** `mydocker run --isolate -- <host path>`; `--network=host` as a
permanent orthogonal mode (see below).

**Do NOT support yet.** No rootfs change — `/` is still the host's, so **a process here can read
and write every host file**. No limits, no capability drop, no seccomp. **Not secure; do not
describe it as isolated to anyone but a developer.**

**Success test (devvm).**
1. `mydocker run --isolate -- ps aux` shows 1–2 processes, PID 1 is ours.
2. `mydocker run --isolate -- hostname` returns the container hostname, and the host's hostname is
   **unchanged** afterwards (this is the `CLONE_NEWUTS` test and the way to catch renaming the
   host — `D1 §10.1` step 20).
3. `mydocker run --isolate -- ip addr` shows only `lo`, up.
4. **The leak test, which is the real point of this stage:** `wc -l /proc/self/mountinfo` on the
   host before and after 50 runs — the count must be identical. `[O]` HIGH — `D1 §4.2` measured a
   real leak without the propagation change, and `V1 §D4` confirms it from the primary source in
   the opposite direction as well.

**Major risks.** Getting the propagation change wrong so every container mount lands permanently
on the host. This is `D5 §15.3`'s second dangerous shortcut and it is a *permanent host-state*
leak, not a transient one.

**Verified-claim dependencies.**
- `[F]` HIGH — the kernel requirement is only that `/` and `new_root`'s parent are **not
  `MS_SHARED`**. `MS_SLAVE`, `MS_PRIVATE` and `MS_UNBINDABLE` all satisfy it; `pivot_root(2)`'s own
  EXAMPLES program uses `MS_REC|MS_PRIVATE`, and crun defaults private while runc and youki
  default slave (`V1 §F2`, MISCLASSIFIED against `D1 §9.3`'s "`MS_REC|MS_SLAVE` required"). Pick
  one, record it as a choice, and do not let the table say "required".
- `[F]` HIGH — mounting `procfs` requires being in the new **PID** namespace; mounting `sysfs`
  requires `CAP_SYS_ADMIN` **in the user namespace that owns your current net namespace**. `V1 §D9`
  reproduced both on a kernel seven majors newer than `D1`'s and **promoted them from `[O]` to
  `[F]`**. The sysfs rule is stated backwards from intuition: it is not "sysfs needs a netns", it
  is "sysfs needs your netns to be *yours*".
- `[F]` HIGH — `clone(CLONE_NEWPID)` puts the child in the namespace as PID 1 directly; the
  "extra fork" is an `unshare`/`setns` artefact, not a kernel requirement (`V1 §C3`, CONFIRMED
  from `pid_namespaces(7)`).
- `[F]` HIGH — `CLONE_NEWUSER` cannot be combined with `CLONE_PARENT`, and `CLONE_NEWNS` cannot be
  combined with `CLONE_FS`, in the same `clone()` (`V1 §C8` — new rows `D1 §3.6` was missing).
  Matters at L15, but the flag-building code is written here.
- `[R]` HIGH — **`--network=host` is a permanent orthogonal mode, not a rung** (`D4 §11.1`,
  CONFIRMED in substance by `V5 §F1`). `V5` corrects its cost: it is ~10 lines, not 1, because the
  publish path must be short-circuited and `/etc/resolv.conf` needs a second branch (the host stub
  *is* reachable in host netns and nowhere else). And its cost column must name that **the abstract
  UNIX-domain socket namespace is per-netns** (`network_namespaces(7)`), so host mode hands the
  container every `@`-prefixed listener a host daemon has open.

**Trap.** Probing `/proc/self/ns/pid_for_children` to confirm `unshare(CLONE_NEWPID)` worked. It is
**empty** until the first child is created, so `readlink` returns `ENOENT` on the healthy path
(`V1 §C4`, CORRECTED — `D1 §1.2`'s transcript only looked right because the `readlink` ran in a
forked child, which is the event that populates the link). Second trap in the same area: `[F]` HIGH
`setns` into a **time** namespace *does* move the caller (`V1 §C1`, REFUTED against `D1 §3.1`) and
returns `EUSERS`, not `EINVAL`, when multithreaded. Only PID is children-only on the `setns` path.

---

### L3 — Filesystem isolation: `pivot_root` onto a directory rootfs

**Goal.** Hand the engine a directory containing an unpacked rootfs and get a shell inside it.
The minimum viable mount set, `pivot_root`, `umount2(put_old, MNT_DETACH)`, `chdir("/")`.

**New kernel/runtime concept.** `pivot_root` and the mount set a real userland needs.

**Commands after this stage.** `mydocker run --rootfs <dir> -- /bin/sh`.

**Do NOT support yet.** No images, no layers, no limits, no capability drop, no seccomp, no
`/proc` masking. **This container holds all 41 capabilities, can write `/proc/sys/kernel/core_pattern`,
and is therefore one `echo` from root on the host (`D5 §15.2`). It is a teaching artefact.**

**Success test (devvm).** Produce the rootfs out of band once
(`docker export $(docker create alpine) | tar -x -C /var/tmp/alpine-rootfs` on the VM), then:
1. `mydocker run --rootfs /var/tmp/alpine-rootfs -- /bin/sh -c 'ls /'` shows alpine's `/`, not the
   host's.
2. `mydocker run --rootfs … -- sh -c 'cat /proc/1/comm'` shows our init, and `/proc` is a fresh
   mount (`stat -f` fstype `proc`).
3. **The escape regression test:** `sh -c 'cd /; ls ..'` must show the container's `/`, and there
   must be no `/oldroot` or `put_old` directory anywhere. `[F]` HIGH — an accessible `put_old` is a
   one-line escape (`V1 §F1`).
4. **The `/dev/null` test:** `sh -c 'echo x > /dev/null; ls -l /dev/null'` must show a character
   device `1, 3`, not a regular file. `[O]` HIGH — an empty `/dev` makes `echo x > /dev/null`
   silently create a **regular file** (`D1 §6.2`, CONFIRMED `V1 §D10`) and every subsequent write
   to it grows the rootfs.
5. Repeat the host mountinfo leak count from L2 — still zero.
**[wants old-kernel host]** — `mknod` → `EPERM` inside a user namespace, which is the whole reason
device nodes should be bind-mounted rather than `mknod`'d (`V1 §D10`, `V3 §I.4`).

**Major risks.** The `pivot_root` error table. `[F]` HIGH — `pivot_root(2)` documents **six**
restrictions and `D1 §5.2` measured four of them under a *shared* `/`, where the shared-propagation
check fires first and every `EINVAL` is uninformative (`V1 §B2`, REFUTED). Concretely:
`new_root == "/"` returns **`EBUSY`**, not `EINVAL` (`V1 §A4`), and a non-directory `put_old`
returns **`ENOTDIR`** — hit on the first try in `V1`'s own re-run. An error handler written from
`D1 §10.1` step 17's table will mis-report both.

**Verified-claim dependencies.**
- `[F]` HIGH — the runc seven-step sequence (`open("/",O_PATH)` → `fchdir` → `pivot_root(".",".")`
  → `fchdir(oldroot)` → `MS_SLAVE|MS_REC` → `umount2(".",MNT_DETACH)` → `chdir("/")`) is verbatim
  accurate against runc v1.5.1, and the `pivot_root(".",".")` idiom is in the man page's own NOTES
  (`V1 §B5`, CONFIRMED verbatim).
- `[F]` HIGH — `pivot_root` does **not** move the caller's cwd, so the trailing `chdir("/")` is
  required (`V1 §B3`, independently reproduced).
- `[F]` HIGH — `chroot` is not a substitute: the load-bearing half of the escape (hold an
  `O_DIRECTORY` fd across the `chroot`, `fchdir` it, read the host by relative path) **requires no
  capability at all**. `V1 §B6` CORRECTS `D1 §5.5` here — dropping `CAP_SYS_CHROOT` does *not*
  mitigate it.
- `[R]` HIGH — MVP is `pivot_root` only; do **not** implement the `MS_MOVE` + `chroot` fallback.
  *"The masking step is worse half-right than absent"* (`D1 §5.6`, CONFIRMED sound `V1 §B7`).
- `[R]` HIGH — device nodes by **bind-mount**, not `mknod` (`D5 §14.3`, `§19.2` strongly
  recommended). It gives an identical rootful/rootless code path and it is what makes L4's
  `CAP_MKNOD` drop free.
- `[F]` HIGH — `/dev/ptmx` must be a **symlink** to `pts/ptmx`; a `mknod`'d `5:2` is a host tty
  leak (`D5 §19.2`, required for `-it`).
- `[C]` HIGH — the Docker mount table (`/proc` `nosuid,nodev,noexec`; `/dev` tmpfs `nosuid` with
  **no** `nodev` and **no** `noexec`; `/dev/pts`, `/dev/shm`, `/dev/mqueue`, `/sys` ro,
  `/sys/fs/cgroup` ro) re-measured row for row on the live host (`V1 §D7`, CONFIRMED). Copy it,
  including both asymmetries, and know why each is there before deviating.

**Trap.** Believing `pivot_root` and a fresh `/proc` make this a container. `V1 §F1` explicitly
**MISCLASSIFIES** `D1 §9.1`'s "required" tier here: `docker run --pid=host` ships, and `D1 §6.1`'s
own measurement shows `id`, `python3`, `openssl` and `df` all work in a pivoted rootfs with **no**
`/proc` at all. What is genuinely irreducible at this stage is `CLONE_NEWNS`, the recursive
self-bind of the rootfs, `pivot_root`, the `put_old` detach and the `chdir("/")` — five things.
Everything else here is "real software breaks without it", which is a different tier and a
different urgency.

---

### L4 — The privilege floor: capabilities, `no_new_privs`, fd hygiene, `/proc` masking

> **This is the first departure from the brief's example ladder.** The brief puts "hardened
> security" at rung 10, after networking, images and layered storage. L4 and L5 are here instead.
> The argument is in §C1; the short version is that these items live **inside the same
> pre-`execve` window that L3 just wrote**, their ordering is kernel-forced, and getting it wrong
> fails *silently* with the container holding all 41 capabilities.

**Goal.** The child's final sequence, complete and in the kernel-forced order: bounding-set drop →
`capset` → `PR_SET_KEEPCAPS` → `setgroups`/`setgid`/`setuid` → `capset` → ambient →
`PR_SET_NO_NEW_PRIVS` → `close_range` → `execve`. Plus masked and read-only `/proc` paths,
read-only `/sys/fs/cgroup`, and the memfd self-seal.

**New kernel/runtime concept.** Capability sets and their `execve` transformation; the fact that
the **bounding set is the only durable drop**; `PR_SET_NO_NEW_PRIVS`; masked paths.

**Commands after this stage.** `mydocker run --rootfs <dir> [--user U:G] [--cap-add X] -- <cmd>`.

**Do NOT support yet.** **No seccomp — so `keyctl`, `userfaultfd`, `io_uring`, `kexec_load` and
`perf_event_open` are all reachable** (`D5 §15.3`). No user namespace, so container root is host
root. No limits: a fork bomb here takes down the VM. **Still not secure. This stage narrows
authority; L5 is where the claim changes.**

**Success test (devvm).** Three tests, and the first is the one that matters:
1. **The bounding-set test.** `mydocker run --rootfs … -- sh -c 'grep Cap /proc/self/status'` and
   assert `CapBnd == 0x00000000a80425fb` minus the two we drop — i.e. **the number is checked, not
   eyeballed**. Then repeat with `--user 1000:1000` and assert `CapBnd` is *still* the reduced set
   and `CapPrm`/`CapEff` are `0`. `[O]` HIGH — the reference value `0xa80425fb` was decoded bit by
   bit without `capsh` in `V6 §B.2` and is exactly moby v28.3.1's 14 defaults.
2. **The `close_range` / CVE-2024-21626 regression test.** A rootfs whose `WORKDIR` is
   `/proc/self/fd/7`; the run must **fail**, and the test must assert that `close_range` is why.
   `D5 §21` item 13 says this test *"should live in CI forever"* — agreed, and it is the cheapest
   security test in the whole project.
3. **The masking test.** `cat /proc/kcore` → fails; `echo x > /proc/sys/kernel/core_pattern` →
   `EROFS`; and — the one runc gets wrong — `mount | grep '/proc/sys'` shows `nosuid,nodev,noexec`
   **still set** after the read-only remount.

**Major risks.** Two, both silent.

1. **The capability ordering bug.** `[F]` HIGH — `V6 §B.4` / `§N.2` item 1 calls this *"the single
   most consequential error I found in the document"*: `D5 §7.7`'s numbered listing and `D5 §20`'s
   C9–C13 both put `setuid` **before** `PR_CAPBSET_DROP`. `setuid` away from 0 clears `CAP_SETPCAP`
   from the **effective** set, so every subsequent `PR_CAPBSET_DROP` fails `EPERM` — and if the
   return value is ignored the container runs with the **full 41-capability bounding set**.
   `PR_SET_KEEPCAPS` does *not* save you: it preserves **permitted** only, and the kernel clears
   **effective** unconditionally when euid leaves 0. The correct order is
   `capbset_drop → capset → [KEEPCAPS] → setgroups/setgid/setuid → capset → ambient`, matching
   runc — which is what `D5 §7.7`'s own **prose** says while its listing says the opposite.
   **Check every `prctl` return value in this sequence; a swallowed `EPERM` here is the whole
   security model.**
2. **The read-only remount clearing the mount flags.** `[F]` HIGH — `MS_REMOUNT|MS_BIND` sets the
   per-mount flags to exactly what is passed; flags not passed are **cleared**. A naive
   `MS_REMOUNT|MS_BIND|MS_RDONLY` on `/proc/sys` makes it read-only *and simultaneously clears
   `nosuid`, `nodev` and `noexec`* — a net regression introduced by the hardening step. runc reads
   the existing flags back with `statfs(2)` and re-specifies them; `V6 §F.4` flags this as omitted
   from `D5 §10.3` entirely.

**Verified-claim dependencies.**
- `[F]` HIGH — the bounding set is the security boundary; P/E are convenience. `P(bounding)` masks
  `F(permitted)`, so a file-capability or setuid-root binary in the image can never gain a
  capability outside the bounding set (`V6 §B.1`, CONFIRMED verbatim against `capabilities(7)`).
- `[F]` HIGH — a root container gets `P' = E' = P(bounding)` back on **every** `execve`
  (`V6 §B.3`, CONFIRMED). Two precision fixes `V6` adds: rule (1) triggers on **real or effective**
  uid 0 while rule (2) needs **effective** uid 0; and the rule does not apply to a setuid-root
  binary that *does* carry file capabilities — which is exactly our threat model, and the case
  where the bounding set does all the work.
- `[C]` MED → adopt — `SECBIT_NOROOT|SECBIT_NOROOT_LOCKED` exists and *does* let P/E be smaller
  than B for a uid-0 container (`V6 §B.1`, a material omission from `D5 §7.2`). `rustix` exposes
  it as `set_capabilities_secure_bits` (`V7 §13` row 21 — the crate carries all 41 capability
  constants, counted). Cheap extra knob; take it.
- **`CAP_MKNOD` is `required`, not hardening.** `[R]` HIGH — `V6 §J.2` **re-tiers** `D5 §19.2`'s
  "hardening (deviation)" row: because the engine also declines the eBPF device controller,
  dropping `CAP_MKNOD` is the load-bearing compensation, and restoring it later collapses the
  device story silently. Gate `--cap-add MKNOD` the way `--cap-add SYS_ADMIN` is gated. The cost is
  far smaller than it reads: `CAP_MKNOD` gates **only** char and block special files —
  `mkfifo`, UNIX sockets and regular files via `mknod` all keep working (`V6 §J.2`).
- **`CAP_NET_RAW` drop ships with a sysctl.** `[R]` HIGH — `V6 §J.2`: set
  `net.ipv4.ping_group_range = 0 2147483647` in the container netns and the entire *stated* cost
  (ping breaks) goes to zero, because modern `ping` prefers a capability-free `SOCK_DGRAM`
  ICMP socket. `V5 §G1` confirms Docker already widens this per container, unasked (host `1 0`,
  every container netns `0 2147483647`). The **real** costs, which `D5` does not name, are
  `AF_PACKET` (DHCP clients, `tcpdump`, `arping`) and `IP_TRANSPARENT` binding (Envoy/HAProxy
  transparent mode).
- **`no_new_privs` ON by default — adopt, with the rationale rewritten.** `[R]` HIGH —
  `V6 §J.1`: the headline benefit `D5 §7.4` leads with ("closes the setuid-root escape") is
  **overstated for the default root container**, which already gets `P' = E' = P(bounding)` on
  every `execve`. The two real benefits are (a) it deletes the `CAP_SYS_ADMIN`-shaped branch in
  seccomp installation, letting the filter go last, and (b) it blocks `execve`-time transitions to
  a *more permissive* LSM profile. Missing costs: file-capability binaries under `--user` break —
  which breaks *hardened* images specifically — and NNP is irreversible and inherited by every
  later `exec` for the container's life. `D5 §21` item 11 (the breakage inventory) should be
  **blocking**, not optional.
- `[F]` HIGH — **the memfd self-seal is mandatory regardless of architecture.** `V2 §7.3` REFUTES
  `D2 §7.3`'s "design so we never re-exec into the container eliminates the entire CVE class":
  `execve` of a user-controlled path that resolves to `/proc/self/exe` reproduces CVE-2019-5736
  with no runtime re-exec involved. crun has exactly the recommended architecture and still ships
  the memfd constructor; youki calls `pentacle::ensure_sealed()` as the first statement of
  `main()`. Four implementations, four seals. `D2 §11.8` is right and `D2 §7.3` is wrong.
- `[F]` HIGH — the CVE-2019-5736 mechanism itself is misdescribed in `D5 §11.1`: the
  `#!/proc/self/exe` shebang is the *bait inside the container*, the write is done by a second
  process via `/proc/<pid>/exe` after `ETXTBSY` clears, and it covers `docker run` with a
  malicious image, not only `docker exec` (`V6 §N.2` item 4). Add `PR_SET_DUMPABLE(0)` on the
  `exec` path at L13.
- `[R]` HIGH — resolve every container-supplied path with
  **`RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV`**, not `RESOLVE_IN_ROOT`. `V4 §V4-3.2`
  REFUTES "the whole answer": `RESOLVE_IN_ROOT` **clamps silently** (a `../../etc/passwd` entry is
  created at `<root>/etc/passwd` with no error), it does **not** stop crossing a mount point
  planted inside the root, and mixing in a single non-`openat2` call on a two-component basename
  re-opens the hole — measured escaping to `/etc/hosts`. `RESOLVE_BENEATH` turns the same attack
  into `EXDEV`.
- `[R]` HIGH — `D5 §11.4` requirement 4 ("confirm the cwd's `st_dev` matches the rootfs's") is
  **CORRECTED** by `V6 §N.2` item 2: it rejects every legitimate `WORKDIR` on a volume or tmpfs.
  Use a beneath-the-rootfs mount-tree check instead.
- `[F]` MED — 4 of Docker's 12 masked paths do not exist on 6.11, and runc skips `ENOENT` **and
  only** `ENOENT` when masking (`V6 §F.2`, `§F.3`, CONFIRMED in `rootfs_linux.go`). Copy that
  exactly; skipping other errnos hides real failures.
- `[C]` MED — `subset=pid` (Linux 5.8) removes most of the masking table in one mount option, but
  also removes `/proc/meminfo`, `/proc/cpuinfo`, `/proc/stat` and `/proc/mounts`, which nearly
  every workload reads (`V6 §K.3`). Not a default; the right lever for a locked-down profile
  later. `hidepid` is correctly skipped, but for the *mount-option/threat-model* reason, not
  `D5 §10.4`'s "everything in a container runs as the same uid", which is false under `--user`.

**Trap.** Adding the capability sequence as a bolt-on "hardening pass" at the end of the child
function. The sequence is **interleaved** with the uid change and the `execve` that L3 already
wrote — five of its orderings are kernel-forced (`D5 §20`) and the listing that most people would
copy is the one with the inverted step (`V6 §B.4`). Write it as one function whose contract is
"everything between the last mount and `execve`", and unit-test the resulting `/proc/self/status`
rather than the code path.

Second trap, from the same window: `[F]` HIGH **post-clone, pass only `&CStr`**. Both `nix` and
`rustix` re-allocate for `&str`/`&Path`/`&OsStr` above a small stack buffer (256 bytes in rustix,
1024 in nix) — so a long rootfs path silently crosses into the allocator *after* the clone
(`V1 §H4`, `V7 §13` rows 7–9). Put `&CStr` in the child's function signatures so the type system
forbids it. `V7 §4.3` adds that this is **necessary but not sufficient** — it misses the seccomp
`Vec<sock_filter>`, argv/envp and any `std::fs` call in the child; build the seccomp program in
the **parent** (`D5 §20` P6) and consider an allocator-abort shim in the child as a permanent
guard.

---

### L5 — Seccomp: the line into security isolation

**Goal.** A cBPF allowlist with `defaultAction = SCMP_ACT_ERRNO`, an architecture prologue, and
the `ENOSYS` special cases, installed as the last thing before `execve`.

**New kernel/runtime concept.** seccomp-bpf, `SECCOMP_FILTER_FLAG_TSYNC`, and the interaction
between `no_new_privs` and filter installation.

**Commands after this stage.** As L4, plus `--security-opt seccomp=unconfined|<profile.json>`.

**Do NOT support yet.** No `seccomp_unotify` (`SECCOMP_RET_USER_NOTIF`) — `D5 §8.7`'s
"do not use in MVP" is well-founded and `V6 §A.5` confirms the TOCTOU caveat needs the
`SECCOMP_IOCTL_NOTIF_ID_VALID` re-check. No LSM profile. No user namespace. **Container root is
still host root.**

**After this stage the honest claim is: "the default `docker run` security posture, minus the
user namespace."** Not "secure against a hostile image."

**Success test (devvm).**
1. `mydocker run --rootfs … -- sh -c 'keyctl show'` → `EPERM`; same for a small C/Rust probe
   calling `userfaultfd`, `io_uring_setup`, `kexec_load`, `perf_event_open`.
2. **The `ENOSYS` test, which is the one that breaks production if wrong:** run a glibc ≥ 2.34
   binary that forks, once with `clone3` denied as `EPERM` and once as `ENOSYS`. The `EPERM`
   container must break; the `ENOSYS` one must fall back to `clone` and work (`D5 §21` item 10).
3. **The arch test:** an `int 0x80` i386 call from a 64-bit binary must be denied. Assert this
   directly — it is *not* implied by the allowlist (see Trap).
4. Compare against `host-a` **read-only** as a reference oracle: parse
   `moby/profiles/seccomp/default.json` and diff our allowed set against its 426 names / 361
   unconditional (`V6 §A.1`, counts CONFIRMED exactly).

**Major risks.** Denying a syscall that glibc probes-and-falls-back on with the wrong errno.
`[F]` HIGH — glibc sets `__clone3_supported = 0` only on **`ENOSYS`** and never retries on any
other errno, so `EPERM` propagates and breaks `fork`/`pthread_create` (`V6 §A.3`, CONFIRMED). The
known probe-and-fallback set as of glibc 2.4x: `clone3`, `faccessat2`, `close_range`, `openat2`,
`statx`, `getrandom`, `membarrier`, `rseq`. A blanket `EPERM` for these is *"a live incident
generator"*.

**Verified-claim dependencies.**
- `[F]` HIGH — allowlist, not blocklist; `defaultAction = SCMP_ACT_ERRNO`; the ~20 dangerous
  syscalls are denied **by the mechanism**, not by named deny rules (`V6 §A.1`, CONFIRMED by
  absence against moby v28.3.1).
- `[F]` HIGH — **`ptrace`, `process_vm_readv` and `process_vm_writev` are ALLOWED by Docker's
  default profile**, unconditionally on any kernel ≥ 4.8, via `includes: {"minKernel":"4.8"}`.
  `V6 §A.2` **REFUTES** `D5 §8.5`'s claim that they are capability-denied. Consequence: a container
  process can `ptrace`/`process_vm_readv` another process of the same uid inside the same
  container, defeating in-container privilege separation, and with `--pid=host` on a host with no
  LSM it can read any host process of the same uid. `[R]` — **deny them by default and re-allow
  under `--cap-add SYS_PTRACE`.** We are not carrying moby's compatibility debt, and `gdb`-in-
  container is exactly what a documented `--cap-add` is for. **judgement:** this is a deliberate
  deviation from Docker and should be listed alongside the other two.
- `[F]` HIGH — the x32 property is structural (an x32 `nr` carries `__X32_SYSCALL_BIT = 0x40000000`
  and can never collide with a legitimate x86_64 `nr`), but the **i386 property is not**: i386
  numbers *collide* with x86_64 numbers, so allowing x86_64 `nr == 26` (`madvise`) allows an
  `int 0x80` caller to reach i386 `nr == 26` (`ptrace`). `V6 §A.4` CORRECTS `D5 §8.3` rule 1: the
  i386 denial comes **entirely** from rule 2 (check `arch` first). Emit `seccomp(2)`'s own
  canonical prologue — check `arch == AUDIT_ARCH_X86_64`, then reject any `nr > X32_SYSCALL_BIT - 1`
  — *before* the allowlist comparison.
- `[F]` MED — kernels **before 5.4** mis-dispatch `nr` in 512–547 and the corresponding numbers
  ORed with `__X32_SYSCALL_BIT`, e.g. `nr == 521` reaching `ptrace` with confused semantics
  (`V6 §A.4`). This is a **security** argument for the 5.10 floor that `D5 §6.3` does not use, and
  it is stronger than any of the convenience reasons it does use.
- `[F]` HIGH — `execve` and `execveat` must be in the allowlist or the container never starts
  (`V6 §A.5`, both present in moby's 361-name unconditional group).
- `[F]` HIGH — all filters run and the most severe result wins, so filters can only be added,
  never relaxed (`V6 §A.5`). And vDSO calls (`time`, `gettimeofday`, `clock_gettime`, `getcpu`)
  bypass seccomp entirely — never build a security property on filtering them.
- `[R]` HIGH — **hand-write the cBPF.** `rust-vmm/seccompiler` was **archived on 2026-08-18** into
  the rust-vmm monorepo and 0.5.0 is 18 months old; `libseccomp-rs` has **0 commits in 180 days**
  (`V7 §3.1`, `§3.2`, OUTDATED against `D5 §18.2`). The verdict is unaffected and strengthened.
  Note `runc` on `host-a` is dynamically linked against `libseccomp.so.2` (`V7 §13` row 29) —
  a dependency we do not want in a static binary.

**Trap.** Assuming the allowlist gives you the architecture check for free. It does not, for i386
(above), and stating rule 1 as independent of rule 2 *"invites someone to skip rule 2"*
(`V6 §A.4`). Write the prologue first and test it with an actual `int 0x80`.

Second trap: installing seccomp before dropping capabilities and then wondering why it worked.
`[O]` HIGH — Docker's live containers show `NoNewPrivs: 0` with `Seccomp: 2, Seccomp_filters: 1`
(`V1 §E6`, `V6 §J.1`), i.e. runc took the branch that installs the filter *while still holding
`CAP_SYS_ADMIN`*. With `no_new_privs` mandated (L4) that branch disappears and the filter goes
last — which is the actual benefit of the NNP deviation, and the reason to mandate it.

---

### L6 — cgroup v2: limits that are real

**Goal.** Create the cgroup before the child exists, place the child atomically with
`CLONE_INTO_CGROUP`, apply `memory.max` / `pids.max` / `cpu.max`, and read limits back.

**New kernel/runtime concept.** cgroup v2 delegation, the no-internal-process constraint, and
atomic placement at clone time.

**Commands after this stage.** `--memory`, `--pids-limit`, `--cpus`, and a `mydocker inspect` that
reports **declared vs effective** limits.

**Do NOT support yet.** No cgroup v1 (`D5 §5.5` — *"a second, worse architecture"*; skip). No
`--blkio-weight` (see below). No `--device-cgroup-rule`, no eBPF device controller, and therefore
**no `--device`** (see risks). No `pause`/`freeze` yet — that is L7.

**Success test (devvm).**
1. `mydocker run --memory 100m --rootfs … -- <allocate 200 MB>`: the process is killed,
   `memory.events` `oom` and `oom_kill` increment, and the wait status is `SIGKILL`
   (`D5 §21` item 1).
2. Same with `--memory-reservation 100m` (`memory.high`) and `memory.max=max`: the process
   **survives** and `memory.events high` climbs instead. The difference between these two tests is
   the whole of `D5 §3.1`.
3. `--pids-limit 100` plus `:(){ :|:& };:` — the fork bomb is contained and the **VM stays
   usable**. `[F]` HIGH — `pids.max` is the only limit whose absence takes down the *host*
   (`D5 §19.1`).
4. **The atomicity test:** a child that forks immediately on entry; assert every descendant is in
   the container cgroup. `[F]` HIGH — between `clone` and a `cgroup.procs` write the child sits in
   the **parent's** cgroup and can fork, spike CPU or allocate past the limit; `V1 §F3` re-tiers
   `CLONE_INTO_CGROUP` from `D1 §9`'s "strongly recommended" to **required for correct resource
   limits**, and runc pays for the missing atomicity with a synchronisation protocol whose in-tree
   comment says exactly why: *"Do this before syncing with child so that no children can escape
   the cgroup"* (`V1 §C6` rule 4, `V2 §3.4`, both verbatim).
5. `mydocker inspect` on a container with no `--pids-limit` must report the **effective** value,
   not silence.

**Major risks.**
- **Reporting a limit the kernel is not applying.** `[O]` HIGH — every container on `host-a`
  has `pids.max = 19147` while `HostConfig.PidsLimit` is unset; the number is systemd's
  `DefaultTasksMax = 15%` of `threads-max` (19147/127653 = 0.14999…), re-measured on two
  containers in `V6 §D.2`. `docker inspect` does not surface it. **Our `inspect` must distinguish
  declared from effective**, and this stage is the only cheap moment to build that in.
- **`--device` has no kernel backstop under this design.** `[R]` HIGH — `V6 §G.2` **MISCLASSIFIES**
  `D5 §14.1`'s "~95% of the device cgroup's value": the substitute (drop `CAP_MKNOD` + deny
  `mknod` in seccomp + `nodev` rootfs) covers the malicious-image and in-container-`mknod` threat
  *completely* and covers **nothing** about devices the operator passes in, because the device
  cgroup gates `open()`, not creation. Therefore: **`--device` is gated on implementing the eBPF
  controller; it is not an option to ship without it.** Free hardening in the meantime: bind each
  passed-through node with `MS_NOSUID|MS_NOEXEC` and refuse block devices by default.

**Verified-claim dependencies.**
- `[F]` HIGH — the no-internal-process rule, including the paragraph most summaries drop: *"the
  restriction doesn't get in the way if there is no enabled controller in the cgroup's
  subtree_control"* (`V6 §C.1`, CONFIRMED verbatim against `cgroup-v2.rst` v6.11). The forced
  layout is `mydocker/` distributing with zero procs, `mydocker/<id>/` holding procs with an empty
  `subtree_control`.
- `[R]` HIGH — **name the path helper `container_cgroup(id).join("init")` now** (`V6 §C.4`
  endorsing `D5 §1.3`). With `CLONE_INTO_CGROUP` the child is *born* in the target cgroup, so
  adopting the `init/` sub-path today costs one extra `mkdir` and a different fd; adopting it later
  requires live-migrating a running container's PID 1.
- `[F]` HIGH — cgroup **placement** must precede the cgroup **namespace**: `cgroup_namespaces(7)`
  states that the process's current cgroup directory *becomes* the namespace root
  (`V1 §C6` rule 5 — upgraded from `D1`'s youki-comment citation to a kernel fact).
- `[F]` MED — `EBUSY` is the errno in both directions of the no-internal-process rule. The value is
  right but `D5 §1.3` tagged it `[F]` HIGH while `D5 §23` admits it was inferred (`V6 §C.2`,
  MISCLASSIFIED tag). `D5 §21` item 3 settles it with one write on the VM — do it here.
- `[R]` HIGH — no cgroup crate. cgroup v2 is `mkdir` plus `std::fs::write`; both `cgroups-rs` and
  youki's `libcgroups` are healthy crates whose value is v1 support and the systemd D-Bus driver,
  neither of which we want (`V7 §8.3`, confirming `D5 §18.1` and `D2 §12.8` agree independently).
  Note `std::fs::write` on a small buffer is one `write(2)` while `BufWriter` may split — and
  cgroup interface files are not stream files.
- `[C]` HIGH — if we ever add the systemd driver, it **contains** the fs driver rather than
  replacing it: `UnifiedManager.Set` calls `m.fsMgr.Set(r)` after the D-Bus call
  (`V6 §D.1`, CONFIRMED verbatim at runc v1.2.5). So build `FsManager` first; `SystemdManager` is
  a wrapper. Also copy `cgroupFilesToChown()` exactly — `host-a` publishes **five** files in
  `/sys/kernel/cgroup/delegate`, and hardcoding the historical three is a live bug on ≥ 4.15.
- `[R]` MED — **`io.weight` is not an anti-feature.** `V6 §N.3` MISCLASSIFIES `D5 §19.1`'s
  "never implement `--blkio-weight`" as a single-host observation generalised to a rule: on this
  host BFQ is `CONFIG_IOSCHED_BFQ=m`, one `modprobe` away, and iocost is merely unconfigured. And
  `io.weight` is **iocost's** file — BFQ's is `io.bfq.weight`, which runc writes first. Correct
  behaviour: probe both, implement the flag, and **refuse loudly** when neither backend is active
  — which is `D5 §5.4`'s own hard-fail-on-impossible policy, applied consistently.
- `[F]` MED — `--oom-kill-disable` does **not** error on v2; moby warn-and-discards it
  (`V6 §M.2`, REFUTED against `D5 §3.0`/`§3.1`; the error string `D5` quotes does not exist in the
  source). `--memory-swappiness` is likewise dropped *with a client-visible warning*, not silently.

**Trap.** Writing limits after placing the process. `[R]` HIGH — the cgroup is created and written
**before** the child exists (`D5 §20` P4 before P8), and `CLONE_INTO_CGROUP` makes even the
placement window vanish. `D2 §4`'s "child writes its own pid to `cgroup.procs` as its first act"
fallback is **`D2`'s invention** — no runtime does it, and it additionally requires the cgroupfs to
still be reachable by path from the child, which is only true *before* `pivot_root` and the
`put_old` detach (`V2 §I2`, caveat on rule 2a). Use the parent-write fallback instead.

---

### L7 — Lifecycle: supervisor, state, and a stop that actually stops

**Goal.** A supervisor process that outlives the CLI, a durable `state.json`, the
`create`/`start` rendezvous, and `stop`/`kill`/`rm`/`ps`/`logs`.

**New kernel/runtime concept.** The `exec.fifo` rendezvous, `PR_SET_CHILD_SUBREAPER`, `flock`
mutual exclusion, atomic state writes, and `cgroup.kill`.

**Commands after this stage.** `mydocker run` (**foreground**), `mydocker run -d`,
`create`, `start`, `ps`, `logs [-f]`, `stop`, `kill`, `rm`.

**Do NOT support yet.** No `attach`, no `-it`, no `exec`, no `events`, no restart policies, no
hooks, no user namespace, no networking beyond L2's empty netns.

**Success test (devvm).**
1. `mydocker run -- sh -c 'exit 42'` in the foreground exits 42; `mydocker run -d` returns an id
   and `mydocker ps` shows it; after exit `mydocker inspect` reports 42 **durably** (kill the
   supervisor's parent, re-read from disk).
2. **The stop test:** `mydocker run -d --rootfs … -- sleep 1000` then `time mydocker stop`. Expect
   ~10 s with a handler-less PID 1 and immediate with a shell that traps SIGTERM
   (`D5 §21` item 9). `[F]` HIGH — this is kernel behaviour, not Docker behaviour: an ancestor's
   `SIGTERM` to a namespace init with no handler is **discarded** (`V6 §E.2`, CONFIRMED verbatim,
   and `D1` measured the same — the two documents agree).
3. **The fork-bomb stop test:** `:(){ :|:& };:` under `pids.max=100`, then stop. `cgroup.kill` must
   empty the cgroup with no survivors; compare against the read-loop fallback with and without a
   preceding freeze (`D5 §21` item 5).
4. **The crash-consistency test:** `kill -9` the supervisor at five points in the create sequence
   and assert that `mydocker rm --force` always succeeds afterwards and leaves no cgroup, no mount
   and no state directory.
5. **The concurrency test:** two `mydocker run` with the same id race; exactly one wins, and the
   loser leaves nothing behind.

**Major risks.**
- **Failure-path ordering.** `[F]` HIGH — runc's deferred block is `OOMKillCount()` →
  **`terminate()`** → `manager.Destroy()` → `intelRdtManager.Destroy()`, with the in-tree comment
  *"Terminate the process to ensure we can remove cgroups."* `D2 §4` lists `Destroy` **before**
  terminate, which would fail to `rmdir` the cgroup — and contradicts `D2 §9.1`'s own analysis
  (`V2 §4`, CORRECTED; *"this one produces a bug if copied"*).
- **Draining stdio forever.** `[F]` HIGH — `V2 §2.2`/`§5.3` **REFUTES** `D2 §5.3`'s `[R]` HIGH
  "the supervisor must hold **both** ends of the container's stdout/stderr pipes", which `D2` calls
  *"the single most-copied trick in the fd table"*. The shim holds only the **read** ends of the
  container pipes (`lr-x` on fds 11 and 13); holding a write-end copy means never seeing EOF and
  hanging forever on the drain — breaking `D2 §6.5` step 3. Correct rule: **create the pipe, hand
  the write end to the child, close your own copy of the write end, never close the read end while
  the container lives.**
- **The lock.** Two design gaps `V2` found: `flock` is on the inode via the open file description,
  so `rm --force` unlinking `<dir>/lock` lets a second process lock a *new* inode and both believe
  they hold it — `fstat` the held fd and `stat` the path and compare `st_dev`/`st_ino`, or put the
  lock somewhere never removed. And **the supervisor must take the same lock for every state
  write**, which `D2 §7.2`'s diagram never shows: a `start` racing the supervisor's `exited` write
  resurrects a dead container as `running` (`V2` design gaps 1–3).

**Verified-claim dependencies.**
- `[R]` HIGH — **ship foreground `run` before `-d`.** This inverts `D2 §7.6`'s phasing.
  `V2 §7.4/§21` shows `D2 §7.4`'s "the CLI must never be in the ancestry, in any mode" quotes
  runc's `docs/terminals.md` *drawback* while omitting the same file's recommendation two
  paragraphs earlier — *"In general we recommend using foreground… Detached mode is difficult to
  get right"* — and directly contradicts `D2 §11.7`'s own `[R]` HIGH *"the container is a process,
  not a service."* The two cannot both be adopted. Foreground first is fewer moving parts (exit
  code is `waitpid`'s return; no exit file, no log rotation, no handshake), and `-d` is then the
  same code path after `setsid()` with stdio redirected to the log.
- `[F]` HIGH — the `exec.fifo` rendezvous, verbatim against runc `main`: `mkfifo(…, 0o622)` +
  `chmod` (the umask defence), `O_PATH|O_CLOEXEC` on the fifo, `readFromExecFifo`, state dir
  `0o711` (`V2 §3.2`, CONFIRMED). `[C]` MED — the FIFO-vs-`notify.sock` choice must be
  **re-derived**: `V2 §10.3` REFUTES the arithmetic that decided it (`/var/lib/mydocker/containers/
  <64-hex>/attach.sock` is 105 bytes *total*, fits in `sun_path`'s 108 with 2 to spare), and youki's
  `chdir`-then-`bind("notify.sock")` removes path length as a factor entirely. The FIFO may still
  win on its *other* reasons (mode `0622` across a userns; `start` needs only `open`+`read`; no fd
  must survive the create/start gap) — but say so.
- `[F]` HIGH — `cgroup.kill` is the only race-free stop: *"Killing a cgroup tree will deal with
  concurrent forks appropriately and is protected against migrations"* (`V6 §E.1`, `V2 §5.5`, both
  CONFIRMED from `cgroup-v2.rst`; `D2`'s `[F]` MED should be **HIGH**). Add the branch `V6 §C.5`
  closes with no experiment: in a **threaded** cgroup the write fails with **`EOPNOTSUPP`**.
- `[F]` HIGH — do not escalate by `SIGKILL`ing PID 1; write `cgroup.kill`. Killing PID 1 relies on
  the PID-namespace teardown guarantee, which is real but is a *second* mechanism doing the work
  and does not exist under `--pid=host` (`V6 §E.5`).
- `[F]` HIGH — namespace-init unkillability is **not durable**: a forced signal with a default
  disposition clears `SIGNAL_UNKILLABLE` outright in `force_sig_info_to_task()`. `V1 §E2` settles
  `D1`'s Open Question 4 and shows `D1`'s guessed mechanism (`sig_task_ignored()`'s `force` logic)
  was **wrong**. Practical consequence for exit-status handling: `unshare -pf sh -c 'kill -SEGV $$'`
  exits **0**, not 139, because a container's own `kill(1, SIGSEGV)` is still discarded.
- `[F]` HIGH — atomic state writes are `CreateTemp` → write → `Close` → `Rename`, with **no
  `fsync` and no lock file** in runc; the PID-reuse guard is `StartTime` plus a zombie/dead check
  (`V2 §6.1`, CONFIRMED byte for byte). Take the guard; add the `fsync` and the lock.
- `[R]` HIGH — **drop `external_fds` from the state model.** `V2 §4/§6.1` REFUTES `D2`'s claim
  that `external_descriptors` is how `runc exec` wires stdio: its only consumers are the state
  serializer and CRIU, and `D2` puts CRIU out of scope. A free simplification the corpus argued
  itself into carrying.
- `[F]` MED — the OCI spec's four status values are a **floor**, not a closed set; additional
  runtime-defined values are explicitly permitted (`V2 §1.1`, CORRECTED). But `unknown` is not a
  legal *reported* status — "we lost the exit code" is not a new runtime state, the container is
  `stopped`. Report `stopped` with a null exit code and keep `unknown` in a separate field.
- `[F]` HIGH — **failure atomicity is a spec MUST**, not an engineering preference:
  *"generating an error MUST leave the state of the environment as if the operation were never
  attempted."* `V2 §1.5` MISCLASSIFIES `D2 §1.5` for filing this under "latitude", and the
  correction upgrades all of `D2 §9` from good practice to conformance.
- `[R]` MED — persist a **best-effort idempotent cleanup**, not an undo log. netavark's
  `firewall/` directory is a real, shipping Rust precedent for the *mechanism* (a small per-entity
  directory of JSON records plus one `flock`), but it is a **replay** list, which is naturally
  idempotent; a rollback list is not (`ReleaseIp` run twice hands one address to two containers)
  (`V2 §9, Adjudication 2`). `D2`'s own "cheap alternative" — on `rm`, best-effort try every
  cleanup and report what failed — *is* the idempotent-by-construction version and is the right
  phase-1 answer for that reason.
- `[R]` MED — signals: prefer **`signalfd` + pidfd**, not pidfd alone. `V7 §5.3` REFUTES `D2`'s
  preference for the pidfd-only route: it loses the supervisor's own `SIGTERM` entirely (breaking
  `D2 §5.5`'s forwarding) and produces no wake for subreaped orphans. rustix has no `signalfd`, so
  this is a hand-written syscall plus `libc::sigprocmask` — more than `D2`'s "3-argument syscall".

**Trap.** Expecting `PR_SET_CHILD_SUBREAPER` on the supervisor to reap zombies **inside** the
container. `[F]` HIGH — it does not, and `V2 §5.4` **REFUTES** `D2 §5.4`'s stated benefit with a
live test on `host-a`: the kernel forbids cross-namespace reparenting, so an orphan created
inside the pidns — including by a `setns`'d `exec` process — is reaped by the **container's PID 1**
and by nothing else. containerd documents this two sentences after the quote `D2` used. What the
flag genuinely buys is reaping of orphans at the *host* pid-namespace level, which covers the
container init itself (confirmed in the same test) and host-side `exec` helpers. The consequence
is L13's in-container reaper.

---

### L8 — Content store and registry client: bytes, verified

**Goal.** `mydocker pull alpine` resolves the name, does the bearer handshake, walks
index → manifest → config, verifies every digest, and leaves verified blobs on disk. **No
extraction at all.**

**New kernel/runtime concept.** Content addressing; the OCI Distribution pull flow; an
`ingest/` → `rename(2)` staging discipline.

**Commands after this stage.** `mydocker pull <ref>`, `mydocker store put/ls/rm`.

**Do NOT support yet.** No extraction, no `run` from an image, no push, no Schema 1, no zstd, no
`urls`/foreign layers, no referrers, no tag listing, no deletion, no cross-repo mount
(`D3 §12.1`'s explicit not-on-the-list).

**Success test (devvm + network).**
1. `mydocker pull alpine` produces exactly three blobs (index-selected manifest, config, one
   layer) whose file names are their own digests, mode `0444`, and re-hashing each file reproduces
   its name.
2. The same pull against a **local `registry:2`** container — this is the offline oracle and it is
   what makes the stage testable without Hub's rate limit.
3. `kill -9` mid-download leaves **nothing** in `blobs/` (everything unfinished stays in
   `ingest/`).
4. **The cross-registry test**, which is the one that catches the real bug: pull the same digest
   from Docker Hub, GHCR and Quay and assert all three succeed. `[F]` HIGH — the three behave
   **completely differently** on `Accept` (`V3 §G.1`): Hub ignores it entirely, Quay
   content-negotiates and *synthesises Schema 1 on the fly* (three different digests for one tag),
   GHCR returns **`404`** for a Docker-only `Accept`.

**Major risks.**
- **Credential leakage to a host learned from image content.** `[F]` HIGH — `V3 §F.3` CORRECTS
  `D3 §2.8`'s account of CVE-2020-15157: it is not a redirect bug, it is a **`urls`/foreign-layer**
  bug — containerd fetched the attacker's URL, the attacker replied `401` with a Docker-style
  `WWW-Authenticate`, and containerd walked the token flow and handed over **registry credentials**
  (worse than a 5-minute scoped token). So there are **two** rules, and `D3` states only one:
  (a) strip `Authorization` on a cross-host redirect — and this is a spec **`MUST NOT`**, not a
  recommendation (`V3 §E.5`, MISCLASSIFIED in `D3`); and (b) **never run an authentication
  handshake against a host that came from image content.** Gate the token-fetch path on the host
  being the configured registry for this pull.
- **Trusting the HTTP `Content-Type` of a blob.** `[O]` HIGH — GHCR serves the *config blob* of
  `ghcr.io/distribution/distribution:latest` as `application/vnd.zarf.layer.v1.blob` (`V3 §E.7`).
  The descriptor wins, always.

**Verified-claim dependencies.**
- `[F]` HIGH — **branch on the response `Content-Type`, never on what you requested**
  (`V3 §A.1`, CONFIRMED six ways including two disjoint-`Accept` probes `D3` did not run). But
  the *reason* `D3` gives is wrong: the rule is not "registries ignore `Accept`" — Quay and GHCR
  very much do not — it is that **no registry's behaviour is predictable from the request**
  (`V3 §L`).
- `[R]` HIGH — send **all four** modern media types. This is load-bearing, not politeness: an
  OCI-only `Accept` on Quay yields Schema 1 (which we then hard-refuse, producing "image was
  pushed before 2017" for a modern 2026 image), and a Docker-only `Accept` on GHCR yields `404`
  ("image not found" for an image that exists) (`V3 §G.2`).
- `[F]` HIGH — **`404` is not proof of absence** and `401` means private-or-absent. Between them,
  neither is a reliable existence signal (`V3 §G.2`, `§C.1`).
- `[F]` HIGH — `Accept` can change **which content a tag resolves to** (three digests for one Quay
  tag). Any cache keyed on `(repo, tag)` must also key on the `Accept` set, or resolve once and pin
  the digest (`V3 §G.2` — an omission that changes the design).
- `[R]` HIGH — do **not** pre-flight `GET /v2/`; drive auth off the `401` of the resource you
  actually want. Sharper reason than `D3`'s round-trip saving: **GHCR's `/v2/` challenge carries a
  fake scope** (`repository:user/image:pull`), so a client that reuses it requests a token for a
  repository that does not exist (`V3 §G.3`).
- `[O]` HIGH — `HEAD` on a manifest is **free** against Hub's rate limit; `GET` is not (11 HEADs,
  zero decrements) (`V3 §C.5`, REFUTING `D3 §2.4`'s stated tradeoff). Prefer HEAD because it is
  never worse; do not *depend* on it — one IP, one day, no spec backing.
- `[F]` HIGH — the ChainID recursion is `Digest(…)`, **algorithm-agnostic**, not hardcoded
  `SHA256` as `D3 §4.5` transcribes it (`V3 §D.2`). Store chain IDs as `sha256:<hex>`, not bare hex.
  The formula reproduced Docker's `layerdb` directory names byte-exactly across **25 layers in 4
  images** (`V3 §D.3`).
- `[F]` MED — parser robustness: `history` is **OPTIONAL** in `config.md` and `manifest.layers` is
  **not marked REQUIRED** (`V3 §H`, `§E.9`). Do not assume either is present. And not every real
  platform manifest has an attestation partner — `busybox`'s `linux/arm/v6` and two
  `hello-world` `windows/amd64` entries have none — so match on `os == "linux"` **positively**
  rather than filtering `unknown/unknown` and taking what is left (`V3 §B.2`).
- `[R]` HIGH — **the puller is a separate process, structurally, not a separate module.**
  `V7 §8.2` REFUTES `D3 §13.1`'s third reason for choosing `ureq` ("keeps the option open of
  linking the puller into the supervisor"): `ureq` spawns a **detached OS thread per DNS
  resolution** whenever a `resolve` or `global` timeout is set, and that thread sits inside glibc
  `getaddrinfo`/NSS, which allocates, takes locks and `dlopen`s modules. It is lazy and
  conditional, so a `Threads == 1` assert passes on most runs. `D2 §8.3`'s *"structural beats
  disciplined"* exists for exactly this.

**Trap.** Fabricating a spec citation. `V3 §E.2`/`§E.3` found two in `D3`: the descriptor digest
is **not** documented as "a checksum of the content as it is transported" (that phrase appears
nowhere in the image spec), and the Distribution Spec contains **no `406 Not Acceptable`
requirement** — the string `406` appears once, inside a git SHA in an appendix URL. Both
substantive claims are true; both citations are invented. *"A verifier who checks one quote and
finds it invented will distrust the other forty, all of which are fine."*

---

### L9 — Extractor, `native` storage, and `mydocker run alpine`

> **The first stage a person could actually use.** Everything before this is infrastructure.

**Goal.** Apply layers from disk into a directory, safely, and hand that directory to L3's
`--rootfs` path. `mydocker run alpine /bin/sh` works end to end.

**New kernel/runtime concept.** The tar-changeset model, whiteouts, and the safety layer —
`openat2` resolution, `unlink`-before-create, single-component basenames.

**Commands after this stage.** `mydocker run <image> [cmd]`, `mydocker images`, `mydocker rmi`,
and `mydocker load ./oci-layout/` (see below).

**Do NOT support yet.** No layer sharing between containers — `native` **copies** the applied tree
per container, so `n` containers cost `n` copies. No overlay, no writable-layer semantics, no
`tar-split` (record the decision, see risks), no `push`, no `build`.

**Success test (devvm).**
1. `mydocker run alpine /bin/sh -c 'cat /etc/alpine-release'` prints a version.
2. The extracted rootfs matches `docker export` of the same image **byte for byte modulo mtimes**
   (compare on the VM, where Docker may be installed; `host-a` is read-only and must not be
   used for this).
3. **The negative-test corpus**, run with no network at all against hand-built malicious tars:
   `../../etc/passwd`, an absolute path, a symlink to `/etc` followed by a write through it, a
   hardlink to `/etc/shadow`, a device node, a setuid binary, `security.capability` and
   `trusted.overlay.opaque` xattrs, a PAX-size-desync entry, a 10 GiB decompression bomb. Each must
   produce a **specific named error**, not a generic failure and not a silent clamp.
4. `mydocker load ./alpine-oci/` from `skopeo copy docker://alpine:latest oci:./alpine-oci:latest`
   round-trips. `[R]` MED — `D3 §11.3`: ~200 lines because the store is already blob-shaped, and it
   makes **every later stage developable offline** with deterministic bytes and no rate limit.
   Pull that insertion forward to here rather than after storage, so L10–L12 never need Hub.

**Major risks.**
- **`tar-split` is the irreversible decision and this is the only moment to take it.** `[R]` HIGH —
  `V3 §L` **reranks** `D3 §11.5`: the storage keys are *not* the expensive lock-in (every
  content-address key is recomputable offline from data already on disk — `V3 §D.4`), but
  `tar-split` can only be captured **during extraction**, so retrofitting means re-extracting the
  entire store. Decide consciously. `D3 §11.5`'s recommendation is still to skip it, with the
  marker written down: `// ponytail: no tar-split; re-push will produce new blob digests`.
- **The xattr namespace is the second irreversible decision, and `D3` misses it entirely.**
  `[R]` HIGH — `V4 §V4-6.5`: a layer whose whiteouts and opaque markers are `trusted.overlay.*`
  cannot be used under a `userxattr` mount and vice versa. Recording it in `info.json` is necessary
  but does not make it cheap to change. Pick `trusted.overlay.*` for the rootful engine and treat
  a future rootless mode as a **re-extraction**, or key the store by
  `(chainID, xattr-namespace, idmap)` from day one.
- **`chown` destroys more than setuid bits.** `[F]` HIGH — `V3 §J` **REFUTES** `D3 §6.6`'s
  parenthetical "as root the `S_ISUID`/`S_ISGID` bits are *not* cleared by `chown`": since Linux
  2.2.13 root is treated like other users, measured on 6.11. And `chown(2)` also clears **all
  capability sets for the file** — so reversing the order silently destroys every
  `security.capability` xattr in every image, breaking `ping`, `newuidmap`/`newgidmap` and every
  capability-based setuid replacement, with the file mode looking untouched. `D3 §6.14`'s checklist
  order (`fchownat` → xattrs → `fchmodat`) is **necessary in exactly that order**; only the prose
  explaining it is wrong.

**Verified-claim dependencies.**
- `[F]` HIGH — all twelve `layer.md` whiteout quotes are verbatim, including the one that decides
  the extractor's shape: an opaque marker is applied *"regardless of the ordering in which the
  whiteout file was encountered"* (`V3 §I.1`). A naive single-pass streaming extractor that applies
  entries in stream order is **wrong**; collect opaque markers and apply them before other entries
  under that directory.
- `[F]` HIGH — the changeset rule is **unlink-then-recreate**, not overwrite-in-place, and that is
  what stops a regular file being written *through* an existing symlink (`V3 §I.1` — *"the
  strongest single argument in §5"*).
- `[F]` HIGH — Docker writes whiteouts as character devices `0/0`, mode `0000`, `root:root`: 44 of
  them found on disk, every one matching (`V3 §I.3` — direct evidence, not inference). And the
  xattr form additionally needs **`trusted.overlay.opaque = "x"`** on the parent directory, a row
  `D3 §5.3` omits; without it the kernel may not honour those whiteouts (`V3 §I.2`).
- `[F]` HIGH — `mknod` of a char device needs `CAP_MKNOD` and `trusted.*` xattrs need
  `CAP_SYS_ADMIN` (`V3 §I.4`, both verbatim). This is *precisely* what makes `docker pull` need
  privileges that `tar -x` does not.
- `[R]` HIGH — the three-part safety invariant, stated explicitly rather than as one decision:
  **`openat2(RESOLVE_BENEATH|RESOLVE_NO_MAGICLINKS|RESOLVE_NO_XDEV)` on the parent, plus a
  strictly single-component basename, plus unlink-before-create.** Drop any one and the hole
  reopens (`V4 §V4-3.2`, REFUTING "the whole answer"). Assert `!name.contains('/')` in the
  `SafeRoot` API's **type**, not in a comment. `fchmodat` on Linux has no working
  `AT_SYMLINK_NOFOLLOW`, so the unlink is what saves step 10.
- `[F]` HIGH — `tar >= 0.4.45` (RUSTSEC-2026-0067). And **the parser is not safe either**:
  RUSTSEC-2026-0068 / CVE-2026-33055, same crate, same versions, filed the same day, is a
  *parser-level* entry-smuggling bug — tar-rs ignores PAX size headers when the base header size is
  nonzero, making it an outlier among tar parsers (`V4 §V4-4.2`, REFUTING `D3 §13.3`'s central
  argument for the split). Mitigation: after each entry, assert bytes consumed == header size, and
  reject any entry where a PAX `size` record and the ustar header disagree.
- `[F]` MED — CVE attributions in `D3 §6.3`/`§13.3` are wrong: **CVE-2025-62518 is
  `astral-tokio-tar`**, not `async-tar` (which has no RustSec advisory at all); the family
  advisory is RUSTSEC-2025-0111 against `tokio-tar` (`V4 §V4-4.3`). And the "TARmageddon"
  mechanism is **per-entry PAX size desync**, not `TypeXGlobalHeader` — ignoring `g` headers is
  good hygiene but mitigates a different bug class (`V4 §V4-4.4`).
- `[R]` MED — `D3 §6.9`'s 5,000,000-entry cap is a number that cannot be violated without
  violating two other limits first (`V4 §V4-3.4`). Replace with a check against
  `statfs(store).f_ffree`, or drop it.
- `[R]` HIGH — keep `native` as a **permanent supported backend**, not a scaffold (`D3 §11.2`).
  `V4 §V4-7` strengthens this: it is the only backend that works for a single-layer image without
  special-casing (see L12).

**Trap.** Believing `RESOLVE_IN_ROOT` reports the attack. It does not — it **clamps silently**, so
a hostile layer entry named `../../etc/passwd` is created at `<root>/etc/passwd` with no error
raised anywhere (`V4 §V4-3.2`). Containment survives; image correctness does not. `D3 §6.1`'s
reject-don't-sanitise rule is therefore **load-bearing, not belt-and-braces**, and `D3 §6.2`'s
"structurally eliminated" for absolute paths overstates it.

---

### L10 — Networking A: routed NAT egress and the resolver minimum

> **Second departure from the brief's ladder**, and it is `V5`'s, not mine: the first networking
> rung that reaches the internet is **routed, with no bridge at all**.

**Goal.** A veth pair, a `/30` or `/31` per container, a default route via the host end,
`ip_forward`, one masquerade rule in **our own** nftables table, and `/etc/resolv.conf` rewriting.
`apk add curl` works.

**New kernel/runtime concept.** rtnetlink; the netns-bound socket; forwarding and source NAT;
and the fact that a fresh netns does **not** have the documented defaults.

**Commands after this stage.** `--network=none` (L2's empty netns, now a named mode),
`--network=host` (L2), and the new default: routed NAT egress.

**Do NOT support yet.** No bridge, so **no container↔container L2 adjacency and no shared
gateway** — each container costs an address and a host route. No published ports, so a container
is a client, not a server. No embedded DNS server, so no container-to-container by name. No IPv6.
No rootless.

**Success test (devvm).**
1. `mydocker run alpine sh -c 'apk add --no-cache curl && curl -sI https://example.com'` succeeds.
   This is the demo that means the engine exists (`D4 §11.2`).
2. `mydocker run alpine sh -c 'nslookup deb.debian.org'` succeeds **on a systemd-resolved host** —
   the VM must have systemd-resolved on, because that is the failure this stage exists to fix.
   `[F]` HIGH — the host's `/etc/resolv.conf` points at `127.0.0.53`, bound scoped to the host's
   `lo` device, and is meaningless inside a container netns (`V5 §E3`, CONFIRMED and sharpened —
   unreachable *twice over*).
3. **The leak test:** run and remove 50 containers, then diff `ip link`, `ip route`,
   `nft list ruleset` and `/proc/self/mountinfo` against a snapshot taken before. Zero residue.
4. **The state-ownership test:** `nft delete table inet mydocker` must remove **all** of our
   networking state in one statement. If anything survives, we wrote into a chain we do not own —
   see the trap.

**Major risks.**
- **Writing into chains we do not own.** `[O]` HIGH — `V5 §A2` independently re-verified the live
  production leak on `host-a` and ruled out six innocent explanations: an orphan bridge on
  `172.19.0.0/16` with zero ports, a surviving MASQUERADE rule and two anti-spoof DROPs, and
  **no record of any of it in boltdb**. `V5` then **CORRECTS** `D4 §9.3`'s diagnosis: a daemon
  restart would *not* have cleaned it, because `removeIPChains()` removes eleven **Docker-owned**
  chains and never touches built-in `nat/POSTROUTING` or `raw/PREROUTING`, where all three leaked
  rules live. The real lesson — *"the engine wrote state into chains it does not own, and then lost
  the key needed to find it again"* — is a sharper argument for `D4 §5.4`'s "one table of our own"
  than `D4` itself makes.
- **Release ordering is a security property.** `[R]` HIGH — `V2 §9, Adjudication 1` adjudicates
  against `D2 §9.1`'s severity table: a leaked IP is slow exhaustion and is the *low*-severity
  case; the dangerous mode is **premature release** — an address returned to the pool while a DNAT
  rule, a neighbour entry or a **conntrack entry** still points at it hands the next container the
  previous one's traffic. Annotate `Cleanup::ReleaseIp` as order-critical and **flush conntrack for
  the address between rule removal and release** — deleting a rule does not tear down existing
  flows.

**Verified-claim dependencies.**
- `[R]` HIGH — **the first genuinely useful rung is routed NAT egress, not bridge+NAT.**
  `V5 §F2`/`§F3` REFUTES `D4 §11.1`'s stage-2/stage-3 framing: delete the bridge and you still have
  full internet egress from veth + `ip_forward` + MASQUERADE. This is not hypothetical — it is
  exactly the **CNI `ptp` plugin** (with `ipMasq`), and **Docker 28's own `gateway_mode: routed`**
  (*"sets no NAT/masquerading but maintains firewall rules"*). Kubernetes clusters run on `ptp`.
- **What deferring the bridge buys, concretely** (`V5 §F3`): no `bridge-nf-call-iptables` question,
  no hairpin question, no STP/FDB, no bridge-MTU-inheritance surprise, and — the big one —
  container↔container traffic is *routed through the host*, so it traverses `filter/FORWARD` and
  per-pair policy is an ordinary rule. `D4 §3.4` spends a page on why `--icc=false` is hard; on a
  routed setup that difficulty does not exist. And there is **nothing in class C**: the veth is
  self-healing (`D4 §9.1`), while the bridge is the shared object with the "who deletes it?"
  problem whose answer is *"nobody does"* (`D4 §9.6`).
- Costs of routed, stated fairly because they are why Docker defaults to bridge (`V5 §F3`): an
  address or `/30` per container instead of one gateway for all; a host route per container; a
  routing hop instead of an L2 hop; and no broadcast/multicast between containers.
- `[F]` HIGH — **`ip_forward` does not do what `D4` says it does.** `V5 §A1` **REFUTES** the
  corpus's single most-repeated kernel claim (`D4 §1.1`, `§9.8`, `§11.2`): the `ip-sysctl.rst`
  sentence *"its change resets all configuration parameters to their default state"* is **stale
  documentation**, faithfully quoted. `inet_forward_change()` at v6.11 sets
  `conf/all/accept_redirects = !on`, propagates `forwarding` to `conf/default` and to every
  existing interface, disables LRO on every netdev, and flushes the route cache. **Nothing else.**
  `rp_filter`, `accept_local`, `route_localnet` and `arp_ignore` — the four `D4` names — are all
  untouched; live proof is `rp_filter = 2` surviving on a host where Docker set `ip_forward = 1`.
  A no-op write is a genuine no-op (`if (write && *valp != val)`).
  The one real, undocumented side effect `D4` missed: **`dev_disable_lro()` is permanent** and is
  not undone by setting `ip_forward` back to 0.
- `[F]` HIGH — a fresh netns **inherits** `ipv4_devconf` from `init_net`'s *current* values,
  governed by `net.core.devconf_inherit_init_net` (default 0), so `ip_forward` is often already 1
  and the documented "default 0" describes a freshly booted `init_net`, not a fresh child netns.
  `V5 §H1` **upgrades** this from `D4`'s `[LOW]` mechanism to `[F]` HIGH on a second host and a
  different kernel. **Read it; do not assume.**
- `[F]` HIGH — a netlink socket is permanently bound to the netns it was created in, and `setns`
  is per-thread — but **no dedicated OS thread is needed**, because the supervisor is
  single-threaded by construction. `V7 §8.1` resolves what it calls *"the highest-risk unstated
  assumption across the corpus"*: `D4 §2.6(4)` demands a thread and `D2 §8.3` forbids one, and
  neither document combines its own premises. Pattern: open `/proc/self/ns/net` first,
  `setns(container_netns_fd)`, create the socket, do the work, `setns(host_ns)` — with a `Drop`
  guard on every exit path and `panic = "abort"` so a panic cannot skip the restore.
- `[R]` HIGH — target **nftables natively**, in our own `table inet mydocker`. `V5 §D3` shows the
  direction of travel is further along than `D4` knew: netavark's iptables driver is **already
  removed** from `main` (the drivers are `nftables`, `firewalld`, `none`, with nftables the
  compile-time default), not merely deprecated. Two corrections to carry: Docker's nftables backend
  is documented **experimental** and, unlike the iptables backend, **does not enable IP forwarding
  itself**; and there is an operator-facing `--firewall-backend` daemon flag.
- `[F]` HIGH — **do not link `rustables`: it is GPL-3.0-or-later** (`V5 §B2`, `V7 §2.4`, confirmed
  on crates.io — 0.8.8, 2026-06-23). For a container runtime, where downstream embedding is normal
  and MIT/Apache-2.0 is universal, this is a project-level decision, not a dependency choice.
  Drive nftables the way both reference implementations do: `nft -f -` / `nft -j` text or JSON, not
  `libnftnl` (`V5 §D4`; note `nftnl` also carries RUSTSEC-2025-0126, a heap overflow — `V7 §13`
  row 17).
- `[R]` HIGH — async is **not** forced into the runtime core. `rtnetlink` has no synchronous
  constructor and hard-depends on `netlink-proto` + `futures-*`, but `netlink-packet-route`,
  `netlink-packet-core` and `netlink-sys` carry **zero** required async dependencies
  (`netlink-sys`'s are optional behind a default-empty feature set), and netavark takes exactly
  that route (`V5 §B1`, CONFIRMED all three legs). Caveat `V5` adds: netavark never calls
  `clone()`, so it witnesses the *crate* claim, not `D2 §8.3`'s thread-purity claim.
- `[R]` MED — set `net.ipv4.ip_unprivileged_port_start = 0` and widen `ping_group_range` **in the
  container netns**. Both are per-netns, free, and no host mutation. `V5 §G1` observed Docker doing
  both, unasked, on all five live containers with `HostConfig.Sysctls = null`.

**Trap.** Ranking the `ip_forward` write as the most dangerous operation in the ladder.
`D4 §11.2` does, and it is **REFUTED** (`V5 §A1`, `§I` finding 2). The most dangerous operation is
at L11 — the first `-p` publish on a host running a foreign firewall manager — and the reason is
in the next stage.

---

### L11 — Networking B: the bridge, published ports, and the firewall others own

**Goal.** A bridge with a shared gateway, published ports via DNAT, Docker-28-style anti-spoof
rules, and the userland helper only where NAT genuinely cannot work.

**New kernel/runtime concept.** L2 bridging; the `nat` hook chain and conntrack; the fact that our
rules live in a ruleset somebody else also writes to.

**Commands after this stage.** `-p host:container`, `--network <name>` (user-defined bridge
networks), container↔container by IP on the same bridge.

**Do NOT support yet.** No embedded DNS (L12 or later — container-to-container **by name** still
does not work). No `--icc=false` (see below). No macvlan/ipvlan. No IPv6 DNAT. No overlay/swarm.

**Success test (devvm).**
1. `mydocker run -d -p 8080:80 nginx` then `curl -s localhost:8080` from the host, and from
   another machine on the VM's network.
2. **The real-client-IP test:** nginx's access log shows the *client's* address, not the gateway's.
   `[F]` HIGH — inbound published-port traffic is not masqueraded, and this is derivable from the
   `-s <bridge subnet>` match on the MASQUERADE rule (`V5 §C2`, CONFIRMED).
3. **The over-publish test**, which is the finding most people miss: assert that a rule with no
   `-d` match publishes on **every** host address, including private ones. On `host-a` the
   `-A DOCKER ! -i br-… -p tcp --dport 80` rule genuinely covers the VPC address `10.0.0.5` and
   `eth1`'s `10.1.0.2` as well as the public one (`V5 §C2` — *"the most underappreciated finding
   in §4"*). Decide deliberately whether we do that; Docker does.
4. **The foreign-firewall test:** install `ufw` on the VM, `ufw deny 80`, then publish port 80 and
   confirm it is still reachable — then confirm our engine **warned** about it.
5. Re-run L10's leak test and `nft delete table inet mydocker`.

**Major risks.**
- **This stage carries the most host-state risk in the entire ladder, and the first `-p` is the
  single most dangerous operation.** `[F]` HIGH — `V5 §C5` CONFIRMS and **strengthens** `D4 §4.6`:
  not only does `ufw deny 80` fail to stop `-p 80:80`, but the knowledgeable admin's correct
  incantation `ufw route deny 80` **also fails**, because it lands in `ufw-user-forward` at FORWARD
  position 4, after `DOCKER-FORWARD` has already terminally ACCEPTed. **There is no ufw command at
  all that closes a published port on such a host**; the only lever is `DOCKER-USER`. `V5 §I`
  ranks this above every other operation in the ladder, replacing `ip_forward`. `[R]` HIGH — detect
  a foreign firewall manager on the first `-p` and warn loudly; this is a security finding with a
  blast radius of *"the user's server was exposed and nothing said so"*.
- **Stage-4 risk in `D4`'s own accounting** — rules naming a **specific container IP**, a **bound
  host port**, **conntrack** entries whose staleness defeats correct rules, a ruleset shared with
  `ufw`/`firewalld`/`fail2ban`, and per-container frequency (`D4 §11.2`, CONFIRMED `V5 §F4`).

**Verified-claim dependencies.**
- `[F]` HIGH — the hop order (`raw` −300 → conntrack −200 → `nat/PREROUTING` −100 → routing →
  `filter/FORWARD` 0 → `nat/POSTROUTING` 100), DNAT applying only to the first packet of a flow,
  and the reverse translation happening in conntrack rather than by any rule — all reproduced
  byte for byte on the live ruleset (`V5 §C1`, `§C2`).
- `[F]` HIGH — three independent firewall writers coexist on a stock Ubuntu server (Docker and ufw
  via `iptables-nft`, fail2ban via native nft at `priority filter - 1`), and two of the three are
  **invisible** to the other's tooling: `iptables -t filter -S | grep -ci f2b` → **0** while
  fail2ban is actively rejecting an address (`V5 §D1`, `§D2`). Within a table order is position;
  across tables at one hook priority decides; a `drop` anywhere is final and an `accept` is
  per-table.
- `[F]` MED — a foreign `-P FORWARD DROP` in `table ip filter` **still applies** even if our own
  table accepts, because policy is evaluated at the end of that base chain. The mitigation `D4`
  states is "warn"; `V5 §D4` adds the one it never joins up — nftables lets us put an `accept`
  ahead of the foreign chain in a table at a **lower priority number**, which iptables does not.
- `[F]` HIGH — Docker 28 writes one `raw/PREROUTING` anti-spoof DROP per container IP
  (`-d <ip> ! -i <its bridge> -j DROP`), at priority −300, before conntrack and before nat
  (`V5 §C4`, seven rules confirmed). Copy it.
- `[R]` HIGH — **gate the address-family helper on the host actually having an address in that
  family.** `V5 §C3` **MISCLASSIFIES** `D4 §4.4`'s headline: *"every IPv6 connection to this
  production web server is being carried by `docker-proxy`… this host proves it"* is vacuously
  true — the host has **no global IPv6 address at all**, only `fe80::/64`. The structural argument
  (a NAT rule cannot change address family) is a `[F]` about NAT, not an `[O]` about this box. What
  the host *does* prove is that Docker starts the v6→v4 helper **unconditionally**, giving two idle
  supervised processes per published port on the majority of hosts that have no IPv6. Do not copy
  that.
- `[F]` HIGH — do **not** use `route_localnet`; a `bind()` is free (`D4 §4.4`, CONFIRMED `V5 §C3`:
  `route_localnet` is 0 on every interface on the live host). The pre-28.0.0 L2 leak — *"hosts
  within the same L2 segment can reach ports published to localhost"* — is in Docker's own docs.
- `[F]` MED — global `bridge-nf-call-iptables = 1` and per-bridge `nf_call_iptables = 0` are two
  independent controls and both are live on the reference host (`V5 §H6`). `IFLA_BRPORT_ISOLATED`
  is exposed on 6.11 and every container veth reports `hairpin off` — so `--icc=false` is either a
  global module with conntrack side effects or an all-or-nothing per-port flag. **judgement:** ship
  neither; `D4`'s own answer (*"anyone needing real policy should use separate networks"*) is a
  limitation of the bridge, and L10's routed mode already gives per-pair L3 policy for free.
- `[C]` MED — `IFLA_NET_NS_FD` inside the `VETH_INFO_PEER` nest, which `D4 §2.2` rates *"Tradeoff:
  none that I found"*, is **UNSUPPORTED** — a strong no-downside claim about a non-obvious kernel
  path that nobody has round-trip tested (`V5 §I` finding 12), and `D4 §9.2` leans hard on it.
  Test it explicitly on the VM before relying on it.

**Trap.** Treating the bridge as the thing that makes networking useful. It is not (L10). What it
actually buys is exactly two things — container↔container **L2** adjacency and a single shared
gateway instead of a `/30` each — and it brings the whole `bridge-nf-call-iptables` / hairpin /
FDB / MTU-inheritance / `br-*`-leaks-forever tax with it (`V5 §F2`). `D4 §9.3`'s live production
leak *is a bridge*.

---

### L12 — Layered filesystem: OverlayFS, the writable layer, and GC

**Goal.** Stop copying a whole rootfs per container. Immutable layers keyed by `diff_id`,
snapshots keyed by chain ID, an OverlayFS mount with `upperdir`/`workdir` per container, and a
mark-and-sweep GC.

**New kernel/runtime concept.** OverlayFS stacking, copy-up, the `fsconfig`/`lowerdir+` mount API
and its fallback.

**Commands after this stage.** `--storage=overlay|native`, `mydocker gc`, and layer sharing —
pulling two images with a common base downloads and stores the shared layers once.

**Do NOT support yet.** No `metacopy`, `redirect_dir`, `index`, `xino`, `nfs_export` or `volatile`
— `D3 §8.7`'s "use none of them for MVP" is sound and `V4 §V4-5.7` confirms every one is accepted
at 6.11 and every one is a way to be surprised. No `diff`/`commit`/`build`. No rootless overlay.

**Success test (devvm).**
1. **The ordering test:** two hand-built layers with the same path and different contents;
   `lowerdir=A:B` must show A's version and `lowerdir=B:A` must show B's. `[F]` HIGH —
   `lowerdir == reverse(diff_ids)`, leftmost is topmost, CONFIRMED three independent ways in
   `V4 §V4-5.1` including an independent `layerdb` walk on a live production container.
2. **The write-isolation test:** write inside the container, then verify the image layer
   directories are unchanged; `rm` a file present in a lower layer and verify a **char 0/0** node
   appears in `upper`.
3. **The truncation guard test** — the most important one here: build an option string of 4094,
   4095 and 4096 bytes and assert the engine **refuses** above 4095 rather than calling `mount(2)`.
4. **The single-layer test:** `mydocker run alpine sh` must work. See risks — the naive done-when
   is impossible for it.
5. **The exclusivity test:** starting the same snapshot twice must return **our** error, not a
   second mount.
6. `mydocker gc` frees exactly the unreferenced blobs and layers and **never a mounted one** —
   consult `/proc/self/mountinfo` before unlinking.
**[wants old-kernel host]** — unprivileged overlay at RHEL 8's 4.18 and the `userxattr` path; the
motivating case for probe-don't-version-gate is still unverified (`V4 §V4-9`, `V6 §I.2`).

**Major risks.**
- **`mount(2)` does not fail on an over-long option string. It silently truncates.** `[F]` HIGH —
  `V4 §V4-2` is the most important correction in the storage domain and it **REFUTES** `D3 §8.5`:
  `copy_mount_options()` copies at most one page and `path_mount()` NUL-terminates at byte 4095;
  whatever errno you get is decided by what the *chopped* string happens to mean. Measured: **4095
  OK / 4096 FAIL**. `ENOENT` dominates (33 of 40 layouts), `ELOOP` is the minority, and — in a
  Docker-shaped `<store>/<id>/diff` layout, which is the shape we would use — **6 of 34 (18%)
  silently mount a wrong filesystem** with layers missing. `D3 §15.2` recorded this once and could
  not reproduce it; it reproduces deterministically per store layout.
  Worse: because truncation eats the **end**, `lowerdir=…,upperdir=…,workdir=…` loses its
  `upperdir` and `workdir` entirely and overlay then legally mounts **read-only** — measured, 139
  layers requested, 131 visible, writes `EROFS`. So: **a hard pre-flight refusal above 4095 bytes
  is mandatory, not a nicety**, and **put `lowerdir` last** so a bug here cannot silently drop
  `upperdir`/`workdir`. `D3 §8.5`'s `[R]` HIGH length check is hereby upgraded from "so the error
  message is nicer" to "so the filesystem is correct".
- **A read-only overlay needs ≥ 2 lower layers, so the naive done-when is impossible for `alpine`.**
  `[F]` HIGH — `V4 §V4-5.4` (`NEW`, `D3` does not record it): `lowerdir=<one layer>` with no
  `upperdir` fails `EINVAL` — *"at least 2 lowerdir are needed while upperdir nonexistent"* — and
  duplicating the layer to make two entries fails **`ELOOP`** ("conflicting lowerdir path").
  `alpine`, the running example throughout `D3`, has exactly **one** layer, so `D3 §11.4` step 6
  fails on the first thing anyone tries. Restate the acceptance criterion as *"read-only mount of a
  ≥ 2-layer committed stack; single-layer stacks fall back to `native` or to a bind mount of the
  diff"* — which is a third, independent reason to keep `native` permanent (L9).
- **The kernel will let two containers share an `upperdir`/`workdir`.** `[F]` HIGH — `V4 §V4-5.3`
  **REFUTES** `D3 §8.1`'s reliance on the doc's *"not allowed and may fail with EBUSY"*: measured
  at 6.11, both mounts returned **OK** with only a `dmesg` warning, and the doc's operative next
  sentence is *"the behavior of the overlay is undefined."* **We must keep our own mount
  bookkeeping** — an exclusive lock or refcount per snapshot. Starting the same container twice
  will silently corrupt its writable layer.

**Verified-claim dependencies.**
- `[F]` HIGH — `lowerdir+` **escapes** the page limit (500 layers / ~10 KB reproduced
  independently) but has three ceilings `D3` states none of: the layer cap is **exactly 500**
  (`#define OVL_MAX_STACK 500`, present since ≤ v5.10 — this **closes** `D3 §15.5`, and `D3`'s
  "500 verified to work" landed on the constant itself); each `lowerdir+` value is capped at **255
  bytes** (`strndup_user(_value, 256)`); and the **non-append** `fsconfig(SET_STRING,"lowerdir",…)`
  form is capped at 255 too, i.e. **16× worse than `mount(2)`** (`V4 §V4-1.1`). So "use the new
  mount API" is not the escape hatch; "use `lowerdir+`, one path per call" is.
- `[R]` HIGH — `lowerdir+` is a **probe-gated fast path, not a floor change** (`V6 §I.3`,
  `V4 §V4-1.4`). Try `fsopen("overlay")` + `fsconfig(…,"lowerdir+",…)`; on `ENOSYS`/`EINVAL`/
  `ENOPARAM` fall back to `mount(2)` with the 4095-byte refusal. The probe is L0's. Note the
  `FSCONFIG_SET_FD` form of `lowerdir+` is **v6.13** and `D3 §8.5` asserts it at `[F]` HIGH with no
  version — it is unavailable even on the reference host.
- `[R]` MED — `D3 §9.2` calls deleting Docker's `l/` short-link subsystem *"the largest single
  simplification available to us versus Docker's design"*. `V4 §M1` **MISCLASSIFIES** that as a
  recommendation contingent on a **6.7 kernel floor** the rest of the corpus does not grant,
  presented unconditionally. It is still a good idea, and `V4 §V4-6.2` strengthens it on different
  grounds: Docker **cannot reach its own `maxDepth = 128`** on the absolute path (~71 layers) and
  falls back to `mountFrom`, which is **not a fork-exec** as `D3 §9.2` claims but
  `unshare(CLONE_FS)` on a pinned OS thread — ~15 lines in Rust. So the thing being deleted is
  cheaper than advertised in both directions.
- `[R]` HIGH — the snapshotter interface must return a **`Mount` description carrying the list of
  lower layers**, not a pre-joined `lowerdir=` string and not a mounted path. `V4 §V4-6.6` makes
  `D3 §9.4`'s "idea 2" **not optional**: `lowerdir+` mounts are built with `fsopen`/`fsconfig` and
  then `move_mount`ed into the target namespace, so the storage layer physically cannot "return a
  path" without mounting in the wrong namespace — and pre-joining throws the whole `lowerdir+`
  benefit away at the interface.
- `[F]` MED — `workdir` "must be empty" is documented but **not enforced** at 6.11 (a stray file
  mounted fine); `workdir` **inside** `upperdir`, or equal to it, **is** enforced (`EINVAL`). So
  `snapshots/<id>/{fs,work}` is fine and `snapshots/<id>/fs/work` is not (`V4 §V4-5.2`).
- `[F]` MED — tmpfs **does** support `trusted.*` xattrs and works as an `upperdir` with the default
  namespace; what arrived late in tmpfs is **`user.*`** (v6.6). `V4 §V4-5.6` **REFUTES** `D3 §8.8`
  and notes the consequence: `/tmp` on the reference host is tmpfs, so every `[O]` overlay
  experiment in `D3 §8` ran on a filesystem `D3 §8.8` declares unusable.
- `[F]` MED — `volatile`'s marker does **not** "force a fresh upper on next mount"; the next mount
  **fails `EINVAL`** until the caller deletes the workdir (`V4 §V4-5.7`). Following `D3`'s sentence
  produces a stuck container on first use.
- `[C]` MED — `D3 §11.5`'s "Docker carries `cache-id` **because** it cannot rename the overlay2
  directories" is **MISCLASSIFIED** — inference presented as observation. `cache-id` lives in the
  graphdriver-*agnostic* layer store and the source says migration (`RegisterByGraphID`, the
  Docker 1.10 content-addressability migration), not a filesystem constraint (`V4 §V4-6.4`). The
  keying recommendation is right; the stated reason is not.
- `[R]` MED — skip Docker's `-init` layer and use bind mounts instead (`D3 §9.3`, no objection in
  `V4 §V4-6.3`).

**Trap.** Treating the `mount(2)` fallback as "~20 lines that just cap usable layer depth"
(`D3 §8.5`). It caps nothing. Unchecked, it **silently produces a wrong filesystem** — and, per
`V4 §V4-8`, the length guard belongs in **this** stage's done-when, not in a later hardening pass:
it is the difference between "fails loudly on kernels < 6.7" and "silently corrupts on kernels
< 6.7".

---

### L13 — `exec`, `-it`, and the in-container reaper

**Goal.** Enter a running container, allocate a pty correctly, and stop leaking zombies.

**New kernel/runtime concept.** `setns` on a pidfd; the console socket and `SCM_RIGHTS`;
subreaping inside the container.

**Commands after this stage.** `mydocker exec [-it] <id> <cmd>`, `mydocker run -it`,
`mydocker attach`, `mydocker --init`.

**Do NOT support yet.** No `events`, no restart policies, no checkpoint/restore.

**Success test (devvm).**
1. `mydocker run -it alpine sh` gives a working shell with correct terminal size, and `SIGWINCH`
   resizes it.
2. **The pty-provenance test:** `ls -l /proc/<container pid>/fd/0` resolves into the **container's**
   `devpts` instance, not the host's (see risks).
3. **The reaper test:** run a container whose PID 1 is `sleep` (does not reap), `exec` a process
   that forks and orphans a child, and assert the zombie count in the container stays **zero**.
   Then repeat without `--init` and assert the zombie appears — the difference is the feature.
4. **The `exec`-membership test:** an `exec`'d process must appear in the container's
   `cgroup.procs` even though a walk of the process tree rooted at container PID 1 **misses it**
   (`V6 §E.3`).

**Major risks.**
- **Allocating the pty on the host.** `[F]` HIGH — `V2 §5.2` **REFUTES** `D2 §5.2`'s sequence and
  `D2 §7.2`'s diagram, which put `openpty` in the supervisor before `clone()`. runc allocates the
  pty **inside the container, after `pivot_root`**, from the container's own `/dev/ptmx`, and sends
  the **master** *up* the socket via `SCM_RIGHTS`, with an in-tree comment naming the reason:
  *"consoles are scoped to a container properly (see runc#814)."* A host-devpts pty inside a
  container leaks a host resource across the isolation boundary and gets the ownership and
  permission model wrong. **If `-it` is implemented from `D2 §7.2`'s diagram it will be built the
  wrong way round.** Correct shape: supervisor creates a `socketpair`, passes one end to the child;
  the child allocates the pty after `pivot_root`, `dup`s the peer onto 0/1/2, and sends the master
  back.
- **The reaper is required, not a nicety.** `[F]` HIGH — following from L7's trap: orphans created
  inside the pidns reparent to the **container's PID 1** and no host-side flag can help, so a
  container whose PID 1 is `nginx` or `bash` accumulates zombies from every `exec`
  (`V2 §5.4`, `§design gap 4`). `docker-init 0.19.0` exists on the reference host and `D2`'s own
  version block records it, yet `D2 §1–§11` never mentions `--init` or the problem it solves. It is
  ~30 lines and it is the only fix.

**Verified-claim dependencies.**
- `[F]` HIGH — since Linux 5.8, `setns` on a **pidfd** moves the caller into *one or more* of the
  target's namespaces **atomically**: `setns(fd, CLONE_NEWUSER|CLONE_NEWNET|CLONE_NEWUTS|…)`.
  `V1 §C2` marks `D1 §3.1`'s "each call changes just one" **OUTDATED** and notes the practical
  win: `exec` is **one** `setns` on the pidfd we already keep, not six `open`+`setns` pairs, which
  removes a partially-joined failure state that the six-call version cannot recover from. Also
  missing from `D1 §3.1`: joining a mount namespace needs **both** `CAP_SYS_CHROOT` and
  `CAP_SYS_ADMIN`, and fails if the caller shares `CLONE_FS` state.
- `[F]` HIGH — never give a container a file descriptor referring to the invoking terminal;
  `TIOCSTI` injects into the host shell, and detached pass-through hangs the container if stdout
  or stderr is a pipe (`V2 §5.2`, CONFIRMED verbatim from runc `docs/terminals.md`).
- `[R]` HIGH — add `PR_SET_DUMPABLE(0)` (or a sealed memfd) on the `exec` path. `V6 §N.2` item 4
  notes `D5 §11.1`'s design 2 ("never exec our binary in container ns") does **not** cover
  `mydocker exec`.
- `[F]` HIGH — the cgroup, not the process tree, is the authoritative membership list. An `exec`'d
  process's parent lives in the **host** pid namespace, so `find_new_reaper()` reparents it to host
  PID 1, not container PID 1 (`V6 §E.3`, CONFIRMED with the mechanism `D5` did not give). Qualifier
  `V6` adds: it is still *in* the container's pidns, so `zap_pid_ns_processes()` kills it when
  container PID 1 dies — it is a leak that survives a tree-walking **stop implementation**, not one
  that survives the container.

**Trap.** Reaching for `PR_SET_CHILD_SUBREAPER` again. See L7's trap: it is the right flag on the
supervisor for host-level orphans and it does **nothing** for the problem this stage exists to
solve.

---

### L14 — OCI runtime-spec compatibility, hooks, and conformance claims

**Goal.** Expose the five operations over a real bundle so `mydocker` can stand in for `runc`, and
run the official pull-conformance suite.

**New kernel/runtime concept.** None. This stage is a serialization and lifecycle-semantics
contract, not an architecture (`D2 §1.5`, CONFIRMED `V2 §1.5`).

**Commands after this stage.** `mydocker create|start|state|kill|delete --root <dir> --bundle
<dir>`, `--console-socket`, `--pid-file`; `createRuntime`/`poststop` hook support.

**Do NOT support yet.** No push, no build, no CRIU, no referrers API, no `_catalog`.

**Success test (devvm).**
1. Drive our binary from **real containerd** on the VM by setting `binary_name` in
   `options.json` — the interop surface is a plain string (`V2 §2.3`, confirmed on the live host).
   If containerd can create/start/kill/delete a container through us, we are compatible in the only
   sense that matters.
2. Run `github.com/opencontainers/distribution-spec/conformance` against a local `registry:2` and
   against Hub (`D3 §12.2` `[R]` HIGH — *"the only credible evidence for the claim"*).
3. Assert `mydocker state` never reports `unknown` (`V2 §1.1`).

**Major risks.** Claiming conformance we do not have, and implementing a hook table that is a spec
version out of date.

**Verified-claim dependencies.**
- `[F]` HIGH — **`poststart` hook failure is now fatal.** OCI runtime-spec **v1.3.0** (released
  2025-11-04) changed step 9 from *"MUST log a warning… continue"* to *"MUST generate an error,
  stop the container, and continue the lifecycle at step 12."* `V2 §1.1/§1.4` marks `D2`'s table
  **OUTDATED** and notes that implementing from it would be non-conformant **in the unsafe
  direction** — leaving a container running that the spec says must be stopped. Only `poststop`
  retains warn-and-continue.
- `[F]` HIGH — `startContainer`'s `path` MUST resolve in the **container** namespace, i.e. it is a
  *container* binary (the image's `ldconfig`). Only `createContainer` is the hard
  host-binary-in-container-namespace case, so `D2 §1.4`'s reason for deferring both is half true
  (`V2 §1.4`).
- `[F]` HIGH — `root.path` may be **bundle-relative** and a directory MUST exist there; and the
  runtime **MUST mount `mounts` entries in the listed order** — an ordering MUST absent from
  `D2 §4`, whose entire subject is ordering (`V2 §1.3`, `§design gaps 6–7`).
- `[R]` HIGH — **interoperability before paper conformance.** `D3 §12`'s framing — *"what is the
  cheapest thing that lets us run a real `alpine` from Docker Hub"* — is the right one and
  `V3 §K.4` endorses it. The ten-item interoperability minimum was already complete at **L9**;
  this stage is where we earn the right to *say* "conformant", which is a different and much later
  thing. **judgement:** the ordering consequence is that OCI runtime-spec compatibility is the
  *last* rung, not an early one, because nothing a user does depends on it.
- `[R]` MED — **support** hooks for other people's plugins but **do not build networking as a
  hook** (`D4 §11.2` `[R]` HIGH, CONFIRMED `V5 §F4`). Three properties decide it: `createRuntime`
  failure is fatal (good), `poststop` failure is *specified* to be ignored (which throws away every
  teardown-ordering guarantee L10–L11 depend on), and **a hook has no return channel** — *"an
  allocation that cannot be reported is not an interface, it is a side effect."* Keep the internal
  trait CNI-shaped (ADD/DEL/CHECK/**GC**); `V5 §F4` calls the observation that CNI's `GC` verb *is*
  the reconcile pass we need *"a genuinely good structural insight"*.

**Trap.** Writing the hook-failure table from `D2 §1.4`. It is the v1.0.2–v1.2.0 table. Fetch the
tag you are targeting.

---

### L15 — Beyond the ladder: user namespaces and rootless

Not a rung, and deliberately so. `D5 §19.2` tiers the user namespace as *"hardening (the big
one)"* and `V6 §N.7` is blunter: the two capability-drop deviations *"do not change the runtime's
security class… Only a user namespace changes the class."* Everything above ships with container
root **=** host root.

What must be true **before** L4 ships so that this is a fill-in rather than a rewrite:

- `[R]` HIGH — "who writes `uid_map`/`gid_map`" is a **named participant** in the create path, e.g.
  a `write_id_maps(child_pid)` function called by the supervisor, even though the root-only engine
  never calls it (`D2 §7.5`, `D5 §17.3` — "the single seam").
- `[F]` HIGH — the map write is a **parent** action and must complete before the child's `execve`,
  because capabilities gained from `CLONE_NEWUSER` do not survive `execve`
  (`V1 §C7`, CONFIRMED derivation). `setgroups=deny` before `gid_map` (`V1 §C6` rule 3).
- `[F]` HIGH — `CLONE_NEWUSER` cannot be combined with `CLONE_PARENT`, so in the three-stage
  topology the userns **must** come from a separate `unshare`, never from the clone flags
  (`V1 §C8`, `§F4` — a hard constraint `D1 §9.2` states nowhere). And `unshare(CLONE_NEWUSER)` from
  a chroot is `EPERM` since 3.9.
- `[F]` HIGH — orphan reparenting lands on the init of the **father's** pid namespace, so in the
  `unshare` topology orphans can reparent *outside* the container — which does not happen in the
  `clone3` MVP topology (`V1 §E3`, an elided qualifier in `D1 §7.3`).
- Storage: `V4 §V4-6.5` — the store must then be keyed by `chainID + idmap`, not chainID alone,
  unless idmapped mounts (5.12+) are adopted; and the xattr-namespace lock-in from L9 bites here.
- Networking: `[F]` HIGH — the hard wall is `rtnl_link_get_net_capable()` →
  `netlink_ns_capable(skb, net->user_ns, CAP_NET_ADMIN)`; moving a veth into the **host** netns
  from an unprivileged userns fails `EPERM` (`V5 §G2`, kernel source verified even though the
  observation host is unreachable). The honest answers are pasta/slirp or `--network=host`.

---

## C. Where this ladder departs from the brief's example, and why

The brief's example was:
`execute process → isolated process → filesystem-isolated container → cgroups → lifecycle →
networking → OCI images → registry pull → layered filesystem → hardened security → OCI runtime
compatibility`, and it explicitly asked for that to be challenged. Eight departures.

### C1. Security is not rung 10. It is rungs L4 and L5, immediately after filesystem isolation.

This is the largest change and the one the corpus argues for hardest.

The usual case for late hardening is "it's polish; the container works without it". **Three
findings say otherwise, and each says it differently:**

1. **Several "hardening" items are required for correctness, not polish.** `V6 §J.2` **re-tiers**
   `CAP_MKNOD` from `D5 §19.2`'s "hardening (deviation)" to **required**, because it is the
   load-bearing compensation for declining the eBPF device controller — restore it later and the
   device story collapses silently. `V1 §F3` re-tiers `CLONE_INTO_CGROUP` from "strongly
   recommended" to **required for correct resource limits**, because without it a limit simply does
   not hold for anything the child forks before placement. `D5 §19.2` already tiers the seccomp
   allowlist, the bounding-set drop, masked `/proc`, read-only `/proc/sys`, read-only
   `/sys/fs/cgroup`, `close_range` and `openat2` path resolution as **required** — that is not a
   hardening list, it is the definition of the artefact.

2. **The failure mode of doing it late is silent, not visible.** `V6 §B.4` / `§N.2` item 1: both
   normative listings in `D5` put `setuid` before `PR_CAPBSET_DROP`, which makes every
   `PR_CAPBSET_DROP` fail `EPERM` and leaves the container with the **full 41-capability bounding
   set**. Nothing errors, nothing logs, `docker inspect` looks fine. Contrast a missing cgroup
   limit, which is visible the moment anyone looks, or missing networking, which the user notices
   immediately. **judgement:** a stage whose absence is invisible must not be scheduled behind
   stages whose absence is obvious, because the obvious ones will keep getting prioritised.

3. **The code lives in the same window.** Every item in L4 and L5 — the capability sequence,
   `no_new_privs`, `close_range`, the masked and read-only paths, the seccomp install — sits
   between the last mount and `execve`, i.e. inside the exact function L3 just wrote, with five
   kernel-forced orderings running through it (`D5 §20`). Cgroups, by contrast, are a **parent**
   action (`D5 §20` P3–P5) and are structurally additive. So doing security immediately after L3 is
   the *smaller* diff, and deferring it means re-opening and re-ordering the most delicate code in
   the project later, under the worst conditions (with users).

**What this costs, honestly.** Between L3 and L6 there are no resource limits, so a fork bomb or a
memory hog takes down the dev VM. That is acceptable **only because the VM is destroyable by
construction**, and it should be stated in L4's own README text. If that trade is unacceptable to a
particular team, the minimal alternative is to hoist `pids.max` alone (three writes and a `mkdir`)
into L3 — not to move the whole of L6.

**What this does not buy.** L5 does not make the engine safe for hostile images. See §A2.

### C2. "cgroups" and "lifecycle" keep their relative order but move behind security.

No other change. `D5 §19.1` and `D2 §6` are unaffected.

### C3. Networking is three things, not one rung

- `[R]` HIGH — `--network=host` is a **permanent orthogonal mode**, not a stage you grow past
  (`D4 §11.1`, CONFIRMED in substance `V5 §F1`). Putting it on a ladder implies supersession that
  never happens.
- The "new netns with `lo` up" rung is **folded into L2**, because `CLONE_NEWNET` is one of the six
  flags L2 already passes and bringing `lo` up is one `RTM_NEWLINK`. It does not deserve a rung of
  its own; `--network=none` is what it is called afterwards.
- `[R]` HIGH — the first genuinely useful rung is **routed NAT egress with no bridge**
  (`V5 §F2`, **REFUTING** `D4 §11.1`'s own corrected ladder). `D4` is right that the bridge stage
  is where the payoff is and wrong about which ingredient produces it: veth + `ip_forward` +
  MASQUERADE + `resolv.conf` rewriting is a complete `apt-get`-capable container network, and it is
  precisely CNI's `ptp` plugin and Docker 28's `gateway_mode: routed`. So L10 (routed) and L11
  (bridged) are separate rungs, and everything expensive about bridging — `bridge-nf-call-iptables`,
  hairpin, FDB/STP, MTU inheritance, the `br-*` device that leaks forever — is paid for at L11 and
  only if wanted.
- `[R]` MED — `V5 §F3` also **REFUTES** `D4 §11.1`'s label for its own stage 2 ("a test fixture
  rather than a feature"). Routed is a shipping model that Kubernetes clusters run on. Keeping
  "test fixture" in the document risks skipping straight to a bridge and inheriting the whole
  bridge tax for a benefit (L2 adjacency) a single-host engine may never need.
- The DNS minimum moves **into** L10 rather than being a separate final rung: `D4 §11.1` `[R]` MED
  already says its stage 5's first two thirds belong at stage 3, and it is thirty lines that decide
  whether `apt-get update` works at all on Ubuntu/Fedora/Arch (`V5 §E3`).

### C4. Registry client before extractor, and layered storage after networking

- `[R]` HIGH — the registry client (L8) comes **before** the extractor (L9), not after
  (`D3 §11.1`). Fusing them means debugging HTTP auth, media-type dispatch, digest verification,
  decompression and tar safety simultaneously, and it means every extractor test needs a network.
  Split, the extractor's whole hazard corpus runs offline against hand-built tars.
- **judgement, and it is a departure from `D3 §11.4` as well as from the brief:** the layered
  filesystem (L12) comes **after** networking, not immediately after extraction. `native` storage
  is correct and merely wasteful — it copies a rootfs per container, which nobody notices at n = 5
  on a dev VM — whereas without egress an image cannot install anything. Ranking by user-visible
  payoff per unit of risk puts networking first. The counter-argument (the layers are already on
  disk after L9, so overlay is the natural next step) is real but is about developer convenience,
  not user value.
- `[R]` MED — `D3 §11.3`'s OCI image-layout import is pulled **forward into L9** rather than
  sitting between storage steps, so that L10–L12 can be developed entirely offline against
  deterministic bytes with no Hub rate limit.

### C5. "Execute process" is two rungs.

L1 (process model, exit status, the `Threads == 1` invariant) and L2 (namespaces). They fail
differently and test differently, and L1's invariant is what `V2 §8.1` shows is the *only* thing
protecting a raw-`clone3` design from nondeterministic allocator hangs.

### C6. There is a rung 0 the brief has no analogue for.

The primary test target **does not exist yet**, and the corpus's strongest single recommendation —
probe features, never version-gate (`V1 §D1c`, `V6 §I.2`) — needs somewhere to live before the
first feature depends on it. L0 is the VM plus `mydocker probe`.

### C7. OCI runtime compatibility stays last, but "conformance" is re-scoped.

`D3 §12` frames the question as *"what is the cheapest thing that lets us run a real `alpine` from
Docker Hub"*, and `V3 §K.4` endorses the framing. That ten-item interoperability minimum is
complete at **L9** — i.e. the valuable half of "OCI compatibility" is delivered nine rungs early,
and what remains at L14 is the *claim*, plus the `runc`-drop-in CLI surface. **judgement:** state
this explicitly in the README, because "OCI compatible" is otherwise read as a single binary
milestone that the project appears not to reach for a very long time.

### C8. Foreground `run` ships before `run -d`.

`D2 §7.6` phases `-d` first. `V2 §7.4/§21` shows this is inverted against `D2`'s own analysis: the
document contains both *"the CLI must never be in the container's ancestry, in any mode"*
(`§7.4`, `[R]` HIGH) and *"the container is a process, not a service — foreground execution with
the CLI as supervisor… adopt"* (`§11.7`, `[R]` HIGH), 400 lines apart, and only `§11.8` admits the
choice is *"a priced choice"*. Foreground is strictly less machinery for the same feature set, and
`-d` is the same code path after `setsid()` with stdio redirected to the log. L7 ships both, in
that order.

---

## D. Reversibility: what each stage locks in

`PHASE4-risks-and-reconciliation.md` does not exist yet. **When it lands, reconcile this table
against it rather than re-deriving it**; the rows below come from `V3 §D.4`/`§L`, `V4 §V4-6.5`,
`D3 §11.5`, `D2 §7.5` and `D5 §17`.

| Stage | Locks in | Reversible? | Cost of changing later |
|---|---|---|---|
| L0 | nothing | fully | — |
| L1 | the sync, single-threaded process model | **hard to reverse** | Introducing a threaded runtime around `clone()` and removing it later means restructuring everything it touched (`D2 §7.5`, "the rewrite-risk decision"). |
| L2 | slave-vs-private propagation for `/` | fully | One flag; record it as a choice, since `V1 §F2` shows mature runtimes disagree and `pivot_root(2)`'s own example takes the option `D1 §9.3` forbids. |
| L3 | the rootfs mount set | fully | Additive. |
| L4 | the shape of the child's final sequence; **the rootless seam** | mostly | The sequence is cheap to reorder while it is one function. The seam (`write_id_maps` as a named participant) is one function signature now and a rewrite later (`D2 §7.5`, `D5 §17.3`). |
| L5 | the seccomp profile format | fully | A profile is data. |
| L6 | the cgroup **path layout** | cheap now, expensive later | `container_cgroup(id).join("init")` costs one `mkdir` today; retrofitting requires live-migrating a running container's PID 1 (`V6 §C.4`). |
| L7 | **who is the container's parent** | **not reversible** | Adding a socket to a supervisor is cheap; changing the parent is not (`D2 §7.5`). Also: one module owns all state writes from day one, or adding transactions later means finding every writer. |
| L8 | store keying (`blobs/` by compressed digest, `layers/` by `diff_id`, `snapshots/` by chain ID) | **reversible — this is a correction** | `V3 §D.4` **demotes** `D3 §11.5`'s "expensive to change": every content-address key is recomputable offline from data already on disk. Keep the recommendation (it matches both incumbents); drop the justification. |
| L9 | **`tar-split` capture** | **NOT reversible** | Can only happen during extraction; retrofitting means re-extracting the whole store (`V3 §L` ranks this first; `D3 §11.5` ranked keying first). |
| L9 | **the xattr namespace layers were extracted with** | **NOT reversible** | `trusted.overlay.*` and `user.overlay.*` layers are mutually unusable; switching means re-extracting the store (`V4 §V4-6.5`, a fourth lock-in `D3 §11.5` misses). |
| L9 | "everything on one filesystem" | expensive later | Assumed by every atomic `rename` and by overlay's workdir rule; relaxing it means copy-instead-of-rename paths everywhere (`D3 §11.5`). |
| L10 | **one nftables table of our own** | cheap now, painful later | Writing into built-in chains puts our state outside our own reconciliation — the mechanism behind the live production leak (`V5 §A2`). `delete table inet mydocker` must be sufficient. |
| L11 | the published-port model (all-addresses vs specific) | fully | A rule shape. |
| L12 | overlay vs native as the default | fully | `native` stays supported permanently (`D3 §11.2`), so this is a flag. |
| L13 | nothing structural | fully | — |
| L14 | the five-operation internal API | cheap now, expensive later | `[R]` — make the internal API literally `create/start/state/kill/delete` plus `exec` from day one; otherwise exposing them later means inventing a translation layer (`D2 §7.5`). |

---

## E. What this ladder assumes and cannot yet verify

1. **The dev VM does not exist.** Every success test above is written to be runnable on it and none
   of them has been run. L0 is not optional scaffolding; it is the precondition for every other
   stage's done-when.
2. **`old-host` is unreachable** (`old-host.example.edu` → NXDOMAIN, re-confirmed independently in four
   verification files). Three stages *want* it — L0, L3, L12 — and none is blocked. The commands
   that would settle each are in `V1 §D1c`/`§D10`, `V3 §I.4` and `V4 §V4-9`.
3. **`PHASE4-risks-and-reconciliation.md` does not exist**, so §D was derived from the verification
   files directly. Reconcile when it lands.
4. **Two claims this ladder leans on are still `UNSUPPORTED`.** `IFLA_NET_NS_FD` inside the
   `VETH_INFO_PEER` nest, rated *"Tradeoff: none that I found"* by `D4 §2.2` and leaned on hard by
   `D4 §9.2`, has never been round-trip tested (`V5 §I` finding 12) — L11's success test should
   include it. And `D3 §7.7`'s `[F]` HIGH that unlinking a live overlay `lowerdir` silently succeeds
   is unsourced and untested (`V3 §K.3`); L12's GC must consult `/proc/self/mountinfo` regardless.
5. **Two `D2` open questions need a Rust toolchain** that neither the macOS workstation nor the
   production host can provide: whether a compiled binary actually keeps libc off rustix's syscall
   path, and whether the needed functions are reachable under our feature set (`V7 §14`). Both are
   ten minutes on the dev VM and both are worth doing **before any `Cargo.toml` is written** —
   i.e. they belong to L0.
6. **The `no_new_privs` breakage inventory is unpriced.** `D5 §21` item 11 is listed as optional;
   `V6 §J.1` says it should be **blocking**, because this is the deviation whose cost the corpus
   has genuinely not measured. It gates L4.
