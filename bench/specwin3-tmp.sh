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
# MODES: off, on, and on with a measure's knob: nofirst (PHITEX_FIRST_READS=0),
# wave (PHITEX_WINDOW_WAVE=1), w8 (PHITEX_WINDOW_START=8)
envof() { case $1 in off) e=() ;; on) e=("${on[@]}") ;; nofirst) e=("${on[@]}" PHITEX_FIRST_READS=0) ;;
  wave) e=("${on[@]}" PHITEX_WINDOW_WAVE=1) ;; nochains) e=("${on[@]}" PHITEX_WINDOW_CHAINS=0) ;;
  floor0) e=("${on[@]}" PHITEX_WINDOW_FLOOR=0) ;; pin0) e=("${on[@]}" PHITEX_WINDOW_FLOOR=0 PHITEX_WINDOW_PIN=1) ;; noaimd) e=("${on[@]}" PHITEX_WINDOW_AIMD=0) ;; w8) e=("${on[@]}" PHITEX_WINDOW_START=8) ;; esac; }
# (the format built first, by a one-line document)
mkdir -p $w/warm && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > $w/warm/warm.tex
(cd $w/warm && PHITEX_CACHE_DIR=$w/cache timeout 900 $B build warm.tex > /dev/null 2>&1)
case $part in
floats)
  for mode in ${MODES:-on}; do envof $mode
  for i in $(seq 1 ${FN:-20}); do
    env "${e[@]}" timeout 600 cargo test -q --release -p phitex-tex --test latex floats2col_routines_run_once_per_fire -- --nocapture > $r/t-$mode-$i.txt 2>&1
    echo "$mode test run $i: exit $? $(grep -E 'fires' $r/t-$mode-$i.txt | grep -v 'j 1:' | cut -c1-160 | tr '\n' ' ') $(grep -E '^window:' $r/t-$mode-$i.txt | tr '\n' ' ')"
  done
  # (the CLI's reference: a -j1 build, window off)
  rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
  (cd $w/f && PHITEX_CACHE_DIR=$w/cache timeout 300 $B build -j1 --exact-pdf floats2col.tex > $r/f-ref.txt 2>&1)
  want=$(md5sum < $w/f/floats2col.pdf | cut -c1-12)
  hangs=0
  for i in $(seq 1 ${CN:-50}); do
    rm -rf $w/f && mkdir -p $w/f && cp oracle/fixtures/latex/pdfdocs/floats2col.tex $w/f/
    s0=$(date +%s.%N)
    (cd $w/f && env "${e[@]}" PHITEX_TEX_TIME=1 PHITEX_STALL_SECS=30 PHITEX_CACHE_DIR=$w/cache timeout 120 $B build -j16 --exact-pdf floats2col.tex > $r/f-$mode-$i.txt 2>&1); x=$?
    [ $x = 124 ] && hangs=$((hangs+1))
    m=$(md5sum < $w/f/floats2col.pdf 2>/dev/null | cut -c1-12)
    echo "$mode cli run $i: exit $x $(python3 -c "import time;print(f'{time.time()-$s0:.2f}')")s md5 $m $([ "$m" = "$want" ] && echo same || echo DIFFERS) $(grep -oE '^acct: [0-9]+ runs' $r/f-$mode-$i.txt | tail -1) $(grep -h '^window:' $r/f-$mode-$i.txt | tail -1)"
  done
  echo "$mode cli: $hangs hangs in ${CN:-50}"
  done
  ;;
clean)
  g=$w/bin/grail-src; ls $w/bin 2>&1 | head -3
  docs="para4000:bench/para4000.tex:para4000 para400:bench/para400.tex:para400 amsldoc:bench/amsldoc:amsldoc"; [ -d "$g" ] && docs="$docs grail:$g:grail"
  for d in $docs; do IFS=: read name src main <<< "$d"
    for i in 1 2 3; do for mode in ${MODES:-off on}; do
      rm -rf $w/c && mkdir -p $w/c && if [ -d "$src" ]; then cp -a $src/. $w/c/; else cp $src $w/c/; fi
      envof $mode
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
census)
  # census_plain with the run log: the runs again, by operation and cause
  for mode in ${MODES:-off on}; do envof $mode
    env "${e[@]}" PHITEX_RUN_LOG=1 timeout 1500 cargo test -q --release -p phitex-tex --features profile,observe --test chain -- census_plain --exact --nocapture > $r/census-$mode.txt 2>&1
    echo "census $mode: $(grep -E '^test result' $r/census-$mode.txt) $(grep -E '^para400(dvi|pdf) \|' $r/census-$mode.txt | cut -c1-70 | tr '\n' ' ')"
    grep -E '^run: node' $r/census-$mode.txt | grep -v 'goes on true' | grep -v 'Created' | sed -E 's/^run: node [0-9]+ \(([a-z]+)\).*: ([a-z ]+); goes on false, woken by Some\(([A-Za-z]+).*/\1 \2 \3/' | sort | uniq -c | sort -rn | head -12 | sed "s/^/  $mode: /"
  done
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
