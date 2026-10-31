#!/bin/bash
# A Shelf release made and checked in the CI image (TeX Live, Rust,
# Python), for .github/workflows/shelf.yml: the releases the host serves
# (scripts/fetch-live.py), TeX Live's archives (build.py FETCH_ONLY, the
# fonts' tree), PhiTeX, the tracer and the font index built from this
# checkout, build.py with the dependency traces, release.py after the
# served releases, the check against TeX Live (scripts/check.sh), and the
# site to deploy (public/ with the releases, as deploy.sh makes it).
# Writes out/shelf-results/ (summary.txt, release.txt: the release made,
# or nothing new) and leaves the site in shelf/public/.
#
#   shelf/scripts/release.sh            # from the checkout's root
#
# LABEL (default tl2026), SHELF_HOST (default https://v1.shelf.phitex.org).
set -euo pipefail
root=$PWD s=$PWD/shelf w=$PWD/out/shelf out=$PWD/out/shelf-results
label=${LABEL:-tl2026} host=${SHELF_HOST:-https://v1.shelf.phitex.org}
jobs=$(nproc)
mkdir -p "$w/ftree" "$out"
cd "$s"
json() { python3 -c "import json,sys;print(json.load(open(sys.argv[1])).get(sys.argv[2]) or '')" "$@"; }

# (1) what the host serves now: the new release is numbered after it, and
# the deployment keeps it (a Pages deployment replaces the whole site)
python3 scripts/fetch-live.py "$host" "$label" releases-v1
old=$(json "releases-v1/$label/release.json" release)
min=$(json "releases-v1/$label/release.json" min_extension)
echo "served: $old (min_extension $min) $(date +%T)"

# (2) TeX Live's package database and archives, and the fonts' tree
FETCH_ONLY=1 FONT_TREE="$w/ftree" python3 build.py "$label"
echo "fetched $(date +%T)"

# (3) PhiTeX and the font index from this checkout, the tracer on it
(cd "$root" && cargo build -q --profile fastdev -p phitex-cli --bin phitex -p phitex-otf --example otf-index)
cargo build -q --release --manifest-path trace/Cargo.toml
cat > "$root/scripts/xetex/fonts.conf" <<CONF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$w/ftree/fonts/opentype</dir>
  <dir>$w/ftree/fonts/truetype</dir>
  <cachedir>$w/fc-cache</cachedir>
</fontconfig>
CONF
"$root/target/fastdev/examples/otf-index" "$w/fontindex.pxfi"
"$root/target/fastdev/examples/otf-index" "$w/fontindex-rel.pxfi" --relative "$w/ftree"
echo "PhiTeX, the tracer and the font index $(date +%T)"

# (4) the build, traced on both engines
TLPDB=cache/texlive.tlpdb.xz TRACE=trace/target/release/shelf-trace TRACE_FONT_INDEX="$w/fontindex.pxfi" \
  FONT_INDEX="$w/fontindex-rel.pxfi" TRACE_JOBS="$jobs" PACK_JOBS="$jobs" \
  python3 build.py "$label" > "$out/build.log" 2>&1 || { tail -40 "$out/build.log"; exit 1; }
tail -8 "$out/build.log"

# (5) the release, after the served ones
python3 release.py "$label" --releases releases-v1 --min-extension "$min" | tee -a "$out/summary.txt"
new=$(json "releases-v1/$label/release.json" release)
if [ "$new" = "$old" ]; then
  echo "$label: nothing new to release (the host serves $old)" | tee -a "$out/summary.txt"
  : > "$out/release.txt"
  exit 0
fi

# (6) the check against TeX Live
ROOT="$root" REL="$s/releases-v1" LABEL="$label" NEW="$new" BIN="$root/target/fastdev/phitex" \
  FTREE="$w/ftree" W="$w/check" OUT="$out" bash scripts/check.sh

# (7) the site, as deploy.sh makes it
rm -rf "public/$label"
cp -r "releases-v1/$label" "public/$label"
n=$(find public -type f | wc -l)
if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (README: Retention)" >&2; exit 1; fi
echo "$new" > "$out/release.txt"
echo "site: $new, $n files, $(du -sh public | cut -f1)" | tee -a "$out/summary.txt"
