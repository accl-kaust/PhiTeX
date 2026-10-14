#!/bin/bash
# Short gate: core (this tree, b) vs master (dev $A, a): amsldoc j1/j8 cold + edit with causes and the
# declaration trace; para4000 j1/j8/j12 x2 alternating; para4000 j12 on b's profiling build (PHITEX_OBS).
set -u; here=$PWD; R=$here/../results/short; mkdir -p $R; A=${A:-72e83197}
git log --oneline -1
git worktree add -q --detach $here/target/awt $A 2>&1 | tail -1; [ -e target/awt/upstream ] || ln -s $here/upstream target/awt/upstream
(cd target/awt && cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head)
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head
L=$here/target/core; mkdir -p $L/a $L/b $L/p; cp target/awt/target/release/phitex $L/a/; cp target/release/phitex $L/b/
cargo build -q --profile profiling -p phitex-cli --features phitex-tex/profile 2>&1 | grep -E "^error" -A6 | head
cp target/profiling/phitex $L/p/ 2>/dev/null || echo "no profiling build"
echo "node $(uname -n) cpus $(nproc) $(lscpu | grep 'Model name' | cut -c1-80)"
cp bench/para4000.tex $L/
ams() { # k j
  rm -rf $L/ams-$1; cp -a bench/amsldoc $L/ams-$1
  (cd $L/ams-$1 && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_DECL_TRACE=1 PHITEX_CACHE_DIR=$L/cache-$1 timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" python3 -I $here/bench/watchedit.py $L/$1/phitex . amsldoc.tex amsldoc.tex 374 vertically bertically $2 $R/ams-$1-j$2.err > $R/ams-$1-j$2.log 2>&1)
  local e=$R/ams-$1-j$2.err
  echo "$1 amsldoc j$2: $(grep -E '^cold_ms' $R/ams-$1-j$2.log) $(grep -oE 'WALL.*' $R/ams-$1-j$2.log)"
  grep -E "^evaluation|causes named" $e | cut -c1-900
  awk '/^incr walk/{w++} /^round rerun causes|^round: /{print (w<2?"  cold ":"  edit ") $0}' $e | cut -c1-420
  echo "  declarations and environment maps (counted):"
  awk '/^incr walk/{w++} /^decl-trace/{print (w<2?"cold ":"edit ") $0}' $e | sort | uniq -c | cut -c1-500
}
run() { # k j [prog-dir]
  local d=${3:-$1}
  (cd $L && rm -rf out-$d && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$L/cache-$1 PHITEX_OBS=1 timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" ./$d/phitex build -j $2 --exact-pdf --program pdftex --output-dir=out-$d para4000.tex > $R/w2-$d-j$2.log 2>&1
   echo "$(grep -oE 'WALL.*' $R/w2-$d-j$2.log) | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs, [0-9]+ cutoffs, [0-9]+ loop rounds' $R/w2-$d-j$2.log | head -1) | pdf $(md5sum < out-$d/para4000.pdf 2>/dev/null | cut -c1-10)")
}
echo "=== amsldoc"
ams b 1; ams b 8; ams a 8
echo "=== para4000"
for k in a b; do run $k 8 > /dev/null; done
for j in 1 8 12; do for i in 1 2; do for k in a b; do echo "$k j$j r$i para4000: $(run $k $j)"; done; done; done
echo "=== para4000 j12, b's profiling build, PHITEX_OBS probes"
[ -x $L/p/phitex ] && { echo "p j12: $(run b 12 p)"; sed -n '/^probes/,$p' $R/w2-p-j12.log | head -120; }
echo done
