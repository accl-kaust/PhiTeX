#!/bin/bash
# r4-latexmk (accl cmd task): each bench/r4/docs/*.tex (or DOCS) run once
# by pdflatex and once by `phitex --compat=pdflatex`, batchmode; the
# normalized logs' differences and the PDFs' md5s.
set -u
export CARGO_BUILD_JOBS=16
f=""; for x in ${FEATURES:-}; do f="$f --features phitex-tex/$x"; done
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli $f > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
grep -q '^EXIT 0$' "$r/build.txt" || { echo "build failed: $(grep -m3 '^error' -A5 "$r/build.txt")" | tee "$r/summary.txt"; exit 1; }
P=$w/t/release/phitex
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
norm() { sed -E -e '/^This is pdfTeX/d' -e '/^ restricted \\write18/d' -e '/^entering extended mode/d' -e '/^ ?%&-line parsing/d' -e '/^\*\*/d' -e '/^Here is how much/,$d' -e 's#/[^ ()]*/texmf[^ ()]*/##g' "$1"; }
for t in ${DOCS:-bench/r4/docs/*.tex}; do
  d=$(basename $t .tex)
  for side in tl ph; do
    D=$w/c/$d-$side; rm -rf $D; mkdir -p $D; cp $t $D/
    if [ $side = tl ]; then (cd $D && timeout 600 pdflatex -interaction=batchmode $d.tex > term.txt 2>&1)
    else (cd $D && PHITEX_CACHE_DIR=$w/ccache NO_COLOR=1 ${PHITEX_ENV:-} timeout 600 $P --compat=pdflatex -interaction=batchmode $d.tex > term.txt 2>&1); fi
    mkdir -p $r/$d; cp $D/$d.log $r/$d/$side.log; cp $D/term.txt $r/$d/$side-term.txt; norm $D/$d.log > $r/$d/$side.norm
  done
  diff $r/$d/tl.norm $r/$d/ph.norm > $r/$d/diff.txt
  echo "$d: pdf $(md5sum < $w/c/$d-tl/$d.pdf | cut -c1-8) $(md5sum < $w/c/$d-ph/$d.pdf | cut -c1-8); log $(grep -c '^[<>]' $r/$d/diff.txt) lines differ" | tee -a $r/summary.txt
  grep -E '^[<>].*(=[0-9]|line [0-9])' $r/$d/diff.txt | head -12 | tee -a $r/summary.txt
done
