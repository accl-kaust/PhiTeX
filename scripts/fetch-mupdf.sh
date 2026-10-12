#!/usr/bin/env bash
# Fetch MuPDF's WebAssembly build (mupdf on npm, AGPL-3.0-or-later) into
# upstream/mupdf/dist/: the files the live viewer draws its pages with,
# embedded in the phitex binary (crates/cli/src/view.rs). Pinned by version
# and the tarball's SHA-256; not committed (it is published minified).
# fetch-upstream.sh runs it; CI too, before building.
#
#   scripts/fetch-mupdf.sh     # through scripts/sandbox --net on a workstation
set -euo pipefail
cd "$(dirname "$0")/.."

version=1.28.1
sha256=e6bd485e9bfa4c618f02beea042fdcee0e6397e1a70cc8fb6baa55095b6d7601
files=(mupdf.js mupdf-wasm.js mupdf-wasm.wasm mupdf.d.ts mupdf-wasm.d.ts)
dir=upstream/mupdf
tgz="$dir/mupdf-$version.tgz"

mkdir -p "$dir/dist"
if [ ! -f "$tgz" ] || [ "$(sha256sum "$tgz" | cut -d' ' -f1)" != "$sha256" ]; then
  curl -fsSL --retry 3 "https://registry.npmjs.org/mupdf/-/mupdf-$version.tgz" -o "$tgz"
fi
[ "$(sha256sum "$tgz" | cut -d' ' -f1)" = "$sha256" ] || {
  echo "SHA-256 mismatch: $tgz" >&2
  exit 1
}
for f in "${files[@]}"; do
  tar -xzOf "$tgz" "package/dist/$f" >"$dir/dist/$f"
done
echo "mupdf @ $version ok"
