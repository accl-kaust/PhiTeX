#!/bin/bash
# (temporary) pages with forms and images replayed: CONTROL against this commit on DOCS (TeX
# Live manuals by directory/main, the CI image; grail): the clean build's PDF at -j1 and -j8
# (md5 against CONTROL's -j1), the watch with its viewer at -j1 with EDITS one-word edits (its
# final PDF against a clean build's, stream-edits-tmp.py's verdict), and with the engine's
# traces (`observe`) the link's pages replayed or shipped again and why, the build's link and
# the pages' files apart.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
build() { ( cd "$2" && cargo build -q --release -p phitex-cli $3 > "$r/build-$1.txt" 2>&1 ) || { echo "BUILD FAILED $1"; tail "$r/build-$1.txt"; return 1; }; mkdir -p $w/bins/$1 && cp "$2/target/release/phitex" $w/bins/$1/; }
rm -rf $w/control && mkdir -p $w/control && git archive "$CONTROL" | tar -x -C $w/control; [ -d upstream ] && ln -s $PWD/upstream $w/control/upstream
build control $w/control "" & build head $PWD "" || exit 1; wait
build obs $PWD "--features phitex-tex/observe" || exit 1
[ -x $w/bins/control/phitex ] || exit 1
list=""
for d in ${DOCS:-}; do
  if [ $d = grail ]; then mkdir -p $w/docs/grail && cp -a $w/bin/grail-src/. $w/docs/grail/ && rm -f $w/docs/grail/grail.pdf; list="$list grail/grail.tex"; continue; fi
  n=$(basename $(dirname $d)); mkdir -p $w/docs/$n && cp -a /usr/share/doc/texlive/$(dirname $d)/. $w/docs/$n/ && rm -f $w/docs/$n/$(basename ${d%.tex}).pdf
  list="$list $n/$(basename $d)"
done
for spec in $list; do
  dn=$(dirname $spec) main=$(basename $spec) pdf=$(basename ${spec%.tex}).pdf
  sums=""
  for v in control:1 head:1 head:8; do
    b=${v%:*} j=${v#*:}
    rm -rf $w/c && cp -a $w/docs/$dn $w/c
    (cd $w/c && PHITEX_CACHE_DIR=$w/cache-$b timeout 1800 $w/bins/$b/phitex build -j$j $main > $r/b-$b-$j-$dn.txt 2>&1)
    sums="$sums $b-j$j $(md5sum < $w/c/$pdf 2>/dev/null | cut -c1-12)"
  done
  set -- $sums
  verdict=$([ "$2" = "$4" ] && [ "$2" = "$6" ] && echo same || echo DIFFERENT)
  echo "== $dn: build md5$sums: $verdict"
  rm -rf $w/e && cp -a $w/docs/$dn $w/e
  python3 -I bench/cut1-paraedit.py $w/e/$main ${EDITS:-4} > $w/edits.json
  PHITEX_CACHE_DIR=$w/cache-head timeout 3000 python3 -I bench/stream-edits-tmp.py $w/bins/head/phitex $w/e $main 1 $w/edits.json $r/e-$dn.json --view --write > $r/e-$dn.txt 2>&1
  echo "   watch -j1: $(tail -1 $r/e-$dn.txt)"
  rm -rf $w/e && cp -a $w/docs/$dn $w/e
  PHITEX_PDF_TRACE=1 PHITEX_STREAM_TRACE=1 EDITS_STDERR=$r/o-$dn.err PHITEX_CACHE_DIR=$w/cache-obs timeout 3000 python3 -I bench/stream-edits-tmp.py $w/bins/obs/phitex $w/e $main 1 $w/edits.json $r/o-$dn.json --view --write > $r/o-$dn.txt 2>&1
  echo "   traced watch: $(tail -1 $r/o-$dn.txt)"
  echo "   build's link:"; grep -E "^  link page" $r/o-$dn.err | sed -E 's/.*\((replayed|shipped again: [^0-9]*).*/\1/' | sort | uniq -c | sort -rn | head -8
  echo "   pages' files:"; grep -E "^stream: page [0-9]+ (replayed|shipped again)" $r/o-$dn.err | sed -E 's/^stream: page [0-9]+ //; s/[0-9]+/N/g' | sort | uniq -c | sort -rn | head -8
done
