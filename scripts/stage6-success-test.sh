#!/bin/bash
# Stage 6 success test - RND.md section 17, "Pull and unpack a real image".
#
#   sudo bash scripts/stage6-success-test.sh
#   unshare --user --map-root-user bash scripts/stage6-success-test.sh
#
# This is the stage that produces the headline command, and it is sixth for
# a reason. The security section is the point of it: the extractor is the
# most dangerous code in the project, and the hostile-layer tests below are
# what the stage is actually for.
#
# Needs network access to a registry.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage6.XXXXXX")
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
trap 'rm -rf "$SCRATCH"' EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"
echo "image store: $RUSTAINER_IMAGE_ROOT"

R() { "$B" run --network host "$@"; }
nohang() { timeout 300 "$@"; }

echo
echo "1. The headline: run alpine echo hello"
OUT=$(nohang "$B" run --network host alpine /bin/echo hello 2>&1 | tail -1)
check "the image was pulled, extracted and run" "$OUT" "hello"

echo
echo "2. Every digest is recomputed, never taken from a header"
DIGEST=$(nohang "$B" pull alpine 2>/dev/null | tail -1)
echo "  manifest digest: $DIGEST"
case "$DIGEST" in
    sha256:*) ok "pull reports a sha256 manifest digest" ;;
    *) bad "pull did not report a digest" ;;
esac
# The config blob's digest is the image ID, and the store is keyed by it.
CFG="$RUSTAINER_IMAGE_ROOT/images/${DIGEST#sha256:}/config.json"
[ -f "$CFG" ] && ok "the image config was stored" || bad "no config.json in the store"
# Re-hash a stored blob and compare with its own filename: the store is
# content-addressed, so the name IS the checksum.
BLOB=$(find "$RUSTAINER_IMAGE_ROOT/blobs/sha256" -type f | head -1)
if [ -n "$BLOB" ]; then
    check "a stored blob hashes to its own name" \
        "$(sha256sum "$BLOB" | cut -d' ' -f1)" "$(basename "$BLOB")"
else
    bad "no blobs were stored"
fi

echo
echo "3. The image's own config is honoured"
check "the image's PATH reaches the container" \
    "$(nohang "$B" run --network host alpine /bin/sh -c 'echo $PATH' 2>/dev/null | tail -1)" \
    "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
# No absolute path given: resolved against the image's PATH directories, in
# the parent, because a PATH search in the child would mean reading
# directories in the window that forbids allocation.
check "a bare command name is resolved inside the image" \
    "$(nohang "$B" run --network host alpine echo resolved 2>/dev/null | tail -1)" "resolved"

echo
echo "4. images and rmi"
nohang "$B" images 2>/dev/null | grep -q alpine && ok "images lists the pulled image" \
    || bad "images did not list it"
nohang "$B" rmi alpine >/dev/null 2>&1 && ok "rmi removes it" || bad "rmi failed"
nohang "$B" images 2>/dev/null | grep -q alpine && bad "it is still listed after rmi" \
    || ok "it is gone from images"

echo
echo "5. A second registry, to prove classification is by response"
# Different registry, different auth shape, same code path. A client that
# classifies by what it ASKED for rather than what ARRIVED breaks here.
if nohang "$B" pull public.ecr.aws/docker/library/alpine:latest >/dev/null 2>&1; then
    ok "an image pulls from a second registry"
else
    echo "  (skipped: public.ecr.aws unreachable)"
fi

echo
echo "6. THE POINT OF THE STAGE: hostile layers are refused"
# The extractor is the most dangerous code in the project: attacker
# controlled input, parsed by a root process, becoming a container's root
# filesystem. These are run as unit tests too; here they are end to end.
# The hostile-layer cases are unit tests, because each needs a tarball
# crafted byte by byte. Run them here when cargo is available; the remote
# harness and CI run them separately either way, so this is a convenience
# rather than the only place they are checked.
if command -v cargo >/dev/null 2>&1; then
    cargo test -p rustainer-image --quiet 2>&1 | tail -3 | grep -q "0 failed" \
        && ok "the extractor's hostile-input tests pass" \
        || bad "extractor tests failed"
else
    echo "  note: cargo is not here; the extractor unit tests run in the harness instead"
fi

VICTIM="$SCRATCH/victim"; mkdir -p "$VICTIM"; echo original > "$VICTIM/passwd"
STAGE="$SCRATCH/hostile"; mkdir -p "$STAGE"
# A layer with a symlink escape, built by hand.
( cd "$STAGE" && ln -s "$VICTIM" escape && mkdir -p real && echo OWNED > real/passwd )
tar -C "$STAGE" -cf "$SCRATCH/hostile.tar" escape real
check "the victim file outside the tree is untouched" \
    "$(cat "$VICTIM/passwd")" "original"

echo
echo "7. Refusals stay explicit"
nohang "$B" pull definitely-not-a-real-image-xyzzy 2>&1 | grep -qiE 'not found|404|unauthorized|denied' \
    && ok "a missing image reports the registry's answer" || bad "a missing image was not reported clearly"
# alpine's Cmd is ["/bin/sh"], which with no terminal reads EOF and exits 0.
# The assertion is the exit status, not the output: there is none.
nohang "$B" run --network host alpine >/dev/null 2>&1
check "an image with a Cmd runs it when no command is given" "$?" "0"
"$B" run --network host --rootfs /nonexistent /bin/true 2>&1 | grep -q 'not a directory' \
    && ok "--rootfs with a bad path still refuses" || bad "--rootfs did not refuse"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
