#!/bin/bash
# Parallelism profile on accl (job.sbatch cmd task): release CLI; para400 and
# para4000 cold at -j 1 4 8 16, three repetitions; amsldoc cold + one edit at
# -j 1 8 16. Each line: wall and busy from /usr/bin/time, the engine's round
# summary (nodes, runs, stops, realised critical path in nodes and time, the
# longest runs), pages shipped. Needs bench/para400.tex, bench/para4000.tex
# and bench/amsldoc/ in the tree.
set -u; here=$PWD; R=${r:-$here/bench/results}; mkdir -p "$R"
git log --oneline -1
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head -20
B=$here/target/release/phitex; md5sum "$B"
echo "node $(uname -n) cpus $(nproc) $(lscpu | grep 'Model name' | cut -c1-80)"
mkdir -p "$here/target/par"; cp bench/para400.tex bench/para4000.tex "$here/target/par/"
run() { # doc j rep
  local d=$1 j=$2 i=$3 L; L=$R/$1-j$2-r$3
  (cd "$here/target/par" && rm -rf "out-$d-$j" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 \
    PHITEX_CACHE_DIR=$here/target/par/cache PHITEX_TEX_TIME=1 timeout 900 \
    /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$B" build -j "$j" --output-dir="out-$d-$j" --program pdftex "$d.tex" > "$L.log" 2>&1)
  echo "$d j$j rep$i: $(grep -oE 'WALL.*' "$L.log") | $(grep -E '^round: |^round rerun causes' "$L.log" | tail -2 | cut -c1-400 | tr '\n' ' ') | pdf $(md5sum < "$here/target/par/out-$d-$j/$d.pdf" 2>/dev/null | cut -c1-10)"
}
run para400 8 0 # (warms the format and font caches)
for d in para400 para4000; do for j in 1 4 8 16; do for i in 1 2 3; do run "$d" "$j" "$i"; done; done; done
if [ -d bench/amsldoc ]; then
  for j in 1 8 16; do
    rm -rf "target/ams-$j" && cp -a bench/amsldoc "target/ams-$j"
    (cd "target/ams-$j" && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 \
      PHITEX_CACHE_DIR=$here/target/par/cache timeout 900 \
      /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" python3 -I "$here/bench/watchedit.py" "$B" . amsldoc.tex amsldoc.tex 374 vertically bertically "$j" "$R/ams-j$j.err" > "$R/ams-j$j.log" 2>&1)
    echo "amsldoc j$j: $(grep -E '^cold_ms' "$R/ams-j$j.log") $(grep -oE 'WALL.*' "$R/ams-j$j.log")"
    awk '/^incr walk/{w++} /^round: |^round rerun causes/{print (w<2?"  cold ":"  edit ") $0}' "$R/ams-j$j.err" | cut -c1-420
  done
else
  echo "no bench/amsldoc in the tree: amsldoc skipped"
fi
echo done
