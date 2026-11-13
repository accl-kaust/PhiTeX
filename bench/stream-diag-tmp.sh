#!/bin/bash
# (temporary) what a streamed page's file holds and costs: this commit's CLI with the engine's
# traces (`observe`), the watch with its viewer at -j1, a clean build and EDITS one-word edits of
# each of DOCS (TeX Live manuals by directory/main, the CI image; `amsldoc` the bench copy), each
# page's file kept (PHITEX_STREAM_DUMP) and its objects summed by kind (qpdf --json).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli --features phitex-tex/observe > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail -30 $r/build.txt; exit 1; }
bin=$PWD/target/release/phitex
for spec in ${DOCS:-amsldoc}; do
  n=$(basename $(dirname $spec)); [ $spec = amsldoc ] && n=amsldoc; [ $spec = grail ] && n=grail
  rm -rf $w/e-$n && mkdir -p $w/e-$n $r/dump-$n
  if [ $spec = grail ]; then n=grail; cp -a $w/bin/grail-src/. $w/e-$n/ && rm -f $w/e-$n/grail.pdf; main=grail.tex
  elif [ $spec = amsldoc ]; then cp bench/amsldoc/amsldoc.tex $w/e-$n/; main=amsldoc.tex; else cp -a /usr/share/doc/texlive/$(dirname $spec)/. $w/e-$n/ && rm -f $w/e-$n/*.pdf; main=$(basename $spec); fi
  (cd $w/e-$n && PHITEX_CACHE_DIR=$w/cache $bin build -j1 $main > /dev/null 2>&1)
  python3 -I bench/cut1-paraedit.py $w/e-$n/$main ${EDITS:-2} > $w/edits-$n.json
  PHITEX_STREAM_TRACE=1 PHITEX_STREAM_DUMP=$r/dump-$n EDITS_STDERR=$r/e-$n.err PHITEX_CACHE_DIR=$w/cache timeout 3000 \
    python3 -I bench/stream-edits-tmp.py $bin $w/e-$n $main 1 $w/edits-$n.json $r/e-$n.json --view --write > $r/e-$n.txt 2>&1
  echo "== $n"; tail -3 $r/e-$n.txt
  grep -c "replayed" $r/e-$n.err | sed 's/^/replayed: /'
  grep "shipped again" $r/e-$n.err | sed 's/.*shipped again: //' | cut -c1-120 | sort | uniq -c | sort -rn | head -8
  grep "closure:" $r/e-$n.err | sed 's/stream: page [0-9]* //' | sort | uniq -c | sort -rn | head -12
  grep "stream: memo" $r/e-$n.err | tail -1
  du -sk $r/dump-$n | sed 's/^/dump KB: /'
  python3 -I bench/stream-pdfkinds-tmp.py $r/dump-$n | head -40
done
