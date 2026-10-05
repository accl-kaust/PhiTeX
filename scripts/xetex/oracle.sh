#!/usr/bin/env bash
# XeTeX's oracle (DESIGN 4.7): runs TeX Live's xetex/xelatex on FILE.tex in
# OUTDIR, deterministically and with TeX Live's fonts only.
#   scripts/xetex/oracle.sh [--latex] [--no-pdf] FILE.tex OUTDIR [ARGS...]
# Writes OUTDIR/NAME.{xdv,pdf,log,aux,...}. With --no-pdf the XDV is kept
# (`xetex -no-pdf`); without it xetex pipes into xdvipdfmx as it does for
# users, and the PDF is the oracle's.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
engine=xetex nopdf=
while [ $# -gt 0 ]; do
  case "$1" in
    --latex) engine=xelatex; shift ;;
    --no-pdf) nopdf=-no-pdf; shift ;;
    *) break ;;
  esac
done
src="$(realpath "$1")"; out="$2"; shift 2
mkdir -p "$out"
export FONTCONFIG_FILE="$here/fonts.conf"
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export TEXINPUTS="$(dirname "$src"):${TEXINPUTS:-}"
cd "$out"
"$engine" -interaction=nonstopmode $nopdf "$@" "$src" </dev/null >/dev/null || true
