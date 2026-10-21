#!/usr/bin/env bash
# phitex-xdvipdfmx's corpus: XDV files and the PDFs xelatex makes of them.
#
#   tools/corpus.sh build DIR    # XDVs and their oracle PDFs into DIR
#   tools/corpus.sh check DIR [BIN] [FILTER]  # BIN (xdv2pdf) on each, cmp
#
# Run inside scripts/sandbox. The oracle PDF is xelatex's own: xelatex
# pipes its XDV into `xdvipdfmx -q -E -o NAME.pdf` (stdin, so no DVI name;
# the subset tags hash "NAME.pdf"), which is what `build` runs on each XDV,
# in the XDV's directory, with oracle.sh's environment.
#
# Each document runs in a directory of its own, CORPUS_JOBS (all CPUs) at
# a time. `check` writes DIR/FAIL.txt (one XDV a line) and DIR/DIFFS.txt
# (each failure's first difference: the byte offset, the object holding
# it and its writer part, the first difference of the decoded stream).
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH="${SOURCE_DATE_EPOCH:-1700000000}"
export FONTCONFIG_FILE="$repo/scripts/xetex/fonts.conf"
export here repo
jobs="${CORPUS_JOBS:-$(nproc)}"

oracle_pdf() { # DIR NAME: DIR/NAME.pdf from DIR/NAME.xdv, as xelatex does
  (cd "$1" && xdvipdfmx -q -E -o "$2.pdf" < "$2.xdv" 2>/dev/null) || true
}

# one document: KIND SRC OUTDIR (KIND: an l3build test's package, or
# `probe` for a document run as oracle.sh runs it)
one() {
  local kind="$1" f="$2" d="$3" n
  n="$(basename "$f" .tex)"; n="${n%.lvt}"
  mkdir -p "$d"
  if [ "$kind" = probe ]; then
    TEXINPUTS="$repo/upstream/latex2e/base/doc:$repo/bench/amsldoc:${TEXINPUTS:-}" \
      "$repo/scripts/xetex/oracle.sh" --latex --no-pdf "$f" "$d" || true
  else
    cp "$f" "$d/$n.tex"
    (cd "$d" && TEXINPUTS="$repo/upstream/$kind//:$repo/upstream/$kind/testfiles:" \
      timeout 120 xelatex -interaction=nonstopmode -no-pdf "$n.tex" \
      </dev/null >/dev/null 2>&1) || true
  fi
  if [ -s "$d/$n.xdv" ]; then oracle_pdf "$d" "$n"; fi
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
  # (fontconfig's cache made once, before the documents race to make it)
  fc-cache > /dev/null 2>&1 || fc-list > /dev/null 2>&1 || true
  export -f one oracle_pdf
  {
    # fontspec, unicode-math, polyglossia: their l3build tests, run once.
    for pkg in fontspec unicode-math polyglossia; do
      for f in "$repo"/upstream/$pkg/testfiles/*.lvt; do
        [ -e "$f" ] && printf '%s|%s|%s\n' "$pkg" "$f" "$out/$pkg/$(basename "$f" .lvt)"
      done
    done
    # The probe documents, the end-to-end tests' XeLaTeX documents and
    # any extra .tex given in $XCORPUS_EXTRA.
    for f in "$here"/probe/*.tex "$repo"/tests/e2e/*xelatex*.tex ${XCORPUS_EXTRA:-}; do
      [ -e "$f" ] && printf 'probe|%s|%s\n' "$f" "$out/probe/$(basename "$f" .tex)"
    done
  } | xargs -d '\n' -P "$jobs" -I{} bash -c 'IFS="|" read -r k f d <<< "$1"; one "$k" "$f" "$d"' _ {}
  find "$out" -name '*.xdv' | wc -l
}

# one XDV: OUT BIN XDV; prints `ok` or `FAIL <path>`
cmp_one() {
  local out="$1" bin="$2" f="$3" d n
  d="$(dirname "$f")"; n="$(basename "$f" .xdv)"
  [ -s "$d/$n.pdf" ] || return 0
  # (an XDV xdvipdfmx stops on leaves what it wrote: compared too)
  (cd "$d" && timeout 120 "$bin" -o "$n.pdf" "$n.xdv" > "$n.out.pdf" 2> "$n.err")
  if cmp -s "$d/$n.pdf" "$d/$n.out.pdf"; then
    rm -f "$d/$n.err"; echo ok
  else
    echo "FAIL ${f#"$out"/}"
  fi
}

check() {
  local out bin="${2:-$repo/target/release/xdv2pdf}" filter="${3:-}" res f
  out="$(realpath "$1")"
  export -f cmp_one
  res="$(find "$out" -name '*.xdv' | grep -e "$filter" | sort \
    | xargs -d '\n' -P "$jobs" -I{} bash -c 'cmp_one "$@"' _ "$out" "$bin" {})"
  grep '^FAIL ' <<< "$res" | cut -c6- | sort > "$out/FAIL.txt"
  : > "$out/DIFFS.txt"
  while read -r f; do
    [ -n "$f" ] || continue
    { echo "== $f"; head -c 600 "$out/${f%.xdv}.err" | sed 's/^/  err: /'
      python3 -I "$here/pdfdiff.py" "$out/${f%.xdv}.pdf" "$out/${f%.xdv}.out.pdf"; } >> "$out/DIFFS.txt" 2>&1
  done < "$out/FAIL.txt"
  echo "$(grep -c '^ok$' <<< "$res")/$(grep -c . <<< "$res") byte-identical"
}

case "${1:-}" in
  build) build "${2:?DIR}" ;;
  check) check "${2:?DIR}" "${3:-}" "${4:-}" ;;
  *) sed -n '2,6p' "$0"; exit 2 ;;
esac
