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
| 8 | A network namespace of its own | **Done** |
| 9 | Bridge, NAT and the outside world | **Done** |
| 10 | Published ports | **Done** |
| 11 | Single-ID rootless | **Done** |
| 12 | `exec` — a second process in a live container | **Done** |
| 13 | Docker image compatibility, measured against real registries | **Done** |
| 14 | Measured against Docker, and what measuring found | **Done** |
| 15 | A layer's deletions are honoured | **Done** |
| 16 | Volumes and bind mounts | **Done** |
| 17 | A local API, and a page that uses it | **Done** |
| 18 | Stacks — several containers from one file | **Done** |

The eleven-stage ladder of `research/pass-2/RND.md` §17 is complete.
Stages 12 onwards are the additive work §19 held back until the mechanisms
were understood, taken in the order that each one's absence started to
cost something real.

What is still deliberately absent: `attach` and interactive terminals,
restart policies, `events`, healthchecks, `commit` and `build`, registry
push, user-defined networks and embedded DNS, swarm, checkpoint/restore,
and drop-in OCI runtime compatibility. §19 lists why each is additive
given what exists.

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
sudo bash scripts/stage8-success-test.sh
sudo bash scripts/stage9-success-test.sh
sudo bash scripts/stage10-success-test.sh

# Stage 11 is the exception: NOT sudo, and not inside unshare either.
# The engine makes its own user namespace; a wrapper would hide the stage.
bash scripts/stage11-success-test.sh

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
rustainer run -d --rootfs ./alpine-rootfs --hostname boxy \
    -p 8080:80 -p 127.0.0.1:5432:5432 \
    -m 512m --cpus 1.5 --pids-limit 128 \
    --user 1000:1000 --cap-drop ALL /bin/sh -c 'sleep 60'
rustainer ps
rustainer logs <id>
rustainer exec -e TZ=UTC <id> /bin/sh -c 'ps -o pid,comm'
rustainer serve                       # a local page at 127.0.0.1:8750
rustainer stack up -f examples/rustainer.toml
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

**Containers reach the internet and each other.** The default is a bridge:
the container gets its own network namespace, a veth pair whose peer is
placed directly into that namespace by the message that creates it, an
address, a default route, and source NAT. `--network none` gives a stack with
nothing but loopback; `--network host` shares the host's and says plainly
what that means.

`/etc/resolv.conf` and `/etc/hosts` are generated per container and bound in.
That is part of this stage rather than a later one, and deliberately: a
container with an address, a route and working NAT has **no nameserver at
all**, so the stage looks broken in a way every user misdiagnoses. The host's
resolvers are filtered — `127.0.0.53` is meaningless inside a container,
because each namespace has its own loopback.

`ip_forward` is read, never written: the kernel resets every other forwarding
parameter to its default when that file is written, so the engine refuses and
gives you the command instead.

**A published port is two rules, written together or not at all.** `-p
8080:80` is a destination NAT rule that *translates* the address and a filter
accept that *authorizes* the forward. They are different decisions and a
half-applied pair is not a broken feature but a security hole: an accept left
behind without its DNAT opens the container to anyone who can route to its
address. Every publish is rolled back whole if any part of it fails, and
teardown asserts nothing is left.

`curl localhost:8080` cannot be served by firewall rules at all. **Loopback is
deliberately excluded from destination NAT** — translating a `127.0.0.1`
destination into a routable address turns a packet that could never leave the
machine into one that can, and the kernel is right to refuse. So the host side
of every published port is a real `bind(2)` in the supervisor, forwarding to
the container. Binding is also what makes the port *reserved*: a second
container publishing the same host port fails immediately with `EADDRINUSE`
instead of quietly adding a second DNAT rule that never matches.

The backend is whichever of `nft` and `iptables` the host already uses, not
whichever we prefer. An accept in our table cannot rescue a packet another
table has dropped, so picking the tool the host is already using is the only
choice that composes; where a foreign ruleset's `DROP` policy may still win,
we say so instead of appearing to work.

The flags refuse rather than pretend: `-p` with `--network host` or
`--network none` says a bridge is required, `udp` refuses rather than silently
publishing tcp, and a `--rootfs` that is not a directory says so.

**It runs without root, and that is a one-flag change.** An unprivileged
user runs `rustainer run` with no `sudo`, no `unshare`, no setuid helper and
no `--rootless` flag. The engine notices it has no `CAP_SYS_ADMIN`, adds
`CLONE_NEWUSER` to the clone it was going to make anyway, and writes one
line to the child's `uid_map`:

```
0 1000 1
```

One extent, the caller's own id, which is the whole of what an unprivileged
process may map for itself. The three-process topology rootless is assumed
to require applies to `subuid`-based multi-extent maps, which are
deliberately not implemented and say so. `setgroups` is denied before
`gid_map` is written, in that order, because the reverse fails and leaves
the container holding the caller's supplementary groups.

That this is a one-flag change is the actual result, not a convenience.
Rootless was last on the ladder precisely so it could find out whether
privilege had been expressed as *"we are root, so this will work"* or as
*"this operation, in this namespace, needs this capability"*. Had it been
the former, the assumption would now be in hundreds of places that never
mention a uid, and this would have been a rewrite. The stage 11 success
test asserts the absence as much as the presence: no rootless-specific
flag, no setuid bit, the same command line as root.

Two things genuinely cannot work, and are named rather than skipped:

```
rustainer: the limits you asked for cannot be enforced: this process has no
  writable cgroup to delegate from. A user namespace grants no authority over
  the cgroup hierarchy - that is a separate delegation the host has to make.
  ...
      systemd-run --user --scope -p Delegate=yes rustainer run ...

rustainer: bridged networking needs CAP_NET_ADMIN in the user namespace that
  owns the HOST's network namespace, and this process is in a nested one,
  which can never have it. Degraded: no bridge, no address, no NAT.
  Run `--network none` for a stack with only loopback, or run as root.
```

Which of those two refusals a user sees depends on ordering, and the
ordering is deliberate: the capability check runs before the `ip_forward`
check, because forwarding is something an operator can fix in one command
and the missing capability is not. Reporting the fixable one first sends a
rootless user off to run a `sysctl` that changes nothing about their
problem.

**`exec` is the only operation that joins namespaces instead of creating
them**, and the asymmetry between those two verbs is the whole of it.
`clone3` takes a flag word; `setns(2)` takes one descriptor at a time, so
there is no atomic "join these six" and a half-joined process is a process
that looks fine while running slightly outside the container. Who joins what
is forced rather than chosen:

- `setns(CLONE_NEWPID)` **does not move the caller**. It decides where the
  caller's next child is created. Joining a pid namespace is therefore
  `setns` *and then fork*, and the fork is not an implementation detail — it
  is the join.
- The **mount namespace is joined by the child, not the parent**, because
  `setns(CLONE_NEWNS)` calls `set_fs_root` and `set_fs_pwd` on the caller.
  Join it in the parent and `/proc` becomes the container's — mounted
  against a pid namespace the parent is not in — so `/proc/self` resolves to
  nothing and the single-thread check that `clone3` depends on fails
  `ENOENT` while reporting a thread-count problem.
- `setns` of a user or mount namespace **refuses a multi-threaded caller**.
  The no-async rule adopted in stage 1 for `clone3`'s sake is what makes
  `exec` possible at all; an engine with a thread pool would have had to
  re-exec a helper here.

The second process is placed in the container's cgroup at clone time by
`CLONE_INTO_CGROUP`, so it is never outside the container's limits, not even
briefly. The cgroup is found by reading `/proc/<init>/cgroup` rather than by
rebuilding the path from the container id — `exec` may run from a different
login session, and therefore a different slice, than the `run` that created
it.

The command is resolved through `/proc/<init>/root`, which is the only view
that is correct: a container's root is an overlay mounted inside its own
mount namespace, so searching the image's layers on disk would miss
everything the container has written since it started.

The stage 12 test asserts namespace *identity*, by inode, not behaviour — a
freshly created namespace of the same kind passes every behavioural check
and fails that one.

### Images built for Docker run here unchanged

This is a claim, not a mechanism, so it is tested the only way a claim can
be: against real images from real registries — Docker Hub, GitHub, Quay and
Google — pulled and run, with the assertion being that each image's own
entrypoint produces its own output.

Running that test for the first time found four defects, and every one of
them looked like something else:

- **`./` as a tar entry was rejected as "an empty path".** It is what
  `tar -C dir .` writes, so it is the first member of a great many layers:
  `debian`, `python`, `busybox` and Google's distroless images all have one.
  A third of the base images in common use could not be pulled at all.
- **An image whose layers contain no `/proc` could not start**, reporting
  `mounting /proc failed with ENOENT`. The mount point was created
  parent-side, in the directory the overlay is then mounted *on top of*,
  which hides it. For any image with a package manager the layers contain
  `/proc` already and the bug is invisible; `busybox` and `hello-world` ship
  without one. The mount points are now created by the child, above the
  overlay — the same trap `/dev/pts` and `/dev/shm` had already been fixed
  for, one level up.
- **A command was searched for in the topmost layer only, and in a hardcoded
  PATH.** `golang:1.24-alpine` is four layers with `/bin/pwd` in the Alpine
  base at the bottom, and puts its own toolchain in `/usr/local/go/bin` —
  which it declares in its config and which appears on no default list. The
  Go image could run neither `pwd` nor `go`. The search is now every layer,
  topmost first, across the image's own `PATH`.
- **`USER nginx` was refused**, because names were not resolved. That rules
  out every image that sets `USER` by name — `prometheus`, `grafana`,
  `nginx-unprivileged`, `elasticsearch`. Names now resolve against the
  **container's own** `/etc/passwd`, never the host's: `nobody` is 65534 on
  Debian and 99 on Fedora, and resolving against the host would silently run
  the container as a different person than the image asked for. A numeric id
  is still taken as given and never looked up, so `--user 1000:1000` keeps
  working in an image with no `/etc/passwd` at all.

The test itself has one honest accommodation. Docker Hub allows 100
anonymous pulls per six hours per address, and a shared CI address has
usually spent that quota on someone else's behalf. A rate limit is not an
incompatibility, so the test falls back to `mirror.gcr.io` — a pull-through
cache of Hub serving the same manifests, blobs and digests — and **says in
its output how many images came from the mirror**. That also tests the
pull-through mirror most corporate networks put in front of Hub.

### Measured against Docker

Both engines in one privileged `docker:28-dind` container, so same kernel,
same CPU, same disk; 40 samples a row, three runs; `bash scripts/bench.sh 40`.
Full methodology and the reasons behind each row are in
[`research/COMPARISON.md`](research/COMPARISON.md).

| measurement | rustainer | Docker 28.5.2 | ratio |
|---|---:|---:|---:|
| run a container to exit (median) | 11–12 ms | 82–83 ms | **6.9–7.5×** |
| start detached, stop, remove | 45 ms | 107–108 ms | 2.4× |
| exec a command in a live container | 7–8 ms | 28–29 ms | 3.5–4.1× |
| resident per running container | 1 MiB | 18 MiB | **18×** |
| resident with nothing running | **0 MiB** | 123–125 MiB | — |
| binaries installed | 5.1 MiB | 200 MiB | **39.8×** |
| pull a cold image (network) | 0.9–1.0 s | 2.6–2.7 s | 2.7–3.0× |

Most of that is architecture, not craftsmanship. `docker run` is CLI → HTTP
over a socket → dockerd → gRPC → containerd → a shim → runc → `clone`:
five processes and two RPC hops before a namespace exists. `rustainer run`
is CLI → supervisor → `clone3`. Docker's layers buy a restartable daemon, an
API, and a runtime boundary other tools plug into; this engine buys none of
them, and 70 ms is what that costs per container. The one row that is
implementation rather than design is the per-container memory: a Go process
doing the shim's job will not reach 1 MiB.

**The first run of this benchmark said rustainer was 5.7× *slower*.** 890 ms
against 155 ms — and the cause was not in the runtime. `run <image>` went
through `pull`, and `pull` begins by fetching the manifest, so starting a
container from an image already on disk cost four TLS round-trips before
anything was cloned. Slow was the smaller half: a fully cached image was
**unrunnable with no network**, failing at a DNS lookup for an image whose
every byte was local. The store is now consulted first — Docker's
`--pull=missing`, which is Docker's default — and `--pull` still means go and
check. 890 ms → 11 ms, and it works offline.

No correctness test would have found that. Every image assertion passed
before and after; the container ran, the layers were right, the config was
obeyed. It took measuring against another implementation to notice a correct
answer being produced the expensive way, and the offline failure — a real
functional bug — was only found by asking why the number was bad. The stage
14 test pins both: a cached image must start inside `unshare --net`, and
nothing of ours may be resident once no container is running.

What Docker does that this engine does not is the other half of that table,
and it is the longer list: image build, push, volumes, user-defined networks
and embedded DNS, an HTTP API, `events`, restart policies, healthchecks, log
drivers, Compose, swarm, plugins, checkpoint/restore, Windows, cgroup v1.
The timings measure starting and supervising a container. They are not a
claim that one tool replaces the other.

### A layer's deletions are honoured

The most serious defect found in this project, and it was invisible to
every test that came before it.

Each layer is extracted into a directory of its own so that layers can be
shared between images, and those directories are composed at run time by
OverlayFS. A **whiteout** — the OCI way of saying "this layer deletes that
file" — was handled by *deleting the name from the layer being extracted*.
That looks right and does nothing: the victim lives in a different layer's
directory, so the unlink is a no-op and the file stays visible through the
lower layer at run time.

`RUN ... && rm -rf /secret` is one of the most common lines in any
Dockerfile. It is how build secrets, private keys, credentials and package
caches are removed. **Every one of them was readable inside the container.**

A whiteout has to be left behind in a form OverlayFS understands: a
character device with major 0 and minor 0, which the kernel recognises by
that exact number and which never appears in the merged view. An opaque
directory is an xattr — `trusted.overlay.opaque`, or `user.overlay.opaque`
when the overlay will be mounted `userxattr`, which it is whenever we are
outside the initial user namespace. Setting the wrong one of those is
silent in exactly the same way: the mount succeeds, the xattr is ignored,
and deleted files reappear.

This is the one place the extractor creates a device node, and the contrast
with the rest of it is the point: a device *from layer content* is refused
outright, because a layer shipping `dev/mem` as char 1:1 would be a direct
path to host memory. `0:0` is not a device at all — there is no driver
behind it — and it is synthesised from a `.wh.` marker, never copied from
the archive.

Where the environment forbids creating one — an unprivileged container's
device filter denies it even with `CAP_MKNOD` in the effective set — the
engine says so, in those terms:

```
rustainer: WARNING: 2 deletion(s) recorded by this image could not be
  applied, so files its later layers deleted REMAIN VISIBLE in containers
  made from it. ...
```

The stage 15 test builds its image byte by byte and serves it over
loopback rather than pulling one, because the properties worth testing
hardest are about what a layer *removes*, and you cannot see what a public
image's layers contain without reimplementing the thing under test. It
then reads the store back: the marker must be a character device, the
number must be exactly 0:0, the lower layer must be untouched, and no
`.wh.` entry may be visible inside the container.

### Volumes and bind mounts

```sh
rustainer run -v /host/path:/in/container        alpine ...
rustainer run -v /host/path:/in/container:ro     alpine ...
rustainer run -v name:/in/container              alpine ...
rustainer volume ls
rustainer volume rm name
```

Docker's syntax, including Docker's rule for telling a host path from a
volume name: a first field beginning with `/` or `.` is a path, anything
else is a name. It looks arbitrary and it is the only rule that can work,
because `data` is a perfectly good directory name and a perfectly good
volume name and the engine has to choose without asking.

A volume is a deliberate hole in the container's filesystem isolation —
that is what it is for — so the two things that make it safe are explicit:

- **The source is resolved by the parent, before the pivot.** The container
  never influences it, so no symlink inside the image can redirect the
  mount. Resolving a mount source through the container's own tree is the
  vector CVE-2025-31133 used.
- **The target, and every directory above it, is created by the child,
  above the overlay.** Created parent-side they land *underneath* the
  overlay, which then hides them, and the mount fails with `ENOENT`
  pointing at a path that plainly exists. `-v v:/var/lib/app/data` works on
  an image with no `/var/lib/app` because the child walks the components
  one at a time, each tolerating `EEXIST`.

The bind is **recursive**, as `docker run -v` is: a filesystem mounted
under the source is visible inside the container rather than appearing as
an empty directory. That is also why `:ro` is `mount_setattr(AT_RECURSIVE)`
— a plain read-only remount would leave every one of those submounts
writable, which is the trap `/sys` hit back at stage 2.

Named volumes live beside the image store, because both must outlive a
reboot and the runtime state directory is usually a tmpfs. Removing a
container never removes them.

### A local API, and a page that uses it

```sh
rustainer serve            # http://127.0.0.1:8750/
```

One binary, one page, no build step, no CDN. The page is compiled into the
binary and references nothing it cannot reach — which the stage 17 test
asserts, because a dashboard that fetches a framework before it can tell
you what is running is useless exactly when you need it, and this engine
is specifically built to work offline.

Two properties matter more than the endpoint list, and both are asserted
rather than described:

**The server does not own the containers.** Each one is owned by its own
supervisor, so this is a *view*. Kill the server and everything keeps
running; start another and it sees the same containers. There is no state
in it to corrupt and no reason not to run two.

**The server cannot start a container itself.** It is multi-threaded — one
thread per connection — and a container is created by a process that has
never had a second thread, because after a raw `clone` the child inherits
every lock in the address space in whatever state it was in. So every
mutation executes the CLI and lets a fresh single-threaded process do it,
exactly as the CLI does for the supervisor. The rule that made `clone3`
safe at stage 1 decides the architecture of the API at stage 17 — which is
the kind of thing a "just add an HTTP server" plan does not survive
contact with.

There is **no authentication**. Anything that can reach the port can start
a container, and a container is a path to root on the host, so it binds
loopback and says so loudly if you give it anything else.

The one thing the page does that other container GUIs do not: **every
action shows the command that performed it.**

```
$ rustainer run -d --network bridge alpine:3.21 /bin/sleep 300
$ rustainer exec web ps -o pid,comm
```

A tool that hides the command teaches you nothing and leaves you helpless
when it is not installed.

Building it found three things the engine was getting wrong, none of which
a test would have caught, because each one was invisible until something
tried to display it:

- **A detached container that failed to start discarded the reason.** The
  supervisor's stderr went to `/dev/null` with everything else, so a
  perfectly good explanation — "cannot enable the memory controller in …,
  it still holds …" — was thrown away, and the container showed as
  `unknown` with no error anywhere. The supervisor's own stderr now goes to
  a file in the state directory, the CLI prints it, and the state records
  it, so `ps`, the API and the page all show the real reason.
- **`rustainer images` printed a key that `rustainer rmi` would not
  accept.** A store key has `/` and `:` replaced by `_`, so
  `mirror.gcr.io/library/alpine:3.21` was listed as
  `mirror.gcr.io_library_alpine_3.21` — and feeding that back produced "no
  such file" naming a path nobody typed. A tool has to accept what it
  prints; the page's remove button was simply the first caller with nothing
  else to pass.
- **`rmi` on a missing image reported a path instead of a name.** It now
  says what the person did.

### Stacks

```sh
rustainer stack up    -f examples/rustainer.toml
rustainer stack ps    -f examples/rustainer.toml
rustainer stack down  -f examples/rustainer.toml
```

```toml
name = "example"

[services.db]
image = "postgres:17-alpine"
env = ["POSTGRES_PASSWORD=dev"]
volumes = ["exampledb:/var/lib/postgresql/data"]

[services.web]
image = "nginx:1.27-alpine"
ports = ["8080:80"]
depends_on = ["db"]
```

Compose-shaped, deliberately not Compose, in three ways that each exist to
stop a specific way of losing an afternoon:

- **TOML, not YAML.** A configuration language should not be able to
  surprise you. `ports: - 8080:80` is a sexagesimal number in YAML 1.1,
  `no` is a boolean, and whether two spaces or four are correct depends on
  what the line above did.
- **An unknown key is an error.** A misspelled key in a Compose file does
  nothing at all, silently, and the service starts without the setting you
  thought you gave it. Here, `imgae = "alpine"` is a refusal that names it.
- **`command` is a list, never a string.** Compose accepts both, and the
  string form is split on spaces — so the quotes in
  `sh -c "echo hi; sleep 1"` become part of the arguments and the shell is
  handed `"echo` as its whole script. There is no shell here to re-quote
  it, and inventing a quoting dialect for a config file is worse than
  requiring a list.

**Services find each other through `/etc/hosts`, not DNS.** Every address
is allocated *before anything starts*, so every container's hosts file can
name every other one — including the services that do not exist yet. The
test asserts the case a naive implementation misses: the service that
starts **first** must be able to reach the one that starts **last**. A
resolver in each namespace would be a second service to run, to supervise
and to get wrong; this way `cat /etc/hosts` is the whole debugging story.

A stack is **not an orchestrator**. No reconciliation loop, no
rescheduling, no health-gated rollout. `up` starts what is not running in
dependency order and leaves the rest alone; `down` stops in reverse, and
keeps the volumes. The containers are ordinary containers — `ps`, `logs`,
`exec` and the page all work on them without knowing a stack exists, which
is what makes the whole thing debuggable with the tools you already have.

Stage 18 also added `run -e/--env`, which the engine simply did not have:
the image's environment could be read but never overridden. It uses the
same merge rule `exec` does, so `--env PATH=…` replaces the image's entry
rather than shadowing it — prepending also "works", but only by accident
of glibc's `getenv`, and `env` inside the container would print the
variable twice.

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
