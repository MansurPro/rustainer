#!/bin/bash
# Stage 12 success test - `exec`: a second process inside a live container.
#
#   sudo bash scripts/stage12-success-test.sh
#   unshare --user --map-root-user bash scripts/stage12-success-test.sh
#
# Every stage before this one CREATES namespaces. This one JOINS them, and
# the failure mode of a join is not a crash - it is a process that runs
# slightly outside the container and looks fine. So almost nothing below
# checks that a command printed the right thing. It checks that the second
# process is in the SAME namespace instance as the first (inode identity
# through /proc/<pid>/ns), under the SAME cgroup limits, with the SAME
# bounding set and seccomp mode, looking at the SAME live filesystem -
# including what the container has written since it started, which has no
# pathname on the host at all.
#
# The namespace the test cares about most is the one nobody thinks about:
# the mount namespace is joined by the CHILD, not the parent, because
# setns(CLONE_NEWNS) repoints the caller's root and cwd and would take
# /proc/self/stat away from the parent that still has to read it.

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

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage12.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
export RUSTAINER_IMAGE_ROOT="$SCRATCH/images"
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
CACHE="${TMPDIR:-/tmp}/rustainer-alpine-$ARCH.tar.gz"
if [ ! -s "$CACHE" ]; then
    ALPINE="https://dl-cdn.alpinelinux.org/alpine/v3.21/releases/$ARCH"
    NAME=$(curl -fsS "$ALPINE/" | grep -o "alpine-minirootfs-[0-9.]*-$ARCH\.tar\.gz" | sort -uV | tail -1)
    curl -fsS -o "$CACHE" "$ALPINE/$NAME" || exit 2
fi
BASE="$SCRATCH/base"; mkdir -p "$BASE"
tar --same-owner -xzf "$CACHE" -C "$BASE" 2>/dev/null || tar --no-same-owner -xzf "$CACHE" -C "$BASE" || exit 2

# A container that writes a marker into its own root and then waits. The
# marker is the point: it lives in the overlay's upper layer, inside the
# container's mount namespace, and has no pathname on the host.
# stderr kept separate from stdout: on a host where some capability is
# missing the engine prints a degradation notice, and folding that into the
# id gives every later assertion a container that does not exist.
ID=$(timeout 60 "$B" run -d --network none --hostname execbox --workdir /tmp \
        --layer "$BASE" /bin/sh -c 'echo alive > /marker; sleep 120' 2>"$SCRATCH/run.err")
RC=$?
if [ $RC -ne 0 ] || [ -z "$ID" ]; then
    echo "could not start the container (rc=$RC):" >&2
    cat "$SCRATCH/run.err" >&2
    exit 2
fi
[ -s "$SCRATCH/run.err" ] && sed 's/^/  note: /' "$SCRATCH/run.err"
sleep 1
INIT=$("$B" ps | awk -v id="$ID" '$1 == id {print $3}')
[ -n "$INIT" ] && [ "$INIT" != "-" ] || { echo "container has no init pid" >&2; exit 2; }
echo "container $ID, init pid $INIT"

echo
echo "1. It runs at all, and the command's own status comes back"
check "exec true exits 0" \
    "$(timeout 30 "$B" exec "$ID" /bin/true >/dev/null 2>&1; echo $?)" "0"
check "exec sh -c 'exit 7' exits 7" \
    "$(timeout 30 "$B" exec "$ID" /bin/sh -c 'exit 7' >/dev/null 2>&1; echo $?)" "7"
check "a command killed by SIGKILL reports 137" \
    "$(timeout 30 "$B" exec "$ID" /bin/sh -c 'kill -9 $$' >/dev/null 2>&1; echo $?)" "137"
check "stdout comes back" \
    "$(timeout 30 "$B" exec "$ID" /bin/echo hello 2>/dev/null)" "hello"
check "the command is found on the container's PATH without an absolute path" \
    "$(timeout 30 "$B" exec "$ID" echo found 2>/dev/null)" "found"

echo
echo "2. The SAME namespace instances, by inode, not by resemblance"
# A fresh namespace of the same kind would pass every behavioural test and
# fail this one. That is the whole reason to compare inodes.
for NS in pid uts ipc mnt net; do
    WANT=$(readlink "/proc/$INIT/ns/$NS" 2>/dev/null)
    GOT=$(timeout 30 "$B" exec "$ID" readlink "/proc/self/ns/$NS" 2>/dev/null)
    if [ -z "$WANT" ]; then
        skip "cannot read the container's $NS namespace link"
    else
        check "same $NS namespace as the container's init" "$GOT" "$WANT"
    fi
done
# And a namespace we are NOT supposed to share: the exec'd process must not
# be in the host's pid namespace.
HOSTPID=$(readlink /proc/self/ns/pid)
GOT=$(timeout 30 "$B" exec "$ID" readlink /proc/self/ns/pid 2>/dev/null)
[ "$GOT" != "$HOSTPID" ] && ok "it is not in the host's pid namespace" \
    || bad "it stayed in the host's pid namespace"

echo
echo "3. It is inside the pid namespace, not merely beside it"
# setns(CLONE_NEWPID) does not move the caller - it decides where the
# caller's next child goes. If the fork were missing, this would be a host
# pid in the hundreds of thousands and /proc/1 would be the host's init.
# busybox sh tail-execs the last command of a `-c` script, so the name
# here is `sleep` and not `sh`. That is the container's init either way.
check "the container's init is PID 1 from in there" \
    "$(timeout 30 "$B" exec "$ID" /bin/sh -c 'cat /proc/1/comm' 2>/dev/null)" "sleep"
PIDIN=$(timeout 30 "$B" exec "$ID" /bin/sh -c 'echo $$' 2>/dev/null)
[ -n "$PIDIN" ] && [ "$PIDIN" -lt 1000 ] 2>/dev/null \
    && ok "its own pid is namespace-local ($PIDIN)" \
    || bad "its own pid looks like a host pid ($PIDIN)"

echo
echo "4. The LIVE filesystem, including what the container wrote since boot"
# /marker exists only in the overlay's upper layer, inside the container's
# mount namespace. Resolving the command against the image on disk - which
# is what the `run` path does - would never see it.
check "a file the container created is visible" \
    "$(timeout 30 "$B" exec "$ID" /bin/cat /marker 2>/dev/null)" "alive"
# And the reverse: the host's filesystem is not.
timeout 30 "$B" exec "$ID" /bin/cat "$SCRATCH/base/etc/alpine-release" >/dev/null 2>&1 \
    && bad "a host path resolved from inside the container" \
    || ok "host paths do not resolve inside the container"

echo
echo "5. The UTS namespace is the container's, not a copy"
check "it reads the container's hostname" \
    "$(timeout 30 "$B" exec "$ID" /bin/cat /proc/sys/kernel/hostname 2>/dev/null)" "execbox"

echo
echo "6. The working directory is the container's, never the caller's"
# A child that does not chdir keeps the directory the CLI was run in, which
# is a live reference into the host filesystem from inside the container.
check "it starts in the container's workdir" \
    "$(timeout 30 "$B" exec "$ID" /bin/pwd 2>/dev/null)" "/tmp"
check "-w overrides it" \
    "$(timeout 30 "$B" exec -w /etc "$ID" /bin/pwd 2>/dev/null)" "/etc"

echo
echo "7. Environment: the container's own, with --env winning"
check "the container's PATH is inherited" \
    "$(timeout 30 "$B" exec "$ID" /bin/sh -c 'echo $PATH' 2>/dev/null | grep -c /usr/bin)" "1"
check "--env adds an entry" \
    "$(timeout 30 "$B" exec -e GREETING=hi "$ID" /bin/sh -c 'echo $GREETING' 2>/dev/null)" "hi"
ENVOUT=$(timeout 30 "$B" exec -e PATH=/zzz "$ID" /bin/busybox env 2>/dev/null)
check "--env overrides rather than duplicating" "$(echo "$ENVOUT" | grep -c '^PATH=')" "1"
check "and the override is the value that survives" \
    "$(echo "$ENVOUT" | grep '^PATH=')" "PATH=/zzz"

echo
echo "8. The same security posture, read back from inside"
BND=$(timeout 30 "$B" exec "$ID" /bin/sh -c 'grep CapBnd /proc/self/status' 2>/dev/null | awk '{print $2}')
INITBND=$(grep CapBnd "/proc/$INIT/status" 2>/dev/null | awk '{print $2}')
if [ -n "$BND" ] && [ -n "$INITBND" ]; then
    check "the bounding set matches the container's init" "$BND" "$INITBND"
else
    skip "could not read both bounding sets"
fi
SEC=$(timeout 30 "$B" exec "$ID" /bin/sh -c 'grep Seccomp: /proc/self/status' 2>/dev/null | awk '{print $2}')
check "seccomp is in filter mode (2)" "${SEC:-none}" "2"
NNP=$(timeout 30 "$B" exec "$ID" /bin/sh -c 'grep NoNewPrivs /proc/self/status' 2>/dev/null | awk '{print $2}')
check "no_new_privs is set" "${NNP:-none}" "1"

echo
echo "9. --user is answered against the CONTAINER's map, not ours"
# `exec` joins a user namespace rather than creating one, so neither of the
# two maps `run` consults is the right file: ours describes a namespace the
# command will not run in, and there is no map we are about to write. Only
# /proc/<init>/uid_map answers it - and getting this wrong is a `setresuid`
# EINVAL from a process that can no longer allocate a message.
if [ "$IN_USERNS" = yes ]; then
    # A single-ID namespace maps exactly one id, so 1000 is not reachable
    # and the engine has to say so rather than fail in the child.
    OUT=$(timeout 30 "$B" exec -u 1000:1000 "$ID" /bin/true 2>&1); RC=$?
    [ "$RC" -ne 0 ] && echo "$OUT" | grep -q "not mapped" \
        && ok "an unmapped uid is refused with the reason" \
        || bad "an unmapped uid was not refused clearly: $OUT"
    check "the id that IS mapped still works" \
        "$(timeout 30 "$B" exec -u 0:0 "$ID" /bin/sh -c 'id -u' 2>/dev/null)" "0"
else
    check "it runs as the requested uid" \
        "$(timeout 30 "$B" exec -u 1000:1000 "$ID" /bin/sh -c 'id -u' 2>/dev/null)" "1000"
    check "and the requested gid" \
        "$(timeout 30 "$B" exec -u 1000:1000 "$ID" /bin/sh -c 'id -g' 2>/dev/null)" "1000"
fi

echo
echo "10. The same cgroup, and therefore the same limits"
# CLONE_INTO_CGROUP places the exec'd process at clone time, so there is no
# window in which it runs outside the container's limits. The path is read
# back from /proc/<init>/cgroup rather than rebuilt from the container id,
# because `exec` may run from a different login session than `run` did.
LID=$(timeout 60 "$B" run -d --network none -m 64m --layer "$BASE" /bin/sh -c 'sleep 120' 2>"$SCRATCH/lim.err")
LRC=$?
if [ "$LRC" -eq 0 ] && [ -n "$LID" ] && "$B" ps | grep -q "^$LID"; then
    check "the limit is visible from inside the exec'd process" \
        "$(timeout 30 "$B" exec "$LID" /bin/cat /sys/fs/cgroup/memory.max 2>/dev/null)" "67108864"
    check "its cgroup view is rooted at the container's own" \
        "$(timeout 30 "$B" exec "$LID" /bin/cat /proc/self/cgroup 2>/dev/null)" "0::/"
    LINIT=$("$B" ps | awk -v id="$LID" '$1 == id {print $3}')
    WANT=$(awk -F: '/^0::/{print $3}' "/proc/$LINIT/cgroup" 2>/dev/null)
    # Counted from the host, where the path is the full one: a
    # /proc/<pid>/cgroup read reports relative to the READER's cgroup
    # namespace, which is the trap this code had to avoid.
    [ -n "$WANT" ] && ok "the container's cgroup path is readable from the host ($WANT)" \
        || skip "no cgroup path for the limited container"
    timeout 60 "$B" rm --force "$LID" >/dev/null 2>&1
else
    skip "limits not exercised here (rc=$LRC): $(tail -1 "$SCRATCH/lim.err")"
fi

echo
echo "11. Refusals say what is wrong"
OUT=$(timeout 30 "$B" exec nosuchcontainer /bin/true 2>&1); RC=$?
[ "$RC" -ne 0 ] && ok "exec into an unknown container fails" \
    || bad "exec into an unknown container succeeded"
OUT=$(timeout 30 "$B" exec "$ID" /bin/nosuchcommand 2>&1); RC=$?
[ "$RC" -ne 0 ] && ok "a missing absolute path fails" \
    || bad "a missing absolute path succeeded"
OUT=$(timeout 30 "$B" exec "$ID" nosuchcommand 2>&1); RC=$?
[ "$RC" -ne 0 ] && echo "$OUT" | grep -qi "PATH" \
    && ok "a command missing from PATH says so" \
    || bad "a command missing from PATH did not say so: $OUT"

echo
echo "12. A stopped container cannot be exec'd into"
timeout 60 "$B" stop -t 3 "$ID" >/dev/null 2>&1
sleep 0.5
OUT=$(timeout 30 "$B" exec "$ID" /bin/true 2>&1); RC=$?
[ "$RC" -ne 0 ] && echo "$OUT" | grep -qi "not running" \
    && ok "exec into a stopped container says it is not running" \
    || bad "exec into a stopped container did not refuse clearly: $OUT"

echo
echo "13. Nothing is left running afterwards"
check "no container is still running" "$("$B" ps | tail -n +2 | grep -c .)" "0"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
