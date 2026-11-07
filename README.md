# rustainer

A Docker-like container engine written in Rust from first principles — owning the
runtime core (namespaces, cgroups, overlayfs, networking), not shelling out to
runc/crun/containerd.

**Status: R&D complete, implementation starting.** The research phase is finished and
its deliverable is [`RND.md`](RND.md).

## Research

[`RND.md`](RND.md) is the entry point: a priority index (P0/P1/P2 by architectural
importance) over 22 sections, routing into [`research/`](research/README.md).

The brief was run **twice, independently**, and both runs are kept:

| | Where | Shape |
|---|---|---|
| **Pass 1** | [`RND.md`](RND.md) + [`research/`](research/README.md) | A 54 KB index routing into satellite files — a 42-step creation trace, a 16-rung ladder, a 28-item risk register |
| **Pass 2** | [`research/pass-2/`](research/pass-2/README.md) | A single self-contained 828 KB `RND.md` — 22 sections with a 51-step trace and an 11-stage ladder inside it |

Neither supersedes the other. They agree on the overwhelming majority; where they
disagree, [`research/pass-2/README.md`](research/pass-2/README.md) lists the four
substantive points rather than reconciling them silently.

| Domain | Topic | State |
|---|---|---|
| D1 | Kernel isolation — namespaces, mount propagation, `pivot_root`, PID 1 | Complete |
| D2 | Runtime architecture — OCI spec, create/start split, state persistence | Complete |
| D3 | OCI images, registries, layered filesystems | Complete |
| D4 | Networking — netns, veth, bridges, port publishing | Complete |
| D5 | cgroups v2, capabilities, seccomp, rootless privilege model | Complete |
| V1–V7 | Adversarial verification — fresh agents tasked with falsifying, never the authors | Complete |

## Implementation

Linux only, x86_64 first, cgroup v2, root-first with the rootless seam respected from
the first commit. The ladder is deliberately slow at the start: the process model is
built and tested before images, storage or networking exist, because every one of those
is easier to reason about once the process model is not also in question.

`run alpine echo hello` is not the first milestone. It is the sixth.

| Stage | Goal | Status |
|---|---|---|
| 1 | A process in namespaces of its own | **Done** |
| 2 | A real root filesystem (`pivot_root`) | Next |
| 3 | Lifecycle, supervision and state | |
| 4 | Resource limits (cgroup v2) | |
| 5 | Security controls (capabilities, seccomp) | |
| 6 | Pull and unpack a real image — `run alpine echo hello` | |
| 7 | Layered filesystem (OverlayFS) | |
| 8 | A network namespace of its own | |
| 9 | Bridge, NAT and the outside world | |
| 10 | Published ports | |
| 11 | Single-ID rootless | |

### Building and testing

```sh
cargo build --release
```

The success test for each stage asserts its guarantee by reading kernel state back from
**inside** the container, never by observing that a command printed the right thing. It
needs Linux and root:

```sh
sudo bash scripts/stage1-success-test.sh
```

On a macOS or Windows workstation, run it in a throwaway Linux VM:

```sh
docker run --rm --privileged -v "$PWD":/w -w /w rust:1.96-slim \
    bash scripts/stage1-success-test.sh
```

Docker there is a VM substitute, not part of the implementation — everything under test
is our own `clone3` call.

### What stage 1 does and does not do

`rustainer run --rootfs / --network host --hostname boxy /bin/sh` puts a process in its
own PID, UTS and IPC namespaces and waits for it correctly. There is no image, no root
filesystem change, no resource limit, no security control and no network isolation, and
the flags refuse rather than pretend: `--rootfs <dir>` and `--network bridge` both fail
with the stage that would support them.

`ps` inside still lists host processes, because `/proc` has not been remounted. That is
the teaching moment, not a bug — stage 2 fixes it.
