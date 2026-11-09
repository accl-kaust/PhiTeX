#!/bin/bash
# synctex1: para4000 word edits under watch, SyncTeX on (the default) and
# off, with the origins' and the watch's phases (PHITEX_ORIGINS_TIME,
# PHITEX_TEX_TIME); amsldoc's edit history too. CONTROL=<commit>: its CLI as well.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
bins=()
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; bins+=("$1"); }
if [ -n "${CONTROL:-}" ]; then rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream; build control $w/control; fi
build head $PWD || exit 1
mkdir -p $w/warm && printf '%s\n' '\hsize=3in x' '\bye' > $w/warm/w.tex
for l in "${bins[@]}"; do (cd $w/warm && PHITEX_CACHE_DIR=$w/cache-$l $w/bins/$l/phitex build w.tex > /dev/null 2>&1); done
for l in "${bins[@]}"; do
  for j in 8 16; do
    for mode in sync nosync; do
      f=(); [ $mode = nosync ] && f=(--no-synctex)
      rm -rf $w/pe $w/pe-cold && mkdir -p $w/pe && cp bench/para4000.tex $w/pe/
      python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/pe/edits.json
      EDITS_STDERR=$r/pe-$l-$mode-j$j.err PHITEX_TEX_TIME=1 PHITEX_ORIGINS_TIME=1 PHITEX_CACHE_DIR=$w/cache-$l timeout 900 python3 -I bench/cut1-edits.py $w/bins/$l/phitex $w/pe para4000.tex $j $w/pe/edits.json $r/pe-$l-$mode-j$j.json --exact-pdf "${f[@]}" 2>&1 | grep -E "verdict|edit" | sed "s|^|para $l $mode -j$j: |" | tail -3
      grep -E "^(origins|synctex|watch phases)" $r/pe-$l-$mode-j$j.err | tail -4 | sed "s|^|para $l $mode -j$j: |"
      [ -f $w/pe/para4000.synctex.gz ] && md5sum < $w/pe/para4000.synctex.gz | sed "s|^|para $l $mode -j$j synctex md5 |"
    done
  done
  if [ "${AMS:-1}" = 1 ]; then for j in 8 16; do
    rm -rf $w/amsh $w/amsh-cold && cp -a bench/amsldoc $w/amsh
    PHITEX_CACHE_DIR=$w/cache-$l timeout 1200 python3 -I scripts/accl/gate-edits.py $w/bins/$l/phitex $w/amsh amsldoc.tex $j 20 $j $r/ams-$l-j$j.json --exact-pdf > /dev/null 2>&1
    [ -f $w/amsh/amsldoc.synctex.gz ] && md5sum < $w/amsh/amsldoc.synctex.gz | sed "s|^|amsedit $l -j$j synctex md5 |"
    python3 -I -c 'import json,sys;r=json.load(open(sys.argv[1]));x=sorted(r["edit_walls_s"]);print(f"amsedit {sys.argv[2]} -j{sys.argv[3]}: "+r["verdict"]+f" median {1000*x[len(x)//2]:.1f} ms")' $r/ams-$l-j$j.json $l $j
  done; fi
done
