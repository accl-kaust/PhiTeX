#!/bin/bash
# amsldoc one-word edit, all of it: rounds, step trace (run vs apply),
# per-node times, re-run nodes with causes. Relaxed (default) -j8.
set -u
D=/usr/share/doc/texlive/latex/amsmath
cargo build --release -q -p phitex-cli || exit 1
mkdir -p $w/bin && cp target/release/phitex $w/bin/phitex
for mode in relaxed exact; do
  [ $mode = exact ] && export TIKZMAN_EXACT=1
  echo "### $mode $(date +%T)"
  PHITEX_T_RERUNS=1 TIKZMAN_RAW_NODETIME=1 PHITEX_STEP_TRACE=1 PHITEX_T_CHAIN=1 PHITEX_SNAPSHOT_SECS=60 \
    python3 scripts/accl/tikzman-prof.py phases $w/bin/phitex $D amsldoc.tex amsldoc.tex 374 vertically bertically 8 ams-$mode
  sed -n '/===== edit /,$p' $r/ams-$mode-stderr.txt | grep -vE 'nodetime|steprun|stepgap|rerun:|chain' | cut -c1-600
  echo "-- edit: nodetime by op"
  sed -n '/===== edit /,$p' $r/ams-$mode-stderr.txt | awk '$2=="nodetime:"{n[$3]++; s[$3]+=$5} END{for(k in n) printf "%s %d runs %.1f ms\n", k, n[k], s[k]}'
  echo "-- edit: batches"
  sed -n '/===== edit /,$p' $r/ams-$mode-stderr.txt | awk '$2=="steprun:"{b++; run+=$6/1000} $2=="stepgap:"{g+=$6/1000} END{printf "%d batches, run %.1f ms, gaps (engine apply) %.1f ms\n", b, run, g}'
done
echo "### done $(date +%T)"
