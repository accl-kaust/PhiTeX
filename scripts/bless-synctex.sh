#!/bin/bash
# Bless the SyncTeX fixtures (oracle/fixtures/latex/synctex, see
# crates/engines/tex/tests/latex.rs `latex_synctex_*`): each NAME.tex
# built by TeX Live's `pdflatex -synctex=1` (twice, for its references)
# under /tmp/tex with SOURCE_DATE_EPOCH=0, as the latex tests' PDF oracles
# are made; writes NAME.pdf and NAME.answers (pdfTeX's SyncTeX answers,
# scripts/synctex-answers.py: needs `synctex` and poppler's `pdftotext`).
# Usage: scripts/bless-synctex.sh [NAME...]   (run under nice -n 19 ionice -c 3)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
fx=$root/oracle/fixtures/latex/synctex
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
sel=("$@")
for f in "$fx"/*.tex; do
  name=$(basename "$f" .tex)
  if [ ${#sel[@]} -gt 0 ] && [[ ! " ${sel[*]} " =~ " $name " ]]; then continue; fi
  w=/tmp/tex/synctex/$name
  rm -rf "$w"; mkdir -p "$w"
  cp "$f" "$w/"
  (cd "$w" && for k in 1 2; do pdflatex -synctex=1 -interaction=batchmode "$name.tex" > pdflatex.out 2>&1; done) \
    || { echo "$name: pdflatex failed"; exit 1; }
  cp "$w/$name.pdf" "$fx/$name.pdf"
  (cd "$w" && python3 "$root/scripts/synctex-answers.py" "$name") > "$fx/$name.answers"
  echo "$name: $(grep -c '^view' "$fx/$name.answers") view, $(grep -c '^edit' "$fx/$name.answers") edit, $(grep -c '^span' "$fx/$name.answers") span; pdf $(md5sum < "$fx/$name.pdf" | cut -c1-32)"
done
