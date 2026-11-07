#!/bin/bash
# A Shelf release made and checked in the CI image (TeX Live, Rust,
# Python), for .github/workflows/shelf.yml: the releases the host serves
# (scripts/fetch-live.py), TeX Live's archives (build.py FETCH_ONLY, the
# fonts' tree), PhiTeX, the tracer and the font index built from this
# checkout, build.py with the dependency traces, release.py after the
# served releases, the check against TeX Live (scripts/check.sh), the
# formats made there published with the release and checked to be taken
# (release.py --formats, scripts/check-formats.sh), and the site to deploy (public/ with the releases, as deploy.sh makes it).
# Writes out/shelf-results/ (summary.txt, release.txt: the release made,
# or nothing new) and leaves the site in shelf/public/.
#
#   shelf/scripts/release.sh            # from the checkout's root
#
# LABEL (default tl2026), SHELF_HOST (default https://v1.shelf.phitex.org).
#
# FORMATS_COMMIT=<PhiTeX commit>: no new release, no TeX Live fetched; the
# formats of that commit's PhiTeX (its engine identity) added to the
# release the host serves now. That commit's phitex is built, checked
# against TeX Live on the served release (scripts/check.sh, TeX Live's
# engines over the release's own files, its fonts among them; the maps'
# check skipped, made when the release was), the formats its builds made
# published with it (release.py --formats: the earlier identities' kept,
# each pack under 20 MB) and each checked to be taken
# (scripts/check-formats.sh); release.txt names the served release and
# formats.txt the formats added.
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

# (formats only: FORMATS_COMMIT's PhiTeX, its formats on the served release)
if [ -n "${FORMATS_COMMIT:-}" ]; then
  at=$w/at
  rm -rf "$at" && mkdir -p "$at"
  git -C "$root" -c safe.directory='*' archive "$FORMATS_COMMIT" | tar -x -C "$at"
  (cd "$at" && CARGO_TARGET_DIR=$w/at-target cargo build -q --profile fastdev -p phitex-cli --bin phitex)
  bin=$w/at-target/fastdev/phitex
  echo "PhiTeX $FORMATS_COMMIT built $(date +%T)" | tee -a "$out/summary.txt"
  rm -rf "$w/check"
  ROOT="$root" REL="$s/releases-v1" LABEL="$label" NEW="$old" BIN="$bin" \
    FTREE="$w/check/tree/texmf-dist" W="$w/check" OUT="$out" TLPDB= bash scripts/check.sh
  before=$(python3 -c "import json,sys;print(' '.join(json.load(open(sys.argv[1])).get('formats') or {}))" "releases-v1/$label/release.json")
  python3 release.py "$label" --releases releases-v1 --formats "$w/check/c-shelf/formats" | tee -a "$out/summary.txt"
  python3 - "releases-v1/$label/release.json" "$before" > "$out/formats.txt" <<'NEW'
import json, sys
have = json.load(open(sys.argv[1])).get("formats") or {}
print(" ".join(sorted(set(have) - set(sys.argv[2].split()))))
NEW
  [ -n "$(tr -d ' \n' < "$out/formats.txt")" ] || { echo "$FORMATS_COMMIT: no format new to $old (its identity's formats are served)" | tee -a "$out/summary.txt"; : > "$out/release.txt"; exit 0; }
  ROOT="$root" REL="$s/releases-v1" LABEL="$label" NEW="$old" BIN="$bin" ONLY="$(cat "$out/formats.txt")" \
    W="$w/check" OUT="$out" bash scripts/check-formats.sh
  rm -rf "public/$label"
  cp -r "releases-v1/$label" "public/$label"
  n=$(find public -type f | wc -l)
  if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (README: Retention)" >&2; exit 1; fi
  echo "$old" > "$out/release.txt"
  echo "site: $old with the formats of $FORMATS_COMMIT ($(cat "$out/formats.txt")), $n files, $(du -sh public | cut -f1)" | tee -a "$out/summary.txt"
  exit 0
fi

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
  FTREE="$w/ftree" W="$w/check" OUT="$out" TLPDB="$s/cache/texlive.tlpdb.xz" bash scripts/check.sh

# (6b) the formats the check's builds made on the release (this PhiTeX's
# engine identity) published with it, and each taken by a build on a
# fresh cache, not made (scripts/check-formats.sh)
python3 release.py "$label" --releases releases-v1 --formats "$w/check/c-shelf/formats" | tee -a "$out/summary.txt"
ROOT="$root" REL="$s/releases-v1" LABEL="$label" NEW="$new" BIN="$root/target/fastdev/phitex" \
  W="$w/check" OUT="$out" bash scripts/check-formats.sh

# (7) the site, as deploy.sh makes it
rm -rf "public/$label"
cp -r "releases-v1/$label" "public/$label"
n=$(find public -type f | wc -l)
if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (README: Retention)" >&2; exit 1; fi
echo "$new" > "$out/release.txt"
echo "site: $new, $n files, $(du -sh public | cut -f1)" | tee -a "$out/summary.txt"
