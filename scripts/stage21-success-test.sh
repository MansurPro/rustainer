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
# What is deliberately absent: parallelism. Stages build in file order.

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

# Every build here is `--network none`, and for two reasons. A step
# that cannot reach a registry cannot quietly depend on one, so the
# test says the same thing on every host; and a bridge needs
# CAP_NET_ADMIN in the user namespace that owns the HOST's network
# namespace, which a nested one can never have - so without this the
# whole stage is unrunnable rootless, even though not one RUN step
# below wants the network. The default is still `bridge`, and stage 24
# builds through the API without the flag, which is where that path is
# covered.

echo
echo "1. The file is read before anything is built"
# Every one of these is a line somebody has written, and the cost of
# getting it wrong is an image that is quietly not what the file says.
printf 'FROM %s\nFROB x\n' "$IMG" > "$CTX/Dockerfile"
refuses "an unknown instruction is named" "not a Dockerfile instruction" \
    "$B" build --network none -t x:1 "$CTX"
printf 'FROM %s\nRUN --mount=type=tmpfs,target=/x true\n' "$IMG" > "$CTX/Dockerfile"
refuses "an instruction we do not implement is refused, not skipped" "not implemented" \
    "$B" build --network none -t x:1 "$CTX"
printf 'RUN true\n' > "$CTX/Dockerfile"
refuses "a file with no FROM is refused" "must begin with FROM" \
    "$B" build --network none -t x:1 "$CTX"
printf 'FROM %s\nCMD ["sh", "-c", "echo \\$X"]\n' "$IMG" > "$CTX/Dockerfile"
refuses "a bracketed line that is not JSON is refused" "exec form" \
    "$B" build --network none -t x:1 "$CTX"
printf 'FROM %s\nCOPY ../secret.txt /s\n' "$IMG" > "$CTX/Dockerfile"
refuses "COPY from outside the context is refused" "build context" \
    "$B" build --network none -t x:1 "$CTX"
printf 'FROM %s\nRUN exit 4\n' "$IMG" > "$CTX/Dockerfile"
refuses "a failing step fails the build, with its number and status" "exit status 4" \
    "$B" build --network none -t x:1 "$CTX"
# The same Dockerfile, so the only thing that differs is the flag: the
# step never runs, because the network is resolved before it does.
refuses "a network that does not exist is refused, exactly as it is for run" "network create" \
    "$B" build --network ghost -t x:1 "$CTX"

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
OUT=$(timeout 600 "$B" build --network none -t demo:1 "$CTX" 2>&1)
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
timeout 600 "$B" build --network none -t demo:2 --build-arg WHO=operator "$CTX" >/dev/null 2>&1
check "the argument reached the RUN step" \
    "$(timeout 120 "$B" run --network none demo:2 2>&1 | tail -1)" "hello operator"
timeout 600 "$B" build --network none -t demo:3 "$CTX" >/dev/null 2>&1
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
timeout 600 "$B" build --network none -t rep:1 "$CTX" >/dev/null 2>&1
BEFORE=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
# A second later, on purpose. A tar header holds whole seconds, so two
# builds inside one second agree by accident - which is how a layer that
# recorded the time of the copy passed this check on a fast machine.
sleep 1.1
timeout 600 "$B" build --network none -t rep:2 "$CTX" >/dev/null 2>&1
AFTER=$(find "$RUSTAINER_IMAGE_ROOT"/layers* -mindepth 1 -maxdepth 1 -type d 2>/dev/null | wc -l)
check "rebuilding adds no new layer" "$AFTER" "$BEFORE"

echo
echo "7b. A step that already ran with the same inputs is not run again"
# Each RUN writes a value nobody could reproduce, so "the step did not
# run" is checked by the value being the same one, not by reading the
# build's own claim that it was cached.
K="$SCRATCH/cachectx"; mkdir -p "$K"
echo "v1" > "$K/input.txt"
kfile() {
    cat > "$K/Dockerfile" <<EOF
FROM $IMG
RUN cat /proc/sys/kernel/random/uuid > /first
COPY input.txt /input.txt
RUN cat /proc/sys/kernel/random/uuid > /second
$1
EOF
}
kval() { timeout 60 "$B" run --network none "$1" /bin/cat "$2" 2>/dev/null | tail -1; }
kfile ""
timeout 600 "$B" build --network none -t kc:1 "$K" >/dev/null 2>&1
F1=$(kval kc:1 /first); S1=$(kval kc:1 /second)
# Without this, every "same value" check below would pass on two empty
# strings - which is how the first version of this section passed.
if [ "${#F1}" -eq 36 ] && [ "${#S1}" -eq 36 ] && [ "$F1" != "$S1" ]; then
    ok "each step wrote a value of its own"
else
    bad "each step wrote a value of its own (got '$F1' and '$S1')"
fi
sleep 1.1   # see section 7: across a second boundary, or it proves nothing
OUT=$(timeout 600 "$B" build --network none -t kc:2 "$K" 2>&1)
contains "the second build says what it reused" "$OUT" "2 step(s) reused"
check "and the first step really did not run again" "$(kval kc:2 /first)" "$F1"
check "nor the second" "$(kval kc:2 /second)" "$S1"

# Change what COPY copies: the step before it is still good, the step
# after it is not.
echo "v2" > "$K/input.txt"
timeout 600 "$B" build --network none -t kc:3 "$K" >/dev/null 2>&1
check "a step before a changed COPY is still reused" "$(kval kc:3 /first)" "$F1"
if [ "$(kval kc:3 /second)" != "$S1" ] && [ -n "$(kval kc:3 /second)" ]; then
    ok "and the step after it runs again"
else
    bad "and the step after it runs again (got '$(kval kc:3 /second)')"
fi
check "with the new file in the image" "$(kval kc:3 /input.txt)" "v2"

# A build that fails late keeps the work it did early.
kfile "RUN exit 7"
timeout 600 "$B" build --network none -t kc:4 "$K" >/dev/null 2>&1
check "a failing step fails the build" "$?" "1"
kfile "RUN echo fixed > /fixed"
OUT=$(timeout 600 "$B" build --network none -t kc:5 "$K" 2>&1)
contains "the steps before a failure are reused once it is fixed" "$OUT" "2 step(s) reused"
check "and the fixed step ran" "$(kval kc:5 /fixed)" "fixed"

timeout 600 "$B" build --no-cache --network none -t kc:6 "$K" >/dev/null 2>&1
if [ "$(kval kc:6 /first)" != "$F1" ] && [ -n "$(kval kc:6 /first)" ]; then
    ok "--no-cache runs every step again"
else
    bad "--no-cache runs every step again (got '$(kval kc:6 /first)')"
fi

echo
echo "7c. What follows the image name is arguments to its ENTRYPOINT"
# Docker's rule, and the one every image with an entrypoint is written
# for: `run image a b` is `<entrypoint> a b`, and CMD is only the default
# arguments. This engine used to replace the entrypoint too.
N="$SCRATCH/epctx"; mkdir -p "$N"
printf 'FROM %s\nENTRYPOINT ["/bin/echo", "entry"]\nCMD ["default-arg"]\n' "$IMG" > "$N/Dockerfile"
timeout 600 "$B" build --network none -t ep:1 "$N" >/dev/null 2>&1
epr() { timeout 60 "$B" run --network none "$@" 2>/dev/null | tail -1; }
check "with no arguments, CMD supplies them" "$(epr ep:1)" "entry default-arg"
check "arguments replace CMD and keep the entrypoint" "$(epr ep:1 one two)" "entry one two"
check "--entrypoint replaces it, and drops CMD with it" "$(epr --entrypoint /bin/pwd ep:1)" "/"
check "an empty --entrypoint makes the arguments the whole command" "$(epr --entrypoint "" ep:1 /bin/echo bare)" "bare"

echo
echo "7d. Stages: FROM ... AS, COPY --from, --target"
# The Dockerfile nearly every real project has: build in one image, ship
# another, and the shipped one holds only what was copied across.
M="$SCRATCH/msctx"; mkdir -p "$M/src/sub" "$M/node_modules/dep" "$M/node_modules/keep"
echo '{"name":"x"}' > "$M/package.json"
echo '{"lock":1}' > "$M/package-lock.json"
echo "main" > "$M/src/main.txt"
echo "deep" > "$M/src/sub/deep.txt"
echo "dep" > "$M/node_modules/dep/index.js"
echo "kept" > "$M/node_modules/keep/k.js"
echo "noise" > "$M/debug.log"
printf 'node_modules\n!node_modules/keep\n*.log\n' > "$M/.dockerignore"
cat > "$M/Dockerfile" <<MSEND
ARG WHO=world
FROM $IMG AS build
ARG WHO
ENV ONLY_IN_BUILD=yes
WORKDIR /app
COPY package*.json ./
COPY . .
WORKDIR out
RUN echo "hello \$WHO" > artefact && echo junk > /app/junk && rm /app/src/main.txt && pwd > /where

FROM build AS tested
RUN echo tested > /app/out/tested

FROM $IMG
WORKDIR /srv
COPY --from=build /app/out/ ./
COPY --from=build /app/src /srv/src
COPY --from=0 /app/package.json /srv/sub/
COPY --from=$IMG /etc/alpine-release /base-release
RUN env | grep -c ONLY_IN_BUILD > /env-leak; true
CMD ["cat", "/srv/artefact"]
MSEND
mrun() { timeout 60 "$B" run --network none "$@" 2>/dev/null; }
OUT=$(timeout 600 "$B" build --network none --build-arg WHO=stages -t ms:1 "$M" 2>&1); RC=$?
check "a Dockerfile with three stages builds" "$RC" "0"
[ "$RC" -eq 0 ] || echo "$OUT" | tail -5
check "the last stage is the image, with what an earlier one made" "$(mrun ms:1)" "hello stages"
check "a COPY of a directory gives its contents" "$(mrun ms:1 cat /srv/src/sub/deep.txt)" "deep"
check "a file an earlier stage deleted is not copied" "$(mrun ms:1 ls /srv/src)" "sub"
check "--from takes a stage's number" "$(mrun ms:1 cat /srv/sub/package.json)" '{"name":"x"}'
check "and an image" "$(mrun ms:1 sh -c 'test -s /base-release && echo yes')" "yes"
check "what the build stage held is not in the image" "$(mrun ms:1 sh -c 'ls /app 2>/dev/null | wc -l')" "0"
check "nor is its environment" "$(mrun ms:1 cat /env-leak)" "0"
contains "a stage the image does not need is not built" "$OUT" "skipped: nothing"
check "a stage that nothing copies from leaves nothing" "$(mrun ms:1 sh -c 'test -e /srv/tested || echo absent')" "absent"

# A COPY --from is reused while its source stage is what it was, and
# done again the moment that stage is not.
OUT=$(timeout 600 "$B" build --network none --build-arg WHO=stages -t ms:1b "$M" 2>&1)
contains "a rebuild reuses every step, the copies between stages too" "$OUT" "6 step(s) reused"
timeout 600 "$B" build --network none --build-arg WHO=again -t ms:2 "$M" >/dev/null 2>&1
check "a copy from a stage that changed is made again" "$(mrun ms:2)" "hello again"
check "and the earlier image is still what it was" "$(mrun ms:1b)" "hello stages"
timeout 600 "$B" build --network none --target build -t ms:build "$M" >/dev/null 2>&1
check "--target stops at a stage and tags it" "$?" "0"
check "a relative WORKDIR is inside the one before" "$(mrun ms:build cat /where)" "/app/out"
check "a wildcard source matches every file it names" "$(mrun ms:build sh -c 'ls /app/package*.json | wc -l')" "2"
check "a relative destination is inside WORKDIR" "$(mrun ms:build cat /app/src/sub/deep.txt)" "deep"
check ".dockerignore keeps a directory out" "$(mrun ms:build sh -c 'test -e /app/node_modules/dep || echo absent')" "absent"
check "and a pattern" "$(mrun ms:build sh -c 'test -e /app/debug.log || echo absent')" "absent"
check "and lets a ! line back in" "$(mrun ms:build cat /app/node_modules/keep/k.js)" "kept"
timeout 600 "$B" build --network none --target tested -t ms:tested "$M" >/dev/null 2>&1
check "a stage built on a stage has both" "$(mrun ms:tested sh -c 'cat /app/out/tested /app/out/artefact' | tr '\n' ' ')" "tested hello world "
refuses "a --target the file does not have is refused" "has no stage" \
    timeout 60 "$B" build --network none --target nope -t ms:x "$M"

# A stage that starts from an image is built at the same time as the
# ones before it, as far as its first COPY --from. The build that asked
# for it then finds those steps done - on a file it has never seen.
NONCE=$RANDOM$RANDOM
cat > "$M/Dockerfile" <<MSEND
FROM $IMG AS build
RUN echo "built $NONCE" > /artefact
FROM $IMG
RUN echo "installed $NONCE" > /installed
COPY --from=build /artefact /artefact
RUN cat /installed /artefact > /both
MSEND
OUT=$(timeout 600 "$B" build --network none -t ms:par "$M" 2>&1); RC=$?
check "a file with a stage to build alongside builds" "$RC" "0"
contains "the later stage's first steps were built alongside, not after" "$OUT" "1 step(s) reused"
check "and the image is what the file says" "$(mrun ms:par cat /both | tr '\n' ' ')" "installed $NONCE built $NONCE "
OUT=$(timeout 600 "$B" build --no-cache --network none -t ms:par2 "$M" 2>&1)
case "$OUT" in *reused*) bad "--no-cache builds every step itself" ;; *) ok "--no-cache builds every step itself" ;; esac

# Two stages that both continue a third do not take turns either: the
# second is started when the one they share is done.
NONCE=$RANDOM$RANDOM
cat > "$M/Dockerfile" <<MSEND
FROM $IMG AS base
RUN echo "base $NONCE" > /base
FROM base AS one
RUN echo "one $NONCE" > /one
FROM base AS two
RUN echo "two $NONCE" > /two
FROM $IMG
COPY --from=one /one /base /
COPY --from=two /two /
MSEND
OUT=$(timeout 600 "$B" build --network none -t ms:sib "$M" 2>&1); RC=$?
check "a file with two stages on one base builds" "$RC" "0"
contains "the second of them was built alongside the first" "$OUT" "1 step(s) reused"
check "and the image has what both made" "$(mrun ms:sib sh -c 'cat /base /one /two' | tr '\n' ' ')" \
    "base $NONCE one $NONCE two $NONCE "

# No base at all: the image is exactly what was copied into it.
cat > "$M/Dockerfile" <<MSEND
FROM $IMG AS build
RUN mkdir /out && cp /bin/busybox /out/busybox
FROM scratch
COPY --from=build /out/busybox /bin/busybox
COPY --from=build /lib/ld-musl-* /lib/
ENTRYPOINT ["/bin/busybox"]
MSEND
timeout 600 "$B" build --network none -t ms:scratch "$M" >/dev/null 2>&1
check "FROM scratch builds" "$?" "0"
check "and runs with only what was copied in" "$(mrun ms:scratch ls / | grep -c -x -e bin -e lib)" "2"

# Whether the engine runs with more than one id: real root, or a user
# with a sub-id range. With one id, owners cannot really change.
MANY_IDS=no
[ "$("$B" unshare awk '{s += $3} END {print (s > 1) ? "yes" : "no"}' /proc/self/uid_map 2>/dev/null)" = yes ] && MANY_IDS=yes

echo
echo "7e. The rest of the language"
R="$SCRATCH/restctx"; mkdir -p "$R/payload/dir"
echo "inside" > "$R/payload/dir/file.txt"
tar -C "$R/payload" -czf "$R/bundle.tar.gz" dir
echo '#!/bin/sh' > "$R/run.sh"; echo 'echo ran' >> "$R/run.sh"; chmod 644 "$R/run.sh"
rfile() {
    { printf 'FROM %s\n' "$IMG"; cat; } > "$R/Dockerfile"
    timeout 600 "$B" build --network none -t "$1" "$R" 2>&1
}
OUT=$(rfile rest:1 <<'RSEND'
RUN adduser -D -u 1234 app
COPY --chown=app:app --chmod=755 run.sh /usr/local/bin/run.sh
COPY --chown=4321 run.sh /numeric.sh
ADD bundle.tar.gz /unpacked/
COPY bundle.tar.gz /copied/
SHELL ["/bin/sh", "-ec"]
RUN false; echo "should not get here" > /shell-ignored-errors || true
RSEND
)
contains "a step run by a SHELL with -e stops at the first failure" "$OUT" "failed with exit status"
OUT=$(rfile rest:1 <<'RSEND'
RUN adduser -D -u 1234 app
COPY --chown=app:app --chmod=755 run.sh /usr/local/bin/run.sh
COPY --chown=4321 run.sh /numeric.sh
ADD bundle.tar.gz /unpacked/
COPY bundle.tar.gz /copied/
RUN --mount=type=cache,target=/cache sh -c 'echo x >> /cache/count; wc -l < /cache/count > /cache-lines'
MAINTAINER someone
VOLUME /data
STOPSIGNAL SIGQUIT
HEALTHCHECK --interval=1s --retries=2 CMD test -e /tmp/well
CMD ["sleep", "300"]
RSEND
); RC=$?
check "a file using the rest of the language builds" "$RC" "0"
[ "$RC" -eq 0 ] || echo "$OUT" | tail -5
rrun() { timeout 60 "$B" run --network none "$@" 2>/dev/null; }
if [ "$MANY_IDS" = yes ]; then
    check "--chown takes names from the image" "$(rrun rest:1 stat -c %u:%g /usr/local/bin/run.sh)" "1234:1234"
    check "and a number alone is both" "$(rrun rest:1 stat -c %u:%g /numeric.sh)" "4321:4321"
else
    skip "--chown needs root"; skip "--chown needs root"
fi
check "--chmod sets the mode" "$(rrun rest:1 stat -c %a /usr/local/bin/run.sh)" "755"
check "ADD unpacks a tar archive" "$(rrun rest:1 cat /unpacked/dir/file.txt)" "inside"
check "COPY does not" "$(rrun rest:1 ls /copied)" "bundle.tar.gz"
check "a cache mount is not in the image" "$(rrun rest:1 sh -c 'test -e /cache/count || echo absent')" "absent"
check "the first build saw it empty" "$(rrun rest:1 cat /cache-lines | tr -d ' ')" "1"
OUT=$(rfile rest:2 <<'RSEND'
RUN --mount=type=cache,target=/cache sh -c 'echo y >> /cache/count; wc -l < /cache/count > /cache-lines'
RSEND
)
check "and a later build finds what it left" "$(rrun rest:2 cat /cache-lines | tr -d ' ')" "2"

# STOPSIGNAL is what `stop` sends, and the image's process decides
# what that means.
OUT=$(rfile rest:sig <<'RSEND'
STOPSIGNAL SIGUSR1
CMD ["/bin/sh", "-c", "trap 'exit 42' USR1; trap '' TERM; while :; do sleep 0.1; done"]
RSEND
)
SG=$("$B" run -d --network none rest:sig 2>/dev/null | tail -1)
sleep 1
timeout 30 "$B" stop --time 5 "$SG" >/dev/null 2>&1
check "stop sends the image's STOPSIGNAL" "$("$B" ps -a | awk -v id="$SG" '$1 == id {print $4}')" "42"
"$B" rm --force "$SG" >/dev/null 2>&1
SG=$("$B" run -d --network none --stop-signal TERM rest:sig 2>/dev/null | tail -1)
sleep 1
timeout 30 "$B" stop --time 1 "$SG" >/dev/null 2>&1
if [ "$("$B" ps -a | awk -v id="$SG" '$1 == id {print $4}')" != 42 ]; then
    ok "--stop-signal replaces it"
else
    bad "--stop-signal replaces it"
fi
"$B" rm --force "$SG" >/dev/null 2>&1

# ONBUILD: an instruction a base image leaves for whoever builds on it.
echo "from the child's context" > "$R/child.txt"
OUT=$(rfile onbase:1 <<'RSEND'
ONBUILD COPY child.txt /from-onbuild.txt
ONBUILD RUN echo ran > /onbuild-ran
RUN test ! -e /onbuild-ran
RSEND
); RC=$?
check "ONBUILD does nothing in the image that declares it" "$RC" "0"
printf 'FROM onbase:1\nRUN cat /onbuild-ran > /seen-by-next-step\n' > "$R/Dockerfile"
timeout 600 "$B" build --network none -t onchild:1 "$R" >/dev/null 2>&1
check "and runs first in a build on that image" "$(rrun onchild:1 cat /seen-by-next-step)" "ran"
check "against that build's context" "$(rrun onchild:1 cat /from-onbuild.txt)" "from the child's context"
printf 'FROM onchild:1\nRUN test ! -e /nothing\n' > "$R/Dockerfile"
OUT=$(timeout 600 "$B" build --network none -t ongrand:1 "$R" 2>&1)
case "$OUT" in *ONBUILD*|*"COPY child.txt"*) bad "and only once: not in a build on the child" ;;
*) ok "and only once: not in a build on the child" ;; esac

# A bind mount lends the step a file from the context without copying it.
echo "lent" > "$R/lent.txt"
OUT=$(rfile rest:bind <<'RSEND'
RUN --mount=type=bind,source=lent.txt,target=/lent.txt cat /lent.txt > /read-it
RSEND
)
check "a bind mount is readable by its step" "$(rrun rest:bind cat /read-it)" "lent"
check "and its content is not in the image" "$(rrun rest:bind sh -c 'cat /lent.txt 2>/dev/null | wc -c' | tr -d ' ')" "0"

# ADD <url> fetches. Served from this machine so the test needs no network.
mkdir -p "$SCRATCH/www"; echo "fetched" > "$SCRATCH/www/file.txt"
( cd "$SCRATCH/www" && exec python3 -m http.server 18765 --bind 127.0.0.1 >/dev/null 2>&1 ) &
WWW=$!
for _ in $(seq 1 40); do (exec 3<>/dev/tcp/127.0.0.1/18765) 2>/dev/null && break; sleep 0.25; done
OUT=$(rfile rest:url <<'RSEND'
ADD http://127.0.0.1:18765/file.txt /dl/
ADD http://127.0.0.1:18765/file.txt /named.txt
RSEND
); RC=$?
kill "$WWW" 2>/dev/null
check "ADD of a URL builds" "$RC" "0"
[ "$RC" -eq 0 ] || echo "$OUT" | tail -3
check "the file is under the URL's name in a directory" "$(rrun rest:url cat /dl/file.txt)" "fetched"
check "or under the name the destination gives" "$(rrun rest:url cat /named.txt)" "fetched"

# VOLUME: a directory of the container's own, outside its layer, that
# starts with what the image has there. And a named volume that is
# still empty starts the same way - both as Docker has them.
OUT=$(rfile rest:vol <<'RSEND'
RUN mkdir -p /data/sub && echo seeded > /data/sub/file && chmod 700 /data/sub
VOLUME /data
RSEND
)
check "an image's VOLUME is a mount of its own" "$(rrun rest:vol sh -c 'grep -c " /data " /proc/mounts')" "1"
check "that starts with the image's files" "$(rrun rest:vol cat /data/sub/file)" "seeded"
check "modes included" "$(rrun rest:vol stat -c %a /data/sub)" "700"
check "and is writable under a read-only root" \
    "$(rrun --read-only rest:vol sh -c 'echo w > /data/w && cat /data/w')" "w"
check "an empty named volume starts with the image's files too" \
    "$(rrun -v r21seed:/data rest:vol cat /data/sub/file)" "seeded"
rrun -v r21seed:/data rest:vol sh -c 'rm -r /data/sub && echo mine > /data/mine' >/dev/null
check "and is left alone once it holds something" \
    "$(rrun -v r21seed:/data rest:vol sh -c 'ls /data | tr "\n" " "')" "mine "
check "a mount over a VOLUME path replaces the image's volume" \
    "$(rrun -v r21seed:/data rest:vol sh -c 'grep -c " /data " /proc/mounts')" "1"
"$B" volume rm r21seed >/dev/null 2>&1
VC=$("$B" run -d --network none rest:vol sleep 300 2>/dev/null | tail -1)
check "the volume is kept beside the container" "$(ls -d "$RUSTAINER_IMAGE_ROOT/containers/$VC/volumes" 2>/dev/null | wc -l | tr -d ' ')" "1"
"$B" rm --force "$VC" >/dev/null 2>&1
check "and goes when the container does" "$(ls -d "$RUSTAINER_IMAGE_ROOT/containers/$VC" 2>/dev/null | wc -l | tr -d ' ')" "0"

# A bind mount from another stage, and the caller's SSH agent.
{ printf 'FROM %s AS tools\nRUN mkdir /kit && echo wrench > /kit/tool\n' "$IMG"
  printf 'FROM %s\n' "$IMG"
  cat <<'RSEND'
RUN --mount=type=bind,from=tools,source=/kit,target=/kit cat /kit/tool > /used
RUN --mount=type=ssh sh -c 'if [ -S "${SSH_AUTH_SOCK:-/none}" ]; then echo agent; else echo none; fi > /agent'
RSEND
} > "$R/Dockerfile"
python3 -c 'import socket,sys; socket.socket(socket.AF_UNIX).bind(sys.argv[1])' "$SCRATCH/agent.sock"
SSH_AUTH_SOCK="$SCRATCH/agent.sock" timeout 600 "$B" build --network none -t rest:noagent "$R" >/dev/null 2>&1
check "a bind mount from a stage lends that stage's files" "$(rrun rest:noagent cat /used)" "wrench"
check "and leaves nothing of them in the image" "$(rrun rest:noagent sh -c 'ls /kit 2>/dev/null | wc -l' | tr -d ' ')" "0"
check "a step that asks for the SSH agent does not get it by asking" "$(rrun rest:noagent cat /agent)" "none"
SSH_AUTH_SOCK="$SCRATCH/agent.sock" timeout 600 "$B" build --no-cache --ssh default --network none -t rest:agent "$R" >/dev/null 2>&1
check "with --ssh the step can reach the agent" "$(rrun rest:agent cat /agent)" "agent"

# --gpus gives the machine's GPU nodes, or says there are none.
if [ -d /dev/dri ] || ls /dev/nvidia* /dev/kfd >/dev/null 2>&1; then
    check "--gpus all makes the GPU nodes visible" \
        "$(rrun --gpus all "$IMG" sh -c 'ls -d /dev/dri /dev/nvidia* /dev/kfd 2>/dev/null | wc -l | tr -d " " | sed "s/^[1-9][0-9]*$/some/"')" "some"
else
    refuses "--gpus on a machine with no GPU says so" "no GPU device nodes" \
        "$B" run --network none --gpus all "$IMG" true
fi

# What a `$` in a command means is the shell's business, at the time it
# runs: the build puts ARG and ENV in the step's environment and leaves
# the text alone. Substituting them itself used to empty every variable
# a step set for itself.
OUT=$(rfile rest:shell <<'RSEND'
ARG GREETING=hello
ENV PLACE=world
RUN for w in a b c; do printf '%s' "$w"; done > /loop
RUN X=local && echo "$X ${UNSET:-fallback} $GREETING $PLACE" > /vars
CMD echo "$HOSTNAME-$PLACE-${GREETING:-arg-is-not-in-the-image}"
RSEND
)
check "a variable a step sets for itself survives" "$(rrun rest:shell cat /loop)" "abc"
check "ARG and ENV reach the step as environment" "$(rrun rest:shell cat /vars)" "local fallback hello world"
check "a CMD's variables are the container's, read when it runs" \
    "$(rrun --hostname box rest:shell)" "box-world-arg-is-not-in-the-image"

# A package manager changes owners and switches user as it goes. Root
# does both for real. A user with one id can do neither, so in a build
# those calls are answered "done": the step succeeds, and the files are
# owned by the one user there is - as every pulled layer already is.
OUT=$(rfile rest:own <<'RSEND'
RUN adduser -D -u 1234 app && touch /owned && chown 1234:1234 /owned && chown -R app /home/app && echo done > /finished
RSEND
); RC=$?
check "a step that changes owners builds, root or not" "$RC" "0"
[ "$RC" -eq 0 ] || echo "$OUT" | tail -3
check "and ran to its end" "$(rrun rest:own cat /finished)" "done"
if [ "$MANY_IDS" = yes ]; then
    check "as root the owner really changed" "$(rrun rest:own stat -c %u /owned)" "1234"
else
    check "without privilege the file is the builder's" "$(rrun rest:own stat -c %u /owned)" "0"
fi
case "$(rrun rest:own sh -c 'chown 1234 /tmp 2>&1; echo rc=$?')" in
*"rc=0"*) [ "$MANY_IDS" = yes ] \
    && ok "outside a build, changing owner is the real call" \
    || bad "outside a build, a chown that cannot work must say so, not pretend" ;;
*) ok "outside a build, a chown that cannot work says so" ;;
esac

# apt checks that it really became its own user, so for a builder with
# one id it is lent its "run as root" setting for the step. Lent: the
# image does not keep it.
OUT=$(rfile rest:apt <<'RSEND'
RUN mkdir -p /etc/apt/apt.conf.d
RUN cat /etc/apt/apt.conf.d/* 2>/dev/null > /apt-saw; true
RSEND
)
if [ "$MANY_IDS" = yes ]; then
    check "root is not told to run apt as root" "$(rrun rest:apt cat /apt-saw)" ""
else
    contains "a builder with one id has apt told to stay root" "$(rrun rest:apt cat /apt-saw)" 'Sandbox::User "root"'
fi
check "and the image is left without the setting" "$(rrun rest:apt sh -c 'ls /etc/apt/apt.conf.d | wc -l' | tr -d ' ')" "0"

# An image that names a user still runs for a caller who can only be
# one: as that one, and saying so. Asking on the command line is refused.
OUT=$(rfile rest:user <<'RSEND'
RUN adduser -D -u 4321 app
USER app
CMD ["id", "-u"]
RSEND
)
if [ "$MANY_IDS" = yes ]; then
    check "an image's USER is who the container runs as" "$(rrun rest:user)" "4321"
else
    check "with one id, an image's USER runs as that one" "$(rrun rest:user)" "0"
    contains "and says that it did" "$(timeout 60 "$B" run --network none rest:user 2>&1)" "has one id"
    refuses "a --user that cannot be is still refused" "not mapped" \
        timeout 60 "$B" run --network none --user 4321 rest:user
fi

# A secret is a file one step can read and no layer holds.
echo "s3cr3t-from-a-file" > "$SCRATCH/tok.txt"
{ printf 'FROM %s\n' "$IMG"; cat <<'RSEND'
ENV A=1 B="two words" \
    C=3
RUN --mount=type=secret,id=tok --mount=type=secret,id=envtok,target=/etc/envtok \
    sh -c 'cat /run/secrets/tok /etc/envtok | grep -c s3cr3t > /secret-lines; echo "$A/$B/$C" > /env-pairs'
RUN --mount=type=secret,id=absent sh -c 'test -e /run/secrets/absent || echo none > /absent'
RUN --mount=type=secret,id=envtok,env=AS_A_VARIABLE sh -c 'echo "${AS_A_VARIABLE}" | grep -c s3cr3t > /secret-as-env; echo "${NOT_GIVEN:-unset}" > /secret-env-absent'
RSEND
} > "$R/Dockerfile"
OUT=$(MY_TOKEN="s3cr3t-from-the-environment" timeout 600 "$B" build --network none \
    --secret "id=tok,src=$SCRATCH/tok.txt" --secret id=envtok,env=MY_TOKEN -t rest:3 "$R" 2>&1); RC=$?
check "a build given secrets builds" "$RC" "0"
[ "$RC" -eq 0 ] || echo "$OUT" | tail -5
check "the step could read both, from a file and from the environment" "$(rrun rest:3 cat /secret-lines | tr -d ' ')" "2"
check "and the image holds neither" \
    "$(rrun rest:3 sh -c 'cat /run/secrets/tok /etc/envtok 2>/dev/null | wc -c' | tr -d ' ')" "0"
if grep -rqs -e s3cr3t-from "$RUSTAINER_IMAGE_ROOT" 2>/dev/null; then
    bad "nor does anything in the image store"
else
    ok "nor does anything in the image store"
fi
check "a secret asked for as a variable is one, for that step" "$(rrun rest:3 cat /secret-as-env)" "1"
check "and is not in the image's environment" "$(rrun rest:3 sh -c 'echo "${AS_A_VARIABLE:-gone}"')" "gone"
check "a secret the build was not given is simply absent" "$(rrun rest:3 cat /absent)" "none"
check "one ENV sets several variables" "$(rrun rest:3 cat /env-pairs)" "1/two words/3"
check "and each is in the image's environment" "$(rrun rest:3 sh -c 'echo "$B"')" "two words"
contains "--check reads a file and builds nothing" \
    "$("$B" build --check "$R" 2>&1)" "nothing this engine refuses"
printf 'FROM %s\nRUN --mount=type=tmpfs,target=/x true\n' "$IMG" > "$R/Dockerfile"
refuses "and says what is wrong with one" "not implemented" "$B" build --check "$R"
refuses "a build with no name is refused" "needs a name" "$B" build "$R"

# The image's own healthcheck runs without being asked for.
HC=$("$B" run -d --network none rest:1 2>/dev/null | tail -1)
health() { cat "$RUSTAINER_ROOT/containers/$1/health" 2>/dev/null || cat "$RUSTAINER_ROOT/$1/health" 2>/dev/null; }
# The check is `exec` in the container, and a shell that is itself inside
# a user namespace cannot enter the container's: nothing to check with.
if "$B" exec "$HC" true >/dev/null 2>&1; then
    for _ in $(seq 1 40); do [ "$(health "$HC")" = unhealthy ] && break; sleep 0.25; done
    check "a container from an image with a HEALTHCHECK is checked" "$(health "$HC")" "unhealthy"
    "$B" exec "$HC" touch /tmp/well >/dev/null 2>&1
    for _ in $(seq 1 40); do [ "$(health "$HC")" = healthy ] && break; sleep 0.25; done
    check "and is healthy once the check passes" "$(health "$HC")" "healthy"
    contains "ps says so" "$("$B" ps)" "healthy"
    "$B" rm --force "$HC" >/dev/null 2>&1
    HC=$("$B" run -d --network none --no-healthcheck rest:1 2>/dev/null | tail -1)
    sleep 1.5
    check "--no-healthcheck leaves it unchecked" "$(health "$HC")" ""
    "$B" rm --force "$HC" >/dev/null 2>&1
    HC=$("$B" run -d --network none --health-cmd true --health-interval 1 "$IMG" sleep 300 2>/dev/null | tail -1)
    for _ in $(seq 1 40); do [ "$(health "$HC")" = healthy ] && break; sleep 0.25; done
    check "--health-cmd gives any container one" "$(health "$HC")" "healthy"
    "$B" rm --force "$HC" >/dev/null 2>&1
    # The process doing the checking goes when what it checks does, and
    # not at its next check: that one is five minutes away.
    HC=$("$B" run -d --network none --health-cmd true --health-interval 300 "$IMG" sleep 300 2>/dev/null | tail -1)
    sleep 1
    WATCHER=$(pgrep -f "health-watch $HC" | head -1)
    "$B" rm --force "$HC" >/dev/null 2>&1
    for _ in $(seq 1 16); do kill -0 "$WATCHER" 2>/dev/null || break; sleep 0.25; done
    if [ -n "$WATCHER" ] && ! kill -0 "$WATCHER" 2>/dev/null; then
        ok "the watcher leaves with its container, not at its next check"
    else
        bad "the watcher leaves with its container, not at its next check (pid '$WATCHER')"
    fi
else
    "$B" rm --force "$HC" >/dev/null 2>&1
    for _ in 1 2 3 4 5; do skip "healthchecks need exec, which this shell cannot do"; done
fi

echo
echo "8. The built image is an ordinary image"
contains "images lists it" "$("$B" images)" "demo_1"
timeout 60 "$B" rmi demo:3 >/dev/null 2>&1 \
    && ok "rmi removes it" || bad "rmi failed on a built image"
contains "the build is in the event log" "$("$B" events --since 1800 --limit 4000)" "build"
# A build history is opened to find out what went wrong, so the builds
# that failed have to be in it, with why.
EV=$("$B" events --since 1800 --limit 4000)
contains "with how long it took and what it reused" "$EV" '"reused":2'
contains "a failed build is recorded too" "$EV" "build-failed"
contains "with the reason" "$EV" "failed with exit status 7"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
