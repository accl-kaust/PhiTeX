#!/bin/bash
# para4000 keystroke, SyncTeX on and off: CONTROL (synctex1) vs head, same node, interleaved.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control & build head $PWD || exit 1; wait
for i in 1; do for mode in sync; do for b in head; do for j in 8; do
  f=(); [ $mode = nosync ] && f=(--no-synctex)
  rm -rf $w/pe $w/pe-cold && mkdir -p $w/pe && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  EDITS_STDERR=$r/pe-$b-$mode-$j-$i.err PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache-$b timeout 900 python3 -I bench/cut1-edits.py $w/bins/$b/phitex $w/pe para4000.tex $j $w/edits.json $r/pe-$b-$mode-$j-$i.json --exact-pdf "${f[@]}" 2>&1 | grep verdict | sed "s|^|$b $mode -j$j run $i: |"
done; done; done; done
for b in control head; do echo "== $b sync j8 watch phases (last edit)"; grep -E "^watch|phases" $r/pe-$b-sync-8-1.err | tail -3 | cut -c1-400; done
