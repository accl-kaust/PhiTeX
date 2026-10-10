#!/bin/bash
# bench/flow-par/abdocs.sh NAME=REV...: each revision's phitex-flow, then DOCS (LaTeX
# documents on the saved format, made by the first) at JS threads pinned to
# physical cores, revisions interleaved, REPS each: wall, cold, rounds, runs,
# output md5 (DVI and written files).
set -u
R=${r:-/tmp/abdocs-results}; W=${w:-/tmp/abdocs-work}/abdocs; mkdir -p "$R" "$W"
P=$PWD
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
DOCS=${DOCS:-"sections titlepage biglatex"}; JS=${JS:-"1 8"}; REPS=${REPS:-3}
log() { echo "$*" | tee -a "$R/summary.txt"; }
names=()
for spec in "$@"; do
  name=${spec%%=*}; rev=${spec#*=}; names+=("$name")
  git cat-file -e "$rev^{commit}" 2>/dev/null || git fetch -q origin "refs/wip/$(git rev-parse --verify -q "$rev" || echo "$rev")" 2>/dev/null || git fetch -q origin "+refs/wip/*:refs/wip/*"
  [ -d "$W/wt-$name" ] || git worktree add -q --detach "$W/wt-$name" "$rev"
  (cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" cargo build --release -q -p phitex-flow --bin phitex-flow) > "$R/build-$name.log" 2>&1 \
    || { log "build $name failed"; tail -20 "$R/build-$name.log"; }
  cp "$W/t-$name/release/phitex-flow" "$W/bin-$name"
  log "$name = $(git rev-parse --short "$rev")"
done
fmt=$W/latex.fmt
"$W/bin-${names[0]}" $(kpsewhich latex.ini) $(bench/flow-par/dirsof.sh sections) --jobname latex --terminal latex.ini -j 8 --dump-fmt $fmt -o $W/fmt.dvi > "$R/format.out" 2>&1
for d in $DOCS; do
  for j in $JS; do
    cpus=$(python3 bench/flow-par/cpus.py percore $j) || { log "-j $j: no cpus"; continue; }
    for rep in $(seq 1 $REPS); do
      for name in "${names[@]}"; do
        export J=$j EXTRA=""
        read s rss st < <(PIN=$cpus python3 bench/flow-par/meas.py $W/out.txt bench/flow-par/rundoc.sh "$W/bin-$name" $d $fmt $W/docs-$name)
        h=$( (cd $W/docs-$name/$d && for f in $(ls); do cat $f; done) | md5sum | cut -c1-12)
        c=$(grep -m1 '^cold:' $W/out.txt | grep -oE '^cold: [0-9.]+ ms; [0-9]+ of [0-9]+ commands run; [0-9]+ pieces \([0-9]+ ran')
        rd=$(grep -m1 'rounds (ms)' $W/out.txt | sed 's/^ *//')
        log "$d $name -j $j rep $rep: ${s}s st $st md5 $h | $c | $rd"
      done
    done
  done
done
