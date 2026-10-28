#!/usr/bin/env bash
# beforeafter-tmp.sh FILTER PATHS...: (accl cmd) the latex test FILTER on this tree, then with PATHS reverted to HEAD~1
set -u
cargo test -q --release -p phitex-tex --test latex "$1" 2>&1 | grep -E "^test |test result|panicked|FAIL|differ" | head -20
f=$1; shift
git diff HEAD~1 HEAD -- "$@" | git apply -R && echo "== reverted $*"
cargo test -q --release -p phitex-tex --test latex "$f" 2>&1 | grep -E "^test |test result|panicked|FAIL|differ" | head -20
