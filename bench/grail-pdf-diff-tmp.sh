#!/bin/bash
# grail-pdf-diff-tmp.sh: the user's Grail paper (xelatex) built cold at -j1 and -j8 with PHITEX_XDV_DIFF=1:
# the PDF the engine writes from its pages against the xdvipdfmx port's PDF of the same build's XDV
set -u
(cargo build -q --release -p phitex-cli > $r/build.txt 2>&1) || { tail -30 $r/build.txt > $r/summary.txt; exit 1; }
bin=$PWD/target/release/phitex
: > $r/summary.txt
for j in 1 8; do
  rm -rf $w/g$j; cp -a $w/bin/grail-src $w/g$j; cd $w/g$j
  t0=$(date +%s.%N)
  PHITEX_XDV_DIFF=1 PHITEX_XDV_DIFF_DIR=$w/diff-j$j PHITEX_CACHE_DIR=$w/pc timeout 3000 $bin build -j $j grail.tex > $r/build-j$j.out 2>&1
  e=$?
  echo "-j$j: exit $e, $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}') s, $(grep -c '^xdv-diff:' $r/build-j$j.out) checks" >> $r/summary.txt
  grep '^xdv-diff:' $r/build-j$j.out | sort | uniq -c >> $r/summary.txt
  ls -l grail.pdf >> $r/summary.txt 2>&1
  md5sum grail.pdf $w/diff-j$j/* >> $r/summary.txt 2>&1
  cd - > /dev/null
done
