#!/bin/bash
# floats2col at -j16 with the window held at 0 (no text past the frontier): run until a build's
# finisher runs pass 300 (MAXTRY tries), keep its run and wake logs; who re-asks the finishers.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache timeout 900 $B build warm.tex > /dev/null 2>&1)
for i in $(seq 1 ${MAXTRY:-30}); do
  # (FRESH=1: each try on an empty cache, no records nor hints from a run before)
  [ -n "${FRESH:-}" ] && rm -rf $w/cache
  rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
  (cd $w/f && env PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 PHITEX_WINDOW_FLOOR=0 PHITEX_WINDOW_PIN=1 PHITEX_WINDOW_START=0 ${TRACE_EXTRA:-} PHITEX_TEX_TIME=1 PHITEX_RUN_LOG=1 PHITEX_WAKE_LOG=1 PHITEX_CACHE_DIR=$w/cache timeout 300 $B build -j16 --exact-pdf floats2col.tex > $w/log.txt 2>&1)
  n=$(grep -c '^run: node [0-9]* (finish)' $w/log.txt)
  echo "try $i: exit $? finisher runs $n $(grep '^window:' $w/log.txt | tail -1) | evaluations $(grep -c '^window:' $w/log.txt) | $(grep -E '^acct: runs/live nodes by op' $w/log.txt | grep -oE 'finish [0-9]+/[0-9]+ \([0-9.]+\)' | tr '\n' ' ')"
  if [ "$n" -gt ${OVER:-300} ]; then gzip -c $w/log.txt > $r/log.txt.gz; break; fi
done
python3 -I bench/fltrace-tmp.py $w/log.txt; python3 -I bench/fltrace2-tmp.py $w/log.txt
