#!/bin/bash
# (temporary) floats2col's test at -j16, W at 0, with the run and wake logs: until a build's
# finisher runs pass OVER (MAXTRY tries); its log kept and summed (fltrace-tmp.py, fltrace2-tmp.py).
set -u
for i in $(seq 1 ${MAXTRY:-10}); do
  env PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 PHITEX_WINDOW_FLOOR=0 PHITEX_WINDOW_PIN=1 PHITEX_WINDOW_START=0 PHITEX_TEST_THREADS=16 \
    PHITEX_RUN_LOG=1 PHITEX_WAKE_LOG=1 ${TRACE_EXTRA:-} timeout 900 cargo test -q --release -p phitex-tex --features profile,observe --test latex floats2col_routines_run_once_per_fire -- --nocapture > $w/log.txt 2>&1
  n=$(grep -c '^run: node [0-9]* (finish)' $w/log.txt)
  echo "try $i: finisher runs $n $(grep -E 'fires' $w/log.txt | head -1 | cut -c1-80)"
  if [ "$n" -le 213 ] && [ ! -f $r/exact.txt.gz ]; then gzip -c $w/log.txt > $r/exact.txt.gz; fi
  if [ "$n" -gt ${OVER:-400} ] && [ ! -f $r/log.txt.gz ]; then gzip -c $w/log.txt > $r/log.txt.gz; cp $w/log.txt $w/bad.txt; fi
  [ -f $r/exact.txt.gz ] && [ -f $r/log.txt.gz ] && break
done
[ -f $w/bad.txt ] && cp $w/bad.txt $w/log.txt
python3 -I bench/fltrace-tmp.py $w/log.txt | head -40; python3 -I bench/fltrace2-tmp.py $w/log.txt
# (the first wakes in the log's order, of any node, after the first finisher's run)
awk '/^run: node [0-9]+ \(finish\)/{f=1} f && /^wake:/' $w/log.txt | head -40 | cut -c1-260
