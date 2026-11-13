#!/bin/bash
# (temporary, agent editpdf) phitex-live's whole suite REPS times, BASE's sources and this tree's; watch_word_edit at BASE.
set -u; cd "$(dirname "$0")/.."; here=$PWD; REPS=${1:-5}; BASE=${2:-81a72ba6d}; S=$r/summary.txt
rm -rf $w/before && mkdir -p $w/before && git archive HEAD | tar -x -C $w/before
for f in $(git diff --name-only $BASE HEAD -- crates | grep -v '/tests/'); do git show $BASE:$f > $w/before/$f 2>/dev/null || rm -f $w/before/$f; done
T="cargo test -q --profile fastdev"
for t in before after; do d=$here; [ $t = before ] && d=$w/before
  for i in $(seq $REPS); do (cd $d && CARGO_TARGET_DIR=$w/target-$t $T -p phitex-live > $r/live-$t-$i.txt 2>&1); echo "live $t #$i: exit $? $(grep -E 'FAILED$' $r/live-$t-$i.txt | head -3 | tr '\n' ' ')" | tee -a $S; done
  (cd $d && CARGO_TARGET_DIR=$w/target-$t $T -p phitex-cli --test tests cli::watch_word_edit -- --exact > $r/word-$t.txt 2>&1); echo "watch_word_edit $t: exit $? $(grep -E '^test result' $r/word-$t.txt)" | tee -a $S; grep -A3 panicked $r/word-$t.txt | head -4 | tee -a $S
done
