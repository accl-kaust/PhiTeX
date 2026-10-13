#!/bin/bash
# speculation A/B (Predicted vs All:256, 4 threads, 3 reps) and the apply/run split at 1 and 8 threads
r=$HOME/code/flinner/phitex-runs/cmd-${SLURM_JOB_ID:-x}; mkdir -p $r
export PHITEX_CACHE_DIR=$PWD/target/phcache
cores=$(python3 -c "
import os
f={}
for c in sorted(os.sched_getaffinity(0)): f.setdefault(open(f'/sys/devices/system/cpu/cpu{c}/topology/core_id').read().strip(), c)
print(' '.join(map(str,sorted(f.values()))))")
echo "commit $(git rev-parse --short HEAD) cores $cores"
st() { grep -E "^\s*($1):" "$2" | tr -s ' ' | tr '\n' ' '; }
bin=$(cargo test --release -p phitex-tex --test profile --no-run --message-format=json 2>/dev/null | grep -o '"executable":"[^"]*"' | tail -1 | cut -d'"' -f4)
[ -x "$bin" ] || { echo no bin; exit 1; }
for t in 1 4 8; do c4=$(echo $cores | tr ' ' '\n' | head -$t | paste -sd,)
for d in para4000 para400; do for rep in 1 2 3; do for v in predicted all:256; do
  m=${v%%:*}; b=${v#*:}; [ $b = $v ] && b=256
  o=$r/s-$d-${v/:/-}-t$t-$rep.txt
  PHITEX_SPEC=$m PHITEX_SPEC_BUDGET=$b PHITEX_PROFILE_DOC=$PWD/scratch/gen/$d.tex PHITEX_THREADS=$t taskset -c $c4 $bin --ignored --exact profile_doc --nocapture > $o 2>&1
  echo "S $d $v t=$t rep=$rep $(st 'nodes_run|runs|wall|busy|critical_path|speculations|confirmations|mispredictions|spec_stops' $o)"
  grep -m2 -E "panicked|FAILED" $o
done; done; done; done
bin=$(cargo test --release --features profile -p phitex-tex --test profile --no-run --message-format=json 2>/dev/null | grep -o '"executable":"[^"]*"' | tail -1 | cut -d'"' -f4)
[ -x "$bin" ] || { echo no profile bin; exit 1; }
for t in 1 8; do for rep in 1 2; do
  cpus=$(echo $cores | tr ' ' '\n' | head -$t | paste -sd,)
  o=$r/a-t$t-$rep.txt
  PHITEX_OBS=1 PHITEX_STEP_TRACE=1 PHITEX_SPEC=predicted PHITEX_PROFILE_DOC=$PWD/scratch/gen/para4000.tex PHITEX_THREADS=$t taskset -c $cpus $bin --ignored --exact profile_doc --nocapture > $o 2>&1
  echo "== A t=$t rep=$rep $(grep -E '^\s*wall:' $o | tr -s ' ')"
  awk '/^probes/{blk=""} {blk=blk"\n"$0} END{print blk}' $o | grep -E "^\s+(evaluate|run_batch|apply_batch) "
done; done
echo done
