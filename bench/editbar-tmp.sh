#!/bin/bash
# accl: the para4000 edit bar test with its rerun causes (diagnostic)
S=$r/summary.txt; touch $S
PHITEX_RUN_LOG=${RUNLOG:-} timeout 1500 cargo test -q --profile fastdev -p phitex-tex --features profile,observe --test edits one_word_edit_reruns_little -- --exact --nocapture > $r/test.log 2>&1; echo "EXIT $?" >> $r/test.log
grep -v '^\s*Compiling' $r/test.log | tail -60 >> $S
