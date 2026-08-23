# Rust Container Engine — Research Folder

R&D-only research toward a Docker-like container engine written in Rust from first
principles (owning the runtime core, not shelling out to runc/crun/containerd).

**Status: Phase 2 partially complete — stopped early on token budget, 2026-08-23.**
Nothing here is a specification and no code has been written. The final deliverable
(`RND.md`) does **not** exist yet.

---

## Files

| File | Domain | Lines | State |
|---|---|---:|---|
| `00-RESEARCH-PLAN.md` | The approved plan: 5-domain decomposition, per-domain questions, environment reality, execution phases | ~330 | Complete |
| `D1-kernel-isolation.md` | Namespaces, `clone`/`unshare`/`setns`, mount propagation, `pivot_root`, rootfs mounts, PID 1 semantics, creation sequence, post-`clone` Rust safety | 1960 | **Complete** (§0–14, incl. Handoff / Open questions / Sources) |
| `D2-runtime-architecture.md` | OCI Runtime Spec, Docker→containerd→shim→runc boundaries, `create`/`start` split, detached I/O, state persistence, architecture recommendation, sync-vs-async, error atomicity, runc/crun/youki, architecture families | 1374 | **Partial** — §1–11 written; missing the dedicated Rust-substrate crate evaluation, Handoff, Open questions, consolidated Sources |
| `D4-networking.md` | netns lifecycle, veth over rtnetlink, bridge model, full `-p 8080:80` trace, iptables-vs-nftables, DNS, container-to-container, rootless networking | 1421 | **Partial** — §0–8 written; missing §9 host-state-that-outlives-us, §10 IPAM, §11 the minimal networking ladder, §12 Rust crates, Handoff / Open questions / Sources |
| — | **D3 — OCI images, registries, layered filesystems** | 0 | **Not written.** Agent had completed research and was beginning to write when stopped |
| — | **D5 — cgroups v2, capabilities, seccomp, LSM, rootless privilege model** | 0 | **Not written.** Same |

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

1. **Finish Phase 2** — run D3 (images/registries/storage) and D5 (cgroups/security/rootless)
   from scratch; finish the missing tail sections of D2 and D4 listed in the table above.
2. **Phase 3 — adversarial verification.** Seven *fresh* agents (never the original authors),
   one per claim cluster: kernel isolation, runtime architecture, OCI, storage/images,
   networking, resource control & security, Rust implementation. Their job is to **falsify**,
   not confirm: check primary sources, find outdated information, surface source
   disagreements, separate fact from recommendation, and flag weak evidence. The `[F]/[C]/[R]/[O]`
   tags and confidence markers are the input list.
3. **Phase 4 — architecture risk pass.** Classify each rewrite-risk decision `DECIDE NOW` /
   `DEFER SAFELY` / `EXPERIMENT FIRST`. The plan already promotes two: the kernel feature
   floor, and the root-required assumption.
4. **Write `RND.md`** at the project root — priority index (P0/P1/P2 by architectural
   importance, each entry linking to its section), the 22 required sections, the detailed
   `mydocker run alpine /bin/sh` creation trace, and the implementation ladder where every
   stage states goal, new concept, commands supported, explicit non-goals, success test, and
   risks.

**Stop condition for the R&D effort:** `RND.md` written. No `SPEC.md`, no task list, no code.
