#!/bin/bash
# The lane A/B (on accl, in a job): one release build and one profile build of this commit;
# para4000 at -j 1 8 16 32 and amsldoc at -j 1 8, REPS times, CLEAN_LANE=0 and =1 interleaved;
# then per side one profile run at -j 8 of each document with CLEAN_PROF=1 PHITEX_TEX_TIME=1
# (the clean counts and clean prof lines, the acct lines: steps, critical path).
#   scripts/accl/accl submit cmd bash bench/r6/ab-lane.sh REPS
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
reps=${1:-3}
git log --oneline -1
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -60 | tee -a "$r/summary.txt"; exit 1; }
mkdir -p "$w/bin/rel" "$w/bin/prof"; cp target/release/phitex "$w/bin/rel/phitex"
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-prof.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build-prof.txt" | head -60 | tee -a "$r/summary.txt"; exit 1; }
cp target/release/phitex "$w/bin/prof/phitex"
echo "node $(cat /proc/sys/kernel/hostname), $(nproc) CPUs, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2)" | tee -a "$r/summary.txt"
run() { # bin lane doc j tag [env...]
  local b=$1 l=$2 d=$3 j=$4 t=$5; shift 5
  local dir=$w/run-$b-$l-$d; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && env CLEAN_LANE=$l "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S" "$w/bin/$b/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  printf "lane=%s %-9s j%-3s %s | %s | %s\n" "$l" "$d" "$j" "$(grep -oE 'WALL.*' "$r/$t.log")" "$(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs.*definitions' "$r/$t.log" | tail -1 | sed 's/guesses refuted: //')" "$(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -c1-8)"
}
for l in 0 1; do run rel $l amsldoc 8 warm-$l > /dev/null; run rel $l para4000 8 warm-$l > /dev/null; done
for i in $(seq 1 "$reps"); do
  for j in 1 8 16 32; do for l in 0 1; do run rel $l para4000 $j p-$l-j$j-$i; done; done
  for j in 1 8; do for l in 0 1; do run rel $l amsldoc $j a-$l-j$j-$i; done; done
done | sort -s -k2,2 -k3,3V -k1,1 | tee -a "$r/summary.txt"
for l in 0 1; do
  for d in para4000 amsldoc; do
    run prof $l $d 8 prof-$l-$d CLEAN_PROF=1 PHITEX_TEX_TIME=1 | tee -a "$r/summary.txt"
    grep -E "^clean (counts|prof)|^acct: (critical path|executor)|^round: |mispredict" "$r/prof-$l-$d.log" | cut -c1-700 | tee -a "$r/summary.txt"
  done
done
echo done
