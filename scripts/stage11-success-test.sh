#!/bin/bash
# Stage 11 success test - RND.md section 17, "Single-ID rootless".
#
#   bash scripts/stage11-success-test.sh        # NOT sudo. That is the point.
#
# Every other stage test is run under sudo or inside `unshare`. This one
# must not be: the whole claim of this stage is that an ORDINARY
# unprivileged user, with no wrapper and no helper, runs a container - and
# that nothing in stages 1-10 had to change shape to let them.
#
# So the test is as much about what is ABSENT as about what passes. There
# is no --rootless flag, no second code path, no setuid binary. The engine
# notices it lacks CAP_SYS_ADMIN, adds one clone flag, and writes one line
# to uid_map. If that were not enough, the privilege seam of section 18.6
# would have been in the wrong place, and THAT is the finding this test
# exists to make or refute.

set -u
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rustainer-target}"

PASS=0
FAIL=0
SKIP=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || { echo "needs python3 to decode capability masks" >&2; exit 2; }

# The one prerequisite that is not optional, stated as a refusal rather
# than worked around. Running this as root would test the path every other
# stage already covers and prove nothing about this one.
if [ "$(id -u)" = 0 ]; then
    echo "stage 11 must NOT be run as root: it is the stage about not being root." >&2
    echo "Run it as an ordinary user, with no sudo and no unshare." >&2
    exit 2
fi

# Equally, not already inside somebody else's user namespace. If the shell
# is already in one, the engine finds CAP_SYS_ADMIN, never creates a
# namespace of its own, and the stage under test does not run.
if [ "$(awk '{print $1, $2, $3}' /proc/self/uid_map 2>/dev/null)" != "0 0 4294967295" ]; then
    echo "already inside a user namespace; this stage tests creating one." >&2
    echo "Run it from a normal shell, not inside unshare." >&2
    exit 2
fi

if [ "$(cat /proc/sys/user/max_user_namespaces 2>/dev/null || echo 0)" -le 0 ]; then
    echo "this host forbids unprivileged user namespaces; nothing to test" >&2
    exit 2
fi

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage11.XXXXXX")
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
mkdir -p "$RUSTAINER_ROOT" && chmod 700 "$RUSTAINER_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    rm -rf "$SCRATCH"
}
trap cleanup EXIT
MYUID=$(id -u)
MYGID=$(id -g)
echo "kernel $(uname -r) $(uname -m), uid $MYUID (NOT root, by design)"

# An overlay upperdir cannot live on overlayfs, which is what $TMPDIR often
# is inside a container. Say so rather than failing eight assertions deep.
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    echo "scratch is on overlayfs, which cannot host an upperdir; set TMPDIR elsewhere" >&2
    exit 2
fi

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
# --no-same-owner is not a concession here: an unprivileged user cannot
# chown, and that is exactly the condition under test.
tar --no-same-owner -xzf "$CACHE" -C "$BASE" || exit 2

# --network none throughout: a bridge needs CAP_NET_ADMIN in the user
# namespace owning the HOST's network namespace, which a nested one can
# never have. That refusal is itself asserted in section 6.
R() { timeout 120 "$B" run --network none --layer "$BASE" "$@" 2>/dev/null; }

echo
echo "1. THE CRITERION: an unprivileged user runs a container"
check "it runs, and is root inside" "$(R /bin/cat /proc/self/status | awk '/^Uid:/{print $2}')" "0"
check "the gid is root inside too" "$(R /bin/cat /proc/self/status | awk '/^Gid:/{print $2}')" "0"

echo
echo "2. Root inside is exactly one id, and it is us"
# The single extent. A range would need /etc/subuid and the setuid
# helpers, which are deliberately not implemented - so if this is ever
# anything but one line, something grew a privileged helper.
MAP=$(R /bin/cat /proc/self/uid_map | tr -s ' ' | sed 's/^ //;s/ $//')
echo "  uid_map: $MAP"
check "uid_map maps our own uid to 0, one id wide" "$MAP" "0 $MYUID 1"
GMAP=$(R /bin/cat /proc/self/gid_map | tr -s ' ' | sed 's/^ //;s/ $//')
check "gid_map maps our own gid to 0, one id wide" "$GMAP" "0 $MYGID 1"
# setgroups must be denied BEFORE gid_map is written, or the write fails;
# a container that kept the parent's supplementary groups is the
# escalation the rule exists to prevent.
check "setgroups is denied" "$(R /bin/cat /proc/self/setgroups)" "deny"
check "exactly one id is mapped, not a range" "$(R /bin/cat /proc/self/uid_map | wc -l | tr -d ' ')" "1"

echo
echo "3. The namespaces are genuinely ours"
check "a user namespace of its own" \
    "$([ "$(R /bin/readlink /proc/self/ns/user)" = "$(readlink /proc/self/ns/user)" ] \
        && echo shared || echo private)" "private"
check "pid 1 is the container's own process" "$(R /bin/cat /proc/1/comm)" "cat"
check "a pid namespace with almost nothing in it" \
    "$([ "$(R /bin/sh -c 'ls /proc | grep -c "^[0-9]*$"')" -lt 10 ] && echo few || echo many)" "few"
check "a mount namespace of its own" \
    "$([ "$(R /bin/readlink /proc/self/ns/mnt)" = "$(readlink /proc/self/ns/mnt)" ] \
        && echo shared || echo private)" "private"
check "a uts namespace, so the hostname is its own" \
    "$(timeout 60 "$B" run --network none --hostname boxy --layer "$BASE" /bin/hostname 2>/dev/null)" "boxy"
# Not a device count: sit, gre, ipip and ip6_tunnel each create a fallback
# device in EVERY namespace, so a fresh one is not "only lo" (stage 8). The
# property is that nothing else is UP and there is no route off the machine.
check "a network namespace of its own, with nothing but lo up" \
    "$(R /bin/sh -c 'ip -o link show up 2>/dev/null | awk -F": " "{print \$2}"' | tr -d ' \n')" "lo"
check "and no route anywhere" \
    "$(R /bin/sh -c 'ip route 2>/dev/null | wc -l')" "0"

echo
echo "4. The root filesystem is the layer, not the host's"
# If pivot_root silently did nothing, this is the assertion that notices:
# the host's root has entries an alpine minirootfs does not.
check "/etc/alpine-release exists inside" \
    "$(R /bin/sh -c '[ -f /etc/alpine-release ] && echo yes || echo no')" "yes"
check "the host's root is not visible" \
    "$(R /bin/sh -c '[ -e /w/Cargo.toml ] && echo leaked || echo private')" "private"
check "/proc is the container's own" "$(R /bin/sh -c 'ls /proc/1/root >/dev/null 2>&1 && echo yes')" "yes"

echo
echo "5. Security controls are the same ones, not a weakened set"
BND=$(R /bin/cat /proc/self/status | awk '/^CapBnd:/{print $2}')
echo "  CapBnd: $BND"
python3 - "$BND" <<'PY' && ok "the default set is still Docker's fourteen" || bad "rootless got a different capability set"
import sys
raise SystemExit(0 if bin(int(sys.argv[1], 16)).count('1') == 14 else 1)
PY
python3 - "$BND" <<'PY' && ok "CAP_SYS_ADMIN is still not in it" || bad "CAP_SYS_ADMIN leaked into the rootless default"
import sys
raise SystemExit(0 if not (int(sys.argv[1], 16) >> 21) & 1 else 1)
PY
check "no_new_privs is set" "$(R /bin/cat /proc/self/status | awk '/^NoNewPrivs:/{print $2}')" "1"
# 2 = SECCOMP_MODE_FILTER. A rootless container with no filter would be
# the quiet failure this whole stage is designed to expose.
check "a seccomp filter is installed" \
    "$(R /bin/cat /proc/self/status | awk '/^Seccomp:/{print $2}')" "2"
OUT=$(timeout 60 "$B" run --network none --layer "$BASE" /bin/mount -t proc proc /proc 2>&1 | tail -1)
echo "  $OUT"
echo "$OUT" | grep -qi 'permission denied\|operation not permitted' \
    && ok "the denied syscall still returns the configured errno" \
    || bad "mount was not denied"

echo
echo "6. Where a facility is unavailable, it is NAMED - not quietly skipped"
# This is half the stage. An engine that silently gives you no limits, or
# a container with no network and no explanation, is the failure mode.
# Which of the two outcomes is correct depends on the host, so the test
# asks the host first. Asserting only the refusal would turn a *working*
# delegated cgroup into a failure - which is what happened the day the test
# harness learned to step out of its own scope.
OUT=$(timeout 60 "$B" run --network none -m 64m --layer "$BASE" \
        /bin/cat /sys/fs/cgroup/memory.max 2>&1)
if echo "$OUT" | grep -q '^67108864$'; then
    ok "a limit IS enforced here, and the container reads it back as 64m"
elif echo "$OUT" | grep -q 'cannot be enforced' && echo "$OUT" | grep -q 'Delegate=yes'; then
    ok "a limit with no writable cgroup refuses and names the fix"
else
    bad "a limit neither applied nor refused with a reason: $(echo "$OUT" | tail -1)"
fi
OUT=$(timeout 60 "$B" run --layer "$BASE" /bin/true 2>&1)
echo "$OUT" | grep -q 'CAP_NET_ADMIN' && echo "$OUT" | grep -q 'network none' \
    && ok "bridged networking refuses and names what is degraded" \
    || bad "the networking degradation was not named"
OUT=$(timeout 60 "$B" run --network none --user 1000:1000 --layer "$BASE" /bin/true 2>&1)
echo "$OUT" | grep -q 'not mapped' \
    && ok "--user with an unmapped id refuses in the parent, not EINVAL in the child" \
    || bad "--user failed obscurely instead of refusing clearly"
echo "$OUT" | grep -q 'subuid' \
    && ok "and names subuid as the thing not implemented" \
    || bad "the refusal did not say why a range is unavailable"
# mknod is the degradation that is NOT fixable at any privilege level
# reachable from here: the check is against the INITIAL user namespace.
timeout 60 "$B" run --network none --layer "$BASE" /bin/true 2>&1 \
    | grep -q 'degraded: device nodes' \
    && ok "device nodes are reported degraded, with the reason" \
    || bad "device nodes were silently bind-mounted with no word about it"

echo
echo "7. Stages 6 and 7 did not change shape either"
# Layers, the overlay and the image path all have to work unchanged. The
# upper is private per container and an unprivileged user owns it.
# Two DISTINCT directories: overlayfs rejects the same lowerdir twice, so
# passing $BASE twice would test the test, not the engine.
UPPER_LAYER="$SCRATCH/layer2"; mkdir -p "$UPPER_LAYER/etc"
echo "from the upper layer" > "$UPPER_LAYER/etc/marker"
check "a two-layer overlay still assembles, upper layer winning" \
    "$(timeout 60 "$B" run --network none --layer "$BASE" --layer "$UPPER_LAYER" \
        /bin/cat /etc/marker 2>/dev/null)" "from the upper layer"
check "and the lower layer is still there underneath" \
    "$(timeout 60 "$B" run --network none --layer "$BASE" --layer "$UPPER_LAYER" \
        /bin/sh -c '[ -f /etc/alpine-release ] && echo yes || echo no' 2>/dev/null)" "yes"
check "the writable upper is writable" \
    "$(R /bin/sh -c 'echo x > /tmp/probe && cat /tmp/probe')" "x"
check "a read-only root refuses writes" \
    "$(timeout 60 "$B" run --network none --read-only --layer "$BASE" \
        /bin/sh -c 'echo x > /probe 2>/dev/null || echo refused' 2>/dev/null)" "refused"
if timeout 180 "$B" pull alpine:3.21 >/dev/null 2>&1; then
    check "an image pulled and unpacked by an unprivileged user runs" \
        "$(timeout 60 "$B" run --network none alpine:3.21 /bin/cat /etc/alpine-release 2>/dev/null \
            | cut -d. -f1,2)" "3.21"
else
    SKIP=$((SKIP+1)); echo "  SKIP  no registry access"
fi

echo
echo "8. Nothing grew a rootless-specific shape"
# The actual success criterion of this stage, stated as an assertion. If
# rootless needed its own flag, its own subcommand or a setuid helper,
# the privilege seam was in the wrong place - and the finding is this
# test failing, not a note in a README.
"$B" run --help 2>&1 | grep -qi 'rootless\|subuid\|userns' \
    && bad "a rootless-specific flag appeared; the seam was in the wrong place" \
    || ok "no --rootless flag exists: the same command line as root"
check "the binary is not setuid" \
    "$([ -u "$B" ] && echo setuid || echo plain)" "plain"
check "ps works without root, like every other stage" \
    "$("$B" ps -a >/dev/null 2>&1 && echo yes || echo no)" "yes"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
