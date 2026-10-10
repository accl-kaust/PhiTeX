#!/bin/bash
# bench/flow-par/abpin.sh NAME=REV...: each revision's phitex-flow built, then big2
# cold at -j N pinned to N physical cores (percore), revisions interleaved,
# REPS each, with the scheduler's events (round 1's wall, lock waits).
set -u
R=${r:-/tmp/abpin-results}; W=${w:-/tmp/abpin-work}/abpin; mkdir -p "$R" "$W"
P=$PWD
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
NS=${NS:-"1 2 4 8"}; REPS=${REPS:-3}
log() { echo "$*" | tee -a "$R/summary.txt"; }
names=()
for spec in "$@"; do
  name=${spec%%=*}; rev=${spec#*=}; names+=("$name")
  git cat-file -e "$rev^{commit}" 2>/dev/null || git fetch -q origin "refs/wip/$(git rev-parse --verify -q "$rev" || echo "$rev")" 2>/dev/null || git fetch -q origin "+refs/wip/*:refs/wip/*"
  git worktree add -q --detach "$W/wt-$name" "$rev"
  (cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" cargo build --release -q -p phitex-flow --bin phitex-flow) > "$R/build-$name.log" 2>&1 \
    || { log "build $name failed"; tail -20 "$R/build-$name.log"; }
  cp "$W/t-$name/release/phitex-flow" "$W/bin-$name"
  log "$name = $(git rev-parse --short "$rev")"
done
mkdir -p $W/j; cp bench/flow-par/big2.tex $W/j/
for n in $NS; do
  cpus=$(python3 bench/flow-par/cpus.py percore $n) || { log "-j $n: no cpus"; continue; }
  for rep in $(seq 1 $REPS); do
    for name in "${names[@]}"; do
      f=$R/sched-$name-j$n-r$rep.tsv; rm -f $f
      read s rss st < <(PIN=$cpus PHITEX_SCHED=$f python3 bench/flow-par/meas.py $W/j/out.txt "$W/bin-$name" $W/j/big2.tex -j $n -o $W/j/big2.dvi)
      r1=$(python3 bench/flow-par/sched.py $f | grep -m1 '^==' | grep -oE 'wall [0-9.]+ ms')
      lk=$(python3 bench/flow-par/sched.py $f | grep -m1 'scheduler lock' | sed 's/^ *//')
      log "$name -j $n rep $rep: wall ${s}s st $st; $(grep -m1 -oE '^cold: [0-9.]+ ms' $W/j/out.txt); round 1 $r1; $lk; md5 $(md5sum < $W/j/big2.dvi | cut -c1-12)"
      gzip -f $f
    done
  done
done
