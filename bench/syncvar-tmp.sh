#!/bin/bash
# para4000 watch .synctex vs a clean build's: as is, the writer's members made fresh, the origins made fresh.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
for v in PHITEX_SYNCTEX_COMPARE; do
  rm -rf $w/pe $w/pe-cold $w/cold && mkdir -p $w/pe $w/cold && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  EDITS_STDERR=$r/err-$v.txt env $v=1 PHITEX_CACHE_DIR=$w/cache timeout 900 python3 -I bench/cut1-edits.py $B $w/pe para4000.tex 8 $w/edits.json $r/pe-$v.json --exact-pdf 2>&1 | grep verdict | sed "s|^|$v: |"
  zcat $w/pe/para4000.synctex.gz > $r/watch-$v.synctex
  cp $w/pe/para4000.tex $w/cold/ && (cd $w/cold && PHITEX_CACHE_DIR=$w/cache $B build -j8 --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold/para4000.synctex.gz > $r/cold-$v.synctex
  echo "$v: lines differing $(diff $r/watch-$v.synctex $r/cold-$v.synctex | grep -c '^[<>]')"
done
cat $w/edits.json | head -c 600
grep "synctex compare" $r/err-*.txt | cut -c1-600
