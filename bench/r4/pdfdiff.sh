#!/bin/bash
# r4-latexmk (accl cmd task): fixture FIX (oracle/fixtures/latexmk/FIX)
# built by `phitex build` and by latexmk -pdf; both directories' files and
# the PDFs' QDF forms compared (the first differences).
set -u
export CARGO_BUILD_JOBS=16 SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
f=""; for x in ${FEATURES:-}; do f="$f --features phitex-tex/$x"; done
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli $f > "$r/build.txt" 2>&1 || { tail -20 $r/build.txt > $r/summary.txt; exit 1; }
for side in ph tl; do rm -rf $w/$side; mkdir -p $w/$side; cp -a oracle/fixtures/latexmk/$FIX/. $w/$side/; done
(cd $w/ph && PHITEX_CACHE_DIR=$w/cache env ${PHITEX_ENV:-} timeout 900 $w/t/release/phitex build $FIX.tex > $r/ph-term.txt 2>&1)
(cd $w/tl && timeout 900 latexmk -pdf -interaction=batchmode $FIX.tex > $r/tl-term.txt 2>&1)
for side in ph tl; do mkdir -p $r/$side; cp $w/$side/$FIX.* $r/$side/ 2>/dev/null; qpdf --qdf --object-streams=disable $w/$side/$FIX.pdf $r/$side.qdf 2>/dev/null; done
{
  echo "pdf $(md5sum < $w/ph/$FIX.pdf | cut -c1-8) vs $(md5sum < $w/tl/$FIX.pdf | cut -c1-8)"
  for x in $(cd $w/tl && ls $FIX.* | grep -v '\.pdf$\|\.log$\|\.fls$\|fdb_latexmk$\|\.blg$'); do cmp -s $w/tl/$x $w/ph/$x && echo "$x same" || echo "$x DIFF"; done
  diff -a $r/tl.qdf $r/ph.qdf | head -60
} > $r/summary.txt
