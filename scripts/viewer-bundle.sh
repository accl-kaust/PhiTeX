#!/usr/bin/env bash
# Bundle the live viewer's script, crates/partex-cli/viewer/viewer.js
# (DESIGN 4.8): the Overleaf extension's viewer (extension/src/viewer.ts,
# page2.ts) at the pinned tag below, and the CLI's host for it
# (crates/partex-cli/viewer/cli.ts). The extension's code is not copied
# into this repository: it is taken from its repository at the tag, and the
# bundle is committed (the build needs neither node nor that repository).
# Run this when the pin or cli.ts changes, and commit viewer.js.
#
#   PHITEX_OVERLEAF=path/to/phitex-overleaf scripts/viewer-bundle.sh
set -euo pipefail

# (the extension's release whose viewer this is)
EXT_REV=v0.2.90-beta1

repo="$(cd "$(dirname "$0")/.." && pwd)"
ext="${PHITEX_OVERLEAF:-$HOME/code/flinner/phitex-overleaf}"
work="$repo/target/viewer-bundle"
mkdir -p "$work"
rm -rf "$work/ext"
mkdir -p "$work/ext"
commit=$(git -C "$ext" rev-parse --short "$EXT_REV^{commit}")
git -C "$ext" archive "$EXT_REV" extension/src/viewer.ts extension/src/page2.ts extension/src/session.ts |
  tar -x -C "$work/ext" --strip-components=2
cp "$repo/crates/partex-cli/viewer/cli.ts" "$work/cli.ts"
# (esbuild at the extension's version, fetched once)
if [ ! -x "$work/node_modules/.bin/esbuild" ]; then
  "$repo/scripts/sandbox" --net npm install --prefix "$work" --no-save --no-audit --no-fund esbuild@0.28.2
fi
"$repo/scripts/sandbox" "$work/node_modules/.bin/esbuild" "$work/cli.ts" \
  --bundle --format=esm --target=es2022 --minify --legal-comments=none \
  --banner:js="// partex's live viewer: phitex-overleaf $EXT_REV ($commit) extension/src/viewer.ts and page2.ts, with crates/partex-cli/viewer/cli.ts; made by scripts/viewer-bundle.sh (AGPL-3.0-only)" \
  --outfile="$repo/crates/partex-cli/viewer/viewer.js"
ls -l "$repo/crates/partex-cli/viewer/viewer.js"
