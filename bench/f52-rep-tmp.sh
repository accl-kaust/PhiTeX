#!/bin/bash
# (temporary) the small repros in bench/f52-rep/ with boxscope off and on:
# md5s, the first difference, where lists are left.
set -u
cd "$w/phitex"
mkdir -p "$w/b" "$r"; bin=$w/b/phitex
CARGO_TARGET_DIR=$w/target-prof cargo build -q --release -p phitex-cli \
  --features phitex-tex/profile,phitex-tex/observe > "$r/build.txt" 2>&1 || { tail -30 "$r/build.txt"; exit 1; }
cp "$w/target-prof/release/phitex" "$bin"
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
for t in "$w"/phitex/${1:-bench/f52-rep}/*.tex; do
  n=$(basename "$t" .tex)
  echo "=================== $n"
  for m in off box; do
    e=(PHITEX_BOXSCOPE=0); [ $m = off ] || e=(PHITEX_BOXSCOPE=1 PHITEX_BOXSCOPE_TRACE=1 PHITEX_CLOSE_TRACE=1)
    d=$w/d-$n-$m; rm -rf "$d"; mkdir -p "$d"; cp "$t" "$d/"
    ( cd "$d" && env PHITEX_CACHE_DIR=$w/cache-$m "${e[@]}" "$bin" build "$n.tex" --exact-pdf -j "${J:-1}" > "$r/$n-$m.txt" 2>&1; echo "$m exit $? $(md5sum < "$n.pdf")"; cp "$n.pdf" "$r/$n-$m.pdf"; /usr/bin/grep -o '\[tpd=[^]]*\]' "$n.log" | head -3 )
  done
  python3 -I "$w/phitex/bench/f52-pdfdiff-tmp.py" "$r/$n-off.pdf" "$r/$n-box.pdf"
  grep "^boxscope: node .* \(leaves\|closes\|ends the paragraph\)\|^close-trace" "$r/$n-box.txt" | cut -c1-300 | head -20
done
