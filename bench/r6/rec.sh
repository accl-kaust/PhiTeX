#!/bin/bash
# Recordings for the engine's player and the realised critical path (on accl, in a job):
#   scripts/accl/accl submit cmd bash bench/r6/rec.sh
# observe build: para4000 and amsldoc at -j 8 recorded (DEFUSE_RECORD), gzipped into $r;
# profile build: the same two at -j 8 with PHITEX_TEX_TIME (the acct lines: critical chain by op).
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1
mkdir -p "$w/bin"
for f in observe profile; do
  cargo build -q --release -p phitex-cli --features phitex-tex/$f > "$r/build-$f.txt" 2>&1 || { grep -E "^error|-->" -A5 "$r/build-$f.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
  mkdir -p "$w/bin/$f"; cp target/release/phitex "$w/bin/$f/phitex"
done
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache"
doc() { # tag bin doc j [env...]
  local t=$1 b=$2 d=$3 j=$4; shift 4
  rm -rf "$w/d-$t"; mkdir -p "$w/d-$t"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$w/d-$t/"; P=""; else cp bench/$d.tex "$w/d-$t/"; P="--program pdftex"; fi
  ( cd "$w/d-$t" && env $E "$@" timeout 1200 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$w/bin/$b/phitex" build --exact-pdf -j $j $P $d.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | pdf $(md5sum < "$w/d-$t/$d.pdf" 2>/dev/null | cut -c1-8)" | tee -a "$r/summary.txt"
}
doc warm profile amsldoc 8
doc warm2 profile para400 8
for d in para4000 amsldoc; do
  doc $d-j8-rec observe $d 8 DEFUSE_RECORD=$r/$d-j8.rec
  gzip -1 "$r/$d-j8.rec"; ls -la "$r/$d-j8.rec.gz" | tee -a "$r/summary.txt"
  for j in 8 1; do
    doc $d-j$j-prof profile $d $j PHITEX_TEX_TIME=1
    grep -E "^acct: (critical|executor|op time by kind)" "$r/$d-j$j-prof.log" | cut -c1-900 | tee -a "$r/summary.txt"
  done
done
echo done
