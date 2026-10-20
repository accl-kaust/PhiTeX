#!/bin/bash
# r3-cuts (accl cmd task): each bench/corpus-repro document named in
# $DOCS built by pdflatex (run until its aux settles, at most 4 runs) and
# by PhiTeX (observe CLI, -j $J, traces from $TRACE), both PDFs kept.
set -u
R=$PWD
cargo build -q --profile fastdev -p phitex-cli --features phitex-tex/observe > $r/build.txt 2>&1 || { tail -30 $r/build.txt; exit 1; }
BIN=$R/target/fastdev/phitex
for d in $DOCS; do
  O=$w/ora-$d; rm -rf $O; mkdir -p $O; cp ${SRC:-bench/corpus-repro}/$d.tex $O/
  ( cd $O && for i in 1 2 3 4; do SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 pdflatex -interaction=batchmode $d.tex > /dev/null 2>&1; grep -q 'Rerun' $d.log || break; done )
  cp $O/$d.pdf $r/$d-oracle.pdf 2>/dev/null; cp $O/$d.log $r/$d-oracle.log 2>/dev/null
  P=$w/phi-$d; rm -rf $P; mkdir -p $P; cp ${SRC:-bench/corpus-repro}/$d.tex $P/
  ( cd $P && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/cache ${TRACE:-} timeout 600 $BIN build $d.tex --exact-pdf -j ${J:-1} > $r/$d.err 2>&1; echo "exit $?" >> $r/$d.err )
  cp $P/$d.pdf $r/$d.pdf 2>/dev/null; cp $P/$d.log $r/$d.log 2>/dev/null
  echo "$d: $(tail -1 $r/$d.err) ours $(md5sum < $P/$d.pdf | cut -c1-8) oracle $(md5sum < $O/$d.pdf | cut -c1-8)" | tee -a $r/summary.txt
done
