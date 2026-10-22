#!/bin/bash
# base vs variant, one job: (A) the 40-edit proof with CLEAN_KEPT_CHECK=1 on both; (B) edit kinds interleaved for time;
# (C) the run audit per edit on both.   bash bench/fc/proofab.sh BASE,VARIANT
set -u
IFS=, read -r BASE VAR <<< "$1"
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
S=$(date +%s); B=$w/pab-$S; mkdir -p "$B"; export CARGO_TARGET_DIR=$w/pab-target
for c in $BASE $VAR; do
  git worktree add -q --detach "$B/wt-$c" "$c" || { echo "worktree $c failed"; exit 1; }
  ln -s "$PWD/upstream" "$B/wt-$c/upstream"
  (cd "$B/wt-$c" && cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/b-$c.log" 2>&1) || { echo "build $c failed"; tail "$r/b-$c.log"; exit 1; }
  mkdir -p "$B/e-$c"; cp "$CARGO_TARGET_DIR/release/phitex" "$B/e-$c/phitex"
  git worktree remove --force "$B/wt-$c"
done
for c in $BASE $VAR; do export PHITEX_CACHE_DIR=$B/tc-$c
  mkdir -p "$B/warm-$c" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$B/warm-$c/warm.tex"
  (cd "$B/warm-$c" && "$B/e-$c/phitex" build warm.tex > /dev/null 2>&1); done
run() { # c j seq tag env...
  local c=$1 j=$2 seq=$3 tag=$4; shift 4
  E=$B/ek-$tag-$c-$j-${seq//,/}; mkdir -p "$E" && cp -a bench/amsldoc/. "$E/"
  env "$@" PHITEX_CACHE_DIR=$B/tc-$c EK_KINDS=$seq EDIT_STDERR="$r/$tag-$c-$j-${seq//,/}.err" timeout 2400 python3 -I bench/fc/editkinds.py "$B/e-$c/phitex" "$E" amsldoc.tex $j "$r/$tag-$c-$j-${seq//,/}.json" --exact > "$r/$tag-$c-$j-${seq//,/}.out"
}
echo "== A: proof with CLEAN_KEPT_CHECK=1"
for j in 8 1; do for seq in 0,4 0,1,2,3 0,4,5,6,7 0,1,2,3,4,5,6,7,8; do for c in $BASE $VAR; do
  run $c $j $seq A CLEAN_KEPT_CHECK=1
  f=$r/A-$c-$j-${seq//,/}
  echo "$c j$j seq $seq: $(grep -c ' exact ' $f.out) exact, $(grep -c DIFFERS $f.out) differ, $(wc -l < $f.out) edits; kept-check lines $(grep -aic 'kept' $f.err), panics $(grep -ac 'panicked' $f.err)"
done; done; done
echo "== B: time, interleaved (load $(cut -d' ' -f1-3 /proc/loadavg))"
for rep in 1 2; do for j in 8 1; do for c in $BASE $VAR; do
  run $c $j 0,1,2,3,4,5,6,7,8 B$rep
  echo "$c j$j rep $rep: $(awk '{for(i=1;i<=NF;i++) if($i=="ms") {printf "%s ", $(i-1); break}}' $r/B$rep-$c-$j-012345678.out) load $(cut -d' ' -f1 /proc/loadavg)"
done; done; done
echo "== C: run audit per edit (j8)"
for c in $BASE $VAR; do
  run $c 8 0,1,2,3,4,5,6,7,8 C PHITEX_RUN_AUDIT=1 PHITEX_ROUND_ACCT=1 EK_NO_COLD=1
  echo "-- $c"; grep -a '^acct-edit: audit' $r/C-$c-8-012345678.err | tail -n +2 | grep -o 'reruns [0-9]* in [0-9.]* ms, wasted [0-9]* in [0-9.]* ms' | nl
done
gzip -f $r/*.err
echo "end $(date +%T)"
