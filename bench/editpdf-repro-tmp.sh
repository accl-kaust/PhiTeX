#!/bin/bash
# (temporary, agent editpdf) amsldoc's paragraph insert and word edit through watch, -j1/-j8, variants;
# bad PDFs kept and diffed against the clean build of the edited file.
#   editpdf-repro-tmp.sh N "VARIANTS" "JS" "KINDS"   (variant: now | idle | exact)
set -u; cd "$(dirname "$0")/.."; here=$PWD
N=${1:-6}; VARS=${2:-now idle exact}; JS=${3:-1 8}; KINDS=${4:-para word}
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
if [ -n "${OBS:-}" ]; then
  CARGO_TARGET_DIR=$w/target-obs cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
  B=$w/target-obs/release/phitex
else
  cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 || { tail -20 $r/build.txt; exit 1; }
  B=$PWD/target/release/phitex
fi
S=$r/summary.txt; mkdir -p $r/bad
rm -rf $w/c && cp -a bench/amsldoc $w/c && (cd $w/c && PHITEX_CACHE_DIR=$w/cache $B build --exact-pdf -j1 amsldoc.tex > /dev/null 2>&1)
for kind in $KINDS; do
  if [ $kind = para ]; then old="the equation."; new=$'the equation.\n\nAn inserted paragraph with a few new words in it.'; else old="vertically"; new="bertically"; fi
  rm -rf $w/k && cp -a bench/amsldoc $w/k
  python3 -I - "$w/k/amsldoc.tex" "$old" "$new" <<'P'
import sys; p=sys.argv[1]; ls=open(p,"rb").read().split(b"\n"); ls[373]=ls[373].replace(sys.argv[2].encode(), sys.argv[3].encode(),1); open(p,"wb").write(b"\n".join(ls))
P
  (cd $w/k && PHITEX_CACHE_DIR=$w/cache $B build --exact-pdf -j1 amsldoc.tex > /dev/null 2>&1); cp $w/k/amsldoc.pdf $r/clean-$kind.pdf
  good=$(md5sum < $r/clean-$kind.pdf | cut -c1-10); echo "clean $kind: $good" | tee -a $S
  for v in $VARS; do for j in $JS; do bad=0
    for i in $(seq $N); do
      ex=(); wt=0; [ $v = idle ] && wt=2; [ $v = exact ] && ex=(--exact)
      rm -rf $w/e && cp -a bench/amsldoc $w/e
      o=$(cd $w/e && PHITEX_CACHE_DIR=$w/cache timeout 600 env ${ENVS:-} python3 -I $here/bench/editpdf-watch-tmp.py $B . amsldoc.tex 374 "$old" "$new" $j $w/s $wt "${ex[@]}" 2>&1 | tail -1)
      post=$(md5sum < $w/s.post.pdf | cut -c1-10)
      if [ "$post" != "$good" ]; then bad=$((bad+1)); [ -e $r/bad/$kind-$post.pdf ] || { cp $w/s.post.pdf $r/bad/$kind-$post.pdf; cp $w/s.pre.pdf $r/bad/$kind-$post.pre.pdf; cp $w/s.err $r/bad/$kind-$post.err; cp $w/s.events.json $r/bad/$kind-$post.events.json; }; fi
      echo "$kind $v -j$j #$i: $o" >> $S
    done
    echo "== $kind $v -j$j: $bad/$N wrong" | tee -a $S
  done; done
done
for f in $r/bad/*.pdf; do [ -e "$f" ] || continue; k=$(basename $f | cut -d- -f1); echo "--- $f vs clean-$k"; python3 -I bench/f52-pdfdiff-tmp.py $r/clean-$k.pdf $f | head -40; done
