#!/bin/bash
# run.sh: intermediate-rerun data on acclnode11 local disk.
#   prep DOC; one TAG DOC J MODE [env...]  MODE=clean | second
set -u
L=/tmp/intermed-local
O=$L/out; mkdir -p $O
envs=(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1 LANG=C.UTF-8 LC_ALL=C.UTF-8 PHITEX_TIME_LIMIT_SECS=1200)
main_of() { case $1 in amsldoc) echo amsldoc.tex;; para4000) echo para4000.tex;; acm) echo sample-sigconf.tex;; cover) echo course.tex;; grail) echo grail.tex;; esac; }
prep() {
  local doc=$1 m; m=$(main_of $doc)
  local c=$L/snap/$doc-second; rm -rf $c $L/snap/$doc-clean; mkdir -p $c
  for i in 1 2; do
    local w=$L/w-$doc; rm -rf $w; mkdir -p $w; cp -r $L/docs/$doc/. $w/
    ( cd $w && env "${envs[@]}" PHITEX_CACHE_DIR=$c nice $L/t-rel/release/phitex build --shelf --exact-pdf --no-synctex -j 8 $m > /dev/null 2>&1 ); rm -rf $w
  done
  cp -a $c $L/snap/$doc-clean; rm -rf $L/snap/$doc-clean/docs
}
one() {
  local tag=$1 doc=$2 j=$3 mode=$4 keep=$5; shift 5
  local m; m=$(main_of $doc)
  local w=$L/w-$doc; rm -rf $w; mkdir -p $w; cp -r $L/docs/$doc/. $w/
  local c; c=$(mktemp -d $L/c-XXXX); rmdir $c; cp -al $L/snap/$doc-$mode $c
  ( cd $w && env "${envs[@]}" PHITEX_CACHE_DIR=$c "$@" /usr/bin/time -f "WALL %e USER %U RSS %M" nice timeout 1300 $L/t-$tag/release/phitex build --shelf --exact-pdf --no-synctex -j $j $m > $w/out.txt 2> $w/err.txt )
  echo "$tag $doc $mode j$j $(grep -aohE 'WALL.*' $w/err.txt | tail -1) md5 $(md5sum < $w/${m%.tex}.pdf 2>/dev/null | cut -c1-10)"
  [ "$keep" = keep ] && gzip -c $w/err.txt > $O/log-$doc-$mode-j$j${TAGX:-}.gz
  rm -rf $w $c
}
"$@"
