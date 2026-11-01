#!/bin/bash
# S2 of docs/spec-predicted-breaks.md on accl (job.sbatch cmd): the scan
# check (`check`: PHITEX_BREAKS_CHECK on amsldoc, para4000 and the LaTeX
# suite) or the numbers (`perf`: para4000 j1/j8/j16, amsldoc cold j1/j8/j16
# exact and a one-word edit).
set -u; here=$PWD; R=${r:-$here/bench/results}; mkdir -p "$R"
mode=${1:-check}
git log --oneline -1
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head -20
B=$here/target/release/phitex; C=$here/target/s2cache
echo "node $(uname -n) cpus $(nproc)"
mkdir -p "$here/target/par"; cp bench/para4000.tex "$here/target/par/"
ams() { # j label env...
  local j=$1 l=$2; shift 2
  rm -rf "$here/target/ams-$l" && cp -a bench/amsldoc "$here/target/ams-$l"
  (cd "$here/target/ams-$l" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$C PHITEX_TEX_TIME=1 "$@" timeout 1200 \
    /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$B" build --exact-pdf -j "$j" amsldoc.tex > "$R/ams-$l.log" 2>&1)
  echo "amsldoc $l: $(grep -oE 'WALL.*' "$R/ams-$l.log") pdf $(md5sum < "$here/target/ams-$l/amsldoc.pdf" | cut -c1-32) | $(grep 'breaks-check: ' "$R/ams-$l.log" | tail -1)"
  grep -E '^round: |^round rerun causes' "$R/ams-$l.log" | cut -c1-700
}
para() { # j label env...
  local j=$1 l=$2; shift 2
  (cd "$here/target/par" && rm -rf "out-$l" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$C PHITEX_TEX_TIME=1 "$@" timeout 1200 \
    /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$B" build -j "$j" --output-dir="out-$l" --program pdftex para4000.tex > "$R/para-$l.log" 2>&1)
  echo "para4000 $l: $(grep -oE 'WALL.*' "$R/para-$l.log") pdf $(md5sum < "$here/target/par/out-$l/para4000.pdf" | cut -c1-32) | $(grep 'breaks-check: ' "$R/para-$l.log" | tail -1)"
  grep -E '^round: |^round rerun causes' "$R/para-$l.log" | cut -c1-700
}
ams 8 warm > /dev/null
if [ "$mode" = check ]; then
  ams 8 check-j8 PHITEX_BREAKS_CHECK=1
  grep -E 'breaks-check DIFF' "$R/ams-check-j8.log" | head -5
  para 8 check-j8 PHITEX_BREAKS_CHECK=1
  grep -E 'breaks-check DIFF' "$R/para-check-j8.log" | head -5
  PHITEX_CACHE_DIR=$C PHITEX_BREAKS_CHECK=1 cargo test -q -p phitex-tex --test latex -- --test-threads=4 --nocapture > "$R/latex.txt" 2>&1
  grep -E "^test result|^    [a-z_0-9]+$" "$R/latex.txt"
  echo "latex: $(grep 'breaks-check: ' "$R/latex.txt" | tail -1); DIFF lines $(grep -c 'breaks-check DIFF' "$R/latex.txt")"
  grep -E 'breaks-check DIFF' "$R/latex.txt" | sort | uniq -c | sort -rn | head -10
else
  para 8 warm > /dev/null
  for j in 1 8 16; do for i in 1 2; do para "$j" "j$j-r$i"; done; done
  for j in 1 8 16; do ams "$j" "j$j"; done
  for j in 1 8 16; do
    rm -rf "$here/target/ams-e$j" && cp -a bench/amsldoc "$here/target/ams-e$j"
    (cd "$here/target/ams-e$j" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$C timeout 1200 \
      python3 -I "$here/bench/watchedit.py" "$B" . amsldoc.tex amsldoc.tex 374 vertically bertically "$j" "$R/ams-e$j.err" > "$R/ams-e$j.log" 2>&1)
    echo "amsldoc edit j$j: $(cat "$R/ams-e$j.log" | tail -1)"
    awk '/^incr walk/{w++} /^round: |^round rerun causes/{print (w<2?"  cold ":"  edit ") $0}' "$R/ams-e$j.err" | cut -c1-500
  done
  grep -h 'phitex: scan step' "$R"/*.log "$R"/*.err 2>/dev/null | head -5
  PHITEX_CACHE_DIR=$C cargo test -q -p phitex-tex --test latex -- --test-threads=4 > "$R/latex.txt" 2>&1
  grep -E "^test result|^    [a-z_0-9]+$" "$R/latex.txt"
  grep -E "differs|byte [0-9]+|phitex: scan step" "$R/latex.txt" | head -10
fi
echo done
