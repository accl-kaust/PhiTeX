#!/bin/bash
# The fixtures of tests/gftopk.rs, made by TeX Live's own mf and gftopk
# (run on accl, in the image: scripts/accl/accl submit cmd bash
# crates/fonts/gftopk/tests/bless.sh): each font's GF file as mktexpk has
# mf make it (\mode:=MODE; mag:=MAG; nonstopmode; input NAME), the date
# fixed, and gftopk's PK of it; into $r/fixtures (then copied by hand to
# tests/fixtures).
set -eu
out=${r:?}/fixtures
mkdir -p "$out"
tmp=$(mktemp -d)
cd "$tmp"
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1
mf --version | head -1 > "$out/versions.txt"
gftopk --version | head -1 >> "$out/versions.txt"
# NAME MODE MAG
while read -r name mode mag; do
  mf "\\mode:=$mode; mag:=$mag; nonstopmode; input $name" > "$name.mflog" 2>&1 < /dev/null || true
  gf=$(ls "$name".*gf | head -1)
  dpi=${gf#"$name".}
  dpi=${dpi%gf}
  gftopk "./$gf" "$name.${dpi}pk"
  cp "$gf" "$name.${dpi}pk" "$out/"
  [ -f "$name.tfm" ] && cp "$name.tfm" "$out/"
  echo "$name $mode $mag $dpi $(md5sum < "$gf" | cut -c1-32) $(md5sum < "$name.${dpi}pk" | cut -c1-32) $(stat -c %s "$gf" "$name.${dpi}pk" | tr '\n' ' ')" >> "$out/list.txt"
  rm -f "$name".*
done <<'LIST'
ghab ljfour 1+0/600
cmr10 ljfour 1+0/600
cmti10 ljfour 1+0/600
logo10 ljfour 1+0/600
cminch ljfour 1+0/600
cmr5 ljfour 0.1
cmtt8 ljfour 0.2
LIST
cat "$out/list.txt"
