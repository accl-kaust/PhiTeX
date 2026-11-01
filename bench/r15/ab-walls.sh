#!/bin/bash
# Walls of BEFORE_REV and this tree side by side on one node (on accl, in a job): release
# builds; REPS (3) of para4000 -j 1, -j 8 and amsldoc -j 8, the two interleaved.
#   scripts/accl/accl submit cmd bash bench/r15/ab-walls.sh BEFORE_REV [REPS]
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
before=$1; reps=${2:-3}; root=$PWD
echo "node $(cat /proc/sys/kernel/hostname); after $(git log --oneline -1 | cut -c1-60); before $(git log --oneline -1 "$before" | cut -c1-60)" | tee -a "$r/summary.txt"
s=$w/src-before; rm -rf "$s"; mkdir -p "$s"; git archive "$before" | tar -x -C "$s"; [ -e "$root/upstream" ] && ln -s "$root/upstream" "$s/upstream"
for nm in before after; do
  d=$root; [ $nm = before ] && d=$s
  (cd "$d" && CARGO_TARGET_DIR=$w/t-$nm cargo build -q --release -p phitex-cli > "$r/build-$nm.txt" 2>&1) || { echo "$nm build FAILED" | tee -a "$r/summary.txt"; exit 1; }
  mkdir -p "$w/bin/$nm"; cp "$w/t-$nm/release/phitex" "$w/bin/$nm/"
done
run() { # side doc j tag
  local nm=$1 d=$2 j=$3 t=$4
  local dir=$w/run-$t; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache-$nm timeout 900 /usr/bin/time -f "WALL %e" "$w/bin/$nm/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | $(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -c1-8)"
}
for nm in before after; do run $nm amsldoc 8 warm-$nm > /dev/null; run $nm para4000 8 warmp-$nm > /dev/null; done
for i in $(seq 1 "$reps"); do
  for nm in before after; do
    run $nm para4000 1 $nm-p1-$i; run $nm para4000 8 $nm-p8-$i; run $nm amsldoc 8 $nm-a8-$i
  done
done | sort | tee -a "$r/summary.txt"
echo done
