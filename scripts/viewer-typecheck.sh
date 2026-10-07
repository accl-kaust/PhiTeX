#!/usr/bin/env bash
# Type-check the renderer (viewer/src) and the CLI's host for it
# (crates/partex-cli/viewer/cli.ts) with viewer/tsconfig.json: strict,
# erasable syntax only, as the Overleaf extension, which runs these files
# in Node with the types stripped, needs them.
#
# tsc runs in scripts/sandbox, fetched once; PHITEX_BUNDLE_DIRECT=1 runs it
# as it is (a CI runner).
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
work="$repo/target/viewer-typecheck"
version=5.9.3
mkdir -p "$work"
if [ ! -x "$work/node_modules/.bin/tsc" ]; then
  if [ -n "${PHITEX_BUNDLE_DIRECT:-}" ]; then
    npm install --prefix "$work" --no-save --no-audit --no-fund "typescript@$version"
  else
    "$repo/scripts/sandbox" --net npm install --prefix "$work" --no-save --no-audit --no-fund "typescript@$version"
  fi
fi
if [ -n "${PHITEX_BUNDLE_DIRECT:-}" ]; then
  "$work/node_modules/.bin/tsc" -p "$repo/viewer/tsconfig.json"
else
  "$repo/scripts/sandbox" "$work/node_modules/.bin/tsc" -p "$repo/viewer/tsconfig.json"
fi
echo "viewer/src and cli.ts type-check (strict, erasable syntax only)"
