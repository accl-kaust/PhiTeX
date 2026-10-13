#!/bin/bash
set -u
export PHITEX_CACHE_DIR=$PWD/target/cache
S=scripts/sandbox
$S cargo fmt --check -p phitex-tex 2>&1 | head -20; echo "fmt: ${PIPESTATUS[0]}"
$S cargo clippy -p phitex-tex --all-targets 2>&1 | grep -E "^(warning|error)" -A6 | head -80; echo "clippy done"
$S cargo test -p phitex-tex --lib incr 2>&1 | tail -5
PHITEX_INCR_DOC=$PWD/scratch/para4000.tex $S cargo test --release -p phitex-tex --lib incr_edit_doc -- --ignored --nocapture 2>&1 | grep -E "^incr|test result|panicked" 
echo "== build"; PHITEX_TEX_TIME=1 PHITEX_INCR_DOC=$PWD/scratch/para4000.tex $S cargo test --release -p phitex-tex --lib incr_edit_build -- --ignored --nocapture 2>&1 | grep -E "^build|^apply_text|^push_text|test result|panicked"
for t in edits hints plain page_steps diagnostics core; do
  echo "== $t"; $S cargo test -p phitex-tex --test $t 2>&1 | grep -E "test result|FAILED|panicked|failures:" | head -20
done
echo "== cli"; $S cargo test -p phitex-cli 2>&1 | grep -E "test result|FAILED|panicked" | head -20
echo "== latex_hello"; timeout 1500 $S cargo test --release -p phitex-tex --test latex latex_hello -- --ignored 2>&1 | grep -E "test result|FAILED|panicked" | head
