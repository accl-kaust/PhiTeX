#!/bin/bash
# specwin: floats2col's test 5 times with the speculation window on (and once off); clean builds
# at -j16 with it off and on (amsldoc, para4000, Grail): time and md5.
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture 2>&1 | grep -E "fires" | sed "s/^/window off: /"
for i in 1 2 3 4 5; do
  PHITEX_SPEC_WINDOW=1 cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture 2>&1 | grep -E "fires|test result" | cut -c1-200 | sed "s/^/window on run $i: /"
done
g=$w/bin/grail-src; [ -d $g ] || g=$(ls -d $HOME/code/flinner/phitex-runs/gate-bin/*/grail-src 2>/dev/null | head -1); [ -d "$g" ] || g=$(find $HOME -maxdepth 6 -type d -name grail-src 2>/dev/null | head -1); echo "grail: ${g:-none}"
docs="amsldoc:bench/amsldoc:amsldoc para4000:bench/para4000.tex:para4000"; [ -n "$g" ] && docs="$docs grail:$g:grail"
for d in $docs; do IFS=: read name src main <<< "$d"
  for i in 1 2 3; do for mode in off on; do
    rm -rf $w/c && mkdir -p $w/c && if [ -d "$src" ]; then cp -a $src/. $w/c/; else cp $src $w/c/; fi
    e=(); [ $mode = on ] && e=(PHITEX_SPEC_WINDOW=1)
    s0=$(date +%s.%N); (cd $w/c && env "${e[@]}" PHITEX_CACHE_DIR=$w/cache timeout 1200 $B build -j16 --exact-pdf $main.tex > $r/$name-$mode-$i.txt 2>&1); x=$?
    t=$(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")
    echo "$name -j16 window $mode run $i: exit $x ${t}s $(md5sum < $w/c/$main.pdf 2>/dev/null | cut -c1-12)"
  done; done
done
