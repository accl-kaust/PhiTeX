#!/bin/bash
# (temporary) specwin2: amsldoc -j16 with the chain holds, the run log's waits and runs
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 || { echo BUILD FAILED; exit 1; }
B=$PWD/target/release/phitex
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache $B build warm.tex > /dev/null 2>&1)
for h in 1 0; do
  rm -rf $w/c && cp -a bench/amsldoc $w/c
  (cd $w/c && PHITEX_CLEAN_HOLDS=$h PHITEX_RUN_LOG=1 PHITEX_CACHE_DIR=$w/cache $B build -vv -j16 --exact-pdf amsldoc.tex > $r/ams-$h.txt 2>&1)
  echo "holds $h: $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' $r/ams-$h.txt | tail -1) waits $(grep -c '^chain: node' $r/ams-$h.txt)"
  gzip -f $r/ams-$h.txt
done
