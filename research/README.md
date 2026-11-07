# Rust Container Engine — Research Folder

R&D-only research toward a Docker-like container engine written in Rust from first
principles (owning the runtime core, not shelling out to runc/crun/containerd).

**Status: COMPLETE. `RND.md` is written at the repo root (11,702 lines).**
Phase 2 produced 13,596 lines across five domains; Phase 3's seven independent
adversarial verifiers produced 11,078 more and corrected ~120 claims; Phase 4
classified the rewrite-risk decisions. Nothing here is a specification and no code
has been written — that was the stop condition and it holds.

**Read `RND.md` first.** This folder is its provenance: `RND.md` carries the corrected
corpus, these files carry the per-claim pinned citations and the reasoning behind each
correction.

---

## Files

| File | Domain | Lines | State |
|---|---|---:|---|
| `00-RESEARCH-PLAN.md` | The approved plan: 5-domain decomposition, per-domain questions, environment reality, execution phases | 331 | Complete |
| `D1-kernel-isolation.md` | Namespaces, `clone`/`unshare`/`setns`, mount propagation, `pivot_root`, rootfs mounts, PID 1 semantics, the corrected 33-step creation sequence, post-`clone` Rust safety | 1960 | Complete |
| `D2-runtime-architecture.md` | OCI Runtime Spec, Docker→containerd→shim→runc boundaries, `create`/`start` split, detached I/O, state persistence, architecture recommendation, sync-vs-async, error atomicity, runc/crun/youki, architecture families, Rust substrate | 1902 | Complete |
| `D3-images-storage.md` | Three OCI specs, full pull trace, media types, dual-digest model, whiteouts, extraction hazards, CAS + GC, OverlayFS, overlay2/snapshotters, ladder, crates | 2458 | Complete |
| `D4-networking.md` | netns lifecycle, veth over rtnetlink, bridge model, full `-p 8080:80` trace, iptables-vs-nftables, DNS, host-state leaks, IPAM, ladder, crates | 2455 | Complete |
| `D5-resources-security.md` | cgroup v2, delegation/systemd, limit mapping + telemetry, stop/kill, v1-vs-v2 detection, kernel floor, capabilities, seccomp, userns/ID mapping, `/proc`+`/sys`, escape classes, LSMs, `--privileged`, devices, isolation distinction, rootless, crates | 3828 | Complete |
| `VERIFIER-CONTRACT.md` | The shared contract every Phase 3 verifier follows: verdict scheme, priority order, source rules, read-only host rules, output format | 124 | Complete |
| `V1-verify-kernel-isolation.md` | Adversarial: D1 §1–§11 — namespace tiers, the 33-step sequence, `/proc` requirement, `pivot_root` preconditions, PID 1 semantics, the post-`clone` window | — | Complete |
| `V2-verify-runtime-architecture.md` | Adversarial: D2 §1–§11, §14 — is `create`/`start` a Go artefact, the §7.2 supervisor design, `poststart` in spec v1.3.0, exit-status collection, concurrency | — | Complete |
| `V3-verify-oci-registries.md` | Adversarial: D3 §0–§3, §12 + D2 §1 — the `root` required/optional conflict, the `Accept`-header claim, the pull trace, media types, conformance targets, dual digests | — | Complete |
| `V4-verify-storage-overlay.md` | Adversarial: D3 §4–§11, §13–§15 — the 4095-byte page limit vs genuine `fsconfig`, OverlayFS kernel doc, whiteouts, extraction hazards + CVEs, CAS/GC | — | Complete |
| `V5-verify-networking.md` | Adversarial: D4 all — the rootless veth/bridge conflict, the `-p 8080:80` trace, nftables verdict semantics, DNS, host-state leaks, IPAM | — | Complete |
| `V6-verify-resources-security.md` | Adversarial: D5 §0–§18 — `cgroup.kill`, OOM detection, delegation + `DefaultTasksMax`, seccomp branch count, capability ordering, userns mapping, escape CVEs, devices | — | Complete |
| `V7-verify-rust-substrate.md` | Adversarial: the Rust substrate across D1 §11, D2 §12, D3 §13, D4 §12, D5 §19 | 2259 | Complete |
| `RND-STYLE-CONTRACT.md` | The house style every `RND.md` drafter followed: the Fact/Convention/Recommendation rule, how verified corrections are carried, citation rules | 113 | Complete |

Every claim in the written files is tagged, which is what makes the adversarial pass cheap
to target later:

- `[F]` **Fact** — what Linux/OCI actually requires, primary-source cited
- `[C]` **Convention** — what Docker/containerd/runc happen to do
- `[R]` **Recommendation** — what our engine should do, with the tradeoff stated
- `[O]` **Observation** — empirically verified; names host + kernel + exact command

plus `HIGH` / `MED` / `LOW` confidence. Tag counts so far: 222 `[F]`, 170 `[C]`,
223 `[R]`, 231 `[O]`.

### Sub-threads that were nearly finished (output not saved)

The domain agents had delegated three sub-investigations of their own. All were killed
before writing anything to disk, but each reported being at the write-up stage — so these
are close-to-complete research threads, not fresh ground:

- **Tar extraction hazards and CVEs** (feeds D3) — path traversal, symlink/hardlink escape,
  device nodes, setuid bits, xattrs, decompression amplification, and the real CVEs
- **Rust substrate crate evaluation** (feeds D2) — `nix` vs `rustix` vs raw `libc`,
  `oci-spec-rs`, the youki crates; had flagged an unverified repo-ownership change worth
  re-checking
- **Rust crates for cgroups / seccomp / capabilities** (feeds D5) — `cgroups-rs`,
  `seccompiler` vs `libseccomp`, `caps`, and whether cgroupfs needs a crate at all

---

## Test environments (probed, not assumed)

The dev machine is macOS, so no kernel claim is locally verifiable. Three targets:

| Target | Reality | Job |
|---|---|---|
| **Local Linux VM** (Lima/UTM) — *does not exist yet* | Needs modern kernel, cgroup v2, root, local disk, destroyable | **Primary.** All destructive stages, especially networking |
| **`ssh my-droplet`** | Ubuntu 24.10, kernel 6.11.0-29, root, cgroup v2 with all controllers delegated, native overlay, `iptables-nft` 1.8.10 + `nft` 1.1.0, AppArmor on, docker+containerd+runc live | **Read-only observation lab.** ⚠️ This is `AIKYNETIX-PROD-NY` serving live public traffic on :80/:443 via `docker-proxy` — never mutate iptables/nft/links, never start or stop containers, never write under `/sys/fs/cgroup` |
| **`ssh uc-arcc`** | Rocky 8.8, kernel 4.18, **no root**, **cgroup v1 with no delegation** (Slurm-owned), no `/etc/subuid` entries, NFS home, SELinux off. Works anyway: userns, PID/mount/net namespaces, and — via a RHEL backport — **overlay mounted inside a userns** on local `/tmp`. podman/apptainer/runc/fuse-overlayfs installed | **Degradation probe:** rootless + old kernel + cgroup v1 + no subuid |

Decisions already fixed: Rust; Linux only; x86_64 first without needless arch coupling;
cgroup v2 preferred; **root-first, rootless later**; eventual OCI compatibility.

Because of the environment split, two items became genuine research questions rather than
assumptions: the **minimum kernel floor**, and how the engine **detects and degrades** when
there is no writable cgroup at all.

---

## Resuming

Remaining work from `00-RESEARCH-PLAN.md`:

1. ~~**Finish Phase 2**~~ — done. All five domain files complete.
2. ~~**Phase 3 — adversarial verification.**~~ *Done — 11,078 lines, ~120 corrections.* Seven *fresh* agents (never the original
   authors), one per claim cluster, all following `VERIFIER-CONTRACT.md`. Their job is to
   **falsify**, not confirm. The `[F]/[C]/[R]/[O]` tags and confidence markers are the input
   list. Named cross-file contradictions handed out for adjudication: the `caps` take-vs-avoid
   split (D2 §12.5 vs D5 §19.2), the three-way overlay-in-userns kernel floor (D1/D3/D5),
   rootless veth+bridge (D4 §8 vs D5 §17), the seccomp branch count (D1 §11 vs D5 §9),
   `root` required-vs-optional in `config.json` (D2 §1.3 vs D3 §1), and `nftnl` vs `nftables`.
3. ~~**Phase 4 — architecture risk pass.** Classify each rewrite-risk decision `DECIDE NOW` /
   `DEFER SAFELY` / `EXPERIMENT FIRST`. The plan already promotes two: the kernel feature
   floor, and the root-required assumption.~~ *Done — `RND.md` §18–§20.*
4. ~~**Write `RND.md`** at the project root — priority index (P0/P1/P2 by architectural
   importance, each entry linking to its section), the 22 required sections, the detailed
   `mydocker run alpine /bin/sh` creation trace, and the implementation ladder where every
   stage states goal, new concept, commands supported, explicit non-goals, success test, and
   risks.~~ *Done — 11,702 lines, 22 sections, 42-entry priority index, an 11-stage ladder.*

**Stop condition reached.** `RND.md` is written. No `SPEC.md`, no task list, no code — and
none should be produced from this folder without a fresh specification session.

**The cheapest next action is not research.** A substantial list of decisive, fast tests is
blocked on one missing thing: a local destroyable Linux VM. See `RND.md` §21.1.
