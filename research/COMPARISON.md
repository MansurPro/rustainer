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

- **Image build** — no `Dockerfile`, no BuildKit, no layer caching for builds.
- **Registry push.**
- **Volumes and bind mounts** — containers have a writable layer and nothing else.
- **User-defined networks** — one bridge, no network creation, no embedded DNS, so containers cannot resolve each other by name.
- **An HTTP API and SDKs** — no daemon, so nothing to connect to.
- **`events`, restart policies, healthchecks, log drivers.**
- **Compose, swarm, plugins.**
- **Checkpoint/restore, Windows containers, cgroup v1 hosts.**
- **Multi-extent rootless** — `/etc/subuid` ranges and the setuid helpers are explicitly not implemented; rootless here maps exactly one id.

The timings above are a fair measure of **starting and supervising a
container**. They are not a claim that one tool replaces the other, and this
engine's purpose was never to replace it — it was to understand the mechanisms
by owning them.

---

## 6. Reproducing this

```sh
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
