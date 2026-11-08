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
| 4 | Resource limits (cgroup v2) | **Done** |
| 5 | Security controls (capabilities, seccomp) | **Done** |
| 6 | Pull and unpack a real image — `run alpine echo hello` | **Done** |
| 7 | Layered filesystem (OverlayFS) | **Done** |
| 8 | A network namespace of its own | Next |
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
sudo bash scripts/stage4-success-test.sh
sudo bash scripts/stage5-success-test.sh
sudo bash scripts/stage6-success-test.sh
sudo bash scripts/stage7-success-test.sh

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
rustainer run -d --rootfs ./alpine-rootfs --network host --hostname boxy \
    -m 512m --cpus 1.5 --pids-limit 128 \
    --user 1000:1000 --cap-drop ALL /bin/sh -c 'sleep 60'
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

Resource limits are cgroup v2 only; v1 is detected and **refused**, never
silently degraded to no limits. A requested limit that cannot be applied is a
hard error, because `--memory=512m` accepted and never enforced is how a host
OOMs three weeks later.

Security controls are on by default: Docker's fourteen capabilities and no
more, `no_new_privs`, a seccomp filter whose first instruction is a
fail-closed architecture check, and Docker's masked and read-only path lists.
`--cap-add`, `--cap-drop`, `--user` and `--security-opt seccomp=<file>` adjust
them; an OCI seccomp profile written for runc or Docker loads unchanged.

Those three are **one property**, not three hardening items: masked paths are
per-superblock, so a container that can still call `mount(2)` simply mounts a
second procfs and gets an unmasked one. They count only together with
`CAP_SYS_ADMIN` absent from the bounding set and the mount family denied by
seccomp — the property being enforced is *the container cannot obtain a
writable `/proc/sys`*.

Images are pulled by our own registry client. Every digest is **recomputed**
rather than taken from a response header, the manifest is classified by what
arrived rather than by what was asked for, and schema 1 is detected and
refused — its signature covers a rearranged document, so the bytes on the
wire never hash to the digest the registry reports.

Extraction is the most dangerous code here: attacker-controlled input, parsed
by a root process, becoming a container's root filesystem. Every filesystem
operation goes through a pinned directory handle with
`openat2(RESOLVE_IN_ROOT | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_XDEV)`, because
lexical path checks cannot close the symlink-swap hole — between "I verified
this path is inside the root" and "I opened it", another entry of the same
tarball can change the answer. Device nodes from layer content are refused
outright; absolute hardlink targets are refused; `security.*` xattrs are
dropped.

Layers are extracted once, keyed by their uncompressed digest, and shared
between containers; each container gets a private writable upper. The overlay
is assembled with **genuine `fsconfig(2)` calls**, never a `lowerdir=a:b:c`
string: the legacy options page is one 4095-byte buffer the kernel fills and
does not NUL-terminate, so a long layer list is silently truncated and the
mount *succeeds* with layers missing. `mount -o lowerdir+=` does not escape
it either — util-linux still issues a single `mount(2)`. A 300-layer stack
mounting and reading back in full is the test that proves it.

There is still no network isolation. The flags refuse rather than pretend:
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
