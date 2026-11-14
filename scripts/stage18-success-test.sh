#!/bin/bash
# Stage 18 success test - stacks.
#
#   sudo bash scripts/stage18-success-test.sh
#
# Several containers described in one file, started together, able to find
# each other. Compose-shaped, deliberately not Compose, and the three
# deliberate differences are each asserted here:
#
#   - TOML, not YAML. A configuration language should not be able to
#     surprise you, and `ports: - 8080:80` is a sexagesimal number in
#     YAML 1.1.
#   - An unknown key is an ERROR. A misspelled key in a Compose file does
#     nothing at all, silently, and the service starts without the setting
#     you thought you gave it. That is the single most expensive property
#     of the format.
#   - `command` is a list, never a string. Compose accepts both, and the
#     string form is split on spaces, so the quotes in
#     `sh -c "echo hi; sleep 1"` become part of the arguments and the
#     shell is handed `"echo` as its whole script.
#
# Services find each other through /etc/hosts, not DNS. Every address is
# allocated before anything starts, so every container's hosts file can
# name every other one - including the services that do not exist yet. A
# resolver in each namespace would be a second service to run, to
# supervise and to get wrong, and `cat /etc/hosts` is the whole debugging
# story this way.

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

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage18.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=3g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_COMPAT_STORE:-$SCRATCH/images}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED_TMPFS" = yes ] && umount "$SCRATCH" 2>/dev/null
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

IN_USERNS=no
[ "$(awk '{print $1, $2, $3}' /proc/self/uid_map 2>/dev/null)" = "0 0 4294967295" ] || IN_USERNS=yes

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"

echo
echo "1. The file format refuses what Compose accepts silently"
# These need no containers and no network, so they run first and run
# everywhere.
F="$SCRATCH/rustainer.toml"
printf 'name="x"\n[services.a]\nimgae="alpine"\n' > "$F"
refuses "a misspelled key is an error, not a no-op" "imgae" "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ncommand="/bin/sh -c '"'"'a; b'"'"'"\n' > "$F"
refuses "a string command is refused, with the shape that works" "list of arguments" \
    "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ndepends_on=["b"]\n[services.b]\nimage="i"\ndepends_on=["a"]\n' > "$F"
refuses "a dependency cycle names the services in it" "cycle" "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ndepends_on=["ghost"]\n' > "$F"
refuses "a dependency that does not exist lists the ones that do" "ghost" "$B" stack ps -f "$F"
printf 'name="a/b"\n[services.a]\nimage="i"\n' > "$F"
refuses "a stack name that cannot be a container name" "usable stack name" "$B" stack ps -f "$F"
refuses "a stack file that is not there says so" "reading the stack file" \
    "$B" stack ps -f "$SCRATCH/nope.toml"

if [ "$IN_USERNS" = yes ]; then
    echo
    echo "In a user namespace there is no bridge, so services cannot be given"
    echo "addresses and a stack cannot be wired together."
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

cat > "$F" <<EOF
name = "stagetest"

[services.db]
image = "$IMG"
command = ["/bin/sh", "-c", "echo db-is-up; sleep 300"]
env = ["ROLE=database"]
volumes = ["stagetestdata:/data"]

[services.api]
image = "$IMG"
command = ["/bin/sleep", "300"]
depends_on = ["db"]

[services.web]
image = "$IMG"
command = ["/bin/sleep", "300"]
depends_on = ["api"]
ports = ["18555:80"]
EOF

echo
echo "2. Up starts everything, in dependency order"
UP=$(timeout 300 "$B" stack up -f "$F" 2>&1)
echo "$UP" | tail -1
check "it reports three services started" \
    "$(echo "$UP" | grep -c 'service(s) started')" "1"
# The order is the contract: `ps` prints it, and it must put a service
# after everything it depends on.
ORDER=$("$B" stack ps -f "$F" | tail -n +2 | awk '{print $1}' | tr '\n' ' ')
check "the order is dependencies first" "$ORDER" "db api web "
sleep 1.5
check "all three are running" \
    "$("$B" stack ps -f "$F" | tail -n +2 | awk '$2 == "running"' | grep -c .)" "3"

echo
echo "3. Services find each other by name"
HOSTS=$(timeout 60 "$B" exec stagetest-web /bin/cat /etc/hosts 2>&1)
contains "a peer is in /etc/hosts by its service name" "$HOSTS" "	db"
contains "and by its container name" "$HOSTS" "	stagetest-db"
contains "including one it does not depend on" "$HOSTS" "	api"
check "and it can actually reach it" \
    "$(timeout 60 "$B" exec stagetest-web /bin/sh -c 'ping -c1 -W2 db >/dev/null 2>&1 && echo ok || echo failed' 2>&1 | tail -1)" \
    "ok"
# The one that would be missed by a naive implementation: db started
# FIRST, so its hosts file had to name services that did not exist yet.
check "the first service can reach the last" \
    "$(timeout 60 "$B" exec stagetest-db /bin/sh -c 'ping -c1 -W2 web >/dev/null 2>&1 && echo ok || echo failed' 2>&1 | tail -1)" \
    "ok"

echo
echo "4. The rest of the file reaches the container"
check "env from the stack file" \
    "$(timeout 60 "$B" exec stagetest-db /bin/sh -c 'echo $ROLE' 2>&1 | tail -1)" "database"
contains "the command from the stack file ran" "$("$B" logs stagetest-db 2>&1)" "db-is-up"
check "a volume from the stack file exists" \
    "$("$B" volume ls | grep -c stagetestdata)" "1"
if command -v iptables >/dev/null 2>&1; then
    check "a port from the stack file is published" \
        "$(iptables -w 5 -t nat -S PREROUTING 2>/dev/null | grep -c 'dport 18555')" "1"
else
    skip "no iptables to inspect"
fi

echo
echo "5. They are ordinary containers"
# The property that makes a stack debuggable with the tools you already
# have: nothing about them is special.
contains "ps shows them" "$("$B" ps)" "stagetest-web"
check "exec works on them" \
    "$(timeout 60 "$B" exec stagetest-api /bin/echo ordinary 2>&1 | tail -1)" "ordinary"
check "stop works on one of them" \
    "$(timeout 60 "$B" stop -t 0 stagetest-api >/dev/null 2>&1; "$B" ps -a | awk '$1=="stagetest-api"{print $2}')" \
    "exited"

echo
echo "6. Up again starts only what is missing"
UP2=$(timeout 300 "$B" stack up -f "$F" 2>&1)
contains "the ones already running are left alone" "$UP2" "already running"
check "and the stopped one is replaced" \
    "$(sleep 1; "$B" ps | awk '$1=="stagetest-api"{print $2}')" "running"

echo
echo "7. Down removes the stack, and keeps the volume"
# A volume removed by `down` is a data-loss bug in a command people run
# freely. Docker makes the same promise.
timeout 300 "$B" stack down -f "$F" >/dev/null 2>&1
check "no container of the stack is left" "$("$B" ps -a | grep -c stagetest-)" "0"
check "the volume survives" "$("$B" volume ls | grep -c stagetestdata)" "1"
check "ps on a stopped stack shows it as absent" \
    "$("$B" stack ps -f "$F" | tail -n +2 | awk '$2 == "-"' | grep -c .)" "3"
"$B" volume rm stagetestdata >/dev/null 2>&1

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
