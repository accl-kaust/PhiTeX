#!/bin/bash
# para4000: the watch's final .synctex (head, control) and a clean build's of the final text, compared.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
bins=()
build() { ( cd "$2" && cargo build -q --release -p phitex-cli > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; bins+=("$1"); }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream; build control $w/control
build head $PWD || exit 1
for l in "${bins[@]}"; do
  rm -rf $w/pe-$l $w/pe-$l-cold && mkdir -p $w/pe-$l && cp bench/para4000.tex $w/pe-$l/
  python3 -I bench/cut1-paraedit.py $w/pe-$l/para4000.tex 6 > $w/edits.json
  PHITEX_CACHE_DIR=$w/cache-$l timeout 900 python3 -I bench/cut1-edits.py $w/bins/$l/phitex $w/pe-$l para4000.tex 8 $w/edits.json $r/pe-$l.json --exact-pdf 2>&1 | grep verdict
  zcat $w/pe-$l/para4000.synctex.gz > $r/watch-$l.synctex
  mkdir -p $w/cold-$l && cp $w/pe-$l/para4000.tex $w/cold-$l/ && (cd $w/cold-$l && PHITEX_CACHE_DIR=$w/cache-$l $w/bins/$l/phitex build -j8 --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold-$l/para4000.synctex.gz > $r/cold-$l.synctex
done
md5sum $r/*.synctex
for a in "watch-control cold-control" "watch-head cold-head" "watch-control watch-head" "cold-control cold-head"; do set -- $a; echo "== $1 vs $2: $(diff $r/$1.synctex $r/$2.synctex | grep -c '^[<>]') lines differ"; diff $r/$1.synctex $r/$2.synctex | head -12; done
