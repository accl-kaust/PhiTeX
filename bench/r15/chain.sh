#!/bin/bash
# amsldoc's realised critical path at -j 8, link by link (on accl, in a job):
#   scripts/accl/accl submit cmd bash bench/r15/chain.sh [REPS]
# Builds: release (walls, md5), observe (the recording), profile+observe (PHITEX_TEX_TIME acct lines and the chain's
# links, PHITEX_CHAIN_TSV), the engine's player (observe). REPS (3) runs of each at -j 8 on
# this node, the governor recorded; one -j 8 recording (DEFUSE_RECORD) replayed by the player
# at 8 threads with PLAY_DAG and DEFUSE_RCHAIN (the play model's critical path and chain);
# bench/r15/chain.py classifies each rep's links.
set -u
cd "$(dirname "$0")/../.."
export CARGO_BUILD_JOBS=16
reps=${1:-3}
git log --oneline -1 | tee -a "$r/summary.txt"
echo "node $(cat /proc/sys/kernel/hostname), $(nproc) CPUs, $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2)" | tee -a "$r/summary.txt"
echo "governor: $(cat /sys/devices/system/cpu/cpu*/cpufreq/scaling_governor 2>/dev/null | sort | uniq -c | tr '\n' ' ') boost: $(cat /sys/devices/system/cpu/cpufreq/boost 2>/dev/null)" | tee -a "$r/summary.txt"
mkdir -p "$w/bin/rel" "$w/bin/po"
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build.txt" | head -60 | tee -a "$r/summary.txt"; exit 1; }
cp target/release/phitex "$w/bin/rel/"
cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > "$r/build-po.txt" 2>&1 || { grep -E "^error|-->" -A7 "$r/build-po.txt" | head -60 | tee -a "$r/summary.txt"; exit 1; }
cp target/release/phitex "$w/bin/po/"
cargo build -q --release -p phitex-cli --features phitex-tex/observe > "$r/build-o.txt" 2>&1 && mkdir -p "$w/bin/o" && cp target/release/phitex "$w/bin/o/"
cargo build -q --release -p defuse --features observe --example play > "$r/build-play.txt" 2>&1 || grep -E "^error|-->" -A7 "$r/build-play.txt" | head -30 | tee -a "$r/summary.txt"
cargo clippy -q -p phitex-tex --no-deps --features profile,observe -- -D warnings > "$r/clippy.txt" 2>&1
echo "clippy (profile,observe) exit $?" | tee -a "$r/summary.txt"; grep -E -- "^(error|warning)|-->" "$r/clippy.txt" | head -20 | tee -a "$r/summary.txt"
run() { # bin j tag [env...]
  local b=$1 j=$2 t=$3; shift 3
  local dir=$w/run-$t; rm -rf "$dir"; mkdir -p "$dir"; cp -a bench/amsldoc/. "$dir/"
  ( cd "$dir" && env "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$w/cache timeout 1200 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" "$w/bin/$b/phitex" build --exact-pdf -j $j amsldoc.tex > "$r/$t.log" 2>&1 )
  echo "$t: $(grep -oE 'WALL.*' "$r/$t.log") | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' "$r/$t.log" | tail -1) | $(grep -oE 'Output written[^)]*\)' "$r/$t.log" | tail -1) | $(md5sum < "$dir/amsldoc.pdf" 2>/dev/null | cut -c1-8)" | tee -a "$r/summary.txt"
}
run rel 8 warm > /dev/null
run po 8 warm-po > /dev/null
for i in $(seq 1 "$reps"); do
  run rel 8 rel-j8-$i
  run po 8 po-j8-$i PHITEX_TEX_TIME=1 PHITEX_CHAIN_TSV=$r/chain-$i.tsv
  grep -E "^acct: critical path" "$r/po-j8-$i.log" | cut -c1-400 | tee -a "$r/summary.txt"
done
run rel 1 rel-j1
run o 8 rec DEFUSE_RECORD=$w/ams.rec
ls -la "$w/ams.rec" | tee -a "$r/summary.txt"
if [ -x target/release/examples/play ]; then
  PLAY_DAG=1 DEFUSE_RCHAIN=1 timeout 2400 target/release/examples/play "$w/ams.rec" --threads 8 > "$r/play.txt" 2>&1
  echo "play exit $?" | tee -a "$r/summary.txt"
  grep -E "^evaluation|realised critical|dag:|bounds:|divergent" "$r/play.txt" | cut -c1-300 | tee -a "$r/summary.txt"
fi
gzip -1 "$w/ams.rec" && cp "$w/ams.rec.gz" "$r/"
for i in $(seq 1 "$reps"); do python3 -I bench/r15/chain.py "$r/chain-$i.tsv" > "$r/kinds-$i.txt" 2>&1; head -40 "$r/kinds-$i.txt" | tee -a "$r/summary.txt"; done
echo done
