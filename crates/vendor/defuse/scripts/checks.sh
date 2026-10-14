#!/bin/sh
# Every build configuration the crate must keep: formatting, pedantic clippy
# in the default, profiling and wasm32 configurations, and the wasm32 build
# with threads (the atomics target feature, standard library from source).
cd "$(dirname "$0")/.." || exit 1
set -e
cargo +nightly fmt --check
for cfg in "" "--features profile" "--features observe" "--target wasm32-unknown-unknown"; do
  echo "== clippy $cfg"
  cargo +nightly clippy -q --all-targets $cfg -- -W clippy::pedantic -D warnings
done
echo "== tests with debug assertions"
cargo +nightly test -q > target/checks-tests.log 2>&1 || { grep -E "FAILED|panicked" target/checks-tests.log; exit 1; }
grep -E "^test result" target/checks-tests.log
echo "== wasm32 with threads"
RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals" \
  cargo +nightly build -q -Z build-std=std,panic_abort --target wasm32-unknown-unknown
echo CHECKS_OK
