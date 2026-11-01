#!/bin/bash
# r3-cuts (accl cmd task): amsldoc cold at -j 1 and -j 8 with the profile
# CLI of commit $A and of this checkout, PHITEX_TEX_TIME's round lines.
set -u
B=$(git rev-parse --short=8 HEAD)
export CARGO_BUILD_JOBS=16
CARGO_TARGET_DIR=$w/t-b cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-b.txt" 2>&1 || exit 1
git -c advice.detachedHead=false checkout -q "$A"
CARGO_TARGET_DIR=$w/t-a cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/build-a.txt" 2>&1 || exit 1
git -c advice.detachedHead=false checkout -q "$B"
for x in a b; do
  for j in 1 8; do
    D=$w/run-$x-$j; rm -rf "$D"; cp -a bench/amsldoc "$D"
    ( cd "$D" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache-$x "$w/t-$x/release/phitex" build amsldoc.tex --exact-pdf -j $j > "$r/$x-j$j.err" 2>&1 )
    c=$([ $x = a ] && echo "$A" || echo "$B")
    echo "== $c -j $j md5 $(md5sum < $D/amsldoc.pdf | cut -c1-8)" >> "$r/summary.txt"
    grep -E '^round: ' "$r/$x-j$j.err" | cut -c1-90 >> "$r/summary.txt"
  done
done
cat "$r/summary.txt"
