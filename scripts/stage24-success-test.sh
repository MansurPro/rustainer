#!/bin/bash
# Stage 24 success test - the page grows a terminal, and the rest.
#
#   sudo bash scripts/stage24-success-test.sh
#
# The terminal is the part worth testing properly, because it is the one
# with state. Everything else the page does is a request and a table.
#
# The shape, which is the same rule as every other mutation here: the
# server allocates the pty and spawns the CLI with the slave as its
# stdio. It does NOT create the container process itself, because it is
# multi-threaded and `clone3` refuses a process that has ever had a
# second thread. The CLI it spawns is single-threaded and does the
# cloning. A terminal in a browser, sitting on top of the oldest
# invariant in the engine.
#
# Output is polled by OFFSET rather than streamed, and the test checks
# the thing that makes that safe: two polls from the same offset must
# return the same bytes, not two copies of them. The page had exactly
# that bug - every keystroke's echo was drawn twice and the line turned
# to mush - and it was fixed by serialising the polls rather than by
# guessing at the rendering.

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

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
command -v curl >/dev/null 2>&1 || { echo "needs curl" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "needs python3" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage24.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=3g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
SERVE_PID=
cleanup() {
    [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    if [ "$MOUNTED_TMPFS" = yes ]; then
        for _ in 1 2 3 4 5 6; do umount "$SCRATCH" 2>/dev/null && break; sleep 0.5; done
    fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

PORT=$((26000 + ($$ % 9000)))
URL="http://127.0.0.1:$PORT"
"$B" serve --addr "127.0.0.1:$PORT" > "$SCRATCH/serve.log" 2>&1 &
SERVE_PID=$!
for _ in $(seq 60); do curl -fsS "$URL/api/system" >/dev/null 2>&1 && break; sleep 0.2; done
curl -fsS "$URL/api/system" >/dev/null 2>&1 || {
    echo "the server did not come up:"; cat "$SCRATCH/serve.log"; exit 2;
}

ID=$(timeout 120 "$B" run -d --network none "$IMG" /bin/sleep 600 2>/dev/null)
[ -n "$ID" ] || { echo "could not start a container" >&2; exit 2; }

jget() { python3 -c "
import json,sys
d=json.load(sys.stdin)
for k in sys.argv[1].split('.'):
    d = d.get(k) if isinstance(d, dict) else None
print('' if d is None else d)" "$1"; }

echo
echo "1. The page carries the new views, and still loads nothing"
PAGE=$(curl -fsS "$URL/")
# The three Docker Desktop has and this did not, found by putting the
# two Containers screens side by side rather than by guessing.
for marker in "only-running" "column-picker" "spark"; do
    contains "the page has $marker" "$PAGE" "$marker"
done
for marker in "view-networks" "view-events" "terminal" "palette"; do
    contains "the page has $marker" "$PAGE" "$marker"
done
EXTERNAL=$(printf '%s' "$PAGE" | grep -coE '(src|href)="https?://|@import|fonts\.googleapis' || true)
check "still nothing from the network" "$EXTERNAL" "0"
# The terminal declares itself a key sink, and the page's shortcuts ask
# about that rather than about tag names. The bug: "/" focuses the
# filter box, and its guard excluded only INPUT and TEXTAREA - so a
# terminal, which is a focusable div, lost focus the moment you typed an
# absolute path. `cat /etc/hosts` put `cat ` in the terminal and
# `etc/hosts` in the filter, which is every second command a person
# types into a shell.
contains "the terminal declares itself a key sink" "$PAGE" "keysink"
# Docker Desktop reports container memory on a host where nothing asked
# for a limit; this reported "-", because `memory.current` only exists
# where the memory controller is enabled and the engine enables no
# controller it was not asked for. Summed RSS needs neither.
contains "memory falls back to summed rss" "$PAGE" "memory_rss"
check "and no shortcut guards on tag names alone" \
    "$(printf '%s' "$PAGE" | grep -c 'tagName !== "INPUT"' || true)" "0"

echo
echo "1b. A running container reports memory even with no controller"
# The number is not memory.current and is not claimed to be: it
# double-counts shared pages and omits page cache. It is a number
# instead of a dash, which is the part that was missing.
MSTATS=$(curl -fsS "$URL/api/containers/$ID/stats")
MRSS=$(echo "$MSTATS" | jget memory_rss)
case "$MRSS" in
    ""|0) bad "no memory figure for a running container (got '$MRSS')" ;;
    *) [ "$MRSS" -gt 0 ] 2>/dev/null && ok "summed rss is reported ($MRSS bytes)" \
         || bad "memory_rss is not a number: '$MRSS'" ;;
esac

echo
echo "2. Networks and events are served"
contains "networks include the built-in bridge" "$(curl -fsS "$URL/api/networks")" "\"name\":\"bridge\""
contains "events are a list" "$(curl -fsS "$URL/api/events?limit=5")" "["

echo
echo "3. A terminal session is a real pty"
SESSION=$(curl -fsS -X POST -H 'Content-Type: application/json' \
    -d '{"command":"/bin/sh","rows":30,"cols":100}' \
    "$URL/api/containers/$ID/term" | jget session)
[ -n "$SESSION" ] && ok "a session opens" || bad "no session id came back"
[ -n "$SESSION" ] || { echo; echo "$PASS passed, $FAIL failed, $SKIP skipped"; exit 1; }

# The shell needs a moment to print its prompt.
sleep 1
OUT1=$(curl -fsS "$URL/api/term/$SESSION/out?from=0")
contains "the shell printed a prompt, which means it has a terminal" "$(echo "$OUT1" | jget data)" "#"

echo
echo "4. Typing at it runs a command in the container"
curl -fsS -X POST -H 'Content-Type: application/json' \
    -d '{"data":"echo terminal-works; stty size\r"}' "$URL/api/term/$SESSION/in" >/dev/null
sleep 1.2
ALL=$(curl -fsS "$URL/api/term/$SESSION/out?from=0" | jget data)
contains "the command ran" "$ALL" "terminal-works"
# The size the session was opened with, through two pty layers.
contains "and the window size reached the container" "$ALL" "30 100"

echo
echo "5. Polling twice from one offset returns the same bytes, not two copies"
# The bug the page had: two polls in flight both asked from the same
# offset, both got the same bytes, and every keystroke's echo was drawn
# twice. The server must be idempotent here; the page must not overlap.
A=$(curl -fsS "$URL/api/term/$SESSION/out?from=0" | jget data)
Bq=$(curl -fsS "$URL/api/term/$SESSION/out?from=0" | jget data)
check "the same read twice is the same answer" "$A" "$Bq"
NEXT=$(curl -fsS "$URL/api/term/$SESSION/out?from=0" | jget next)
TAIL=$(curl -fsS "$URL/api/term/$SESSION/out?from=$NEXT" | jget data)
check "and reading from the end returns nothing" "$TAIL" ""

echo
echo "6. The session ends when the shell does"
curl -fsS -X POST -H 'Content-Type: application/json' \
    -d '{"data":"exit\r"}' "$URL/api/term/$SESSION/in" >/dev/null
sleep 1.2
check "the session reports it is done" \
    "$(curl -fsS "$URL/api/term/$SESSION/out?from=0" | jget done)" "True"
curl -fsS -X DELETE "$URL/api/term/$SESSION" >/dev/null
contains "and it is gone afterwards" \
    "$(curl -sS "$URL/api/term/$SESSION/out?from=0")" "no such session"

echo
echo "7. A terminal leaves no process behind"
# A pty session holds a child process and a master descriptor; closing
# it must take both. A page left open all day would otherwise accumulate
# one shell per visit.
sleep 0.5
LEFT=0
for p in /proc/[0-9]*; do
    [ -r "$p/cmdline" ] || continue
    case "$(tr '\0' ' ' < "$p/cmdline" 2>/dev/null)" in
        *"exec -t $ID"*) LEFT=$((LEFT + 1)) ;;
    esac
done
check "no exec process survived the session" "$LEFT" "0"

echo
echo "8. Bad session ids are refused, not resolved"
contains "a traversing session id is refused" \
    "$(curl -sS "$URL/api/term/..%2F..%2Fetc/out")" "not a session id"
check "an unknown session is a 404" \
    "$(curl -sS -o /dev/null -w '%{http_code}' "$URL/api/term/deadbeef/out")" "404"

echo
echo "9. Building through the API"
CTX="$SCRATCH/ctx"
mkdir -p "$CTX"
echo "built-through-the-api" > "$CTX/f.txt"
printf 'FROM %s\nCOPY f.txt /f.txt\nCMD ["/bin/cat", "/f.txt"]\n' "$IMG" > "$CTX/Dockerfile"
RES=$(curl -fsS -X POST -H 'Content-Type: application/json' \
    -d "{\"tag\":\"api-built:1\",\"context\":\"$CTX\"}" "$URL/api/build")
check "the build reported success" "$(echo "$RES" | jget ok)" "True"
check "and the image runs" \
    "$(timeout 120 "$B" run --network none api-built:1 2>&1 | tail -1)" "built-through-the-api"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
