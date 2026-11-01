#!/bin/bash
# Bless the correctness ladder's oracle (oracle/fixtures/ladder, see
# crates/cli/tests/ladder.rs): each rung's NAME.tex and its edits
# (edit-word/, edit-para/) through the installed pdflatex (pdftex for the
# plain rung), bibtex, makeindex and biber, run until the files read back
# settle, with SOURCE_DATE_EPOCH=0 under /tmp/tex. Writes <variant>/oracle/:
# the PDF, the files read back, and stats (rounds, wall seconds, pages).
# Usage: scripts/bless-ladder.sh [NN...]   (run under nice -n 19 ionice -c 3)
set -eu
root=$(cd "$(dirname "$0")/.." && pwd)
lad=$root/oracle/fixtures/ladder
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
sel=("$@")
for rd in "$lad"/[0-9][0-9]-*/; do
  rd=${rd%/}; base=$(basename "$rd"); nn=${base%%-*}; name=${base#*-}
  if [ ${#sel[@]} -gt 0 ] && [[ ! " ${sel[*]} " =~ " $nn " ]]; then continue; fi
  eng=pdflatex; [ "$name" = plain ] && eng=pdftex
  for v in . edit-word edit-para; do
    w=/tmp/tex/ladder/$base/${v/#./cold}
    rm -rf "$w"; mkdir -p "$w"
    find "$rd" -maxdepth 1 -type f ! -name '*.tex' -exec cp {} "$w" \;
    cp "$rd/$v/$name.tex" "$w/"
    cd "$w"
    t0=$(date +%s.%N); rounds=0; prev=
    while [ $rounds -lt 8 ]; do
      rounds=$((rounds+1))
      $eng -interaction=batchmode "$name.tex" >/dev/null || true
      [ -f "$name.aux" ] && grep -q '\\bibdata' "$name.aux" && bibtex -terse "$name" >/dev/null || true
      [ -f "$name.idx" ] && makeindex -q "$name" || true
      [ -f "$name.bcf" ] && /usr/bin/vendor_perl/biber --quiet "$name" >/dev/null || true
      cur=$(cat "$name".{aux,toc,lof,lot,bbl,ind,out} 2>/dev/null | md5sum)
      [ "$cur" = "$prev" ] && break
      prev=$cur
    done
    t1=$(date +%s.%N)
    o=$rd/$v/oracle; rm -rf "$o"; mkdir -p "$o"
    for e in pdf aux toc lof lot bbl ind out; do [ -f "$name.$e" ] && cp "$name.$e" "$o/"; done
    pages=$(grep -c '/Type */Page[^s]' "$name.pdf" 2>/dev/null || echo 0)
    pages=$(sed -n 's/^Output written on .*(\([0-9]*\) page.*/\1/p' "$name.log")
    errs=$(grep -c '^! ' "$name.log" || true)
    printf 'rounds %s\nwall_s %.2f\npages %s\nerrors %s\n' "$rounds" "$(echo "$t1 - $t0" | bc)" "$pages" "$errs" > "$o/stats"
    echo "$base/$v: $(tr '\n' ' ' < "$o/stats")"
    cd "$root"
  done
done
