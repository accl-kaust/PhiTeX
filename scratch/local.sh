#!/bin/bash
export PHITEX_CACHE_DIR=$PWD/target/cache
S="nice -n 19 ionice -c3 scripts/sandbox"
$S cargo test -p phitex-tex --lib incr 2>&1 | grep -E "test result|panicked"
for t in edits hints plain page_steps diagnostics core; do echo "== $t"; $S cargo test -p phitex-tex --test $t 2>&1 | grep -E "test result|FAILED|panicked" | head; done
echo "== cli"; $S cargo test -p phitex-cli 2>&1 | grep -E "test result|FAILED|panicked" | head
echo "== latex_hello"; $S cargo test --release -p phitex-tex --test latex latex_hello -- --ignored 2>&1 | grep -E "test result|FAILED|panicked" | head
echo DONE
