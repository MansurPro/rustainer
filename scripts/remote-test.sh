#!/bin/bash
# Build a static binary and run the stage success tests on a remote Linux box,
# without installing a Rust toolchain there.
#
#   scripts/remote-test.sh <ssh-host> [stage ...]
#   scripts/remote-test.sh my-linux-box 1 2
#
# Why this exists: the development machine is often not Linux, and the useful
# test hosts are often shared machines where installing a toolchain, or
# running as root, is rude or impossible. This cross-compiles to static musl
# in a container, copies two files to /tmp, and runs the tests inside an
# unprivileged user namespace.
#
# The remote side needs: ssh, tar, curl, and either root or
# /proc/sys/user/max_user_namespaces > 0. Nothing is installed and nothing
# outside /tmp is touched.

set -eu
cd "$(dirname "$0")/.."

HOST=${1:-}
[ -n "$HOST" ] || { sed -n '2,16p' "$0" >&2; exit 2; }
shift
STAGES=${*:-1 2 3 4 5 6 7 8}

ARCH=$(ssh -o BatchMode=yes "$HOST" 'uname -m')
case "$ARCH" in
    x86_64)  TARGET=x86_64-unknown-linux-musl;  PLATFORM=linux/amd64 ;;
    aarch64) TARGET=aarch64-unknown-linux-musl; PLATFORM=linux/arm64 ;;
    *) echo "unsupported remote arch: $ARCH" >&2; exit 2 ;;
esac
echo "remote is $ARCH; building $TARGET"

OUT=$(mktemp -d)
trap 'rm -rf "$OUT"' EXIT

# Static musl, so the binary does not care what libc the remote box has.
docker run --rm --platform "$PLATFORM" -v "$PWD":/w -v "$OUT":/out -w /w \
    -e CARGO_TARGET_DIR=/tmp/t rust:1.96-slim bash -c "
        set -e
        # The registry client's TLS stack has a C component, so a musl
        # build needs a musl-targeting compiler. Without it the failure is
        # a build script exiting 1 with no explanation.
        apt-get -qq update >/dev/null 2>&1
        apt-get -qq install -y musl-tools >/dev/null 2>&1
        export CC_${TARGET//-/_}=musl-gcc
        rustup target add $TARGET >/dev/null 2>&1 || true
        cargo build --release --target $TARGET --quiet
        cargo test --release --target $TARGET --no-run --quiet
        cp /tmp/t/$TARGET/release/rustainer /out/rustainer
        T=\$(ls -t /tmp/t/$TARGET/release/deps/rustainer_core-* | grep -v '\.d\$' | head -1)
        cp \"\$T\" /out/unit-tests
    "

DIR=/tmp/rustainer-test
ssh -o BatchMode=yes "$HOST" "rm -rf $DIR && mkdir -p $DIR/scripts"
scp -q "$OUT/rustainer" "$OUT/unit-tests" "$HOST:$DIR/"
scp -q scripts/stage*-success-test.sh "$HOST:$DIR/scripts/"

# A delegated cgroup scope when the host has a user systemd, so the resource
# stages exercise real limits instead of only the refusal path. Without it
# the engine correctly refuses every limit, which is worth testing too but
# covers much less.
# shellcheck disable=SC2029  # $DIR and $STAGES are meant to expand locally
ssh -o BatchMode=yes "$HOST" "
    set -u; cd $DIR; chmod +x rustainer unit-tests
    SCOPE=''
    if systemd-run --user --scope -p Delegate=yes -q true >/dev/null 2>&1; then
        SCOPE='systemd-run --user --scope -p Delegate=yes -q'
        echo 'using a delegated cgroup scope'
    else
        echo 'no delegated cgroup scope available; limits will be refused, not applied'
    fi
    echo \"host: \$(uname -r) \$(uname -m), uid \$(id -u)\"
    echo; echo '=== unit tests ==='
    ./unit-tests 2>&1 | tail -3
    for s in $STAGES; do
        echo; echo \"=== stage \$s ===\"
        # Unprivileged by design: a user namespace gives CAP_SYS_ADMIN in the
        # namespace that owns our mount and pid namespaces, which is all these
        # stages need. Expressing it this way is also what proves the rootless
        # seam of RND.md 18.6 is holding.
        \$SCOPE unshare --user --map-root-user \
            env RUSTAINER_BIN=$DIR/rustainer bash scripts/stage\${s}-success-test.sh \
            2>&1 | tail -n 8
    done
"
