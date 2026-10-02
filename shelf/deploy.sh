#!/usr/bin/env bash
# Build and upload Shelf to Cloudflare Pages (project "shelf-phitex", custom
# domain shelf.phitex.org). Run it sandboxed:
#   scripts/sandbox --net npx wrangler login   # once
#   scripts/sandbox --net ./deploy.sh
set -euo pipefail
cd "$(dirname "$0")"
python3 build.py "${LABEL:-tl2026}"
npx wrangler pages deploy public --project-name shelf-phitex --branch main
