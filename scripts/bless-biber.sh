#!/bin/sh
# Bless the native biber's oracle: for each oracle/fixtures/biber/*.tex,
# latex writes the .bcf, the installed biber writes the .bbl; both are
# committed (the .bcf is the test's input, the .bbl its expected output).
# Run through scripts/sandbox, under nice -n 19 ionice -c 3.
set -eu
root=$PWD
dir=$(cd "$(dirname "$0")/../oracle/fixtures/biber" && pwd)
work=$(mktemp -d "$root/target/bless-biber.XXXXXX")
cp "$dir"/*.tex "$dir"/*.bib "$work"
cd "$work"
for t in *.tex; do
  n=${t%.tex}
  latex -interaction=batchmode "$t" >/dev/null || true
  /usr/bin/vendor_perl/biber --quiet "$n" >/dev/null || echo "biber failed on $n" >&2
  cp "$n.bcf" "$n.bbl" "$dir"
done
cd "$root"
rm -rf "$work"
# The CLI's fixpoint oracle: numeric through latex, biber, latex, latex
# (the DVI file phitex build must give, its date masked by the test).
mkdir -p "$dir/fixpoint"
work=$(mktemp -d "$root/target/bless-biber.XXXXXX")
cp "$dir/numeric.tex" "$dir/refs.bib" "$work"
cd "$work"
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
latex -interaction=batchmode numeric.tex >/dev/null || true
/usr/bin/vendor_perl/biber --quiet numeric >/dev/null
latex -interaction=batchmode numeric.tex >/dev/null || true
latex -interaction=batchmode numeric.tex >/dev/null || true
cp numeric.dvi "$dir/fixpoint/numeric.dvi"
cmp numeric.bbl "$dir/numeric.bbl"
cd "$root"
rm -rf "$work"
