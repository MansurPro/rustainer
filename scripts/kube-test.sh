#!/bin/bash
# The Kubernetes check: a cluster on this engine that schedules real work.
#
#   sudo RUSTAINER_BIN=... bash scripts/kube-test.sh
#
# Not part of CI. It pulls a 70 MiB node image and two more for the
# workload, needs root, a firewall tool and several minutes - the cost of
# testing a cluster rather than a claim about one. Run it by hand after
# touching `--privileged`, cgroups, or `kube`.
set -u
cd "$(dirname "$0")/.." || exit 1
B="${RUSTAINER_BIN:-target/release/rustainer}"
[ -x "$B" ] || { echo "no rustainer binary at '$B'" >&2; exit 2; }
[ "$(id -u)" -eq 0 ] || { echo "needs root" >&2; exit 2; }
# Not set for you: forwarding is a host-wide setting and the engine
# refuses to flip it, so say what is missing before ten minutes of pulls.
[ "$(cat /proc/sys/net/ipv4/ip_forward)" = 1 ] || { echo "needs: sysctl -w net.ipv4.ip_forward=1" >&2; exit 2; }
PASS=0; FAIL=0
ok() { PASS=$((PASS + 1)); echo "  PASS  $1"; }
bad() { FAIL=$((FAIL + 1)); echo "  FAIL  $1"; }
check() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1 (got '$2', want '$3')"; fi; }
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1 (got '$(echo "$2" | head -c 160)')" ;; esac; }
trap '"$B" kube down >/dev/null 2>&1' EXIT

contains "the cluster starts and its node is Ready" "$(timeout 600 "$B" kube up 2>&1 | tail -1)" "the cluster is ready"
"$B" kube kubectl create deployment web --image=nginx:1.27-alpine --replicas=2 >/dev/null 2>&1
"$B" kube kubectl expose deployment web --port=80 >/dev/null 2>&1
contains "a two-replica deployment rolls out" \
    "$("$B" kube kubectl rollout status deployment/web --timeout=240s 2>&1 | tail -1)" "successfully rolled out"
check "both pods are Running" "$("$B" kube kubectl get pods -l app=web --no-headers 2>/dev/null | grep -c Running)" "2"
IP=$("$B" kube kubectl get svc web -o jsonpath='{.spec.clusterIP}' 2>/dev/null)
contains "the service answers on its cluster address" \
    "$("$B" exec rustainer-kube wget -qO- --timeout=10 "http://$IP/" 2>&1)" "Welcome to nginx"
contains "the API server is reachable from the host" \
    "$(curl -sk --max-time 10 https://127.0.0.1:6443/version 2>&1)" '"kind"'
contains "a kubeconfig is printed" "$("$B" kube config 2>/dev/null)" "server: https"
"$B" kube down >/dev/null 2>&1
contains "it comes back after down" "$(timeout 600 "$B" kube up 2>&1 | tail -1)" "the cluster is ready"
contains "with the deployment it had" "$("$B" kube kubectl get deployment web --no-headers 2>&1)" "web"

echo
echo "$PASS passed, $FAIL failed"
[ "$FAIL" -eq 0 ]
