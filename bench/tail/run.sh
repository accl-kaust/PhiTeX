#!/bin/bash
# edit-tail measurement (accl cmd): amsldoc exact cold j1/j8 cmp oracle, one-word edits mid/end at j1/j8
set -u; here=$PWD; R=$here/bench/results; mkdir -p $R; git log --oneline -1 | cut -c1-80
cargo build -q --release -p phitex-cli 2>&1 | grep -E "^error" -A6 | head
B=$here/target/release/phitex; export PHITEX_CACHE_DIR=$here/target/cache SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
echo "node $(uname -n) cpus $(nproc)"
mkdir -p target/warm && cp bench/amsldoc/amsldoc.tex target/warm/ && (cd target/warm && $B build --exact-pdf -j 8 amsldoc.tex >/dev/null 2>&1)
for j in 1 8; do d=target/cold$j; rm -rf $d; mkdir -p $d; cp bench/amsldoc/amsldoc.tex $d/
  (cd $d && /usr/bin/time -f "WALL %e" $B build --exact-pdf -j $j amsldoc.tex > log 2>&1); echo "cold j$j: $(grep -o 'WALL.*' $d/log) $(cmp $d/amsldoc.pdf bench/amsldoc/oracle-epoch.pdf >/dev/null && echo IDENTICAL || echo DIFFERS)"; done
for j in 1 8; do for w in "mid 1654 operator operators" "end 2767 discussion discussions"; do set -- $w; d=target/e-$1-$j; rm -rf $d; mkdir -p $d; cp bench/amsldoc/amsldoc.tex $d/
  PHITEX_TEX_TIME=1 PHITEX_RERUN_OPS=1 python3 -I bench/tail/watchedit.py $B $d amsldoc.tex amsldoc.tex $2 "$3" "$4" $j $d/err > $d/out 2>&1
  # the edited file built cold, for the bytes
  mkdir -p $d/c; cp $d/amsldoc.tex $d/c/; (cd $d/c && $B build --exact-pdf -j $j amsldoc.tex >/dev/null 2>&1)
  echo "== edit $1 j$j: $(cat $d/out) | vs cold of the edited file: $(cmp $d/edited.pdf $d/c/amsldoc.pdf >/dev/null && echo IDENTICAL || echo DIFFERS)"
  python3 -I bench/tail/summ.py $d/err | grep -v "^round rerun"; grep -E "^pdf end" $d/err | tail -1; cp $d/err $R/err-$1-$j; done; done
