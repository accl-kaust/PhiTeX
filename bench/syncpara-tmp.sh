#!/bin/bash
# para4000 watch edits (SyncTeX on): keystroke, origins checks, the final .synctex vs a clean build's.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
for v in plain PHITEX_ORIGINS_CHECK; do for j in 8 16; do
  rm -rf $w/pe $w/pe-cold $w/cold && mkdir -p $w/pe $w/cold && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  EDITS_STDERR=$r/err-$v-$j.txt env $v=1 PHITEX_CACHE_DIR=$w/cache timeout 900 python3 -I bench/cut1-edits.py $B $w/pe para4000.tex $j $w/edits.json $r/pe-$v-$j.json --exact-pdf 2>&1 | grep verdict | sed "s|^|$v -j$j: |"
  zcat $w/pe/para4000.synctex.gz > $r/watch-$v-$j.synctex
  cp $w/pe/para4000.tex $w/cold/ && (cd $w/cold && PHITEX_CACHE_DIR=$w/cache $B build -j$j --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold/para4000.synctex.gz > $r/cold-$v-$j.synctex
  echo "$v -j$j: synctex lines differing from a clean build's $(diff $r/watch-$v-$j.synctex $r/cold-$v-$j.synctex | grep -c '^[<>]') (2: its path); origins checks $(grep -c 'origins check' $r/err-$v-$j.txt) different $(grep -c 'origins check.*DIFFERENT' $r/err-$v-$j.txt)"
done; done
