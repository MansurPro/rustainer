#!/bin/bash
# Stage 2 success test - RND.md section 17, "A real root filesystem".
#
# Needs CAP_SYS_ADMIN in the current user namespace, and an alpine rootfs,
# which it downloads and extracts with tar - exactly as the ladder specifies,
# because pulling an image is stage 6.
#
#   sudo bash scripts/stage2-success-test.sh
#   unshare --user --map-root-user bash scripts/stage2-success-test.sh
#
#   docker run --rm --privileged -v "$PWD":/w -w /w rust:1.96-slim \
#       bash scripts/stage2-success-test.sh
#
# This is the stage where host damage first becomes possible, so the
# assertions are as much about what the HOST looks like afterwards as about
# what the container sees.

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
if ! have_cap_sys_admin; then
    cat >&2 <<'MSG'
This test needs CAP_SYS_ADMIN in the current user namespace, because
CLONE_NEWNS and pivot_root do. Two ways to get it:

  sudo bash scripts/stage2-success-test.sh
  unshare --user --map-root-user bash scripts/stage2-success-test.sh
MSG
    exit 2
fi

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

echo "kernel $(uname -r) $(uname -m), uid $(id -u), userns $(readlink /proc/self/ns/user)"

# --- a rootfs, extracted by hand ------------------------------------------
case "$(uname -m)" in
    x86_64)  ARCH=x86_64 ;;
    aarch64) ARCH=aarch64 ;;
    *) echo "unsupported arch $(uname -m)" >&2; exit 2 ;;
esac
CACHE="${TMPDIR:-/tmp}/rustainer-alpine-$ARCH.tar.gz"
if [ ! -s "$CACHE" ]; then
    BASE="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/$ARCH"
    NAME=$(curl -fsS "$BASE/" | grep -o "alpine-minirootfs-[0-9.]*-$ARCH\.tar\.gz" | sort -uV | tail -1)
    [ -n "$NAME" ] || { echo "could not find an alpine minirootfs at $BASE" >&2; exit 2; }
    echo "fetching $NAME"
    curl -fsS -o "$CACHE" "$BASE/$NAME" || exit 2
fi
echo "rootfs tarball sha256: $(sha256sum "$CACHE" | cut -d' ' -f1)"

# Each extraction gets its own new, empty directory: the tarball is
# untrusted input, and stage 6 is where we learn to police it properly.
SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage2.XXXXXX")
cleanup() { rm -rf "$SCRATCH" 2>/dev/null; }
trap cleanup EXIT
# Own state root, so a test run never touches a real one.
export RUSTAINER_ROOT="$SCRATCH/state"
ROOTFS="$SCRATCH/rootfs"
ROOTFS_B="$SCRATCH/rootfs-b"
mkdir -p "$ROOTFS" "$ROOTFS_B"
# --no-same-owner because a single-ID user namespace maps only our own uid:
# restoring alpine's `gid 42` on /etc/shadow fails EINVAL. Preserving image
# ownership needs root or a multi-extent subuid map, which the ladder defers
# to stage 11 - so the test says so rather than quietly running as root.
TAR_OWNER=--no-same-owner
[ "$(id -u)" = 0 ] && [ "$(awk '{print $1, $2, $3}' /proc/self/uid_map)" = "0 0 4294967295" ] && TAR_OWNER=--same-owner
tar $TAR_OWNER -xzf "$CACHE" -C "$ROOTFS" || exit 2
tar $TAR_OWNER -xzf "$CACHE" -C "$ROOTFS_B" || exit 2
echo "rootfs extracted with $TAR_OWNER"

# A marker the container must NOT be able to see: it lives beside the
# rootfs, not inside it.
echo "host-only" > "$SCRATCH/host-marker"

# A stable signature of the host's mount table. Not raw bytes: on a shared
# machine another process may mount something during the run, and mount IDs
# churn. (mountpoint, fstype, source) is what we actually care about.
mountsig() { awk '{for(i=7;i<=NF;i++) if($i=="-"){print $5, $(i+1), $(i+2); break}}' /proc/self/mountinfo | sort; }
BEFORE=$(mountsig)

R() { "$B" run --rootfs "$ROOTFS" --network host --hostname boxy "$@" 2>/dev/null; }

echo
echo "1. The container's root really is the new root"
check "pwd is / after the pivot" "$(R /bin/sh -c 'pwd')" "/"
check "the rootfs's own marker file is visible" \
    "$(R /bin/sh -c '[ -f /etc/alpine-release ] && echo yes')" "yes"
check "the host marker beside the rootfs is NOT reachable" \
    "$(R /bin/sh -c '[ -e /host-marker ] && echo leaked || echo no')" "no"
check "the host's /etc/debian_version is gone" \
    "$(R /bin/sh -c '[ -e /etc/debian_version ] && echo leaked || echo no')" "no"

echo
echo "2. The old root is detached, not merely hidden"
# Trap 3: leaving put_old mounted and reachable is a one-line escape.
OLD=$(R /bin/sh -c 'grep -cE " /(oldroot|\.pivot_root|put_old)" /proc/self/mountinfo' )
check "no put_old mount remains" "${OLD:-0}" "0"
# Field 4 of mountinfo is the subtree of the SOURCE filesystem that this
# mount exposes. For our self-bound rootfs that is the host path we bound,
# and seeing it here looks alarming but is correct and unavoidable: it is a
# label, not a reachable path. Containment is proved by section 1, which
# shows nothing outside the rootfs can actually be opened. Asserted rather
# than ignored so nobody later "fixes" it.
check "the root mount names the bound subtree, which is not a leak" \
    "$(R /bin/sh -c 'awk "\$5 == \"/\" {print (\$4 ~ /rootfs/) ? \"rootfs\" : \$4}" /proc/self/mountinfo | head -1')" "rootfs"
# The real test that the host's tree is gone: the host has dozens of mounts
# and the container must have almost none of them.
HOST_MOUNTS=$(wc -l < /proc/self/mountinfo)
CT_MOUNTS=$(R /bin/sh -c 'wc -l < /proc/self/mountinfo' | tr -d ' ')
echo "  host has $HOST_MOUNTS mounts, container has $CT_MOUNTS"
# Not a count comparison: the container can legitimately have MORE mounts
# than the host, because masked paths are mounts and the rootless path binds
# each device node separately. What must hold is that every mount point in
# the container is one we put there - nothing of the host's tree survived.
STRAY=$(R /bin/sh -c 'awk "{print \$5}" /proc/self/mountinfo' 2>/dev/null \
    | grep -vE '^/$|^/proc|^/dev|^/sys' | tr '\n' ' ')
if [ -z "$STRAY" ]; then
    ok "every mount in the container is one we created"
else
    bad "the container has mounts we did not create: $STRAY"
fi
for host_only in /run /boot; do
    if [ "$(R /bin/sh -c "grep -c \" $host_only \" /proc/self/mountinfo")" = "0" ]; then
        ok "the host's $host_only mount is absent"
    else
        bad "the host's $host_only mount is still visible"
    fi
done

# /sys is bind-mounted recursively when we cannot mount a fresh sysfs, so its
# submounts ARE visible - that is expected. What must hold is that every one
# of them is read-only. A recursive bind plus a read-only *remount* passes
# the top-level check and leaves exactly these writable, which is the defect
# mount_setattr(AT_RECURSIVE) exists to close. Read the flag back from
# mountinfo rather than probing with a write, which can fail for unrelated
# permission reasons and look like a pass.
for sub in /sys /sys/fs/cgroup /sys/kernel/debug /sys/kernel/tracing /sys/fs/bpf; do
    OPTS=$(R /bin/sh -c "awk '\$5 == \"$sub\" {print \$6}' /proc/self/mountinfo")
    case "$OPTS" in
        "")      ok "$sub is not present (a fresh sysfs was mounted instead)" ;;
        ro,*|*,ro|*,ro,*|ro) ok "$sub is mounted read-only" ;;
        *)       bad "$sub is mounted '$OPTS' - writable submount under a read-only /sys" ;;
    esac
done

echo
echo "3. The mount set is ours and nothing else"
MOUNTS=$(R /bin/sh -c 'awk "{print \$5}" /proc/self/mountinfo | sort | tr "\n" " "')
echo "  mounts inside: $MOUNTS"
for want in / /proc /dev /dev/shm /sys; do
    case " $MOUNTS " in
        *" $want "*) ok "$want is mounted" ;;
        *) bad "$want is missing from the container's mount table" ;;
    esac
done

echo
echo "4. /proc was mounted by the child, after the PID namespace"
# Mounted by the supervisor instead, the container would see host PIDs.
# Counted with a shell glob, not `ls | grep`: a pipeline would fork two more
# processes and then count them, which is how this assertion first "failed".
N=$(R /bin/sh -c 'set -- /proc/[0-9]*; echo $#')
if [ "${N:-99}" -le 2 ]; then
    ok "/proc shows only $N container process(es)"
else
    bad "/proc shows $N processes; procfs bound the wrong pid namespace"
fi
check "init is PID 1" "$(R /bin/sh -c 'echo $$')" "1"

echo
echo "5. /dev is a fresh tmpfs, not the image's"
check "/dev/null works" "$(R /bin/sh -c 'echo discard > /dev/null && echo ok')" "ok"
check "/dev/shm is writable" "$(R /bin/sh -c 'echo x > /dev/shm/t && cat /dev/shm/t')" "x"
check "/dev/fd resolves through /proc" "$(R /bin/sh -c '[ -e /dev/fd/0 ] && echo ok')" "ok"
check "/sys is read-only inside" \
    "$(R /bin/sh -c 'touch /sys/rustainer-probe 2>/dev/null && echo WRITABLE || echo readonly')" "readonly"

echo
echo "6. Two containers do not share each other's mounts"
"$B" run --rootfs "$ROOTFS" --network host /bin/sh -c 'echo A > /dev/shm/marker; sleep 1' >/dev/null 2>&1 &
P1=$!
sleep 0.4
SEEN=$("$B" run --rootfs "$ROOTFS_B" --network host /bin/sh -c '[ -e /dev/shm/marker ] && echo leaked || echo private' 2>/dev/null)
wait $P1
check "a tmpfs written in one container is private to it" "$SEEN" "private"

echo
echo "7. The host is exactly as it was"
AFTER=$(mountsig)
if [ "$BEFORE" = "$AFTER" ]; then
    ok "the host's mount table is unchanged"
else
    bad "the host's mount table changed:"
    diff <(echo "$BEFORE") <(echo "$AFTER") | sed 's/^/        /'
fi
LEFT=$(awk -v r="$SCRATCH" '$5 ~ "^" r {print $5}' /proc/self/mountinfo | wc -l)
check "nothing is still mounted under the container's directory" "$LEFT" "0"

echo
echo "8. Refusals stay explicit"
"$B" run --rootfs "$SCRATCH/host-marker" --network host /bin/true 2>&1 | grep -q 'not a directory' \
    && ok "a non-directory rootfs refuses" || bad "a non-directory rootfs did not refuse"
"$B" run --rootfs "$ROOTFS" --network nonsense /bin/true 2>&1 | grep -q 'not supported' \
    && ok "an unknown network mode refuses" || bad "an unknown network mode was accepted"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
