#!/bin/bash
# (mc job) callgrind Ir of a cold -j1 build, base (BASE commit) and this tree: bench/errctx/cg.sh BASE DOC...
set -u
base=$1; shift
top=$(pwd); mkdir -p $top/target
git worktree add -f $top/target/cg-base-src "$base" > /dev/null 2>&1 || { git fetch -q origin "$base" 2>/dev/null; git worktree add -f $top/target/cg-base-src "$base"; }
for t in new base; do
  d=$top; [ $t = base ] && d=$top/target/cg-base-src
  (cd $d && CARGO_TARGET_DIR=$top/target/cg-$t cargo build -q --release -p phitex-cli) || exit 1
done
for doc in "$@"; do
  for t in base new; do
    w=$(mktemp -d -p $top/target); src=$(dirname $doc); case $src in /*) ;; *) src=$top/$src ;; esac; cp -r "$src"/. $w/
    (cd $w && export PHITEX_GRAPH=0 PHITEX_CACHE_DIR=$top/target/cg-cache-$t SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
     $top/target/cg-$t/release/phitex build -q -j 1 $(basename $doc) > /dev/null 2>&1; rm -f *.pdf
     valgrind --tool=callgrind --trace-children=yes --callgrind-out-file=$w/cg.out.%p $top/target/cg-$t/release/phitex build -q -j 1 $(basename $doc) > $w/out.txt 2>&1
     ir=$(for f in $w/cg.out.*; do callgrind_annotate $f 2>/dev/null | grep -m1 "PROGRAM TOTALS" | awk '{print $1}' | tr -d ,; done | awk '{s+=$1} END {print s}')
     for f in $w/cg.out.*; do callgrind_annotate $f 2>/dev/null | head -80 > $r/ann-$t-$(basename $doc .tex)-$(basename $f).txt; done
     echo "$doc $t Ir $ir $(md5sum < ${w}/$(basename ${doc%.tex}).pdf | cut -c1-10)" | tee -a $r/cg.txt)
    rm -rf $w
  done
done
