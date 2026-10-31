#!/bin/bash
# XeTeX's PDF, written by the engine from its pages (crates/engines/tex/src/xdvpdf.rs), checked on the XeTeX
# corpus (the documents of sweep.sh, crates/output/xdvipdfmx/tools/corpus.sh's) at -threads=1 and -threads=8.
# Each document runs once under TeX Live's xelatex -no-pdf, its XDV then made a PDF as xelatex does
# (`xdvipdfmx -q -E -o NAME.pdf < NAME.xdv`): the oracle XDV and PDF. Ours runs as phitex --compat=xelatex with
# PHITEX_XDV_DIFF=1: the build writes its PDF directly and also writes its XDV and has the xdvipdfmx port make
# a PDF of it, compared byte for byte (the line `xdv-diff: same ...` on its stderr); PHITEX_XDV_DIFF_DIR keeps
# the XDV. table.tsv: name, xelatex's exit; for -j1 and -j8: the direct PDF against the port's (same |
# DIFFERENT@BYTE | port-failed | none), our XDV against xelatex's, our PDF against xelatex's (same | differs@BYTE
# | no-pdf | no-oracle | timeout). With extra arguments, only the documents whose names contain one of them.
set -u
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH=1700000000 NO_COLOR=1
export FONTCONFIG_FILE=$(realpath scripts/xetex/fonts.conf) OSFONTDIR=
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 || { grep -E "^error" -A10 "$r/build.txt" | head -40 > "$r/summary.txt"; exit 1; }
export B=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/release/phitex
export PHITEX_CACHE_DIR=$w/pdf-sweep-cache
export repo=$PWD S=$w/pdf-sweep
rm -rf "$S"; mkdir -p "$S"
fc-cache >/dev/null 2>&1 || true
# the documents: kind|file
{
  for pkg in fontspec unicode-math polyglossia; do
    for f in upstream/$pkg/testfiles/*.lvt; do [ -e "$f" ] && echo "$pkg|$PWD/$f"; done
  done
  for f in crates/output/xdvipdfmx/tools/probe/*.tex tests/e2e/*xelatex*.tex; do [ -e "$f" ] && echo "probe|$PWD/$f"; done
} > "$S/all"
if [ $# -gt 0 ]; then
  : > "$S/list"; for p in "$@"; do grep -- "$p" "$S/all" >> "$S/list"; done
else cp "$S/all" "$S/list"; fi
echo "documents: $(wc -l < "$S/list")" > "$r/summary.txt"
# one run: MODE (oracle|j1|j8) KIND FILE
run() {
  local mode=$1 kind=$2 f=$3 n d t0 e
  n=$(basename "$f"); n=${n%.tex}; n=${n%.lvt}
  d=$S/$kind-$n/$mode; mkdir -p "$d"; cp "$f" "$d/$n.tex"
  if [ "$kind" = probe ]; then export TEXINPUTS="$(dirname "$f"):$repo/upstream/latex2e/base/doc:$repo/bench/amsldoc:"
  else export TEXINPUTS="$repo/upstream/$kind//:$repo/upstream/$kind/testfiles:"; fi
  t0=$(date +%s.%N)
  if [ $mode = oracle ]; then
    (cd "$d" && timeout 120 xelatex -interaction=nonstopmode -no-pdf "$n.tex" </dev/null >/dev/null 2>&1); e=$?
    [ -s "$d/$n.xdv" ] && (cd "$d" && timeout 120 xdvipdfmx -q -E -o "$n.pdf" < "$n.xdv" >/dev/null 2>&1)
  else
    (cd "$d" && PHITEX_XDV_DIFF=1 PHITEX_XDV_DIFF_DIR="$d/diff" timeout 300 "$B" --compat=xelatex -interaction=nonstopmode -threads=${mode#j} "$n.tex" </dev/null >"$d/run.out" 2>&1); e=$?
  fi
  echo "$e $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}')" > "$d/exit"
}
export -f run
for mode in oracle j1 j8; do
  P=16; [ $mode = j1 ] && P=12; [ $mode = j8 ] && P=2
  tr '|' ' ' < "$S/list" | sed "s/^/$mode /" | xargs -P $P -L 1 bash -c 'run "$@"' _
done
# cmp A B: same | differs@BYTE | no-ours | no-oracle
cmpv() { if [ ! -s "$1" ]; then echo no-ours; elif [ ! -s "$2" ]; then echo no-oracle; elif cmp -s "$1" "$2"; then echo same
  else echo "differs@$(cmp "$1" "$2" 2>&1 | head -1 | sed 's/.*byte \([0-9]*\).*/\1/')"; fi; }
printf 'name\txelatex\tj1_direct_port\tj1_xdv\tj1_pdf\tj8_direct_port\tj8_xdv\tj8_pdf\tj1_wall\n' > "$r/table.tsv"
mkdir -p "$r/logs"
while IFS='|' read -r kind f; do
  n=$(basename "$f"); n=${n%.tex}; n=${n%.lvt}; D=$S/$kind-$n
  row="$kind-$n\t$(cut -d' ' -f1 "$D/oracle/exit")"
  for m in j1 j8; do
    e=$(cut -d' ' -f1 "$D/$m/exit")
    if [ "$e" = 124 ]; then row="$row\ttimeout\ttimeout\ttimeout"; continue; fi
    l=$(grep '^xdv-diff:' "$D/$m/run.out" | tail -1)
    case "$l" in
      "xdv-diff: same"*) v=same ;;
      "xdv-diff: DIFFERENT at byte"*) v="DIFFERENT@$(echo "$l" | sed 's/.*at byte \([0-9]*\).*/\1/')" ;;
      "xdv-diff: the port failed"*) v=port-failed ;;
      *) v=none ;;
    esac
    row="$row\t$v\t$(cmpv "$D/$m/diff/$n.pdf.xdv" "$D/oracle/$n.xdv")\t$(cmpv "$D/$m/$n.pdf" "$D/oracle/$n.pdf")"
    case "$v" in same|none) ;; *) mkdir -p "$r/diffs/$kind-$n-$m"; cp "$D/$m/diff/"* "$D/$m/$n.pdf" "$r/diffs/$kind-$n-$m/" 2>/dev/null ;; esac
  done
  row="$row\t$(cut -d' ' -f2 "$D/j1/exit")"
  printf "$row\n" >> "$r/table.tsv"
  mkdir -p "$r/logs/$kind-$n"; for m in j1 j8; do cp "$D/$m/run.out" "$r/logs/$kind-$n/$m.out" 2>/dev/null; done
done < "$S/list"
for c in 3 4 5 6 7 8; do echo "$(head -1 "$r/table.tsv" | cut -f$c): $(cut -f$c "$r/table.tsv" | tail -n +2 | sed 's/@.*//' | sort | uniq -c | tr '\n' ' ')" >> "$r/summary.txt"; done
true
