#!/bin/bash
# Stage 10 success test - RND.md section 17, "Published ports".
#
#   sudo bash scripts/stage10-success-test.sh
#
# The obvious test - `curl localhost:8080` - cannot pass with firewall rules
# alone. Loopback is DELIBERATELY excluded from destination NAT, because
# translating a 127.0.0.1 destination into a routable address turns a packet
# that could never leave the machine into one that can. So either the
# userland helper is part of this stage or the test has to avoid loopback.
# This ladder includes the helper, and both paths are checked below.
#
# This is the highest-rate host mutation in the project and the only place
# where partial state is a security hole rather than a failure, so the
# teardown assertions matter as much as the reachability ones.

set -u
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rustainer-target}"

PASS=0
FAIL=0
SKIP=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage10.XXXXXX")
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

if [ "$IN_USERNS" = yes ]; then
    echo
    echo "In a user namespace there is no bridge, so there is nothing to publish to."
    OUT=$(timeout 60 "$B" run --network none -p 18080:80 --layer "$BASE" /bin/true 2>&1); RC=$?
    [ "$RC" -ne 0 ] && echo "$OUT" | grep -q 'network bridge' \
        && ok "-p without a bridge refuses and says why" \
        || bad "-p without a bridge did not refuse clearly"
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

[ "$(cat /proc/sys/net/ipv4/ip_forward 2>/dev/null)" = 1 ] || echo 1 > /proc/sys/net/ipv4/ip_forward

# A routable address of this host, for the non-loopback path.
HOST_IP=$(ip -4 route get 1.1.1.1 2>/dev/null | awk '{for(i=1;i<=NF;i++) if($i=="src") print $(i+1)}' | head -1)
echo "host address: ${HOST_IP:-none}"

# A container that answers HTTP on port 80, using only what alpine ships.
SERVE='while true; do printf "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nhi" | nc -l -p 80 >/dev/null 2>&1; done'

echo
echo "1. A published port is reachable on loopback"
# This is the case firewall rules cannot serve, and the reason the userland
# forwarder exists.
ID=$(timeout 60 "$B" run -d -p 18080:80 --layer "$BASE" /bin/sh -c "$SERVE" 2>/dev/null)
sleep 1.5
check "curl localhost:18080 answers" \
    "$(timeout 10 curl -sS --max-time 5 http://127.0.0.1:18080/ 2>/dev/null)" "hi"

echo
echo "2. And on the host's routable address"
if [ -n "$HOST_IP" ]; then
    check "curl \$HOST_IP:18080 answers" \
        "$(timeout 10 curl -sS --max-time 5 "http://$HOST_IP:18080/" 2>/dev/null)" "hi"
else
    SKIP=$((SKIP+1)); echo "  SKIP  no routable host address"
fi

echo
echo "3. An unpublished port is refused"
timeout 5 curl -sS --max-time 3 http://127.0.0.1:18081/ >/dev/null 2>&1 \
    && bad "a port nobody published answered" || ok "a port nobody published is refused"

echo
echo "4. Both rules are present: translation AND authorization"
# A DNAT without its filter accept merely does not work; an accept left
# behind without the DNAT opens the container to anyone who can route to
# its address. They are written together or not at all.
if command -v iptables >/dev/null 2>&1; then
    DNAT=$(iptables -w 5 -t nat -S PREROUTING 2>/dev/null | grep -c 'dport 18080.*DNAT')
    ACC=$(iptables -w 5 -S FORWARD 2>/dev/null | grep -c 'dport 80.*ACCEPT')
    check "a DNAT rule exists" "$DNAT" "1"
    [ "${ACC:-0}" -ge 1 ] && ok "a matching filter accept exists" || bad "the DNAT has no accept"
else
    SKIP=$((SKIP+1)); echo "  SKIP  no iptables to inspect"
fi
timeout 60 "$B" stop -t 3 "$ID" >/dev/null 2>&1
sleep 0.5

echo
echo "5. Teardown leaves nothing behind"
if command -v iptables >/dev/null 2>&1; then
    check "the DNAT rule is gone" \
        "$(iptables -w 5 -t nat -S PREROUTING 2>/dev/null | grep -c 'dport 18080')" "0"
    check "the filter accept is gone" \
        "$(iptables -w 5 -S FORWARD 2>/dev/null | grep -c '18080')" "0"
fi
timeout 5 curl -sS --max-time 3 http://127.0.0.1:18080/ >/dev/null 2>&1 \
    && bad "the port still answers after the container stopped" \
    || ok "the host port is released"

echo
echo "6. A loopback-only publish is reachable only from the host"
ID2=$(timeout 60 "$B" run -d -p 127.0.0.1:18090:80 --layer "$BASE" /bin/sh -c "$SERVE" 2>/dev/null)
sleep 1.5
check "it answers on loopback" \
    "$(timeout 10 curl -sS --max-time 5 http://127.0.0.1:18090/ 2>/dev/null)" "hi"
if [ -n "$HOST_IP" ]; then
    timeout 5 curl -sS --max-time 3 "http://$HOST_IP:18090/" >/dev/null 2>&1 \
        && bad "a loopback-only publish answered on a routable address" \
        || ok "it does not answer on the routable address"
fi
timeout 60 "$B" stop -t 3 "$ID2" >/dev/null 2>&1

echo
echo "7. The host port is reserved, not silently double-published"
ID3=$(timeout 60 "$B" run -d -p 18099:80 --layer "$BASE" /bin/sh -c 'sleep 20' 2>/dev/null)
sleep 1
OUT=$(timeout 60 "$B" run -p 18099:80 --layer "$BASE" /bin/true 2>&1); RC=$?
[ "$RC" -ne 0 ] && ok "a second container cannot take the same host port" \
    || bad "two containers published the same host port"
timeout 60 "$B" stop -t 3 "$ID3" >/dev/null 2>&1

echo
echo "8. Refusals stay explicit"
timeout 30 "$B" run -p nonsense --layer "$BASE" /bin/true 2>&1 | grep -q 'host-port:container-port' \
    && ok "a malformed -p refuses" || bad "a malformed -p was accepted"
timeout 30 "$B" run -p 8080:80/udp --layer "$BASE" /bin/true 2>&1 | grep -q 'only tcp' \
    && ok "udp refuses rather than silently publishing tcp" || bad "udp was accepted"
timeout 30 "$B" run --network host -p 8080:80 --layer "$BASE" /bin/true 2>&1 | grep -q 'network bridge' \
    && ok "-p with host networking refuses" || bad "-p with host networking was accepted"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
