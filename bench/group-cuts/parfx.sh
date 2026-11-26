#!/bin/bash
# parfx.sh OUT ROUNDS BIN [ENV=V...] : six watch builds of deck2.tex at once
# at -j8, each a clean build then fixins.py's frame insert; one line per
# build with the two PDFs' md5s (clean 7930ee1ad4, edit ea27d8c13a).
# Each build has its own cache (CACHE_BASE-<i>, copied from CACHE_BASE).
set -u
OUT=$1; R=$2; BIN=$3; shift 3
H=$(cd "$(dirname "$0")" && pwd); C=${CACHE_BASE:-$HOME/.cache/phitex}; : > "$OUT"
for r in $(seq 1 "$R"); do
  for i in 1 2 3 4 5 6; do (
    W=${TMPDIR:-/tmp}/gcuts-$r-$i; rm -rf "$W"; mkdir -p "$W"; cp "$H/deck2.tex" "$W/deck.tex"
    python3 "$H/fixins.py" base "$W/deck.tex"
    [ -d "$C-$i" ] || cp -a "$C" "$C-$i"; rm -rf "$C-$i/docs"
    env "$@" PHITEX_CACHE_DIR="$C-$i" SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1 WARGS="--shelf --exact-pdf" \
      python3 "$H/wdrive.py" "$BIN" "$W" deck.tex 8 "$W/out" "python3 $H/fixins.py insert deck.tex" > /dev/null 2>&1
    echo "$r $i $(grep -o 'md5 [0-9a-f]*' "$W/out/table.txt" | tr '\n' ' ')" >> "$OUT") & done; wait
done
