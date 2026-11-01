#!/usr/bin/env bash
# The WebAssembly build with threads: phitex-wasm (crates/wasm) for
# wasm32-wasip1-threads, run in Node by scripts/wasm-run.mjs (WASI for the
# files, a Node worker per engine thread on the module's shared memory).
#
#   scripts/sandbox scripts/wasm.sh build   # target/wasm32-wasip1-threads/fastdev/phitex-wasm.wasm
#   scripts/sandbox scripts/wasm.sh check   # build, then a plain TeX and a LaTeX document,
#                                           # byte for byte against pdfTeX's (oracle/fixtures),
#                                           # at 1 and 4 threads, the formats made in wasm;
#                                           # a fatal error's exit status, log and no PDF
#   scripts/sandbox scripts/wasm.sh run [phitex-wasm's arguments] FILE   # in FILE's directory
#
# Needs the pinned toolchain's wasm32-wasip1-threads target (not in
# rust-toolchain.toml, whose targets every machine installs, the accl
# image's read-only toolchain included: `rustup target add
# wasm32-wasip1-threads --toolchain nightly-2026-08-08` once, outside the
# sandbox), Node >= 22, and TeX Live for the
# formats' files and the fonts. The fastdev profile (thin LTO): a release
# build's speed without fat LTO's link.
set -euo pipefail
cd "$(dirname "$0")/.."
repo=$PWD
# (WASM_TARGET=wasm32-wasip1: the single-threaded build, for a browser
# without shared memory between threads, Firefox's extensions)
target=${WASM_TARGET:-wasm32-wasip1-threads}
wasm=$repo/target/$target/fastdev/phitex-wasm.wasm

build() {
  cargo build --profile fastdev --target "$target" -p phitex-wasm
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

# check_ini NAME FIXTURE-DIR FILE ORACLE: an -ini job (`--ini`, no
# format) at 4 and 1 threads, its PDF against the oracle.
check_ini() {
  local name=$1 src=$2 file=$3 oracle=$4
  local dir=$repo/target/wasm-check/$name job=${file%.tex}
  rm -rf "$dir" && mkdir -p "$dir"
  cp "$src/$file" "$dir"/
  for j in 4 1; do
    rm -f "$dir/$job.pdf"
    (cd "$dir" && run --ini --epoch 0 -j "$j" "$file") >"$dir/run-j$j.txt" 2>&1 || {
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

# check_fatal NAME FIXTURE-DIR FILE LINE: an -ini job pdfTeX stops with a
# fatal error at its end (pdftex_fail): exit status 1, no PDF, the error's
# LINE in the log and said on standard error.
check_fatal() {
  local name=$1 src=$2 file=$3 line=$4
  local dir=$repo/target/wasm-check/$name job=${file%.tex}
  rm -rf "$dir" && mkdir -p "$dir"
  cp "$src/$file" "$src"/*.tfm "$dir"/
  local rc=0
  (cd "$dir" && run --ini --epoch 0 -j 4 "$file") >"$dir/run.txt" 2>&1 || rc=$?
  if [ "$rc" = 1 ] && [ ! -e "$dir/$job.pdf" ] && grep -qxF -- "$line" "$dir/$job.log" \
      && grep -qF -- "${line#!}" "$dir/run.txt"; then
    echo "ok $name: exit 1, no PDF, '$line'"
  else
    cat "$dir/run.txt" "$dir/$job.log" 2>/dev/null
    echo "FAIL $name: exit $rc (1 wanted), $(ls "$dir")"
    return 1
  fi
}

case "${1:-check}" in
  build) build ;;
  run) shift; run "$@" ;;
  check)
    build
    fx=$repo/oracle/fixtures
    check_doc plain "$fx/plain/ini" doc.tex "$fx/plain/ini/doc-pdfetex.pdf" pdftex
    check_doc latex "$fx/latex/hello" hello.tex "$fx/latex/hello/hello.pdf" pdflatex
    # (links: their object numbers are read-time identities past 32
    # bits, which wasm32's usize does not hold)
    check_ini linkbreak "$fx/pdf" linkbreak.tex "$fx/pdf/linkbreak.pdf"
    # (a font with no map entry: pdfTeX's fatal error, no PDF)
    check_fatal nomap "$fx/pdf" nomap.tex '!pdfTeX error: pdftex (file nomap10): Font nomap10 at 72 not found'
    echo WASM_OK
    ;;
  *) echo "usage: $0 build|check|run [arguments]" >&2; exit 2 ;;
esac
