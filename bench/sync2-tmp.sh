#!/bin/bash
# sync2's check (the ci image): 5 manuals at -j1/-j16 against their oracles; para4000 and
# amsldoc watch edits with SyncTeX (keystroke, the final .synctex vs a clean build's).
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
( cargo build -q --release -p phitex-cli > $r/build.txt 2>&1 ) || { echo BUILD FAILED; tail $r/build.txt; exit 1; }
B=$PWD/target/release/phitex
export CI_WORK=$w/ci; mkdir -p $CI_WORK $w/no-oracle
python3 -I scripts/ci/sweep.py run --list bench/manuals5-tmp.json --doc /usr/share/doc/texlive --cache $w/no-oracle --bin $B --out $r/manuals --engines pdflatex --tools bibtex,biber,makeindex,glossaries --jobs 1,16 --par 1:2,16:1 --timeout 900 --diff-oracle > $r/sweep.txt 2>&1 || echo "sweep failed: $(tail -1 $r/sweep.txt)"
python3 -I scripts/ci/sweep.py table $r/manuals > $r/table.txt 2>&1; cat $r/table.txt | tail -15
python3 -I scripts/ci/verdict.py bench/manuals5-tmp.json 0/1 1,16 $r/manuals --prefix "manuals: " | sed 's/^/RED /' | head
for j in 8 16; do
  rm -rf $w/pe $w/pe-cold $w/cold && mkdir -p $w/pe $w/cold && cp bench/para4000.tex $w/pe/
  python3 -I bench/cut1-paraedit.py $w/pe/para4000.tex 6 > $w/edits.json
  PHITEX_CACHE_DIR=$w/cache timeout 900 python3 -I bench/cut1-edits.py $B $w/pe para4000.tex $j $w/edits.json $r/pe-$j.json --exact-pdf 2>&1 | grep verdict | sed "s|^|para -j$j: |"
  zcat $w/pe/para4000.synctex.gz > $r/watch-pe-$j.synctex
  cp $w/pe/para4000.tex $w/cold/ && (cd $w/cold && PHITEX_CACHE_DIR=$w/cache $B build -j$j --synctex --exact-pdf para4000.tex > /dev/null 2>&1); zcat $w/cold/para4000.synctex.gz > $r/cold-pe-$j.synctex
  echo "para -j$j: .synctex lines differing from a clean build's: $(diff $r/watch-pe-$j.synctex $r/cold-pe-$j.synctex | grep -c '^[<>]') (2: its path)"
  rm -rf $w/amsh $w/amsh-cold $w/acold && cp -a bench/amsldoc $w/amsh
  PHITEX_CACHE_DIR=$w/cache timeout 1200 python3 -I scripts/accl/gate-edits.py $B $w/amsh amsldoc.tex $j 20 8 $r/ams-$j.json --exact-pdf > /dev/null 2>&1
  python3 -I -c 'import json,sys;r=json.load(open(sys.argv[1]));x=sorted(r["edit_walls_s"]);print(f"amsedit -j{sys.argv[2]}: "+r["verdict"]+f" median {1000*x[len(x)//2]:.1f} ms")' $r/ams-$j.json $j
  zcat $w/amsh/amsldoc.synctex.gz > $r/watch-ams-$j.synctex
  cp -a $w/amsh $w/acold && (cd $w/acold && rm -f *.pdf *.aux *.toc *.out *.log *.synctex.gz && PHITEX_CACHE_DIR=$w/cache $B build -j$j --synctex --exact-pdf amsldoc.tex > /dev/null 2>&1); zcat $w/acold/amsldoc.synctex.gz > $r/cold-ams-$j.synctex
  echo "amsldoc -j$j: .synctex lines differing from a clean build's: $(diff $r/watch-ams-$j.synctex $r/cold-ams-$j.synctex | grep -c '^[<>]') (2: its path)"; diff $r/watch-ams-$j.synctex $r/cold-ams-$j.synctex | head -6
done
