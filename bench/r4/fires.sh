#!/bin/bash
# r4-latexmk (accl cmd task): the pages that fire at a candidate a piece
# runs through (a group or conditional of its own open there), per
# document: distinct (piece, candidate) pairs over the build's rounds,
# how many with a read the routine changed; and whether the PDF equals
# latexmk -pdf's (TeX Live) in the same job. amsldoc, para4000, the
# latexmk fixtures, bench/corpus-repro, bench/r4/docs.
set -u
export CARGO_BUILD_JOBS=16
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
grep -q '^EXIT 0$' "$r/build.txt" || { echo "build failed: $(grep -m3 '^error' -A5 "$r/build.txt")" | tee "$r/summary.txt"; exit 1; }
P=$w/t/release/phitex
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
one() { # NAME SRCDIR FILE
  local n=$1 f=$3 stem=${3%.*}
  for side in ph tl; do rm -rf $w/d/$n-$side; mkdir -p $w/d/$n-$side; cp -a $2/. $w/d/$n-$side/; done
  (cd $w/d/$n-ph && PHITEX_CACHE_DIR=$w/cache PHITEX_TEX_TIME=1 timeout 900 $P build $f -j 1 > $r/$n.log 2>&1)
  (cd $w/d/$n-tl && timeout 900 latexmk -pdf -interaction=batchmode $f > $r/$n-lm.log 2>&1)
  local pairs=$(grep -a 'page fires at a candidate a piece runs through' $r/$n.log | sed -E 's/.*piece ([0-9]+) at candidate ([0-9]+) \((.*)\)$/\1 \2 \3/' | sort -u)
  local np=$(printf '%s\n' "$pairs" | grep -c . ) nr=$(printf '%s\n' "$pairs" | grep -vc 'no read seen$' )
  [ -z "$pairs" ] && nr=0
  local a=$(md5sum < $w/d/$n-ph/$stem.pdf 2>/dev/null | cut -c1-8) b=$(md5sum < $w/d/$n-tl/$stem.pdf 2>/dev/null | cut -c1-8)
  echo "$n: $np fires at a run-through candidate ($nr with a read the routine changed); pdf $([ -n "$a" ] && [ "$a" = "$b" ] && echo SAME || echo "DIFF ($a vs $b)")" | tee -a $r/summary.txt
  printf '%s\n' "$pairs" | grep -v 'no read seen$' | grep . | sed 's/^/    /' | head -8 | tee -a $r/summary.txt
}
mkdir -p $w/s/amsldoc && cp bench/amsldoc/amsldoc.tex $w/s/amsldoc/ && one amsldoc $w/s/amsldoc amsldoc.tex
mkdir -p $w/s/para4000 && cp bench/para4000.tex $w/s/para4000/ && one para4000 $w/s/para4000 para4000.tex
for f in oracle/fixtures/latexmk/*/; do n=$(basename $f); one $n $f $n.tex; done
for t in bench/corpus-repro/*; do n=$(basename $t); n=${n%.*}; mkdir -p $w/s/$n; cp $t $w/s/$n/; one cr-$n $w/s/$n $(basename $t); done
for t in bench/r4/docs/*.tex; do n=$(basename $t .tex); mkdir -p $w/s/$n; cp $t $w/s/$n/; one $n $w/s/$n $n.tex; done
