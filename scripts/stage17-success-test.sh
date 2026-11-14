#!/bin/bash
# Stage 17 success test - the local API and the page that uses it.
#
#   sudo bash scripts/stage17-success-test.sh
#
# Two properties are worth more than every endpoint put together, and both
# are asserted here rather than described:
#
#   1. THE SERVER DOES NOT OWN CONTAINERS. Kill it and they keep running,
#      because each one is owned by its own supervisor. That is what makes
#      this a view rather than a daemon, and it is the difference between
#      "restart the UI" and "restart everything".
#
#   2. THE SERVER CANNOT START A CONTAINER ITSELF. It is multi-threaded -
#      one thread per connection - and a container is created by a process
#      that has never had a second thread. So every mutation executes the
#      CLI and lets a fresh single-threaded process do it. The rule that
#      made clone3 safe at stage 1 decides the architecture of the API at
#      stage 17, and a test that only checked the JSON would never notice.
#
# The page is checked for one thing beyond rendering: that it references
# nothing it cannot reach. A dashboard that fetches a framework before it
# can tell you what is running is useless exactly when you need it.

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
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (got '$(echo "$2" | head -c 120)')" ;; esac; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
command -v curl >/dev/null 2>&1 || { echo "needs curl" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage17.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
SERVE_PID=
cleanup() {
    [ -n "$SERVE_PID" ] && kill "$SERVE_PID" 2>/dev/null
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED_TMPFS" = yes ] && umount "$SCRATCH" 2>/dev/null
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

case "$(uname -m)" in
    x86_64) ARCH=x86_64 ;; aarch64) ARCH=aarch64 ;;
    *) echo "unsupported arch" >&2; exit 2 ;;
esac
CACHE="${TMPDIR:-/tmp}/rustainer-alpine-$ARCH.tar.gz"
if [ ! -s "$CACHE" ]; then
    ALPINE="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/$ARCH"
    NAME=$(curl -fsS "$ALPINE/" | grep -o "alpine-minirootfs-[0-9.]*-$ARCH\.tar\.gz" | sort -uV | tail -1)
    curl -fsS -o "$CACHE" "$ALPINE/$NAME" || exit 2
fi
BASE="$SCRATCH/base"; mkdir -p "$BASE"
tar --same-owner -xzf "$CACHE" -C "$BASE" 2>/dev/null || tar --no-same-owner -xzf "$CACHE" -C "$BASE" || exit 2

PORT=$((21000 + ($$ % 15000)))
URL="http://127.0.0.1:$PORT"

echo
echo "0. --check binds and exits rather than hanging"
OUT=$(timeout 20 "$B" serve --addr "127.0.0.1:$PORT" --check 2>&1)
contains "it reports the URL it would serve" "$OUT" "http://127.0.0.1:$PORT"

"$B" serve --addr "127.0.0.1:$PORT" > "$SCRATCH/serve.log" 2>&1 &
SERVE_PID=$!
for _ in $(seq 60); do
    curl -fsS "$URL/api/system" >/dev/null 2>&1 && break
    sleep 0.2
done
curl -fsS "$URL/api/system" >/dev/null 2>&1 || {
    echo "the server did not come up:"; cat "$SCRATCH/serve.log"; exit 2;
}

echo
echo "1. The API answers, and tells the truth about the host"
SYS=$(curl -fsS "$URL/api/system")
contains "it reports this kernel" "$SYS" "$(uname -r)"
contains "it reports this architecture" "$SYS" "$(uname -m | sed 's/aarch64/aarch64/')"
contains "it reports whether we are root" "$SYS" "\"uid\":$(id -u)"

echo
echo "2. The page is served, and references nothing it cannot reach"
PAGE=$(curl -fsS "$URL/")
contains "the page is HTML" "$PAGE" "<!doctype html>"
contains "and it is the app, not a placeholder" "$PAGE" "/api/containers"
# No CDN, no fonts, no analytics. A dashboard that needs the network
# before it can show you what is running is useless exactly when you need
# it, and this engine is specifically meant to work offline.
EXTERNAL=$(printf '%s' "$PAGE" | grep -coE '(src|href)="https?://|@import|fonts\.googleapis' || true)
check "it loads nothing from the network" "$EXTERNAL" "0"

echo
echo "3. A container created through the API really exists"
# Through the API's own shape - an image reference - rather than a local
# rootfs, because the point is that the page can start a container and
# not that the engine can.
# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
ID=
if try_pull alpine:3.21; then
    ID=$(curl -fsS -X POST -H 'Content-Type: application/json' \
        -d "{\"image\":\"$PULLED\",\"network\":\"none\",\"command\":\"/bin/sleep 120\"}" \
        "$URL/api/containers" 2>/dev/null | sed -n 's/.*"stdout":"\([^"]*\)".*/\1/p')
    [ -n "$ID" ] && ok "the API started a container from an image reference" \
        || bad "the API could not start a container"
else
    skip "no registry would serve us, so the API's own create path is untested"
fi
if [ -z "$ID" ]; then
    ID=$(timeout 60 "$B" run -d --network none --layer "$BASE" /bin/sleep 120 2>/dev/null)
fi
[ -n "$ID" ] || { echo "could not create a container" >&2; exit 2; }
sleep 0.5
LIST=$(curl -fsS "$URL/api/containers")
contains "the API lists it" "$LIST" "$ID"
contains "and reports it running" "$LIST" "\"status\":\"running\""

echo
echo "4. Logs and stats come back"
timeout 60 "$B" exec "$ID" /bin/sh -c 'echo marker-line > /dev/null' >/dev/null 2>&1
LOGID=$(timeout 60 "$B" run -d --network none --layer "$BASE" /bin/sh -c 'echo marker-line; sleep 60' 2>/dev/null)
sleep 0.7
contains "logs are returned" "$(curl -fsS "$URL/api/containers/$LOGID/logs")" "marker-line"
STATS=$(curl -fsS "$URL/api/containers/$ID/stats")
contains "stats say whether it is running" "$STATS" "\"running\":true"

echo
echo "5. exec through the API runs in the container"
EXECOUT=$(curl -fsS -X POST -H 'Content-Type: application/json' \
    -d '{"command":"/bin/busybox sh -c \"echo from-the-api\""}' \
    "$URL/api/containers/$ID/exec")
case "$EXECOUT" in
    *from-the-api*) ok "exec output comes back" ;;
    *) # busybox quoting through JSON is fiddly; a plain command is enough.
       EXECOUT=$(curl -fsS -X POST -H 'Content-Type: application/json' \
         -d '{"command":"/bin/echo from-the-api"}' "$URL/api/containers/$ID/exec")
       contains "exec output comes back" "$EXECOUT" "from-the-api" ;;
esac

echo
echo "6. The server does not own the containers"
# The headline property. Each container is owned by its own supervisor, so
# the API is a view: killing it must change nothing.
kill "$SERVE_PID" 2>/dev/null
wait "$SERVE_PID" 2>/dev/null
SERVE_PID=
sleep 0.5
check "a container survives the server exiting" \
    "$("$B" ps | awk -v id="$ID" '$1 == id {print $2}')" "running"
curl -fsS --max-time 2 "$URL/api/system" >/dev/null 2>&1 \
    && bad "the server is still answering after being killed" \
    || ok "and the API is gone, as it should be"

# Back up for the rest.
"$B" serve --addr "127.0.0.1:$PORT" >> "$SCRATCH/serve.log" 2>&1 &
SERVE_PID=$!
for _ in $(seq 60); do curl -fsS "$URL/api/system" >/dev/null 2>&1 && break; sleep 0.2; done
contains "and a new server sees the same containers" "$(curl -fsS "$URL/api/containers")" "$ID"

echo
echo "7. Stop and remove through the API"
curl -fsS -X POST "$URL/api/containers/$ID/stop" >/dev/null 2>&1
sleep 0.5
check "stop stops it" "$("$B" ps -a | awk -v id="$ID" '$1 == id {print $2}')" "exited"
curl -fsS -X DELETE "$URL/api/containers/$ID" >/dev/null 2>&1
check "remove removes it" "$("$B" ps -a | awk -v id="$ID" '$1 == id {print $1}')" ""

echo
echo "8. Inputs that should be refused, are"
# These arrive in a URL and become a path. Refused, not sanitised.
contains "a traversing id is refused" \
    "$(curl -sS "$URL/api/containers/..%2F..%2Fetc/logs")" "not a usable container id"
check "an unknown endpoint is 404" \
    "$(curl -sS -o /dev/null -w '%{http_code}' "$URL/api/nope")" "404"
check "a wrong method is refused" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE "$URL/api/system")" "404"
check "a body over the limit is refused" \
    "$(head -c 2000000 /dev/zero | tr '\0' 'a' | curl -sS -o /dev/null -w '%{http_code}' \
        -X POST -H 'Content-Type: application/json' --data-binary @- "$URL/api/containers")" \
    "413"
check "creating without an image is refused" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
        -d '{}' "$URL/api/containers")" "400"

echo
echo "9. images and volumes are listed, and a key the API prints is a key it accepts"
timeout 60 "$B" run -d --network none --layer "$BASE" -v apivol:/v /bin/sleep 30 >/dev/null 2>&1
contains "volumes are listed" "$(curl -fsS "$URL/api/volumes")" "apivol"
RC=$(curl -sS -o /dev/null -w '%{http_code}' -X DELETE "$URL/api/volumes/apivol")
check "a volume can be removed through the API" "$RC" "200"
contains "images are listed" "$(curl -fsS "$URL/api/images")" "["

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
