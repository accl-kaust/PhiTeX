#!/bin/bash
# r3-cuts (accl cmd task): scripts/accl/edit-classes.py on commit $A and
# on this checkout in one job (release CLIs, the users' build standing in
# for the profile build too), A, B, then A and B again.
set -u
B=$(git rev-parse --short=8 HEAD)
export CARGO_BUILD_JOBS=16
CARGO_TARGET_DIR=$w/t-b cargo build -q --release -p phitex-cli > "$r/build-b.txt" 2>&1 || exit 1
git -c advice.detachedHead=false checkout -q "$A"
CARGO_TARGET_DIR=$w/t-a cargo build -q --release -p phitex-cli > "$r/build-a.txt" 2>&1 || exit 1
git -c advice.detachedHead=false checkout -q "$B"
for pass in 1 2; do
  for x in a b; do
    c=$([ $x = a ] && echo "$A" || echo "$B")
    mkdir -p "$r/$x$pass"
    echo "== $c pass $pass" >> "$r/summary.txt"
    python3 -I scripts/accl/edit-classes.py "$w/t-$x/release/phitex" "$w/t-$x/release/phitex" "$w/ec-$x$pass" "$r/$x$pass" | grep -E '^\| [a-z]' | cut -d'|' -f2-6 >> "$r/summary.txt"
  done
done
cat "$r/summary.txt"
