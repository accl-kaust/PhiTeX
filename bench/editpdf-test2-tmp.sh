#!/bin/bash
# (temporary, agent editpdf) the watch's way-out tests before the fix (BASE's sources, these tests) and after, REPS runs
# after; then the suites the change touches.   editpdf-test2-tmp.sh [REPS] [BASE]
set -u; cd "$(dirname "$0")/.."; here=$PWD
REPS=${1:-3}; BASE=${2:-ad18373c0}; S=$r/summary.txt
rm -rf $w/before && mkdir -p $w/before && git archive HEAD | tar -x -C $w/before
for f in $(git diff --name-only $BASE HEAD -- crates | grep -v '/tests/'); do git show $BASE:$f > $w/before/$f 2>/dev/null || rm -f $w/before/$f; done
T="cargo test -q --profile fastdev"
TESTS=${TESTS:-"cli::watch_quit_leaves_the_exact_files cli::watch_signal_ends_as_q cli::watch_word_edit"}
for t in before after; do
  d=$here; [ $t = before ] && d=$w/before; n=$REPS; [ $t = before ] && n=1
  (cd $d && CARGO_TARGET_DIR=$w/target-$t $T -p phitex-cli --test tests --no-run > $r/build-$t.txt 2>&1) || { echo "$t: build failed" | tee -a $S; grep -E "^error" -A8 $r/build-$t.txt | head -40 | tee -a $S; continue; }
  for i in $(seq $n); do for x in $TESTS; do
    (cd $d && CARGO_TARGET_DIR=$w/target-$t timeout 3000 $T -p phitex-cli --test tests $x -- --exact --nocapture > $r/$t-$x-$i.txt 2>&1)
    echo "$t $x #$i: exit $? $(grep -E '^test result' $r/$t-$x-$i.txt) $(grep -oE '(not a clean build.s|not end the watch as q does) in [0-9]+ sessions' $r/$t-$x-$i.txt)" | tee -a $S
    grep -E "after (the edit's build|the save)|panicked|exit status" $r/$t-$x-$i.txt | sort | uniq -c | sort -rn | head -8 | tee -a $S
  done; done
done
CARGO_TARGET_DIR=$w/target-after $T -p phitex-cli --bin phitex > $r/unit.txt 2>&1; echo "cli unit: exit $? $(grep -E '^test result' $r/unit.txt)" | tee -a $S
CARGO_TARGET_DIR=$w/target-after $T -p phitex-cli --test tests cli:: > $r/cli.txt 2>&1; echo "cli suite: exit $? $(grep -E '^test result' $r/cli.txt)" | tee -a $S; grep -E "^test .* FAILED" $r/cli.txt | tee -a $S
CARGO_TARGET_DIR=$w/target-after $T -p phitex-live > $r/live.txt 2>&1; echo "live: exit $? $(grep -E '^test result' $r/live.txt | tr '\n' ' ')" | tee -a $S; grep -E "^test .* FAILED" $r/live.txt | tee -a $S
CARGO_TARGET_DIR=$w/target-after $T -p phitex-tex --test suites > $r/suites.txt 2>&1; echo "tex suites: exit $? $(grep -E '^test result' $r/suites.txt)" | tee -a $S; grep -E "^test .* FAILED" $r/suites.txt | tee -a $S
for t in before after; do d=$here; [ $t = before ] && d=$w/before
  for i in 1 2 3 4 5; do (cd $d && CARGO_TARGET_DIR=$w/target-$t $T -p phitex-live --lib a_cold_build_streams_its_pages_before_it_settles -- --nocapture > $r/stream-$t-$i.txt 2>&1); echo "live stream test $t #$i: exit $?" | tee -a $S; grep -E "panicked" -A3 $r/stream-$t-$i.txt | cut -c1-300 | head -4 | tee -a $S; done
done
