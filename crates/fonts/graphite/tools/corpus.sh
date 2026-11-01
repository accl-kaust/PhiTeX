#!/usr/bin/env bash
# phitex-graphite's corpus, shaped by XeTeX (the oracle) and checked:
#
#   tools/corpus.sh build DIR [GR2TESTS]  # documents and their XDVs into DIR
#   tools/corpus.sh check DIR INDEX       # otf-xdv over DIR (built first)
#
# `build`: the TeX Live fonts with a Silf table (fontconfig's list, as
# XeTeX's), the documents tools/gencorpus.py writes for them (and for
# graphite2's test fonts, given its tests/ directory), each run with
# scripts/xetex/oracle.sh --shaping (plain XeTeX, XDV kept), CORPUS_JOBS
# (all CPUs) at a time. Prints the fonts and the documents' count.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"
export FONTCONFIG_FILE="$repo/scripts/xetex/fonts.conf"
jobs="${CORPUS_JOBS:-$(nproc)}"

build() {
  local out="$1" extra="${2:-}"
  mkdir -p "$out"
  out="$(realpath "$out")"
  fc-cache > /dev/null 2>&1 || true
  mapfile -t fonts < <(fc-list -f '%{file}|%{capability}\n' | grep 'ttable:Silf' | cut -d'|' -f1 | sort -u)
  python3 -I "$here/gencorpus.py" "$out" "$here/../corpus/texts" "${fonts[@]}" ${extra:+--extra "$extra"}
  export repo
  run_all() { # the documents on stdin, each in a directory of its own
    xargs -P "$jobs" -I{} bash -c 'f="$1"; d="${f%.tex}"; mkdir -p "$d"; timeout 1800 "$repo/scripts/xetex/oracle.sh" --no-pdf --shaping "$f" "$d" 2> "$d.err"' _ {}
  }
  # (a document XeTeX did not finish: no `Output written` in its log)
  stopped() { while read -r f; do d="${f%.tex}"; n="$(basename "$d")"; grep -q '^Output written' "$d/$n.log" 2>/dev/null || echo "$f"; done; }
  find "$out" -maxdepth 1 -name '*.tex' | sort | run_all
  # The units XeTeX stopped in ("all shapers failed"), again a line at a
  # time; the lines it stops in too are listed in STOPPED.txt, and no
  # stopped document's XDV is kept.
  find "$out" -maxdepth 1 -name '*.tex' | sort | stopped > "$out/STOPPED-units.txt"
  while read -r f; do
    python3 -I "$here/gencorpus.py" --split "$f" > /dev/null
    rm -rf "${f%.tex}"
  done < "$out/STOPPED-units.txt"
  sed 's/\.tex$//' "$out/STOPPED-units.txt" | while read -r u; do ls "$u"-l*.tex; done | run_all
  sed 's/\.tex$//' "$out/STOPPED-units.txt" | while read -r u; do ls "$u"-l*.tex; done | stopped > "$out/STOPPED.txt"
  while read -r f; do
    { printf '%s: ' "$(basename "$f")"; grep -o '\\T{.*}' "$f"; cat "$(dirname "$f")/$(grep -o '{[^{}]*\.txt}' "$f" | tr -d '{}')"; } >> "$out/STOPPED-lines.txt"
    rm -f "${f%.tex}"/*.xdv
  done < "$out/STOPPED.txt"
  echo "documents: $(find "$out" -name '*.xdv' | wc -l) XDVs of $(find "$out" -maxdepth 1 -name '*.tex' | wc -l); units stopped $(wc -l < "$out/STOPPED-units.txt"), lines stopped $(wc -l < "$out/STOPPED.txt")"
}

check() {
  local out="$1" index="$2"
  "$repo/target/release/examples/otf-xdv" "$index" "$out" --examples "${EXAMPLES:-60}"
}

case "${1:-}" in
  build) build "${2:?DIR}" "${3:-}" ;;
  check) check "${2:?DIR}" "${3:?INDEX}" ;;
  *) sed -n '2,11p' "$0"; exit 2 ;;
esac
