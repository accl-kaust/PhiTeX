#!/bin/bash
# core (this tree, core-switch, b) vs master (github dev, a): correctness + speed. Measurement only.
set -u; here=$PWD; R=$here/../results/core; mkdir -p $R; A=${A:-72e83197}
git log --oneline -1; echo "a = dev $A (master d2f98e2), b = core-switch: dev $A + core ${CORE:-cf28044}"
git worktree add -q --detach $here/target/awt $A 2>&1 | tail -1; [ -e target/awt/upstream ] || ln -s $here/upstream target/awt/upstream
declare -A T=([a]=$here/target/awt [b]=$here)
for k in a b; do (cd ${T[$k]} && cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head -20; cargo test -q -p phitex-tex --profile fastdev --no-run 2>&1 | grep -E "^error" -A6 | head -20); done
L=$here/target/core; mkdir -p $L/a $L/b; cp target/awt/target/release/phitex $L/a/; cp target/release/phitex $L/b/; md5sum $L/*/phitex
echo "node $(uname -n) cpus $(nproc) $(lscpu | grep 'Model name' | cut -c1-80)"
echo "=== speed (release CLI, SOURCE_DATE_EPOCH, alternating a/b, 3 reps)"
cp bench/para400.tex bench/para4000.tex $L/; cp oracle/fixtures/latex/pdfdocs/paras30.tex $L/
D=$here/bench/amsldoc
run() { # k doc j prog
  (cd $L && rm -rf out-$1 && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$L/cache-$1 timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" ./$1/phitex build -j $3 --exact-pdf --output-dir=out-$1 $4 $2.tex > $R/run-$1.log 2>&1
   echo "$(grep -oE 'WALL.*' $R/run-$1.log) | $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs, [0-9]+ cutoffs, [0-9]+ loop rounds' $R/run-$1.log | head -1) | pdf $(md5sum < out-$1/$2.pdf 2>/dev/null | cut -c1-10) | gone at settle $(grep -c 'gone at settle' $R/run-$1.log)")
}
ams() { # k j
  rm -rf $L/ams-$1; cp -a $D $L/ams-$1; (cd $L/ams-$1 && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$L/cache-$1 timeout 900 /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" python3 -I $here/bench/watchedit.py $L/$1/phitex . amsldoc.tex amsldoc.tex 374 vertically bertically $2 $R/ams-$1.err > $R/ams-$1.log 2>&1; echo "$(grep -E '^cold_ms' $R/ams-$1.log) $(grep -oE 'WALL.*' $R/ams-$1.log) | edit round: $(grep -E '^round rerun causes' $R/ams-$1.err | tail -1 | cut -c1-300) | $(grep -E '^round: ' $R/ams-$1.err | tail -1 | cut -c1-330) | gone runs $(awk '/^round gone:/{s+=$3} END{print s+0}' $R/ams-$1.err), at settle $(grep -c 'gone at settle' $R/ams-$1.err)")
}
for k in a b; do run $k para400 8 "--program pdftex" >/dev/null; run $k paras30 8 "" > /dev/null; done
for j in 1 4 8 12; do for i in 1 2 3; do for k in a b; do
  echo "$k j$j r$i amsldoc  : $(ams $k $j)"; cp $R/ams-$k.err $R/ams-$k-j$j-r$i.err
  echo "$k j$j r$i para4000 : $(run $k para4000 $j '--program pdftex')"
  echo "$k j$j r$i para400  : $(run $k para400 $j '--program pdftex')"
  echo "$k j$j r$i paras30  : $(run $k paras30 $j '')"
done; done; done
echo "=== correctness: suites (fastdev, fresh formats each)"
for k in a b; do (cd ${T[$k]} && export PHITEX_CACHE_DIR=${T[$k]}/target/pcache && rm -rf $PHITEX_CACHE_DIR && mkdir -p $PHITEX_CACHE_DIR
  for t in $(ls crates/engines/tex/tests/*.rs | xargs -n1 basename | sed 's/\.rs$//' | grep -vE '^(common|profile|trip)$'); do
    timeout 1200 cargo test -q -p phitex-tex --profile fastdev --test $t 2>&1 | grep -E "^test .* \.\.\. |^test result" | sed "s/finished in.*//" | sed "s/^/$t: /"; done > $R/suites-$k.txt
  PHITEX_CHAIN=1 timeout 600 cargo test -q -p phitex-tex --profile fastdev --test latex -- --exact latex_paragraph_nodes --nocapture 2>&1 | grep -E "^chains|^cold|^edit" > $R/paras30-$k.txt
  PHITEX_CHAIN=1 timeout 600 cargo test -q -p phitex-tex --profile fastdev --test chain -- --ignored --exact para400 --nocapture 2>&1 | grep -E "^para4" > $R/para400-$k.txt
  /usr/bin/time -f "edits suite: %e s wall, %U+%S s busy, %M kB" timeout 1200 cargo test -q -p phitex-tex --profile fastdev --test edits 2>&1 | grep -E "edits suite|test result" > $R/edits-$k.txt ); done
diff $R/suites-a.txt $R/suites-b.txt > /dev/null && echo "suites: identical results ($(grep -c ' \.\.\. ' $R/suites-a.txt) tests)" || { echo "suites DIFFER:"; diff $R/suites-a.txt $R/suites-b.txt | head -30; }
for k in a b; do echo "[$k]"; cut -c1-200 $R/paras30-$k.txt; cut -c1-160 $R/para400-$k.txt; cat $R/edits-$k.txt; done
echo "=== amsldoc edit-equals-cold (j8): cold build of the edited text"
for k in a b; do rm -rf $L/amsc-$k; cp -a $D $L/amsc-$k; sed -i '374s/vertically/bertically/' $L/amsc-$k/amsldoc.tex; (cd $L/amsc-$k && SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 PHITEX_CACHE_DIR=$L/cache-$k timeout 900 $L/$k/phitex build -j 8 --exact-pdf amsldoc.tex > /dev/null 2>&1; echo "$k cold-of-edited pdf $(md5sum < amsldoc.pdf 2>/dev/null | cut -c1-10)"); done
echo done
