#!/bin/bash
# picloop.sh N TEST : runs one latex test N times while three other latex
# tests loop beside it on the job's cores; keeps the output of
# each failure and each run's -j8 summary line (results in ./picloop/).
set -u
N=$1; T=$2
L=$(cargo test -q --profile fastdev -p phitex-tex --features profile,observe --test latex --no-run --message-format=json 2>/dev/null | grep -o '"executable":"[^"]*latex-[^"]*"' | tail -1 | cut -d'"' -f4)
echo "binary $L"
mkdir -p picloop; cd crates/engines/tex
"$L" "$T" --nocapture > ../../../picloop/warm.txt 2>&1; grep -a "^-j\|test result\|panicked" ../../../picloop/warm.txt | cut -c1-300
for t in latex_a_picture_asked_again_runs_again latex_newpage_chain_settles a_heading_edit_does_not_run_the_rest_of_begin_document_again; do
  ( while [ ! -e ../../../picloop/stop ]; do "$L" "$t" > /dev/null 2>&1; done ) &
done
for i in $(seq "$N"); do
  [ "$i" = 1 ] || [ -s ../../../picloop/o.txt ] || break
  "$L" "$T" --nocapture > ../../../picloop/o.txt 2>&1
  grep -a "^-j8" ../../../picloop/o.txt | head -1 | sed "s/(pictures moved).*asked/asked/" >> ../../../picloop/j8.txt
  if grep -q "test result: FAILED" ../../../picloop/o.txt; then cp ../../../picloop/o.txt ../../../picloop/fail-$i.txt; fi
done
touch ../../../picloop/stop; wait
echo "runs $N failed $(ls ../../../picloop | grep -c fail-)"
sort ../../../picloop/j8.txt | uniq -c | sort -rn | head -20
for f in ../../../picloop/fail-*; do [ -e "$f" ] || continue; echo "== $f"; grep -a "^-j\|panicked" -A1 "$f" | cut -c1-6000; done
