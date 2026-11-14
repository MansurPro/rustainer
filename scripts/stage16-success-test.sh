#!/bin/bash
# Stage 16 success test - volumes and bind mounts.
#
#   sudo bash scripts/stage16-success-test.sh
#   unshare --user --map-root-user bash scripts/stage16-success-test.sh
#
# A volume is a deliberate hole in the container's filesystem isolation.
# That is what it is for, and it is why the assertions here are as much
# about what does NOT happen as about what does: the target is created
# inside the container and never on the host, `:ro` is read-only through
# submounts and not merely at the top, a volume nobody mounted is not
# visible, and mounting over `/` is refused rather than attempted.
#
# The two mechanics worth knowing, because both have a wrong version that
# appears to work:
#
#   - The SOURCE is resolved by the parent, before the pivot. The container
#     never influences it, so no symlink inside the image can redirect the
#     mount. Resolving device and volume sources through the container's
#     own tree is the vector CVE-2025-31133 used.
#   - The TARGET and every directory above it are created by the CHILD,
#     above the overlay. Created parent-side they land underneath the
#     overlay, which then hides them, and the mount fails with ENOENT
#     pointing at a path that plainly exists.

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
refuses() { # label, expected substring, command...
    local label=$1 want=$2; shift 2
    local out rc
    out=$("$@" 2>&1); rc=$?
    if [ "$rc" -ne 0 ] && echo "$out" | grep -q "$want"; then
        ok "$label"
    else
        bad "$label (rc=$rc, said: $(echo "$out" | tail -1 | cut -c1-110))"
    fi
}

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage16.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    umount "$SCRATCH/host/sub" 2>/dev/null
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

HOST="$SCRATCH/host"
mkdir -p "$HOST"
echo "from-the-host" > "$HOST/file.txt"

R() { timeout 60 "$B" run --network none --layer "$BASE" "$@" 2>&1 | tail -1; }

echo
echo "1. A bind mount is the same directory, seen from both sides"
check "the container reads what the host wrote" \
    "$(R -v "$HOST:/data" /bin/cat /data/file.txt)" "from-the-host"
R -v "$HOST:/data" /bin/sh -c 'echo from-the-container > /data/written.txt' >/dev/null
check "and the host reads what the container wrote" \
    "$(cat "$HOST/written.txt" 2>/dev/null)" "from-the-container"

echo
echo "2. :ro is read-only, and read-only through submounts"
check "a write is refused" \
    "$(R -v "$HOST:/data:ro" /bin/sh -c '( echo x > /data/nope ) 2>/dev/null && echo wrote || echo refused')" \
    "refused"
# A plain read-only remount leaves every submount of the source writable,
# which is the trap /sys hit: the top looks right and the inside is not.
mkdir -p "$HOST/sub"
if mount -t tmpfs tmpfs "$HOST/sub" 2>/dev/null; then
    echo seed > "$HOST/sub/seed"
    check "a submount of the source is read-only too" \
        "$(R -v "$HOST:/data:ro" /bin/sh -c '( echo x > /data/sub/nope ) 2>/dev/null && echo wrote || echo refused')" \
        "refused"
    check "and still readable" "$(R -v "$HOST:/data:ro" /bin/cat /data/sub/seed)" "seed"
    umount "$HOST/sub" 2>/dev/null
else
    skip "cannot create a submount here, so recursive read-only is untested"
fi

echo
echo "3. A named volume outlives the container that made it"
R -v keeper:/v /bin/sh -c 'echo persisted > /v/a' >/dev/null
check "a second container sees the first one's writes" \
    "$(R -v keeper:/v /bin/cat /v/a)" "persisted"
check "a container that does not mount it cannot see it" \
    "$(R /bin/sh -c 'ls /v >/dev/null 2>&1 && echo visible || echo absent')" "absent"

echo
echo "4. The mount point is created inside the container, never on the host"
# Every directory above the target too: `-v v:/var/lib/app/data` has to
# work on an image with no /var/lib/app.
check "a nested target the image lacks is created" \
    "$(R -v keeper:/var/lib/app/data /bin/cat /var/lib/app/data/a)" "persisted"
R -v "$HOST:/etc/mountpoint" /bin/true >/dev/null
[ -e /etc/mountpoint ] \
    && bad "the container's mount point was created on the HOST" \
    || ok "no mount point appeared on the host"
check "and it did not appear in the image's layer either" \
    "$([ -e "$BASE/etc/mountpoint" ] && echo leaked || echo clean)" "clean"

echo
echo "5. A volume shadows the image's content, and only while it is mounted"
mkdir -p "$BASE/shadowed" && echo from-the-image > "$BASE/shadowed/which"
check "without a volume the image's file is there" \
    "$(R /bin/cat /shadowed/which)" "from-the-image"
check "with a volume the image's file is hidden" \
    "$(R -v keeper:/shadowed /bin/sh -c 'cat /shadowed/which 2>/dev/null || echo hidden')" "hidden"
check "and the image is unchanged afterwards" \
    "$(cat "$BASE/shadowed/which")" "from-the-image"

echo
echo "6. Removing a container does not remove its volumes"
# The opposite would be a data-loss bug, and `rm` is the command people
# run most freely.
ID=$(timeout 60 "$B" run -d --network none --layer "$BASE" -v keeper:/v /bin/sleep 30 2>/dev/null)
[ -n "$ID" ] && timeout 60 "$B" stop -t 0 "$ID" >/dev/null 2>&1 && "$B" rm "$ID" >/dev/null 2>&1
check "the volume survives rm" "$(R -v keeper:/v /bin/cat /v/a)" "persisted"

echo
echo "7. volume ls and volume rm"
"$B" volume ls | grep -q keeper && ok "volume ls lists it" || bad "volume ls did not list it"
"$B" volume rm keeper >/dev/null 2>&1 && ok "volume rm removes it" || bad "volume rm failed"
"$B" volume ls | grep -q keeper && bad "it is still listed after rm" || ok "it is gone after rm"
refuses "volume rm refuses a path, not just a name" "not a volume name" \
    "$B" volume rm ../../etc
refuses "volume rm refuses one that does not exist" "no such volume" \
    "$B" volume rm neverexisted

echo
echo "8. Refusals say what is wrong"
refuses "a relative destination" "absolute path" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "$HOST:relative" /bin/true
refuses "mounting over /" "whole" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "$HOST:/" /bin/true
refuses "a destination containing .." "may not contain" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "$HOST:/a/../b" /bin/true
refuses "an unusable volume name" "usable volume name" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "bad name:/x" /bin/true
refuses "an unknown mount option" "not a mount option" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "$HOST:/data:z" /bin/true
refuses "a one-field volume spec" "is not" \
    timeout 60 "$B" run --network none --layer "$BASE" -v "/data" /bin/true

echo
echo "9. A missing host directory is created, a missing host FILE is not"
# Docker creates a missing directory and scripts depend on it. A missing
# file is far more likely to be a typo than an intention, so binding one
# that does not exist produces an empty file rather than a confusing
# mount error - we create the directory, and the leaf becomes a file only
# when the source already is one.
NEW="$SCRATCH/created-on-demand"
R -v "$NEW:/d" /bin/sh -c 'echo made > /d/f' >/dev/null
check "a missing host directory is created" "$(cat "$NEW/f" 2>/dev/null)" "made"
echo hostfile > "$SCRATCH/single.txt"
check "a host FILE binds as a file" \
    "$(R -v "$SCRATCH/single.txt:/etc/single.txt" /bin/cat /etc/single.txt)" "hostfile"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
