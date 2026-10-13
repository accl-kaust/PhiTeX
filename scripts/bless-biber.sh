#!/bin/sh
# Bless the native biber's oracle: for each oracle/fixtures/biber/*.tex,
# latex writes the .bcf, the installed biber writes the .bbl; both are
# committed (the .bcf is the test's input, the .bbl its expected output).
# Run through scripts/sandbox, under nice -n 19 ionice -c 3.
set -eu
dir=$(cd "$(dirname "$0")/../oracle/fixtures/biber" && pwd)
work=$(mktemp -d "$PWD/target/bless-biber.XXXXXX")
cp "$dir"/*.tex "$dir"/*.bib "$work"
cd "$work"
for t in *.tex; do
  n=${t%.tex}
  latex -interaction=batchmode "$t" >/dev/null || true
  /usr/bin/vendor_perl/biber --quiet "$n" >/dev/null || echo "biber failed on $n" >&2
  cp "$n.bcf" "$n.bbl" "$dir"
done
rm -rf "$work"
