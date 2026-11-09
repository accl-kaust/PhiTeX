#!/bin/bash
# para4000 keystroke, SyncTeX on and off: CONTROL (synctex1) vs head, same node, interleaved;
# head's final .synctex (SyncTeX on) vs a clean build's; amsedit.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control & build head $PWD || exit 1; wait
for i in 1 2; do for mode in sync nosync; do for b in control head; do for j in 8 16; do
  f=(); [ $mode = nosync ] && f=(--no-synctex)
  rm -rf $w/pe $w/pe-cold $w/cold && mkdir -p $w/pe $w/cold && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  EDITS_STDERR=$r/pe-$b-$mode-$j-$i.err PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache-$b timeout 900 python3 -I bench/cut1-edits.py $w/bins/$b/phitex $w/pe para4000.tex $j $w/edits.json $r/pe-$b-$mode-$j-$i.json --exact-pdf "${f[@]}" 2>&1 | grep verdict | sed "s|^|$b $mode -j$j run $i: |"
  if [ $mode = sync ] && [ $b = head ] && [ $i = 1 ]; then
    zcat $w/pe/para4000.synctex.gz > $r/w.synctex; cp $w/pe/para4000.tex $w/cold/ && (cd $w/cold && PHITEX_CACHE_DIR=$w/cache-$b $w/bins/$b/phitex build -j$j --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold/para4000.synctex.gz > $r/c.synctex
    echo "head sync -j$j: .synctex lines differing from a clean build's: $(diff $r/w.synctex $r/c.synctex | grep -c '^[<>]') (2: its path)"
  fi
done; done; done; done
for b in control head; do
  rm -rf $w/amsh $w/amsh-cold && cp -a bench/amsldoc $w/amsh
  PHITEX_CACHE_DIR=$w/cache-$b timeout 1200 python3 -I scripts/accl/gate-edits.py $w/bins/$b/phitex $w/amsh amsldoc.tex 8 20 8 $r/ams-$b.json --exact-pdf > /dev/null 2>&1
  python3 -I -c 'import json,sys;r=json.load(open(sys.argv[1]));x=sorted(r["edit_walls_s"]);print(f"amsedit {sys.argv[2]} -j8: "+r["verdict"]+f" median {1000*x[len(x)//2]:.1f} ms")' $r/ams-$b.json $b
done
