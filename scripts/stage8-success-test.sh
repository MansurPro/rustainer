#!/bin/bash
# Stage 8 success test - RND.md section 17, "A network namespace of its own".
#
#   sudo bash scripts/stage8-success-test.sh
#   unshare --user --map-root-user bash scripts/stage8-success-test.sh
#
# A small stage with one real trap: making --network=host the default past
# this point. Defaults are the hardest thing to change later, and host
# networking reaches every host-local service, including anything trusting
# 127.0.0.1 and the cloud metadata address. `none` is the default from here.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage8.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
cleanup() {
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
TAR_OWNER=--no-same-owner
[ "$(id -u)" = 0 ] && [ "$(awk '{print $1, $2, $3}' /proc/self/uid_map)" = "0 0 4294967295" ] && TAR_OWNER=--same-owner
tar $TAR_OWNER -xzf "$CACHE" -C "$BASE" || exit 2

# No --network: the default.
R() { timeout 120 "$B" run --layer "$BASE" "$@"; }
HOSTNET() { timeout 120 "$B" run --network host --layer "$BASE" "$@"; }

HOST_NETNS=$(readlink /proc/self/ns/net)
echo "host netns: $HOST_NETNS"

echo
echo "1. The default is a network namespace of the container's own"
GOT=$(R /bin/cat /proc/self/ns/net 2>/dev/null | tail -1)
GOT=$(R /bin/sh -c 'readlink /proc/self/ns/net' 2>/dev/null | tail -1)
[ -n "$GOT" ] && [ "$GOT" != "$HOST_NETNS" ] \
    && ok "the container's net namespace differs from the host's ($GOT)" \
    || bad "the container shares the host's net namespace (got '$GOT')"

echo
echo "2. It contains loopback, up, and nothing else"
# /proc/net/dev, not /sys/class/net. When a fresh sysfs cannot be mounted -
# rootless sharing the host's network namespace ownership - /sys is
# bind-mounted from the host, and /sys/class/net then lists the HOST's
# interfaces whatever network namespace the container is in. procfs is
# per-namespace and tells the truth.
IFACES=$(R /bin/sh -c 'awk -F: "NR>2 {gsub(/ /, \"\", \$1); print \$1}" /proc/net/dev | tr "\n" " "' 2>/dev/null | tail -1)
echo "  devices: $IFACES"
# "A new network namespace contains exactly one device" is not quite true on
# a kernel with the tunnel modules loaded: sit, gre, ipip and ip6_tunnel
# each create a fallback device in EVERY namespace, so a fresh one can also
# hold sit0, tunl0, gre0, ip6tnl0 and friends. They arrive DOWN and
# unconfigured. The property that matters is not "only lo exists" but
# "nothing except lo is up, and there is no route anywhere".
echo "$IFACES" | grep -qw lo && ok "lo is present" || bad "lo is missing"
UP=$(R /bin/sh -c 'ip -o link show up 2>/dev/null | awk -F": " "{print \$2}" | tr "\n" " "' 2>/dev/null | tail -1)
check "nothing but lo is up" "$(echo "$UP" | tr -d ' ')" "lo"
# A fresh namespace has lo DOWN with no 127.0.0.1 at all: the kernel
# installs the address in response to the device coming up, not at
# creation. Software that binds a local listener fails without it.
check "lo is up" \
    "$(R /bin/sh -c 'cat /sys/class/net/lo/operstate' 2>/dev/null | tail -1)" "unknown"
check "lo carries 127.0.0.1" \
    "$(R /bin/sh -c 'ip -4 addr show lo 2>/dev/null | grep -c 127.0.0.1' 2>/dev/null | tail -1)" "1"
check "loopback actually works" \
    "$(R /bin/sh -c 'ping -c1 -W1 127.0.0.1 >/dev/null 2>&1 && echo reachable || echo dead' 2>/dev/null | tail -1)" \
    "reachable"

echo
echo "3. The container has no route off the machine"
check "there is no default route" \
    "$(R /bin/sh -c 'ip route 2>/dev/null | wc -l' 2>/dev/null | tail -1)" "0"

echo
echo "4. A listener inside is invisible to the host"
R /bin/sh -c 'nc -l -p 45999 -s 127.0.0.1 &  sleep 1' >/dev/null 2>&1 &
sleep 0.6
if ss -ltn 2>/dev/null | grep -q ':45999'; then
    bad "the container's listener is visible in the host's socket table"
else
    ok "the container's listener is invisible to the host"
fi
wait 2>/dev/null

echo
echo "5. --network host still behaves as it did in stages 1-7"
check "it shares the host's net namespace" \
    "$(HOSTNET /bin/sh -c 'readlink /proc/self/ns/net' 2>/dev/null | tail -1)" "$HOST_NETNS"
HOSTNET /bin/true 2>&1 | grep -q 'metadata address' \
    && ok "and says plainly what that means" || bad "host networking gave no warning"

echo
echo "6. Nothing is left behind"
sleep 0.5
BEFORE=$(ls /var/run/netns 2>/dev/null | wc -l | tr -d ' ')
check "no named network namespace was created" "${BEFORE:-0}" "0"
LEFT=$(awk '$5 ~ /netns/ {print $5}' /proc/self/mountinfo | wc -l | tr -d ' ')
check "no nsfs bind mount was left" "$LEFT" "0"

echo
echo "7. Refusals stay explicit"
"$B" run --network bridge --layer "$BASE" /bin/true 2>&1 | grep -q 'stage 9' \
    && ok "--network bridge names the stage that will support it" \
    || bad "--network bridge did not refuse clearly"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
