#!/bin/bash
# The format build and amsldoc's preamble, split by phase (on accl, in a job), beside pdfTeX:
#   scripts/accl/accl submit cmd bash bench/r6/region.sh
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1 | tee -a "$r/summary.txt"
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build.txt" 2>&1 || { tail -20 "$r/build.txt" | tee -a "$r/summary.txt"; exit 1; }
B=$PWD/target/release/phitex
echo "node $(cat /proc/sys/kernel/hostname) $(nproc) CPUs; pdftex: $(command -v pdftex || echo none) $(pdftex --version 2>/dev/null | head -1)" | tee -a "$r/summary.txt"
mkdir -p "$w/one"
printf '%s\n' '\documentclass{article}' '\begin{document}' 'One page.' '\end{document}' > "$w/one/one.tex"
E="SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1"
show() { grep -E "^prelude|^acct: (round|op time|executor|critical)|^clean (counts|prof)|^round: |^time:|nodes run of" "$1" | cut -c1-600; }
# 1. the format build: a fresh cache each time
for j in 8 8 8 1; do
  rm -rf "$w/cache-f"
  ( cd "$w/one" && env $E PHITEX_CACHE_DIR=$w/cache-f PHITEX_TEX_TIME=1 CLEAN_PROF=1 /usr/bin/time -f "WALL %e RSS %M KB" "$B" build -j $j one.tex > "$r/fmt-j$j.log" 2>&1 )
  echo "== format build + one page, -j $j: $(grep -oE 'WALL.*' "$r/fmt-j$j.log")" | tee -a "$r/summary.txt"
  show "$r/fmt-j$j.log" | tee -a "$r/summary.txt"
done
# 2. amsldoc and its preamble alone (format cached)
mkdir -p "$w/pre"; cp -a bench/amsldoc/. "$w/pre/"
awk '/\\begin\{document\}/{print "\\begin{document}"; print "\\end{document}"; exit} {print}' bench/amsldoc/amsldoc.tex > "$w/pre/pre.tex"
( cd "$w/one" && env $E PHITEX_CACHE_DIR=$w/cache "$B" build -j 8 one.tex > /dev/null 2>&1 )
for d in amsldoc pre; do for j in 1 8 1 8 1 8; do
  rm -rf "$w/pre/out"; mkdir -p "$w/pre/out"
  ( cd "$w/pre" && env $E PHITEX_CACHE_DIR=$w/cache PHITEX_TEX_TIME=1 CLEAN_PROF=1 /usr/bin/time -f "WALL %e RSS %M KB" "$B" build --exact-pdf -j $j --output-dir=out $d.tex > "$r/$d-j$j.log" 2>&1 )
  echo "== $d -j $j: $(grep -oE 'WALL.*' "$r/$d-j$j.log")" | tee -a "$r/summary.txt"
done; done
for d in amsldoc pre; do for j in 1 8; do
  echo "-- $d -j $j (last rep)" | tee -a "$r/summary.txt"; show "$r/$d-j$j.log" | tee -a "$r/summary.txt"
  grep -E "^(nodetime|longrun)" "$r/$d-j$j.log" | sort -k4 -rn | head -12 | cut -c1-220 | tee -a "$r/summary.txt"
done; done
# 3. pdfTeX beside it
if command -v pdftex > /dev/null; then
  mkdir -p "$w/ini" && cd "$w/ini"
  for i in 1 2 3; do
    rm -f pdflatex.fmt
    /usr/bin/time -f "WALL %e RSS %M KB" pdftex -ini -etex -jobname=pdflatex -progname=pdflatex -interaction=batchmode pdflatex.ini > ini.log 2> ini.time
    echo "pdftex -ini pdflatex.ini: $(cat ini.time | tail -1) fmt $(ls -la pdflatex.fmt 2>/dev/null | awk '{print $5}')" | tee -a "$r/summary.txt"
  done
  cd "$w/pre"
  for d in pre amsldoc; do for i in 1 2 3; do
    /usr/bin/time -f "WALL %e RSS %M KB" pdflatex -interaction=batchmode -output-directory=out $d.tex > /dev/null 2> pt.time
    echo "pdflatex $d: $(tail -1 pt.time)" | tee -a "$r/summary.txt"
  done; done
fi
echo done
