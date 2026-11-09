#!/bin/bash
# cut1 debug: a lib test and one edits test under the observe build, traced
set -u
T="cargo test -q --profile fastdev -p phitex-tex --features observe"
$T --lib a_long_paragraph_is_cut -- --nocapture > $r/lib.txt 2>&1; tail -5 $r/lib.txt
PHITEX_CUT_TRACE=1 $T --test edits ${DBG_TEST:-an_edited_long_paragraph} -- --nocapture --test-threads 1 > $r/dbg.txt 2>&1
grep -E "^cut:" $r/dbg.txt | sed 's/node [0-9]*/node N/; s/after [0-9]* commands/after C commands/' | sort | uniq -c | sort -rn | head -30
grep -E "panicked|test result" -A2 $r/dbg.txt | head
