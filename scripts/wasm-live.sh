#!/usr/bin/env bash
# The frontends' engine (crates/wasm/live) and their latexdiff
# (crates/wasm/diff) built for the browser into one directory, as the
# frontends take them (PhiTeX-frontends' scripts/build/build.sh, or its
# PHITEX_WASM_DIR):
#   core.wasm   the live core, wasm32-wasip1-threads (its threads the host's
#               workers on one shared memory)
#   core1.wasm  the same crate for wasm32-wasip1: one thread, Firefox's
#               (an extension there gets no shared memory)
#   diff.wasm   phitex-diff for Compare, wasm32-unknown-unknown
# The formats a core reads at a first open are made and checked by the
# frontends' build, which runs these cores in its own host.
#
#   scripts/wasm-live.sh [OUT_DIR]   # default build/wasm-live
#
# Release builds with thin LTO and parallel codegen (the engine's speed
# without fat LTO's long link), panic=abort; the core a reactor
# (`_initialize` run once by the host), SIMD on: every browser and Node has
# wasm SIMD, and hayro, vello_cpu and zlib are scalar without it.
set -euo pipefail
cd "$(dirname "$0")/.."
out=${1:-build/wasm-live}
mkdir -p "$out"
fast=(--config 'profile.release.lto="thin"' --config 'profile.release.codegen-units=16' --config 'profile.release.debug=false' --config 'profile.release.panic="abort"')
reactor="-Zwasi-exec-model=reactor -C target-feature=+simd128"
CARGO_TARGET_WASM32_WASIP1_THREADS_RUSTFLAGS="$reactor" \
  cargo build --release "${fast[@]}" -p phitex-wasm-live --target wasm32-wasip1-threads
cp target/wasm32-wasip1-threads/release/core.wasm "$out/core.wasm"
CARGO_TARGET_WASM32_WASIP1_RUSTFLAGS="$reactor" \
  cargo build --release "${fast[@]}" -p phitex-wasm-live --target wasm32-wasip1
cp target/wasm32-wasip1/release/core.wasm "$out/core1.wasm"
# (small: size over speed, one codegen unit)
cargo build --release --config 'profile.release.opt-level="s"' --config 'profile.release.lto=true' \
  --config 'profile.release.codegen-units=1' --config 'profile.release.debug=false' --config 'profile.release.panic="abort"' \
  --config 'profile.release.strip=true' -p phitex-wasm-diff --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/phitex_wasm_diff.wasm "$out/diff.wasm"
ls -la "$out"
