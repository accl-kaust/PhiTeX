#!/bin/bash
# cut1: tests on the control commit (CONTROL) for comparison, and a traced cut test
set -u
rm -rf $w/control && mkdir -p $w/control && git archive ${CONTROL:-47d7561f3} | tar -x -C $w/control
[ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
( cd $w/control && cargo test -q --profile fastdev -p phitex-tex --test latex -- --test-threads 8 floats2col_routines_run_once_per_fire latexmk_biber > $r/ctl-latex.txt 2>&1 ); grep -E "test result|^    [a-z_0-9]+$" $r/ctl-latex.txt
T="cargo test -q --profile fastdev -p phitex-tex --features observe"
PHITEX_CUT_TRACE=1 $T --test edits ${DBG_TEST:-a_box_over} -- --nocapture --test-threads 1 > $r/dbg.txt 2>&1
grep -E "^cut:" $r/dbg.txt | sed 's/node [0-9]*/node N/; s/after [0-9]* commands/after C commands/' | sort | uniq -c | sort -rn | head -30
grep -E "panicked|test result" -A2 $r/dbg.txt | head
