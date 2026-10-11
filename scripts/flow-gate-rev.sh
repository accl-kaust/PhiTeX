#!/usr/bin/env bash
# The commit a gate job tests: HEAD, or, when scripts/flow-gate-submit.sh
# laid the gate scripts over it, the commit under them.
cd "$(dirname "$0")/.."
s=$(git log -1 --format=%s 2>/dev/null)
case "$s" in
"gate scripts over "*) r=${s#gate scripts over }; echo "${r%% *}" ;;
*) git rev-parse --short=8 HEAD 2>/dev/null || echo '?' ;;
esac
