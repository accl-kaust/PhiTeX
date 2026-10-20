#!/bin/bash
# r4-latexmk (accl cmd task): one pdflatex run against one
# `phitex --compat=pdflatex` run on the same starting state: A from the
# sources alone, B with the files pdflatex's run A left (both sides given
# pdflatex's). Compares the PDF (md5), the .aux and the log (lines without
# the banner, paths and memory). Env: DOCS (fixture names under
# oracle/fixtures/latexmk, or "amsldoc"), FEATURES.
set -u
export CARGO_BUILD_JOBS=16
f=""; for x in ${FEATURES:-}; do f="$f --features phitex-tex/$x"; done
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli $f > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
grep -q '^EXIT 0$' "$r/build.txt" || { echo "build failed: $(grep -m3 '^error' -A5 "$r/build.txt")" | tee "$r/summary.txt"; exit 1; }
P=$w/t/release/phitex
S=$(pwd)
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
docs=${DOCS:-$(ls oracle/fixtures/latexmk | grep -v '\.' ) amsldoc}
norm() { sed -E -e '/^This is pdfTeX/d' -e '/^ restricted \\write18/d' -e '/^entering extended mode/d' -e '/^ ?%&-line parsing/d' -e '/^\*\*/d' -e '/^Here is how much/,$d' -e 's#/[^ ()]*/texmf[^ ()]*/##g' "$1"; }
for d in $docs; do
  if [ "$d" = amsldoc ]; then src=bench/amsldoc; else src=oracle/fixtures/latexmk/$d; fi
  for side in tl ph; do rm -rf $w/c/$d-$side; mkdir -p $w/c/$d-$side; cp -a $src/. $w/c/$d-$side/; done
  for run in A B; do
    if [ $run = B ]; then
      # (both sides start from what pdflatex's run A left)
      for x in $w/c/$d-tl/*; do case $x in *.tex|*.pdf|*.log) ;; *) [ -f "$x" ] && cp "$x" $w/c/$d-ph/;; esac; done
    fi
    mkdir -p $r/$d/$run
    for side in tl ph; do
      D=$w/c/$d-$side
      if [ $side = tl ]; then
        (cd $D && timeout 600 pdflatex -interaction=${MODE:-nonstopmode} $d.tex > term.txt 2>&1; echo "exit $?" >> term.txt)
      else
        (cd $D && PHITEX_CACHE_DIR=$w/ccache NO_COLOR=1 timeout 600 $P --compat=pdflatex -interaction=${MODE:-nonstopmode} $d.tex > term.txt 2>&1; echo "exit $?" >> term.txt)
      fi
      for x in $D/$d.* $D/term.txt; do case $x in *.tex) ;; *) cp "$x" $r/$d/$run/$side-$(basename $x);; esac; done
    done
    O=$r/$d/$run
    pm() { md5sum < "$1" 2>/dev/null | cut -c1-8; }
    a=$(cmp -s $O/tl-$d.aux $O/ph-$d.aux && echo same || echo DIFF)
    norm $O/tl-$d.log > $O/tl.lognorm; norm $O/ph-$d.log > $O/ph.lognorm
    l=$(diff $O/tl.lognorm $O/ph.lognorm | grep -c '^[<>]')
    pt=$(pm $O/tl-$d.pdf); pp=$(pm $O/ph-$d.pdf)
    echo "$d $run: pdf $([ "$pt" = "$pp" ] && echo same || echo DIFF) ($pt $pp) aux $a log $l lines differ; exits $(tail -1 $O/tl-term.txt)/$(tail -1 $O/ph-term.txt)" | tee -a $r/summary.txt
  done
done
