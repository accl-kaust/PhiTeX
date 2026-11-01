#!/bin/bash
# r11: the thread caches' hits and misses (PHITEX_CACHE_STATS) and the
# engine's probes (PHITEX_OBS) by thread count.
source "$(dirname "$0")/lib.sh"
warm
C="$P PHITEX_CACHE_STATS=1"
for k in 1 2; do
  for J in 1 8 16; do ams ams-j$J-$k "$C"; done
  J=8 ams ams-w1-j8-$k "$C CLEAN_WINDOW=1"
  J=8 ams ams-w1pool-j8-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
  for J in 1 8; do para para-j$J-$k "$C"; done
  J=8 para para-w1pool-j8-$k "$C CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
done
for t in ams-j1-1 ams-j8-1 ams-j16-1 ams-w1-j8-1 ams-w1pool-j8-1 para-j1-1 para-j8-1 para-w1pool-j8-1; do
  { echo "== $t"; grep -h '^acct: thread caches' $r/$t.err; } >> $S
done
for J in 1 8; do ams ams-obs-j$J "PHITEX_OBS=1"; para para-obs-j$J "PHITEX_OBS=1"; done
J=8 ams ams-obs-w1pool-j8 "PHITEX_OBS=1 CLEAN_WINDOW=1 PHITEX_POOL_SINGLE=1"
echo "end $(date +%T)" | tee -a "$S"
