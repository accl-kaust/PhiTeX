#!/bin/bash
# (temporary, agent editpdf) par-editpdf-tmp.sh's sequence (the clean builds, then watch sessions alternating -j1/-j8,
# all on one cache), the cache copied before each session; a wrong session's cache kept and replayed REPLAY times.
#   editpdf-hist-tmp.sh [ROUNDS] [REPLAY] [KIND]
set -u; cd "$(dirname "$0")/.."; here=$PWD
R=${1:-6}; REP=${2:-3}; KIND=${3:-para}
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
if [ -n "${OBS:-}" ]; then
  CARGO_TARGET_DIR=$w/target-obs cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
  B=$w/target-obs/release/phitex
else
  cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
  B=$PWD/target/release/phitex
fi
S=$r/summary.txt; mkdir -p $r/bad
if [ $KIND = para ]; then o="the equation."; n=$'the equation.\n\nAn inserted paragraph with a few new words in it.'; else o=vertically; n=bertically; fi
rm -rf $w/c && cp -a bench/amsldoc $w/c && (cd $w/c && PHITEX_CACHE_DIR=$w/cache $B build --exact-pdf -j1 amsldoc.tex > /dev/null 2>&1)
python3 -I - "$w/c/amsldoc.tex" "$o" "$n" <<'P'
import sys; p=sys.argv[1]; ls=open(p,"rb").read().split(b"\n"); ls[373]=ls[373].replace(sys.argv[2].encode(), sys.argv[3].encode(),1); open(p,"wb").write(b"\n".join(ls))
P
for j in 1 8; do (cd $w/c && rm -f amsldoc.pdf && PHITEX_CACHE_DIR=$w/cache $B build --exact-pdf -j$j amsldoc.tex > /dev/null 2>&1; echo "clean edited -j$j: $(md5sum < amsldoc.pdf | cut -c1-10)" | tee -a $S); done
good=$(md5sum < $w/c/amsldoc.pdf | cut -c1-10); cp $w/c/amsldoc.pdf $r/clean.pdf
echo "cache size $(du -sh $w/cache | cut -f1)" | tee -a $S
nb=0
for i in $(seq $R); do for j in 1 8; do
  rm -rf $w/cache-before && cp -a $w/cache $w/cache-before
  rm -rf $w/e && cp -a bench/amsldoc $w/e
  (cd $w/e && PHITEX_CACHE_DIR=$w/cache timeout 600 python3 -I $here/bench/watchedit.py $B . amsldoc.tex amsldoc.tex 374 "$o" "$n" $j > $r/w.txt 2>&1)
  m=$(md5sum < $w/e/amsldoc.pdf | cut -c1-10)
  echo "watch -j$j #$i: $(tail -1 $r/w.txt) after exit $m" | tee -a $S
  if [ "$m" != "$good" ] && [ $nb -lt 2 ]; then nb=$((nb+1))
    cp $w/e/amsldoc.pdf $r/bad/bad$nb-$m.pdf; rm -rf $w/badcache$nb; cp -a $w/cache-before $w/badcache$nb
    (cd $w/cache-before && find . -type f | sort | xargs md5sum) > $r/bad/cache$nb.md5
    for k in $(seq $REP); do for jj in 1 8; do
      rm -rf $w/rc && cp -a $w/badcache$nb $w/rc; rm -rf $w/e2 && cp -a bench/amsldoc $w/e2
      (cd $w/e2 && PHITEX_CACHE_DIR=$w/rc timeout 600 python3 -I $here/bench/watchedit.py $B . amsldoc.tex amsldoc.tex 374 "$o" "$n" $jj > $r/w2.txt 2>&1)
      echo "  replay bad$nb's cache -j$jj #$k: $(md5sum < $w/e2/amsldoc.pdf | cut -c1-10)" | tee -a $S
    done; done
    echo "--- diff"; python3 -I bench/f52-pdfdiff-tmp.py $r/clean.pdf $r/bad/bad$nb-$m.pdf | head -30 | tee -a $S
  fi
done; done
(cd $w/cache && find . -type f | sort | xargs md5sum) > $r/cache-end.md5
[ -d $w/badcache1 ] && tar czf $r/badcache1.tgz -C $w --exclude='*.fmt' badcache1 2>/dev/null; ls -la $r/
