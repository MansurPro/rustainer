#!/bin/bash
# Stage 9 success test - RND.md section 17, "Bridge, NAT, and the outside world".
#
#   sudo bash scripts/stage9-success-test.sh
#
# The criterion that is satisfiable with only this stage's work is "two
# containers ping each other by address". `apk add curl` comes after, and it
# is the reason DNS generation is folded INTO this stage rather than left to
# a later one: a container with an address, a route and working NAT has no
# nameserver at all, so the stage looks broken in a way every user
# misdiagnoses as "networking is broken".
#
# This stage makes the largest irreversible host mutation in the project - a
# bridge holding an address and an on-link route - so it needs real root,
# not a user namespace.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage9.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
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

# A user namespace cannot put a veth peer into the host's namespace, so the
# host half of a bridge is out of reach. Say so and test the refusal
# instead of pretending.
IN_USERNS=no
[ "$(awk '{print $1, $2, $3}' /proc/self/uid_map 2>/dev/null)" = "0 0 4294967295" ] || IN_USERNS=yes

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

R() { timeout 120 "$B" run --layer "$BASE" "$@"; }

if [ "$IN_USERNS" = yes ]; then
    echo
    echo "In a user namespace: the host half of a bridge is unreachable."
    OUT=$(R /bin/true 2>&1); RC=$?
    [ "$RC" -ne 0 ] && ok "bridged networking refuses rather than half-configuring" \
        || bad "it claimed to work"
    echo "  $(echo "$OUT" | tail -1)"
    OUT=$(timeout 60 "$B" run --network none --layer "$BASE" /bin/sh -c 'ip route | wc -l' 2>/dev/null | tail -1)
    check "--network none still works" "$OUT" "0"
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

if ! [ "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)" = 1 ]; then
    echo
    echo "ip_forward is off; checking that we say so rather than writing it"
    R /bin/true 2>&1 | grep -q 'sysctl -w net.ipv4.ip_forward=1' \
        && ok "the refusal names the command to run" \
        || bad "forwarding was off and we did not say how to fix it"
    sysctl -w net.ipv4.ip_forward=1 >/dev/null 2>&1 || true
fi

echo
echo "1. A bridged container has an address and a route"
# `ip addr` and grep, not `-o` or a per-device filter: busybox's ip has
# neither. Stderr is shown when it comes back empty, because an empty
# answer here almost always means the container never started.
IP=$(R /bin/sh -c 'ip addr 2>/dev/null | grep -o "inet 172\.30\.[0-9.]*"' 2>/dev/null | tail -1)
if [ -z "$IP" ]; then
    echo "  (no address; container output was:)"
    R /bin/sh -c 'ip addr' 2>&1 | tail -4 | sed 's/^/    /'
fi
case "$IP" in
    "inet 172.30."*) ok "it holds an address from the bridge subnet ($IP)" ;;
    *) bad "no address on eth0 (got '$IP')" ;;
esac
# Filter with awk, not `ip route show default`: busybox's ip ignores the
# filter and prints the whole table, so `tail -1` reads the wrong line.
check "it has a default route via the bridge" \
    "$(R /bin/sh -c 'ip route 2>/dev/null | awk "/^default/ {print \$3}"' 2>/dev/null | tail -1)" \
    "172.30.0.1"
check "the bridge exists on the host" \
    "$([ -d /sys/class/net/rustainer0 ] && echo present || echo missing)" "present"

echo
echo "2. THE CRITERION: two containers reach each other by address"
A=$("$B" run -d --layer "$BASE" /bin/sh -c 'sleep 25' 2>/dev/null)
sleep 1
A_IP=$(grep -o '"ip": *"[^"]*"' "$RUSTAINER_ROOT/containers/$A/config.json" | cut -d'"' -f4)
echo "  container A: $A_IP"
if [ -n "$A_IP" ]; then
    OUT=$(R /bin/sh -c "ping -c2 -W2 $A_IP >/dev/null 2>&1 && echo reachable || echo unreachable" 2>/dev/null | tail -1)
    check "a second container can ping the first" "$OUT" "reachable"
else
    bad "the first container recorded no address"
fi
timeout 60 "$B" stop -t 2 "$A" >/dev/null 2>&1

echo
echo "3. The container can reach the outside world"
check "a public address answers" \
    "$(R /bin/sh -c 'ping -c2 -W3 1.1.1.1 >/dev/null 2>&1 && echo reachable || echo unreachable' 2>/dev/null | tail -1)" \
    "reachable"

echo
echo "4. DNS, which is part of this stage and not the next one"
# A container with an address, a route and working NAT has NO nameserver.
# 127.0.0.53 is meaningless here: each namespace has its own loopback, so a
# copied systemd-resolved stub address points at nothing.
NS=$(R /bin/cat /etc/resolv.conf 2>/dev/null | grep nameserver | head -1)
echo "  $NS"
echo "$NS" | grep -q '127\.' && bad "a loopback resolver was copied into the container" \
    || ok "no loopback resolver was copied in"
check "a name resolves" \
    "$(R /bin/sh -c 'nslookup example.com >/dev/null 2>&1 && echo resolved || echo failed' 2>/dev/null | tail -1)" \
    "resolved"
check "/etc/hosts names the container itself" \
    "$(R --hostname boxy /bin/sh -c 'grep -c boxy /etc/hosts' 2>/dev/null | tail -1)" "1"

echo
echo "5. The real payoff, which needs all of the above"
check "apk can reach a mirror and install a package" \
    "$(R /bin/sh -c 'apk add --no-cache curl >/dev/null 2>&1 && echo installed || echo failed' 2>/dev/null | tail -1)" \
    "installed"

echo
echo "6. Host state is bounded"
BRIDGES=$(ls -d /sys/class/net/rustainer* 2>/dev/null | wc -l | tr -d ' ')
check "exactly one bridge, however many containers ran" "$BRIDGES" "1"
sleep 1
VETHS=$(ls -d /sys/class/net/rtv* 2>/dev/null | wc -l | tr -d ' ')
check "no veth outlives its container" "$VETHS" "0"

echo
echo "7. Refusals stay explicit"
"$B" run --network nonsense --layer "$BASE" /bin/true 2>&1 | grep -q 'not supported' \
    && ok "an unknown network mode refuses" || bad "an unknown network mode was accepted"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
