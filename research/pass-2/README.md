# Pass 2 — an independent second research run

This folder is a **second, independent execution of the same research brief** that produced
[`../../RND.md`](../../RND.md) and [`../`](../README.md). Two sessions ran the plan in
[`../00-RESEARCH-PLAN.md`](../00-RESEARCH-PLAN.md) separately, with no visibility into each
other's output, and both finished. Neither supersedes the other and nothing was discarded.

**If you only read one thing, read [`RND.md`](RND.md) in this folder or
[`../../RND.md`](../../RND.md) at the repo root.** They are two renderings of the same
deliverable and they agree on the overwhelming majority of it. Where they disagree, the
disagreements are listed below rather than silently reconciled — a reader who needs to act on
one of those points should look at both and decide.

---

## Why two runs are worth keeping

The brief asked for adversarial verification: fresh agents whose job was to falsify, never the
agents that wrote the research. Two independent runs give something a single run cannot — the
places where two separate adversarial passes over the same primary sources reached *different*
conclusions. Those places are the ones worth re-testing first, because at most one of them is
right and the cost of finding out is an afternoon on a VM.

---

## What is in this folder

| File | Lines | What it is |
|---|---:|---|
| `RND.md` | 11,701 | The deliverable, **self-contained**: 22 sections, a 42-entry priority index sorted by architectural importance, the 51-step creation trace in §4, and the eleven-stage ladder in §17 — all in one file |
| `D2-runtime-architecture.md` | 1,902 | OCI Runtime Spec, Docker→containerd→shim→runc boundaries, `create`/`start`, detached I/O, state, sync-vs-async, error atomicity, runc/crun/youki |
| `D3-images-storage.md` | 2,458 | Three OCI specs, full pull trace, media types, dual-digest model, whiteouts, extraction hazards, CAS + GC, OverlayFS, overlay2/snapshotters |
| `D4-networking.md` | 2,455 | netns lifecycle, veth over rtnetlink, bridge model, full `-p 8080:80` trace, iptables-vs-nftables, DNS, host-state leaks, IPAM |
| `D5-resources-security.md` | 3,828 | cgroup v2, delegation, limit mapping, stop/kill, kernel floor, capabilities, seccomp, userns, `/proc` masking, escape classes, LSMs, devices, rootless seam |
| `V1`–`V7` | 11,078 | The adversarial verification pass, one file per claim cluster |
| `VERIFIER-CONTRACT.md` | 124 | The contract every verifier followed: verdict scheme, priority order, source rules, read-only host rules, output format |
| `RND-STYLE-CONTRACT.md` | 113 | The house style every `RND.md` drafter followed: the Fact/Convention/Recommendation rule, how a verified correction is carried into prose |

**Shared with pass 1, so not duplicated here.** `00-RESEARCH-PLAN.md` and
`D1-kernel-isolation.md` are **byte-identical** between the two runs — both sessions started
from the same approved plan and the same kernel-isolation study, and diverged after it. Read
them at [`../00-RESEARCH-PLAN.md`](../00-RESEARCH-PLAN.md) and
[`../D1-kernel-isolation.md`](../D1-kernel-isolation.md).

The two contract files have no pass-1 equivalent. They are the reusable method rather than
scaffolding — they are what kept thirteen independent agents producing comparable output, and
they are the part of this run most likely to be useful on an unrelated project.

---

## How the two renderings differ in shape

|  | Pass 1 (repo root) | Pass 2 (this folder) |
|---|---|---|
| `RND.md` | 54 KB **index** that routes into satellite files | 828 KB, **self-contained** |
| Creation trace | `../TRACE-container-creation.md`, 42 steps | `RND.md` §4, 51 steps |
| Ladder | `../LADDER-implementation-stages.md`, 16 rungs `L0`–`L15` | `RND.md` §17, 11 stages |
| Risks / reconciliation | `../PHASE4-risks-and-reconciliation.md`, 28-item register | `RND.md` §18–§21 |
| Section headings | the same 22 | the same 22 |

The step and rung counts differ because the two runs chose different granularity, not because
they describe different sequences. Both traces cover the same `mydocker run alpine /bin/sh`.

---

## Where the two runs actually disagree

These are substantive and a decision rests on each one. Listed so neither file silently
contradicts the other.

**1. The kernel floor.** Pass 1 sets the floor at **5.10** and says *probe for features, never
version-gate*. Pass 2 sets a **primary target of 6.7 with a hard minimum of 5.15**, on the
grounds that genuine `fsconfig(2)` escapes OverlayFS's 4095-byte options page and arrived in
6.7, so below that line overlay2's short-symlink indirection has to be reimplemented.

*These are less opposed than they look.* Pass 1's mechanism is right — probing beats
version-gating in every case — and pass 2's target is a statement about which code paths you
are willing to write. Probe, and treat 6.7 as the line above which the storage design gets
simpler.

**2. Where layered storage sits in the ladder.** Pass 1 puts OverlayFS at **L12, after
networking**, arguing that flat per-image extraction is correct-but-wasteful while egress is
blocking. Pass 2 puts it at **stage 7, before networking**. Both state their reasoning; pass
1's is the stronger argument on user-visible payoff and pass 2's on keeping the filesystem work
contiguous.

**3. Where security sits.** Pass 1 moves it early and explicit, to **L4–L5**, and says so as a
deliberate departure from convention. Pass 2 has it at **stage 5**. Substantively the same
call, reached independently — which is the most useful kind of agreement in the corpus.

**4. First genuinely useful rung.** Pass 1 says **L9**; pass 2 says **stage 6**. Same claim,
different numbering: everything before it is infrastructure with no user-visible payoff, and
both runs chose to say so rather than pretend otherwise.

---

## Anonymisation

The hosts in this folder are anonymised on **exactly the same scheme as pass 1**, which is
verifiable: after the substitution, `00-RESEARCH-PLAN.md` and `D1-kernel-isolation.md` are
byte-identical to their pass-1 counterparts.

`HOST-A` / `host-a` is a modern x86_64 Linux host (Ubuntu 24.10, kernel 6.11.0-29, root,
cgroup v2, native overlay, `iptables-nft`, AppArmor, live Docker) used **read-only** as an
observation lab. `OLD-HOST` / `old-host` is an unprivileged Rocky 8.8 / kernel 4.18 HPC login
node. Addresses shown are in RFC 5737 documentation ranges, and the observed application's
project, container and image names are pseudonymised (`webapp-*`, `ghcr.io/example-org/*`).
Container ids, veth names and MAC addresses are **not** pseudonymised in this folder — they
are ephemeral local identifiers for containers that no longer exist. Every measurement, kernel
version and command is otherwise unaltered.

---

## Claim tagging

Every claim in these files carries a tag and a confidence level, which is what made the
adversarial pass cheap to target:

- `[F]` **Fact** — what Linux/OCI actually requires, primary-source cited
- `[C]` **Convention** — what Docker/containerd/runc happen to do
- `[R]` **Recommendation** — what this engine should do, with the tradeoff stated
- `[O]` **Observation** — empirically verified; names host, kernel and the exact command

plus `HIGH` / `MED` / `LOW`. A `[R]` cannot be falsified; its *stated reasoning* can be, and
`VERIFIER-CONTRACT.md` says so explicitly because it is the rule verifiers got wrong most often.

`RND.md` carries only the **corrected** corpus. Where verification overturned a claim, the
original appears nowhere except as a `Trap.` note in the place a reader would otherwise
rediscover it. The D-files are deliberately left unedited after verification, so where a `V*`
file contradicts a `D*` file, **the verification wins**.

---

## The blocker both runs reached

Neither run could test destructively. The research ran against two machines, neither suitable:
one is live production and read-only, the other has no root, cgroup v1 and no delegation.

`RND.md` §21.1 lists thirteen decisive, fast experiments blocked on one missing thing — a local
destroyable Linux VM — including the only finding that blocks the MVP creation sequence
(`copy_cgroup_ns()` rooting a new cgroup namespace at the *parent's* cgroup, which is
source-derived and never measured). Each unverified claim says so in place and names its test.
