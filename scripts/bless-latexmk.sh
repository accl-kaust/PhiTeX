#!/bin/bash
# Bless the latexmk fixtures (oracle/fixtures/latexmk, see
# crates/engines/tex/tests/latex.rs `latexmk_*`): each NAME/ directory's
# sources built from nothing by TeX Live's `latexmk -pdf` (pdflatex,
# bibtex, biber, makeindex as latexmk runs them) under /tmp/tex with
# SOURCE_DATE_EPOCH=0, as the latex tests' PDF oracles are made. Writes
# NAME.pdf beside the directory and NAME.runs (the pdflatex runs latexmk
# made: the most rounds a cold build may take).
# Usage: scripts/bless-latexmk.sh [NAME...]   (run under nice -n 19 ionice -c 3)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
fx=$root/oracle/fixtures/latexmk
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
sel=("$@")
for d in "$fx"/*/; do
  d=${d%/}; name=$(basename "$d")
  if [ ${#sel[@]} -gt 0 ] && [[ ! " ${sel[*]} " =~ " $name " ]]; then continue; fi
  w=/tmp/tex/latexmk/$name
  rm -rf "$w"; mkdir -p "$w"
  cp "$d"/* "$w/"
  (cd "$w" && latexmk -pdf -interaction=batchmode "$name.tex" > latexmk.out 2>&1) || { echo "$name: latexmk failed"; exit 1; }
  cp "$w/$name.pdf" "$fx/$name.pdf"
  grep -c "^Running 'pdflatex" "$w/latexmk.out" > "$fx/$name.runs" || true
  echo "$name: $(cat "$fx/$name.runs") pdflatex runs, $(md5sum < "$fx/$name.pdf" | cut -c1-32)"
done
