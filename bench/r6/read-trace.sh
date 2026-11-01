#!/bin/bash
# Where a piece reads a field (on accl, in a job): observe build; amsldoc -j 1 once per field
# with PHITEX_READS=<field> and PHITEX_DEF_TRACE=<field> (the reads with the current token and
# the token lists being read; the definitions), gzipped into $r.
#   scripts/accl/accl submit cmd bash bench/r6/read-trace.sh FIELD...
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
git log --oneline -1
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -40 | tee -a "$r/summary.txt"; exit 1; }
B=$PWD/target/release/phitex
for f in "$@"; do
  t=$(echo "$f" | tr -c 'a-zA-Z0-9\n' '_')
  rm -rf "$w/a" && cp -a bench/amsldoc "$w/a"
  ( cd "$w/a" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache PHITEX_READS="$f" PHITEX_DEF_TRACE="$f" timeout 900 "$B" build --exact-pdf -j 1 amsldoc.tex 2>&1 | grep -E "^(def-trace|read|reads)|$f" | gzip -1 > "$r/$t.log.gz" )
  echo "$f: $(zcat "$r/$t.log.gz" | wc -l) lines" | tee -a "$r/summary.txt"
  zcat "$r/$t.log.gz" | head -12 | cut -c1-400 | tee -a "$r/summary.txt"
done
echo done
