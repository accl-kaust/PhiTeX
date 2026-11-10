#!/bin/bash
# (temporary) amsldoc with boxscope on: where its lists are left, and
# the PDF against the flag off.
set -u
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-prof cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-prof/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
for m in off box; do
  e=(); [ $m = off ] || e=(PHITEX_BOXSCOPE=1 PHITEX_BOXSCOPE_TRACE=1)
  rm -rf "$w/ams-$m"; cp -a bench/amsldoc "$w/ams-$m"
  ( cd "$w/ams-$m" && env PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" "$bin" build amsldoc.tex --exact-pdf -j 1 > "$r/ams-$m.txt" 2>&1; echo "$m exit $? $(md5sum < amsldoc.pdf)"; cp amsldoc.pdf "$r/ams-$m.pdf" )
done
echo "== leaves, closes, paragraphs (box)"
grep "^boxscope: node .* \(leaves\|closes\|ends the paragraph\|owns the paragraph\)" "$r/ams-box.txt" | cut -c1-200 | head -60
