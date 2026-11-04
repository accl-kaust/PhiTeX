#!/usr/bin/env bash
# bench/para40000.tex, the scale document: para4000's body ten times between
# its header and \bye (31.5 MB, so it is made here, not kept in git).
#
#   bench/para40000.sh [OUT]   # default bench/para40000.tex, next to this script
set -euo pipefail
src=$(dirname "$0")/para4000.tex
out=${1:-$(dirname "$0")/para40000.tex}
n=$(wc -l < "$src")
{
  head -n 1 "$src"
  for _ in 1 2 3 4 5 6 7 8 9 10; do sed -n "2,$((n - 1))p" "$src"; done
  tail -n 1 "$src"
} > "$out.tmp"
mv "$out.tmp" "$out"
