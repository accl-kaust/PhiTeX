#!/bin/bash
# r3-cuts (accl cmd task): build the observe CLI (or use $BIN), run lu
# (plain), amsidx (a latexmk fixture), amsldoc, or a document of bench/r3
# (its oracle made here by pdftex), with the traces asked for.
# Env: DOCS="lu amsidx ams" J="1" TRACE="PHITEX_CAND_TRACE=1 ..."
set -u
R=$PWD
if [ -z "${BIN:-}" ]; then
  cargo build -q --profile fastdev -p phitex-cli --features phitex-tex/observe > $r/build.txt 2>&1 || { tail -60 $r/build.txt; exit 1; }
  BIN=$R/target/fastdev/phitex
fi
mkdir -p $w/luo && cp bench/annot/lu.tex $w/luo/ && (cd $w/luo && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 pdftex -interaction=batchmode lu.tex > /dev/null 2>&1); cp $w/luo/lu.pdf $w/lu-oracle.pdf; cp $w/luo/lu.pdf $w/luo/lu.log $r/ 2>/dev/null
TR=${TRACE-PHITEX_CAND_TRACE=1 PHITEX_TEX_TRACE=1}
for d in ${DOCS:-lu amsidx ams}; do
 for j in ${J:-1}; do
  case $d in
    lu) src=bench/annot; f=lu; epoch=0; prog="--program pdftex" ;;
    amsidx) src=oracle/fixtures/latexmk/amsidx; f=amsidx; epoch=0; prog="" ;;
    ams) src=bench/amsldoc; f=amsldoc; epoch=1700000000; prog="" ;;
    *) src=bench/r3; f=$d; epoch=0; prog="--program pdftex"
       mkdir -p $w/ora-$d && cp bench/r3/$d.tex $w/ora-$d/ && (cd $w/ora-$d && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 pdftex -interaction=batchmode $d.tex > /dev/null 2>&1); cp $w/ora-$d/$d.pdf $r/$d-oracle.pdf 2>/dev/null; cp $w/ora-$d/$d.log $r/$d-oracle.log 2>/dev/null ;;
  esac
  D=$w/run-$d-$j; rm -rf $D; mkdir -p $D; cp -a $src/. $D/
  ( cd $D && env SOURCE_DATE_EPOCH=$epoch FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache $TR timeout 1200 $BIN build $f.tex --exact-pdf $prog -j $j > $r/$d-j$j.err 2>&1; echo "exit $?" >> $r/$d-j$j.err )
  cp $D/$f.pdf $r/$d-j$j.pdf 2>/dev/null; cp $D/$f.log $r/$d-j$j.log 2>/dev/null
  m=$(md5sum < $D/$f.pdf 2>/dev/null | cut -c1-32)
  case $d in lu) o=$(md5sum < $w/lu-oracle.pdf 2>/dev/null | cut -c1-32) ;; amsidx) o=$(md5sum < oracle/fixtures/latexmk/amsidx.pdf | cut -c1-32) ;; ams) o=98c96e004c1c82db1e3d562cf0181a3e ;; *) o=$(md5sum < $r/$d-oracle.pdf 2>/dev/null | cut -c1-32) ;; esac
  echo "$d j$j: $(tail -1 $r/$d-j$j.err) md5 $m oracle $o $([ "$m" = "$o" ] && echo SAME || echo DIFFERS) pages $(grep -ac '/Type /Page\b' $D/$f.pdf 2>/dev/null) $(grep -aoE '[0-9]+ nodes run of [0-9]+ live' $r/$d-j$j.err | tail -1)" | tee -a $r/summary.txt
 done
done
