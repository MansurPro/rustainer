#!/bin/bash
# Stage 23 success test - user-defined networks, and names between them.
#
#   sudo bash scripts/stage23-success-test.sh
#
# A network is three things: a bridge, a subnet, and a boundary. The
# first two are easy and the third is the one that is quietly missing
# from a naive implementation - `ip_forward` is on and both bridges are
# on the same host, so the kernel cheerfully routes between them. Names
# not resolving across networks makes it LOOK isolated, right up until
# somebody types an address. This test types the address.
#
# Name resolution is /etc/hosts, rewritten as containers come and go, and
# not DNS. Docker runs a resolver at 127.0.0.11 inside each container's
# network namespace; that needs a process living in that namespace, and
# this engine has no daemon to be one. The file is already bind-mounted
# from the state directory, `std::fs::write` truncates the same inode,
# and both glibc and musl re-read /etc/hosts per lookup - so writing it
# again is the entire mechanism, and `cat /etc/hosts` is the entire
# debugging story.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage23.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=3g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    if [ "$MOUNTED_TMPFS" = yes ]; then
        for _ in 1 2 3 4 5 6; do umount "$SCRATCH" 2>/dev/null && break; sleep 0.5; done
    fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

IN_USERNS=no
[ "$(awk '{print $1, $2, $3}' /proc/self/uid_map 2>/dev/null)" = "0 0 4294967295" ] || IN_USERNS=yes

echo
echo "1. The records are validated before any interface is made"
refuses "a name that cannot be a bridge is refused" "usable network name" \
    "$B" network create "bad name"
for builtin in bridge none host; do
    refuses "the built-in mode $builtin cannot be shadowed" "built-in network mode" \
        "$B" network create "$builtin"
done
refuses "a subnet that is not CIDR is refused" "CIDR" \
    "$B" network create weird --subnet 10.89.9.0
refuses "an impractical prefix is refused" "not supported" \
    "$B" network create weird --subnet 10.89.9.0/30
refuses "removing a network that is not there says so" "no network" \
    "$B" network rm neverexisted
refuses "--network with no such network points at create" "network create" \
    timeout 60 "$B" run --network ghost --rootfs / /bin/true

echo
echo "2. Networks are created, listed and removed"
"$B" network create app >/dev/null 2>&1 && ok "a network is created" || bad "create failed"
"$B" network create data --subnet 10.90.7.0/24 >/dev/null 2>&1 \
    && ok "and one with a subnet of its own" || bad "create with --subnet failed"
LS=$("$B" network ls)
contains "ls shows the built-in bridge" "$LS" "bridge"
contains "ls shows ours" "$LS" "app"
contains "with the subnet we asked for" "$LS" "10.90.7.1/24"
refuses "creating the same name twice is refused" "already exists" \
    "$B" network create app
refuses "and so is a colliding subnet" "already uses" \
    "$B" network create third --subnet 10.90.7.0/24

if [ "$IN_USERNS" = yes ]; then
    echo
    echo "In a user namespace there is no bridge, so nothing below can run."
    echo "The engine refuses it by name rather than producing a container"
    echo "with an address nothing routes."
    OUT=$(timeout 60 "$B" run --network app --rootfs / /bin/true 2>&1)
    echo "$OUT" | grep -q 'CAP_NET_ADMIN' \
        && ok "bridged networking refuses and names what is degraded" \
        || bad "the refusal did not name the capability: $(echo "$OUT" | tail -1 | cut -c1-90)"
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

[ "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)" = 1 ] || echo 1 > /proc/sys/net/ipv4/ip_forward

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

R() { timeout 60 "$B" exec "$1" /bin/sh -c "$2" 2>&1 | tail -1; }
# The WHOLE output, newlines folded: `tail -1` is right for a one-line
# answer and wrong for a file, and /etc/hosts is a file.
RALL() { timeout 60 "$B" exec "$1" /bin/sh -c "$2" 2>&1 | tr '\n' ' '; }

echo
echo "3. Each network has a subnet of its own"
timeout 120 "$B" run -d --name alpha --network app  "$IMG" /bin/sleep 300 >/dev/null 2>&1
timeout 120 "$B" run -d --name beta  --network app  "$IMG" /bin/sleep 300 >/dev/null 2>&1
timeout 120 "$B" run -d --name gamma --network data "$IMG" /bin/sleep 300 >/dev/null 2>&1
sleep 2
A_IP=$(R alpha 'ip -4 -o addr show eth0 | grep -o "inet [0-9.]*" | cut -d" " -f2')
G_IP=$(R gamma 'ip -4 -o addr show eth0 | grep -o "inet [0-9.]*" | cut -d" " -f2')
case "$A_IP" in 10.89.0.*) ok "app's container is on app's subnet ($A_IP)" ;; *) bad "app address is $A_IP" ;; esac
case "$G_IP" in 10.90.7.*) ok "data's container is on data's subnet ($G_IP)" ;; *) bad "data address is $G_IP" ;; esac

echo
echo "4. Containers on one network find each other by name"
contains "the peer is in /etc/hosts" "$(RALL alpha 'cat /etc/hosts')" "beta"
check "and is reachable by that name" \
    "$(R alpha 'ping -c1 -W2 beta >/dev/null 2>&1 && echo ok || echo failed')" "ok"
check "in both directions" \
    "$(R beta 'ping -c1 -W2 alpha >/dev/null 2>&1 && echo ok || echo failed')" "ok"

echo
echo "5. A container on another network cannot reach them - by name OR address"
# The assertion that matters. Names not resolving is not isolation; it
# only looks like it until somebody types an address.
check "the name does not resolve" "$(R gamma 'grep -c alpha /etc/hosts || echo 0')" "0"
check "and the ADDRESS does not answer" \
    "$(R gamma "ping -c1 -W2 $A_IP >/dev/null 2>&1 && echo REACHABLE || echo blocked")" "blocked"
# Isolation must not cost egress: the whole point of a bridge is a route
# off the machine.
check "but the outside world is still reachable from both" \
    "$(R gamma 'ping -c1 -W3 1.1.1.1 >/dev/null 2>&1 && echo ok || echo no-egress')" "ok"
check "and from the other network too" \
    "$(R alpha 'ping -c1 -W3 1.1.1.1 >/dev/null 2>&1 && echo ok || echo no-egress')" "ok"

echo
echo "6. Isolation survives the container that is started afterwards"
# The bug this caught: `ensure_forward` inserts blanket accepts at the
# TOP of FORWARD, so every container after the first pushed its accepts
# above the isolation jump and the jump stopped seeing packets.
timeout 120 "$B" run -d --name delta --network data "$IMG" /bin/sleep 300 >/dev/null 2>&1
sleep 1.5
check "a later container still cannot cross" \
    "$(R delta "ping -c1 -W2 $A_IP >/dev/null 2>&1 && echo REACHABLE || echo blocked")" "blocked"
check "and the first one still cannot either" \
    "$(R gamma "ping -c1 -W2 $A_IP >/dev/null 2>&1 && echo REACHABLE || echo blocked")" "blocked"

echo
echo "7. The hosts file follows membership"
timeout 60 "$B" stop -t 0 beta >/dev/null 2>&1
sleep 1.5
check "a peer that stopped is removed from it" "$(R alpha 'grep -c beta /etc/hosts || echo 0')" "0"
timeout 120 "$B" rm --force beta >/dev/null 2>&1
timeout 120 "$B" run -d --name epsilon --network app "$IMG" /bin/sleep 300 >/dev/null 2>&1
sleep 1.5
contains "and a peer that joined is added" "$(RALL alpha 'cat /etc/hosts')" "epsilon"

echo
echo "8. A network in use cannot be removed"
refuses "removing it names the containers in the way" "still on app" \
    "$B" network rm app
for c in alpha gamma delta epsilon; do
    timeout 60 "$B" stop -t 0 "$c" >/dev/null 2>&1
    timeout 60 "$B" rm --force "$c" >/dev/null 2>&1
done
sleep 1
"$B" network rm app >/dev/null 2>&1 && ok "and it can once they are gone" || bad "rm failed when empty"
"$B" network rm data >/dev/null 2>&1

echo
echo "9. It is all in the event log"
EV=$("$B" events --since 900 --limit 500)
contains "creating a network" "$EV" "network-create"
contains "removing one" "$EV" "network-remove"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
