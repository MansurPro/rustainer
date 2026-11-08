#!/bin/bash
# Stage 5 success test - RND.md section 17, "Security controls".
#
#   sudo bash scripts/stage5-success-test.sh
#   unshare --user --map-root-user bash scripts/stage5-success-test.sh
#
# Every assertion runs as a NON-ROOT container user and reads kernel state
# back from inside. The non-root part is non-negotiable: the capability
# ordering bug fails EPERM for a non-root user and SILENTLY SUCCEEDS for
# `USER root`, so a suite that only runs stock images proves nothing.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage5.XXXXXX")
export RUSTAINER_ROOT="$SCRATCH/state"
trap 'rm -rf "$SCRATCH"' EXIT
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
ROOTFS="$SCRATCH/rootfs"; mkdir -p "$ROOTFS"
TAR_OWNER=--no-same-owner
[ "$(id -u)" = 0 ] && [ "$(awk '{print $1, $2, $3}' /proc/self/uid_map)" = "0 0 4294967295" ] && TAR_OWNER=--same-owner
tar $TAR_OWNER -xzf "$CACHE" -C "$ROOTFS" || exit 2

R() { "$B" run --rootfs "$ROOTFS" --network host "$@"; }
# The whole point: a non-root container user.
NR() { R --user "$TESTUID:$TESTUID" "$@"; }

# Can we actually become a different uid here? In a single-ID user
# namespace - `unshare --user --map-root-user`, and the whole of stage 11 -
# exactly one id is mapped and setresuid to any other returns EINVAL. That
# is a real limitation, not a bug, so the test says which path it took
# rather than quietly asserting less.
TESTUID=1000
NONROOT=yes
if ! grep -qE '^\s*0\s+0\s+4294967295' /proc/self/uid_map 2>/dev/null; then
    if ! awk '{ if (1000 >= $1 && 1000 < $1 + $3) found=1 } END { exit !found }' /proc/self/uid_map; then
        NONROOT=no
    fi
fi
echo "non-root container user available: $NONROOT"

if [ "$NONROOT" = no ]; then
    echo
    echo "0. Only one id is mapped here, so --user must refuse and explain"
    OUT=$(R --user 1000:1000 /bin/true 2>&1); RC=$?
    [ "$RC" -ne 0 ] && echo "$OUT" | grep -q 'not mapped into this user namespace' \
        && ok "--user with an unmapped id refuses and names stage 11" \
        || bad "--user with an unmapped id did not refuse clearly: $OUT"
    # Everything below needs a second uid; run it as the one id we have, so
    # the capability and seccomp mechanics are still exercised.
    TESTUID=0
    skip_note="(as uid 0: the only id mapped here)"
    echo "  note: the remaining checks run $skip_note"
fi

echo
echo "1. The bounding set is actually empty for the container user"
# Dropped while euid was still 0. Done after the uid change it returns EPERM
# for this user and silently succeeds for root - which is why this runs as
# 1000 and reads CapBnd back from inside rather than trusting the call.
check "CapBnd is empty with --cap-drop ALL" \
    "$(NR --cap-drop ALL /bin/cat /proc/self/status 2>/dev/null | awk '/^CapBnd:/{print $2}')" \
    "0000000000000000"
check "CapEff is empty with --cap-drop ALL" \
    "$(NR --cap-drop ALL /bin/cat /proc/self/status 2>/dev/null | awk '/^CapEff:/{print $2}')" \
    "0000000000000000"
check "CapInh is empty, closing the inheritable hole" \
    "$(NR /bin/cat /proc/self/status 2>/dev/null | awk '/^CapInh:/{print $2}')" \
    "0000000000000000"
# From /proc, not `id`: busybox's id fails outright for a uid with no
# /etc/passwd entry, and reading kernel state is the discipline anyway.
check "the container runs as the uid asked for" \
    "$(NR /bin/cat /proc/self/status 2>/dev/null | awk '/^Uid:/{print $2}')" "$TESTUID"
check "and the gid asked for" \
    "$(NR /bin/cat /proc/self/status 2>/dev/null | awk '/^Gid:/{print $2}')" "$TESTUID"

echo
echo "2. The default set is Docker's fourteen, and excludes CAP_SYS_ADMIN"
BND=$(R /bin/cat /proc/self/status 2>/dev/null | awk '/^CapBnd:/{print $2}')
echo "  default CapBnd: $BND"
# CAP_SYS_ADMIN is bit 21.
python3 - "$BND" <<'PY' && ok "CAP_SYS_ADMIN is not in the default bounding set" || bad "CAP_SYS_ADMIN is in the default bounding set"
import sys
v = int(sys.argv[1], 16)
raise SystemExit(0 if not (v >> 21) & 1 else 1)
PY
python3 - "$BND" <<'PY' && ok "the default set has fourteen capabilities" || bad "the default set is not fourteen capabilities"
import sys
raise SystemExit(0 if bin(int(sys.argv[1], 16)).count('1') == 14 else 1)
PY

echo
echo "3. --cap-add actually adds"
ADD=$(R --cap-add NET_ADMIN /bin/cat /proc/self/status 2>/dev/null | awk '/^CapBnd:/{print $2}')
python3 - "$ADD" <<'PY' && ok "CAP_NET_ADMIN (bit 12) is present after --cap-add" || bad "--cap-add had no effect"
import sys
raise SystemExit(0 if (int(sys.argv[1], 16) >> 12) & 1 else 1)
PY

echo
echo "4. A denied syscall returns the configured errno"
# The default profile denies the mount family with EPERM. Without that, a
# container could mount a second procfs - a fresh superblock with nothing
# masked - and walk straight around section 6's masked paths.
OUT=$(NR /bin/mount -t proc proc /proc 2>&1 | tail -1)
echo "  $OUT"
echo "$OUT" | grep -qiE 'permission denied|not permitted|operation not permitted' \
    && ok "mount(2) is refused by the filter" || bad "mount was not refused"
check "no_new_privs is set" \
    "$(NR /bin/cat /proc/self/status 2>/dev/null | awk '/^NoNewPrivs:/{print $2}')" "1"
check "seccomp is in filter mode" \
    "$(NR /bin/cat /proc/self/status 2>/dev/null | awk '/^Seccomp:/{print $2}')" "2"

echo
echo "5. seccomp=unconfined really is unconfined"
check "no filter is installed when asked for none" \
    "$(NR --security-opt seccomp=unconfined /bin/cat /proc/self/status 2>/dev/null | awk '/^Seccomp:/{print $2}')" \
    "0"

echo
echo "6. Masked and read-only paths"
# Masking a file is /dev/null bound over it, chosen over deleting so that
# programs which stat() the path still work. So "can it be opened?" is the
# WRONG question - opening the mask succeeds, because /dev/null opens. The
# right ones are whether something is mounted there at all, and whether any
# data comes back.
check "something is mounted over /proc/kcore" \
    "$(NR /bin/sh -c 'grep -c " /proc/kcore " /proc/self/mountinfo' 2>/dev/null | tail -1)" "1"
check "and no byte of physical memory can be read through it" \
    "$(NR /bin/sh -c 'head -c1 /proc/kcore 2>/dev/null | wc -c' 2>/dev/null | tail -1 | tr -d " ")" "0"
check "/proc/sys is read-only" \
    "$(NR /bin/sh -c 'echo 1 > /proc/sys/kernel/dmesg_restrict 2>/dev/null && echo writable || echo readonly' 2>/dev/null | tail -1)" \
    "readonly"
check "/proc/sysrq-trigger cannot crash the host" \
    "$(NR /bin/sh -c 'echo h > /proc/sysrq-trigger 2>/dev/null && echo writable || echo readonly' 2>/dev/null | tail -1)" \
    "readonly"

echo
echo "7. mknod is refused without CAP_MKNOD"
check "mknod fails with the capability dropped" \
    "$(NR --cap-drop ALL /bin/sh -c '/bin/mknod /tmp/d c 1 3 2>/dev/null && echo created || echo refused' 2>/dev/null | tail -1)" \
    "refused"

echo
echo "8. A custom profile is honoured, and a bad one is refused"
cat > "$SCRATCH/prof.json" <<'JSON'
{"defaultAction":"SCMP_ACT_ALLOW",
 "syscalls":[{"names":["chmod","fchmodat"],"action":"SCMP_ACT_ERRNO","errnoRet":1}]}
JSON
OUT=$(NR --security-opt "seccomp=$SCRATCH/prof.json" /bin/sh -c 'chmod 777 /tmp 2>&1' 2>/dev/null | tail -1)
echo "  $OUT"
echo "$OUT" | grep -qi 'not permitted' && ok "the custom profile's errno is returned" \
    || bad "the custom profile was not applied"
# EPERM is errno 1, which is what the profile asked for.
cat > "$SCRATCH/args.json" <<'JSON'
{"defaultAction":"SCMP_ACT_ALLOW",
 "syscalls":[{"names":["clone"],"action":"SCMP_ACT_ERRNO","args":[{"index":0,"value":1,"op":"SCMP_CMP_EQ"}]}]}
JSON
R --security-opt "seccomp=$SCRATCH/args.json" /bin/true 2>&1 | grep -q 'args' \
    && ok "a profile using per-argument rules is refused, not silently weakened" \
    || bad "an args profile was accepted"

echo
echo "9. Refusals stay explicit"
R --cap-add ALL /bin/true 2>&1 | grep -q 'CAP_SYS_ADMIN' \
    && ok "--cap-add ALL refuses and says why" || bad "--cap-add ALL did not refuse"
R --user nobody /bin/true 2>&1 | grep -q 'etc/passwd' \
    && ok "a user name refuses and says what is missing" || bad "a user name did not refuse"
R --security-opt apparmor=unconfined /bin/true 2>&1 | grep -q 'not implemented' \
    && ok "an unimplemented security-opt refuses" || bad "apparmor= was silently accepted"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
