#!/bin/bash
# r11: is the inflation the clock the cores run at? The same builds with
# SCHED_IDLE spinners on the build's CPUs (they hold the cores busy, so
# the frequency governor clocks them up and they never idle, and yield at
# once to any build thread), against without; clock probe on.
source "$(dirname "$0")/lib.sh"
echo "governor $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_governor 2>&1); drivers $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_driver 2>&1); freqs $(cat /sys/devices/system/cpu/cpu0/cpufreq/scaling_available_frequencies 2>&1); boost $(cat /sys/devices/system/cpu/cpufreq/boost 2>&1); ondemand $(grep . /sys/devices/system/cpu/cpufreq/ondemand/* 2>&1 | tr '\n' ' ')" | tee -a $S
command -v chrt taskset | tee -a $S
warm
C="$P PHITEX_CLOCK_PROBE=1"
cpus() { python3 -I $root/bench/r11/cpus.py $1; }
CCX1=$(cpus ccx1); ALL=$(python3 -I -c 'import os; print(",".join(map(str, sorted(os.sched_getaffinity(0)))))')
PHYS=$(python3 -I -c '
import os
seen=set(); out=[]
for c in sorted(os.sched_getaffinity(0)):
    k=open(f"/sys/devices/system/cpu/cpu{c}/topology/core_id").read().strip()
    if k not in seen: seen.add(k); out.append(c)
print(",".join(map(str,out)))')
echo "ccx1 $CCX1 phys $PHYS" | tee -a $S
spin_on() { rm -f $w/stop; for c in ${1//,/ }; do taskset -c $c chrt -i 0 bash -c "while [ ! -f $w/stop ]; do :; done" & done; sleep 2; }
spin_off() { touch $w/stop; wait; }
for k in 1 2 3; do
  J=1 ams ams-j1-$k "$C" taskset -c $CCX1
  J=8 ams ams-j8-$k "$C" taskset -c $CCX1
  J=8 ams ams-w1pool-j8-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1" taskset -c $CCX1
  J=1 para para-j1-$k "$C" taskset -c $CCX1
  J=8 para para-j8-$k "$C" taskset -c $CCX1
  spin_on $CCX1
  J=1 ams ams-j1-spin-$k "$C" taskset -c $CCX1
  J=8 ams ams-j8-spin-$k "$C" taskset -c $CCX1
  J=8 ams ams-w1pool-j8-spin-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1" taskset -c $CCX1
  J=1 para para-j1-spin-$k "$C" taskset -c $CCX1
  J=8 para para-j8-spin-$k "$C" taskset -c $CCX1
  spin_off
  # unpinned, spinners on every physical core of the allocation
  spin_on $PHYS
  J=8 ams ams-j8-free-spin-$k "$C"
  J=16 ams ams-j16-free-spin-$k "$C"
  spin_off
done
for f in $r/*.err; do t=$(basename $f .err); echo "$t: $(grep -h '^acct: clock at run' $f | tail -1 | sed 's/^acct: clock at run start.end by kind: //')"; done >> $S
echo "end $(date +%T)" | tee -a "$S"
