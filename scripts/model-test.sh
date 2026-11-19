#!/bin/bash
# The models check: a real model pulled from Docker Hub, served, and asked.
#
#   sudo RUSTAINER_BIN=... bash scripts/model-test.sh
#
# Its own CI job, like the cluster: it downloads a 100 MiB model and a
# llama.cpp server image, and needs root and a firewall tool for the
# bridge the server is reached on.
set -u
cd "$(dirname "$0")/.." || exit 1
B="${RUSTAINER_BIN:-target/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }
[ "$(id -u)" -eq 0 ] || { echo "needs root" >&2; exit 2; }
M=ai/smollm2:135M-Q4_K_M
C=model-ai-smollm2-135m-q4-k-m
PASS=0; FAIL=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (got '$(echo "$2" | head -c 160)')" ;; esac; }
trap '"$B" model rm "$M" >/dev/null 2>&1' EXIT

contains "an image is refused as a model" "$("$B" model pull alpine:3.21 2>&1)" "not a model"
FILE=$(timeout 600 "$B" model pull "$M" 2>&1 | tail -1)
check "a pull leaves a GGUF file" "$(head -c 4 "$FILE" 2>/dev/null)" "GGUF"
check "and nothing half-downloaded beside it" "$(ls "$(dirname "$FILE")" | grep -c partial)" "0"
contains "it is listed by the reference that pulled it" "$("$B" model ls)" "$M"
contains "run serves it" "$(timeout 900 "$B" model run "$M" 2>&1 | tail -1)" "serving $M at http://127.0.0.1:12434/v1"
check "and the list says so" "$("$B" model ls | awk -F'\t' -v m="$M" '$1 == m {print $3}')" "serving"
contains "the OpenAI-compatible API answers on the published port" \
    "$(curl -s --max-time 120 http://127.0.0.1:12434/v1/chat/completions -H 'Content-Type: application/json' \
        -d '{"messages":[{"role":"user","content":"Say hello."}],"max_tokens":16}')" '"choices"'
ANSWER=$(timeout 300 "$B" model ask "$M" "Say hello in one short sentence." 2>&1)
[ -n "$ANSWER" ] && ok "ask prints an answer" || bad "ask prints an answer"
# The server is a llama.cpp build on glibc: every thread it starts is the
# check that clone3 is answered ENOSYS, which is what lets glibc fall back.
contains "the model file is mounted read-only" \
    "$("$B" exec "$C" sh -c 'touch /models/x 2>&1; echo rc=$?')" "rc=1"
"$B" model stop "$M" >/dev/null 2>&1
check "stop leaves the file" "$("$B" model ls | grep -c "$M")" "1"
contains "and a stopped model cannot be asked" "$("$B" model ask "$M" hi 2>&1)" "is not running"
"$B" model rm "$M" >/dev/null 2>&1
check "rm deletes it" "$("$B" model ls | grep -c "$M")" "0"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
