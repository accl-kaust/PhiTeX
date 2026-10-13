#!/bin/bash
export PHITEX_CACHE_DIR=$PWD/target/cache
echo "== latex_hello"; scripts/sandbox cargo test --profile fastdev -p phitex-tex --test latex latex_hello -- --ignored 2>&1 | grep -E "test result|FAILED|panicked" | head
