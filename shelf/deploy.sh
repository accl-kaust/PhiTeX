#!/usr/bin/env bash
# Build and upload Shelf to Cloudflare Pages. Run it sandboxed:
#   scripts/sandbox --net npx wrangler login   # once
#   scripts/sandbox --net ./deploy.sh
#
# Each public protocol is a Pages project of its own with a host of its
# own: schema 1 is PROJECT (default shelf-phitex-v1) at v1.shelf.phitex.org;
# a schema older clients can't read gets the next (shelf-phitex-v2,
# v2.shelf.phitex.org) beside it. The development sites (shelf-phitex at
# shelf.phitex.org, shelf-phitex-v4) were never released. A Pages
# deployment replaces its project's whole site, so this script refuses
# those projects: a mistake would take their files away.
set -euo pipefail
cd "$(dirname "$0")"
label="${LABEL:-tl2026}"
project="${PROJECT:-shelf-phitex-v1}"
# (RELEASES: the releases directory deployed, default releases-v1/)
releases="${RELEASES:-releases-v1}"
if [ "$project" = shelf-phitex ] || [ "$project" = shelf-phitex-v4 ]; then
  echo "$project is a development site, kept as it is: schema 1 goes to shelf-phitex-v1 (PROJECT)" >&2
  exit 1
fi
# (BUILD=1: build again first, into build/, with TRACE and
# TRACE_FONT_INDEX for the dependency columns, or the kept traces; else
# the release as it stands)
if [ -n "${BUILD:-}" ]; then python3 build.py "$label"; fi
# (packs by content, the release's index, pack table and release.json;
# the packs no kept release names dropped: release.py)
python3 release.py "$label" --releases "$releases" ${MIN_EXTENSION:+--min-extension "$MIN_EXTENSION"}
rm -rf "public/$label"
cp -r "$releases/$label" "public/$label"
n=$(find public -type f | wc -l)
if [ "$n" -gt 19500 ]; then echo "public/ has $n files: Cloudflare Pages allows 20000 (README: Retention)" >&2; exit 1; fi
npx wrangler pages deploy public --project-name "$project" --branch main
