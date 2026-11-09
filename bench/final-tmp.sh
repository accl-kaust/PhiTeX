#!/bin/bash
# synctex1's one check: tests; amsldoc -j1/-j8 clean builds (time, md5, second passes) and
# amsedit vs CONTROL; para4000 watch edits with SyncTeX (keystroke, origins checks, the
# final .synctex vs a clean build's).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
bins=()
build() { ( cd "$2" && cargo build -q --release -p phitex-cli $3 > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail -20 "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control "" & build head $PWD "" || exit 1; wait
build headp $PWD "--features phitex-tex/profile,phitex-tex/observe" &
( cargo test -q --release -p phitex-live --lib 2>&1 | grep -E "^test result|FAILED|panicked" | head -20 | sed "s|^|tests live: |"
  cargo test -q --release -p phitex-tex --lib 2>&1 | grep -E "^test result|FAILED|panicked" | head -20 | sed "s|^|tests tex lib: |" ) > $r/tests.txt 2>&1 &
one() { b=$1; j=$2; shift 2; rm -rf $w/a-$b && cp -a bench/amsldoc $w/a-$b && cd $w/a-$b && /usr/bin/time -f "%e s %M KB" env PHITEX_CACHE_DIR=$w/cache-$b "$@" $w/bins/$b/phitex build -j$j --exact-pdf amsldoc.tex > /dev/null 2> $r/err-$b.txt; tail -1 $r/err-$b.txt; md5sum < amsldoc.pdf; cd - > /dev/null; }
for b in control head; do one $b 1 > /dev/null; done
for i in 1 2 3; do for b in control head; do echo "amsldoc j1 $b run $i: $(one $b 1 | tr '\n' ' ')"; done; done
for b in control head; do echo "amsldoc j8 $b: $(one $b 8 | tr '\n' ' ')"; done
wait
cat $r/tests.txt
one headp 1 PHITEX_TEX_TIME=1 PHITEX_CUT_TRACE=1 > /dev/null; grep -E "^acct: commands" $r/err-headp.txt | sed "s|^|headp: |"; echo "headp cuts taken: $(grep -c ': cut, nest' $r/err-headp.txt)"
for b in control head; do for j in 1 8; do
  rm -rf $w/amsh $w/amsh-cold && cp -a bench/amsldoc $w/amsh
  PHITEX_CACHE_DIR=$w/cache-$b timeout 1200 python3 -I scripts/accl/gate-edits.py $w/bins/$b/phitex $w/amsh amsldoc.tex $j 20 8 $r/ams-$b-j$j.json --exact-pdf > /dev/null 2>&1
  python3 -I -c 'import json,sys;r=json.load(open(sys.argv[1]));x=sorted(r["edit_walls_s"]);print(f"amsedit {sys.argv[2]} -j{sys.argv[3]}: "+r["verdict"]+f" median {1000*x[len(x)//2]:.1f} ms")' $r/ams-$b-j$j.json $b $j
done; done
for b in control head; do for j in 8 16; do
  rm -rf $w/pe $w/pe-cold && mkdir -p $w/pe && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  EDITS_STDERR=$r/pe-$b-j$j.err PHITEX_ORIGINS_CHECK=1 PHITEX_CACHE_DIR=$w/cache-$b timeout 900 python3 -I bench/cut1-edits.py $w/bins/$b/phitex $w/pe para4000.tex $j $w/edits.json $r/pe-$b-j$j.json --exact-pdf 2>&1 | grep verdict | sed "s|^|para $b -j$j: |"
  echo "para $b -j$j: origins checks $(grep -c 'origins check' $r/pe-$b-j$j.err) different $(grep -c 'origins check.*DIFFERENT' $r/pe-$b-j$j.err)"
  zcat $w/pe/para4000.synctex.gz > $r/watch-$b-j$j.synctex
  mkdir -p $w/cold && rm -f $w/cold/* && cp $w/pe/para4000.tex $w/cold/ && (cd $w/cold && PHITEX_CACHE_DIR=$w/cache-$b $w/bins/$b/phitex build -j$j --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold/para4000.synctex.gz > $r/cold-$b-j$j.synctex
  echo "para $b -j$j: synctex watch $(md5sum < $r/watch-$b-j$j.synctex | cut -c1-8) clean $(md5sum < $r/cold-$b-j$j.synctex | cut -c1-8), lines differing $(diff $r/watch-$b-j$j.synctex $r/cold-$b-j$j.synctex | grep -c '^[<>]')"
done; done
