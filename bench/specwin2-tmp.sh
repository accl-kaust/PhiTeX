#!/bin/bash
# specwin: floats2col -j16 with the window on, 20 times (observe build): a stall's dump.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
for i in $(seq 1 20); do
  rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
  (cd $w/f && env PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 PHITEX_OBS=1 PHITEX_STALL_SECS=8 PHITEX_CACHE_DIR=$w/cache timeout 40 $B build -j16 --exact-pdf floats2col.tex > $r/f-$i.txt 2>&1); x=$?
  echo "run $i: exit $x $(grep -h '^window:' $r/f-$i.txt | tail -1)"
  if [ $x != 0 ]; then grep -A40 "^stall:" $r/f-$i.txt | head -60 | cut -c1-600; break; fi
done
