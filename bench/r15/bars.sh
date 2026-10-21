#!/bin/bash
# The bars of a commit on one node (on accl, in a job): release build; amsldoc -j 1 and 4x
# -j 8 md5, para4000 -j 1 and -j 8 md5, with their walls.
#   scripts/accl/accl submit cmd bash bench/r15/bars.sh
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1 | tee -a "$r/summary.txt"
echo "node $(cat /proc/sys/kernel/hostname); governor: $(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort | uniq -c | tr '\n' ' ')" | tee -a "$r/summary.txt"
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
bin=$PWD/target/release/phitex
run() { # doc j tag
  local d=$1 j=$2 t=$3
  local dir=$w/run-$t; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 900 /usr/bin/time -f "WALL %e" "$bin" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | $(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -d' ' -f1)" | tee -a "$r/summary.txt"
}
run amsldoc 8 warm > /dev/null
run amsldoc 1 ams-j1
for i in 1 2 3 4; do run amsldoc 8 ams-j8-$i; done
run para4000 1 para-j1
for i in 1 2; do run para4000 8 para-j8-$i; done
echo done
