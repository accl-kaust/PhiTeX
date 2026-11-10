#!/bin/bash
# specwin2: the speculation window (PHITEX_SPEC_WINDOW) measured, one part per job:
#   bench/specwin3-tmp.sh floats   floats2col's test 20 times, the CLI at -j16 50 times (hangs, md5)
#   bench/specwin3-tmp.sh clean    clean builds at -j16 off and on, 3 reps (para4000, para400, amsldoc, Grail)
#   bench/specwin3-tmp.sh edits    para4000 and amsldoc edit latency off and on (-j8, -j16)
#   bench/specwin3-tmp.sh tests    chains, census_plain with the window on and off
# WIN_EXTRA: more env for the window-on runs (PHITEX_WINDOW_START=8, ...).
set -u
part=$1
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
on=(PHITEX_SPEC_WINDOW=1 PHITEX_WINDOW_LOG=1 ${WIN_EXTRA:-})
# (the format built first, by a one-line document)
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache timeout 900 $B build warm.tex > /dev/null 2>&1)
case $part in
floats)
  for i in $(seq 1 20); do
    env "${on[@]}" timeout 600 cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture > $r/t-$i.txt 2>&1
    echo "test run $i: exit $? $(grep -E 'fires' $r/t-$i.txt | grep -v 'j 1:' | cut -c1-160 | tr '\n' ' ') $(grep -E '^window:' $r/t-$i.txt | tr '\n' ' ')"
  done
  want=$(md5sum < oracle/fixtures/latex/pdf/floats2col.pdf | cut -c1-12)
  hangs=0
  for i in $(seq 1 50); do
    rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
    s0=$(date +%s.%N)
    (cd $w/f && env "${on[@]}" PHITEX_STALL_SECS=30 PHITEX_CACHE_DIR=$w/cache timeout 120 $B build -j16 --exact-pdf floats2col.tex > $r/f-$i.txt 2>&1); x=$?
    [ $x = 124 ] && hangs=$((hangs+1))
    m=$(md5sum < $w/f/floats2col.pdf 2>/dev/null | cut -c1-12)
    echo "cli run $i: exit $x $(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")s md5 $m $([ "$m" = "$want" ] && echo same || echo DIFFERS) $(grep -h '^window:' $r/f-$i.txt | tail -1)"
  done
  echo "cli: $hangs hangs in 50"
  ;;
clean)
  g=$w/bin/grail-src
  docs="para4000:bench/para4000.tex:para4000 para400:bench/para400.tex:para400 amsldoc:bench/amsldoc:amsldoc"; [ -d "$g" ] && docs="$docs grail:$g:grail"
  for d in $docs; do IFS=: read name src main <<< "$d"
    for i in 1 2 3; do for mode in off on; do
      rm -rf $w/c && mkdir -p $w/c && if [ -d "$src" ]; then cp -a $src/. $w/c/; else cp $src $w/c/; fi
      e=(); [ $mode = on ] && e=("${on[@]}")
      s0=$(date +%s.%N); (cd $w/c && env "${e[@]}" PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$w/cache timeout 1200 $B build -j16 --exact-pdf $main.tex > $r/$name-$mode-$i.txt 2>&1); x=$?
      t=$(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")
      echo "$name -j16 $mode run $i: exit $x ${t}s $(md5sum < $w/c/$main.pdf 2>/dev/null | cut -c1-12) $(grep -oE '[0-9]+ nodes run of [0-9]+ live' $r/$name-$mode-$i.txt | tail -1) runs $(grep -oE 'acct: [0-9]+ runs' $r/$name-$mode-$i.txt | tail -1) $(grep -h '^window:' $r/$name-$mode-$i.txt | tail -1)"
    done; done
  done
  ;;
edits)
  for i in 1 2; do for mode in off on; do for j in 8 16; do
    e=(); [ $mode = on ] && e=("${on[@]}")
    rm -rf $w/pe $w/pe-cold && mkdir -p $w/pe && cp bench/para4000.tex $w/pe/
    python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
    env "${e[@]}" EDITS_STDERR=$r/pe-$mode-$j-$i.err PHITEX_CACHE_DIR=$w/cache timeout 900 python3 -I bench/cut1-edits.py $B $w/pe para4000.tex $j $w/edits.json $r/pe-$mode-$j-$i.json --exact-pdf 2>&1 | grep verdict | sed "s|^|para4000 edits $mode -j$j run $i: |"
  done; done; done
  for i in 1 2; do for mode in off on; do
    e=(); [ $mode = on ] && e=("${on[@]}")
    rm -rf $w/amsh $w/amsh-cold && cp -a bench/amsldoc $w/amsh
    env "${e[@]}" PHITEX_CACHE_DIR=$w/cache timeout 1500 python3 -I scripts/accl/gate-edits.py $B $w/amsh amsldoc.tex 8 20 8 $r/ams-$mode-$i.json --exact-pdf > /dev/null 2>&1
    python3 -I -c 'import json,sys;r=json.load(open(sys.argv[1]));x=sorted(r["edit_walls_s"]);print(f"amsedit {sys.argv[2]} -j8: "+r["verdict"]+f" median {1000*x[len(x)//2]:.1f} ms mean {1000*sum(x)/len(x):.1f} ms")' $r/ams-$mode-$i.json "$mode run $i"
  done; done
  ;;
tests)
  for mode in off on; do
    e=(); [ $mode = on ] && e=("${on[@]}")
    for k in 1 2 3; do
      env "${e[@]}" timeout 1500 cargo test -q --release -p phitex-tex --test chain -- chains census_plain --exact --nocapture > $r/chain-$mode-$k.txt 2>&1
      echo "chain tests window $mode rep $k: $(grep -E '^test result' $r/chain-$mode-$k.txt) $(grep -E 'runs for .* nodes|^test .* (ok|FAILED)$' $r/chain-$mode-$k.txt | tr '\n' ';' | cut -c1-600)"
      grep -E 'para400(dvi|pdf).*(runs|nodes run)|^census|runs a node' $r/chain-$mode-$k.txt | head -8 | cut -c1-300
    done
  done
  timeout 1500 cargo test -q --release -p phitex-defuse > $r/defuse.txt 2>&1; echo "defuse: $(grep -E '^test result' $r/defuse.txt | tr '\n' ' ') $(grep -E 'FAILED|panicked' $r/defuse.txt | head -5 | tr '\n' ' ')"
  ;;
esac
