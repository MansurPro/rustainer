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
| 2 | A real root filesystem (`pivot_root`) | **Done** |
| 3 | Lifecycle, supervision and state | **Done** |
| 4 | Resource limits (cgroup v2) | Next |
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

Each stage's success test asserts its guarantee by reading kernel state back
from **inside** the container, never by observing that a command printed the
right thing. They need Linux and `CAP_SYS_ADMIN` — which does **not** mean
root:

```sh
sudo bash scripts/stage1-success-test.sh
sudo bash scripts/stage2-success-test.sh
sudo bash scripts/stage3-success-test.sh

# or with no root at all, if /proc/sys/user/max_user_namespaces > 0
unshare --user --map-root-user bash scripts/stage2-success-test.sh
```

On a macOS or Windows workstation, use a throwaway Linux VM:

```sh
docker run --rm --privileged -v "$PWD":/w -w /w rust:1.96-slim \
    bash scripts/stage2-success-test.sh
```

Docker there is a VM substitute, not part of the implementation — everything
under test is our own `clone3` and `pivot_root`.

To run against a remote Linux box without installing a toolchain on it,
`scripts/remote-test.sh <ssh-host>` cross-compiles to static musl, copies two
files to `/tmp`, and runs the tests in an unprivileged user namespace.

### What works now, and what refuses

```sh
rustainer run -d --rootfs ./alpine-rootfs --network host --hostname boxy /bin/sh -c 'sleep 60'
rustainer ps
rustainer logs <id>
rustainer stop -t 5 <id>
rustainer rm <id>
```

The container gets its own PID, UTS, IPC and mount namespaces, a root
filesystem pivoted onto a directory you extracted yourself, a private `/proc`,
a fresh `tmpfs` on `/dev` with the core device nodes, `/dev/pts`, `/dev/shm`,
and a read-only `/sys`. A supervisor process owns it and outlives the CLI, so
`run -d`, `ps` after the CLI exits, exit-code collection and `logs` all work
without a daemon.

There is still no image, no layering, no resource limit, no capability or
seccomp work, and no network isolation. The flags refuse rather than pretend:
`--network bridge` names stage 9, and a `--rootfs` that is not a directory
says so.

### The engine does not lie about what it does not know

Two behaviours are worth stating because they are easy to get wrong in the
comfortable direction:

**An exit nobody observed is reported as unknown, never as 0.** Kill a
container's supervisor and its init, and `ps` says so:

```
ID             STATUS    PID      EXIT     COMMAND
9f2c1a4e7b03   unknown   -        ?        /bin/sleep 30
```

**Where a facility is genuinely unavailable, the engine names what is
degraded and why** rather than quietly producing less than it looks. Rootless,
for example:

```
rustainer: degraded: device nodes - mknod of a device requires CAP_MKNOD in the
  INITIAL user namespace, and we are in a nested one; the host's nodes are
  bind-mounted instead
```

And `stop` takes its full grace period against a container that installed no
`SIGTERM` handler, because PID 1 in a new PID namespace has no default
disposition and genuinely does ignore it.
