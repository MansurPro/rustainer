#!/bin/bash
# Stage 3 success test - RND.md section 17, "Lifecycle, supervision and state".
#
#   sudo bash scripts/stage3-success-test.sh
#   unshare --user --map-root-user bash scripts/stage3-success-test.sh
#
# Two of these assertions are about the engine telling the truth rather than
# about it working, and they are the point of the stage: an exit nobody
# observed must be reported as unknown, not as 0; and a lock inherited by the
# supervisor must not wedge every later command on the container.

set -u
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rustainer-target}"

PASS=0
FAIL=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
have_cap_sys_admin() {
    local eff low
    eff=$(awk '/^CapEff:/{print $2}' /proc/self/status) || return 1
    low=${eff: -8}
    (( (0x$low >> 21) & 1 ))
}
have_cap_sys_admin || { echo "needs CAP_SYS_ADMIN (sudo, or unshare --user --map-root-user)" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

STATE=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage3.XXXXXX")
export RUSTAINER_ROOT="$STATE"
trap 'rm -rf "$STATE"' EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u), state $STATE"

R() { "$B" run --rootfs / --network host "$@"; }
field() { grep -o "\"$2\": *[0-9-]*" "$STATE/containers/$1/state.json" | grep -o '[0-9-]*$'; }
# Fail a command that blocks, rather than hanging the whole suite.
nohang() { timeout 10 "$@"; }

echo
echo "1. run -d returns, the CLI is gone, the container is not"
ID=$(R -d /bin/sleep 30)
check "an id was printed" "$(printf %s "$ID" | wc -c | tr -d ' ')" "12"
sleep 0.3
check "ps reports it running" "$("$B" ps | awk -v i="$ID" '$1==i{print $2}')" "running"
INIT=$("$B" ps | awk -v i="$ID" '$1==i{print $3}')
SUP=$(field "$ID" supervisor_pid)
[ -n "$INIT" ] && [ -n "$SUP" ] && ok "init pid $INIT and supervisor pid $SUP are recorded" \
    || bad "pids were not recorded"

echo
echo "2. The CLI is not in the container's ancestry"
# init's parent must be the supervisor, and the supervisor's parent must not
# be a live rustainer CLI - the CLI exited when `run -d` returned.
PPID_OF_INIT=$(sed 's/.*) //' "/proc/$INIT/stat" | cut -d' ' -f2)
check "init's parent is the supervisor" "$PPID_OF_INIT" "$SUP"
PPID_OF_SUP=$(sed 's/.*) //' "/proc/$SUP/stat" | cut -d' ' -f2)
if [ ! -r "/proc/$PPID_OF_SUP/cmdline" ] || ! tr '\0' ' ' < "/proc/$PPID_OF_SUP/cmdline" | grep -q rustainer; then
    ok "the supervisor was reparented; no CLI remains above it"
else
    bad "a rustainer CLI is still the supervisor's parent"
fi

echo
echo "3. The inherited lock does not wedge later commands"
# The O_CLOEXEC test. If the CLI's flock leaked into the supervisor through
# fork, the supervisor would hold it for the container's whole life and this
# would block forever. Invisible to any test that runs one command.
nohang "$B" logs "$ID" >/dev/null 2>&1 && ok "logs does not block after run -d" || bad "logs blocked or failed"
nohang "$B" ps >/dev/null 2>&1 && ok "ps does not block after run -d" || bad "ps blocked"

echo
echo "4. stop: SIGTERM, a grace period, then SIGKILL"
START=$(date +%s)
nohang "$B" stop -t 2 "$ID" >/dev/null 2>&1 || bad "stop blocked or failed"
ELAPSED=$(( $(date +%s) - START ))
# sleep(1) installs no SIGTERM handler, and PID 1 in a new namespace has no
# default disposition - so it ignores SIGTERM and the full grace period must
# elapse before SIGKILL. A `stop` that returned instantly would mean we had
# signalled something other than the container's init.
if [ "$ELAPSED" -ge 2 ]; then
    ok "the grace period was honoured ($ELAPSED s); PID 1 ignored SIGTERM"
else
    bad "stop returned in ${ELAPSED}s; the grace period was skipped"
fi
check "the container leaves the default ps listing" "$("$B" ps | awk -v i="$ID" '$1==i{print $1}')" ""
check "ps -a records the signal, not a fake exit code" \
    "$("$B" ps -a | awk -v i="$ID" '$1==i{print $4}')" "sig9"

echo
echo "5. A container that handles SIGTERM exits on its own terms"
ID2=$(R -d /bin/sh -c 'trap "exit 143" TERM; while :; do sleep 0.2; done')
sleep 0.5
nohang "$B" stop -t 5 "$ID2" >/dev/null 2>&1
check "its own exit code is recorded, not the signal" \
    "$("$B" ps -a | awk -v i="$ID2" '$1==i{print $4}')" "143"

echo
echo "6. The honesty test: an unobserved exit is unknown, never 0"
ID3=$(R -d /bin/sleep 30)
sleep 0.5
S3=$(field "$ID3" supervisor_pid)
I3=$("$B" ps | awk -v i="$ID3" '$1==i{print $3}')
# Kill the supervisor first, so nobody is left to record the exit, then the
# container. The state file still says "running" and that is now a lie.
kill -KILL "$S3" 2>/dev/null
sleep 0.2
kill -KILL "$I3" 2>/dev/null
sleep 0.5
check "status is reported as unknown" "$("$B" ps -a | awk -v i="$ID3" '$1==i{print $2}')" "unknown"
check "the exit code is '?', not 0" "$("$B" ps -a | awk -v i="$ID3" '$1==i{print $4}')" "?"
check "no exit code was invented in the state file" "$(field "$ID3" exit_code)" ""

echo
echo "7. A normal exit is recorded exactly"
ID4=$(R -d /bin/sh -c 'exit 42')
sleep 0.8
check "exit 42 is recorded" "$("$B" ps -a | awk -v i="$ID4" '$1==i{print $4}')" "42"
check "its status is exited" "$("$B" ps -a | awk -v i="$ID4" '$1==i{print $2}')" "exited"

echo
echo "8. logs capture output after the container is gone"
ID5=$(R -d /bin/sh -c 'echo to-stdout; echo to-stderr >&2')
sleep 0.8
OUT=$(nohang "$B" logs "$ID5" 2>/dev/null)
ERR=$(nohang "$B" logs "$ID5" 2>&1 >/dev/null)
check "stdout was captured" "$OUT" "to-stdout"
check "stderr was captured separately" "$ERR" "to-stderr"

echo
echo "9. Foreground run streams output and returns the exit code"
OUT=$(R /bin/sh -c 'echo hello-foreground; exit 7'; echo "rc=$?")
echo "$OUT" | grep -q 'hello-foreground' && ok "output reached our stdout" || bad "output was lost"
echo "$OUT" | grep -qx 'rc=7' && ok "the exit code propagated" || bad "the exit code did not propagate"

echo
echo "10. Concurrent runs do not corrupt the state directory"
for _ in 1 2 3 4 5; do R -d /bin/sleep 5 >/dev/null 2>&1 & done
wait
sleep 0.5
N=$("$B" ps | tail -n +2 | wc -l | tr -d ' ')
check "all five containers are listed" "$N" "5"
BROKEN=0
for d in "$STATE"/containers/*/; do
    [ -f "$d/state.json" ] || BROKEN=$((BROKEN+1))
done
check "every container has a readable state file" "$BROKEN" "0"

echo
echo "11. rm refuses a running container, and removes a stopped one"
ID6=$(R -d /bin/sleep 30)
sleep 0.4
nohang "$B" rm "$ID6" >/dev/null 2>&1 && bad "rm removed a running container" || ok "rm refuses a running container"
nohang "$B" rm --force "$ID6" >/dev/null 2>&1 && ok "rm --force stops and removes it" || bad "rm --force failed"
check "its directory is gone" "$([ -e "$STATE/containers/$ID6" ] && echo present || echo gone)" "gone"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
