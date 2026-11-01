#!/bin/bash
# r4-latexmk (accl cmd task): DOC (default amsldoc) built cold, then
# rebuilt with its files and the round cap at 1 (observe build): the
# warning names the loop-backs still moving after round one.
set -u
export CARGO_BUILD_JOBS=16
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
grep -q '^EXIT 0$' "$r/build.txt" || { echo "build failed: $(grep -m3 '^error' -A5 "$r/build.txt")" | tee "$r/summary.txt"; exit 1; }
P=$w/t/release/phitex
doc=${DOC:-amsldoc}
D=$w/run/$doc; mkdir -p $D; cp bench/amsldoc/amsldoc.tex bench/r4/docs/*.tex $D/ 2>/dev/null
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$w/cache
(cd $D && timeout 900 $P build $doc.tex --exact-pdf -j 1 > $r/cold.log 2>&1)
(cd $D && PHITEX_MAX_ROUNDS=1 ${PHITEX_ENV:-} timeout 900 $P build $doc.tex --exact-pdf -j 1 > $r/capped.log 2>&1)
grep -a "still chang" $r/capped.log | tr ',' '\n' | head -60 | tee $r/summary.txt
