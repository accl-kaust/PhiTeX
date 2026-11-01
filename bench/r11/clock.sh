#!/bin/bash
# r11: the clock each run sees (PHITEX_CLOCK_PROBE) and op time by kind,
# by thread count and CPU placement (taskset): one CCX, two CCXs, SMT
# pairs; one job a step inline (w1) or on the pool (w1pool).
source "$(dirname "$0")/lib.sh"
grep -E 'MHz' /proc/cpuinfo | head -3 >> $S
warm
C="$P PHITEX_CLOCK_PROBE=1"
T() { echo "taskset -c $(python3 -I $root/bench/r11/cpus.py $1)"; }
for m in one ccx1 ccx2 smt4; do echo "cpus $m: $(python3 -I $root/bench/r11/cpus.py $m)"; done >> $S
for k in 1 2; do
  J=1 ams ams-j1-$k "$C"; J=8 ams ams-j8-$k "$C"; J=16 ams ams-j16-$k "$C"
  J=1 ams ams-j1-cpu0-$k "$C" $(T one)
  J=8 ams ams-j8-ccx1-$k "$C" $(T ccx1)
  J=8 ams ams-j8-ccx2-$k "$C" $(T ccx2)
  J=8 ams ams-j8-smt4-$k "$C" $(T smt4)
  J=8 ams ams-w1-j8-$k "$C CLEAN_WINDOW=1"
  J=8 ams ams-w1pool-j8-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
  J=8 ams ams-w1pool-j8-ccx1-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1" $(T ccx1)
  J=1 para para-j1-$k "$C"; J=8 para para-j8-$k "$C"
  J=8 para para-j8-ccx1-$k "$C" $(T ccx1)
  J=8 para para-w1pool-j8-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
done
for f in $r/*.err; do t=$(basename $f .err); echo "$t: $(grep -h '^acct: clock at run' $f | head -1 | sed 's/^acct: clock at run start.end by kind: //')"; done >> $S
echo "end $(date +%T)" | tee -a "$S"
