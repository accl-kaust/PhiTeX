#!/usr/bin/env bash
# Bundle the live viewer's script, crates/partex-cli/viewer/viewer.js
# (DESIGN 4.8): the renderer (viewer/src: viewer.ts, page2.ts, sync.ts,
# css.ts; the Overleaf extension draws its pages with the same files, taken
# from the PhiTeX commit it pins) and the CLI's host for it
# (crates/partex-cli/viewer/cli.ts). The bundle is committed (the build
# needs no node); CI checks it is what these sources make.
#
#   scripts/viewer-bundle.sh            # make viewer.js
#   scripts/viewer-bundle.sh --check    # fail if viewer.js is not what they make
#
# esbuild runs in scripts/sandbox, fetched once; PHITEX_BUNDLE_DIRECT=1 runs
# it as it is (a CI runner, itself a throwaway machine).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$repo/target/viewer-bundle"
out="$repo/crates/partex-cli/viewer/viewer.js"
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
target="$out"
$check && target="$work/viewer.js"
run "$work/node_modules/.bin/esbuild" "$repo/crates/partex-cli/viewer/cli.ts" \
  --bundle --format=esm --target=es2022 --minify --legal-comments=none --log-level=warning \
  --banner:js="// phitex's live viewer: viewer/src and crates/partex-cli/viewer/cli.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)" \
  --outfile="$target"
if $check; then
  cmp -s "$target" "$out" || {
    echo "crates/partex-cli/viewer/viewer.js is not what viewer/src and cli.ts make: run scripts/viewer-bundle.sh and commit it" >&2
    exit 1
  }
  echo "viewer.js is up to date"
else
  ls -l "$out"
fi
