#!/bin/bash
# (temporary, agent editpdf) amsldoc's word edit and paragraph insert at -j1 through watch, BASE's binary and this
# tree's interleaved, N sessions each: the edit's wall_ms medians.   editpdf-time-tmp.sh [N] [BASE]
set -u; cd "$(dirname "$0")/.."; here=$PWD
N=${1:-30}; BASE=${2:-81a72ba6d}; S=$r/summary.txt
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
rm -rf $w/before && mkdir -p $w/before && git archive HEAD | tar -x -C $w/before
for f in $(git diff --name-only $BASE HEAD -- crates | grep -v '/tests/'); do git show $BASE:$f > $w/before/$f 2>/dev/null || rm -f $w/before/$f; done
for t in before after; do d=$here; [ $t = before ] && d=$w/before
  (cd $d && CARGO_TARGET_DIR=$w/target-$t cargo build -q --release -p phitex-cli > $r/build-$t.txt 2>&1) || { echo "$t build failed"; tail -20 $r/build-$t.txt; exit 1; }
  mkdir -p $w/bin-$t && cp $w/target-$t/release/phitex $w/bin-$t/
  rm -rf $w/w && cp -a bench/amsldoc $w/w && (cd $w/w && PHITEX_CACHE_DIR=$w/cache-$t $w/bin-$t/phitex build -j1 amsldoc.tex > /dev/null 2>&1)
done
for kind in word para; do
  if [ $kind = para ]; then o="the equation."; n=$'the equation.\n\nAn inserted paragraph with a few new words in it.'; else o=vertically; n=bertically; fi
  for i in $(seq $N); do for t in before after; do
    rm -rf $w/e && cp -a bench/amsldoc $w/e
    (cd $w/e && PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache-$t timeout 600 python3 -I $here/bench/watchedit.py $w/bin-$t/phitex . amsldoc.tex amsldoc.tex 374 "$o" "$n" 1 $r/err.txt > $r/w.txt 2>&1)
    echo "$kind $t $(tail -1 $r/w.txt | awk '{print $4}')" >> $r/times.txt
    echo "$kind $t $(grep -E '^watch phases' $r/err.txt | tail -1 | grep -oE 'files written [0-9.]+(µs|ms|ns)')" >> $r/writes.txt
  done; done
  for t in before after; do echo "$kind -j1 $t files written: $(grep "^$kind $t " $r/writes.txt | awk '{v=$5; if (v ~ /ms$/) {sub(/ms$/,"",v); v*=1000} else if (v ~ /µs$/) {sub(/µs$/,"",v)} else {sub(/ns$/,"",v); v/=1000}; print v}' | sort -g | awk '{a[NR]=$1} END{print "median", a[int((NR+1)/2)], "us (n=" NR ", min " a[1] ")"}')" | tee -a $S; done
  for t in before after; do echo "$kind -j1 $t: median $(grep "^$kind $t " $r/times.txt | awk '{print $3}' | sort -g | awk '{a[NR]=$1} END{print a[int((NR+1)/2)], "ms (n=" NR ", min " a[1] ", max " a[NR] ")"}')" | tee -a $S; done
done
