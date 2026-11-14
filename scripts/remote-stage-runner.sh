#!/bin/bash
# Run one stage success test on a remote host, inside whatever cgroup scope
# the caller arranged.
#
#   remote-stage-runner.sh <dir> <stage> wrap|nowrap
#
# A separate file and not an inline `bash -c` because of the one thing it
# does besides exec'ing the test:
#
# cgroup v2's no-internal-process rule forbids a cgroup from having both
# member processes and enabled controllers. A delegated scope therefore is
# not enough on its own - the scope's member process is this shell, and the
# engine cannot enable the memory controller on a cgroup that contains it.
# The engine moves ITSELF aside (cgroup::enter_own_leaf) but it has no
# business moving a shell it does not own, so without the three lines below
# every limit is refused and the resource stages quietly exercise the
# refusal path instead of the enforcement path - passing, while testing
# much less than they appear to.
#
# The leaf is named `rustainer.supervisor` deliberately: that is the name
# `cgroup::delegating_parent()` strips when deriving where containers go,
# so containers are created as the scope's children rather than as this
# shell's, and the scope above them stays empty of processes.
set -u
DIR=$1
STAGE=$2
WRAP=$3

rel=$(awk -F: '/^0::/{print $3}' /proc/self/cgroup 2>/dev/null)
if [ -n "$rel" ] && [ "$rel" != / ]; then
    leaf="/sys/fs/cgroup$rel/rustainer.supervisor"
    mkdir -p "$leaf" 2>/dev/null && echo $$ > "$leaf/cgroup.procs" 2>/dev/null
fi

cd "$DIR" || exit 2
export RUSTAINER_BIN="$DIR/rustainer"
export TMPDIR="$DIR/tmp"
TEST="scripts/stage${STAGE}-success-test.sh"

# Stage 11 is the one that must NOT be wrapped: the engine makes its own
# user namespace, and run inside `unshare` it would find CAP_SYS_ADMIN
# already present, never create one, and pass without testing anything.
if [ "$WRAP" = wrap ]; then
    exec unshare --user --map-root-user bash "$TEST"
fi
exec bash "$TEST"
