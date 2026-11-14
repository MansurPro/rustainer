# A microsecond clock that exists on the hosts these benchmarks run on.
# Sourced, not executed: it defines now_us().
#
# NOT `date +%s%N`: busybox's date has no %N and prints a literal "N",
# so the arithmetic silently produces nonsense rather than failing - and
# every timing comes out as 0ms. The benchmark host is a docker-in-docker
# container, which has busybox date even once bash is installed. bash 5's
# EPOCHREALTIME needs no process at all, so it is also the cheapest.
# Learned the first time in scripts/bench.sh, and again here.

if [ -n "${EPOCHREALTIME:-}" ]; then
    now_us() { u=${EPOCHREALTIME}; echo "${u/./}"; }
elif [ "$(date +%N 2>/dev/null)" != "N" ] && [ -n "$(date +%N 2>/dev/null)" ]; then
    now_us() { echo $(( $(date +%s%N) / 1000 )); }
else
    echo "no sub-second clock here: need bash 5 (EPOCHREALTIME) or GNU date" >&2
    exit 2
fi
