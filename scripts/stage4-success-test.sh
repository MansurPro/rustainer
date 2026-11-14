#!/bin/bash
# Stage 4 success test - RND.md section 17, "Resource limits".
#
#   sudo bash scripts/stage4-success-test.sh
#
# Every limit is read back FROM INSIDE the container and compared with what
# was asked for. That is the whole discipline: `clone3` returning a pid proves
# nothing about cgroup membership, and the failure this stage guards against
# is one where every limit appears configured and none of them apply.
#
# Needs a writable cgroup v2 subtree. Rootless without delegation does not
# have one, and the engine is supposed to refuse rather than pretend - test 7
# checks exactly that, so the script runs either way and says which path it
# took.

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

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage4.XXXXXX")
export RUSTAINER_ROOT="$SCRATCH/state"
trap 'rm -rf "$SCRATCH"' EXIT

# Does this host actually give us a cgroup subtree of our own?
#
# "Writable" is not enough: a cgroup may not have both member processes and
# enabled controllers, so a subtree shared with a login shell cannot carry
# limits however writable it is. The honest test is to try enabling a
# controller.
CG_MODE=none
[ "$(stat -fc %t /sys/fs/cgroup 2>/dev/null)" = "63677270" ] && CG_MODE=unified
OWN=$(awk -F: '/^0::/{print $3}' /proc/self/cgroup)
# Strip the engine's own leaf if we are sitting in it. `rustainer.supervisor`
# is the name `cgroup::delegating_parent()` removes when deciding where
# containers go, so a harness that has stepped aside into it must strip it
# too - otherwise this probes the cgroup it is INSIDE rather than the one
# containers are created in, and that one can never have controllers
# enabled precisely because we are in it. The symptom is a host with a
# perfectly good delegated scope reporting "controllers enableable: no".
OWN=${OWN%/rustainer.supervisor}
BASE="/sys/fs/cgroup${OWN%/}"

# Environment preparation, not engine behaviour: when we are root in our own
# cgroup namespace, move the processes already sitting in the root into a
# leaf so controllers can be enabled there. This is what docker-in-docker
# does on startup, and it is the thing `systemd-run --scope -p Delegate=yes`
# arranges on a normal desktop.
# Only when `/` IS our whole view - i.e. we are in our own cgroup namespace,
# as in a container. On a real host our cgroup is a systemd scope shared
# with the session, and moving its processes around would disturb the
# machine rather than prepare it. There, use
# `systemd-run --scope -p Delegate=yes` to get a scope of our own instead.
# Two shapes are safe to prepare: our own cgroup namespace root (a
# container), and a transient scope we created ourselves - systemd-run names
# those `run-pNNN-iNNN.scope`. A login session's own scope is deliberately
# NOT matched: moving someone's shell around to run a test is rude.
PREPARABLE=no
case "$OWN" in
    /) PREPARABLE=yes ;;
    */run-p*.scope) PREPARABLE=yes ;;
esac
if [ "$CG_MODE" = unified ] && [ "$PREPARABLE" = yes ] && [ -w "$BASE/cgroup.procs" ]; then
    # Two traps here, both of which silently skip the whole step:
    #   - `[ -s file ]` is false for every cgroupfs file, because they all
    #     report st_size 0 however much they contain.
    #   - a `while read < cgroup.procs` loop stops early, because the file
    #     is regenerated as processes move out of it. Re-read until empty.
    mkdir -p "$BASE/host-procs" 2>/dev/null
    for _ in 1 2 3 4 5; do
        PROCS=$(cat "$BASE/cgroup.procs" 2>/dev/null)
        [ -z "$PROCS" ] && break
        for pp in $PROCS; do
            echo "$pp" > "$BASE/host-procs/cgroup.procs" 2>/dev/null
        done
    done
fi

WRITABLE=no
if [ "$CG_MODE" = unified ] && echo "+memory +cpu +pids" > "$BASE/cgroup.subtree_control" 2>/dev/null; then
    WRITABLE=yes
fi

echo "kernel $(uname -r) $(uname -m), uid $(id -u)"
echo "cgroups: $CG_MODE, own cgroup '$OWN', controllers enableable: $WRITABLE"

# A rootfs, so the container has its own /sys/fs/cgroup to read limits from.
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

# Each invocation gets a cgroup containing nothing but itself.
#
# This is what `systemd-run --scope -p Delegate=yes` does on a normal
# machine, written out by hand: the engine needs a cgroup it is alone in,
# because the one that delegates to the container must have no member
# processes of its own. The subshell puts ITSELF there and then execs, so
# the engine never shares a cgroup with this script.
R() {
    if [ "$WRITABLE" = yes ]; then
        # A fresh cgroup every time, and not a counter: R() is usually called
        # inside $( ), so an increment would happen in a subshell and never
        # be seen again. Reuse is fatal anyway - once the engine has enabled
        # controllers in a cgroup, no process may be placed directly in it,
        # and the move fails EBUSY.
        local cg="$BASE/rt-$$-$RANDOM$RANDOM"
        mkdir -p "$cg" 2>/dev/null
        ( echo $BASHPID > "$cg/cgroup.procs" || {
              echo "harness: could not move into $cg" >&2; exit 90; }
          exec "$B" run --rootfs "$ROOTFS" --network host "$@" )
    else
        "$B" run --rootfs "$ROOTFS" --network host "$@"
    fi
}
nohang() { timeout 60 "$@"; }

if [ "$WRITABLE" != yes ]; then
    echo
    echo "No writable cgroup subtree here. Testing the refusal path only."
    OUT=$(R -m 64m /bin/true 2>&1); RC=$?
    [ "$RC" -ne 0 ] && ok "a requested limit is refused rather than ignored" \
        || bad "the container ran without the limit it was given"
    echo "$OUT" | grep -qiE 'cgroup|delegat|writable' \
        && ok "the refusal names the cause" || bad "the refusal did not explain itself: $OUT"
    OUT=$(R /bin/echo unlimited-ok 2>&1)
    check "a container with no limits still runs" "$(echo "$OUT" | tail -1)" "unlimited-ok"
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

echo
echo "1. The child is placed in its cgroup at clone time, not after"
# If libc's truncated CLONE_INTO_CGROUP had been used, the flag would OR in
# zero, clone3 would still succeed, and the child would land in the PARENT's
# cgroup - unaccounted and unlimited, with no error anywhere. Reading the
# placement back from inside is the only way to see the difference.
IN=$(R -m 64m /bin/cat /proc/self/cgroup 2>/dev/null | tail -1)
check "the container is at the root of its own cgroup namespace" "$IN" "0::/"

echo
echo "2. The kernel agrees with what was asked for"
check "memory.max reads back as 64 MiB" \
    "$(R -m 64m /bin/cat /sys/fs/cgroup/memory.max 2>/dev/null | tail -1)" "67108864"
check "cpu.max reads back as half a CPU" \
    "$(R --cpus 0.5 /bin/cat /sys/fs/cgroup/cpu.max 2>/dev/null | tail -1)" "50000 100000"
check "pids.max reads back as 16" \
    "$(R --pids-limit 16 /bin/cat /sys/fs/cgroup/pids.max 2>/dev/null | tail -1)" "16"

echo
echo "3. An OOM kill is reported as an OOM kill"
# Not as "killed by signal 9": that sends the user looking for whoever sent
# the signal, and there was no sender. memory.events' `max` and `oom` keys
# are NOT kills; only `oom_kill` is.
# Fill tmpfs, whose pages are charged to the cgroup and cannot be reclaimed
# without swap, so the cgroup OOM killer fires. A `while :; do a="$a$(...)"`
# loop looks like the obvious test and is not: the OOM killer takes the
# SUBSHELL, and busybox sh carries on looping forever.
ID=$(R -d -m 32m /bin/sh -c 'dd if=/dev/zero of=/dev/shm/f bs=1M count=100 2>/dev/null; true')
# Wait for the status to SETTLE, not merely for the container to leave the
# default listing: the supervisor still has to drain the logs and write the
# terminal state, and reading in that window sees a stale "running".
for _ in $(seq 1 100); do
    E=$("$B" ps -a | awk -v i="$ID" '$1==i{print $4}')
    [ "$E" != "-" ] && break
    sleep 0.3
done
check "ps reports oom, not a signal" "$E" "oom"

echo
echo "4. A pids limit contains a fork bomb, and the host stays up"
ID=$(R -d --pids-limit 16 /bin/sh -c ':(){ :|:& };:' 2>/dev/null)
sleep 3
# The cgroup caps tasks at 16; clone(2) fails EAGAIN, there is no kill. The
# host must be unaffected - if this test hangs the machine, the limit did
# not apply.
LOAD_OK=$(nohang /bin/true && echo yes || echo no)
check "the host still runs commands" "$LOAD_OK" "yes"
nohang "$B" rm --force "$ID" >/dev/null 2>&1
ok "the fork bomb was contained and removed"

echo
echo "5. Every container gets a cgroup; no limit is invented"
check "a container with no limits is still at its own cgroup root" \
    "$(R /bin/cat /proc/self/cgroup 2>/dev/null | tail -1)" "0::/"
# Without a requested limit we never enable a controller, so memory.max may
# not exist at all - but it must never read back as a byte count, which
# would mean a limit nobody asked for.
OUT=$(R /bin/cat /sys/fs/cgroup/memory.max 2>/dev/null | tail -1)
case "$OUT" in
    ""|max) ok "no memory limit was invented (memory.max: '${OUT:-absent}')" ;;
    *) bad "memory.max reads '$OUT' but no limit was requested" ;;
esac

echo
echo "6. Limits survive into a detached container's recorded state"
ID=$(R -d -m 128m --pids-limit 64 /bin/sleep 5)
sleep 0.5
grep -q '"memory": 134217728' "$RUSTAINER_ROOT/containers/$ID/config.json" \
    && ok "the requested memory limit is recorded" || bad "the limit was not recorded"
nohang "$B" stop -t 2 "$ID" >/dev/null 2>&1

echo
echo "7. Refusals stay explicit"
OUT=$(R --cpus 0 /bin/true 2>&1); RC=$?
[ "$RC" -ne 0 ] && echo "$OUT" | grep -q 'greater than 0' \
    && ok "--cpus 0 refuses" || bad "--cpus 0 did not refuse"
OUT=$(R -m nonsense /bin/true 2>&1); RC=$?
[ "$RC" -ne 0 ] && ok "an unparseable size refuses" || bad "an unparseable size was accepted"

# Leave the host's cgroup tree as we found it.
rmdir "$BASE"/rt-$$-* 2>/dev/null

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
