#!/bin/bash
# Stage 14 success test - the local store is authoritative, and the engine
# can be found in `ps`.
#
#   sudo bash scripts/stage14-success-test.sh
#
# This stage exists because measuring the engine against Docker found a
# defect that no correctness test would ever have caught: `run <image>`
# went through `pull`, and `pull` begins by fetching the manifest. Starting
# a container from an image already on disk therefore cost four TLS
# round-trips to a registry before anything was cloned - 870 of the 890
# milliseconds a container start took.
#
# Slow was the smaller half. The real defect is that a fully cached image
# was **unrunnable with no network**: a machine on a plane, behind a
# proxy, or on a site with egress rules could not start a container whose
# every byte was already local. Docker has run cached images offline since
# the beginning, and an engine that cannot is not a Docker-compatible
# engine however many images it can pull.
#
# The test for "it does not use the network" is not a stopwatch. It is a
# network namespace with nothing in it: `unshare --net` gives this process
# a stack with only loopback, so DNS and TCP both genuinely fail, and
# nothing on the host is touched to arrange it.

set -u
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rustainer-target}"

PASS=0
FAIL=0
SKIP=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
skip() { SKIP=$((SKIP + 1)); echo "  SKIP  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage14.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=4g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED_TMPFS" = yes ] && umount "$SCRATCH" 2>/dev/null
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"

try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address; nothing here can be tested" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac
echo "image: $IMG"

# A network namespace with nothing in it. Not a firewall rule, not a
# resolv.conf edit: nothing on this host is changed to arrange it, and
# there is nothing to undo if the test dies half way.
OFFLINE="unshare --net"
if ! $OFFLINE true 2>/dev/null; then
    OFFLINE=""
fi

echo
echo "1. A cached image runs with no network at all"
if [ -z "$OFFLINE" ]; then
    skip "cannot make a network namespace here, so offline cannot be tested"
else
    # Proof the namespace is really empty, so a pass below means something.
    $OFFLINE sh -c 'exec 3<>/dev/tcp/1.1.1.1/443' 2>/dev/null \
        && bad "the 'offline' namespace still has a route out" \
        || ok "the test's network namespace really has no way out"
    OUT=$($OFFLINE "$B" run --network none "$IMG" /bin/echo offline-ok 2>&1 | tail -1)
    check "a cached image starts with no registry reachable" "$OUT" "offline-ok"
fi

echo
echo "2. --pull still means 'go and check', and fails honestly when it cannot"
if [ -z "$OFFLINE" ]; then
    skip "no network namespace"
else
    OUT=$($OFFLINE "$B" run --pull --network none "$IMG" /bin/true 2>&1 | tail -1)
    case "$OUT" in
        *"pulling"*) ok "--pull with no network fails, and says it was pulling" ;;
        *) bad "--pull did not go to the registry: $OUT" ;;
    esac
fi

echo
echo "3. Half an image is a cache miss, never a cache hit"
# A layer directory removed behind the engine's back must send it back to
# the registry, not produce an overlay with a hole in it. An image that
# mounts and is missing a layer looks plausible and is wrong, and the
# wrongness surfaces as a missing file hours later.
LAYER=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | head -1)
if [ -z "$LAYER" ]; then
    skip "no extracted layer to remove"
else
    mv "$LAYER" "$LAYER.hidden"
    if [ -n "$OFFLINE" ]; then
        OUT=$($OFFLINE "$B" run --network none "$IMG" /bin/true 2>&1 | tail -1)
        case "$OUT" in
            *"pulling"*) ok "a missing layer sends it back to the registry" ;;
            *) bad "a missing layer did not force a pull: $OUT" ;;
        esac
    else
        skip "no network namespace to prove it tried to pull"
    fi
    mv "$LAYER.hidden" "$LAYER"
    OUT=$("$B" run --network none "$IMG" /bin/echo restored 2>&1 | tail -1)
    check "and it runs again once the layer is back" "$OUT" "restored"
fi

echo
echo "4. The supervisor can be found in ps"
# It is started by executing /proc/self/exe - which is what makes the
# CVE-2019-5736 seal work - and the kernel names a process after the final
# component of the path it executed. Every supervisor on the machine was
# therefore called `exe`, which tells an operator nothing and matches no
# monitoring rule. prctl(PR_SET_NAME) is the whole fix; the test is that
# somebody keeps calling it.
ID=$("$B" run -d --network none "$IMG" /bin/sleep 60 2>/dev/null)
sleep 0.5
if [ -z "$ID" ]; then
    bad "could not start a detached container"
else
    SPID=$(sed -n 's/.*"supervisor_pid":[ ]*\([0-9]*\).*/\1/p' \
        "$RUSTAINER_ROOT/containers/$ID/state.json" 2>/dev/null)
    if [ -n "$SPID" ] && [ -r "/proc/$SPID/comm" ]; then
        COMM=$(cat "/proc/$SPID/comm")
        check "the supervisor names itself" "$COMM" "rustainer-sup"
    else
        skip "no supervisor pid recorded"
    fi
    "$B" stop -t 0 "$ID" >/dev/null 2>&1
    "$B" rm "$ID" >/dev/null 2>&1
fi

echo
echo "5. Nothing of ours is resident once no container is running"
# The structural claim the comparison rests on: there is no daemon. If
# this ever fails, the engine has grown one.
LEFT=0
for p in /proc/[0-9]*; do
    [ -r "$p/cmdline" ] || continue
    case "$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null)" in
        *rustainer*supervise*) LEFT=$((LEFT + 1)) ;;
    esac
done
check "no supervisor outlives its container" "$LEFT" "0"

echo
echo "6. The benchmark script runs and produces a table"
# The comparison in research/COMPARISON.md is only worth anything if the
# thing that produced it still works.
BOUT=$(RUSTAINER_BIN="$B" RUSTAINER_BENCH_IMAGE="$IMG" \
    timeout 600 bash scripts/bench.sh 2 2>&1)
case "$BOUT" in
    *"run a container to exit"*) ok "bench.sh produces its table" ;;
    *) bad "bench.sh produced no results: $(echo "$BOUT" | tail -1)" ;;
esac
# And it refuses to report a timing for a command that failed, because a
# time-to-failure averaged with real samples is a number about nothing.
grep -q "timing means nothing" scripts/bench.sh \
    && ok "bench.sh refuses to time a failing command" \
    || bad "bench.sh would happily average a failure into its median"

echo
mirror_note
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
