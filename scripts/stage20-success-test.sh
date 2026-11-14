#!/bin/bash
# Stage 20 success test - restart policies and the event log.
#
#   sudo bash scripts/stage20-success-test.sh
#
# Both are supervisor features, which is why they arrive together: the
# supervisor already outlives the CLI and already owns the container's
# exit, so "start it again" and "write down what happened" are additions
# to a process that exists rather than a new one.
#
# The rule that surprises people, and which this asserts:
# `--restart always` does NOT restart a container the operator stopped.
# A policy that undid `stop` would make `stop` meaningless. The flag is
# written by `stop` BEFORE it signals, because the supervisor reads it the
# moment its child dies and there is otherwise a window in which it sees
# nothing and restarts what was just stopped.
#
# The events file needs no lock: every writer opens O_APPEND and writes
# one line in one `write`, which Linux does not interleave below
# PIPE_BUF. That is why the line length is capped rather than the file
# being locked - and why two supervisors restarting at once cannot
# produce a row that will not parse.

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
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (got '$(echo "$2" | head -c 130)')" ;; esac; }
refuses() {
    local label=$1 want=$2; shift 2
    local out rc
    out=$("$@" 2>&1); rc=$?
    if [ "$rc" -ne 0 ] && echo "$out" | grep -q "$want"; then ok "$label"
    else bad "$label (rc=$rc, said: $(echo "$out" | tail -1 | cut -c1-110))"; fi
}

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "needs python3" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage20.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_COMPAT_STORE:-$SCRATCH/images}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    # A supervisor in restart backoff takes a moment to notice its
    # container is gone, and until it does the tmpfs is busy. Retry
    # rather than leaving a mount behind.
    if [ "$MOUNTED_TMPFS" = yes ]; then
        for _ in 1 2 3 4 5 6 7 8 9 10; do
            umount "$SCRATCH" 2>/dev/null && break
            sleep 0.5
        done
    fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

STATEF() { echo "$RUSTAINER_ROOT/containers/$1/state.json"; }
field() { python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2]))" "$(STATEF "$1")" "$2" 2>/dev/null; }

echo
echo "1. The policy is parsed, and nonsense is refused by name"
refuses "an unknown policy names the ones that exist" "is not a restart policy" \
    timeout 60 "$B" run --restart sometimes --network none --rootfs / /bin/true
refuses "a maximum on a policy that has none is refused" "only applies to" \
    timeout 60 "$B" run --restart always:3 --network none --rootfs / /bin/true
refuses "a non-numeric maximum is refused" "number of attempts" \
    timeout 60 "$B" run --restart on-failure:lots --network none --rootfs / /bin/true

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

echo
echo "2. on-failure restarts a failing container, exactly as many times as asked"
ID=$(timeout 120 "$B" run -d --restart on-failure:3 --network none "$IMG" \
    /bin/sh -c 'exit 1' 2>/dev/null)
[ -n "$ID" ] || { echo "could not start a container" >&2; exit 2; }
# 200 + 400 + 800 ms of backoff, plus three container lifetimes.
sleep 6
check "it restarted exactly three times" "$(field "$ID" restarts)" "3"
check "and then stayed exited" "$(field "$ID" status)" "exited"
"$B" rm --force "$ID" >/dev/null 2>&1

echo
echo "3. on-failure does NOT restart a container that succeeded"
ID=$(timeout 120 "$B" run -d --restart on-failure:3 --network none "$IMG" /bin/true 2>/dev/null)
sleep 2
check "a clean exit is not a failure" "$(field "$ID" restarts)" "0"
check "and it is simply exited" "$(field "$ID" status)" "exited"
"$B" rm --force "$ID" >/dev/null 2>&1

echo
echo "4. always restarts a container that exits cleanly"
ID=$(timeout 120 "$B" run -d --restart always --network none "$IMG" \
    /bin/sh -c 'sleep 1' 2>/dev/null)
sleep 4
R=$(field "$ID" restarts)
[ -n "$R" ] && [ "$R" -ge 1 ] 2>/dev/null \
    && ok "it came back after a clean exit ($R restarts)" \
    || bad "always did not restart a clean exit (restarts=$R)"

echo
echo "5. ...but a manual stop is not undone"
# The rule that makes `stop` mean something. The flag is written before
# the signal, because the supervisor reads it the moment its child dies.
timeout 60 "$B" stop -t 2 "$ID" >/dev/null 2>&1
BEFORE=$(field "$ID" restarts)
sleep 3
check "it stayed stopped" "$(field "$ID" status)" "exited"
check "and did not restart again after the stop" "$(field "$ID" restarts)" "$BEFORE"
check "the stop was recorded" "$(field "$ID" stop_requested)" "True"
"$B" rm --force "$ID" >/dev/null 2>&1

echo
echo "6. The backoff grows, so a broken image cannot be a fork bomb"
EV="$RUSTAINER_ROOT/events.jsonl"
DELAYS=$(python3 - "$EV" <<'PY'
import json, sys
out = []
for line in open(sys.argv[1]):
    try:
        d = json.loads(line)
    except Exception:
        continue
    if d.get("kind") == "restart":
        out.append(str(d["detail"].get("after_ms")))
print(" ".join(out[:3]))
PY
)
check "the first three delays double" "$DELAYS" "200 400 800"

echo
echo "7. The event log records the lifecycle"
ALL=$(timeout 60 "$B" events --since 600 --limit 500)
contains "a container being created" "$ALL" "create"
contains "it starting" "$ALL" "start"
contains "it dying" "$ALL" "die"
contains "it restarting" "$ALL" "restart"
contains "it being stopped" "$ALL" "stop"
contains "it being removed" "$ALL" "remove"

echo
echo "8. Every line parses, even written by several supervisors at once"
# The atomicity claim: O_APPEND plus one write per line, under PIPE_BUF.
# Four crash-looping containers write interleaved events as fast as the
# backoff allows; not one row may be spliced.
for i in 1 2 3 4; do
    timeout 120 "$B" run -d --name burst$i --restart on-failure:2 --network none "$IMG" \
        /bin/sh -c 'exit 3' >/dev/null 2>&1 &
done
wait
sleep 5
BAD=$(python3 - "$EV" <<'PY'
import json, sys
bad = 0
for line in open(sys.argv[1]):
    if not line.strip():
        continue
    try:
        json.loads(line)
    except Exception:
        bad += 1
print(bad)
PY
)
check "no row was spliced" "$BAD" "0"
COUNT=$(timeout 60 "$B" events --since 600 --limit 2000 | grep -c 'burst')
[ "${COUNT:-0}" -ge 8 ] \
    && ok "all four containers' events are there ($COUNT rows)" \
    || bad "events are missing (only $COUNT rows mentioning burst)"
for i in 1 2 3 4; do "$B" rm --force burst$i >/dev/null 2>&1; done

echo
echo "9. A restarted container keeps its identity"
# Same id, same cgroup, same address, same published ports: a restart is
# the same container running again, not a new one. Tearing all of that
# down and back up would flap the host's firewall on every crash loop.
ID=$(timeout 120 "$B" run -d --restart always --network none "$IMG" \
    /bin/sh -c 'echo alive; sleep 1' 2>/dev/null)
sleep 4
check "the id is unchanged" "$(field "$ID" id)" "$ID"
LOG=$("$B" logs "$ID" | grep -c alive)
[ "${LOG:-0}" -ge 2 ] \
    && ok "its log accumulates across restarts ($LOG runs)" \
    || bad "the log did not accumulate ($LOG)"
timeout 60 "$B" stop -t 1 "$ID" >/dev/null 2>&1
"$B" rm --force "$ID" >/dev/null 2>&1

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
