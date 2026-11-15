#!/bin/bash
# run2.sh cnt TAG DOC J MODE [env...]: a -vv build; wall, md5 and the run counts line
set -u
L=/tmp/intermed-local
envs=(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1 LANG=C.UTF-8 LC_ALL=C.UTF-8 PHITEX_TIME_LIMIT_SECS=1200)
main_of() { case $1 in amsldoc) echo amsldoc.tex;; acm) echo sample-sigconf.tex;; grail) echo grail.tex;; esac; }
cnt() {
  local tag=$1 doc=$2 j=$3 mode=$4; shift 4
  local m; m=$(main_of $doc)
  local w=$L/w2-$doc; rm -rf $w; mkdir -p $w; cp -r $L/docs/$doc/. $w/
  local c; c=$(mktemp -d $L/c-XXXX); rmdir $c; cp -a $L/snap/$doc-$mode $c
  ( cd $w && env "${envs[@]}" PHITEX_CACHE_DIR=$c "$@" /usr/bin/time -f "WALL %e USER %U RSS %M" nice timeout 1300 $L/t-$tag/release/phitex build -vv --shelf --exact-pdf --no-synctex -j $j $m > $w/out.txt 2> $w/err.txt )
  echo "$tag $doc $mode j$j [$*] $(grep -aohE "WALL.*" $w/err.txt | tail -1) md5 $(md5sum < $w/${m%.tex}.pdf 2>/dev/null | cut -c1-10) | $(cat $w/out.txt $w/err.txt | grep -aoE "[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs" | tail -1)"
  rm -rf $w $c
}
"$@"
