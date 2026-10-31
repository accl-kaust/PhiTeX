#!/usr/bin/env bash
# beforeafter-tmp.sh REV FILTER PATHS...: (accl cmd) the latex test FILTER on this tree, then with commit REV's change to PATHS reverted
set -u
rev=$1 f=$2; shift 2
cargo test -q --release -p phitex-tex --test latex "$f" 2>&1 | grep -E "^test |test result|panicked|FAIL|differ" | head -20
git diff "$rev~1" "$rev" -- "$@" | git apply -R && echo "== reverted $rev's $*"
cargo test -q --release -p phitex-tex --test latex "$f" 2>&1 | grep -E "^test |test result|panicked|FAIL|differ" | head -20
