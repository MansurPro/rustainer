# rustainer measured against Docker

What this engine actually costs, next to the one it is modelled on, measured
rather than asserted. Everything here is reproducible with one command, which
is at the bottom.

The short answer: **2.4× to 7.5× faster** on the container operations both
tools perform, **18× less memory per running container**, **40× less binary**,
and **no resident daemon at all**. The longer answer is that most of that gap
is architecture rather than craftsmanship, and that the list of things Docker
does which this engine does not is longer than the list of things it does
faster.

---

## 1. How it was measured

| | |
|---|---|
| Host | Docker Desktop's LinuxKit VM, kernel 7.0.14 aarch64, 8 vCPU |
| Both engines | inside one privileged `docker:28-dind` container, so same kernel, same CPU, same disk |
| Docker | 28.5.2 (dockerd + containerd + containerd-shim-runc-v2 + runc) |
| rustainer | static musl build, one binary |
| Image | `alpine:3.21`, already in both local stores for every timed row except the pull |
| Samples | 40 per row, median and p90; three independent runs |
| Command | `bash scripts/bench.sh 40` |

The numbers below are the range across the three runs. **Absolute figures move
with host load — this is a laptop VM, not a quiet server — but the ratios held
within about 10% across all three.** The one run where absolutes doubled moved
both engines together and left every ratio intact, which is the signature of
contention rather than of a difference between the tools.

A timing from a command that *failed* is a time-to-failure, and averaging
those with real samples produces a number that is not about anything. The
harness aborts the whole measurement loudly instead; an early draft of this
document reported `0 ms` across the board because busybox's `date` has no
`%N` and printed a literal `N`.

---

## 2. Results

| measurement | rustainer | Docker | ratio |
|---|---:|---:|---:|
| run a container to exit (median) | 11–12 ms | 82–83 ms | **6.9–7.5×** |
| run a container to exit (p90) | 12–13 ms | 87–89 ms | 6.8–7.2× |
| start detached, stop, remove | 45 ms | 107–108 ms | 2.4× |
| exec a command in a live container | 7–8 ms | 28–29 ms | 3.5–4.1× |
| resident per running container | 1 MiB | 18 MiB | **18×** |
| resident with nothing running | **0 MiB** | 123–125 MiB | — |
| binaries installed | 5.1 MiB | 200 MiB | **39.8×** |
| pull a cold image (network) | 0.9–1.0 s | 2.6–2.7 s | 2.7–3.0× |

---

## 3. Why each difference exists

Naming the mechanism matters more than the number, because a number without a
mechanism is a benchmark you cannot trust the next version of.

### Container start — 7×

`docker run` is: CLI → HTTP over a unix socket → **dockerd** → gRPC →
**containerd** → a **shim** process → **runc** → `clone`. Five processes and
two RPC hops before a namespace is created, and the shim is forked and exec'd
per container so that containerd can be restarted without killing it.

`rustainer run` is: CLI → **supervisor** → `clone3`. One fork, one exec, one
syscall.

Those are different architectures, not two implementations of one. Docker's
layers buy things — a restartable daemon, an API, a runtime boundary that
other tools plug into — and this engine buys none of them. The 70 ms is what
that costs per container.

### exec — 3.5×

Smaller gap, same reason: `docker exec` is still a round trip through dockerd
and containerd to the shim, but there is no image to resolve and no bundle to
write, so the fixed cost dominates less. Ours is `setns` plus `clone3`.

### Memory per running container — 18×

Docker keeps a `containerd-shim-runc-v2` per container, ~18 MiB resident. Its
job is to own the container's I/O and exit status so containerd can restart
without taking the container with it.

Our supervisor does the same job in ~1 MiB, because it is a static binary with
no runtime, no GC and no RPC stack. **This is the one row where the difference
is implementation rather than architecture** — a Go process that does what the
shim does will not get to 1 MiB, and that is a property of the language
runtime, not of the design.

### Memory with nothing running — a daemon that does not exist

Docker holds ~124 MiB resident whether or not any container is running,
because the API has to be there to answer. `rustainer` holds nothing: the CLI
exits, and the only long-lived processes are one supervisor per *running*
container. Stop them all and the engine's resident footprint is zero bytes,
which the stage 14 test asserts explicitly so that nobody can quietly grow a
daemon later.

This is not free. No daemon means no API, no `events` stream, no restart
policies, and no way to ask "what is running" except by reading the state
directory. That trade is written down in `research/pass-2/RND.md` §18.2 and
was made before any of this was measured.

### Binaries — 40×

5.1 MiB against roughly 200 MiB of `dockerd`, `containerd`,
`containerd-shim-runc-v2`, `runc`, `docker`, `docker-proxy` and `ctr`. Mostly
this is scope, partly it is Go's static linking, and it is the least
interesting number here.

### Image pull — 2.8×

The only row that measures the network, and the only one where we expected to
lose. We do not, by about three times — but the sample size is one per run and
the variance is the internet's, so treat this as "not slower" rather than as a
result. Both engines fetch the same bytes from the same host; the difference
is in how much work happens between them.

---

## 4. What the measurement found

The first run of this benchmark reported that **rustainer was 5.7× slower than
Docker** at starting a container: 890 ms against 155 ms.

The cause was not in the runtime. `run <image>` called `Store::pull`, and
`pull` begins by fetching the manifest — so starting a container from an image
already on disk cost four TLS round-trips to a registry before anything was
cloned. 870 of those 890 milliseconds were network.

Slow was the smaller half of the defect. A fully cached image was
**unrunnable with no network**: on a plane, behind a proxy, or on a site with
egress rules, `rustainer run alpine` failed at a DNS lookup for an image whose
every byte was local. Docker has run cached images offline since the
beginning, and an engine that cannot is not Docker-compatible however many
images it can pull.

The fix is `Store::local`: consult the store first, go to the registry only on
a miss or when `--pull` asks. That is Docker's `--pull=missing`, which is
Docker's default. 890 ms → 11 ms, and the engine works offline.

**No correctness test would have caught this.** Every assertion about images
passed before and after; the image ran, the layers were right, the config was
obeyed. It took measuring the thing against another implementation to notice
that a correct answer was being produced the expensive way — and the offline
failure, which is a genuine functional bug, was only found by asking why the
number was bad.

The same run also found that every supervisor on the machine was called `exe`
in `ps`, because the process is started by executing `/proc/self/exe` — which
is what makes the CVE-2019-5736 seal work — and the kernel names a process
after the final component of the path it executed. It now calls
`prctl(PR_SET_NAME)`.

---

## 5. The other half: what Docker does that this engine does not

A benchmark that omits this is an advertisement. None of the following is
implemented here, and several are deliberate permanent omissions rather than
work not yet done — `research/pass-2/RND.md` §19 says which are which.

- **BuildKit** — `build` exists (stage 21), but it is a sequential interpreter
  for a Dockerfile, not a DAG solver: no parallel stages, no cache mounts, no
  `--mount=type=secret`, no remote cache import/export. Layer reuse across
  builds is not implemented either; every build runs every step.
- **A real in-namespace resolver.** User-defined networks exist (stage 23) and
  containers on one find each other by name, but the mechanism is `/etc/hosts`,
  rewritten as containers come and go. Docker runs a resolver at 127.0.0.11
  inside each container's network namespace; that needs a process living in
  that namespace, and this engine has no daemon to be one. The consequence is
  visible: a name is resolvable only to containers that were running when the
  file was last written, and nothing resolves a *service* to several addresses.
- **Healthchecks and log drivers.** Logs are files; there is no `json-file`
  rotation, no syslog, no fluentd, and nothing restarts a container because an
  application-level probe failed.
- **IPv6, overlay networks, `macvlan`, and port publishing on anything but a
  userspace forwarder.** Docker programs DNAT; this publishes a port with a
  process that copies bytes, which costs a hop and cannot preserve the client
  address.
- **Swarm, plugins, checkpoint/restore, Windows containers, cgroup v1 hosts.**
- **`commit`, `save`/`load`, `cp`, `diff`, `top`, `stats`.**
- **Multi-extent rootless** — `/etc/subuid` ranges and the setuid helpers are
  explicitly not implemented; rootless here maps exactly one id, so an image
  that wants two different non-root uids cannot run unprivileged.
- **A stable API.** `rustainer serve` speaks its own small JSON, not Docker's
  Engine API, so nothing built against `/v1.4x/containers/json` works.

And what has arrived since this was first written, which changes the shape of
the comparison rather than the numbers:

- **Volumes and bind mounts** (`-v`, `volume ls/rm`), recursive and with
  recursive `:ro`.
- **A local HTTP API and a page** (`rustainer serve`), now with a terminal,
  an event stream, a network view and a build dialog. It is not a daemon:
  containers outlive it, it owns nothing, and it cannot start a container
  itself — it executes the CLI, because a container is created by a process
  that has never had a second thread. The "0 MiB resident" row above is still
  true of the engine; the server is opt-in and runs only while you are
  looking at it.
- **Stacks** (`stack up/down/ps`), Compose-shaped and TOML-based.
- **Interactive terminals** — `run -it`, `exec -it`, and a pty in the page.
- **Restart policies and an event log** (`--restart`, `events --follow`).
- **`build` and `push`**, so an image can be made here and consumed by Docker.
- **User-defined networks** (`network create/ls/rm`) with isolation enforced
  by address, not merely by name.

The timings above are a fair measure of **starting and supervising a
container**. They are not a claim that one tool replaces the other, and this
engine's purpose was never to replace it — it was to understand the mechanisms
by owning them.

---

## 6. Compatibility, as a rate

`scripts/compat-matrix.sh`, twelve real images, both engines on one
kernel. Docker is the reference: an image Docker itself cannot run is
reported and excluded rather than counted against us.

There is no "3x more compatible", and the question is worth answering
precisely because the honest answer is a shape rather than a multiple.
Compatibility is **parity** - the target is that an image behaves
identically - so what follows is a rate, with the denominators.

| | measured |
|---|---|
| images offered | 30 |
| excluded (Docker could not pull it) | 2 |
| `id -u`, `id -g`, `pwd`, `$PATH` byte-identical | **28/28** |
| exit status of the image's own ENTRYPOINT/CMD identical | **28/28** |

alpine ×2, debian ×2, ubuntu, busybox, fedora, almalinux, python ×2,
node ×2, ruby, php, golang, rust, eclipse-temurin, nginx ×2, httpd,
caddy, traefik, redis ×2, postgres, mariadb, memcached, mongo. glibc and
musl, four distribution families, six language runtimes, and six
servers. The interesting ones are `redis`, `postgres` and `memcached`,
which set `USER` to a *name* that has to be resolved through the image's
own `/etc/passwd`, and `nginx` and `mongo`, whose entrypoints are
scripts that run other scripts and then drop privileges.

**Widening this list from twelve images to thirty found a real bug**,
which is the only reason to widen it. At twelve it was 12/12; at thirty
it was 27/28, and the one that differed was `mongo:7` - exit 48 against
Docker's 0, with `setup bind :: caused by :: Permission denied` in its
log.

`mkdirat` masks its mode argument with the process **umask**. A layer
declaring `/tmp` as `1777` got `1755`, so mongod - whose entrypoint
drops to uid 999 - could not create its socket there. Nothing in the
engine looked wrong because nothing was: the kernel applied the umask to
a number that had been computed correctly. A directory that already
existed was worse, because `mkdirat` returns `EEXIST` without touching
the mode at all.

Directory modes are now applied in a final pass, deepest first, through
the pinned parent with `O_NOFOLLOW` and `fchmod` on the descriptor - no
second path resolution, which is the hole that module exists to close.
GNU tar calls the same thing `--delay-directory-restore`. The regression
test runs under `umask 022` on purpose, so it fails if the pass is ever
removed.

The same run timed one container start per image, across all thirty:

| image | docker | rustainer | |
|---|---|---|---|
| alpine:3.21 | 85ms | 13ms | 6.4x |
| debian:12-slim | 96ms | 18ms | 5.2x |
| ubuntu:22.04 | 85ms | 13ms | 6.3x |
| busybox:1.37 | 83ms | 13ms | 6.4x |
| fedora:41 | 84ms | 12ms | 6.6x |
| almalinux:9 | 77ms | 12ms | 6.2x |
| python:3.13-alpine | 83ms | 11ms | 7.0x |
| node:22-slim | 77ms | 12ms | 6.5x |
| ruby:3.3-alpine | 82ms | 16ms | 5.1x |
| php:8.3-cli-alpine | 81ms | 11ms | 6.8x |
| golang:1.23-alpine | 82ms | 12ms | 6.7x |
| rust:1-slim | 86ms | 14ms | 6.0x |
| eclipse-temurin:21-jre-alpine | 84ms | 12ms | 6.5x |
| nginx:1.27 | 84ms | 12ms | 6.5x |
| caddy:2-alpine | 78ms | 12ms | 6.5x |
| traefik:v3.2 | 117ms | 12ms | 9.7x |
| redis:7 | 84ms | 12ms | 6.6x |
| postgres:17-alpine | 83ms | 12ms | 6.8x |
| mariadb:11 | 78ms | 12ms | 6.4x |
| mongo:7 | 92ms | 13ms | 6.7x |

(Twenty of the thirty, trimmed for width; the rest sit in the same band.
An earlier, quieter run of twelve images measured 6.6-7.5x, which is the
same result with less contention on the host.)

**5.1x to 9.7x across thirty images, clustered near 6.5x, and the image
barely matters.** That is the signature of a difference that is
architectural rather than incidental: the work both engines skip for an
image already in the store is the same work, so what is left is the path
from the command to `clone`. The outlier in our favour is `traefik`,
where Docker took 117ms; the two nearest the bottom, `ruby` and
`debian-slim`, are ours being slower rather than Docker being faster.

---

## 7. Stacks, against Docker Compose

`scripts/stack-bench.sh`, the same three-service application written once
as `compose.yaml` and once as `rustainer.toml`, against Compose v2.40.3.

`up` is measured not to the command returning but to every service being
able to reach every other **by name**, because a stack whose containers
exist and cannot find each other is not up.

| | docker compose | rustainer | |
|---|---|---|---|
| `up`, to mutually reachable by name | 209-361ms | 34-39ms | **6.0-9.3x** |
| `down -t 1` | 1282-1293ms | 1048-1060ms | 1.2x |
| **`up` with 3 replicas** | 243ms | **25ms** | **9.7x** |
| **health-gated `up`** | 4214ms | **3047ms** | see below |

The health-gated row is the one worth reading carefully, and it is the
only measurement here of the features this comparison itself caused.
Both files say the same thing: `db` is ready when `/tmp/ready` exists,
it writes that file three seconds in, and `web` must not start before
then. **Both engines pay the same three seconds**, so subtract them and
what is left is the machinery around the wait: **47ms here against
1214ms**. `docker compose up --wait` and `depends_on: condition:
service_healthy` do a great deal more than poll a command - and for the
question `depends_on` actually asks, that is the price of it.

**Two things this measurement changed rather than confirmed**, which is
the only reason it was worth running:

- **`down` was 2.4x SLOWER** (3.1s against 1.3s) until it was measured.
  `down` stopped one service at a time in reverse dependency order, and
  nearly all of what `down` costs is the grace period, because PID 1 in
  a new PID namespace ignores a SIGTERM it has no handler for. Three
  services meant waiting three times. Services in one dependency level
  are independent *by definition*, so they are now stopped together, and
  `down` grew the `-t` Compose has always had. The first version of the
  benchmark also gave Compose `-t 1` and us our own default, which
  measured the flag rather than the engine.
- **"Stricter about typos" did not survive the measurement.** A
  misspelled service key is refused here - and `docker compose` refuses
  it too. Parity. A comment in `crates/cli/src/stack.rs` claimed
  otherwise and has been corrected.

### Closed because this measurement made the gap concrete

Four of the six things this section used to list are now implemented,
which is the practical result of running the comparison at all:

- **`profiles`** and `--profile`. A service with no profiles is always
  in; one with them is in only when asked for. A kept service that
  depends on an excluded one is **refused**, rather than started without
  it or silently dragging it in.
- **`${VAR}` interpolation with `.env`**, read from beside the *stack
  file* rather than the working directory, with the environment winning
  over the file. An **undefined variable with no default is an error** -
  Compose substitutes empty and warns, which turns `${PORT}:80` into
  `:80` and fails somewhere else entirely. `${VAR:-}` asks for empty on
  purpose, and `$$` is a literal dollar.
- **`build:`** - a service built from a Dockerfile as part of the stack,
  with the context relative to the stack file. Everything that has to be
  built is built **before any address is allocated or any container
  starts**, because a stack that starts three services and then fails to
  build the fourth leaves the operator worse off than one that refused.
- **`restart`** per service, validated at parse time so a typo is caught
  before half the stack is running.

Then three more, on a second pass:

- **Several `-f` files merged in order**, and **`extends`**. One
  mechanism serves both, because both are "take that table and override
  parts of it": tables merge key by key, and **a list replaces rather
  than appends**. Appending looks friendlier and is a trap - an override
  that sets `command` would append to the base command and run something
  nobody wrote. `extends` in a circle names the loop.
- **`healthcheck` and `depends_on = { db = "healthy" }`.** The probe is
  `exec` in the container, so a healthcheck that passes here passes when
  a person runs the same command by hand. `up` waits before starting the
  dependant, and **a service that exits while being waited on fails the
  `up` immediately** rather than burning through thirty retries. Waiting
  on a service that has no healthcheck is refused at parse time - in
  Compose that waits forever.

  It is **not** a probe loop running for the life of the container:
  nothing here reacts to a container becoming unhealthy later. The half
  that is implemented is the half `depends_on` needs.

**That work found a real bug, and it was not in the stacks code.**
Running the probe in-process left `stack up` attached to the probed
container's namespaces, because `setns` does not move the caller - it
moves the caller's next *child*. The next service started inside the
previous one's namespaces and failed with ENOENT for paths that plainly
existed. The probe is now a child process, which is the same rule the
API server follows for the same reason.

And a third pass closed two more:

- **`replicas`** - a service becomes N containers, `<stack>-<service>-1`
  and so on, while a service with one replica keeps its unsuffixed name
  so no existing stack's containers get renamed for a feature it is not
  using. `0` is refused rather than quietly running nothing.

  Replicas are where `/etc/hosts` runs out, and the limit is stated
  rather than discovered under load: a name may appear more than once in
  that file but every resolver takes the first match, so the *service*
  name reaches replica 1 and **nothing round-robins**. Each replica is
  reachable by its own name. Load balancing needs a resolver, and there
  is no daemon here to be one.
- **`stack run`** - `compose run`: a one-off with the service's image,
  environment, volumes and network, a command of your choosing, removed
  when it exits. It does **not** start the service's dependencies:
  starting half a stack as a side effect of a one-off command is how you
  end up with two copies of a database.

A fourth pass closed the last two that were worth closing:

- **`configs` and `secrets`.** Both are read-only bind mounts, which is
  what they are in Compose outside swarm too; a config lands at
  `/<name>` and a secret at `/run/secrets/<name>`. The difference
  between the two keywords here is a check, and it is the one worth
  having: **a secret whose file is readable by anyone but its owner is
  refused**, before anything starts, with the `chmod` to run. A secret
  in a world-readable file in a repository is the most common way one
  leaks, and a tool that mounts it without comment has helped.
- **A healthcheck that keeps running.** A watcher process beside each
  container with a healthcheck - not inside the supervisor, which has
  to stay able to call `clone3` - writes `starting`, `healthy` or
  `unhealthy` to a file that `ps`, the API and the page read. It exits
  when its container does; it is not a daemon and the test asserts that.
  `starting` means "has not passed yet and has not run out of
  attempts", not "unknown", because reporting a container unhealthy in
  its first second is how healthchecks come to be ignored.

  The page shows health as a **second pill** beside the status, not as a
  different colour on the first: a container that is running and
  unhealthy is both of those things, and collapsing them loses the half
  the operator needs.

And `watch`, in the two actions that can be honoured honestly:

- **`rebuild`** builds the image again and replaces the container;
  **`restart`** replaces it without building, which is what a changed
  config file needs. **`sync` is refused by name**, because copying
  files into a running container needs a way to write into its
  filesystem from outside and this engine deliberately has no `cp`. A
  `sync` that quietly rebuilt instead would be worse than not having it.
- `rebuild` on a service with no `build` is refused while the file is
  parsed, rather than discovered when a file changes at midnight.
- It **polls mtimes** rather than subscribing to inotify. A watch that
  misses an event is worse than one that notices a second late, and
  inotify's queue overflows silently under a build that touches a
  thousand files.

### What Compose still has that these stacks do not

The parts of `deploy` beyond `replicas`: **placement constraints and
rollback policy**, which are orchestrator concepts this deliberately is
not - there is no scheduler to place anything and no rolling update to
roll back. `deploy.resources` has no equivalent spelling because the
limits themselves already do: `memory`, `cpus` and `pids_limit` are
service keys here.

And `watch`'s `sync` action, above.

That is the list.

---

## 8. The page, against Docker Desktop

Measured and observed on one macOS machine (Apple Silicon) with Docker
Desktop 4.x running and `docker info` answering.

### What it costs to have open

| | Docker Desktop | rustainer |
|---|---|---|
| macOS helper processes | **718-1041 MiB** across 8-9 processes | — |
| the Linux VM | **3.82 GiB allocated, 8 CPUs** (`docker info`), of which its own status bar reports ~2.19 GB in use | — |
| the UI process | part of the above (Electron) | **3.9 MiB** (`rustainer serve`) |
| on disk | 2.1 GB app bundle | 5.1 MB binary, page included |
| the page itself | Electron | one **53 KB** HTML file |
| loaded from the network | — | **nothing**: no CDN, no font, no framework |
| resident when you are not looking | dockerd + containerd + the VM, always | **0** - `serve` is a process you start and stop |

An earlier version of this section said 718-745 MiB and stopped there.
That was only the macOS-side helpers; **the VM sits on top of it**, and
the figure understated Docker Desktop. Corrected rather than left
flattering.

**The caveat belongs next to the number, not under it.** Most of that
footprint buys a Linux VM on a machine with no Linux kernel. rustainer
does not do that at all: it **requires a Linux host and has no macOS or
Windows story**. On Linux, Docker needs no VM and the gap is the daemon
alone. The honest claim is about the UI process and the engine, not
about replacing the product.

### Side by side, both Containers screens

Docker Desktop's Containers view, read off the running application:

- **Header**: Search (⌘K), help, notifications, learning, settings, a
  grid of apps, **Sign in**.
- **Navigation**: Gordon, Containers, Images, Volumes, Kubernetes,
  Builds, Models, MCP Toolkit (BETA), Docker Hub, Logs, Settings,
  Extensions.
- **Above the table**: `Container CPU usage 1.55% / 800% (8 CPUs
  available)`, `Container memory usage 350.91MB / 3.73GB`, *Show
  charts*; a search box, a column chooser, and an *Only show running
  containers* toggle.
- **Columns**: select, expand, status, Name, Container ID, Image,
  Port(s), CPU (%), Last started, Actions (stop / menu / delete).
- **Status bar**: Engine running, RAM 1.82 GB, CPU 1.87%, Disk 60.97 GB
  used (limit 86.31 GB), a terminal button, *Update available*.

rustainer's: RUNNING / TOTAL / CPU / MEMORY tiles, a filter, *Run a
container*, and columns ID, STACK, STATUS, COMMAND, ADDRESS, CPU,
MEMORY, AGE with details / stop / remove.

| | Docker Desktop | rustainer |
|---|---|---|
| Containers, Images, Volumes | yes | yes |
| per-container CPU and memory | yes | yes |
| **container address in the list** | no - Ports only | **yes**, ADDRESS column |
| **which stack a container belongs to** | expandable grouping | **a column, and a Stacks view** |
| **Networks** | **not in the navigation** - CLI only | **a view**: subnet, bridge, members |
| **Events** | **not in the navigation** | **a view**, colour-coded |
| only-show-running toggle | yes | **yes** — added because of this comparison |
| column chooser | yes | **yes** — added because of this comparison |
| per-container CPU history | full charts | a sparkline in the row |
| Image and Port(s) columns | yes | ports fold into ADDRESS |
| Kubernetes, Models, MCP Toolkit, Hub, Extensions, Gordon, Sign in | yes | no, and not planned |
| Settings for CPU/memory/disk | yes - it owns a VM | nothing to allocate |

**Where Docker Desktop was better, and what was done about it.** Three
things, all found by looking at the two screens rather than by
reasoning about them: an only-show-running toggle, a column chooser,
and per-container CPU history. All three now exist here - the history
as a sparkline in the row rather than a chart panel, because the page
already keeps the samples it needs and a 46-pixel SVG costs nothing.

The fourth was the one that mattered. Its Containers screen reports `Container memory usage 350.91MB / 3.73GB` on
a host where nothing asked for a memory limit. Ours printed `-` for
every container, because `memory.current` exists only where the memory
controller is enabled in the parent's `subtree_control`, and this engine
enables no controller it was not asked for. Looking at the two screens
next to each other is what turned that from a defensible design note
into a missing number.

It now sums the RSS of the processes in the container's cgroup, which
needs no controller and adds no accounting overhead - `cgroup.procs` is
always present and `/proc/<pid>/status` always has `VmRSS`. **It is not
the same number**: it double-counts pages two processes share and omits
page cache the cgroup is charged for. So the cell is labelled `rss`, and
hovering says exactly that. A number with its limits stated beats a dash.

**The limits of this comparison, stated rather than implied.** Docker
Desktop's window can be captured and **keystrokes reach it** - its own
⌘K search opens and takes typing - but its navigation cannot be driven
from a script with the tools here. Four attempts: clicking by
accessibility element, clicking by screen coordinate, navigating via its
⌘K search (which searches Docker Hub and offers Pull/Run, rather than
moving between screens - a difference from this page's palette, which is
local), and setting `AXManualAccessibility`, which left the web content
a single opaque `AXGroup`. So what
is compared above is the Containers screen and the navigation, both read
off the running application, and **nothing else**: not Images, not
Volumes, and not the per-container detail pages, which in recent
versions carry an exec terminal and a file browser. Those may well be
better than anything here. This document does not say otherwise, because
it did not look.

That comparison is still not made, and is tracked as issue #23 rather
than left as a sentence. What changed is this side of it: the page now
has the three things those screens are for, built without sight of the
other one. Images show size, layer count, age, what the image declares
and **which containers are using it**; volumes show size and who mounts
them; both have a two-click "remove unused"; and a running container or
a volume can be browsed file by file. The browser resolves every path
with `openat2(RESOLVE_IN_ROOT)`, because the tree belongs to a
container and a symlink in it is hostile input - stage 17 plants an
absolute link and a climbing one at a host file and asserts neither is
read. Building it found one engine bug: removing one of two tags for
the same image deleted the record the other still needed.

Stated as limits: an image's size is its **compressed** layers, not disk
use; a volume's size stops counting at 20,000 entries and is then shown
as a floor; and a stopped container cannot be browsed.

`start` and `restart` followed, because a Containers screen without them
is a list of things you can only delete. An exited container comes back
as itself - same id, address, published ports, volumes and writable
layer - which stage 17 asserts by having the container append a line to
a file in its own filesystem on every run and counting the lines. It
cost thirty lines, because bringing a container up from its stored
config more than once is what a restart policy already had to do. One
limit: a stack's healthcheck watcher is not restarted with it, so a
started container reports no health rather than a stale one.

### What the page does that is worth something

- **A real terminal into a container**, over a pty, from the browser.
- **The commands it ran**, shown as CLI you could have typed. It teaches
  its own command line instead of hiding it.
- **It says what it does not know, and what kind of number it is
  showing.** Memory is `memory.current` where the controller is enabled
  and summed RSS where it is not, labelled `rss` either way so the two
  are never passed off as interchangeable. The summary tile sums only
  containers whose usage is known - it printed `0B` until this was
  looked at, which reads as "using nothing".

## 9. Reproducing this

```sh
# Sections 6 and 7 run on the same host as section 2:
#   docker exec -e RUSTAINER_BIN=/w/rustainer-musl bench bash scripts/compat-matrix.sh
#   docker exec -e RUSTAINER_BIN=/w/rustainer-musl bench bash scripts/stack-bench.sh

# Build a static binary (any Linux host with a toolchain can skip the container)
docker run --rm --platform linux/arm64 -v "$PWD":/w -w /w \
    -e CC_aarch64_unknown_linux_musl=musl-gcc rust:1.96-slim bash -c '
      apt-get -qq update && apt-get -qq install -y musl-tools
      rustup target add aarch64-unknown-linux-musl
      cargo build --release --target aarch64-unknown-linux-musl
      cp target/aarch64-unknown-linux-musl/release/rustainer /w/rustainer-musl'

# A throwaway host where Docker and rustainer share one kernel
docker run -d --privileged --name bench -e DOCKER_TLS_CERTDIR= \
    -v "$PWD":/w -w /w docker:28-dind
docker exec bench apk add --no-cache bash
docker exec -e RUSTAINER_BIN=/w/rustainer-musl bench bash scripts/bench.sh 40
```

`scripts/bench.sh` runs without Docker present too, and says so rather than
printing a comparison it did not make.
