#!/usr/bin/env bash
# Bundle the live viewer's script, crates/cli/viewer/viewer.js, and a
# folder watch's index page's, crates/cli/viewer/dir.js
# (DESIGN 4.8): the renderer (viewer/src: viewer.ts, page2.ts, sync.ts,
# css.ts; the Overleaf extension draws its pages with the same files, taken
# from the PhiTeX commit it pins) and the CLI's host for it
# (crates/cli/viewer/cli.ts), not minified. The bundle is committed (the
# build needs no node); CI checks it is what these sources make. The pages
# are drawn by the watch as strips (crates/output/render); the page loads
# no PDF renderer.
#
#   scripts/viewer-bundle.sh            # make viewer.js
#   scripts/viewer-bundle.sh --check    # fail if viewer.js is not what they make
#
# esbuild runs in scripts/sandbox, fetched once; PHITEX_BUNDLE_DIRECT=1 runs
# it as it is (a CI runner, itself a throwaway machine).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$repo/target/viewer-bundle"
out="$repo/crates/cli/viewer/viewer.js"
check=false
[ "${1:-}" = "--check" ] && check=true
mkdir -p "$work"
version=0.28.2

run() {
  if [ -n "${PHITEX_BUNDLE_DIRECT:-}" ]; then "$@"; else "$repo/scripts/sandbox" "$@"; fi
}
if [ ! -x "$work/node_modules/.bin/esbuild" ]; then
  if [ -n "${PHITEX_BUNDLE_DIRECT:-}" ]; then
    npm install --prefix "$work" --no-save --no-audit --no-fund "esbuild@$version"
  else
    "$repo/scripts/sandbox" --net npm install --prefix "$work" --no-save --no-audit --no-fund "esbuild@$version"
  fi
fi
# (the viewer, and a folder watch's index page: crates/cli/viewer/dir.ts)
bundle() {
  local src="$1" dest="$2" banner="$3"
  local target="$dest"
  $check && target="$work/$(basename "$dest")"
  run "$work/node_modules/.bin/esbuild" "$src" \
    --bundle --format=esm --target=es2022 --legal-comments=none --log-level=warning \
    --banner:js="$banner" \
    --outfile="$target"
  if $check; then
    cmp -s "$target" "$dest" || {
      echo "${dest#"$repo"/} is not what its sources make: run scripts/viewer-bundle.sh and commit it" >&2
      exit 1
    }
    echo "$(basename "$dest") is up to date"
  else
    ls -l "$dest"
  fi
}
bundle "$repo/crates/cli/viewer/cli.ts" "$out" \
  "// phitex's live viewer: viewer/src and crates/cli/viewer/cli.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)"
bundle "$repo/crates/cli/viewer/dir.ts" "$repo/crates/cli/viewer/dir.js" \
  "// phitex watch DIR's index page: viewer/src and crates/cli/viewer/dir.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)"
