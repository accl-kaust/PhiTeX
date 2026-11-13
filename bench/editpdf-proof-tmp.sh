#!/bin/bash
# (temporary, agent editpdf) watch sessions as the scaling agent's harness runs them (bench/watchedit.py: q at once
# after the edit's build event, the PDF read after the watch ended) against the clean build of the edited file.
#   editpdf-proof-tmp.sh N "BINS" "JS" "DOCS" [BASE]     BINS: before after; DOCS: ams or manual paths (TEXMFDIST/doc/...)
set -u; cd "$(dirname "$0")/.."; here=$PWD
N=${1:-60}; BINS=${2:-after}; JS=${3:-1 8}; DOCS=${4:-ams}; BASE=${5:-81a72ba6d}; S=$r/summary.txt
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
for t in $BINS; do
  d=$here
  if [ $t = before ]; then rm -rf $w/before && mkdir -p $w/before && git archive HEAD | tar -x -C $w/before
    for f in crates/cli/src/session.rs crates/cli/src/watch.rs; do git show $BASE:$f > $w/before/$f; done; d=$w/before; fi
  (cd $d && CARGO_TARGET_DIR=$w/target-$t cargo build -q --release -p phitex-cli > $r/build-$t.txt 2>&1) || { echo "$t: build failed"; tail -30 $r/build-$t.txt; exit 1; }
  mkdir -p $w/bin-$t && cp $w/target-$t/release/phitex $w/bin-$t/
done
DIST=$(kpsewhich -var-value TEXMFDIST 2>/dev/null)
for doc in $DOCS; do
  if [ $doc = ams ]; then src=$here/bench/amsldoc; main=amsldoc.tex; else src=$DIST/doc/$(dirname $doc); main=$(basename $doc); fi
  [ -f $src/$main ] || { echo "$doc: no $src/$main" | tee -a $S; continue; }
  tag=$(basename $main .tex)
  # (the edits: amsldoc's line 374 as the scaling agent's; else a prose line in the body's middle)
  ed=$(python3 -I $here/bench/editpdf-pick-tmp.py $src/$main $doc) || { echo "$doc: no edit found" | tee -a $S; continue; }
  echo "$doc: $ed" | tee -a $S
  line=$(echo "$ed" | cut -f1); wold=$(echo "$ed" | cut -f2); wnew=$(echo "$ed" | cut -f3); pold=$(echo "$ed" | cut -f4)
  for kind in word para; do
    if [ $kind = word ]; then o=$wold; n=$wnew; else o=$pold; n="$pold"$'\n\nAn inserted paragraph with a few new words in it.'; fi
    for t in $BINS; do B=$w/bin-$t/phitex; C=$w/cache-$t-$tag
      rm -rf $w/k && cp -a $src $w/k
      (cd $w/k && PHITEX_CACHE_DIR=$C $B build --exact-pdf -j1 $main > /dev/null 2>&1)
      python3 -I - "$w/k/$main" "$line" "$o" "$n" <<'P'
import sys; p=sys.argv[1]; k=int(sys.argv[2])-1; ls=open(p,"rb").read().split(b"\n"); assert sys.argv[3].encode() in ls[k]; ls[k]=ls[k].replace(sys.argv[3].encode(), sys.argv[4].encode(),1); open(p,"wb").write(b"\n".join(ls))
P
      good=""; for j in 1 8; do (cd $w/k && rm -f ${main%.tex}.pdf && PHITEX_CACHE_DIR=$C $B build --exact-pdf -j$j $main > /dev/null 2>&1); good="$good $(md5sum < $w/k/${main%.tex}.pdf | cut -c1-10)"; done
      good1=$(echo $good | cut -d' ' -f1)
      echo "$tag $kind $t clean edited -j1/-j8:$good" | tee -a $S
      for j in $JS; do bad=0; ms=""
        for i in $(seq $N); do
          rm -rf $w/e && cp -a $src $w/e
          (cd $w/e && PHITEX_CACHE_DIR=$C timeout 600 python3 -I $here/bench/watchedit.py $B . $main $main $line "$o" "$n" $j > $r/w.txt 2>&1)
          m=$(md5sum < $w/e/${main%.tex}.pdf 2>/dev/null | cut -c1-10)
          [ "$m" = "$good1" ] || { bad=$((bad+1)); echo "  $tag $kind $t -j$j #$i wrong: $m $(tail -1 $r/w.txt)" >> $S; }
          ms="$ms $(tail -1 $r/w.txt | awk '{print $4}')"
        done
        med=$(echo $ms | tr ' ' '\n' | grep -v None | sort -g | awk '{a[NR]=$1} END{print a[int((NR+1)/2)]}')
        echo "== $tag $kind $t -j$j: $bad/$N wrong; edit ms median $med" | tee -a $S
      done
    done
  done
done
