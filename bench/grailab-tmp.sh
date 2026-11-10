#!/bin/bash
# (temporary) specwin2: Grail clean at -j16, commits A and B in one job, alternated, 3 runs each. $1 A, $2 B.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
R=$PWD; G=${GRAIL:-$HOME/code/flinner/grail-src}; ls $G | head -3
for c in $1 $2; do
  git checkout -q "$c" -- crates Cargo.lock && cargo build -q --release -p phitex-cli > /dev/null 2>&1 && mkdir -p $w/b-$c && cp target/release/phitex $w/b-$c/phitex || { echo "BUILD $c FAILED"; exit 1; }
done
git checkout -q HEAD -- crates Cargo.lock
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache $w/b-$1/phitex build warm.tex > /dev/null 2>&1)
for i in 1 2 3; do for c in $1 $2; do
  rm -rf $w/c && mkdir -p $w/c && cp -a $G/. $w/c/ && ls $w/c/grail.tex > /dev/null || { echo "no grail.tex in $w/c"; ls $w/c | head; }
  s0=$(date +%s.%N); (cd $w/c && PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache timeout 1200 $w/b-$c/phitex build -j16 --exact-pdf grail.tex > $r/grail-$c-$i.txt 2>&1); x=$?
  echo "grail $c run $i: exit $x $(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")s $(md5sum < $w/c/grail.pdf 2>/dev/null | cut -c1-12) $(grep -oE 'acct: [0-9]+ runs' $r/grail-$c-$i.txt | tail -1)"
done; done
