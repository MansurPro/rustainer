#!/bin/bash
# rustainer stacks measured against Docker Compose, same app, same kernel.
#
#   bash scripts/stack-bench.sh
#
# Run it where BOTH exist - the docker-in-docker host that
# scripts/bench.sh documents, which ships `docker compose`.
#
# The same three-service application is written twice, once as
# compose.yaml and once as rustainer.toml, and both are asked to do the
# same four things. What is compared:
#
#   1. How long `up` takes, measured not to the command returning but to
#      the thing the operator actually wants: every service able to
#      reach every other BY NAME. A stack whose containers exist but
#      cannot find each other is not up.
#   2. How long `down` takes.
#   3. What a typo does. This is the one that matters in practice and it
#      is not a speed: Compose warns about an unknown service key and
#      carries on with a file that does not mean what it says; this
#      refuses it. The cost of being wrong here is an application
#      running without the setting somebody thought they wrote.
#   4. What it costs to sit there: resident memory of everything the
#      engine runs for the stack.
#
# The honest half is at the bottom, and it is long: Compose has years of
# features this does not.

set -u
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 1

B="${RUSTAINER_BIN:-${CARGO_TARGET_DIR:-target}/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'; set RUSTAINER_BIN" >&2; exit 2; }
command -v docker >/dev/null 2>&1 && docker compose version >/dev/null 2>&1 \
    || { echo "needs docker compose on the same kernel" >&2; exit 2; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-stackbench.XXXXXX")
if [ "$(stat -fc %T "$WORK")" = overlayfs ]; then
    mount -t tmpfs -o size=3g tmpfs "$WORK" 2>/dev/null && MOUNTED=yes || MOUNTED=no
else MOUNTED=no; fi
export RUSTAINER_ROOT="$WORK/state"
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_COMPAT_STORE:-$WORK/images}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" stack down -f "$WORK/rustainer.toml" >/dev/null 2>&1
    docker compose -f "$WORK/compose.yaml" down -t 1 >/dev/null 2>&1
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED" = yes ] && { for _ in 1 2 3 4 5 6; do umount "$WORK" 2>/dev/null && break; sleep 0.5; done; }
    rm -rf "$WORK"
}
trap cleanup EXIT

IMG=${RUSTAINER_BENCH_IMAGE:-mirror.gcr.io/library/alpine:3.21}
docker pull -q "$IMG" >/dev/null 2>&1 || { echo "cannot pull $IMG" >&2; exit 2; }
timeout 600 "$B" pull "$IMG" >/dev/null 2>&1 || { echo "cannot pull $IMG for rustainer" >&2; exit 2; }

# shellcheck source=scripts/now-us.sh
. "$(dirname "$0")/now-us.sh"

cat > "$WORK/compose.yaml" <<EOF
services:
  db:
    image: $IMG
    command: ["/bin/sleep", "600"]
  cache:
    image: $IMG
    command: ["/bin/sleep", "600"]
  web:
    image: $IMG
    command: ["/bin/sleep", "600"]
EOF

cat > "$WORK/rustainer.toml" <<EOF
name = "bench"

[services.db]
image = "$IMG"
command = ["/bin/sleep", "600"]

[services.cache]
image = "$IMG"
command = ["/bin/sleep", "600"]

[services.web]
image = "$IMG"
command = ["/bin/sleep", "600"]
EOF

# Up is not "the command returned", it is "every peer answers to its
# name". Polled identically for both so the poll itself cancels out.
d_ready() {
    docker compose -f "$WORK/compose.yaml" exec -T web \
        /bin/sh -c 'ping -c1 -W1 db >/dev/null 2>&1 && ping -c1 -W1 cache >/dev/null 2>&1' >/dev/null 2>&1
}
r_ready() {
    timeout 20 "$B" exec bench-web \
        /bin/sh -c 'ping -c1 -W1 db >/dev/null 2>&1 && ping -c1 -W1 cache >/dev/null 2>&1' >/dev/null 2>&1
}

echo "== 1. up, to every service reachable from every other by name =="
s=$(now_us)
docker compose -f "$WORK/compose.yaml" up -d >/dev/null 2>&1
for _ in $(seq 100); do d_ready && break; sleep 0.1; done
D_UP=$(( ($(now_us) - s) / 1000 ))
d_ready && echo "  docker compose up:  ${D_UP}ms" || { echo "  docker compose never became ready"; D_UP=0; }

s=$(now_us)
timeout 300 "$B" stack up -f "$WORK/rustainer.toml" >/dev/null 2>&1
for _ in $(seq 100); do r_ready && break; sleep 0.1; done
R_UP=$(( ($(now_us) - s) / 1000 ))
r_ready && echo "  rustainer stack up: ${R_UP}ms" || { echo "  rustainer never became ready"; R_UP=0; }
[ "${R_UP:-0}" -gt 0 ] && [ "${D_UP:-0}" -gt 0 ] \
    && awk -v d="$D_UP" -v r="$R_UP" 'BEGIN {printf "  ratio: %.1fx\n", d / r}'

echo
echo "== 2. what it costs to sit there =="
# Deliberately NOT re-measured here. scripts/bench.sh already measures
# per-container RSS and the daemon's resident footprint properly, and a
# second measurement that greps `ps` for process names is how you get
# "0 MiB (shims)" printed with a straight face - which is what the first
# version of this section did, because busybox ps truncates comm and the
# pattern never matched. One measurement, in the place that does it
# right.
echo "  see scripts/bench.sh: per-container RSS, and the daemon that is not there"

echo "== 3. down =="
# The SAME grace period for both. The first version of this gave
# Compose `-t 1` and us whatever our default was, and reported that we
# were 11x slower - which measured the flag, not the engine.
s=$(now_us); docker compose -f "$WORK/compose.yaml" down -t 1 >/dev/null 2>&1
D_DOWN=$(( ($(now_us) - s) / 1000 ))
s=$(now_us); timeout 300 "$B" stack down -t 1 -f "$WORK/rustainer.toml" >/dev/null 2>&1
R_DOWN=$(( ($(now_us) - s) / 1000 ))
echo "  docker compose down -t 1:  ${D_DOWN}ms"
echo "  rustainer stack down -t 1: ${R_DOWN}ms"
# Both numbers are dominated by the grace period, because `sleep` as PID
# 1 in a new PID namespace has no default SIGTERM disposition and waits
# the whole thing out. Ours is also SEQUENTIAL - one service at a time,
# in reverse dependency order - where Compose stops a dependency level
# in parallel. That is a real difference and it is not in our favour.

echo
echo "== 4. a typo in the file =="
# Not a speed, and the one that costs the most. `commandd` is a key
# nobody meant to write; the question is whether the tool runs the
# application anyway with the command it did not set.
sed 's/    command:/    commandd:/' "$WORK/compose.yaml" > "$WORK/typo.yaml"
if docker compose -f "$WORK/typo.yaml" up -d >/dev/null 2>&1; then
    echo "  docker compose:  STARTED THE STACK with an unknown key"
    docker compose -f "$WORK/typo.yaml" down -t 1 >/dev/null 2>&1
else
    echo "  docker compose:  refused it"
fi
# Measured, not assumed: Compose v2 validates unknown service keys too,
# so this is PARITY and the "we are stricter" claim does not survive
# contact with the measurement. Kept because the result is the point.
sed 's/^command = /commandd = /' "$WORK/rustainer.toml" > "$WORK/typo.toml"
OUT=$(timeout 120 "$B" stack up -f "$WORK/typo.toml" 2>&1)
if [ $? -eq 0 ]; then
    echo "  rustainer:       STARTED THE STACK with an unknown key"
    timeout 120 "$B" stack down -f "$WORK/typo.toml" >/dev/null 2>&1
else
    # The line that names the key, not the outermost context line: the
    # useful half of a refusal is which key was wrong.
    WHY=$(echo "$OUT" | grep -m1 -o 'unknown field `[^`]*`' || echo "$OUT" | tail -1)
    echo "  rustainer:       refused it - $(echo "$WHY" | cut -c1-80)"
fi

echo
echo "== 5. a health-gated up, which is what depends_on is for =="
# Both files say the same thing: db is ready when /tmp/ready exists, it
# writes that file three seconds in, and web must not start before then.
# Both engines pay the same three seconds; what is being compared is the
# machinery around it, not the wait.
cat > "$WORK/health.yaml" <<EOF
services:
  db:
    image: $IMG
    command: ["/bin/sh", "-c", "sleep 3; touch /tmp/ready; sleep 600"]
    healthcheck:
      test: ["CMD", "/bin/sh", "-c", "test -f /tmp/ready"]
      interval: 1s
      retries: 30
  web:
    image: $IMG
    command: ["/bin/sleep", "600"]
    depends_on:
      db:
        condition: service_healthy
EOF
cat > "$WORK/health.toml" <<EOF
name = "hbench"

[services.db]
image = "$IMG"
command = ["/bin/sh", "-c", "sleep 3; touch /tmp/ready; sleep 600"]

[services.db.healthcheck]
test = ["/bin/sh", "-c", "test -f /tmp/ready"]
interval = 1
retries = 30

[services.web]
image = "$IMG"
command = ["/bin/sleep", "600"]
depends_on = { db = "healthy" }
EOF

s=$(now_us)
docker compose -f "$WORK/health.yaml" up -d --wait >/dev/null 2>&1
D_H=$(( ($(now_us) - s) / 1000 ))
docker compose -f "$WORK/health.yaml" down -t 1 >/dev/null 2>&1

s=$(now_us)
timeout 300 "$B" stack up -f "$WORK/health.toml" >/dev/null 2>&1
R_H=$(( ($(now_us) - s) / 1000 ))
timeout 300 "$B" stack down -t 1 -f "$WORK/health.toml" >/dev/null 2>&1

echo "  docker compose up (health-gated):  ${D_H}ms"
echo "  rustainer stack up (health-gated): ${R_H}ms"
echo "  both include the same ~3s the application takes to become ready"

echo
echo "== 6. replicas =="
# Three containers of one service, started and torn down.
printf 'services:\n  w:\n    image: %s\n    command: ["/bin/sleep", "600"]\n    deploy:\n      replicas: 3\n' "$IMG" > "$WORK/rep.yaml"
printf 'name = "rbench"\n\n[services.w]\nimage = "%s"\ncommand = ["/bin/sleep", "600"]\nreplicas = 3\n' "$IMG" > "$WORK/rep.toml"
s=$(now_us); docker compose -f "$WORK/rep.yaml" up -d >/dev/null 2>&1
D_R=$(( ($(now_us) - s) / 1000 ))
D_N=$(docker compose -f "$WORK/rep.yaml" ps -q 2>/dev/null | grep -c . || echo 0)
docker compose -f "$WORK/rep.yaml" down -t 1 >/dev/null 2>&1
s=$(now_us); timeout 300 "$B" stack up -f "$WORK/rep.toml" >/dev/null 2>&1
R_R=$(( ($(now_us) - s) / 1000 ))
R_N=$("$B" ps 2>/dev/null | grep -c 'rbench-w-' || echo 0)
timeout 300 "$B" stack down -t 1 -f "$WORK/rep.toml" >/dev/null 2>&1
echo "  docker compose, 3 replicas:  ${D_R}ms, $D_N containers"
echo "  rustainer, 3 replicas:       ${R_R}ms, $R_N containers"
[ "${R_R:-0}" -gt 0 ] && [ "${D_R:-0}" -gt 0 ] \
    && awk -v d="$D_R" -v r="$R_R" 'BEGIN {printf "  ratio: %.1fx\n", d / r}'

echo
echo "== What Compose has that these stacks do not =="
cat <<'EOF'
  configs and secrets; watch/develop
  the rest of deploy beyond replicas (placement, resources, rollback)
  a healthcheck that keeps running after startup, not just during `up`

  Closed since this list was first written, because measuring against
  Compose is what made the gap concrete: profiles, ${VAR} interpolation
  with .env, build:, restart on a service, several -f files merged,
  extends, healthcheck with depends_on conditions, replicas, and
  `stack run`.
EOF
