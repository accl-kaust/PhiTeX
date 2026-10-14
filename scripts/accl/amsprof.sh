#!/bin/bash
# amsldoc phase profile on this head: cold format + cold doc + one-word
# edit, -j8, relaxed (headline) then exact; PHITEX_TEX_TIME/OBS timed.
set -u
D=/usr/share/doc/texlive/latex/amsmath
cargo build --release -q -p phitex-cli || exit 1
mkdir -p $w/bin && cp target/release/phitex $w/bin/phitex
for mode in relaxed exact; do
  echo "### $mode $(date +%T)"
  rm -rf $w/pcache
  if [ $mode = exact ]; then export TIKZMAN_EXACT=1; fi
  PHITEX_SNAPSHOT_SECS=10 python3 scripts/accl/tikzman-prof.py phases $w/bin/phitex $D amsldoc.tex amsldoc.tex 374 vertically bertically 8 ams-$mode
  grep -E 'measure:|time: |prelude settle|prelude:|round:|settle phases|edit phases|push_text|=====|no fixpoint|engine failed' $r/ams-$mode-stderr.txt | cut -c1-400 | head -40
done
echo "### oracle $(date +%T)"
mkdir -p $w/o && cp -a $D/. $w/o/ && (cd $w/o && for i in 1 2 3; do /usr/bin/time -f "pdflatex pass $i %e s %M kB" pdflatex -interaction=nonstopmode amsldoc.tex > /dev/null; done) 2>&1 | grep pass
which valgrind && echo "valgrind present"
echo "### done $(date +%T)"
