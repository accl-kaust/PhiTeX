#!/usr/bin/env bash
# The WebAssembly build with threads: phitex-wasm (crates/wasm) for
# wasm32-wasip1-threads, run in Node by scripts/wasm-run.mjs (WASI for the
# files, a Node worker per engine thread on the module's shared memory).
#
#   scripts/sandbox scripts/wasm.sh build   # target/wasm32-wasip1-threads/fastdev/phitex-wasm.wasm
#   scripts/sandbox scripts/wasm.sh check   # build, then a plain TeX and a LaTeX document,
#                                           # byte for byte against pdfTeX's (oracle/fixtures),
#                                           # at 1 and 4 threads, the formats made in wasm
#   scripts/sandbox scripts/wasm.sh run [phitex-wasm's arguments] FILE   # in FILE's directory
#
# Needs the pinned toolchain's wasm32-wasip1-threads target
# (rust-toolchain.toml lists it), Node >= 22, and TeX Live for the
# formats' files and the fonts. The fastdev profile (thin LTO): a release
# build's speed without fat LTO's link.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD
wasm=$repo/target/wasm32-wasip1-threads/fastdev/phitex-wasm.wasm

build() {
  cargo build --profile fastdev --target wasm32-wasip1-threads -p phitex-wasm
}

run() {
  node --no-warnings "$repo/scripts/wasm-run.mjs" "$wasm" "$@"
}

# check NAME FIXTURE-DIR FILE ORACLE ENGINE: the document in a scratch
# directory (its format made there, in wasm, at 4 threads; then again at 1
# thread from the stored format), each output against the oracle.
check_doc() {
  local name=$1 src=$2 file=$3 oracle=$4 engine=$5
  local dir=$repo/target/wasm-check/$name job=${file%.tex}
  rm -rf "$dir" && mkdir -p "$dir"
  cp "$src"/*.tex "$dir"/
  for j in 4 1; do
    rm -f "$dir/$job.pdf"
    (cd "$dir" && run --engine "$engine" --epoch 0 -j "$j" --fmt-cache "$engine.state" "$file") \
      >"$dir/run-j$j.txt" 2>&1 || {
      cat "$dir/run-j$j.txt"
      echo "FAIL $name -j $j: phitex-wasm failed"
      return 1
    }
    if cmp -s "$dir/$job.pdf" "$oracle"; then
      echo "ok $name -j $j: byte-identical to pdfTeX's ($(stat -c %s "$oracle") bytes)"
    else
      echo "FAIL $name -j $j: $dir/$job.pdf differs from $oracle"
      return 1
    fi
  done
}

case "${1:-check}" in
  build) build ;;
  run) shift; run "$@" ;;
  check)
    build
    fx=$repo/oracle/fixtures
    check_doc plain "$fx/plain/ini" doc.tex "$fx/plain/ini/doc-pdfetex.pdf" pdftex
    check_doc latex "$fx/latex/hello" hello.tex "$fx/latex/hello/hello.pdf" pdflatex
    echo WASM_OK
    ;;
  *) echo "usage: $0 build|check|run [arguments]" >&2; exit 2 ;;
esac
