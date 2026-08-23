# rustainer

A Docker-like container engine written in Rust from first principles — owning the
runtime core (namespaces, cgroups, overlayfs, networking), not shelling out to
runc/crun/containerd.

**Status: R&D phase.** No code yet. All work currently lives in [`research/`](research/README.md).

## Research

See [`research/README.md`](research/README.md) for the full state of the investigation:

| Domain | Topic | State |
|---|---|---|
| D1 | Kernel isolation — namespaces, mount propagation, `pivot_root`, PID 1 | Complete |
| D2 | Runtime architecture — OCI spec, create/start split, state persistence | Partial |
| D3 | OCI images, registries, layered filesystems | Not written |
| D4 | Networking — netns, veth, bridges, port publishing | Partial |
| D5 | cgroups v2, capabilities, seccomp, rootless privilege model | Not written |

Remaining roadmap: finish Phase 2 (D3/D5 + missing tail sections), adversarial
verification pass, architecture risk pass, then write `RND.md`.
