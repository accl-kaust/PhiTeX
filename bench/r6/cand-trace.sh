#!/bin/bash
# Candidates by why a piece ended there or not (on accl, in a job): observe build; amsldoc and
# para4000 at -j 1 with PHITEX_CAND_TRACE; bench/r6/cand-classify.py on the traces.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
B=$PWD/target/release/phitex
for d in amsldoc para4000; do
  rm -rf "$w/d-$d"; mkdir -p "$w/d-$d"
  if [ "$d" = amsldoc ]; then cp -a bench/amsldoc/. "$w/d-$d/"; P=""; else cp bench/$d.tex "$w/d-$d/"; P="--program pdftex"; fi
  ( cd "$w/d-$d" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache PHITEX_CAND_TRACE=1 timeout 1200 "$B" build --exact-pdf -j 1 $P $d.tex 2>&1 | gzip -1 > "$r/$d-cand.log.gz" )
  echo "$d md5 $(md5sum < "$w/d-$d/$d.pdf" | cut -c1-8)" | tee -a "$r/summary.txt"
  python3 -I bench/r6/cand-classify.py "$r/$d-cand.log.gz" | tee -a "$r/summary.txt"
done
echo done
