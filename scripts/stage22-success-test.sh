#!/bin/bash
# Stage 22 success test - pushing an image to a registry.
#
#   sudo bash scripts/stage22-success-test.sh
#
# RND.md §19 predicted this would be "strictly additive; shares the auth
# and transport code that pull already needs", and it was: three requests
# and a credential header on the token exchange.
#
# The ordering is the one thing that is not negotiable. BLOBS FIRST,
# MANIFEST LAST - a manifest naming blobs the registry does not have is a
# tag that resolves to a broken image, and not every registry checks.
# scripts/fake-registry.py does check, so getting it backwards fails here
# rather than six months later against somebody's Harbor.
#
# The test is a ROUND TRIP: build, push, pull into a clean store, run.
# Anything less proves the request succeeded, not that the right bytes
# arrived - and the first version of this code pushed an image whose
# manifest omitted the base layers, which passed every "did it 201?"
# check and produced a diff_id mismatch on the way back.

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
command -v python3 >/dev/null 2>&1 || { echo "needs python3" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage22.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=4g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
REG_PID=
cleanup() {
    [ -n "$REG_PID" ] && kill "$REG_PID" 2>/dev/null
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    if [ "$MOUNTED_TMPFS" = yes ]; then
        for _ in 1 2 3 4 5 6; do umount "$SCRATCH" 2>/dev/null && break; sleep 0.5; done
    fi
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
echo "kernel $(uname -r) $(uname -m), uid $(id -u)"

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

PORT=$((23000 + ($$ % 12000)))
python3 scripts/fake-registry.py "$SCRATCH/reg" "$PORT" > "$SCRATCH/reg.out" 2>&1 &
REG_PID=$!
for _ in $(seq 60); do grep -q '^ready ' "$SCRATCH/reg.out" 2>/dev/null && break; sleep 0.2; done
grep -q '^ready ' "$SCRATCH/reg.out" || { echo "the test registry did not start"; cat "$SCRATCH/reg.out"; exit 2; }
REG="127.0.0.1:$PORT"

CTX="$SCRATCH/ctx"
mkdir -p "$CTX"
echo "round-tripped" > "$CTX/payload.txt"
cat > "$CTX/Dockerfile" <<EOF
FROM $IMG
COPY payload.txt /payload.txt
ENV PUSHED=yes
CMD ["/bin/sh", "-c", "cat /payload.txt; echo \$PUSHED"]
EOF
timeout 600 "$B" build -t local/pushable:1 "$CTX" >/dev/null 2>&1 \
    || { echo "the build failed" >&2; exit 2; }

echo
echo "1. Refusals before any bytes move"
refuses "an image that is not in the store" "no image" \
    "$B" push nosuchimage:1 --to "$REG/x/y:1"
refuses "a reference that cannot be parsed" "" \
    "$B" push local/pushable:1 --to "$REG/BAD NAME:1"

echo
echo "2. A push uploads every blob the manifest names"
OUT=$(timeout 600 "$B" push local/pushable:1 --to "$REG/team/app:v1" 2>&1)
RC=$?
[ $RC -eq 0 ] && ok "the push succeeded" || bad "the push failed: $(echo "$OUT" | tail -1)"
UPLOADED=$(echo "$OUT" | grep -c '^uploaded ')
# Alpine's one layer, the layer COPY made, and the config.
check "three blobs were uploaded" "$UPLOADED" "3"
contains "and then the manifest" "$OUT" "uploading the manifest"
# Order matters: the registry refuses a manifest naming blobs it lacks.
contains "the tag is reported" "$OUT" "pushed $REG/team/app:v1"

echo
echo "3. The round trip: pull it into a CLEAN store and run it"
# The assertion that matters. The first version of this code pushed an
# image whose manifest omitted the base layers, which satisfied every
# "did it 201?" check and failed only here.
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images2"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
RAN=$(timeout 600 "$B" run --network none "$REG/team/app:v1" 2>&1)
contains "the file we copied in came back" "$RAN" "round-tripped"
contains "and so did the environment" "$RAN" "yes"
check "the base image's own files came back too" \
    "$(timeout 120 "$B" run --network none "$REG/team/app:v1" /bin/sh -c 'test -x /bin/busybox && echo yes' 2>&1 | tail -1)" \
    "yes"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"

echo
echo "4. A second push skips the blobs already there"
# Every layer but one is already on the registry for any image built
# FROM something, so a push that re-uploads them takes minutes instead
# of seconds.
OUT2=$(timeout 600 "$B" push local/pushable:1 --to "$REG/team/app:v2" 2>&1)
check "nothing was uploaded the second time" "$(echo "$OUT2" | grep -c '^uploaded ')" "0"
check "all three were reported as already present" \
    "$(echo "$OUT2" | grep -c 'is already there')" "3"
contains "the manifest still goes, because the tag is new" "$OUT2" "uploading the manifest"

echo
echo "5. The registry refuses a manifest naming blobs it does not have"
# Asserted on the test registry rather than on us: it is what makes
# "blobs first, manifest last" a property this test can prove rather
# than a comment.
BOGUS='{"schemaVersion":2,"mediaType":"application/vnd.oci.image.manifest.v1+json","config":{"mediaType":"application/vnd.oci.image.config.v1+json","digest":"sha256:0000000000000000000000000000000000000000000000000000000000000000","size":1},"layers":[]}'
CODE=$(printf '%s' "$BOGUS" | curl -sS -o /dev/null -w '%{http_code}' -X PUT \
    -H 'Content-Type: application/vnd.oci.image.manifest.v1+json' \
    --data-binary @- "http://$REG/v2/team/app/manifests/bogus" 2>/dev/null)
check "a manifest with a missing blob is rejected" "$CODE" "400"

echo
echo "6. A pulled image can be pushed on, not only a built one"
# Both keep a manifest in the store now; before this stage neither did,
# and push had nothing to work from.
timeout 600 "$B" push "$IMG" --to "$REG/mirror/alpine:3.21" >/dev/null 2>&1 \
    && ok "an image that was pulled can be pushed elsewhere" \
    || bad "pushing a pulled image failed"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images3"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
check "and it runs when pulled back" \
    "$(timeout 600 "$B" run --network none "$REG/mirror/alpine:3.21" /bin/echo mirrored 2>&1 | tail -1)" \
    "mirrored"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"

echo
echo "7. The push is in the event log"
contains "a push is recorded" "$("$B" events --since 900 --limit 400)" "push"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
