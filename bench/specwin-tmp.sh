#!/bin/bash
# specwin: floats2col's test with the speculation window on (first width 8, and the widest), and
# off once; clean builds at -j16 off and on (amsldoc, para4000, para400, Grail): time and md5.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture 2>&1 | grep -E "fires" | sed "s/^/window off: /"
for s in 8 1048576; do for i in 1 2 3; do
  PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_START=$s cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture 2>&1 | grep -E "fires|test result" | grep -v "j 1:" | cut -c1-120 | sed "s/^/window on start $s run $i: /"
done; done
g=$w/bin/grail-src
docs="amsldoc:bench/amsldoc:amsldoc para4000:bench/para4000.tex:para4000 para400:bench/para400.tex:para400"; [ -d "$g" ] && docs="$docs grail:$g:grail"
for d in $docs; do IFS=: read name src main <<< "$d"
  for i in 1 2; do for mode in off 8 1048576; do
    rm -rf $w/c && mkdir -p $w/c && if [ -d "$src" ]; then cp -a $src/. $w/c/; else cp $src $w/c/; fi
    e=(); [ $mode != off ] && e=(PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 PHITEX_WINDOW_START=$mode)
    s0=$(date +%s.%N); (cd $w/c && env "${e[@]}" PHITEX_CACHE_DIR=$w/cache timeout 1200 $B build -j16 --exact-pdf $main.tex > $r/$name-$mode-$i.txt 2>&1); x=$?
    t=$(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")
    echo "$name -j16 window $mode run $i: exit $x ${t}s $(md5sum < $w/c/$main.pdf 2>/dev/null | cut -c1-12) $(grep -h "^window:" $r/$name-$mode-$i.txt | tail -1)"
  done; done
done
