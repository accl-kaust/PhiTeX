#!/bin/bash
# amsldoc -j1 clean build: control vs head, head with no cuts (PHITEX_CUT_MIN=max);
# profile builds: the acct lines, the cuts taken.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
bins=()
build() { ( cd "$2" && cargo build -q --release -p phitex-cli $3 > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; bins+=("$1"); }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control ""; build head $PWD "" || exit 1
P="--features phitex-tex/profile,phitex-tex/observe"
build controlp $w/control "$P"; build headp $PWD "$P"
one() { b=$1; shift; rm -rf $w/a-$b && cp -a bench/amsldoc $w/a-$b && cd $w/a-$b && /usr/bin/time -f "%e s %M KB" env PHITEX_CACHE_DIR=$w/cache-$b "$@" $w/bins/$b/phitex build -j1 --exact-pdf amsldoc.tex > /dev/null 2> $r/err-$b.txt; tail -1 $r/err-$b.txt; md5sum < amsldoc.pdf; cd - > /dev/null; }
for b in control head controlp headp; do one $b > /dev/null; done
for i in 1 2 3; do
  echo "control run $i: $(one control | tr '\n' ' ')"
  echo "head run $i: $(one head | tr '\n' ' ')"
  echo "head nocut run $i: $(one head PHITEX_CUT_MIN=18446744073709551615 | tr '\n' ' ')"
done
for b in controlp headp; do one $b PHITEX_TEX_TIME=1 PHITEX_CUT_TRACE=1 > /dev/null; cp $r/err-$b.txt $r/time-$b.txt; grep -E "^acct: commands" $r/time-$b.txt | sed "s|^|$b: |"; echo "$b cuts taken: $(grep -c ': cut, nest' $r/time-$b.txt) refused: $(grep -c '^cut: node.*: none' $r/time-$b.txt)"; grep -E "^(round|eval|phase|evaluation)" $r/time-$b.txt | head -6 | sed "s|^|$b: |"; done
one headp PHITEX_TEX_TIME=1 PHITEX_CUT_MIN=18446744073709551615 > /dev/null; grep -E "^acct: commands" $r/err-headp.txt | sed "s|^|headp nocut: |"
