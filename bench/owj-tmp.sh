#!/bin/bash
# (temporary) offwave2: floats2col's test and beamer_second_pass at -j1, -j2, -j16, release and debug (assertions)
set -u
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC
for prof in release debug; do
  fl=; [ $prof = release ] && fl=--release
  for k in 1 2 3; do
    PHITEX_TEST_THREADS=1,2,16 timeout 2400 cargo test -q $fl -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture > $r/fl-$prof-$k.txt 2>&1
    echo "$prof floats rep $k: exit $? $(grep -E 'fires' $r/fl-$prof-$k.txt | cut -c1-70 | tr '\n' ' ') $(grep -m2 -E 'panicked|outlives|unwalked' $r/fl-$prof-$k.txt | tr '\n' ' ')"
  done
  cargo build -q $fl -p phitex-cli > /dev/null 2>&1; B=$PWD/target/$prof/phitex
  for j in 1 2 16 1 2 16; do
    rm -rf $w/b && mkdir -p $w/b && cp oracle/fixtures/corpus-repros/beamer_second_pass.tex $w/b/ && cp -r oracle/fixtures/corpus-repros/beamer_second_pass/. $w/b/ 2>/dev/null
    (cd $w/b && PHITEX_CACHE_DIR=$w/cache timeout 900 $B build -v --exact-pdf -j $j beamer_second_pass.tex > $r/b-$prof-$j.txt 2>&1); x=$?
    cmp -s $w/b/beamer_second_pass.pdf oracle/fixtures/corpus-repros/beamer_second_pass.pdf && s=same || s=DIFFERS
    echo "$prof beamer -j$j: exit $x $s $(grep -m2 -E 'panicked|outlives' $r/b-$prof-$j.txt | tr '\n' ' ')"
  done
done
