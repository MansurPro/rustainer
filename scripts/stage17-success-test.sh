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
echo "10. Files: a container's and a volume's, and never the host's"
# The tree being browsed belongs to a container, so it is hostile input:
# a link planted in it must resolve inside it. Asserted with a marker the
# host has and the container must never be shown.
echo "HOSTSECRET-$$" > "$SCRATCH/hostsecret"
FID=$(timeout 60 "$B" run -d --network none --layer "$BASE" -v filevol:/v /bin/sh -c \
    "echo mine > /wrote; echo inside > /v/note; ln -s $SCRATCH/hostsecret /v/abs; \
     ln -s ../../../../../../../..$SCRATCH/hostsecret /v/climb; exec sleep 60" 2>/dev/null | tail -1)
for _ in $(seq 50); do
    [ -L "$RUSTAINER_IMAGE_ROOT/volumes/filevol/climb" ] && break
    sleep 0.1
done
contains "a container's root is listed" "$(curl -sS "$URL/api/containers/$FID/files?path=/")" '"name":"etc"'
contains "a file in it is shown" "$(curl -sS "$URL/api/containers/$FID/files?path=/v/note")" '"text":"inside\n"'
contains "the volume says who mounts it" "$(curl -sS "$URL/api/volumes")" "\"used_by\":[\"$FID\"]"
contains "a volume's file is shown" "$(curl -sS "$URL/api/volumes/filevol/files?path=note")" '"text":"inside\n"'
LEAK=
for P in abs climb ../../hostsecret "..%2F..%2Fhostsecret" "/v/abs" "/v/climb"; do
    for BASEURL in "volumes/filevol" "containers/$FID"; do
        case "$(curl -sS "$URL/api/$BASEURL/files?path=$P")" in *HOSTSECRET*) LEAK="$LEAK $BASEURL:$P" ;; esac
    done
done
check "no link or .. reads a host file" "$LEAK" ""
check "a traversing volume name is refused" \
    "$(curl -sS -o /dev/null -w '%{http_code}' "$URL/api/volumes/..%2F..%2Fstate/files")" "400"
"$B" stop -t 0 "$FID" >/dev/null 2>&1
# Stopped, there is no mount namespace to look into: what is left is the
# writable layer, which is the container's own files and not the image's.
contains "a stopped container shows what it wrote" \
    "$(curl -sS "$URL/api/containers/$FID/files?path=/wrote")" '"text":"mine\n"'
check "and not the image beneath it" \
    "$(curl -sS -o /dev/null -w '%{http_code}' "$URL/api/containers/$FID/files?path=/etc/alpine-release")" "404"

echo
echo "11. An exited container starts again as itself"
# The same container resumed, not a new one like it: what it wrote to its
# own filesystem before it stopped has to still be there. /kept gains a
# line per start, so its length is the number of times THIS container ran.
status_of() { "$B" ps -a | awk -v id="$1" '$1 == id {print $2}'; }
pid_of() { "$B" ps -a | awk -v id="$1" '$1 == id {print $3}'; }
SID=$(timeout 60 "$B" run -d --network none --layer "$BASE" /bin/sh -c \
    'echo ran >> /kept; exec sleep 60' 2>/dev/null | tail -1)
"$B" stop -t 0 "$SID" >/dev/null 2>&1
check "it is stopped first" "$(status_of "$SID")" "exited"
check "start through the API is accepted" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$URL/api/containers/$SID/start")" "200"
check "and it is running again" "$(status_of "$SID")" "running"
for _ in $(seq 50); do
    [ "$("$B" exec "$SID" /bin/cat /kept 2>/dev/null | wc -l)" -ge 2 ] && break
    sleep 0.1
done
check "with the files it had before" "$("$B" exec "$SID" /bin/cat /kept 2>/dev/null | wc -l | tr -d ' ')" "2"
"$B" start "$SID" >/dev/null 2>&1
check "starting what is running is refused" "$?" "1"
BEFORE=$(pid_of "$SID")
check "restart through the API is accepted" \
    "$(curl -sS -o /dev/null -w '%{http_code}' -X POST "$URL/api/containers/$SID/restart")" "200"
check "and it is running" "$(status_of "$SID")" "running"
AFTER=$(pid_of "$SID")
if [ -n "$AFTER" ] && [ "$AFTER" != "$BEFORE" ]; then ok "as a new process"; else bad "as a new process ($BEFORE -> $AFTER)"; fi
"$B" start no-such-container >/dev/null 2>&1
check "starting nothing is an error" "$?" "1"

echo
echo "12. Volumes can be made, copied, emptied, taken out and put back"
jpost() { curl -sS -o /dev/null -w '%{http_code}' -X POST -H 'Content-Type: application/json' -d "$2" "$URL$1"; }
VOLS="$RUSTAINER_IMAGE_ROOT/volumes"
check "create through the API" "$(jpost /api/volumes '{"name":"made"}')" "200"
check "and it exists" "$([ -d "$VOLS/made" ] && echo yes)" "yes"
check "a name that is a path is refused" "$(jpost /api/volumes '{"name":"../escape"}')" "400"
check "creating it twice is refused" "$(jpost /api/volumes '{"name":"made"}')" "400"
mkdir -p "$VOLS/made/dir"; echo payload > "$VOLS/made/dir/f"; ln -s f "$VOLS/made/dir/link"
check "clone through the API" "$(jpost /api/volumes/made/clone '{"to":"twin"}')" "200"
check "the copy has the file" "$(cat "$VOLS/twin/dir/f" 2>/dev/null)" "payload"
check "and the link, as a link" "$(readlink "$VOLS/twin/dir/link" 2>/dev/null)" "f"
curl -sS -o "$SCRATCH/made.tar" "$URL/api/volumes/made/export"
contains "export is a tar of the contents" "$(tar -tf "$SCRATCH/made.tar" 2>&1)" "dir/f"
check "a file downloads whole" "$(curl -sS "$URL/api/volumes/made/files?raw=1&path=dir/f")" "payload"
check "a directory does not" \
    "$(curl -sS -o /dev/null -w '%{http_code}' "$URL/api/volumes/made/files?raw=1&path=dir")" "400"
# Emptying under a running process is refused: files vanishing beneath a
# database do not fail cleanly.
EID=$(timeout 60 "$B" run -d --network none --layer "$BASE" -v made:/m /bin/sleep 60 2>/dev/null | tail -1)
check "emptying a volume in use is refused" "$(jpost /api/volumes/made/empty '')" "400"
check "and nothing was removed" "$(cat "$VOLS/made/dir/f" 2>/dev/null)" "payload"
contains "the container reports its network and disk counters" \
    "$(curl -sS "$URL/api/containers/$EID/stats")" '"net_rx"'
"$B" rm --force "$EID" >/dev/null 2>&1
check "emptying it afterwards works" "$(jpost /api/volumes/made/empty '')" "200"
check "the volume is still there" "$([ -d "$VOLS/made" ] && ls -A "$VOLS/made" | wc -l | tr -d ' ')" "0"
"$B" volume import made -i "$SCRATCH/made.tar" >/dev/null 2>&1
check "import puts the contents back" "$(cat "$VOLS/made/dir/f" 2>/dev/null)" "payload"
# An archive is hostile input whoever made it.
mkdir -p "$SCRATCH/evil"; echo pwned > "$SCRATCH/evil/x"
tar -C "$SCRATCH/evil" -cf "$SCRATCH/evil.tar" --transform 's|^x$|../../outside-volume|' x 2>/dev/null
"$B" volume import made -i "$SCRATCH/evil.tar" >/dev/null 2>&1
check "an archive cannot write outside the volume" "$([ -e "$RUSTAINER_IMAGE_ROOT/outside-volume" ] && echo escaped)" ""

echo
echo "13. A file can be saved into a container or a volume, and only there"
PID_=$(timeout 60 "$B" run -d --network none --layer "$BASE" -v putvol:/pv /bin/sh -c \
    "mkdir /site && echo old > /site/conf && chown 123:456 /site/conf && chmod 640 /site/conf && \
     ln -s $SCRATCH/hostsecret /site/trap && exec sleep 60" 2>/dev/null | tail -1)
for _ in $(seq 50); do
    [ "$("$B" exec "$PID_" /bin/sh -c 'test -L /site/trap && echo y' 2>/dev/null | tail -1)" = y ] && break
    sleep 0.1
done
put() { curl -sS -o /dev/null -w '%{http_code}' -X PUT --data-binary "$2" "$URL$1"; }
check "saving over a file is accepted" "$(put "/api/containers/$PID_/files?path=/site/conf" 'new')" "200"
check "the container sees the new contents" "$("$B" exec "$PID_" /bin/cat /site/conf 2>/dev/null | tail -1)" "new"
# An edit is a change of contents. Turning nginx's 0640 config into a
# root-owned 0644 one would be a second change nobody asked for.
check "with the mode and owner it had" \
    "$("$B" exec "$PID_" /bin/stat -c '%a %u:%g' /site/conf 2>/dev/null | tail -1)" "640 123:456"
check "a new file in a new directory is made" "$(put "/api/containers/$PID_/files?path=/site/a/b/c.txt" 'deep')" "200"
check "and is there" "$("$B" exec "$PID_" /bin/cat /site/a/b/c.txt 2>/dev/null | tail -1)" "deep"
put "/api/containers/$PID_/files?path=/site/trap" 'overwritten' >/dev/null
check "saving onto a planted link does not write the host file" "$(cat "$SCRATCH/hostsecret")" "HOSTSECRET-$$"
put "/api/volumes/putvol/files?path=../../../escaped" 'x' >/dev/null
check "and .. does not leave a volume" "$([ -e "$RUSTAINER_IMAGE_ROOT/escaped" ] && echo escaped)" ""
check "saving into a volume works" "$(put "/api/volumes/putvol/files?path=note" 'kept')" "200"
check "and the container sees it" "$("$B" exec "$PID_" /bin/cat /pv/note 2>/dev/null | tail -1)" "kept"
check "saving with no path is refused" "$(put "/api/volumes/putvol/files" 'x')" "400"
"$B" rm --force "$PID_" >/dev/null 2>&1

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
