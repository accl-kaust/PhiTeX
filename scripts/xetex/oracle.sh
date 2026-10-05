#!/usr/bin/env bash
# The Unicode engines' oracle (DESIGN 4.7): runs TeX Live's xetex (or
# another engine) on FILE.tex in OUTDIR, deterministically and with TeX
# Live's fonts only.
#   scripts/xetex/oracle.sh [--engine E] [--latex] [--no-pdf] FILE.tex OUTDIR [ARGS...]
# E is xetex (the default) or luatex (luahbtex for --latex, as lualatex);
# --latex runs the LaTeX format. Writes OUTDIR/NAME.{xdv,pdf,log,aux,...}.
# With --no-pdf xetex keeps its XDV (`xetex -no-pdf`); without it xetex
# pipes into xdvipdfmx as it does for users, and the PDF is the oracle's.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
engine=xetex latex= nopdf=
while [ $# -gt 0 ]; do
  case "$1" in
    --engine) engine="$2"; shift 2 ;;
    --latex) latex=1; shift ;;
    --no-pdf) nopdf=-no-pdf; shift ;;
    *) break ;;
  esac
done
case "$engine$latex" in
  xetex) prog=xetex ;; xetex1) prog=xelatex ;;
  luatex) prog=luatex ;; luatex1) prog=lualatex ;;
  *) echo "oracle.sh: unknown engine $engine" >&2; exit 2 ;;
esac
src="$(realpath "$1")"; out="$2"; shift 2
mkdir -p "$out"
# Fonts: TeX Live's only. fontconfig (xetex) reads fonts.conf; luaotfload
# builds its name database from the same list, in a cache of its own
# (TEXMFVAR) so no user's database or system font enters it.
export FONTCONFIG_FILE="$here/fonts.conf" OSFONTDIR=
export TEXMFVAR="/tmp/partex-oracle-texmfvar-$engine"
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export TEXINPUTS="$(dirname "$src"):${TEXINPUTS:-}"
cd "$out"
"$prog" -interaction=nonstopmode $nopdf "$@" "$src" </dev/null >/dev/null || true
