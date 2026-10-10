#!/bin/bash
# bench/flow-par/hw.sh: is big2's per-run cost growth with threads hardware or
# contention? For MODE percore (one hardware thread per core) and ht (two
# per core): (a) N independent `big2 -j 1` processes at once, each pinned
# to its own CPU (no shared state: the hardware's share); (b) one build at
# -j N pinned to N CPUs, with the scheduler's events. N in NS, REPS each.
set -u
R=${r:-/tmp/hw-results}; W=${w:-/tmp/hw-work}/hw; mkdir -p "$R" "$W"
P=$PWD
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
NS=${NS:-"1 2 4 8"}; REPS=${REPS:-3}; MODES=${MODES:-"percore ht"}
log() { echo "$*" | tee -a "$R/summary.txt"; }
log "commit $(git rev-parse --short HEAD); node $(cat /proc/sys/kernel/hostname); allowed $(python3 -c 'import os; print(sorted(os.sched_getaffinity(0)))')"
cargo build --release -q -p phitex-flow --bin phitex-flow > "$R/build.log" 2>&1 || { log "build failed"; tail -30 "$R/build.log"; exit 1; }
B=$P/target/release/phitex-flow
reading() { grep -m1 -oE 'reading [0-9.]+ ms' $1 | awk '{print $2}'; }
coldms() { grep -m1 -oE '^cold: [0-9.]+' $1 | awk '{print $2}'; }
for mode in $MODES; do
  for n in $NS; do
    cpus=$(python3 bench/flow-par/cpus.py $mode $n) || { log "$mode $n: no cpus"; continue; }
    for rep in $(seq 1 $REPS); do
      # (a) n independent processes, one per CPU
      IFS=, read -ra cs <<< "$cpus"
      for i in $(seq 0 $((n-1))); do
        mkdir -p $W/p$i; cp -n bench/flow-par/big2.tex $W/p$i/
        PIN=${cs[$i]} python3 bench/flow-par/meas.py $W/p$i/out.txt "$B" $W/p$i/big2.tex -j 1 -o $W/p$i/big2.dvi > $W/p$i/m.txt &
      done
      wait
      rs=""; ws=""
      for i in $(seq 0 $((n-1))); do rs="$rs $(reading $W/p$i/out.txt)"; ws="$ws $(coldms $W/p$i/out.txt)"; done
      log "$mode procs=$n rep $rep cpus $cpus: reading ms:$rs | cold ms:$ws"
      # (b) one build, n threads on those CPUs
      mkdir -p $W/j; cp -n bench/flow-par/big2.tex $W/j/
      f=$R/sched-$mode-j$n-r$rep.tsv; rm -f $f
      read s rss st < <(PIN=$cpus PHITEX_SCHED=$f python3 bench/flow-par/meas.py $W/j/out.txt "$B" $W/j/big2.tex -j $n -o $W/j/big2.dvi)
      log "$mode -j $n rep $rep cpus $cpus: wall ${s}s status $st; cold ms $(coldms $W/j/out.txt); reading ms $(reading $W/j/out.txt); md5 $(md5sum < $W/j/big2.dvi | cut -c1-12)"
      python3 bench/flow-par/sched.py $f | grep -E '^==|scheduler lock|all workers|readers at once|install:' | head -12 >> "$R/summary.txt"
      gzip -f $f
    done
  done
done
