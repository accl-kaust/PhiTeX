#!/bin/bash
# bench/flow-par/tripab.sh NAME=REV...: trip, etrip and fuzz tests per revision
set -u
R=${r:-/tmp/tripab-results}; W=${w:-/tmp/tripab-work}/tripab; mkdir -p "$R" "$W"
log() { echo "$*" | tee -a "$R/summary.txt"; }
for spec in "$@"; do
  name=${spec%%=*}; rev=${spec#*=}
  git cat-file -e "$rev^{commit}" 2>/dev/null || git fetch -q origin "refs/wip/$rev" 2>/dev/null || git fetch -q origin "+refs/wip/*:refs/wip/*"
  git worktree add -q --detach "$W/wt-$name" "$rev"
  for t in trip etrip fuzz; do
    (cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" timeout 1800 cargo test --release -q -p phitex-flow --test $t -- --test-threads=2) > "$R/$name-$t.log" 2>&1
    log "$name ($(git rev-parse --short $rev)) $t: status $? $(grep -E '^test result' $R/$name-$t.log | tail -1)"
    grep -E 'panicked' -A2 "$R/$name-$t.log" | head -6 | sed 's/^/    /' | tee -a "$R/summary.txt"
  done
done
