#!/bin/bash
# probe.sh: persisted-graph probe on acclnode11 local disk.
#   prep DOC   (two builds into $L/snap/DOC: a second build's cache)
#   one DOC J TAG [env...]
set -u
L=/tmp/persist-local
O=$L/out; mkdir -p $O
B=$L/t/release/phitex
envs=(SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1 LANG=C.UTF-8 LC_ALL=C.UTF-8 PHITEX_TIME_LIMIT_SECS=1200)
main_of() { case $1 in amsldoc) echo amsldoc.tex;; para4000) echo para4000.tex;; acm) echo sample-sigconf.tex;; grail) echo grail.tex;; tiny) echo tiny.tex;; esac; }
prep() {
  local doc=$1 m; m=$(main_of $doc)
  local c=$L/snap/$doc; rm -rf $c; mkdir -p $c
  if [ -d /tmp/intermed-local/snap/$doc-second ]; then cp -a /tmp/intermed-local/snap/$doc-second/. $c/; fi
  for i in 1 2; do
    local w=$L/w-$doc; rm -rf $w; mkdir -p $w; cp -r $L/docs/$doc/. $w/
    ( cd $w && env "${envs[@]}" PHITEX_CACHE_DIR=$c nice $B build --shelf --exact-pdf --no-synctex -j 8 $m > /dev/null 2>&1 ); rm -rf $w
  done
}
one() {
  local doc=$1 j=$2 tag=$3; shift 3
  local m; m=$(main_of $doc)
  local w=$L/w-$doc; rm -rf $w; mkdir -p $w; cp -r $L/docs/$doc/. $w/
  local c; c=$(mktemp -d $L/c-XXXX); rmdir $c; cp -a $L/snap/$doc $c
  ( cd $w && env "${envs[@]}" PHITEX_CACHE_DIR=$c "$@" /usr/bin/time -f "WALL %e USER %U RSS %M" nice timeout 1300 $B build --shelf --exact-pdf --no-synctex -j $j $m > $w/out.txt 2> $w/err.txt )
  echo "== $doc j$j $tag [$*] $(grep -aohE 'WALL.*' $w/err.txt | tail -1) md5 $(md5sum < $w/${m%.tex}.pdf 2>/dev/null | cut -c1-10)"
  grep -a "${GREP:-persist-probe}" $w/err.txt
  rm -rf $w $c
}
"$@"
