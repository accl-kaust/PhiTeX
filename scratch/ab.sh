#!/bin/bash
# A/B: latex_hello with the environment provider on (A) and off (B), 300 s each.
set -u
export PHITEX_CACHE_DIR=$PWD/target/cache PHITEX_TEX_TIME=1 PHITEX_THREADS=1 PHITEX_HEARTBEAT=1
dst=$HOME/code/flinner/phitex-runs/cmd-${SLURM_JOB_ID:-x}; mkdir -p $dst
scripts/sandbox cargo test -p phitex-tex --test latex --no-run 2>&1 | tail -1
for m in A B; do
  log=$dst/test-$m.log
  s=$(date +%s)
  if [ $m = B ]; then nop=1; else nop=; fi
  timeout 300 env ${nop:+PHITEX_NO_PROVIDER=1} scripts/sandbox cargo test -p phitex-tex --test latex latex_hello -- --ignored --nocapture > $log 2>&1
  echo "== $m: status $? wall $(( $(date +%s) - s )) s; rounds $(grep -c '^round' $log); hb $(grep -c '^hb' $log)"
  grep -E '^round|^provider|test result|panicked' $log | cut -c1-300 | tail -12
  echo "-- last hb:"; grep '^hb' $log | tail -3 | cut -c1-300
done
