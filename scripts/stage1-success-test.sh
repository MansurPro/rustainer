#!/bin/bash
# Stage 1 success test - RND.md section 17, "A process in namespaces of its own".
#
# Requires Linux and root (CLONE_NEWPID needs CAP_SYS_ADMIN). On a macOS or
# Windows workstation, run it in a throwaway Linux VM:
#
#   docker run --rm --privileged -v "$PWD":/w -w /w rust:1.96-slim \
#       bash scripts/stage1-success-test.sh
#
# Docker here is a VM substitute, not part of the implementation. Everything
# under test is our own clone3 call.
#
# Every assertion reads kernel state back from inside the container. None of
# them check that a command printed the right thing - that is the rule the
# whole ladder is built on (RND.md section 1.2), because the failures that
# matter are the ones where the operation succeeds and the guarantee is absent.

set -u
cd "$(dirname "$0")/.." || exit 1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/rustainer-target}"

PASS=0
FAIL=0
ok()   { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad()  { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check(){ if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }

if [ "$(uname -s)" != "Linux" ]; then
    echo "This test needs a Linux kernel; see the header for the one-liner." >&2
    exit 2
fi
if [ "$(id -u)" != "0" ]; then
    echo "This test needs root: CLONE_NEWPID requires CAP_SYS_ADMIN." >&2
    exit 2
fi

echo "kernel $(uname -r) $(uname -m), uid $(id -u)"
# CI builds as the unprivileged runner and passes the binary in, because
# rustup has no default toolchain for root and installing one just to build
# is slower than not needing to.
B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
if [ ! -x "$B" ]; then
    echo "no rustainer binary at '$B'" >&2
    exit 2
fi

HOST_NS=$(readlink /proc/self/ns/pid)
HOST_IPC=$(readlink /proc/self/ns/ipc)
HOST_HN=$(hostname)

# The direct child of a pid, without procps: ppid is the second field after
# the last ')' in /proc/<pid>/stat. Split on the LAST ')', because a process
# can be named "foo) 0 R".
childof() {
    for d in /proc/[0-9]*; do
        [ -r "$d/stat" ] || continue
        if [ "$(sed 's/.*) //' "$d/stat" | cut -d' ' -f2)" = "$1" ]; then
            basename "$d"
            return
        fi
    done
}

echo
echo "1. The container is in namespaces of its own"
OUT=$("$B" run --rootfs / --network host --hostname boxy /bin/sh -c \
    'echo "pid=$$"; echo "hn=$(hostname)"; echo "pidns=$(readlink /proc/self/ns/pid)"; echo "ipcns=$(readlink /proc/self/ns/ipc)"')
check "init is PID 1 inside"        "$(echo "$OUT" | sed -n 's/^pid=//p')"   "1"
check "hostname set in the UTS ns"  "$(echo "$OUT" | sed -n 's/^hn=//p')"    "boxy"
GOT_NS=$(echo "$OUT" | sed -n 's/^pidns=//p')
GOT_IPC=$(echo "$OUT" | sed -n 's/^ipcns=//p')
[ "$GOT_NS"  != "$HOST_NS"  ] && ok "pid namespace differs from the host's" || bad "pid namespace is the host's"
[ "$GOT_IPC" != "$HOST_IPC" ] && ok "ipc namespace differs from the host's" || bad "ipc namespace is the host's"
check "the host's hostname is untouched" "$(hostname)" "$HOST_HN"

echo
echo "2. Exit status is collected correctly"
"$B" run --rootfs / --network host /bin/true          >/dev/null 2>&1; check "normal exit"   "$?" "0"
"$B" run --rootfs / --network host /bin/sh -c 'exit 42' >/dev/null 2>&1; check "non-zero exit" "$?" "42"

echo
echo "3. Signal death is reported as signal death, not as an exit code"
# The signal must come from an ancestor namespace. See test 4 for why.
"$B" run --rootfs / --network host /bin/sleep 20 >/dev/null 2>&1 & RP=$!
sleep 0.5; C=$(childof "$RP")
if [ -n "$C" ]; then
    kill -KILL "$C"; wait "$RP"; check "SIGKILL from an ancestor -> 128+9" "$?" "137"
else
    bad "could not find the container's init process"
fi

echo
echo "4. PID 1 semantics - the part that surprises people"
# A namespace's init has no default disposition: sig_task_ignored() drops any
# signal whose handler is SIG_DFL unless the sender is in an ancestor
# namespace. `stop` therefore cannot assume that signalling means leaving,
# which is why stage 3's grace period is real rather than decorative.
"$B" run --rootfs / --network host /bin/sleep 20 >/dev/null 2>&1 & RP=$!
sleep 0.5; C=$(childof "$RP")
if [ -n "$C" ]; then
    kill -TERM "$C"; sleep 0.5
    if kill -0 "$C" 2>/dev/null; then
        ok "PID 1 ignores SIGTERM with no handler installed"
    else
        bad "PID 1 died of SIGTERM - the no-default-disposition rule did not hold"
    fi
    kill -KILL "$C" 2>/dev/null; wait "$RP" 2>/dev/null
else
    bad "could not find the container's init process"
fi

# Stronger, and not in the original research: SIGNAL_UNKILLABLE means even
# SIGKILL is dropped when it originates *inside* the namespace with SIG_DFL.
# A container cannot kill its own init. `stop` must signal from the
# supervisor (an ancestor) or use cgroup.kill - there is no in-container
# fallback.
"$B" run --rootfs / --network host /bin/sh -c 'kill -KILL 1; exit 7' >/dev/null 2>&1
check "PID 1 cannot SIGKILL itself from inside" "$?" "7"

echo
echo "5. Teaching moment, not a bug: /proc has not been remounted"
# `ps` inside still lists the host's processes, because /proc is the host's
# procfs mount. Stage 2 fixes it. Observing it is the point.
N=$("$B" run --rootfs / --network host /bin/sh -c 'ls /proc | grep -c "^[0-9][0-9]*$"')
if [ "$N" -gt 1 ]; then
    ok "/proc still shows $N host processes (stage 2 remounts it)"
else
    bad "/proc showed $N pids; expected the host's, since it was never remounted"
fi

echo
echo "6. Unsupported options refuse, and say what would support them"
"$B" run --rootfs /tmp --network host /bin/true 2>&1 | grep -q 'stage 2'  && ok "--rootfs <dir> refuses"   || bad "--rootfs <dir> did not refuse"
"$B" run --rootfs / --network bridge /bin/true 2>&1 | grep -q 'stage 9'   && ok "--network bridge refuses" || bad "--network bridge did not refuse"
"$B" run --rootfs / --network host sh 2>&1 | grep -q 'absolute path'      && ok "relative command refuses" || bad "relative command did not refuse"

echo
echo "7. A failure between clone3 and execve is reported with its step"
"$B" run --rootfs / --network host /bin/nonexistent 2>&1 \
    | grep -q 'executing the container command failed' && ok "execve failure names the step and errno" || bad "execve failure was not reported"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
