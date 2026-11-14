#!/bin/bash
# Measure rustainer against Docker on one machine, one kernel, one disk.
#
#   bash scripts/bench.sh [iterations]
#
# Run it where BOTH engines exist. The intended host is a throwaway
# docker-in-docker container, which is how you get Docker and this engine
# onto the same kernel without installing Docker on a development machine:
#
#   docker run --rm --privileged -v "$PWD":/w -w /w \
#       -e RUSTAINER_BIN=/w/target/release/rustainer docker:28-dind \
#       sh -c 'dockerd-entrypoint.sh >/dev/null 2>&1 & sleep 10
#              apk add --no-cache bash >/dev/null; bash scripts/bench.sh'
#
# What is being compared, stated plainly, because a benchmark that does not
# say this is an advertisement:
#
#   `docker run` is CLI -> HTTP over a unix socket -> dockerd -> gRPC ->
#   containerd -> a shim process -> runc -> clone. `rustainer run` is CLI ->
#   supervisor -> clone3. Those are different architectures, not two
#   implementations of one, and the architecture is most of the difference
#   below. Including the daemon round-trip is fair, because it is what the
#   user waits for. Leaving out the work Docker does that we do not - image
#   building, volumes, user-defined networks, an HTTP API, plugins, swarm,
#   log drivers, healthchecks - would NOT be fair, which is why the summary
#   ends by listing it rather than hiding it.
#
# Every timing is a wall-clock median over N runs with the image already in
# the local store, so nothing measures the network except the pull row,
# which says so.

set -u
# A comma decimal separator would break the microsecond arithmetic below.
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 1

N=${1:-20}
IMG=${RUSTAINER_BENCH_IMAGE:-alpine:3.21}
WORK=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-bench.XXXXXX")

B="${RUSTAINER_BIN:-${CARGO_TARGET_DIR:-target}/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'; set RUSTAINER_BIN" >&2; exit 2; }

# overlayfs refuses to host an upperdir, and a container's writable layer
# is an upperdir. In a container - which is where this benchmark is meant
# to run - the default work directory is exactly that, so every timing
# would be a time-to-failure rather than a time-to-run.
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$WORK")" = overlayfs ]; then
    mount -t tmpfs -o size=6g tmpfs "$WORK" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$WORK/state"
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_BENCH_STORE:-$WORK/images}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"

cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED_TMPFS" = yes ] && umount "$WORK" 2>/dev/null
    rm -rf "$WORK"
}
trap cleanup EXIT

HAVE_DOCKER=no
if command -v docker >/dev/null 2>&1 && timeout 30 docker info >/dev/null 2>&1; then
    HAVE_DOCKER=yes
fi

# --- helpers ------------------------------------------------------------

# Microseconds since the epoch.
#
# NOT `date +%s%N`: busybox's date has no %N and prints a literal "N", so
# every interval computes as zero and the whole benchmark reports 0 ms
# without failing. Alpine - which is what a docker-in-docker image is -
# has busybox date even once bash is installed. bash 5's EPOCHREALTIME is
# built in, needs no process, and cannot silently degrade.
if [ -n "${EPOCHREALTIME:-}" ]; then
    now_us() { u=${EPOCHREALTIME}; echo "${u/./}"; }
elif [ "$(date +%N 2>/dev/null)" != "N" ] && [ -n "$(date +%N 2>/dev/null)" ]; then
    now_us() { echo $(( $(date +%s%N) / 1000 )); }
else
    echo "no sub-second clock here: need bash 5 (EPOCHREALTIME) or GNU date" >&2
    exit 2
fi

# Median and p90, in milliseconds, of N runs of a command. Measured rather
# than `time`d, whose resolution and output format both vary by shell.
# A sample from a command that FAILED is a time-to-failure, and averaging
# those with real ones produces a number that is not about anything. The
# whole measurement is abandoned instead, loudly.
TIMEIT_FAILED=no
timeit() {
    : > "$WORK/samples"
    TIMEIT_FAILED=no
    for _ in $(seq "$N"); do
        s=$(now_us)
        if ! "$@" >"$WORK/cmd.out" 2>&1; then
            TIMEIT_FAILED=yes
            echo "command failed, so its timing means nothing: $*" >&2
            sed 's/^/    /' "$WORK/cmd.out" | tail -3 >&2
            echo "0 0"
            return 1
        fi
        e=$(now_us)
        echo $(( (e - s) / 1000 )) >> "$WORK/samples"
    done
    sort -n "$WORK/samples" | awk '
        { a[NR] = $1 }
        END {
            if (NR == 0) { print "0 0"; exit }
            m = (NR % 2) ? a[(NR + 1) / 2] : int((a[NR / 2] + a[NR / 2 + 1]) / 2)
            i = int(NR * 0.9); if (i < 1) i = 1
            print m, a[i]
        }'
}

# "3.1x" - how many times the second number fits in the first. Printed as a
# ratio only when both are meaningful; a sub-millisecond denominator would
# make the number noise rather than information.
ratio() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        if (b <= 0 || a <= 0) { print "-" } else { printf "%.1fx", a / b }
    }'
}

# Sum of VmRSS, in MiB, over every process whose COMMAND LINE matches.
#
# The command line and not `comm`, because `comm` lies here. Our supervisor
# is started by executing /proc/self/exe - which is what makes the
# CVE-2019-5736 seal work - and the kernel names a process after the final
# component of the path it executed, so every supervisor was called `exe`
# and a comm-based match reported a confident zero. (The engine now also
# sets its own name, but the cmdline is the thing that cannot drift.)
rss_of() {
    total=0
    for p in /proc/[0-9]*; do
        [ -r "$p/cmdline" ] || continue
        c=$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null) || continue
        # shellcheck disable=SC2254  # the pattern is meant to glob
        case "$c" in
            $1)
                k=$(awk '/^VmRSS:/{print $2}' "$p/status" 2>/dev/null)
                total=$((total + ${k:-0}))
                ;;
        esac
    done
    echo $((total / 1024))
}

row() { printf '| %s | %s | %s | %s |\n' "$1" "$2" "$3" "$4"; }

# --- preparation --------------------------------------------------------

echo "host:       $(uname -r) $(uname -m), $(nproc) cpu"
echo "rustainer:  $B"
if [ "$HAVE_DOCKER" = yes ]; then
    echo "docker:     $(docker version --format '{{.Server.Version}}' 2>/dev/null)"
else
    echo "docker:     NOT PRESENT - only rustainer's own numbers are measured"
fi
echo "iterations: $N"
echo
echo "pulling $IMG into both stores..."
if ! timeout 600 "$B" pull "$IMG" >/dev/null 2>&1; then
    # Docker Hub's anonymous quota. The mirror serves identical content.
    IMG=mirror.gcr.io/library/alpine:3.21
    timeout 600 "$B" pull "$IMG" >/dev/null 2>&1 \
        || { echo "cannot pull an image" >&2; exit 2; }
fi
[ "$HAVE_DOCKER" = yes ] && timeout 600 docker pull -q "$IMG" >/dev/null 2>&1

echo
echo "## Results"
echo
echo "| measurement | rustainer | docker | docker / rustainer |"
echo "|---|---:|---:|---:|"

# --- 1. run a container to exit -----------------------------------------
# The number most people mean by "how fast is it": you type a command, the
# container runs it, you get your shell back.
set -- $(timeit "$B" run --network none "$IMG" /bin/true)
R_RUN=$1; R_RUN90=$2
if [ "$HAVE_DOCKER" = yes ]; then
    set -- $(timeit docker run --rm --network none "$IMG" /bin/true)
    D_RUN=$1; D_RUN90=$2
    row "run a container to exit (median)" "${R_RUN} ms" "${D_RUN} ms" "$(ratio "$D_RUN" "$R_RUN")"
    row "run a container to exit (p90)" "${R_RUN90} ms" "${D_RUN90} ms" "$(ratio "$D_RUN90" "$R_RUN90")"
else
    row "run a container to exit (median)" "${R_RUN} ms" "n/a" "-"
fi

# --- 2. start detached, then stop ---------------------------------------
# The lifecycle an orchestrator drives, rather than the one a human types.
r_cycle() {
    id=$("$B" run -d --network none "$IMG" /bin/sleep 30 2>/dev/null)
    [ -n "$id" ] && "$B" stop -t 0 "$id" >/dev/null 2>&1 && "$B" rm "$id" >/dev/null 2>&1
}
d_cycle() {
    id=$(docker run -d --network none "$IMG" /bin/sleep 30 2>/dev/null)
    [ -n "$id" ] && docker stop -t 0 "$id" >/dev/null 2>&1 && docker rm "$id" >/dev/null 2>&1
}
set -- $(timeit r_cycle)
R_CYCLE=$1
if [ "$HAVE_DOCKER" = yes ]; then
    set -- $(timeit d_cycle)
    D_CYCLE=$1
    row "start detached, stop, remove" "${R_CYCLE} ms" "${D_CYCLE} ms" "$(ratio "$D_CYCLE" "$R_CYCLE")"
else
    row "start detached, stop, remove" "${R_CYCLE} ms" "n/a" "-"
fi

# --- 3. exec into a running container -----------------------------------
RID=$("$B" run -d --network none "$IMG" /bin/sleep 600 2>/dev/null)
DID=
[ "$HAVE_DOCKER" = yes ] && DID=$(docker run -d --network none "$IMG" /bin/sleep 600 2>/dev/null)
if [ -n "$RID" ]; then
    set -- $(timeit "$B" exec "$RID" /bin/true)
    R_EXEC=$1
    if [ -n "$DID" ]; then
        set -- $(timeit docker exec "$DID" /bin/true)
        D_EXEC=$1
        row "exec a command in a live container" "${R_EXEC} ms" "${D_EXEC} ms" "$(ratio "$D_EXEC" "$R_EXEC")"
    else
        row "exec a command in a live container" "${R_EXEC} ms" "n/a" "-"
    fi
fi

# --- 4. memory held per running container -------------------------------
# What each engine keeps resident to supervise ONE container: our
# supervisor, versus containerd's shim.
R_SUP=$(rss_of '*supervise*')
if [ "$HAVE_DOCKER" = yes ]; then
    D_SHIM=$(rss_of '*containerd-shim*')
    row "resident per running container" "${R_SUP} MiB" "${D_SHIM} MiB" "$(ratio "$D_SHIM" "$R_SUP")"
else
    row "resident per running container" "${R_SUP} MiB" "n/a" "-"
fi

[ -n "$RID" ] && "$B" stop -t 0 "$RID" >/dev/null 2>&1 && "$B" rm "$RID" >/dev/null 2>&1
[ -n "$DID" ] && docker rm -f "$DID" >/dev/null 2>&1

# --- 5. memory held with nothing running --------------------------------
# The structural difference. There is no rustainer process when no
# container is running: the CLI exits and the supervisors are per
# container. Docker keeps dockerd and containerd resident whether or not
# anything is running, because the API has to be there to answer.
R_IDLE=$(rss_of '*rustainer*')
if [ "$HAVE_DOCKER" = yes ]; then
    D_IDLE=$(( $(rss_of 'dockerd*') + $(rss_of 'containerd *') + $(rss_of 'containerd') ))
    row "resident with no containers running" "${R_IDLE} MiB" "${D_IDLE} MiB" "$(ratio "$D_IDLE" "${R_IDLE:-0}")"
else
    row "resident with no containers running" "${R_IDLE} MiB" "n/a" "-"
fi

# --- 6. what you install ------------------------------------------------
R_BIN=$(( $(stat -c %s "$B") / 1024 ))
if [ "$HAVE_DOCKER" = yes ]; then
    D_BIN=0
    for f in dockerd containerd containerd-shim-runc-v2 runc docker docker-proxy ctr; do
        p=$(command -v "$f" 2>/dev/null) || continue
        D_BIN=$(( D_BIN + $(stat -c %s "$p") / 1024 ))
    done
    row "binaries installed" "${R_BIN} KiB" "${D_BIN} KiB" "$(ratio "$D_BIN" "$R_BIN")"
else
    row "binaries installed" "${R_BIN} KiB" "n/a" "-"
fi

# --- 7. pull, cold ------------------------------------------------------
# The one row that measures the network, so it is a single sample and the
# slowest thing here. It is included because it would be dishonest to show
# only the rows we win.
"$B" rmi "$IMG" >/dev/null 2>&1
s=$(now_us); timeout 600 "$B" pull "$IMG" >/dev/null 2>&1; e=$(now_us)
R_PULL=$(( (e - s) / 1000 ))
if [ "$HAVE_DOCKER" = yes ]; then
    docker rmi -f "$IMG" >/dev/null 2>&1
    s=$(now_us); timeout 600 docker pull -q "$IMG" >/dev/null 2>&1; e=$(now_us)
    D_PULL=$(( (e - s) / 1000 ))
    row "pull a cold image (1 sample, network)" "${R_PULL} ms" "${D_PULL} ms" "$(ratio "$D_PULL" "$R_PULL")"
else
    row "pull a cold image (1 sample, network)" "${R_PULL} ms" "n/a" "-"
fi

echo
echo "### What Docker does here that rustainer does not"
echo
echo "Not a footnote - it is the other half of the comparison:"
echo "image build, volumes, user-defined networks, an HTTP API and SDKs,"
echo "log drivers, healthchecks, restart policies, plugins, swarm,"
echo "checkpoint/restore, Windows containers, and cgroup v1 hosts."
echo
echo "The timings above are a fair measure of starting and supervising a"
echo "container, and not a claim that one tool replaces the other."
