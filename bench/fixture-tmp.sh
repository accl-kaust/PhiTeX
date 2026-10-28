#!/usr/bin/env bash
# fixture-tmp.sh NAME...: (accl cmd, CI image, ACCL_STORE_IN=<gate-bin>/cli) each latexmk fixture blessed by pdflatex and built by the gate store's phitex at -j1 and -j8; PDFs and logs to $r/NAME
set -u; cd "$(dirname "$0")/.."
bin=$w/bin/cli/prof/phitex
for n in "$@"; do
  mkdir -p "$r/$n"
  bash scripts/bless-latexmk.sh "$n"
  cp /tmp/tex/latexmk/$n/$n.pdf "$r/$n/oracle.pdf"; cp /tmp/tex/latexmk/$n/$n.log "$r/$n/oracle.log"
  for j in 1 8; do
    d=/tmp/tex/ph/$n-j$j; rm -rf "$d"; mkdir -p "$d"; cp /tmp/tex/latexmk/$n/$n.tex "$d/"
    (cd "$d" && SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 timeout 300 "$bin" build -j$j --exact-pdf --no-synctex "$n.tex" > term.txt 2>&1; echo "exit $?" >> term.txt)
    cp "$d/$n.pdf" "$r/$n/j$j.pdf" 2>/dev/null; cp "$d/$n.log" "$r/$n/j$j.log" 2>/dev/null; cp "$d/term.txt" "$r/$n/j$j.term"
  done
  md5sum "$r/$n"/*.pdf
done
