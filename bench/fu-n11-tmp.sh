#!/bin/bash
# (temporary, agent firstuse) on acclnode11: build COMMIT into $R/bin-TAG (PROF=1: profile+observe), then clean builds.
#   fu-n11-tmp.sh build TAG COMMIT
#   fu-n11-tmp.sh run TAG DOC "JS" REPS [env...]   (DOC: grail lie ams); KEEP=1 keeps out/err
#   fu-n11-tmp.sh ab "TAGS" DOC "JS" REPS [env...]  interleaved: rep, j, tag
set -u
R=$HOME/code/flinner/phitex-runs/n11-fu
mkdir -p $R
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=cc RUSTFLAGS=-Clink-arg=-fuse-ld=bfd
S=$HOME/code/flinner/n11-fu-src
one() { # tag doc j i env...
  local tag=$1 doc=$2 j=$3 i=$4; shift 4
  case $doc in grail) src=$HOME/code/flinner/grail-src m=grail.tex ;; lie) src=$HOME/code/flinner/phitex-runs/n11-scale/lie-src m=lie-hasse.tex ;; ams) src=$S/bench/amsldoc m=amsldoc.tex ;; *) src=$R/doc-$doc m=$doc.tex ;; esac
  w=$(mktemp -d /tmp/fu-XXXX); cp -r $src/. $w/
  ( cd $w && env "$@" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_CACHE_DIR=$R/cache-$tag timeout 3000 /usr/bin/time -f "WALL %e USER %U RSS %M" nice $R/bin-$tag/phitex build --shelf --exact-pdf -j $j $m > $w/out.txt 2> $w/err.txt )
  echo "$tag $doc j$j #$i $(grep -ohE 'WALL.*' $w/err.txt $w/out.txt | head -1) md5 $(md5sum < $w/${m%.tex}.pdf 2>/dev/null | cut -c1-10) logmd5 $(md5sum < $w/${m%.tex}.log 2>/dev/null | cut -c1-10)"
  if [ -n "${KEEP:-}" ]; then cp $w/out.txt $R/out-$tag-$doc-j$j-$i.txt; gzip -c $w/err.txt > $R/err-$tag-$doc-j$j-$i.txt.gz; cp $w/${m%.tex}.log $R/log-$tag-$doc-j$j-$i.log 2>/dev/null; fi
  rm -rf $w
}
case $1 in
build)
  tag=$2 c=$3
  [ -d $S ] || git clone -q $HOME/code/flinner/phitex.git $S
  cd $S && git fetch -q origin && git checkout -q -f $c || exit 1
  if [ -n "${PROF:-}" ]; then T=$R/target-p F="--features phitex-tex/profile,phitex-tex/observe"; else T=$R/target F=; fi
  CARGO_TARGET_DIR=$T nice $HOME/.cargo/bin/cargo +nightly-2026-08-08 build -j 16 -q --release -p phitex-cli $F > $R/build-$tag.log 2>&1 || { tail -30 $R/build-$tag.log; exit 1; }
  mkdir -p $R/bin-$tag; cp $T/release/phitex $R/bin-$tag/phitex
  echo "built $tag $(git log --oneline -1 | cut -c1-70)" ;;
run)
  tag=$2 doc=$3 js=$4 reps=$5; shift 5
  for i in $(seq 1 $reps); do for j in $js; do one $tag $doc $j $i "$@"; done; done ;;
ab)
  tags=$2 doc=$3 js=$4 reps=$5; shift 5
  for i in $(seq 1 $reps); do for j in $js; do for t in $tags; do one $t $doc $j $i "$@"; done; done; done ;;
esac
