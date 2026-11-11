#!/bin/bash
# (temporary) specwin2: clean builds at -j16 with the edit's chain holds (PHITEX_CLEAN_HOLDS=1) and without,
# alternated in one job. DOCS: para4000 amsldoc grail tikz (grail: $w/bin/grail-src; tikz: TeX Live's pgf docs). REPS (3).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache $B build warm.tex > /dev/null 2>&1)
for d in ${DOCS:-para4000 amsldoc grail}; do
  case $d in para4000) src=bench/para4000.tex main=para4000 ;; amsldoc) src=bench/amsldoc main=amsldoc ;;
    grail) src=$w/bin/grail-src main=grail ;; tikz) src=${TIKZMAN_DOC:-/usr/share/doc/texlive}/generic/pgf main=pgfmanual ;; esac
  [ -e "$src" ] || { echo "$d: no $src"; continue; }
  for i in $(seq 1 ${REPS:-3}); do for h in ${HS:-0 1}; do
    rm -rf $w/c && mkdir -p $w/c && if [ -d "$src" ]; then cp -a $src/. $w/c/; else cp $src $w/c/; fi
    s0=$(date +%s.%N); (cd $w/c && PHITEX_CLEAN_HOLDS=$h PHITEX_TIME_LIMIT_SECS=0 PHITEX_CACHE_DIR=$w/cache timeout 3600 $B build -vv -j${J:-16} --exact-pdf $main.tex > $r/$d-$h-$i.txt 2>&1); x=$?
    echo "$d -j${J:-16} holds=$h run $i: exit $x $(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")s $(md5sum < $w/c/$main.pdf 2>/dev/null | cut -c1-12) $(grep -oE '[0-9]+ nodes run of [0-9]+ live, [0-9]+ runs' $r/$d-$h-$i.txt | tail -1)"
  done; done
done
