#!/bin/bash
# The TeX Live oracle for bench/errctx: each case's .log into $1/<case>.log
out=${1:-.}; mkdir -p "$out"
here=$(cd "$(dirname "$0")" && pwd)
pdflatex --version | head -1
for d in "$here"/*/; do
  c=$(basename "$d"); w=$(mktemp -d); cp -r "$d"/. "$w"/
  if grep -q documentclass "$w/main.tex"; then e=pdflatex; else e=pdftex; fi
  (cd "$w" && $e -interaction=nonstopmode main.tex > /dev/null 2>&1; cp main.log "$out/$c.log")
  rm -rf "$w"
done
ls "$out"
