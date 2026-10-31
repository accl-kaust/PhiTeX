#!/bin/bash
# amsldoc through watch as the gate runs it (cold + one edit), -j1 then -j4, the evaluation's diagnostics kept.
set -u; here=$PWD; R=$here/../results/ams; mkdir -p $R
git log --oneline -1
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head -20
for j in 1 8; do
  rm -rf target/ams && cp -a bench/amsldoc target/ams
  (cd target/ams && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_DECL_TRACE=1 PHITEX_CACHE_DIR=$here/target/acache timeout 900 python3 -I $here/bench/watchedit.py $here/target/release/phitex . amsldoc.tex amsldoc.tex 374 vertically bertically $j $R/err-$j.log > $R/out-$j.log 2>&1)
  echo "== j$j: $(cat $R/out-$j.log | tail -1)"
  grep -E "^evaluation|causes named|^round rerun|^round: " $R/err-$j.log | cut -c1-1500
  echo "-- declarations and environment maps, cold | edit (split at the second incr walk), counted"
  awk '/^incr walk/{w++} /^decl-trace/{print (w<2?"cold ":"edit ") $0}' $R/err-$j.log | sort | uniq -c | cut -c1-600
done
