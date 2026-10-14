#!/bin/bash
# tikzman's quick gate (<= 15 min): one fastdev build of the test
# targets, then the area's tests and the smoke set in parallel, fresh
# formats (each run's target tmp is new in the job's clone).
set -u
T="cargo test -q --profile fastdev"
t0=$(date +%s)
$T -p phitex-tex --test diagnostics --test tikzman_repros --test plain --test latex --test chain --no-run 2>&1 | tail -3
$T -p phitex-cli --bin phitex --test tikzman_repros --test ladder --no-run 2>&1 | tail -3
echo "built in $(( $(date +%s) - t0 )) s"
run() { local n=$1; shift; ( $T "$@" > $r/qg-$n.txt 2>&1; echo "EXIT $? $n" >> $r/qg-$n.txt ) & }
run diag -p phitex-tex --test diagnostics
run repros-tex -p phitex-tex --test tikzman_repros
run repros-cli -p phitex-cli --test tikzman_repros
run clibin -p phitex-cli --bin phitex
run story -p phitex-tex --test plain story -- --exact
run hello -p phitex-tex --test latex pdflatex_hello -- --exact
run paras30 -p phitex-tex --test latex latex_paragraph_nodes -- --exact
run envs -p phitex-tex --test latex latex_environments -- --exact
run ladder08 -p phitex-cli --test ladder r08_hyperref -- --ignored --exact
run para400 -p phitex-tex --test chain para400 -- --exact --ignored
wait
for f in $r/qg-*.txt; do echo "== $(basename $f): $(grep -E 'test result' $f | tail -1) $(tail -1 $f)"; grep -E '^    [a-z_0-9]+$|panicked at' $f | head -4; done
echo "gate wall $(( $(date +%s) - t0 )) s"
