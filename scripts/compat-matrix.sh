#!/bin/bash
# How compatible is "compatible", as a number, across real images.
#
#   bash scripts/compat-matrix.sh [image ...]
#
# Run it where BOTH engines exist - the docker-in-docker host that
# scripts/bench.sh documents. Docker is the reference: an image Docker
# itself cannot run is not evidence about us, so it is reported and
# excluded rather than counted as a failure.
#
# Compatibility is PARITY, not a multiple. There is no "3x more
# compatible": the target is that an image behaves identically, and the
# honest way to report it is a rate - how many of N real images produce
# the same answer under both engines - with the failures named. That is
# what this prints.
#
# Two probes per image, chosen because they are exactly what an image's
# config means at runtime:
#
#   1. `sh -c 'id -u; id -g; pwd; printf %s "$PATH"'` - USER (including
#      a name that has to be resolved through the image's own
#      /etc/passwd), WORKDIR, and ENV. Byte-compared.
#   2. The image's own ENTRYPOINT and CMD with no override, under a
#      timeout. Exit codes compared - a server that runs until it is
#      killed scores the same under both, which is the point.
#
# Probe 1 needs a shell. An image without one (distroless) is marked
# `no-shell` and only probe 2 counts, because inventing a shell for it
# would be measuring something else.

set -u
export LC_ALL=C
cd "$(dirname "$0")/.." || exit 1

B="${RUSTAINER_BIN:-${CARGO_TARGET_DIR:-target}/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'; set RUSTAINER_BIN" >&2; exit 2; }
command -v docker >/dev/null 2>&1 || { echo "needs docker on the same kernel" >&2; exit 2; }
docker info >/dev/null 2>&1 || { echo "dockerd is not answering" >&2; exit 2; }

WORK=$(mktemp -d "${TMPDIR:-/tmp}/rustainer-compat.XXXXXX")
if [ "$(stat -fc %T "$WORK")" = overlayfs ]; then
    mount -t tmpfs -o size=4g tmpfs "$WORK" 2>/dev/null \
        && MOUNTED=yes || MOUNTED=no
else
    MOUNTED=no
fi
export RUSTAINER_ROOT="$WORK/state"
# The image store on the tmpfs is fine for a dozen images and not for
# thirty: tmpfs is RAM, and the benchmark VM has less of it than thirty
# images need - which is how this script came to report fourteen
# consecutive "PULL-FAIL"s that were ENOSPC.
#
# Point RUSTAINER_COMPAT_STORE at a REAL filesystem for a long list. Not
# a path inside a container's own root, which is overlayfs: an overlay
# cannot be assembled from lowerdirs that are themselves on overlayfs,
# and the symptom is every container exiting 127 because its root
# filesystem came out empty. In the docker-in-docker benchmark host,
# /var/lib/docker is the real disk.
export RUSTAINER_IMAGE_ROOT="${RUSTAINER_COMPAT_STORE:-${RUSTAINER_IMAGE_DIR:-$WORK/images}}"
mkdir -p "$RUSTAINER_IMAGE_ROOT"
cleanup() {
    "$B" ps -a 2>/dev/null | tail -n +2 | awk '{print $1}' \
        | while read -r c; do [ -n "$c" ] && "$B" rm --force "$c" >/dev/null 2>&1; done
    [ "$MOUNTED" = yes ] && { for _ in 1 2 3 4 5 6; do umount "$WORK" 2>/dev/null && break; sleep 0.5; done; }
    rm -rf "$WORK"
}
trap cleanup EXIT

# A spread of shapes rather than a spread of names: musl and glibc, an
# image with no shell, images whose ENTRYPOINT is a script, images that
# set USER to a name, and images that are mostly a compiler.
DEFAULT_IMAGES="alpine:3.21 alpine:3.19 debian:12-slim debian:11-slim \
ubuntu:24.04 ubuntu:22.04 busybox:1.37 fedora:41 almalinux:9 \
python:3.13-slim python:3.13-alpine node:22-alpine node:22-slim \
ruby:3.3-alpine php:8.3-cli-alpine golang:1.23-alpine rust:1-slim \
eclipse-temurin:21-jre-alpine nginx:1.27-alpine nginx:1.27 \
httpd:2.4-alpine caddy:2-alpine haproxy:3-alpine traefik:v3.2 \
redis:7-alpine redis:7 postgres:17-alpine mariadb:11 \
memcached:1.6-alpine mongo:7"
IMAGES=${*:-$DEFAULT_IMAGES}

# Docker Hub rate-limits anonymous pulls hard enough that this script
# exhausts it; the Google mirror is a pull-through cache of Hub, so the
# manifests and blobs are the same bytes.
mirror() { case "$1" in */*) echo "mirror.gcr.io/$1" ;; *) echo "mirror.gcr.io/library/$1" ;; esac; }

# shellcheck source=scripts/now-us.sh
. "$(dirname "$0")/now-us.sh"
median() { tr ' ' '\n' | sort -n | awk '{a[NR]=$1} END {print (NR%2) ? a[(NR+1)/2] : int((a[NR/2]+a[NR/2+1])/2)}'; }

PROBE='id -u; id -g; pwd; printf "%s" "$PATH"'

TOTAL=0; SKIPPED=0; CFG_OK=0; CFG_N=0; EXIT_OK=0; EXIT_N=0
FAILED=""
printf '%-22s %-10s %-10s %10s %10s %7s\n' IMAGE "probe 1" "probe 2" docker rustainer ratio
printf '%s\n' "----------------------------------------------------------------------------"

for img in $IMAGES; do
    TOTAL=$((TOTAL + 1))
    ref=$(mirror "$img")
    if ! docker pull -q "$ref" >/dev/null 2>&1; then
        echo "  $img: docker could not pull it; excluded"
        SKIPPED=$((SKIPPED + 1)); continue
    fi
    # The error, not just the fact. A run of these turned out to be the
    # 4g tmpfs this script mounts filling up - tmpfs is RAM, and the
    # benchmark VM has less of it than thirty images need - and the
    # discarded message said so all along.
    if ! PERR=$(timeout 600 "$B" pull "$ref" 2>&1 >/dev/null); then
        printf '%-22s %-10s %s\n' "$img" "PULL-FAIL" "$(echo "$PERR" | tail -1 | cut -c1-70)"
        FAILED="$FAILED $img(pull)"; continue
    fi

    # --- probe 1: what the image's config means at runtime --------------
    D1=$(timeout 60 docker run --rm --network none "$ref" /bin/sh -c "$PROBE" 2>/dev/null)
    D1RC=$?
    if [ $D1RC -ne 0 ]; then
        P1=no-shell
    else
        R1=$(timeout 60 "$B" run --network none "$ref" /bin/sh -c "$PROBE" 2>/dev/null)
        CFG_N=$((CFG_N + 1))
        if [ "$D1" = "$R1" ]; then P1=same; CFG_OK=$((CFG_OK + 1))
        else
            P1=DIFFER
            FAILED="$FAILED $img(probe1)"
            printf '    docker:    %s\n    rustainer: %s\n' "$(echo "$D1" | tr '\n' '|')" "$(echo "$R1" | tr '\n' '|')"
        fi
    fi

    # --- probe 2: the image's own entrypoint and command ----------------
    timeout 12 docker run --rm --network none "$ref" >/dev/null 2>&1; D2=$?
    timeout 12 "$B" run --network none "$ref" >/dev/null 2>&1; R2=$?
    EXIT_N=$((EXIT_N + 1))
    if [ "$D2" = "$R2" ]; then P2=same; EXIT_OK=$((EXIT_OK + 1))
    else P2="$D2/$R2"; FAILED="$FAILED $img(exit)"; fi

    # --- and how long each takes to run one, on this kernel -------------
    dt=""; rt=""
    for _ in 1 2 3; do
        s=$(now_us); docker run --rm --network none "$ref" /bin/true >/dev/null 2>&1; e=$(now_us)
        dt="$dt $((e - s))"
        s=$(now_us); "$B" run --network none "$ref" /bin/true >/dev/null 2>&1; e=$(now_us)
        rt="$rt $((e - s))"
    done
    DM=$(echo "$dt" | median); RM=$(echo "$rt" | median)
    if [ "${RM:-0}" -gt 0 ] && [ "${DM:-0}" -gt 0 ]; then
        RATIO=$(awk -v d="$DM" -v r="$RM" 'BEGIN {printf "%.1fx", d / r}')
    else RATIO="-"; fi
    printf '%-22s %-10s %-10s %9sms %9sms %7s\n' \
        "$img" "$P1" "$P2" "$((DM / 1000))" "$((RM / 1000))" "$RATIO"

    docker rmi -f "$ref" >/dev/null 2>&1
    "$B" rmi "$ref" >/dev/null 2>&1
done

echo
echo "images offered:           $TOTAL"
echo "excluded (docker failed): $SKIPPED"
echo "config identical:         $CFG_OK/$CFG_N  (uid, gid, workdir, PATH)"
echo "exit status identical:    $EXIT_OK/$EXIT_N  (the image's own entrypoint and command)"
[ -n "$FAILED" ] && echo "not identical:           $FAILED"
echo
echo "Compatibility is parity. The number to read is how close both rates"
echo "are to their denominators, not how large they are."
[ "$CFG_OK" = "$CFG_N" ] && [ "$EXIT_OK" = "$EXIT_N" ]
