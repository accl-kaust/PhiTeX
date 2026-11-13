#!/bin/bash
# (temporary, agent editpdf) watch_quit_leaves_the_exact_pdf before the fix (dev's watch.rs/session.rs) and after, REPS runs each.
#   editpdf-test-tmp.sh [REPS] [BASE]
set -u; cd "$(dirname "$0")/.."; here=$PWD
REPS=${1:-1}; BASE=${2:-81a72ba6d}; S=$r/summary.txt
rm -rf $w/before && mkdir -p $w/before && git archive HEAD | tar -x -C $w/before
for f in crates/cli/src/session.rs crates/cli/src/watch.rs; do git show $BASE:$f > $w/before/$f; done
for t in before after; do
  d=$here; [ $t = before ] && d=$w/before
  (cd $d && CARGO_TARGET_DIR=$w/target-$t cargo test -q --profile fastdev -p phitex-cli --test tests --no-run > $r/build-$t.txt 2>&1) || { echo "$t: build failed"; grep -E "^(error|warning)" -A8 $r/build-$t.txt | head -60; continue; }
done
for i in $(seq $REPS); do for t in before after; do
  d=$here; [ $t = before ] && d=$w/before
  (cd $d && CARGO_TARGET_DIR=$w/target-$t timeout 3000 cargo test -q --profile fastdev -p phitex-cli --test tests cli::watch_quit_leaves_the_exact_pdf -- --exact --nocapture > $r/test-$t-$i.txt 2>&1)
  echo "$t #$i: exit $? $(grep -E '^test result' $r/test-$t-$i.txt) $(grep -oE 'not a clean build.s in [0-9]+ sessions' $r/test-$t-$i.txt)" | tee -a $S
  grep -E "q after" $r/test-$t-$i.txt | head -20 | tee -a $S
done; done
