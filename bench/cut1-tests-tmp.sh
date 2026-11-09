#!/bin/bash
# cut1's scoped tests (mc): the converted checkpoint tests, the cut
# tests, paragraphs/environments/pictures, under PHITEX_CUT_CHECK=1
set -u
T="cargo test -q --profile fastdev -p phitex-tex"
t0=$(date +%s)
$T --test edits --test latex --test cuts --test plain --test chain --test hints --no-run 2>&1 | tail -3
echo "built in $(( $(date +%s) - t0 )) s"
export PHITEX_CUT_CHECK=1
run() { local n=$1; shift; ( $T "$@" > $r/t-$n.txt 2>&1; echo "EXIT $? $n" >> $r/t-$n.txt ) & }
run edits --test edits -- --test-threads 8 ${EDITS_FILTER:-}
run latex --test latex -- --test-threads 8 ${LATEX_FILTER:-paragraph environments picture rerun latex_input}
run cuts --test cuts -- --test-threads 8
run plain --test plain -- --test-threads 8
run hints --test hints -- --test-threads 8
run lib --lib -- --test-threads 8
wait
for f in $r/t-*.txt; do echo "== $(basename $f): $(grep -E 'test result' $f | tail -1) $(tail -1 $f)"; grep -E '^    [a-z_0-9:]+$|panicked at' $f | head -12; echo "   cut checks: $(grep -c 'cut check:.*same' $f) same, $(grep -c 'cut check:.*DIFFERENT' $f) different"; grep -m3 'cut check:.*DIFFERENT' $f | cut -c1-400; done
echo "wall $(( $(date +%s) - t0 )) s"
