#!/bin/bash
# accl: the para4000 edit bar test with its rerun causes, then the pages and edits suites (diagnostic)
S=$r/summary.txt; touch $S
PHITEX_RUN_LOG=${RUNLOG:-} timeout 1500 cargo test -q --profile fastdev -p phitex-tex --features profile,observe --test edits one_word_edit_reruns_little -- --exact --nocapture > $r/test.log 2>&1; echo "EXIT $?" >> $r/test.log
grep -v '^\s*Compiling' $r/test.log | tail -40 >> $S
for t in ${SUITES:-pages edits}; do
  timeout 1500 cargo test -q --profile fastdev -p phitex-tex --features profile,observe --test $t > $r/suite-$t.log 2>&1; echo "EXIT $?" >> $r/suite-$t.log
  echo "== suite $t: $(grep -E '^test result|EXIT' $r/suite-$t.log | tr '\n' ' ')" >> $S
  grep -E '^    [a-z_:]+$|panicked at' $r/suite-$t.log | head -20 >> $S
done
