#!/bin/bash
export PHITEX_CACHE_DIR=$PWD/target/cache PHITEX_THREADS=4
for spec in predicted all; do
  echo "=== spec $spec"
  PHITEX_SPEC=$spec scripts/sandbox cargo test -p phitex-tex --test chain chains -- --include-ignored --nocapture 2>&1 | grep -E "nodes run|token-frame|wait [0-9]|height|a link|^test |spec|speculations" | head -80
done
echo "== done"
