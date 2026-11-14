#!/bin/bash
# Stage 7 success test - RND.md section 17, "Layered filesystem".
#
#   sudo bash scripts/stage7-success-test.sh
#   unshare --user --map-root-user bash scripts/stage7-success-test.sh
#
# The 300-layer case is the one that matters. The legacy mount(2) options
# page is a single 4095-byte buffer that the kernel fills and does NOT
# NUL-terminate, so a long layer list is silently truncated and the mount
# SUCCEEDS with layers missing. `mount -o lowerdir+=` does not escape it
# either - util-linux still issues one mount(2). Only genuine fsconfig(2)
# calls do, and this is the test that proves we are making them.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage7.XXXXXX")
# OverlayFS refuses an upperdir that is itself on overlayfs, which is
# exactly where a container's /tmp lives. Give the store a tmpfs when that
# is the case: tmpfs IS accepted as an upperdir on a current kernel, and
# the engine reports the kernel's own refusal when it is not.
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    if mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null; then
        MOUNTED_TMPFS=yes
        echo "store on a tmpfs, because the default location is overlayfs"
    else
        echo "cannot host an upperdir here (overlayfs) and cannot mount a tmpfs" >&2
        exit 2
    fi
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

# NOT a shell function wrapped in `timeout`: `timeout` execs a program, so
# `timeout 300 L ...` is "command not found" and the assertion silently sees
# empty output. Everything timed out below is the binary itself.
L() { timeout 300 "$B" run --network host --layer "$BASE" "$@"; }
nohang() { timeout 300 "$@"; }

echo
echo "1. A container runs on a stack of layers"
check "the base layer is readable through the overlay" \
    "$(L /bin/cat /etc/alpine-release 2>/dev/null | head -1 | cut -d. -f1)" "3"
# The filesystem type is the field after the " - " separator, not a fixed
# column: mountinfo has a variable number of optional fields before it.
check "/ is an overlay mount" \
    "$(L /bin/sh -c 'awk "\$5 == \"/\" { for (i = 7; i <= NF; i++) if (\$i == \"-\") { print \$(i + 1); break } }" /proc/self/mountinfo' 2>/dev/null | tail -1)" \
    "overlay"

echo
echo "2. Each container gets a private writable upper"
# Two containers from one set of layers: a write in one must be invisible in
# the other, and gone when it exits.
L /bin/sh -c 'echo mine > /private-marker' >/dev/null 2>&1
check "a second container cannot see the first's write" \
    "$(L /bin/sh -c '[ -e /private-marker ] && echo leaked || echo private' 2>/dev/null | tail -1)" \
    "private"
check "and the shared lower layer was not modified" \
    "$([ -e "$BASE/private-marker" ] && echo written || echo untouched)" "untouched"

echo
echo "3. A whiteout in an upper layer hides a lower file"
UPPER="$SCRATCH/upper-layer"; mkdir -p "$UPPER/etc"
# A tombstone, as OverlayFS records a deletion: a character device 0/0.
if mknod "$UPPER/etc/alpine-release" c 0 0 2>/dev/null; then
    check "the lower file is hidden by the whiteout" \
        "$("$B" run --network host --layer "$BASE" --layer "$UPPER" /bin/sh -c \
            '[ -e /etc/alpine-release ] && echo visible || echo hidden' 2>/dev/null | tail -1)" \
        "hidden"
else
    echo "  note: mknod needs CAP_MKNOD in the initial user namespace; whiteout check skipped"
fi
echo "x" > "$UPPER/added-by-upper"
check "a file added by the upper layer is visible" \
    "$("$B" run --network host --layer "$BASE" --layer "$UPPER" /bin/cat /added-by-upper 2>/dev/null | tail -1)" \
    "x"

echo
echo "4. THE 300-LAYER TEST: the options page is escaped"
MANY="$SCRATCH/many"; mkdir -p "$MANY"
ARGS=()
for i in $(seq 1 300); do
    mkdir -p "$MANY/l$i"
    echo "$i" > "$MANY/l$i/layer-$i"
    ARGS+=(--layer "$MANY/l$i")
done
# Every one of the 300 marker files must be present. A silently truncated
# option string mounts fine and is missing most of them, which is exactly
# the failure this is here to catch.
N=$(nohang "$B" run --network host --layer "$BASE" "${ARGS[@]}" \
    /bin/sh -c 'ls /layer-* 2>/dev/null | wc -l' 2>/dev/null | tail -1 | tr -d ' ')
check "all 300 layers are present and readable" "${N:-0}" "300"

echo
echo "5. Beyond the kernel's own ceiling, we refuse before the syscall"
TOO=("--layer" "$BASE")
for i in $(seq 1 520); do mkdir -p "$MANY/x$i"; TOO+=(--layer "$MANY/x$i"); done
"$B" run --network host "${TOO[@]}" /bin/true 2>&1 | grep -q '500' \
    && ok "more than OVL_MAX_STACK layers names the limit" \
    || bad "an over-long stack did not name the kernel limit"

echo
echo "6. --read-only"
check "the filesystem is read-only when asked" \
    "$(nohang "$B" run --network host --read-only --layer "$BASE" /bin/sh -c \
        'echo x > /nope 2>/dev/null && echo writable || echo readonly' 2>/dev/null | tail -1)" \
    "readonly"
check "and writable when not" \
    "$(L /bin/sh -c 'echo x > /yes 2>/dev/null && echo writable || echo readonly' 2>/dev/null | tail -1)" \
    "writable"

echo
echo "7. Layers are extracted once and shared"
# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine
IMG=$PULLED
BEFORE=$(find "$RUSTAINER_IMAGE_ROOT" -maxdepth 2 -name 'layers*' -prune -o -type d -print 2>/dev/null | wc -l)
nohang "$B" run --network host "$IMG" /bin/true >/dev/null 2>&1
LAYERS=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -maxdepth 1 -mindepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
[ "${LAYERS:-0}" -ge 1 ] && ok "the image's layers are in the store as separate directories" \
    || bad "no extracted layers found"
nohang "$B" run --network host "$IMG" /bin/sh -c 'echo a > /tmp/x' >/dev/null 2>&1
AFTER=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -maxdepth 1 -mindepth 1 -type d 2>/dev/null | wc -l | tr -d ' ')
check "a second run re-extracts nothing" "$AFTER" "$LAYERS"
echo "  (before: $BEFORE)"

echo
mirror_note
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
