#!/usr/bin/env bash
# The viewer's tests (viewer/test/*.test.ts): node's own runner on the
# TypeScript as it is (types stripped), no DOM, no packages.
#
#   scripts/viewer-test.sh
#
# Runs in scripts/sandbox; PHITEX_BUNDLE_DIRECT=1 runs it as it is (a CI runner).
set -euo pipefail
repo="$(cd "$(dirname "$0")/.." && pwd)"
run() {
  if [ -n "${PHITEX_BUNDLE_DIRECT:-}" ]; then "$@"; else "$repo/scripts/sandbox" "$@"; fi
}
run node --test "$repo"/viewer/test/*.test.ts
