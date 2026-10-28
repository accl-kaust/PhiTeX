#!/usr/bin/env bash
# Build and upload Shelf to Cloudflare Pages. Run it sandboxed:
#   scripts/sandbox --net npx wrangler login   # once
#   scripts/sandbox --net ./deploy.sh
#
# Schema 4 is a Pages project of its own, PROJECT (default shelf-phitex-v4:
# shelf-phitex-v4.pages.dev), beside the schema-3 site (project
# shelf-phitex: shelf-phitex.pages.dev, shelf.phitex.org), which stays as
# it is for the extensions that read it. A Pages deployment replaces its
# project's whole site, so schema 4 deployed to shelf-phitex would take
# their files away: this script refuses that project.
set -euo pipefail
cd "$(dirname "$0")"
label="${LABEL:-tl2026}"
project="${PROJECT:-shelf-phitex-v4}"
# (RELEASES: the releases directory deployed, default releases-v4/)
releases="${RELEASES:-releases-v4}"
if [ "$project" = shelf-phitex ]; then
  echo "shelf-phitex is the schema-3 site, kept as it is: schema 4 goes to its own project (PROJECT)" >&2
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
