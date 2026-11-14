#!/bin/bash
# Stage 15 success test - a layer's deletions are honoured.
#
#   sudo bash scripts/stage15-success-test.sh
#
# The bug this stage fixes was invisible to every test that came before it,
# and it is the most serious one found in this project.
#
# Each layer is extracted into a directory of its own so layers can be
# shared between images, and those directories are composed at run time by
# overlayfs. A whiteout - the OCI way of saying "this layer deletes that
# file" - was handled by DELETING THE NAME from the layer being extracted.
# That looks right and does nothing: the victim is in a different layer's
# directory, so the unlink is a no-op, and the file stays visible through
# the lower layer at run time.
#
# The consequence is not subtle. `RUN ... && rm -rf /secret` is one of the
# most common lines in any Dockerfile - it is how build secrets, private
# keys, credentials and package caches are removed - and every one of them
# was readable inside the container.
#
# The image here is built byte by byte and served over loopback rather than
# pulled from a registry, because the properties worth testing hardest are
# about what a layer REMOVES, and you cannot see what a public image's
# layers contain without reimplementing the thing under test.

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
command -v python3 >/dev/null 2>&1 || {
    echo "needs python3: the test image is built byte by byte rather than pulled" >&2
    exit 2
}

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage15.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
REG_PID=
cleanup() {
    [ -n "$REG_PID" ] && kill "$REG_PID" 2>/dev/null
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED_TMPFS" = yes ] && umount "$SCRATCH" 2>/dev/null
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

case "$(uname -m)" in
    x86_64) ARCH=amd64; MARCH=x86_64 ;;
    aarch64) ARCH=arm64; MARCH=aarch64 ;;
    *) echo "unsupported arch" >&2; exit 2 ;;
esac

# A real root filesystem as the bottom layer, so the image has a shell to
# run the assertions with. The same cached tarball the other stages use.
CACHE="${TMPDIR:-/tmp}/rustainer-alpine-$MARCH.tar.gz"
if [ ! -s "$CACHE" ]; then
    ALPINE="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/$MARCH"
    NAME=$(curl -fsS "$ALPINE/" | grep -o "alpine-minirootfs-[0-9.]*-$MARCH\.tar\.gz" | sort -uV | tail -1)
    curl -fsS -o "$CACHE" "$ALPINE/$NAME" || { echo "cannot fetch a base rootfs" >&2; exit 2; }
fi

# A free-ish port. Loopback only, so collisions are the only risk.
PORT=$((20000 + ($$ % 20000)))
FAKE_ARCH=$ARCH python3 scripts/fake-registry.py "$SCRATCH/reg" "$PORT" "$CACHE" > "$SCRATCH/reg.out" 2>&1 &
REG_PID=$!
REF=
for _ in $(seq 50); do
    REF=$(awk '/^ready /{print $2}' "$SCRATCH/reg.out" 2>/dev/null)
    [ -n "$REF" ] && break
    sleep 0.2
done
[ -n "$REF" ] || { echo "the test registry did not start:"; cat "$SCRATCH/reg.out"; exit 2; }
echo "serving $REF"

OUT=$("$B" pull "$REF" 2>&1)
RC=$?
if [ $RC -ne 0 ]; then
    echo "could not pull the hand-built image: $OUT" >&2
    exit 2
fi
echo "$OUT" | grep -qi 'WARNING.*REMAIN VISIBLE' && WHITEOUTS_RECORDABLE=no || WHITEOUTS_RECORDABLE=yes

R() { timeout 60 "$B" run --network none "$REF" "$@" 2>&1 | tail -1; }

echo
echo "1. A file deleted by a later layer is gone"
# The headline. Layer 1 writes /secret, layer 2 whites it out.
if [ "$WHITEOUTS_RECORDABLE" = no ]; then
    skip "this environment forbids creating overlay whiteouts, and the engine said so"
else
    check "a whited-out file cannot be read" \
        "$(R /bin/sh -c 'cat /secret 2>/dev/null || echo absent')" "absent"
    check "and it is not even listed" \
        "$(R /bin/sh -c 'ls /secret >/dev/null 2>&1 && echo present || echo absent')" "absent"
fi

echo
echo "2. A file deleted inside a directory is gone, and its siblings are not"
# The failure mode worth separating: a whiteout that removes too much is as
# wrong as one that removes nothing.
if [ "$WHITEOUTS_RECORDABLE" = no ]; then
    skip "no whiteouts here"
else
    check "the deleted sibling is gone" \
        "$(R /bin/sh -c 'ls /d | tr "\n" " "')" "keep "
fi

echo
echo "3. An opaque directory hides the lower layer's contents, not this layer's"
# `.wh..wh..opq` means "everything beneath this from lower layers is
# hidden". The entries the SAME layer puts there must survive, which is
# what makes "empty the directory" the wrong implementation.
check "the lower layer's entry is hidden" \
    "$(R /bin/sh -c 'ls /opq/old >/dev/null 2>&1 && echo present || echo absent')" "absent"
check "and the same layer's entry survives" \
    "$(R /bin/sh -c 'cat /opq/new')" "new"

echo
echo "4. The markers themselves never appear in the container"
# A `.wh.` file visible inside a container is the other way to get this
# wrong: the deletion is expressed, and the plumbing leaks.
check "no .wh. entries at the root" \
    "$(R /bin/sh -c 'ls -a / | grep -c "^\.wh\." || true')" "0"
check "none inside a directory either" \
    "$(R /bin/sh -c 'ls -a /d /opq | grep -c "^\.wh\." || true')" "0"

echo
echo "5. The whiteout is recorded in the layer, not applied to it"
# Reading the store back: the marker must be a character device 0:0 in the
# UPPER layer's directory, because that is the only form overlayfs
# understands, and the lower layer must be untouched.
LAYERS=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null)
NLAYERS=$(echo "$LAYERS" | grep -c .)
# Three: the base rootfs, the layer that creates, the layer that deletes.
check "each layer is in the store as its own directory" "$NLAYERS" "3"
if [ "$WHITEOUTS_RECORDABLE" = yes ]; then
    FOUND_DEV=0
    FOUND_SRC=0
    for d in $LAYERS; do
        [ -c "$d/secret" ] && FOUND_DEV=1
        [ -f "$d/secret" ] && FOUND_SRC=1
    done
    check "one layer holds the whiteout as a character device" "$FOUND_DEV" "1"
    check "and the other still holds the real file" "$FOUND_SRC" "1"
    # Major 0, minor 0 exactly: the kernel tests for that value, and any
    # other device number is a real device node in a container's rootfs.
    DEV=$(for d in $LAYERS; do [ -c "$d/secret" ] && stat -c '%t:%T' "$d/secret"; done)
    check "the device number is 0:0" "$DEV" "0:0"
else
    skip "whiteouts could not be recorded here"
fi

echo
echo "6. When it cannot be recorded, the engine says exactly what that means"
# Not a debug line. An operator has to be able to learn that files the
# image deleted are readable, from the output of the command they ran.
if [ "$WHITEOUTS_RECORDABLE" = no ]; then
    echo "$OUT" | grep -q 'REMAIN VISIBLE' \
        && ok "the warning names the consequence, not the mechanism" \
        || bad "the warning did not name the consequence"
else
    # Prove the message exists and is specific, without needing to be in an
    # environment that triggers it.
    grep -q 'REMAIN VISIBLE in containers' crates/cli/src/main.rs \
        && ok "the warning text names the consequence" \
        || bad "no warning text names the consequence"
fi

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
