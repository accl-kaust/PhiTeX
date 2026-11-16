#!/bin/bash
# latex_pictures_that_all_change_take_one_run, 48 runs, 8 at once, at dev's base and at the branch
set -u
G=$HOME/code/flinner/phitex-runs/gate-bin
cd crates/engines/tex
export CARGO_MANIFEST_DIR=$PWD
for sha in 17c45b6518978a3894866ea42c84d91a58395c47 a1e9cb6b1; do
  d=$(ls -d $G/$sha* | head -1)
  mkdir -p /tmp/pf-$sha
  cp $d/tests/latex /tmp/pf-$sha/latex
  seq 1 48 | xargs -P 8 -I{} bash -c "timeout 300 /tmp/pf-$sha/latex --exact latex_pictures_that_all_change_take_one_run > /tmp/pf-$sha/o{}.txt 2>&1; echo \$? > /tmp/pf-$sha/e{}"
  ok=$(grep -lx 0 /tmp/pf-$sha/e* | wc -l)
  echo "$sha: $ok of 48 pass"
  grep -ho "asked their paragraph [0-9]* times" /tmp/pf-$sha/o*.txt | sort | uniq -c
done
