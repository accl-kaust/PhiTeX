#!/bin/bash
# book-tmp.sh JOBS JS: the CLI built (fastdev), the course's editions
# JOBS (course course-solutions) built cold at each -j of JS with
# --exact-pdf, compared with the oracle (course only)
set -u
jobs=${1:-course}; js=${2:-8}
(cargo build --profile fastdev -p phitex-cli 2>&1 | tail -2) || exit 1
bin=$PWD/target/fastdev/phitex
for job in $jobs; do
  for j in $js; do
    rm -rf "$w/c" && cp -a "$w/course" "$w/c" && cd "$w/c"
    s0=$(date +%s.%N)
    env ${BOOK_ENV:-} SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC PHITEX_CACHE_DIR=$w/pcache timeout ${BOOK_TIMEOUT:-3000} "$bin" build "$job.tex" --exact-pdf -j $j ${BOOK_ARGS:-} > "$r/$job-j$j.log" 2>&1
    e=$?
    s1=$(date +%s.%N)
    echo "$job -j $j: exit $e wall $(awk -v a="$s0" -v b="$s1" 'BEGIN{printf "%.1f", b-a}') s md5 $(md5sum < $job.pdf 2>/dev/null | cut -c1-32)"
    grep -E "^!|error|panicked|commit.rs|order.rs|segment.rs|engine.rs|at crates" "$r/$job-j$j.log" | head -${BOOK_ERRLINES:-5}
    tail -3 "$r/$job-j$j.log"
    cp "$job.pdf" "$r/$job-j$j.pdf" 2>/dev/null
    cp "$job.log" "$r/$job-j$j.texlog" 2>/dev/null
    mkdir -p "$w/tmpq"; [ -f "$job.pdf" ] && TMPDIR="$w/tmpq" python3 -I "$w/phitex/bench/pdfcmp-tmp.py" "$job.pdf" "$job-oracle-epoch.pdf"
    cd "$w/phitex"
  done
done
