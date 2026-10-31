#!/bin/bash
# W2 (para4000) on this tree's engine: j1/j8/j12 x2 (default build), then j12 on the observe build with
# worker dumps every 0.5 s, then j12 recorded (DEFUSE_RECORD) for the engine's player
set -u; here=$PWD; R=$here/../results; mkdir -p $R; L=$here/target/w2; mkdir -p $L/d $L/o
git log --oneline -1; echo "node $(uname -n) cpus $(nproc)"
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head; cp target/release/phitex $L/d/
cargo build -q --release -p phitex-cli --features phitex-tex/observe 2>&1 | grep -E "^error" -A6 | head; cp target/release/phitex $L/o/
cp bench/para4000.tex $L/
run() { # dir j tag [env...]
  local d=$1 j=$2 t=$3; shift 3
  (cd $L && rm -rf out-$t && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$L/cache "$@" timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" ./$d/phitex build -j $j --exact-pdf --program pdftex --output-dir=out-$t para4000.tex > $R/w2-$t.log 2>&1
   echo "$(grep -oE 'WALL.*' $R/w2-$t.log) | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs, [0-9]+ cutoffs, [0-9]+ loop rounds' $R/w2-$t.log | head -1) | pdf $(md5sum < out-$t/para4000.pdf 2>/dev/null | cut -c1-10)")
}
run d 8 warm > /dev/null
for j in 1 8 12; do for i in 1 2; do echo "default j$j r$i para4000: $(run d $j d-j$j-r$i)"; done; done
echo "observe j12 (no env): $(run o 12 o-j12)"
echo "observe j12 dump every 0.5 s: $(run o 12 o-dump DEFUSE_DUMP=$R/w2-dump-j12.txt DEFUSE_DUMP_EVERY=0.5)"
echo "observe j12 recorded: $(run o 12 o-rec DEFUSE_RECORD=$R/w2-rec-j12.jsonl)"
ls -la $R/w2-dump-j12.txt $R/w2-rec-j12.jsonl; gzip -kf $R/w2-rec-j12.jsonl
echo done
