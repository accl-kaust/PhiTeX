#!/bin/bash
# Before/after of a change on one node (on accl, in a job):
#   scripts/accl/accl submit cmd bash bench/r15/ab.sh BEFORE_REV [REPS]
# The tree (after) and BEFORE_REV each built release and profile+observe. After's bars:
# amsldoc -j 1 and 4x -j 8 md5, para4000 -j 1 and -j 8 md5 (release). Then REPS (3) amsldoc
# -j 8 reps of each side interleaved: release walls, and profile+observe with PHITEX_TEX_TIME
# and PHITEX_CHAIN_TSV (the realised path; bench/r15/chain.py's kinds per rep).
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
before=$1; reps=${2:-3}
root=$PWD
git log --oneline -1 | tee -a "$r/summary.txt"
echo "before: $(git log --oneline -1 "$before")" | tee -a "$r/summary.txt"
echo "node $(cat /proc/sys/kernel/hostname), $(nproc) CPUs, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2)" | tee -a "$r/summary.txt"
echo "governor: $(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort | uniq -c | tr '\n' ' ') boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)" | tee -a "$r/summary.txt"
build() { # side dir
  local nm=$1 d=$2
  mkdir -p "$w/bin/$nm-rel" "$w/bin/$nm-po"
  (cd "$d" && CARGO_TARGET_DIR=$w/t-$nm cargo build -q --release -p phitex-cli > "$r/build-$nm.txt" 2>&1) || { echo "$nm build FAILED" | tee -a "$r/summary.txt"; grep -E "^error|-->" -A7 "$r/build-$nm.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
  cp "$w/t-$nm/release/phitex" "$w/bin/$nm-rel/"
  (cd "$d" && CARGO_TARGET_DIR=$w/t-$nm cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build-$nm-po.txt" 2>&1) || { echo "$nm po build FAILED" | tee -a "$r/summary.txt"; exit 1; }
  cp "$w/t-$nm/release/phitex" "$w/bin/$nm-po/"
}
s=$w/src-before; rm -rf "$s"; mkdir -p "$s"; git archive "$before" | tar -x -C "$s"; [ -e "$root/upstream" ] && ln -s "$root/upstream" "$s/upstream"
build before "$s"
build after "$root"
run() { # bin doc j tag [env...]
  local b=$1 d=$2 j=$3 t=$4; shift 4
  local dir=$w/run-$t; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && env "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 1200 /usr/bin/time -f "WALL %e BUSY %U+%S" "$w/bin/$b/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | $(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -d' ' -f1)" | tee -a "$r/summary.txt"
}
run after-rel amsldoc 8 warm > /dev/null
run before-rel amsldoc 8 warm-b > /dev/null
run after-rel amsldoc 1 bar-ams-j1
for i in 1 2 3 4; do run after-rel amsldoc 8 bar-ams-j8-$i; done
run after-rel para4000 1 bar-para-j1
run after-rel para4000 8 bar-para-j8
for i in $(seq 1 "$reps"); do
  for nm in before after; do
    run $nm-rel amsldoc 8 $nm-rel-j8-$i
    run $nm-po amsldoc 8 $nm-po-j8-$i PHITEX_TEX_TIME=1 PHITEX_CHAIN_TSV=$r/chain-$nm-$i.tsv
    grep -E "^acct: critical path" "$r/$nm-po-j8-$i.log" | cut -c1-300 | tee -a "$r/summary.txt"
  done
done
for nm in before after; do for i in $(seq 1 "$reps"); do
  python3 -I bench/r15/chain.py "$r/chain-$nm-$i.tsv" > "$r/kinds-$nm-$i.txt" 2>&1
  echo "== $nm $i" | tee -a "$r/summary.txt"; sed -n 1,20p "$r/kinds-$nm-$i.txt" | tee -a "$r/summary.txt"
done; done
echo done
