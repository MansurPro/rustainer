#!/bin/bash
# Stage 21 success test - building an image from a Dockerfile.
#
#   sudo bash scripts/stage21-success-test.sh
#
# The shape is the whole trick: a build step that changes the filesystem
# is A CONTAINER RUN ON THE LAYERS SO FAR, and the container's own
# overlay upper directory is the new layer. Nothing has to diff anything
# - overlayfs already wrote down exactly what changed, including the
# deletions, which it records as the same `0:0` character devices this
# engine's layer store has used since stage 15.
#
# So the build inherits the engine's isolation, cgroups, networking and
# security posture for free, because it is not a second implementation of
# any of them. The test checks that claim from both ends: the built image
# runs, and the layer in the store is the shape a layer is supposed to be.
#
# What is deliberately absent: no parallelism, no instruction-keyed build
# cache, no multi-stage `--from`. BuildKit is those three things and they
# are each a project; a cache that is wrong is worse than no cache.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage21.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=4g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
# Seed from a shared store when one is offered, so a build test is not
# also a Docker Hub quota test.
if [ -n "${RUSTAINER_COMPAT_STORE:-}" ] && [ -d "$RUSTAINER_COMPAT_STORE" ]; then
    cp -r "$RUSTAINER_COMPAT_STORE"/. "$RUSTAINER_IMAGE_ROOT"/ 2>/dev/null || true
fi
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

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

CTX="$SCRATCH/ctx"
mkdir -p "$CTX/app"
echo "from-the-context" > "$CTX/greeting.txt"
echo "second" > "$CTX/other.txt"
printf '#!/bin/sh\necho app-ran\n' > "$CTX/app/run.sh"
chmod +x "$CTX/app/run.sh"
echo "outside" > "$SCRATCH/secret.txt"

echo
echo "1. The file is read before anything is built"
# Every one of these is a line somebody has written, and the cost of
# getting it wrong is an image that is quietly not what the file says.
printf 'FROM %s\nFROB x\n' "$IMG" > "$CTX/Dockerfile"
refuses "an unknown instruction is named" "not a Dockerfile instruction" \
    "$B" build -t x:1 "$CTX"
printf 'FROM %s\nHEALTHCHECK CMD true\n' "$IMG" > "$CTX/Dockerfile"
refuses "an instruction we do not implement is refused, not skipped" "not implemented" \
    "$B" build -t x:1 "$CTX"
printf 'RUN true\n' > "$CTX/Dockerfile"
refuses "a file with no FROM is refused" "must begin with FROM" \
    "$B" build -t x:1 "$CTX"
printf 'FROM %s\nCMD ["sh", "-c", "echo \\$X"]\n' "$IMG" > "$CTX/Dockerfile"
refuses "a bracketed line that is not JSON is refused" "exec form" \
    "$B" build -t x:1 "$CTX"
printf 'FROM %s\nCOPY ../secret.txt /s\n' "$IMG" > "$CTX/Dockerfile"
refuses "COPY from outside the context is refused" "build context" \
    "$B" build -t x:1 "$CTX"
printf 'FROM %s\nRUN exit 4\n' "$IMG" > "$CTX/Dockerfile"
refuses "a failing step fails the build, with its number and status" "exit status 4" \
    "$B" build -t x:1 "$CTX"

echo
echo "2. A real build produces a runnable image"
cat > "$CTX/Dockerfile" <<EOF
ARG BASE=$IMG
FROM \$BASE
ENV GREETING=configured
WORKDIR /opt/app
COPY greeting.txt other.txt /opt/app/
COPY app /opt/app/app
RUN echo built-here > /opt/app/stamp.txt && rm -f /etc/motd
LABEL org.example.kind=demo
CMD ["/bin/sh", "-c", "cat /opt/app/greeting.txt /opt/app/stamp.txt; /opt/app/app/run.sh; echo \$GREETING; pwd"]
EOF
OUT=$(timeout 600 "$B" build -t demo:1 "$CTX" 2>&1)
RC=$?
if [ $RC -ne 0 ]; then
    echo "the build failed:"; echo "$OUT" | tail -5; exit 2
fi
contains "it reports the tag" "$OUT" "tagged demo:1"
RAN=$(timeout 120 "$B" run --network none demo:1 2>&1)
contains "a COPY'd file is in the image" "$RAN" "from-the-context"
contains "a RUN's output is in the image" "$RAN" "built-here"
contains "a COPY'd directory is there and executable" "$RAN" "app-ran"
contains "ENV reaches the container" "$RAN" "configured"
contains "WORKDIR is where it starts" "$RAN" "/opt/app"
check "a second COPY source landed too" \
    "$(timeout 120 "$B" run --network none demo:1 /bin/cat /opt/app/other.txt 2>&1 | tail -1)" \
    "second"

echo
echo "3. A deletion in a RUN step becomes a whiteout in the built layer"
# The property stage 15 established, now produced by the build rather
# than consumed from a registry: overlayfs wrote the marker, and the
# layer is that directory.
check "the deleted file is gone from the image" \
    "$(timeout 120 "$B" run --network none demo:1 /bin/sh -c 'ls /etc/motd >/dev/null 2>&1 && echo present || echo absent' 2>&1 | tail -1)" \
    "absent"
WH=0
for d in "$RUSTAINER_IMAGE_ROOT"/layers*/*/; do
    [ -c "$d/etc/motd" ] && WH=1
done
check "and the layer records it as a character device" "$WH" "1"

echo
echo "4. The base image's layers are shared, not copied"
# Both are keyed by the hash of their uncompressed tar, so a `FROM
# alpine` build adds only the layers it actually created.
BASE_LAYERS=$(timeout 120 "$B" run --network none "$IMG" /bin/true >/dev/null 2>&1; \
    find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
# alpine is one layer; the build added three (two COPY, one RUN).
[ "${BASE_LAYERS:-0}" -ge 4 ] \
    && ok "the store holds the base layer plus the built ones ($BASE_LAYERS)" \
    || bad "unexpected layer count ($BASE_LAYERS)"

echo
echo "5. The image's config is what the file said"
CFG=$(find "$RUSTAINER_IMAGE_ROOT/images" -name config.json -newer "$CTX/Dockerfile" | head -1)
if [ -n "$CFG" ] && command -v python3 >/dev/null 2>&1; then
    F() { python3 -c "import json,sys;d=json.load(open(sys.argv[1]))['config'];print(json.dumps(d.get(sys.argv[2])))" "$CFG" "$1"; }
    contains "the label is recorded" "$(F Labels)" "org.example.kind"
    contains "the working directory is recorded" "$(F WorkingDir)" "/opt/app"
    contains "the environment is recorded" "$(F Env)" "GREETING=configured"
    contains "the command is recorded" "$(F Cmd)" "/bin/sh"
else
    skip "no config to inspect"
fi

echo
echo "6. --build-arg overrides the file's default"
cat > "$CTX/Dockerfile" <<EOF
ARG BASE=$IMG
ARG WHO=nobody
FROM \$BASE
RUN echo "hello \$WHO" > /who.txt
CMD ["/bin/cat", "/who.txt"]
EOF
timeout 600 "$B" build -t demo:2 --build-arg WHO=operator "$CTX" >/dev/null 2>&1
check "the argument reached the RUN step" \
    "$(timeout 120 "$B" run --network none demo:2 2>&1 | tail -1)" "hello operator"
timeout 600 "$B" build -t demo:3 "$CTX" >/dev/null 2>&1
check "and the default applies when it is not given" \
    "$(timeout 120 "$B" run --network none demo:3 2>&1 | tail -1)" "hello nobody"

echo
echo "7. The same context builds to the same layer, twice"
# The diff_id is the hash of the layer's tar, so an unstable readdir
# order or a live mtime would give the same filesystem two identities
# and defeat sharing entirely.
cat > "$CTX/Dockerfile" <<EOF
FROM $IMG
COPY greeting.txt /g.txt
EOF
timeout 600 "$B" build -t rep:1 "$CTX" >/dev/null 2>&1
BEFORE=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
timeout 600 "$B" build -t rep:2 "$CTX" >/dev/null 2>&1
AFTER=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
check "rebuilding adds no new layer" "$AFTER" "$BEFORE"

echo
echo "8. The built image is an ordinary image"
contains "images lists it" "$("$B" images)" "demo_1"
timeout 60 "$B" rmi demo:3 >/dev/null 2>&1 \
    && ok "rmi removes it" || bad "rmi failed on a built image"
contains "the build is in the event log" "$("$B" events --since 900 --limit 400)" "build"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
