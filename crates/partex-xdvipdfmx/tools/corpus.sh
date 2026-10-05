#!/usr/bin/env bash
# partex-xdvipdfmx's corpus: XDV files and the PDFs xelatex makes of them.
#
#   tools/corpus.sh build DIR    # XDVs and their oracle PDFs into DIR
#   tools/corpus.sh check DIR [BIN] [FILTER]  # BIN (xdv2pdf) on each, cmp
#
# Run inside scripts/sandbox. The oracle PDF is xelatex's own: xelatex
# pipes its XDV into `xdvipdfmx -q -E -o NAME.pdf` (stdin, so no DVI name;
# the subset tags hash "NAME.pdf"), which is what `build` runs on each XDV,
# in the XDV's directory, with oracle.sh's environment.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../.." && pwd)"
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export FONTCONFIG_FILE="$repo/scripts/xetex/fonts.conf"

oracle_pdf() { # DIR NAME: DIR/NAME.pdf from DIR/NAME.xdv, as xelatex does
  (cd "$1" && xdvipdfmx -q -E -o "$2.pdf" < "$2.xdv" 2>/dev/null) || true
}

build() {
  local out="$1" f d n
  mkdir -p "$out"
  out="$(realpath "$out")"
  # refs/xetex: the XDVs l3build made of LaTeX's own tests.
  if [ -d "$repo/refs/xetex" ]; then
    (cd "$repo/refs/xetex" && find . -name '*.xdv') | while read -r f; do
      d="$out/refs/$(dirname "$f")"; n="$(basename "$f" .xdv)"
      mkdir -p "$d"; cp "$repo/refs/xetex/$f" "$d/"
      oracle_pdf "$d" "$n"
    done
  fi
  # fontspec, unicode-math, polyglossia: their l3build tests, run once.
  for pkg in fontspec unicode-math polyglossia; do
    for f in "$repo"/upstream/$pkg/testfiles/*.lvt; do
      [ -e "$f" ] || continue
      n="$(basename "$f" .lvt)"; d="$out/$pkg"
      mkdir -p "$d"; cp "$f" "$d/$n.tex"
      (cd "$d" && TEXINPUTS="$repo/upstream/$pkg//:$repo/upstream/$pkg/testfiles:" \
        timeout 120 xelatex -interaction=nonstopmode -no-pdf "$n.tex" \
        </dev/null >/dev/null 2>&1) || true
      [ -s "$d/$n.xdv" ] && oracle_pdf "$d" "$n"
    done
  done
  # Probe documents and any extra .tex given in $XCORPUS_EXTRA.
  for f in "$here"/probe/*.tex ${XCORPUS_EXTRA:-}; do
    [ -e "$f" ] || continue
    n="$(basename "$f" .tex)"; d="$out/probe"
    mkdir -p "$d"
    "$repo/scripts/xetex/oracle.sh" --latex --no-pdf "$f" "$d" || true
    [ -s "$d/$n.xdv" ] && oracle_pdf "$d" "$n"
  done
  find "$out" -name '*.xdv' | wc -l
}

check() {
  local out bin="${2:-$repo/target/release/xdv2pdf}" filter="${3:-}" ok=0 all=0 f d n
  out="$(realpath "$1")"
  : > "$out/FAIL.txt"
  while read -r f; do
    d="$(dirname "$f")"; n="$(basename "$f" .xdv)"
    [ -s "$d/$n.pdf" ] || continue
    all=$((all + 1))
    if (cd "$d" && timeout 120 "$bin" -o "$n.pdf" "$n.xdv" > "$n.out.pdf" 2> "$n.err") \
        && cmp -s "$d/$n.pdf" "$d/$n.out.pdf"; then
      ok=$((ok + 1)); rm -f "$d/$n.err"
    else
      echo "${f#"$out"/}" >> "$out/FAIL.txt"
    fi
  done < <(find "$out" -name '*.xdv' | grep -e "$filter" | sort)
  echo "$ok/$all byte-identical"
}

case "${1:-}" in
  build) build "${2:?DIR}" ;;
  check) check "${2:?DIR}" "${3:-}" "${4:-}" ;;
  *) sed -n '2,6p' "$0"; exit 2 ;;
esac
