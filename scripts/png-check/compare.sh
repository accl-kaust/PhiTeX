#!/usr/bin/env bash
# libpng 1.6 (the system's, which pdfTeX links) against partex's port
# (crates/partex-engine/src/png.rs), reading PNGs as pdfTeX's writepng.c
# does (harness.c and the png_dump example print the same report): run
# from the repository,
#
#   scripts/sandbox scripts/png-check/compare.sh WORK
#
# WORK/suite/*.png: PngSuite (PngSuite-2017jul19.tgz, unpacked), if there;
# the edge cases (gen_edge.py) are made in WORK/edge. Needs png.h and a C
# compiler. Prints IDENTICAL, or how many lines differ (WORK/out.diff).
set -eu
here=$(cd "$(dirname "$0")" && pwd)
w=${1:?usage: compare.sh WORK}
mkdir -p "$w"
cc -O2 -o "$w/harness" "$here/harness.c" -lpng
python3 "$here/gen_edge.py" "$w/edge"
CARGO_TARGET_DIR="$w/ct" cargo build -q -p partex-engine --example png_dump
files=$(ls "$w"/suite/*.png "$w"/edge/*.png 2>/dev/null)
"$w/harness" $files > "$w/out.libpng"
"$w/ct/debug/examples/png_dump" $files > "$w/out.port"
echo "files: $(grep -c '^== ' "$w/out.libpng"), images: $(grep -c '^image' "$w/out.libpng"), image errors: $(grep -c '^image.*error' "$w/out.libpng"), info errors: $(grep -c '^info error' "$w/out.libpng")"
if diff "$w/out.libpng" "$w/out.port" > "$w/out.diff"; then
  echo IDENTICAL
else
  echo "DIFFERENT: $(grep -c '^<' "$w/out.diff") lines (see $w/out.diff)"
  exit 1
fi
