#!/usr/bin/env bash
# Make the native pdflatex format used by the manual PDF oracle checks.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
shim="$repo/target/partex-shim"
test -x "$repo/target/release/phitex" || {
  echo "build partex-cli --release first" >&2
  exit 2
}
mkdir -p "$shim/formats"
ln -sfn "$repo/target/release/phitex" "$shim/pdftex"
ln -sfn "$repo/target/release/phitex" "$shim/pdflatex"
(
  cd "$shim/formats"
  "$repo/scripts/sandbox" env SOURCE_DATE_EPOCH=1700000000 TZ=UTC \
    "$shim/pdftex" -ini -interaction=nonstopmode -jobname=pdflatex \
    -translate-file=cp227.tcx '*pdflatex.ini' >pdflatex-build.log
)
test -s "$shim/formats/pdflatex.fmt"
