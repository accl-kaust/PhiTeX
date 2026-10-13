#!/bin/bash
export PHITEX_CACHE_DIR=$PWD/target/cache
S=scripts/sandbox
for t in edits hints plain page_steps diagnostics core inputs; do echo "== $t"; $S cargo test --profile fastdev -p phitex-tex --test $t 2>&1 | grep -E "test result|FAILED|panicked|error(\[|:)" | head; done
echo "== incr"; $S cargo test --profile fastdev -p phitex-tex --lib incr 2>&1 | grep -E "test result|panicked"
echo "== cli"; $S cargo test --profile fastdev -p phitex-cli 2>&1 | grep -E "test result|FAILED|panicked" | head
