# Rust Container Engine — Research Folder

R&D toward a Docker-like container engine written in Rust from first principles (owning the
runtime core, not shelling out to runc/crun/containerd).

**Status: R&D COMPLETE.** All four phases done. The deliverable is
[`../RND.md`](../RND.md) — start there; it is the digest and the router into this folder.
No code has been written, and none should be until a specification session has read `RND.md`.

> **A second, independent run of this same brief exists** in
> [`pass-2/`](pass-2/README.md): the plan in `00-RESEARCH-PLAN.md` was executed twice by
> separate sessions with no visibility into each other. `00-RESEARCH-PLAN.md` and
> `D1-kernel-isolation.md` are byte-identical between the two; everything after them
> diverged. Neither run supersedes the other, and `pass-2/README.md` lists the four points
> where they substantively disagree.

**Read order for a fresh session:** `RND.md` Priority Index → the individual `RND.md` section
you need → only then the `research/` file that section points to.


> **Note on host names.** The hosts in this corpus are anonymised. `HOST-A` / `host-a` is a
> modern x86_64 Linux host (Ubuntu 24.10, kernel 6.11.0-29, root, cgroup v2, native overlay,
> `iptables-nft`, AppArmor, live Docker) used **read-only** as an observation lab. `OLD-HOST` /
> `old-host` is an unprivileged Rocky 8.8 / kernel 4.18 HPC login node. Addresses shown are in
> RFC 5737 documentation ranges, and container, bridge, veth and MAC identifiers are
> pseudonymised. Every measurement, kernel version and command is otherwise unaltered.

---

## Corpus

| File | Domain | Lines | Claims |
|---|---|---:|---:|
| `00-RESEARCH-PLAN.md` | Approved plan: 5-domain decomposition, per-domain questions, environments, phases | 331 | — |
| `D1-kernel-isolation.md` | Namespaces, `clone`/`unshare`/`setns`, mount propagation, `pivot_root`, rootfs mounts, PID 1 semantics, creation sequence, post-`clone` Rust safety | 1960 | 216 |
| `D2-runtime-architecture.md` | OCI Runtime Spec, Docker→containerd→shim→runc boundaries, `create`/`start` split, detached I/O, state, architecture recommendation, sync-vs-async, error atomicity, runc/crun/youki, architecture families, **Rust substrate** | 1933 | 479 |
| `D3-images-storage.md` | OCI specs, pull trace, media types, layer format, whiteouts, extraction hazards, CAS, OverlayFS, overlay2/snapshotters, progression, conformance, crates | 2700 | 375 |
| `D4-networking.md` | netns lifecycle, veth over rtnetlink, bridge, `-p` trace, iptables-vs-nftables, DNS, rootless, host-state leaks, IPAM, ladder, crates | 2133 | 422 |
| `D5-resources-security.md` | cgroup v2, delegation, limit mapping, stop semantics, kernel floor, capabilities, seccomp, userns, `/proc` masking, escapes, LSMs, devices, rootless seam, crates | 3544 | 336 |

**1,828 tagged claims** — 477 `[F]` fact, 306 `[C]` convention, 655 `[R]` recommendation,
390 `[O]` empirical observation — each carrying `HIGH`/`MED`/`LOW` confidence. 58 are marked
LOW and are the priority targets for verification.

---

## Test environments

The dev machine is macOS, so no kernel claim is locally verifiable.

| Target | Reality | Job |
|---|---|---|
| **Local Linux VM** — *does not exist yet* | Modern kernel, cgroup v2, root, local disk, destroyable | **Primary.** All destructive stages, especially networking. Every ladder success test targets this |
| **`ssh host-a`** | Ubuntu 24.10, kernel 6.11.0-29, root, cgroup v2 all controllers, native overlay, `iptables-nft` + `nft` 1.1.0, AppArmor, docker+containerd+runc live | **Read-only observation lab.** ⚠️ `HOST-A`, live traffic on :80/:443 — never mutate |
| **`ssh old-host`** | Rocky 8.8, kernel 4.18, no root, cgroup v1 no delegation, no subuid, NFS home | **Degradation probe — UNREACHABLE.** `old-host.example.edu` → NXDOMAIN mid-session (resolved earlier the same day; likely campus DNS/VPN). All Rocky claims after D1 are inherited and marked as such |

Fixed decisions: Rust; Linux only; x86_64 first without needless arch coupling; cgroup v2;
**root-first, rootless later**; eventual OCI compatibility.

---

## Two deviations from Docker awaiting sign-off

Recorded as recommendations, not yet decisions:

1. **`no_new_privs` ON by default** (Docker sets it off). Collapses the two-branch seccomp
   install into one and neutralises setuid binaries; breaks `sudo`/`su`/`ping` loudly.
2. **Drop `CAP_MKNOD` and `CAP_NET_RAW` from defaults.** `CAP_NET_RAW` breaks in-container
   `ping`; D4 may want it back.

---

## Phase 3–4 output

| File | Lines | Contents |
|---|---:|---|
| `V1`–`V7` verification files | 7,058 | Adversarial pass by seven fresh agents: **419 confirmed, 111 corrected, 74 refuted, 52 unsupported, 49 misclassified, 25 outdated** |
| `PHASE4-risks-and-reconciliation.md` | 1,747 | Reconciliation ledger (196 entries) + 28-item risk register (22 DECIDE NOW, 2 DEFER, 4 EXPERIMENT FIRST) |
| `TRACE-container-creation.md` | 1,372 | The 42-step annotated `mydocker run alpine /bin/sh` trace |
| `LADDER-implementation-stages.md` | 1,657 | The 16-rung ladder, L0–L15 |

**Important:** the `D1`–`D5` research files were **deliberately not edited** after
verification. Where a `V*` file contradicts a `D*` file, **the verification wins**. Reading a
`D*` file alone will give you claims now known to be wrong — check the matching `V*` file, or
use the ledger in `PHASE4` Part 1.

## Next session

Write `SPEC.md` from `RND.md`. Before deciding anything in the risk register, run the four
experiments in `RND.md` §20 — they need the dev VM, and take under an hour together. One of
them (the `no_new_privs` breakage inventory) is **blocking** for a shipped default.
