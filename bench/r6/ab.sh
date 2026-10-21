#!/bin/bash
# Walls of commits side by side on one node (on accl, in a job):
#   scripts/accl/accl submit cmd bash bench/r6/ab.sh REPS NAME:REV[:ENV=V,ENV2=V]...
# Each REV's release CLI built once (a NAME may reuse a REV with env set); para4000 (plain
# pdfTeX) at -j 1 8 16 32 and amsldoc at -j 1 8, REPS times, the NAMEs interleaved; per line
# the wall, nodes/runs and the PDF md5. Then per NAME one para4000 and one amsldoc -j 8 with
# CLEAN_PROF=1 (its lines) and the engine's round line.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
reps=$1; shift
root=$PWD
declare -A built
for nr in "$@"; do
  IFS=: read -r nm rev env <<< "$nr"
  [ -n "${built[$rev]:-}" ] && continue
  s=$w/src-$rev; rm -rf "$s"; mkdir -p "$s"; git archive "$rev" | tar -x -C "$s"; ln -s "$root/upstream" "$s/upstream"
  (cd "$s" && CARGO_TARGET_DIR=$w/t-$rev cargo build -q --release -p phitex-cli > "$r/build-$nm.txt" 2>&1) || echo "$nm build FAILED" | tee -a "$r/summary.txt"
  mkdir -p "$w/bin/$rev"; cp "$w/t-$rev/release/phitex" "$w/bin/$rev/phitex"; built[$rev]=1
done
echo "node $(hostname), $(nproc) CPUs, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2)" | tee -a "$r/summary.txt"
mkdir -p "$w/par"; cp bench/para4000.tex "$w/par/"
run() { # name rev env doc j tag [extra env]
  local nm=$1 rev=$2 env=$3 d=$4 j=$5 t=$6; shift 6
  local dir=$w/run-$nm-$d; rm -rf "$dir"; mkdir -p "$dir"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$dir/"; P=""; else cp bench/$d.tex "$dir/"; P="--program pdftex"; fi
  ( cd "$dir" && env ${env//,/ } "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S" "$w/bin/$rev/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  printf "%-10s %-9s j%-3s %s | %s | %s\n" "$nm" "$d" "$j" "$(grep -oE 'WALL.*' "$r/$t.log")" "$(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1)" "$(md5sum < "$dir/$d.pdf" 2>/dev/null | cut -c1-8)"
}
for nr in "$@"; do IFS=: read -r nm rev env <<< "$nr"; run "$nm" "$rev" "$env" amsldoc 8 warm-$nm > /dev/null; run "$nm" "$rev" "$env" para4000 8 warm-$nm > /dev/null; done
for i in $(seq 1 "$reps"); do
  for j in 1 8 16 32; do for nr in "$@"; do IFS=: read -r nm rev env <<< "$nr"; run "$nm" "$rev" "$env" para4000 $j p-$nm-j$j-$i; done; done
  for j in 1 8; do for nr in "$@"; do IFS=: read -r nm rev env <<< "$nr"; run "$nm" "$rev" "$env" amsldoc $j a-$nm-j$j-$i; done; done
done | sort -s -k2,2 -k3,3V -k1,1 | tee -a "$r/summary.txt"
for nr in "$@"; do
  IFS=: read -r nm rev env <<< "$nr"
  for d in para4000 amsldoc; do
    echo "$(run "$nm" "$rev" "$env" $d 8 prof-$nm-$d CLEAN_PROF=1)" | tee -a "$r/summary.txt"
    grep -iE "^clean|prof|^round" "$r/prof-$nm-$d.log" | head -30 | cut -c1-400 | tee -a "$r/summary.txt"
  done
done
echo done
