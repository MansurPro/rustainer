#!/bin/bash
# Merge a pull request only if every check on it passed.
#
#   bash scripts/merge-if-green.sh <pr-number>
#
# Counts the passes instead of looking for a failure: a `gh` call that
# prints nothing (rate limit, network) must read as "not green", and a
# gate that greps for the word "fail" reads it as "nothing failed".
set -u
pr=${1:?usage: merge-if-green.sh <pr-number>}
checks=$(gh pr checks "$pr" --json name,bucket) || { echo "NOT MERGED: could not read the checks"; exit 1; }
total=$(echo "$checks" | jq length)
passed=$(echo "$checks" | jq '[.[] | select(.bucket == "pass")] | length')
if [ "$total" -lt 1 ] || [ "$total" != "$passed" ]; then
    echo "NOT MERGED: $passed of $total checks passed"
    echo "$checks" | jq -r '.[] | select(.bucket != "pass") | "  \(.name): \(.bucket)"'
    exit 1
fi
echo "$passed of $total checks passed"
gh pr merge "$pr" --squash --delete-branch
