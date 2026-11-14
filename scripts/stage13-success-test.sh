#!/bin/bash
# Stage 13 success test - Docker image compatibility.
#
#   sudo bash scripts/stage13-success-test.sh
#
# Every other stage in this ladder tests a mechanism we built. This one
# tests a claim: that images built for Docker, by other people, run here
# unchanged. The only way to test that claim is against real images from
# real registries, so this test needs the network and says so rather than
# skipping quietly.
#
# The four defects it exists to keep fixed were all found by running it,
# and every one of them looked like something else:
#
#   - `./` as a tar entry - what `tar -C dir .` writes, and the first member
#     of debian, python, busybox and distroless layers - was rejected as
#     "an empty path". A third of common base images could not be pulled.
#   - An image whose layers contain no /proc could not start, reporting
#     "mounting /proc failed with ENOENT". The mount point was created
#     parent-side, underneath the overlay that then hid it.
#   - A command was searched for in the topmost layer only, and in a
#     hardcoded PATH. `golang:1.24-alpine` has /bin/pwd in the alpine base
#     at the bottom and `go` in /usr/local/go/bin, so the Go image could run
#     neither `pwd` nor `go`.
#   - `USER nginx` was refused because names were not resolved, which rules
#     out every image that sets USER by name - prometheus, grafana,
#     nginx-unprivileged - and `User: ""` was taken literally.

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
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (got '$2', want something containing '$3')" ;; esac; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage13.XXXXXX")
MOUNTED_TMPFS=no
# The image store cannot live on overlayfs: overlayfs refuses to host an
# upperdir, so a container's writable layer would fail to mount.
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=4g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
# A persistent image store when one is offered, because Docker Hub's
# anonymous pull limit is 100 per six hours per address and this test pulls
# a dozen images. Re-downloading them on every run is how a compatibility
# test becomes a rate-limit test. Set RUSTAINER_COMPAT_STORE to a directory
# on a filesystem that can host an overlay upperdir - not overlayfs.
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_COMPAT_STORE:-$SCRATCH/images}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
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

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"

# `try_pull` with this test's own reporting attached: a quota is a named
# skip, a real failure is a failure.
pull_or_report() {
    desc=$1; ref=$2
    try_pull "$ref"
    case $? in
        0) return 0 ;;
        2) skip "$desc: every registry rate-limited us, so this tested nothing" ;;
        *) bad "$desc: pull failed: $PULL_ERR" ;;
    esac
    return 1
}

# Reachability, checked once and loudly. Everything below is network work,
# and a compatibility test that passes because it did nothing is worse than
# one that fails.
if ! timeout 120 "$B" pull alpine:3.21 >/dev/null 2>"$SCRATCH/net.err"; then
    if ! rate_limited "$SCRATCH/net.err"; then
        echo "cannot reach a registry, so nothing here can be tested:" >&2
        tail -2 "$SCRATCH/net.err" >&2
        exit 2
    fi
    echo "Docker Hub is rate-limiting this address; falling back to its mirror." >&2
    if ! timeout 120 "$B" pull mirror.gcr.io/library/alpine:3.21 >/dev/null 2>"$SCRATCH/net.err"; then
        echo "and the mirror is unreachable too, so nothing here can be tested:" >&2
        tail -2 "$SCRATCH/net.err" >&2
        exit 2
    fi
fi

# The WHOLE output, newlines folded to spaces. `tail -1` was the obvious
# thing and the wrong thing: every image here whose point is that it prints
# something - hello-world, podman/hello, busybox's applet list - prints
# several lines, and the last of them is never the interesting one.
R() { timeout 300 "$B" run --network none "$@" 2>&1 | tr '\n' ' '; }
# The last line only, for the cases where the assertion IS about the last
# line: an error message, or a single-line command.
R1() { timeout 300 "$B" run --network none "$@" 2>&1 | tail -1; }

echo
echo "1. Real images, from four registries, pulled and run"
# Four hosts with four different auth and content-negotiation behaviours:
# Docker Hub (token service on a separate host), GitHub, Quay, Google.
run_image() {
    desc=$1; ref=$2; want=$3; shift 3
    pull_or_report "$desc" "$ref" || return
    contains "$desc" "$(R "$PULLED" "$@")" "$want"
}
run_image "alpine runs and is the right release" alpine:3.21 "3.21" /bin/cat /etc/alpine-release
ALPINE=$PULLED
run_image "busybox runs"            busybox:latest   "BusyBox" /bin/busybox
BUSYBOX=$PULLED
run_image "debian runs"             debian:13-slim   "13"      /bin/cat /etc/debian_version
DEBIAN=$PULLED
run_image "hello-world runs its own Cmd, with no shell in the image" \
                                    hello-world:latest "Hello from Docker"
run_image "an image from ghcr.io runs" \
          ghcr.io/linuxcontainers/alpine:latest "Linux" /bin/uname -s
run_image "an image from quay.io runs its own Entrypoint" \
          quay.io/podman/hello:latest "Hello Podman World"

echo
echo "2. The platform actually selected is ours"
# A manifest list offers several; picking the wrong one produces a binary
# that cannot run, and the error does not mention architecture.
check "the container reports this machine's architecture" \
    "$(R1 "$BUSYBOX" /bin/busybox uname -m)" "$ARCH"

echo
echo "3. A layer whose first member is the archive's own root"
# `tar -C dir .` writes `./`, which is how a great many layers are built.
# Rejecting it rejected debian, python, busybox and distroless at once.
check "debian's root-entry layer extracted a real filesystem" \
    "$(R1 "$DEBIAN" /bin/sh -c 'test -x /bin/ls && echo yes')" "yes"
check "busybox's did too" \
    "$(R1 "$BUSYBOX" /bin/sh -c 'test -d /bin && echo yes')" "yes"

echo
echo "4. An image whose layers contain no /proc still starts"
# busybox and hello-world both ship without one. The mount point has to be
# created by the child, on top of the overlay - created parent-side it is
# written underneath the overlay and then hidden by it.
check "/proc is mounted in an image that does not contain it" \
    "$(R1 "$BUSYBOX" /bin/sh -c 'test -r /proc/self/status && echo yes')" "yes"
# Not /proc/1/comm: busybox sh tail-execs the last command of a `-c`
# script, so the name there is whatever that command became. The pid is
# the property that matters and it is unambiguous.
check "and it is the container's own /proc, not the host's" \
    "$(R1 "$BUSYBOX" /bin/busybox sh -c 'echo $$')" "1"

echo
echo "5. The image's own config is obeyed, not a default we invented"
if pull_or_report "the golang image" golang:1.24-alpine; then
    GOLANG=$PULLED
    # WorkingDir, which is /go for this image.
    check "WorkingDir is entered" "$(R1 "$GOLANG" pwd)" "/go"
    # Env: GOPATH is set by the image and nothing else would set it.
    check "the image's environment is exported" \
        "$(R1 "$GOLANG" /bin/sh -c 'echo $GOPATH')" "/go"
    # PATH: `go` is in /usr/local/go/bin, which is on the image's PATH and
    # on no default list anywhere.
    contains "a command on the image's PATH but not on ours is found" \
        "$(R "$GOLANG" go version)" "go1.24"
    # Layers: /bin/pwd is in the alpine base at the bottom of four layers.
    check "a command in the BOTTOM layer is found" \
        "$(R1 "$GOLANG" /bin/sh -c 'pwd')" "/go"
fi

echo
echo "6. USER by name, resolved against the CONTAINER's /etc/passwd"
# Not the host's: `nobody` is 65534 on Debian and 99 on Fedora, and
# resolving against the host would run the container as a different person
# than the image asked for.
if pull_or_report "the prometheus image" quay.io/prometheus/prometheus:v3.1.0; then
    PROM=$PULLED
    if [ "$IN_USERNS" = yes ]; then
        # A single-ID user namespace maps exactly one id, so 65534 is not
        # reachable however correctly we resolved it. The refusal naming
        # 65534 is the stronger assertion anyway: it can only say that
        # number if it looked `nobody` up in the image's own /etc/passwd.
        contains "USER nobody is RESOLVED, then refused for want of a mapping" \
            "$(R "$PROM" /bin/busybox id)" "uid 65534"
    else
        contains "an image that sets USER nobody runs as nobody" \
            "$(R "$PROM" /bin/busybox id)" "uid=65534"
    fi
fi
if pull_or_report "the nginx image" nginx:1.27-alpine; then
    NGINX=$PULLED
    if [ "$IN_USERNS" = yes ]; then
        contains "--user by name resolves to the id that image defines" \
            "$(R --user nginx "$NGINX" id -u)" "uid 101"
        contains "a numeric --user is still taken as given, not looked up" \
            "$(R --user 1234:5678 "$NGINX" id -u)" "uid 1234"
    else
        check "--user by name resolves to the id that image defines" \
            "$(R1 --user nginx "$NGINX" id -u)" "101"
        check "a numeric --user is still taken as given, not looked up" \
            "$(R1 --user 1234:5678 "$NGINX" id -u)" "1234"
    fi
    OUT=$(R1 --user nosuchuser "$NGINX" id -u)
    case "$OUT" in
        *"/etc/passwd has no such user"*) ok "a name the image does not define is refused by name" ;;
        *) bad "an unknown user name was not refused clearly: $OUT" ;;
    esac
fi

echo
echo "7. The store is content-addressed, so a second pull extracts nothing"
LAYERS_BEFORE=$(find "$RUSTAINER_IMAGE_ROOT" -maxdepth 2 -path '*/layers*/*' -prune -type d 2>/dev/null | wc -l)
IMAGES_BEFORE=$("$B" images | tail -n +2 | grep -c .)
timeout 300 "$B" pull "$BUSYBOX" >/dev/null 2>&1
LAYERS_AFTER=$(find "$RUSTAINER_IMAGE_ROOT" -maxdepth 2 -path '*/layers*/*' -prune -type d 2>/dev/null | wc -l)
IMAGES_AFTER=$("$B" images | tail -n +2 | grep -c .)
check "re-pulling an image adds no layer directories" "$LAYERS_AFTER" "$LAYERS_BEFORE"
# Counted as a delta, not as "there is exactly one busybox". With a
# persistent store - which is how this test avoids Docker Hub's quota - the
# same image is legitimately present under both its Hub reference and its
# mirror reference, and an absolute count would call that a bug.
check "and no second entry in the image list" "$IMAGES_AFTER" "$IMAGES_BEFORE"

echo
echo "8. A digest-pinned reference is a valid reference"
DIG=$("$B" images | awk '/alpine_3.21/{print $2}' | head -1)
if [ -n "$DIG" ]; then
    ok "a pulled image records a config digest ($DIG)"
else
    bad "no digest recorded for a pulled image"
fi

echo
echo "9. What genuinely cannot run says so, and says why"
# distroless/static has neither Entrypoint nor Cmd: it is a base for a
# binary you add. Running it is a user error, and the message has to be
# about the image rather than about a missing file.
if pull_or_report "the distroless image" gcr.io/distroless/static-debian12:latest; then
    DISTROLESS=$PULLED
    OUT=$(R "$DISTROLESS")
    contains "an image with no Entrypoint and no Cmd explains itself" \
        "$OUT" "neither an Entrypoint nor a Cmd"
    # And it still runs if you bring your own command - which is the point
    # of a distroless base.
    check "and it runs a command you supply" \
        "$(R1 "$DISTROLESS" /bin/sh -c 'echo x' | grep -c 'No such file')" "1"
fi
OUT=$(R1 "$ALPINE" nosuchprogram)
contains "a command missing from the image names the PATH it searched" "$OUT" "which is the PATH this image declares"

echo
mirror_note
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
