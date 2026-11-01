#!/bin/bash
# r4-latexmk (accl cmd task): rounds of phitex build vs latexmk's pdflatex
# runs, cold and rebuilt with the written files on disk. Env: FEATURES
# (phitex-tex cargo features), CASES (default: all), PHITEX_ENV (extra
# environment for phitex runs).
set -u
export CARGO_BUILD_JOBS=16
f=""; for x in ${FEATURES:-}; do f="$f --features phitex-tex/$x"; done
CARGO_TARGET_DIR=$w/t cargo build -q --release -p phitex-cli $f > "$r/build.txt" 2>&1; echo "EXIT $?" >> "$r/build.txt"
grep -q '^EXIT 0$' "$r/build.txt" || { echo "build failed: $(grep -m3 '^error' -A5 "$r/build.txt")" | tee "$r/summary.txt"; exit 1; }
P=$w/t/release/phitex
S=$(pwd)
mkdir -p $w/run
printf '%s\n' '\documentclass{article}' '\begin{document}' 'Hello, world.' '\end{document}' > $w/hello.tex
printf '%s\n' '\documentclass{article}' '\begin{document}' '\section{One}\label{one}See section~\ref{one} on page~\pageref{one}.' '\end{document}' > $w/label.tex
cp bench/amsldoc/amsldoc.tex $w/amsldoc.tex
# ph TAG DIR DOC J CACHE: one phitex build, its rounds and md5
ph() {
  local tag=$1 D=$2 doc=$3 j=$4 c=$5 stem=${3%.tex}
  ( cd "$D" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 PHITEX_TEX_TIME=1 PHITEX_CACHE_DIR=$c ${PHITEX_ENV:-} timeout 900 "$P" build "$doc" --exact-pdf -j "$j" > "$r/$tag.log" 2>&1; echo "exit $?" >> "$r/$tag.log" )
  mkdir -p "$r/$tag.files"; cp "$D"/$stem.aux "$D"/$stem.toc "$D"/$stem.out "$D"/$stem.pdf "$r/$tag.files/" 2>/dev/null
  echo "$tag: $(tail -1 "$r/$tag.log") rounds $(grep -aoE "[0-9]+ loop rounds" "$r/$tag.log" | tail -1) md5 $(md5sum < "$D/$stem.pdf" | cut -c1-8)" | tee -a "$r/summary.txt"
}
# lm TAG DIR DOC: latexmk -pdf, its pdflatex runs
lm() {
  local tag=$1 D=$2 doc=$3 stem=${3%.tex}
  ( cd "$D" && env SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 timeout 900 latexmk -pdf -interaction=batchmode "$doc" > "$r/$tag.log" 2>&1; echo "exit $?" >> "$r/$tag.log" )
  mkdir -p "$r/$tag.files"; cp "$D"/$stem.aux "$D"/$stem.toc "$D"/$stem.out "$D"/$stem.pdf "$r/$tag.files/" 2>/dev/null
  echo "$tag: $(tail -1 "$r/$tag.log") pdflatex runs $(grep -c 'Running .pdflatex' "$r/$tag.log") md5 $(md5sum < "$D/$stem.pdf" | cut -c1-8)" | tee -a "$r/summary.txt"
}
for doc in ${CASES:-hello label amsldoc}; do
  D=$w/run/$doc-ph; mkdir -p $D; cp $w/$doc.tex $D/
  ph $doc-cold-j1 $D $doc.tex 1 $w/c-$doc-a
  ph $doc-rebuild-nocache-j1 $D $doc.tex 1 $w/c-$doc-b
  ph $doc-rebuild-cache-j1 $D $doc.tex 1 $w/c-$doc-a
  D2=$w/run/$doc-ph8; mkdir -p $D2; cp $w/$doc.tex $D2/
  ph $doc-cold-j8 $D2 $doc.tex 8 $w/c-$doc-c
  L=$w/run/$doc-lm; mkdir -p $L; cp $w/$doc.tex $L/
  lm $doc-lm-cold $L $doc.tex
  rm -f $L/*.fdb_latexmk $L/*.pdf
  lm $doc-lm-rebuild $L $doc.tex
done
