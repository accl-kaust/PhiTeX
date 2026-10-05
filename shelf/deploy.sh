#!/usr/bin/env bash
# Build and upload Shelf to Cloudflare Pages (project "shelf-phitex", custom
# domain shelf.phitex.org). Run it sandboxed:
#   scripts/sandbox --net npx wrangler login   # once
#   scripts/sandbox --net ./deploy.sh
set -euo pipefail
cd "$(dirname "$0")"
label="${LABEL:-tl2026}"
# (BUILD=1: build public/ again first, with TRACE/TRACE_FMT for the
# dependency column; else what public/ holds, built and checked before)
if [ -n "${BUILD:-}" ]; then python3 build.py "$label"; fi
# (packs by content, the release's index and release.json: every released
# pack, of every release, goes up with each deploy; release.py)
python3 release.py "$label" ${MIN_EXTENSION:+--min-extension "$MIN_EXTENSION"}
cp -r "releases/$label/." "public/$label/"
# (the build's own names, p/: no released extension reads them; the
# content-named packs under h/ are the only copy deployed)
rm -rf "public/$label/p"
n=$(find public -type f | wc -l)
if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (move old releases' packs to R2)" >&2; exit 1; fi
npx wrangler pages deploy public --project-name shelf-phitex --branch main
