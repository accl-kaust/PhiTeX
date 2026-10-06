#!/usr/bin/env bash
# Build and upload Shelf to Cloudflare Pages (project "shelf-phitex", custom
# domain shelf.phitex.org). Run it sandboxed:
#   scripts/sandbox --net npx wrangler login   # once
#   scripts/sandbox --net ./deploy.sh
set -euo pipefail
cd "$(dirname "$0")"
label="${LABEL:-tl2026}"
# (RELEASES: the releases directory deployed, default releases/)
releases="${RELEASES:-releases}"
# (BUILD=1: build again first, into build/, with TRACE/TRACE_FMT (and
# TRACE_XETEX/TRACE_XETEX_FMT) for the dependency columns; else the release
# as it stands)
if [ -n "${BUILD:-}" ]; then python3 build.py "$label"; fi
# (packs by content, the release's index and release.json; the packs no
# kept release names dropped: release.py)
python3 release.py "$label" --releases "$releases" ${MIN_EXTENSION:+--min-extension "$MIN_EXTENSION"}
rm -rf "public/$label"
cp -r "$releases/$label" "public/$label"
n=$(find public -type f | wc -l)
if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (move old releases' packs to R2)" >&2; exit 1; fi
npx wrangler pages deploy public --project-name shelf-phitex --branch main
