#!/bin/bash
# Stage 19 success test - pseudo-terminals.
#
#   sudo bash scripts/stage19-success-test.sh
#
# `-t` is not cosmetic. `isatty(1)` decides whether a shell prints a
# prompt at all, whether `ls` colours its output, whether `apt` draws a
# progress bar - and, through the controlling terminal, whether `^C`
# arrives as SIGINT or as the byte 0x03. A container whose stdio is a
# pipe is a container where an interactive shell does not work, however
# correct everything else is.
#
# The properties worth asserting cannot be observed from a shell
# pipeline, because a pipeline is exactly what `-t` is distinguishable
# from. `scripts/pty-drive.py` provides a terminal to be observed from:
# to check that a window size reaches the container, something has to
# HAVE a window.
#
# Where the pty is allocated is a deliberate trade, stated in
# crates/core/src/pty.rs: on the host, so the child does not have to
# build an SCM_RIGHTS message in the window where it may not allocate.
# The descriptors work exactly as they should; the slave's PATHNAME
# belongs to the host's devpts, so `ttyname` inside names a different
# terminal. Programs that ask "am I a terminal" are unaffected; programs
# that ask "which terminal am I" are.

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

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }
command -v python3 >/dev/null 2>&1 || {
    echo "needs python3: a terminal has to be provided to observe one" >&2
    exit 2
}

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage19.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=2g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
fi
export RUSTAINER_ROOT="$SCRATCH/state"
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

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"
try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

# Strip the CR a terminal adds to every line ending.
nocr() { tr -d '\r'; }
drive() { python3 scripts/pty-drive.py "$@"; }

echo
echo "1. Without -t the container's stdio is a pipe; with -t it is a terminal"
# The single distinction the whole stage exists for.
check "no -t: stdout is not a terminal" \
    "$(timeout 120 "$B" run --network none "$IMG" /bin/sh -c 'test -t 1 && echo tty || echo pipe' 2>&1 | tail -1)" \
    "pipe"
check "-t: stdout is a terminal" \
    "$(timeout 120 "$B" run -t --network none "$IMG" /bin/sh -c 'test -t 1 && echo tty || echo pipe' 2>&1 | nocr | tail -1)" \
    "tty"
check "-t: stdin is a terminal too" \
    "$(timeout 120 "$B" run -t --network none "$IMG" /bin/sh -c 'test -t 0 && echo tty || echo pipe' 2>&1 | nocr | tail -1)" \
    "tty"

echo
echo "2. A terminal merges the two output streams, because it is one device"
# Not a shortcut: stdout and stderr are the same file description, so a
# tty container has one log, exactly as Docker's does.
OUT=$(timeout 120 "$B" run -t --network none "$IMG" \
    /bin/sh -c 'echo to-stdout; echo to-stderr 1>&2' 2>&1 | nocr)
contains "stdout is there" "$OUT" "to-stdout"
contains "and so is stderr" "$OUT" "to-stderr"

echo
echo "3. The window size reaches the container"
# Through two pty layers: the one this test provides, and the one the
# engine allocates. Without it a shell believes it has 24x80 and wraps
# its lines in the wrong place, which looks like corruption.
ID=$(timeout 120 "$B" run -d --network none "$IMG" /bin/sleep 300 2>/dev/null)
[ -n "$ID" ] || { echo "could not start a container" >&2; exit 2; }
check "stty size inside matches the terminal outside" \
    "$(drive 40 123 -- "$B" exec -t "$ID" /bin/sh -c 'stty size' | nocr | tail -1)" \
    "40 123"

echo
echo "4. ^C becomes SIGINT inside, not the byte 0x03"
# This is what the controlling terminal buys, and it needs BOTH setsid
# and TIOCSCTTY in the child: without them the pty has no foreground
# process group and the kernel never generates the signal at all.
OUT=$(drive 24 80 --send '\x03' --settle 1.0 -- \
    "$B" exec -it "$ID" /bin/sh -c 'trap "echo caught-int" INT; sleep 5' | nocr)
contains "the container's trap fired" "$OUT" "caught-int"

echo
echo "5. An interactive shell is actually interactive"
OUT=$(printf 'echo hello-from-shell\nexit\n' \
    | timeout 120 "$B" exec -it "$ID" /bin/sh 2>&1 | nocr)
contains "it ran what was typed" "$OUT" "hello-from-shell"
contains "and it printed a prompt, which it does not do on a pipe" "$OUT" "/ #"

echo
echo "6. The exit status survives the terminal"
# A pty is a byte stream with no status in it, so the status has to come
# from waiting on the process. Easy to lose and silent when lost.
check "exec -t reports the command's status" \
    "$(timeout 120 "$B" exec -t "$ID" /bin/sh -c 'exit 7' >/dev/null 2>&1; echo $?)" "7"
check "run -t reports it too" \
    "$(timeout 120 "$B" run -t --network none "$IMG" /bin/sh -c 'exit 9' >/dev/null 2>&1; echo $?)" "9"

echo
echo "7. A detached container can have a terminal, and its output is logged"
# `run -dt` has no CLI to hold the master, which is why the SUPERVISOR
# allocates the pty rather than the CLI.
DID=$(timeout 120 "$B" run -dt --network none "$IMG" \
    /bin/sh -c 'test -t 1 && echo detached-tty; sleep 60' 2>/dev/null)
sleep 1.2
contains "the detached container had a terminal" "$("$B" logs "$DID" | nocr)" "detached-tty"
timeout 60 "$B" stop -t 0 "$DID" >/dev/null 2>&1

echo
echo "8. -t works when the CALLER has no terminal"
# Which is every CI runner, every cron job and every pipeline. The
# engine allocates its own pty; it does not need one to copy from.
check "run -t from a pipeline still works" \
    "$(echo | timeout 120 "$B" run -t --network none "$IMG" \
        /bin/sh -c 'test -t 1 && echo tty || echo pipe' < /dev/null 2>&1 | nocr | tail -1)" \
    "tty"

echo
echo "9. The caller's terminal is restored afterwards"
# Raw mode is entered by the CLI and restored by a Drop guard, because
# every path out - the command exiting, an error, a panic, the container
# being killed from another window - has to restore it. A shell left in
# raw mode is one the user has to `reset` blind.
BEFORE=$(drive 24 80 -- /bin/sh -c 'stty -g' | nocr | tail -1)
AFTER=$(drive 24 80 -- /bin/sh -c "$B exec -t $ID /bin/true >/dev/null 2>&1; stty -g" | nocr | tail -1)
if [ -n "$BEFORE" ] && [ "$BEFORE" = "$AFTER" ]; then
    ok "the terminal settings are unchanged after an interactive exec"
else
    bad "the terminal was left modified (before '$BEFORE', after '$AFTER')"
fi

echo
echo "10. Nothing is left behind"
timeout 60 "$B" stop -t 0 "$ID" >/dev/null 2>&1
check "no container is still running" "$("$B" ps | tail -n +2 | grep -c .)" "0"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
