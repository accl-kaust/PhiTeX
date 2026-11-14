#!/bin/bash
# (temporary, agent firstuse) scoped tests in the CI container: for each "TARGET:FILTER" argument,
# cargo test -p phitex-tex --release --test TARGET -- FILTER (output per argument in $r/test-N.txt).
set -u
cd "$(dirname "$0")/.."
export CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS:-16}
S=$r/summary.txt
# FU_COMMIT=<rev>: its crates/ and Cargo.lock tested instead (the script stays)
[ -n "${FU_COMMIT:-}" ] && { git checkout -q $FU_COMMIT -- crates Cargo.lock Cargo.toml || exit 1; echo "sources of $FU_COMMIT" | tee -a $S; }
git log --oneline -1 | tee -a $S
i=0
for a in "$@"; do
  i=$((i+1)); t=${a%%:*}; f=${a#*:}
  echo "=== $i $t $f" | tee -a $S
  cargo test -q --release -p phitex-tex --test $t -- $f --test-threads=${TT:-4} > $r/test-$i.txt 2>&1
  echo "exit $?" | tee -a $S
  grep -E "^test result|panicked|FAILED|^error" $r/test-$i.txt | head -30 | tee -a $S
done
