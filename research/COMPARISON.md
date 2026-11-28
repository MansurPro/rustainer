# rustainer measured against Docker

What this engine actually costs, next to the one it is modelled on, measured
rather than asserted. Everything here is reproducible with one command, which
is at the bottom.

## Is it 2× or 3× better, and does it still run Docker's images?

**Yes to both, with one exception that is stated rather than averaged
away.** Twelve speed ratios were measured against Docker, on two hosts:

| | ratio (midpoint of the range measured) |
|---|---:|
| network throughput, container to container | 1.15× |
| start detached, stop, remove - arm64 host | 2.4× |
| rebuild an image from cache | 2.8× |
| pull a cold image - arm64 host | 2.85× |
| build an image, no cache | 3.0× |
| pull a cold image - x86_64 host | 3.6× |
| exec in a live container - arm64 host | 3.8× |
| start detached, stop, remove - x86_64 host | 4.0× |
| run a container to exit - x86_64 host | 6.5× |
| run a container to exit - arm64 host | 7.2× |
| bring up a stack with replicas | 9.7× |
| exec in a live container - x86_64 host | 14.5× |

- **The median is 3.7×.**
- **11 of the 12 are at least 2×, and 8 are at least 3×.**
- The one that is not is network throughput, which is the same as
  Docker's and will stay the same: both hand packets to the same kernel
  bridge, and there is nothing in between to make faster.
- **One that is not in the table is a tie: a multi-stage build from
  nothing.** The two build rows above are five trivial steps. A real
  four-stage Node build takes rustainer 5.4-5.5 s and Docker 5.5 s; it
  was 9.0 s before this was measured, and what closed it is in section
  3. Rebuilding the same image after a source edit is 0.85 s to
  Docker's 1.37 s.
- Memory is a separate axis and a larger one: 7× to 18× less per running
  container, and nothing resident when nothing runs (Docker: 120-170 MiB).
- Compatibility: **29 of 29** Docker Hub images behave identically to
  the same image under Docker - same user, directory, `PATH` and exit
  status, and the same owner, group, mode, time and capabilities on
  every one of their 123,217 files (section 6; 30 were offered, 1
  excluded because Docker itself could not pull it). It also reads
  Docker's model artifacts.

So "2× to 3× better with compatibility" is true as a statement about
the typical operation and false as a statement about every operation.
The sections below are where each number comes from.

---

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

### Measured again, eighty changes later

The table above is from when the engine was much smaller. It was run
again on the same machine after the build, Compose, volume, healthcheck,
sub-id and device work (Docker 28.5.2, 20 iterations, two runs):

| measurement | rustainer | Docker | ratio |
|---|---:|---:|---:|
| run a container to exit (median) | 13 ms | 78 ms | 6.0× |
| run a container to exit (p90) | 14–15 ms | 80–82 ms | 5.3–5.9× |
| start detached, stop, remove | 40 ms | 101 ms | 2.5× |
| exec a command in a live container | 9 ms | 27 ms | 3.0× |
| resident per running container | 2 MiB | 18 MiB | 9× |
| resident with nothing running | **0 MiB** | 119–121 MiB | — |
| binaries installed | 5.1 MiB | 200 MiB | 39× |
| pull a cold image (network) | 0.44–0.47 s | 1.1 s | 2.3–2.5× |

Docker's figures have not moved. Rustainer's had: the first re-run read
16 ms, 59 ms and 12 ms for the first three rows, and 3 MiB per
container. The cause was not any one feature. The binary had grown from
5.1 to 8.5 MiB, and it is copied into a sealed memory file every time it
starts - which a single `run` does more than once - so its size is paid
for on every command. The workspace had never had a release profile;
whole-program optimisation and no symbol table put the binary back at
5.1 MiB and took 3 ms off each row. Start-stop-remove was then still
52 ms against the first table's 45, and the reason was the same copy:
`stop` and `rm` each sealed themselves too, though neither goes near a
container's namespaces. A command that only reads the engine's records,
removes files or sends a signal no longer does - anything it starts
seals itself - which takes a bare `ps` from 5.2 ms to 1.6 and that row
to 40 ms. What is left of the gap to the first table is 13 ms against
11-12 for a run and 9 against 7-8 for an exec, both of which do have
to seal, and is what the added features cost.

### The same script on a second host

Everything above is one machine: an arm64 laptop's VM, both engines
inside a container. So the script was run again somewhere that shares
none of that - a stock Debian 13 x86_64 host (kernel 6.12, systemd, 4
vCPU under KVM), rustainer as root in a login shell, and Docker
installed the way a distribution installs it (`apt install docker.io`:
Docker 26.1.5, containerd 1.7.24, runc 1.1.15, started by systemd).
Three runs of `bash scripts/bench.sh 40`:

| measurement | rustainer | Docker | ratio |
|---|---:|---:|---:|
| run a container to exit (median) | 35 ms | 217–237 ms | **6.2–6.8×** |
| run a container to exit (p90) | 36 ms | 251–301 ms | 7.0–8.4× |
| start detached, stop, remove | 58–84 ms | 262–279 ms | 3.3–4.6× |
| exec a command in a live container | 8–10 ms | 123–136 ms | 12–17× |
| resident per running container | 2–3 MiB | 21–23 MiB | 7–11× |
| resident with nothing running | **0 MiB** | 164–172 MiB | — |
| binaries installed | 7.5 MiB | 228 MiB | 30.5× |
| pull a cold image (network) | 0.41–0.45 s | 1.5–1.7 s | 3.3–3.9× |

What carried over is the headline: **running a container is six to
seven times faster on both hosts**, on different architectures, kernels,
Docker versions and ways of installing Docker. Pull and
start-stop-remove moved a little in rustainer's favour.

What did not carry over, in both directions. `exec` is 12-17× here
against 3.5-4.1× there, entirely because Docker's `exec` takes 123-136
ms on this host against 28 ms on the other; why was not investigated,
and the smaller figure is the one to quote. Memory per container is
7-11× rather than 18×: the supervisor is 2-3 MiB on x86_64, not 1.
And this is a different, older Docker (26 against 28), so the two
tables are two measurements, not one measurement repeated.

The idle row needed a fix to be measured at all: the script matched
process command lines, which missed `/usr/sbin/dockerd` and counted the
shell that had the binary's path in its arguments. It matches process
names now.

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

### Build — 2.6 to 3.4× cold, 2.3 to 3.3× from cache

Measured later than the rows above, with `scripts/bench-build-net.sh`,
because a comparison that only times process start-up is a comparison
of the one thing this architecture is good at. Three runs, ten builds
each, no network in the build:

| measurement | rustainer | Docker | ratio |
|---|---:|---:|---:|
| build a 5-step image, no cache | 61–90 ms | 208–240 ms | **2.6–3.4×** |
| rebuild with nothing changed | 33–37 ms | 75–91 ms | **2.3–2.5×** |
| rebuild after a 2 s step | 33–42 ms | 79–139 ms | **2.4–3.3×** |

The first time this was measured the second row read "0.9–1.3×", and
the text under it said what that hid: **there was no layer cache**, the
tie existed only because the steps took milliseconds, and any build
that compiled something would lose to Docker by as long as the build
was. That was the largest real gap this document had found, so the
cache was written, and the third row is the case it exists for - a
two-second step that neither engine runs again.

A step's key is a hash of everything beneath it and of how it runs: the
base image, each `COPY` by the content it copied, each earlier `RUN` by
its own key, then this step's command, environment, working directory,
user and network. Layers are committed as they are produced rather than
when the image is tagged, so a build that fails at step nine starts the
next attempt at step nine. Stage 21 checks this with steps that write a
random value: a reused step is one whose value did not change.

**A build shaped like a real one.** The rows above are five trivial
steps. A four-stage Node application - `apk add nodejs npm`, `npm
install` under a cache mount, a build step, then a runtime stage that
takes `node_modules` and `dist` with `COPY --from --chown` and runs as a
non-root user with a `HEALTHCHECK` - was built by both engines from the
same Dockerfile, the base image pinned by digest, on the same machine
(Docker 29 with BuildKit 0.34 in Docker Desktop's VM; rustainer in a
privileged container in that VM). Three runs of each rebuild:

| | rustainer | Docker | |
|---|---|---|---|
| build from nothing, base image present | 5.40-5.53 s | 5.49 s (one run) | the same |
| rebuild, nothing changed | 0.01 s | 0.20-0.22 s | rustainer ~20× |
| rebuild after a source edit | 0.83-0.88 s | 1.29-1.45 s | rustainer 1.6× |

Read the three rows separately. **From nothing they are level**, and
only since this was measured: the first figure was 9.0 s. A second of
that was pulling a base image Docker already had, which is not the
build. The rest was two things. BuildKit builds the `runtime` stage's
`apk add` while the `base` stage's is still running; this engine built
its stages in turn (7.9 s). It now starts a stage that begins from an
image alongside the ones before it, as far as that stage's first `COPY
--from` - a second process of the same program whose steps land in the
cache, where the main build finds them (6.8 s). And every build
compressed the final image's layers at gzip's default level, which
Docker leaves until a push; the fastest level costs about a tenth more
to upload, once (5.4 s). Docker's cold figure is a single run - each
one leaves build cache on a disk that had 2 GB free - so "the same"
means within the spread of rustainer's three, not a measured tie.

The unchanged-rebuild ratio flatters rustainer: Docker's 0.2 s includes
its CLI on macOS talking to the VM, and nobody waits on either figure.
The row that matters day to day is the third, and it was not always a
win - the first measurement had rustainer at 2.6 s unchanged and 3.4 s
after an edit, behind Docker on both. Three things were wrong: every
build compressed every layer of the final image again; every cache hit
read its whole layer back to check it still hashed to its name; and
every `COPY --from` was done again. What a layer compresses to is now
noted once beside the blob, a hit trusts the store as a `run` of a
pulled image already does, and a `COPY --from` is keyed on the chain of
the stage it reads.

**Larger builds, as an ordinary user.** Two more were built on an
x86_64 machine by a user with no root, no sub-id ranges and no bridge
(`--network host`): a Python service on `python:3.12-slim` that
`apt-get install`s a compiler, `libpq`, and `ffmpeg`, `pip install`s
Django, NumPy, Pillow, psycopg and Celery and runs its migrations; and
a four-stage Next.js application generated by `create-next-app`, built
with `npm ci` and `next build`, shipped as a standalone server. Both
build, both answer HTTP 200, the second reports `healthy`, and
rebuilding it unchanged takes 0.015 s. Neither built at first. That
found four defects, the first three of which are not about builds:

- `apt-get` cannot work for a user with one id: it gives files to other
  users and switches to one of its own. A build step for such a user
  now has those calls answered "done" by its seccomp filter, as
  fakeroot answers them, and apt - which checks - is lent its own
  setting for a builder who cannot switch user. Docker's rootless mode
  solves this with sub-id ranges and a setuid helper; this needs
  neither, and the price is that every file is owned by the one user
  there is, which is already true of every layer that user pulls.
  Where the helper *is* installed and the user has a range, the engine
  now does what Docker does - one user namespace per user with the
  range mapped, held open and joined by every command - and then owners
  are real: an unprivileged `alpine` shows `/etc/shadow` in group 42,
  as under Docker, and a build with `USER app` works.
- A container on the host's network had no `resolv.conf`.
- **A seccomp profile naming a syscall the architecture lacks was
  applied with no rules at all.** On arm64, a profile denying `chmod`
  and `fchmodat` denied neither, because `chmod` does not exist there
  and the code that skips such names discarded every name. The test
  for it ran the command as an ordinary user, for whom `chmod` fails
  anyway, so it passed. It now runs as root in the container.
- `exec` failed for any user without a delegated cgroup, and with it
  every healthcheck.

Those were public stand-ins. The two applications they stand in for
were then built from their own source on the same machine, the same
way: a Django API (`python:3.13-slim`, a compiler, libpq and ffmpeg
from apt, its dependencies and `faster-whisper` from pip,
`collectstatic`) in 124 s, and a three-stage Next.js application in an
npm workspace (`npm ci`, `next build`, a standalone runtime copied with
`--chown`) in 218 s. The API image imports Django 6.1 and its native
dependencies; the Next image reports ready and answers its first
request. Rebuilding the second with nothing changed takes 5.4 s, nearly
all of it reading the 370 MB context to see that nothing changed. Two
more things came out of it: a build secret asked for as a variable
(`--mount=type=secret,...,env=`), now supported, and that an image
declaring `USER node` could not be started at all by a user with one
id - it now runs as that one and says so, while an explicit `--user`
is still refused.

Thirteen Dockerfiles from a production monorepo were then read with
`build --check` and three built. All thirteen parse. The exercise found
three defects, all fixed: an image that lists the same layer more than
once could not be mounted at all (`overlayfs: conflicting lowerdir
path`; one base image repeats an empty layer seven times), `ENV A=1
B=2` set one variable to `1 B=2`, and `RUN --mount=type=secret` was
refused.

One limit, shared with Docker: the key cannot see the outside world.
`RUN apk add curl` is one key whatever the package index says today,
and `--no-cache` is the answer in both. A hit used to re-hash the layer
it reused; it no longer does, for the reason in the table above.

### Network — the same

| measurement | rustainer | Docker | ratio |
|---|---:|---:|---:|
| container to container, 1 GiB | 846–951 MiB/s | 635–939 MiB/s | 1.0–1.3× |
| host to a published port, 1 GiB | 1159–1289 MiB/s | 1049–1210 MiB/s | 1.0–1.1×  |

No difference, which is the expected and the correct result. A packet
between two containers crosses a veth pair and a bridge in the kernel,
and it does that identically whoever created them; neither engine is in
the data path. The one 1.3× is the first run of three and did not
repeat - the other two were 950 against 939 - so it is noise, and is
left in the range rather than trimmed out of it.

So the summary of "how much better" is two numbers, not one. Starting,
stopping, inspecting and building are **2.3 to 9.7 times faster** and
use a fraction of the memory, because there is no daemon in the way.
Moving bytes is **the same**, because the kernel does it and neither
engine is involved.

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

- **BuildKit** — `build` reads the whole Dockerfile language: stages, `COPY
  --from`, `--target`, `.dockerignore`, `ONBUILD`, `ADD <url>`, cache, secret,
  bind and ssh mounts, and a layer cache are all there, and a stage the
  image is not built from is skipped. Stages build side by side: one that starts
  from an image alongside the stages before it, one that continues another
  from the moment that one is done. It is still not BuildKit's solver - a
  stage is only run ahead as far as its first `COPY --from` - and there is
  no remote cache import or export.
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
- **Stacks** (`stack up/down/ps`), from a TOML file or a Docker Compose file.
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
| excluded (Docker could not pull it) | 1 |
| `id -u`, `id -g`, `pwd`, `$PATH` byte-identical | **29/29** |
| exit status of the image's own ENTRYPOINT/CMD identical | **29/29** |
| owner, group, mode, mtime of every file, and file capabilities | **29/29** - 123,217 entries, none differing |

The last run excluded one image (haproxy:3-alpine, which the mirror
refused to Docker that day); earlier runs excluded two and scored 28/28
on the first two rows. The third row is new - see below for why.

**What that table did not check, and what was wrong underneath it.** The
four properties above are about the process. Nothing compared the
*files*, and until this was looked at every file in every pulled image
belonged to root and was dated from the moment of the pull: the layer
extractor wrote contents and modes and dropped owner, group and time.
`/etc/shadow` in alpine was `root:root` instead of `root:shadow`; an
image whose unprivileged user owns its data directory did not. All 28
passed anyway, because their entrypoints either run as root or `chown`
what they need at start - which is exactly why a rate over process
properties could not see it. It was found by a screenshot of the file
browser showing every file "modified 29 s ago".

Fixed, and then measured the way it should have been from the start:
`compat-matrix.sh` has a third probe that runs `find / -xdev` in the
same image under each engine and compares owner, group, mode and mtime
of every entry, plus `getcap` on every executable where the image has
it. The first version of the fix was checked by hand on five images;
the probe then ran over all of them: **29 of 29 images, 123,217
entries, no differences** - from 447 entries in busybox to 15,713 in
golang.

File capabilities were the second half. `security.capability` was
dropped with every other extended attribute, so `arping` and
`newuidmap` in almalinux and fedora had lost theirs; it is kept now,
when extracting as real root. Inside a user namespace it is still
dropped, deliberately - the kernel would rewrite the value to a
namespaced form - and so are all other extended attributes. An image
pulled before this fix keeps its old layers until `pull --force`.

Running the probe also re-measured start-to-exit time on every image,
on one kernel: Docker 75-107 ms, rustainer 12-42 ms, **5.6× at the
median** (2.5× for traefik, the one outlier; 4.9× to 6.6× for the rest).

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

And `watch`, in all three of Compose's actions:

- **`rebuild`** builds the image again and replaces the container;
  **`restart`** replaces it without building, which is what a changed
  config file needs; **`sync`** copies changed files into the running
  container and replaces nothing. `sync` was refused by name until the
  file browser needed a path walk a container cannot redirect; the same
  walk made writing safe. Stage 18 has the container plant a symlink at
  the destination pointing at a host directory and asserts nothing
  arrives there. It does not propagate deletions or copy symlinks.
- `rebuild` on a service with no `build` is refused while the file is
  parsed, rather than discovered when a file changes at midnight.
- It **polls mtimes** rather than subscribing to inotify. A watch that
  misses an event is worse than one that notices a second late, and
  inotify's queue overflows silently under a build that touches a
  thousand files.

### Compose files themselves

For a long time the answer to "can it run my `docker-compose.yml`" was
no: a stack was a TOML file with Compose's ideas and its own spelling.
`stack up -f docker-compose.yml` now reads the YAML. It is a
translation, not a second engine - the file becomes the same table a
`rustainer.toml` parses to, and everything after that is the stack code.

What is translated: `image`, `build` (context, `dockerfile`, `args`),
`command` and `entrypoint` as a string or a list, `environment` as a
list or a mapping, `env_file`, `ports` and `volumes` in short and long
form, `depends_on` with `service_started`, `service_healthy` and
`service_completed_successfully`,
`healthcheck` (`CMD`, `CMD-SHELL`, durations like `1m30s`), `restart`,
`profiles`, `user`, `working_dir`, `mem_limit`, `cpus`, `pids_limit`,
`privileged`, `read_only`, `cap_add`, `cap_drop`, `security_opt`,
`devices`, `platform`, `network_mode`, `pid`, `tmpfs`, `extra_hosts`,
`configs`, `secrets`,
`deploy.replicas`, `deploy.resources.limits`, `develop.watch`,
`extends` from the same file or another, YAML anchors and `<<`, the
`!override` and `!reset` tags, and the shell's substitution forms
(`${V-d}`, `${V:-d}`, `${V:?msg}`, `${V:+x}`, and a bare `$V`). With
no `-f`, `compose.yaml` and Compose's other three names are looked for
after `rustainer.toml`.

What is not, in two kinds. Keys that do not change whether the
application works are dropped and **said to be** on stderr:
`container_name`, `hostname`, `labels`, `logging`, `networks`,
`stop_grace_period`, `init`, `oom_score_adj`, `cpu_shares` and a few
more. Anything else - `uts`, `userns` remapping, swarm-only `deploy` keys
- is refused by name before anything starts. Several `-f` Compose files are merged by Compose's rules - a list in a
later file adds to the earlier one, and an entry naming the same
variable or container path replaces it - where two stack files replace
a list whole. One difference in behaviour remains: a service's
containers are named `<stack>-<service>`, not `<project>-<service>-1`,
unless the file gives a `container_name`, which is used as it is.

It was checked against a real project rather than a file written to
pass: a development stack of Postgres 16, Redis, MinIO and a one-shot
bucket-creation job, written for Docker Compose across three files with
`extends`, `!override` and anchors, came up unmodified, with
Postgres accepting connections, Redis answering and the job finding
MinIO by name. The project's largest file was refused at first, for
`pid: host` on a metrics exporter; `run --pid host` exists now, and
that file parses. Only the development stack was brought up - the
others want registry images and production secrets.

`--pid host` had a consequence worth writing down. With no pid
namespace nothing dies with the container's first process, so what it
started lived on holding the log pipes, and `stop` waited five seconds
on a supervisor that was waiting for them. The supervisor now kills the
container's cgroup when that process exits.

Getting there found three bugs in the engine, none of them about YAML:

- **`/dev/null` was read-only to everyone but root.** `mknod`'s mode is
  cut down by the caller's umask, so the device nodes were `0644`.
  Postgres' entrypoint drops to its own user and redirects to
  `/dev/null`; it died on line 352.
- **A read-only volume was also `noexec`.** `-v src:dst:ro` reused the
  attribute set written for `/sys`. A script on a read-only volume could
  be read and not run, and an entrypoint that asks `[ -x ]` before
  choosing between running and sourcing chose wrongly.
- **`--privileged` made read-only volumes writable.** It removes the
  read-only attribute from `/sys` by removing every operation of that
  kind, and a volume's `:ro` was one of them.

### What Compose still has that these stacks do not

**Placement** across hosts: there is one host, so nothing chooses
between them. What is here is what a one-node swarm does with the same
file - `placement = ["node.platform.arch == aarch64"]` is evaluated
against this machine, and a constraint that does not hold refuses the
whole stack before anything starts. Hostname, OS and architecture are
the keys one machine can answer for itself; `node.role` and
`node.labels.*` are refused by name, since only an orchestrator could
have assigned them. `deploy.resources` has no equivalent spelling
because the limits themselves already do: `memory`, `cpus` and
`pids_limit` are service keys here.

Rollback, the other part, is here in the one form a single host can
honour. `stack update <service>` stops the old container and **sets it
aside** rather than removing it, starts the new one under the same name
and address, and lets the service's own healthcheck decide: healthy, and
the old one is deleted; not, and the new one is removed and the old one
started again exactly as it was. Stage 18 gives each version a file only
it could have written and reads it back after a failed update. Two
things it is not: zero-downtime, because two containers cannot hold one
published port and there is no proxy in front; and available without a
healthcheck, because "it started" is not evidence that it works.

Writing it found a bug in `up`: on a half-running stack it allocated a
fresh address for every service, including the running ones, and wrote
those into the new services' `/etc/hosts` - so a service started later
could not find one already running. An existing container now keeps the
address it has.

`watch`'s `sync` propagating deletions, above.

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
| Kubernetes, Models, MCP Toolkit, Hub, Extensions | yes | **yes, all five** - built after this table was first written; see below |
| Gordon (the assistant), a sign-in screen | yes | no |
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

That comparison against the running application is still not made, and
is tracked as issue #23 rather than left as a sentence; the one against
Docker's documentation is below. What changed is this side of it: the page now
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
as a floor; and a stopped container shows only its writable layer - the
files it created or changed - because the merged view went with its
mount namespace.

`start` and `restart` followed, because a Containers screen without them
is a list of things you can only delete. An exited container comes back
as itself - same id, address, published ports, volumes and writable
layer - which stage 17 asserts by having the container append a line to
a file in its own filesystem on every run and counting the lines. It
cost thirty lines, because bringing a container up from its stored
config more than once is what a restart policy already had to do. One
limit: a stack's healthcheck watcher is not restarted with it, so a
started container reports no health rather than a stale one.

### The three screens, against the running application

First compared against Docker's documentation, because the window could
not be driven; then against the application itself - Docker Desktop
4.98.0, engine 29.9.2 - once a way in was found. A **second** dashboard
window started with its own profile directory and
`--remote-debugging-port` attaches to the same engine, and the Chrome
DevTools protocol then navigates it and reads each screen back as text
and as a screenshot. Nothing about the engine is disturbed.

That is not how the first attempt went, and it is worth writing down:
restarting the *existing* dashboard process with the flag took the whole
engine down with it, stopping every running container. The dashboard is
not a view that can be closed independently. Leave it alone and start
another.

What the application shows that the documentation did not make plain:

- **Images** list an in-use marker, a disk-in-use bar with a total, a
  search box, a filter, a column picker and bulk selection. An image's
  page lists **every layer with its command and size, grouped by which
  base image it came from**, beside Scout's vulnerabilities and packages
  - 18 and 187 for `caddy:2` - searchable and filterable by "fixable".
- **Volumes** have search, a filter, a column picker, bulk selection and
  a creation date. A volume's page has three tabs: stored data, the
  containers using it, and its exports.
- **A container's Files tab marks each entry `ADDED` or `MODIFIED`
  against the image**, which is a diff, not just a listing, and offers a
  file editor. Its Stats tab is four charts with axes, limits and zoom:
  CPU, memory, disk read/write and network I/O.

The tables below are corrected against that.

**Images**

| | Docker Desktop | this page |
| --- | --- | --- |
| In use / unused shown | yes, plus "dangling" | yes; no dangling state |
| Search and bulk select | yes | yes |
| Filter by state, column picker | yes | yes |
| Disk used by images, as a total | yes, on disk | the sum of what the images would cost to pull, labelled as that |
| Remove unused | bulk select, shows space reclaimed | two-click "remove unused", no space figure |
| Run from an image, with options | yes | yes, the same dialog as Run |
| Pull | yes | yes |
| Size | on disk | compressed layers, labelled |
| Layers and history | each layer with its command and size, grouped by base image | per-layer sizes and the build history, as two lists |
| Packages and CVEs | Scout: OS and application packages, grouped by layer | a scan against OSV: apk and dpkg packages, Python and Node packages, each finding with the layer that introduced it |
| Push from the UI | yes | yes, with the credentials the server was started with |
| Registry | "My Hub": your repositories, signed in | the repositories of any Docker Hub namespace and the tags of any repository; with `RUSTAINER_REGISTRY_AUTH=user:token` in the environment, your own including private ones, every page, and pulls of private images |

**Volumes**

| | Docker Desktop | this page |
| --- | --- | --- |
| Which containers use it | yes | yes |
| Search, bulk select, creation date, total size | yes | yes |
| Filter by state, column picker | yes | yes |
| Browse stored files | yes | yes |
| Size | yes | yes, a floor past 20,000 entries |
| Delete, and remove unused | yes | yes, with the space it would free on the button |
| Create an empty volume | yes | yes |
| Empty without deleting | yes, stopping and restarting its containers | yes, refused while a running container mounts it |
| Clone | yes | yes |
| Export / import | file, image, registry, cloud | a tar file, both from the page; 64M through the page, any size from the CLI |

**A container's detail**

| | Docker Desktop | this page |
| --- | --- | --- |
| Logs | live, with search and timestamps | tail, refreshed twice a second, with a line filter and timestamps to 0.1 s |
| Inspect | yes | yes |
| Run a command | Exec, or Docker Debug | one-shot exec, and a pty terminal |
| Files: browse | yes | running, and a stopped one's own writes |
| Files: what differs from the image | each entry marked ADDED or MODIFIED | the same two marks, from the writable layer, and a third for what it deleted |
| Files: download | yes | yes |
| Files: edit, upload | yes | yes, up to 64M per upload; mode and owner kept on edit |
| Stats | four charts with axes, limits and zoom | the same four series over the last minute, each with its peak and time span, a choice of the last 1, 5 or 15 minutes, a click to enlarge, and the memory limit drawn as the top of its chart; history starts when the page is opened; disk only with the io controller |
| Bind mounts tab | yes | yes: each mount's source, target and whether it is read-only |
| Start / stop / restart / delete | yes | yes |

The first version of this table was a list of what to build, and most
of it then was: volume create, empty, clone, export and import; file
download; a log filter; network and disk counters; per-layer sizes,
build history and push. Volume import reuses the image layer extractor,
because an archive is hostile input whoever made it - stage 17 imports
one whose entry is named `../../outside-volume` and asserts nothing
appears there.

**A scan** came last, because it is the one feature here that talks to
a third party. `rustainer scan <image>` reads the image's own package
database and asks OSV about each package; what it sends is a package
name, a release and a version, and stage 22 asserts exactly that against
a stand-in server - no image name, path, digest or hostname in any
request. For `nginx:1.27-alpine` it reports 142 known vulnerabilities in
58 packages in about five seconds, worst first, each with the release it
is fixed in.

It reads more than it first did. Beside the distribution's database it
finds Python distributions (`*.dist-info`) and Node packages
(`node_modules/*/package.json`) wherever they sit in the layers, and it
says **which layer introduced each package** - the lowest one whose copy
of the database, or whose files, has it at that version - because that
is the layer to rebuild. `python:3.9-alpine` reports 73 findings across
Alpine and PyPI in about two seconds, `pip` and `setuptools` among them
at layer 3 of 4.

What it still cannot see is anything that leaves no record on disk: a
statically linked binary, the modules compiled into a Go or Rust
program, a vendored copy without metadata; and nothing RPM-based, whose
database is SQLite. An image where it finds nothing is reported as
having nothing to look up rather than nothing wrong. A package an upper
layer deleted is still reported, which errs toward saying too much.

**A Builds view**, which Docker Desktop has as its own section, lists
every build this engine has done - taken from the event log, so there
is no second record to disagree with it - with how long it took, how
many steps came from the cache, and for a build that failed, the step
and exit status. Each can be rebuilt from the page, with or without the
cache. Docker Desktop's own Builds screen, read from the running
application, has a build history and an active-builds tab, a builder
selector, import of build records, search, and columns for name, id,
builder, duration and age; opening a build there shows per-step timings,
logs and provenance. This has the history, the duration and the
rebuild; it has no active-builds view, one builder, and none of the
per-step detail.

**MCP servers**, which Docker Desktop has as its MCP Toolkit. A server
packaged as an image is a program that speaks a protocol on stdin and
stdout, so running one needs nothing new except a thing this engine
turned out not to have: `run -i` without a terminal left the container's
stdin on `/dev/null`. With that fixed, `mcp/time` from Docker Hub
completes the MCP handshake, lists its tools and answers a call, driven
through a pipe. The page lists the hundred most pulled servers in Hub's
`mcp` catalogue - Hub refuses an anonymous caller more than that - takes
any other server by name, and gives the exact
`mcpServers` entry a client wants, with secrets left as placeholders to
fill in rather than written on a command line. What it does not have is
Docker's **gateway**: one endpoint that fronts many servers, holds their
secrets and handles their OAuth. Here each client starts its own.

**Extensions**, in a format of this project's own and deliberately a
small one: a directory with a stack file, which says what to run, and
an `extension.toml`, which says what it is for and where its page is.
Enabling one is `stack up`; the page then shows its UI in a sandboxed
frame. The address must be on this machine, because a frame that an
extension could point anywhere is somebody else's page inside this one.
It is **not Docker's extension format** and cannot load anything from
Docker's marketplace: there is one extension, the example in
`extensions/hello`, and no SDK for an extension to call back into the
engine.

**Kubernetes**, which Docker Desktop offers as a switch. Here it is
`rustainer kube up`: k3s, running as a node inside one container of this
engine. Getting there needed three things the engine did not have, and
all three are compatibility fixes in their own right:

- **`--privileged`** - every capability, no syscall filter, nothing
  masked, `/sys` and the container's own cgroup writable, every cgroup
  controller delegated, and the host's kmsg, fuse, loop and tun devices.
  One explicit flag, off by default, implemented as an edit of the
  finished mount plan so that an ordinary container's plan is not
  touched by its existence. A process in such a container is root on
  the host in every way that matters.
- **Arguments after the image name are arguments to its ENTRYPOINT.**
  This engine replaced the entrypoint as well as the CMD, which is right
  only for images that have none - most small ones, and so most of the
  images it had been tested with. `run rancher/k3s server` was looking
  for a program called `server`. `--entrypoint` now exists to replace it
  on purpose.
- **A container's nested cgroups are removed with it.** A kubelet builds
  a tree several levels deep; left behind, it made the next container of
  the same name fail at `clone3` with EBUSY.

`scripts/kube-test.sh` is the evidence: the node becomes Ready, a
two-replica nginx deployment rolls out, its service answers on a cluster
address, the API server is reachable from the host, and after `kube
down` and `kube up` the deployment is still there. It runs in CI as
its own job, since it needs three image pulls and several minutes. It passes 8 of 8 in
two places: nested inside Docker Desktop's VM, and on a stock Debian 13
host (kernel 6.12, systemd, booted for the purpose under KVM) as root.

The second place found two things the first could not, because a
container inside Docker's VM inherits a host someone already arranged:

- **A login session's cgroup has no `cpu` controller to give.** systemd
  hands a session scope `memory` and `pids`; k3s stopped with *"failed
  to find cpu cgroup (v2)"*. A privileged container is now placed in
  `/sys/fs/cgroup/rustainer`, directly under the root, whenever that has
  more controllers than the cgroup the command was typed in - where
  Docker's cgroupfs driver keeps `/docker`, and for the same reason.
  The same scope also refused `--memory` and `--cpus` to *every*
  container started from a root shell, since it cannot enable a
  controller while it holds that shell; a container with a limit goes
  to the same place now.
- **A host on systemd-resolved got public DNS, on a network that blocks
  it.** `/etc/resolv.conf` there names only `127.0.0.53`, which means
  nothing in another network namespace, so containers were given
  `1.1.1.1` - and every image pull inside the cluster timed out on a
  name lookup. The resolvers systemd-resolved forwards to are in
  `/run/systemd/resolve/resolv.conf`, and they are used now, as Docker
  uses them. The public pair remains the last resort.

One thing it needs that it will not do for you: `sysctl -w
net.ipv4.ip_forward=1`. A fresh distribution has it off, the engine
refuses to change a host-wide setting, and says so.

With those fixed, the whole stage suite was run on that host as root:
23 of the 24 stages, 642 assertions, none failing and none skipped
(stage 11 is the rootless stage and refuses to run as root; two
assertions of stage 18 failed once in the first run and not in three
repeats).

Root and Linux only; there is no rootless cluster - tried, on a shared
machine without root: bridged networking needs a firewall tool the
unprivileged user cannot drive, and a single-uid user namespace cannot
unpack the images a cluster pulls. Docker Desktop's own
Kubernetes was switched off on the machine this was compared on, so its
screen was seen only in that state: a "create cluster" button.

**Models.** Docker publishes models in Docker Hub's `ai/` namespace as
OCI artifacts whose layer is a GGUF file, and its Model Runner serves
them on port 12434. Here `rustainer model pull ai/smollm2:135M-Q4_K_M`
downloads that layer - streamed to disk and verified against its
digest, since a model does not fit in memory the way an image layer
does - and `model run` starts a llama.cpp server container with the
file mounted read-only and an OpenAI-compatible API on the same port.
The page lists them, runs and stops them, and has a box to ask the
running one a question. `scripts/model-test.sh` (12 assertions, its own
CI job) pulls a real model and gets an answer out of it.

What it is not: Docker's runner is a host process with Metal and CUDA
behind it. This is a container on the CPU unless you name a GPU build
of the server with `--image`. Measured on an Apple-silicon Mac with the
same `ai/smollm2:360M-Q4_K_M` file, 256 tokens per request, five
requests each: Docker's Model Runner (llama.cpp on Metal, on the host)
generated 210-240 tokens per second; `rustainer model run` (llama.cpp on
8 CPU threads, inside the Linux VM) generated 39-70. **Docker's runner
is 3-6x faster here**, and the reason is the GPU, which a Linux
container on a Mac cannot reach under any engine. Its first answer after
an unload took 1.6 s, against roughly 15 s for a first `model run` here,
which includes pulling the server image.

Engine against engine it is a tie: the same server image and the same
file under Docker and under this, on the CPU, in the same VM, ran at
3-12 tokens per second under both in an earlier session (slower than
the 39-70 above for a reason not found), the spread within
one engine wider than any gap between them, with the server answering
`/health` 0.40-0.62 s after `run` here against 0.61-0.71 s under Docker.
A model is a CPU-bound process and a container engine is not in its way.

**A GPU inside a container works, and was checked on real hardware.**
`run --gpus all` binds the machine's GPU device nodes (`/dev/dri`, and
NVIDIA's and AMD's compute nodes where they exist). On the one Linux
machine available with a graphics card - a 2014 FirePro W5100 - an
unprivileged container built from `alpine` with Mesa reports its
OpenGL renderer as `llvmpipe` (software) without the flag and as
`BONAIRE (radeonsi, ... DRM 2.50)` with it: the card. No root, no
daemon, nothing installed on the host.

What that does not show is a model running on a GPU. llama.cpp's GPU
backends want Vulkan or CUDA; this card is bound to the `radeon`
kernel driver, which has no Vulkan, and moving it to `amdgpu` needs
root and a reboot of a shared machine. So the Model Runner comparison
above stands as measured, CPU against GPU.

**On an NVIDIA machine it works, and a model runs on the GPU.** An
NVIDIA container needs the userspace libraries that match the host's
kernel module, and those cannot be in an image. The vendor's answer is
a CDI spec - a file `nvidia-ctk cdi generate` writes, listing the
device nodes, the libraries to mount, the environment and some hooks -
which Docker (25 and later) and Podman read. This engine reads them
too (`--device nvidia.com/gpu=all`), and where there is no spec
`--gpus all` asks the driver's own `nvidia-container-cli list` for the
same information. Nodes and libraries are binds. Hooks are not
executed; the two every GPU spec has are done by what they are for -
the links `create-symlinks` lists, and for `update-ldcache` a link
from each mounted library's soname, read out of the ELF file.

Checked on a shared DGX machine - four A100-SXM4-80GB, driver
580.178.04, no CDI spec, no `nvidia-ctk` - as an ordinary user:

- `rustainer run --gpus all ubuntu:22.04 nvidia-smi` lists the same
  five devices, driver and memory the host's `nvidia-smi` does. The
  image is stock Ubuntu: `nvidia-smi` and the 28 driver libraries are
  the host's, bound in, and `libnvidia-ml.so.1` and `libcuda.so.1`
  resolve to them. Without the flag the same image has neither the
  program nor `/dev/nvidia0`.
- llama.cpp's CUDA server in a container, `ai/smollm2:360M-Q4_K_M`,
  400 tokens: **418-458 tokens/s on one A100** (1088 MiB of its memory
  in use by the server, by the host's own accounting), against 84-85
  tokens/s for the same image on 8 CPU threads. 5x, from a flag.

Two things about that machine shaped how it was run, and neither is
hidden. It forbids unprivileged user namespaces to ordinary programs
(Ubuntu's AppArmor restriction), so rustainer could not create one; it
was run inside the namespace `podman unshare` opens, which is allowed
to. And it has no bridge for such a user, so the server was on the
host's network. Docker is installed there and was not usable by this
user, so there is no Docker figure from that machine beside these.

That changes what the Model Runner comparison above says. On the Mac,
Docker's runner is 3-6x faster because it has the Mac's GPU and a
Linux container there has none; that is still true and will stay
true. On a machine whose GPU a container *can* reach, this engine's
model server runs on it, at twice the tokens per second Docker's
runner reached on the Mac's.

It loads GGUF only; the `-mlx` and `-safetensors`
tags in the same catalogue are refused with a sentence saying why.

Building it found a bug that had nothing to do with models. The server
died at start with *"Operation not permitted"*: the default seccomp
filter answered `clone3` with EPERM, and glibc since 2.34 starts every
thread with `clone3`, falling back to `clone` only when told the call
does not exist. It is answered ENOSYS now, as Docker's profile does,
which meant teaching the filter compiler to carry two error codes.
`unshare` and the rest are still EPERM.

Private repositories work with a personal access token in
`RUSTAINER_REGISTRY_AUTH`: checked against a real account, the private
repository was listed, its 102 tags were listed, and its manifest was
served signed in and refused (401) anonymously. The image itself - 2.6 GB
extracted, Debian 12 - then pulled in 35 s and ran. There is no sign-in
*screen* - the token comes from the environment the server was started
in, never from the browser - and nothing else of an account (teams,
subscriptions, Scout). Search, bulk selection, totals, the added/modified marks and
chart axes were added after seeing the application have them.

`pull` and `run` take Docker's `--platform`. On an arm64 machine
`run --platform linux/amd64 alpine uname -m` prints `x86_64`, through the
same kernel-registered emulator Docker uses (Rosetta or QEMU via
binfmt_misc - the engine supplies none of its own). A platform the image
has no build for is refused, including when the image is a single
manifest with no index to choose from. As with Docker, the tag then
points at whichever platform was pulled last.

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
#   docker exec -e RUSTAINER_BIN=/w/rustainer-musl bench bash scripts/bench-build-net.sh

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
