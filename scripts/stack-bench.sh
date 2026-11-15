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
    echo "  rustainer:       refused it - $(echo "$OUT" | tail -1 | cut -c1-80)"
fi

echo
echo "== What Compose has that these stacks do not =="
cat <<'EOF'
  deploy/replicas, configs and secrets
  watch/develop, and `compose run` for a one-off against the stack
  a healthcheck that keeps running after startup, not just during `up`

  Closed since this list was first written, because measuring against
  Compose is what made the gap concrete: profiles, ${VAR} interpolation
  with .env, build:, restart on a service, several -f files merged,
  extends, and healthcheck with depends_on conditions.
EOF
