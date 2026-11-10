#!/bin/bash
# specwin: floats2col and para4000 by the CLI with the window on (a hang shows as a timeout).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
for j in 1 8 16; do
  rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
  s0=$(date +%s.%N); (cd $w/f && env PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 PHITEX_CACHE_DIR=$w/cache timeout 120 $B build -j$j --exact-pdf floats2col.tex > $r/f-$j.txt 2>&1); x=$?
  echo "floats2col -j$j: exit $x $(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")s $(md5sum < $w/f/floats2col.pdf 2>/dev/null | cut -c1-12) $(grep -h '^window:' $r/f-$j.txt | tail -1)"; tail -3 $r/f-$j.txt | cut -c1-200
done
