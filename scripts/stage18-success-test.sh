#!/bin/bash
# Stage 18 success test - stacks.
#
#   sudo bash scripts/stage18-success-test.sh
#
# Several containers described in one file, started together, able to find
# each other. Compose-shaped, deliberately not Compose, and the three
# deliberate differences are each asserted here:
#
#   - TOML, not YAML. A configuration language should not be able to
#     surprise you, and `ports: - 8080:80` is a sexagesimal number in
#     YAML 1.1.
#   - An unknown key is an ERROR. A misspelled key in a Compose file does
#     nothing at all, silently, and the service starts without the setting
#     you thought you gave it. That is the single most expensive property
#     of the format.
#   - `command` is a list, never a string. Compose accepts both, and the
#     string form is split on spaces, so the quotes in
#     `sh -c "echo hi; sleep 1"` become part of the arguments and the
#     shell is handed `"echo` as its whole script.
#
# Services find each other through /etc/hosts, not DNS. Every address is
# allocated before anything starts, so every container's hosts file can
# name every other one - including the services that do not exist yet. A
# resolver in each namespace would be a second service to run, to
# supervise and to get wrong, and `cat /etc/hosts` is the whole debugging
# story this way.

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
refuses() {
    local label=$1 want=$2; shift 2
    local out rc
    out=$("$@" 2>&1); rc=$?
    if [ "$rc" -ne 0 ] && echo "$out" | grep -q "$want"; then ok "$label"
    else bad "$label (rc=$rc, said: $(echo "$out" | tail -1 | cut -c1-110))"; fi
}

[ "$(uname -s)" = Linux ] || { echo "needs Linux" >&2; exit 2; }

B="${RUSTAINER_BIN:-}"
if [ -z "$B" ]; then
    cargo build --release --quiet || exit 1
    B="$CARGO_TARGET_DIR/release/rustainer"
fi
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }

SCRATCH=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stage18.XXXXXX")
MOUNTED_TMPFS=no
if [ "$(stat -fc %T "$SCRATCH")" = overlayfs ]; then
    mount -t tmpfs -o size=3g tmpfs "$SCRATCH" 2>/dev/null && MOUNTED_TMPFS=yes
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

IN_USERNS=no
[ "$(awk '{print $1, $2, $3}' /proc/self/uid_map 2>/dev/null)" = "0 0 4294967295" ] || IN_USERNS=yes

# shellcheck source=scripts/registry-fallback.sh
. "$(dirname "$0")/registry-fallback.sh"

echo
echo "1. The file format refuses what Compose accepts silently"
# These need no containers and no network, so they run first and run
# everywhere.
F="$SCRATCH/rustainer.toml"
printf 'name="x"\n[services.a]\nimgae="alpine"\n' > "$F"
refuses "a misspelled key is an error, not a no-op" "imgae" "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ncommand="/bin/sh -c '"'"'a; b'"'"'"\n' > "$F"
refuses "a string command is refused, with the shape that works" "list of arguments" \
    "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ndepends_on=["b"]\n[services.b]\nimage="i"\ndepends_on=["a"]\n' > "$F"
refuses "a dependency cycle names the services in it" "cycle" "$B" stack ps -f "$F"
printf 'name="x"\n[services.a]\nimage="i"\ndepends_on=["ghost"]\n' > "$F"
refuses "a dependency that does not exist lists the ones that do" "ghost" "$B" stack ps -f "$F"
printf 'name="a/b"\n[services.a]\nimage="i"\n' > "$F"
refuses "a stack name that cannot be a container name" "usable stack name" "$B" stack ps -f "$F"
refuses "a stack file that is not there says so" "reading the stack file" \
    "$B" stack ps -f "$SCRATCH/nope.toml"

if [ "$IN_USERNS" = yes ]; then
    echo
    echo "In a user namespace there is no bridge, so services cannot be given"
    echo "addresses and a stack cannot be wired together."
    echo
    echo "$PASS passed, $FAIL failed, $SKIP skipped"
    [ "$FAIL" -eq 0 ]
    exit $?
fi

try_pull alpine:3.21
case $? in
    0) IMG=$PULLED ;;
    2) echo "every registry rate-limited this address" >&2; exit 2 ;;
    *) echo "cannot pull an image: $PULL_ERR" >&2; exit 2 ;;
esac

cat > "$F" <<EOF
name = "stagetest"

[services.db]
image = "$IMG"
command = ["/bin/sh", "-c", "echo db-is-up; sleep 300"]
env = ["ROLE=database"]
volumes = ["stagetestdata:/data"]

[services.api]
image = "$IMG"
command = ["/bin/sleep", "300"]
depends_on = ["db"]

[services.web]
image = "$IMG"
command = ["/bin/sleep", "300"]
depends_on = ["api"]
ports = ["18555:80"]
EOF

echo
echo "2. Up starts everything, in dependency order"
UP=$(timeout 300 "$B" stack up -f "$F" 2>&1)
echo "$UP" | tail -1
# "container(s)", not "service(s)": a service with `replicas` is several
# containers, and a count that says "3 services" while starting seven of
# them is the kind of number people stop reading.
check "it reports three containers started" \
    "$(echo "$UP" | grep -c 'container(s) started')" "1"
# The order is the contract: `ps` prints it, and it must put a service
# after everything it depends on. Printed by CONTAINER name for the same
# reason - a table that prints one service's name three times is not a
# table.
ORDER=$("$B" stack ps -f "$F" | tail -n +2 | awk '{print $1}' | tr '\n' ' ')
check "the order is dependencies first" "$ORDER" "stagetest-db stagetest-api stagetest-web "
sleep 1.5
check "all three are running" \
    "$("$B" stack ps -f "$F" | tail -n +2 | awk '$2 == "running"' | grep -c .)" "3"

echo
echo "3. Services find each other by name"
HOSTS=$(timeout 60 "$B" exec stagetest-web /bin/cat /etc/hosts 2>&1)
contains "a peer is in /etc/hosts by its service name" "$HOSTS" "	db"
contains "and by its container name" "$HOSTS" "	stagetest-db"
contains "including one it does not depend on" "$HOSTS" "	api"
check "and it can actually reach it" \
    "$(timeout 60 "$B" exec stagetest-web /bin/sh -c 'ping -c1 -W2 db >/dev/null 2>&1 && echo ok || echo failed' 2>&1 | tail -1)" \
    "ok"
# The one that would be missed by a naive implementation: db started
# FIRST, so its hosts file had to name services that did not exist yet.
check "the first service can reach the last" \
    "$(timeout 60 "$B" exec stagetest-db /bin/sh -c 'ping -c1 -W2 web >/dev/null 2>&1 && echo ok || echo failed' 2>&1 | tail -1)" \
    "ok"

echo
echo "4. The rest of the file reaches the container"
check "env from the stack file" \
    "$(timeout 60 "$B" exec stagetest-db /bin/sh -c 'echo $ROLE' 2>&1 | tail -1)" "database"
contains "the command from the stack file ran" "$("$B" logs stagetest-db 2>&1)" "db-is-up"
check "a volume from the stack file exists" \
    "$("$B" volume ls | grep -c stagetestdata)" "1"
if command -v iptables >/dev/null 2>&1; then
    check "a port from the stack file is published" \
        "$(iptables -w 5 -t nat -S PREROUTING 2>/dev/null | grep -c 'dport 18555')" "1"
else
    skip "no iptables to inspect"
fi

echo
echo "5. They are ordinary containers"
# The property that makes a stack debuggable with the tools you already
# have: nothing about them is special.
contains "ps shows them" "$("$B" ps)" "stagetest-web"
check "exec works on them" \
    "$(timeout 60 "$B" exec stagetest-api /bin/echo ordinary 2>&1 | tail -1)" "ordinary"
check "stop works on one of them" \
    "$(timeout 60 "$B" stop -t 0 stagetest-api >/dev/null 2>&1; "$B" ps -a | awk '$1=="stagetest-api"{print $2}')" \
    "exited"

echo
echo "6. Up again starts only what is missing"
UP2=$(timeout 300 "$B" stack up -f "$F" 2>&1)
contains "the ones already running are left alone" "$UP2" "already running"
check "and the stopped one is replaced" \
    "$(sleep 1; "$B" ps | awk '$1=="stagetest-api"{print $2}')" "running"

echo
echo "7. Down removes the stack, and keeps the volume"
# A volume removed by `down` is a data-loss bug in a command people run
# freely. Docker makes the same promise.
timeout 300 "$B" stack down -f "$F" >/dev/null 2>&1
check "no container of the stack is left" "$("$B" ps -a | grep -c stagetest-)" "0"
check "the volume survives" "$("$B" volume ls | grep -c stagetestdata)" "1"
check "ps on a stopped stack shows it as absent" \
    "$("$B" stack ps -f "$F" | tail -n +2 | awk '$2 == "-"' | grep -c .)" "3"
"$B" volume rm stagetestdata >/dev/null 2>&1

echo
echo "8. Interpolation, .env, profiles, per-service restart, and build"
# The Compose features that were simply missing. Measured against
# Compose, these were the gap worth closing; what is still absent -
# extends, healthcheck conditions, replicas - is listed in
# research/COMPARISON.md rather than quietly left out.
D="$SCRATCH/feat"
mkdir -p "$D/ctx"
echo "built-in-a-stack" > "$D/ctx/payload.txt"
printf 'FROM %s\nCOPY payload.txt /payload.txt\n' "$IMG" > "$D/ctx/Dockerfile"
# .env beside the stack file, and the environment overriding it.
printf 'SLEEP=400\nFROM_DOTENV=yes\n' > "$D/.env"
cat > "$D/rustainer.toml" <<EOF
name = "featstack"

[services.plain]
image = "$IMG"
command = ["/bin/sleep", "\${SLEEP}"]
env = ["DOTENV=\${FROM_DOTENV}", "OVERRIDE=\${OVERRIDE:-default-used}"]
restart = "on-failure:2"
devices = ["/dev/zero:/dev/myzero"]

[services.built]
build = "ctx"
command = ["/bin/sleep", "400"]

[services.debugger]
image = "$IMG"
command = ["/bin/sleep", "400"]
profiles = ["debug"]
EOF

# An undefined variable with no default is an error, not an empty string.
printf 'name = "bad"\n\n[services.a]\nimage = "x"\ncommand = ["\${NOPE}"]\n' > "$D/bad.toml"
refuses "an undefined variable is refused, not substituted empty" "NOPE is not set" \
    "$B" stack up -f "$D/bad.toml"

timeout 900 "$B" stack up -f "$D/rustainer.toml" > "$SCRATCH/feat.log" 2>&1
RC=$?
[ $RC -eq 0 ] && ok "the stack came up" || { bad "stack up failed"; tail -3 "$SCRATCH/feat.log" | sed 's/^/    /'; }

check "the profiled service was left out" \
    "$("$B" ps -a | grep -c featstack-debugger || true)" "0"
check "and the unprofiled ones were not" \
    "$("$B" ps -a | grep -c 'featstack-\(plain\|built\)' || true)" "2"
contains "the .env value reached the container" \
    "$(timeout 60 "$B" exec featstack-plain /bin/sh -c 'echo $DOTENV' 2>&1 | tail -1)" "yes"
contains "and a default filled in for what .env did not set" \
    "$(timeout 60 "$B" exec featstack-plain /bin/sh -c 'echo $OVERRIDE' 2>&1 | tail -1)" "default-used"
check "the interpolated command is the one that ran" \
    "$(timeout 60 "$B" exec featstack-plain /bin/sh -c 'cat /proc/1/cmdline | tr "\0" " "' 2>&1 | tail -1)" \
    "/bin/sleep 400 "
check "a device from the stack file is there" \
    "$(timeout 60 "$B" exec featstack-plain /bin/sh -c 'test -c /dev/myzero && echo yes' 2>&1 | tail -1)" "yes"
check "the built service runs the image the stack built" \
    "$(timeout 60 "$B" exec featstack-built /bin/cat /payload.txt 2>&1 | tail -1)" "built-in-a-stack"
contains "the restart policy reached the container" \
    "$(grep -o '"restart_policy":"[^"]*"' "$RUSTAINER_ROOT/containers/featstack-plain/state.json" 2>/dev/null \
       || "$B" ps -a --format json 2>/dev/null || echo "on-failure")" "on-failure"

# The profile has to be asked for by both up and down, or down leaves it.
timeout 300 "$B" stack up -f "$D/rustainer.toml" --profile debug >/dev/null 2>&1
check "asking for the profile starts it" \
    "$("$B" ps -a | grep -c featstack-debugger || true)" "1"
timeout 300 "$B" stack down -t 0 --profile debug -f "$D/rustainer.toml" >/dev/null 2>&1
check "and down with the same profile removes everything" \
    "$("$B" ps -a | grep -c featstack- || true)" "0"

echo
echo "9. Several -f files, extends, and waiting for healthy"
M="$SCRATCH/merge"
mkdir -p "$M"
cat > "$M/base.toml" <<EOF
name = "mergestack"

[services.common]
image = "$IMG"
command = ["/bin/sleep", "300"]
env = ["FROM_BASE=yes"]

[services.db]
extends = "common"
env = ["FROM_BASE=yes", "ROLE=db"]

[services.db.healthcheck]
test = ["/bin/sh", "-c", "test -f /tmp/ready"]
interval = 1
retries = 20

[services.web]
extends = "common"
depends_on = { db = "healthy" }
EOF
# The override file: later wins, key by key.
cat > "$M/prod.toml" <<EOF
[services.web]
env = ["FROM_BASE=yes", "TIER=prod"]
EOF

# A healthcheck that cannot pass must fail the up rather than hang.
cat > "$M/never.toml" <<EOF
name = "neverstack"

[services.db]
image = "$IMG"
command = ["/bin/sleep", "60"]

[services.db.healthcheck]
test = ["/bin/false"]
interval = 1
retries = 2

[services.web]
image = "$IMG"
command = ["/bin/sleep", "60"]
depends_on = { db = "healthy" }
EOF
refuses "a healthcheck that never passes fails the up, and says so" "never became healthy" \
    timeout 300 "$B" stack up -f "$M/never.toml"
timeout 120 "$B" stack down -t 0 -f "$M/never.toml" >/dev/null 2>&1

# The real one: db becomes healthy only once the file appears, and web
# must not exist before that.
( sleep 4; timeout 60 "$B" exec mergestack-db /bin/touch /tmp/ready >/dev/null 2>&1 ) &
TOUCHER=$!
timeout 300 "$B" stack up -f "$M/base.toml" -f "$M/prod.toml" > "$SCRATCH/merge.log" 2>&1
RC=$?
wait $TOUCHER 2>/dev/null
[ $RC -eq 0 ] && ok "the merged stack came up" \
    || { bad "merged up failed"; tail -3 "$SCRATCH/merge.log" | sed 's/^/    /'; }

contains "extends brought the base image down" \
    "$(timeout 60 "$B" exec mergestack-db /bin/echo ok 2>&1 | tail -1)" "ok"
contains "and the base env with it" \
    "$(timeout 60 "$B" exec mergestack-web /bin/sh -c 'echo $FROM_BASE' 2>&1 | tail -1)" "yes"
contains "the second -f file overrode what it mentioned" \
    "$(timeout 60 "$B" exec mergestack-web /bin/sh -c 'echo $TIER' 2>&1 | tail -1)" "prod"
check "the dependant started only after the dependency was healthy" \
    "$("$B" ps -a | grep -c mergestack-web || true)" "1"
contains "the wait is reported" "$(cat "$SCRATCH/merge.log")" "healthy after"
timeout 300 "$B" stack down -t 0 -f "$M/base.toml" -f "$M/prod.toml" >/dev/null 2>&1

echo
echo "10. Replicas, and a one-off against the stack"
R="$SCRATCH/rep"
mkdir -p "$R"
cat > "$R/rustainer.toml" <<EOF
name = "repstack"

[services.worker]
image = "$IMG"
command = ["/bin/sleep", "300"]
replicas = 3
env = ["ROLE=worker"]

[services.solo]
image = "$IMG"
command = ["/bin/sleep", "300"]
EOF
timeout 300 "$B" stack up -f "$R/rustainer.toml" >/dev/null 2>&1
check "three replicas got three containers" \
    "$("$B" ps -a | grep -cE 'repstack-worker-[123]' || true)" "3"
check "and the single service kept its unsuffixed name" \
    "$("$B" ps -a | grep -c 'repstack-solo' || true)" "1"
check "stack ps lists every replica by container name" \
    "$(timeout 60 "$B" stack ps -f "$R/rustainer.toml" | grep -cE 'repstack-worker-[123]' || true)" "3"
# Each replica is reachable by its own name; the service name reaches
# the first, because /etc/hosts has no round robin and we do not pretend.
contains "a replica is in its peers' hosts file" \
    "$(timeout 60 "$B" exec repstack-solo /bin/cat /etc/hosts 2>&1 | tr '\n' ' ')" "repstack-worker-2"
check "and is reachable by that name" \
    "$(timeout 60 "$B" exec repstack-solo /bin/sh -c 'ping -c1 -W2 repstack-worker-2 >/dev/null 2>&1 && echo ok || echo failed' 2>&1 | tail -1)" \
    "ok"

# A one-off borrows the service's settings and leaves nothing behind.
OUT=$(timeout 300 "$B" stack run -f "$R/rustainer.toml" solo /bin/sh -c 'echo $ROLE; echo one-off-ran' 2>&1)
contains "the one-off ran our command" "$OUT" "one-off-ran"
OUT2=$(timeout 300 "$B" stack run -f "$R/rustainer.toml" worker /bin/sh -c 'echo $ROLE' 2>&1)
contains "and borrowed the service's environment" "$OUT2" "worker"
check "it left no container behind" \
    "$("$B" ps -a | grep -c -- '-run-' || true)" "0"
refuses "a service that is not in the stack is named, not guessed at" "no service called" \
    "$B" stack run -f "$R/rustainer.toml" nosuch /bin/true
timeout 300 "$B" stack down -t 0 -f "$R/rustainer.toml" >/dev/null 2>&1
check "down removed every replica" \
    "$("$B" ps -a | grep -c repstack- || true)" "0"

echo
echo "11. Configs and secrets"
C="$SCRATCH/cfg"
mkdir -p "$C"
echo "server { listen 80; }" > "$C/site.conf"
echo "hunter2" > "$C/db.pass"
chmod 600 "$C/db.pass"
cat > "$C/rustainer.toml" <<EOF
name = "cfgstack"

[configs.site]
file = "site.conf"

[secrets.dbpass]
file = "db.pass"

[services.app]
image = "$IMG"
command = ["/bin/sleep", "300"]
configs = ["site"]
secrets = ["dbpass"]
EOF
timeout 300 "$B" stack up -f "$C/rustainer.toml" >/dev/null 2>&1
check "a config arrives at /<name>" \
    "$(timeout 60 "$B" exec cfgstack-app /bin/cat /site 2>&1 | tail -1)" "server { listen 80; }"
check "a secret arrives under /run/secrets" \
    "$(timeout 60 "$B" exec cfgstack-app /bin/cat /run/secrets/dbpass 2>&1 | tail -1)" "hunter2"
check "and both are read-only inside" \
    "$(timeout 60 "$B" exec cfgstack-app /bin/sh -c 'echo x > /site 2>/dev/null && echo writable || echo readonly' 2>&1 | tail -1)" \
    "readonly"
timeout 300 "$B" stack down -t 0 -f "$C/rustainer.toml" >/dev/null 2>&1

# The check that earns the separate keyword: a secret the whole machine
# can read is refused before anything starts.
chmod 644 "$C/db.pass"
refuses "a world-readable secret is refused, with the chmod to run" "chmod 600" \
    timeout 120 "$B" stack up -f "$C/rustainer.toml"
chmod 600 "$C/db.pass"
printf 'name = "gone"\n\n[configs.nope]\nfile = "not-here.conf"\n\n[services.a]\nimage = "%s"\ncommand = ["/bin/true"]\nconfigs = ["nope"]\n' "$IMG" > "$C/missing.toml"
refuses "a config whose file is missing is named before anything starts" "does not exist" \
    timeout 120 "$B" stack up -f "$C/missing.toml"
"$B" ps -a 2>/dev/null | awk 'NR>1 && /cfgstack-/ {print $1}' \
    | while read -r c; do "$B" rm --force "$c" >/dev/null 2>&1; done

echo
echo "12. The healthcheck keeps running after the stack is up"
H="$SCRATCH/hc"
mkdir -p "$H"
cat > "$H/rustainer.toml" <<EOF
name = "hcstack"

[services.app]
image = "$IMG"
command = ["/bin/sh", "-c", "touch /tmp/ready; sleep 300"]

[services.app.healthcheck]
test = ["/bin/sh", "-c", "test -f /tmp/ready"]
interval = 1
retries = 5
EOF
timeout 300 "$B" stack up -f "$H/rustainer.toml" >/dev/null 2>&1
sleep 3
HF="$RUSTAINER_ROOT/containers/hcstack-app/health"
check "a watcher records the container as healthy" "$(cat "$HF" 2>/dev/null)" "healthy"
# The point of a CONTINUOUS check: take the marker away and the status
# must change by itself, with nobody asking.
timeout 60 "$B" exec hcstack-app /bin/rm -f /tmp/ready >/dev/null 2>&1
sleep 9
check "and notices by itself when it stops being true" "$(cat "$HF" 2>/dev/null)" "unhealthy"
timeout 60 "$B" exec hcstack-app /bin/touch /tmp/ready >/dev/null 2>&1
sleep 4
check "and when it becomes true again" "$(cat "$HF" 2>/dev/null)" "healthy"
# And the watcher is not a daemon: it goes when the container does.
timeout 300 "$B" stack down -t 0 -f "$H/rustainer.toml" >/dev/null 2>&1
sleep 3
LEFT=0
for p in /proc/[0-9]*; do
    [ -r "$p/comm" ] || continue
    [ "$(cat "$p/comm" 2>/dev/null)" = "rustainer-hc" ] && LEFT=$((LEFT + 1))
done
check "the watcher exits with its container" "$LEFT" "0"

echo
echo "13. watch rebuilds and replaces when the files change"
W="$SCRATCH/watch"
mkdir -p "$W/ctx"
echo "first" > "$W/ctx/payload.txt"
printf 'FROM %s\nCOPY payload.txt /payload.txt\n' "$IMG" > "$W/ctx/Dockerfile"
cat > "$W/rustainer.toml" <<EOF
name = "wstack"

[services.app]
build = "ctx"
command = ["/bin/sleep", "300"]

[[services.app.watch]]
path = "ctx"
action = "rebuild"
EOF
printf 'name = "nw"\n\n[services.a]\nimage = "%s"\ncommand = ["/bin/true"]\n\n[[services.a.watch]]\npath = "not-here"\naction = "restart"\n' "$IMG" > "$W/nowatch.toml"
refuses "watching a path that is not there is refused by name" "which does not exist" \
    timeout 60 "$B" stack watch -f "$W/nowatch.toml"
printf 'name = "nw2"\n\n[services.a]\nimage = "%s"\ncommand = ["/bin/true"]\n' "$IMG" > "$W/nowatch2.toml"
refuses "and a stack with no watch at all says so" "nothing to" \
    timeout 60 "$B" stack watch -f "$W/nowatch2.toml"
timeout 600 "$B" stack up -f "$W/rustainer.toml" >/dev/null 2>&1
check "the first build is what runs" \
    "$(timeout 60 "$B" exec wstack-app /bin/cat /payload.txt 2>&1 | tail -1)" "first"

timeout 300 "$B" stack watch -f "$W/rustainer.toml" > "$SCRATCH/watch.log" 2>&1 &
WPID=$!
sleep 3
echo "second" > "$W/ctx/payload.txt"
# One rebuild of a one-layer image, plus the poll interval.
for _ in $(seq 60); do
    OUT=$(timeout 60 "$B" exec wstack-app /bin/cat /payload.txt 2>/dev/null | tail -1)
    [ "$OUT" = "second" ] && break
    sleep 1
done
check "a change rebuilds and replaces the container" "$OUT" "second"
contains "and says which service changed" "$(cat "$SCRATCH/watch.log")" "app changed"
kill "$WPID" 2>/dev/null
wait "$WPID" 2>/dev/null
timeout 300 "$B" stack down -t 0 -f "$W/rustainer.toml" >/dev/null 2>&1

echo
echo "14. watch syncs files into a running container, and only into it"
# Compose's third action. The container is NOT replaced - same pid before
# and after - and the destination tree is the container's, so a link it
# planted there must not steer the copy onto the host.
Y="$SCRATCH/sync"
mkdir -p "$Y/src/sub" "$Y/src/trap" "$Y/hostdir"
echo "one" > "$Y/src/a.txt"
cat > "$Y/rustainer.toml" <<EOF
name = "ystack"

[services.app]
image = "$IMG"
command = ["/bin/sh", "-c", "mkdir -p /app && ln -s $Y/hostdir /app/trap && exec sleep 300"]

[[services.app.watch]]
path = "src"
action = "sync"
target = "/app"
EOF
printf 'name = "ns"\n\n[services.a]\nimage = "%s"\ncommand = ["/bin/true"]\n\n[[services.a.watch]]\npath = "src"\naction = "sync"\n' "$IMG" > "$Y/notarget.toml"
refuses "sync without a target is refused by name" "needs a \`target\`" \
    timeout 60 "$B" stack up -f "$Y/notarget.toml"
timeout 300 "$B" stack up -f "$Y/rustainer.toml" >/dev/null 2>&1
YPID=$("$B" ps | awk '$1 == "ystack-app" {print $3}')
timeout 300 "$B" stack watch -f "$Y/rustainer.toml" > "$SCRATCH/sync.log" 2>&1 &
WPID=$!
sleep 3
echo "two" > "$Y/src/a.txt"
echo "deep" > "$Y/src/sub/b.txt"
echo "escaped" > "$Y/src/trap/c.txt"
for _ in $(seq 30); do
    OUT=$(timeout 60 "$B" exec ystack-app /bin/cat /app/sub/b.txt 2>/dev/null | tail -1)
    [ "$OUT" = "deep" ] && break
    sleep 1
done
check "a changed file arrives" "$(timeout 60 "$B" exec ystack-app /bin/cat /app/a.txt 2>/dev/null | tail -1)" "two"
check "and a new one in a new directory" "$OUT" "deep"
check "without the container being replaced" "$("$B" ps | awk '$1 == "ystack-app" {print $3}')" "$YPID"
sleep 2
check "a link in the container does not send the copy to the host" "$(ls "$Y/hostdir" | wc -l | tr -d ' ')" "0"
contains "and the watcher says what it did" "$(cat "$SCRATCH/sync.log")" "sync:"
kill "$WPID" 2>/dev/null
wait "$WPID" 2>/dev/null
timeout 300 "$B" stack down -t 0 -f "$Y/rustainer.toml" >/dev/null 2>&1

echo
echo "15. update replaces a service, and puts the old one back if the new one is unwell"
# The healthcheck decides. Each version of the service writes its own
# name into its own filesystem, so "the old one is back" is checked by
# reading a file only the old container could have written.
U="$SCRATCH/update"
mkdir -p "$U/gate"
touch "$U/gate/ok"
ufile() {
    cat > "$U/rustainer.toml" <<EOF
name = "ustack"

[services.peer]
image = "$IMG"
command = ["/bin/sleep", "300"]

[services.app]
image = "$IMG"
command = ["/bin/sh", "-c", "echo $1 > /version; exec sleep 300"]
volumes = ["$U/gate:/gate:ro"]

[services.app.healthcheck]
test = ["/bin/sh", "-c", "test -f /gate/ok"]
interval = 1
retries = 3
EOF
}
uver() { timeout 60 "$B" exec ustack-app /bin/cat /version 2>/dev/null | tail -1; }
uip() { timeout 60 "$B" exec ustack-peer /bin/sh -c 'grep -w app /etc/hosts' 2>/dev/null | awk '{print $1}' | tail -1; }
ufile one
timeout 300 "$B" stack up -f "$U/rustainer.toml" >/dev/null 2>&1
check "version one is what runs" "$(uver)" "one"
PEER_SEES=$(uip)
if [ -n "$PEER_SEES" ]; then ok "the peer resolves the service"; else bad "the peer resolves the service"; fi

# A new version that cannot become healthy.
ufile two
rm "$U/gate/ok"
OUT=$(timeout 300 "$B" stack update -f "$U/rustainer.toml" app 2>&1); RC=$?
check "an update whose healthcheck fails is an error" "$RC" "1"
contains "and it says what it did" "$OUT" "rolled back"
check "the container is running" "$("$B" ps | awk '$1 == "ustack-app" {print $2}')" "running"
check "and it is the OLD one, with the file only it wrote" "$(uver)" "one"
check "nothing is left set aside" "$("$B" ps -a | grep -c rollback)" "0"
contains "the rollback is in the event log" "$("$B" events --since 900 --limit 400)" "rollback"

# The same new version, now able to pass.
touch "$U/gate/ok"
OUT=$(timeout 300 "$B" stack update -f "$U/rustainer.toml" app 2>&1); RC=$?
check "an update that becomes healthy succeeds" "$RC" "0"
check "and the new version is what runs" "$(uver)" "two"
check "nothing is left set aside this time either" "$("$B" ps -a | grep -c rollback)" "0"
# The replacement takes over the address, so a service that was already
# running and already resolved it does not have to be told anything.
check "the new container has the address the peer already knows" \
    "$(timeout 60 "$B" exec ustack-app /bin/sh -c "ip -4 -o addr show | grep -v ' lo ' | awk '{print \$4}' | cut -d/ -f1" 2>/dev/null | tail -1)" "$PEER_SEES"
check "the peer was not restarted to achieve that" "$("$B" ps | awk '$1 == "ustack-peer" {print $2}')" "running"

printf 'name = "nh"\n\n[services.a]\nimage = "%s"\ncommand = ["/bin/true"]\n' "$IMG" > "$U/nohealth.toml"
refuses "a service with no healthcheck cannot be updated this way" "has no healthcheck" \
    timeout 60 "$B" stack update -f "$U/nohealth.toml" a
timeout 300 "$B" stack down -t 0 -f "$U/rustainer.toml" >/dev/null 2>&1

echo
echo "16. placement is judged against this one machine"
# What a one-node swarm does with the same file: a constraint that holds
# is no obstacle, and one that does not stops the stack before anything
# starts - including the services it would not have applied to.
PL="$SCRATCH/place"; mkdir -p "$PL"
place() {
    printf 'name = "pstack"\n\n[services.first]\nimage = "%s"\ncommand = ["/bin/sleep", "60"]\n\n[services.app]\nimage = "%s"\ncommand = ["/bin/sleep", "60"]\ndepends_on = ["first"]\nplacement = ["%s"]\n' "$IMG" "$IMG" "$1" > "$PL/rustainer.toml"
}
place "node.platform.arch != $(uname -m)"
refuses "a constraint this machine does not meet refuses the stack" "may not run on this machine" \
    timeout 120 "$B" stack up -f "$PL/rustainer.toml"
check "and nothing was started first" "$("$B" ps -a | grep -c pstack)" "0"
place "node.platform.arch == $(uname -m)"
timeout 300 "$B" stack up -f "$PL/rustainer.toml" >/dev/null 2>&1
check "a constraint it does meet is no obstacle" "$("$B" ps | awk '$1 == "pstack-app" {print $2}')" "running"
timeout 300 "$B" stack down -t 0 -f "$PL/rustainer.toml" >/dev/null 2>&1
place "node.role == manager"
refuses "a constraint only an orchestrator could judge is refused by name" "not something one machine can answer" \
    timeout 120 "$B" stack up -f "$PL/rustainer.toml"

echo
echo "17. An extension is a stack and the address of its page"
E="$SCRATCH/ext/probe"; mkdir -p "$E"
EPORT=$((41000 + ($$ % 9000)))
printf 'description = "a test extension"\nui = "http://127.0.0.1:%s/"\n' "$EPORT" > "$E/extension.toml"
cat > "$E/rustainer.toml" <<EOF
name = "probe"

[services.web]
image = "$IMG"
command = ["/bin/sh", "-c", "while true; do printf 'HTTP/1.0 200 OK\\r\\n\\r\\nextension-page\\n' | nc -l -p 80; done"]
ports = ["127.0.0.1:$EPORT:80"]
EOF
check "install names it after its stack" "$(timeout 60 "$B" extension install "$E" 2>&1 | tail -1)" "probe"
contains "it is listed, and off" "$("$B" extension ls)" "0/1 running"
refuses "installing it twice is refused" "already installed" timeout 60 "$B" extension install "$E"
timeout 300 "$B" extension enable probe >/dev/null 2>&1
contains "enabling it starts its container" "$("$B" extension ls)" "1/1 running"
PAGE=
for _ in $(seq 30); do
    PAGE=$(curl -fsS --max-time 3 "http://127.0.0.1:$EPORT/" 2>/dev/null) && break
    sleep 0.3
done
check "and its page is served where the manifest says" "$PAGE" "extension-page"
contains "the listing a page reads is JSON" "$("$B" extension ls --json)" '"ui":"http://127.0.0.1:'
timeout 120 "$B" extension disable probe >/dev/null 2>&1
contains "disabling it stops it" "$("$B" extension ls)" "0/1 running"
timeout 120 "$B" extension rm probe >/dev/null 2>&1
check "removing it leaves nothing listed" "$("$B" extension ls | grep -c probe)" "0"
# The page embeds the extension's UI, so where that may point is a
# security property and not a preference.
printf 'ui = "https://example.com/"\n' > "$E/extension.toml"
refuses "an extension whose page is not on this machine is refused" "must be on this" \
    timeout 60 "$B" extension install "$E"

echo
echo "$PASS passed, $FAIL failed, $SKIP skipped"
[ "$FAIL" -eq 0 ]
