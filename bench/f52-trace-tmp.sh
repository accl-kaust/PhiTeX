#!/bin/bash
# (temporary) one small file with boxscope on, every command of the
# document's nodes traced (observe build).   bench/f52-trace-tmp.sh bench/f52-one/h14.tex
set -u
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-obs cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile,phitex-tex/observe > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-obs/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
t=$w/phitex/$1; n=$(basename "$t" .tex)
for m in off box; do
  e=(PHITEX_BOXSCOPE=0); [ $m = off ] || e=(PHITEX_BOXSCOPE=1 PHITEX_BOXSCOPE_TRACE=1 PHITEX_CLOSE_TRACE=1 PHITEX_CUT_TRACE=1 PHITEX_CMD_TRACE=${2:-all} PHITEX_GIVEBACK_TRACE=1)
  d=$w/d-$m; rm -rf "$d"; mkdir -p "$d"; cp "$t" "$d/"
  ( cd "$d" && env PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" "$bin" build "$n.tex" --exact-pdf --engine pdflatex -j 1 > "$r/$m.txt" 2>&1; echo "$m exit $? $(md5sum < "$n.pdf")" )
done
ids=$(echo "${2:-}" | tr : "|"); [ -n "$ids" ] || ids=$(grep -oE "^(boxscope: node|close-trace: node) [0-9]+" "$r/box.txt" | awk "{print \$3}" | sort -un | tr "\n" "|" | sed "s/|$//")
echo "nodes: $ids"
grep -E "^(cmd-trace ($ids) |boxscope|close-trace|cut: node ($ids) |giveback)" "$r/box.txt" | grep -v 'stays: the innermost' | cut -c1-240 > "$r/focus.txt"
wc -l "$r/focus.txt"; head -400 "$r/focus.txt"
# (FIND=pattern as $3: the nodes whose commands match it, all their commands)
if [ -n "${3:-}" ]; then
  fids=$(grep -E "^cmd-trace [0-9]+ .*$3" "$r/box.txt" | awk '{print $2}' | sort -un | tr '\n' '|' | sed 's/|$//')
  echo "found nodes: $fids"
  grep -E "^cmd-trace ($fids) " "$r/box.txt" | cut -c1-200 > "$r/found.txt"; wc -l "$r/found.txt"
  rm -f "$r/box.txt" "$r/focus.txt"
fi
