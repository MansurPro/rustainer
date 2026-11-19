#!/bin/bash
# Two more rows for research/COMPARISON.md: building an image, and moving
# bytes over the network. Same rules as scripts/bench.sh - both engines
# on one kernel, in a docker-in-docker container, medians of N runs.
#
#   bash scripts/bench-build-net.sh [iterations]
#
# These are the two rows most likely NOT to favour this engine, which is
# why they are measured. A build is dominated by running the RUN steps,
# which is the same work for anybody; and a packet between two containers
# on a bridge takes the same path through the same kernel whoever set the
# bridge up. A comparison that only timed process start-up would be a
# comparison of the one thing the architecture is good at.

set -u
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 1
N=${1:-10}
IMG=${RUSTAINER_BENCH_IMAGE:-alpine:3.21}
B="${RUSTAINER_BIN:-target/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'; set RUSTAINER_BIN" >&2; exit 2; }
command -v docker >/dev/null 2>&1 || { echo "needs docker, to compare against" >&2; exit 2; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-bench2.XXXXXX")
# The store must be on a real filesystem: an overlay cannot host an
# upperdir, and a tmpfs would time RAM rather than disk for one engine.
export RUSTAINER_ROOT="$WORK/state"
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_BENCH_STORE:-/var/lib/docker/rustainer-bench2}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    docker rm -f bn-srv bn-cli >/dev/null 2>&1
    docker rmi -f bn-built >/dev/null 2>&1
    rm -rf "$WORK" "$RUSTAINER_IMAGE_ROOT"
}
trap cleanup EXIT

# shellcheck source=scripts/now-us.sh
. scripts/now-us.sh
median() { sort -n | awk '{ a[NR] = $1 } END { print (NR % 2) ? a[(NR + 1) / 2] : int((a[NR / 2] + a[NR / 2 + 1]) / 2) }'; }
ratio() { awk -v a="$1" -v b="$2" 'BEGIN { if (a <= 0 || b <= 0) print "-"; else printf "%.1fx", a / b }'; }
row() { printf '| %s | %s | %s | %s |\n' "$1" "$2" "$3" "$4"; }

"$B" pull "$IMG" >/dev/null 2>&1 || { echo "cannot pull $IMG" >&2; exit 2; }
docker pull -q "$IMG" >/dev/null 2>&1 || { echo "docker cannot pull $IMG" >&2; exit 2; }

echo "kernel $(uname -r) $(uname -m); docker $(docker version --format '{{.Server.Version}}'); $N runs each"
echo
echo "| measurement | rustainer | Docker | ratio |"
echo "|---|---:|---:|---:|"

# --- build ---------------------------------------------------------------
# No network in the build, and no cache: three RUN steps and a COPY, which
# is the shape of a small real Dockerfile. `--no-cache` for Docker because
# a cached build measures a lookup; this engine has no layer cache at all,
# which is said in the document rather than hidden behind this flag.
CTX="$WORK/ctx"; mkdir -p "$CTX"
head -c 1048576 /dev/urandom > "$CTX/blob"
cat > "$CTX/Dockerfile" <<DOCKERFILE
FROM $IMG
RUN echo one > /one
COPY blob /blob
RUN sha256sum /blob > /sum
ENV BUILT=yes
RUN mkdir -p /app && echo two > /app/two
DOCKERFILE
build_times() {
    for _ in $(seq "$N"); do
        s=$(now_us); "$@" >/dev/null 2>"$WORK/err" || { echo "build failed: $*" >&2; tail -3 "$WORK/err" >&2; return 1; }
        e=$(now_us); echo $(( (e - s) / 1000 ))
    done
}
R=$(build_times "$B" build --network none -t bn-built:1 "$CTX" | median)
D=$(build_times docker build --no-cache --network none -q -t bn-built "$CTX" | median)
row "build a 5-step image, no cache (median)" "${R} ms" "${D} ms" "$(ratio "$D" "$R")"

# What a rebuild costs when nothing changed - the case a layer cache
# exists for, and the one this engine has no answer to.
D2=$(build_times docker build --network none -q -t bn-built "$CTX" | median)
row "rebuild with nothing changed (median)" "${R} ms (no cache)" "${D2} ms (cached)" "$(ratio "$D2" "$R")"

# --- network: container to container ------------------------------------
# One gigabyte of zeroes through busybox nc, between two containers on
# each engine's default bridge. MiB/s, so HIGHER is better and the ratio
# is rustainer over Docker.
MB=1024
xfer() { # $1 = how to run a shell command in the client; prints MiB/s
    for _ in $(seq 5); do
        s=$(now_us); "$@" >/dev/null 2>&1; e=$(now_us)
        awk -v us=$(( e - s )) -v mb=$MB 'BEGIN { printf "%d\n", mb / (us / 1000000) }'
    done
}
"$B" run -d --name bn-srv "$IMG" /bin/sh -c 'while true; do nc -l -p 9000 > /dev/null; done' >/dev/null 2>&1
"$B" run -d --name bn-cli "$IMG" /bin/sleep 600 >/dev/null 2>&1
sleep 1
RIP=$("$B" exec bn-srv /bin/sh -c "ip -4 -o addr show | grep -v ' lo ' | awk '{print \$4}' | cut -d/ -f1" 2>/dev/null | tail -1)
RN=$(xfer "$B" exec bn-cli /bin/sh -c "dd if=/dev/zero bs=1M count=$MB 2>/dev/null | nc $RIP 9000" | median)
"$B" rm --force bn-srv >/dev/null 2>&1; "$B" rm --force bn-cli >/dev/null 2>&1

docker run -d --name bn-srv "$IMG" /bin/sh -c 'while true; do nc -l -p 9000 > /dev/null; done' >/dev/null 2>&1
docker run -d --name bn-cli "$IMG" /bin/sleep 600 >/dev/null 2>&1
sleep 1
DIP=$(docker inspect -f '{{.NetworkSettings.IPAddress}}' bn-srv)
DN=$(xfer docker exec bn-cli /bin/sh -c "dd if=/dev/zero bs=1M count=$MB 2>/dev/null | nc $DIP 9000" | median)
docker rm -f bn-srv bn-cli >/dev/null 2>&1
row "container to container, 1 GiB (median of 5)" "${RN} MiB/s" "${DN} MiB/s" "$(ratio "$RN" "$DN")"

# --- network: host to a published port ----------------------------------
# The path a browser takes. Docker forwards with NAT rules and falls back
# to docker-proxy; this engine forwards with NAT and a forwarder process.
"$B" run -d --name bn-srv -p 19000:9000 "$IMG" /bin/sh -c 'while true; do nc -l -p 9000 > /dev/null; done' >/dev/null 2>&1
sleep 1
RP=$(xfer /bin/sh -c "dd if=/dev/zero bs=1M count=$MB 2>/dev/null | nc 127.0.0.1 19000" | median)
"$B" rm --force bn-srv >/dev/null 2>&1
docker run -d --name bn-srv -p 19000:9000 "$IMG" /bin/sh -c 'while true; do nc -l -p 9000 > /dev/null; done' >/dev/null 2>&1
sleep 1
DP=$(xfer /bin/sh -c "dd if=/dev/zero bs=1M count=$MB 2>/dev/null | nc 127.0.0.1 19000" | median)
docker rm -f bn-srv >/dev/null 2>&1
row "host to a published port, 1 GiB (median of 5)" "${RP} MiB/s" "${DP} MiB/s" "$(ratio "$RP" "$DP")"
