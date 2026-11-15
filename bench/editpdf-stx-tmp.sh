#!/bin/bash
# (temporary, agent editpdf) technote's paragraph insert: the watch's .synctex.gz (q at once, and after idle) against
# the clean build's, decompressed and diffed.
set -u; cd "$(dirname "$0")/.."; here=$PWD; S=$r/summary.txt
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
B=$PWD/target/release/phitex; DIST=$(kpsewhich -var-value TEXMFDIST); src=$DIST/doc/latex/amsmath; main=technote.tex
ed=$(python3 -I bench/editpdf-pick-tmp.py $src/$main x); line=$(echo "$ed" | cut -f1); pold=$(echo "$ed" | cut -f4)
n="$pold"$'\n\nAn inserted paragraph with a few new words in it.'
edit() { python3 -I - "$1" "$line" "$pold" "$n" <<'P'
import sys; p=sys.argv[1]; k=int(sys.argv[2])-1; ls=open(p,"rb").read().split(b"\n"); ls[k]=ls[k].replace(sys.argv[3].encode(), sys.argv[4].encode(),1); open(p,"wb").write(b"\n".join(ls))
P
}
rm -rf $w/e && cp -a $src $w/e && (cd $w/e && PHITEX_CACHE_DIR=$w/c $B build -j1 $main >/dev/null 2>&1); edit $w/e/$main
(cd $w/e && rm -f technote.pdf technote.synctex.gz && PHITEX_CACHE_DIR=$w/c $B build --synctex -j1 $main >/dev/null 2>&1); cp $w/e/technote.synctex.gz $r/clean.synctex.gz; cp $w/e/technote.pdf $r/clean.pdf
# (a watch of the edited text from the start: its cold build's synctex)
rm -rf $w/e && cp -a $src $w/e && edit $w/e/$main
(cd $w/e && PHITEX_CACHE_DIR=$w/c timeout 600 python3 -I $here/bench/editpdf-watch-tmp.py $B . $main 1 "zzzz-none" "x" 1 $w/s0 3 2>&1 | tail -1) || true
for v in 0 3; do
  rm -rf $w/e && cp -a $src $w/e
  (cd $w/e && PHITEX_CACHE_DIR=$w/c timeout 600 python3 -I $here/bench/editpdf-watch-tmp.py $B . $main $line "$pold" "$n" 1 $w/s $v > $r/w$v.txt 2>&1)
  cp $w/e/technote.synctex.gz $r/watch-wait$v.synctex.gz 2>/dev/null; cp $w/e/technote.pdf $r/watch-wait$v.pdf
  echo "wait $v: pdf $(cmp -s $r/clean.pdf $r/watch-wait$v.pdf && echo same || echo differs), synctex $(cmp -s $r/clean.synctex.gz $r/watch-wait$v.synctex.gz && echo same || echo differs)" | tee -a $S
  zcat $r/clean.synctex.gz > $r/clean.stx; zcat $r/watch-wait$v.synctex.gz > $r/watch$v.stx
  diff $r/clean.stx $r/watch$v.stx | head -30 | tee -a $S; echo "diff lines: $(diff $r/clean.stx $r/watch$v.stx | wc -l)" | tee -a $S
done
