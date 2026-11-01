#!/bin/bash
# XeTeX corpus sweep (milestone 3): the documents of crates/output/xdvipdfmx/tools/corpus.sh (fontspec,
# unicode-math, polyglossia l3build tests; the probe documents; tests/e2e's xelatex documents), each run once by
# TeX Live's xelatex -no-pdf (the oracle) and by phitex --compat=xelatex -no-pdf at -threads=1 and -threads=8;
# the XDVs compared byte for byte; table.tsv: name, xelatex exit, our j1 and j8 (same | differs at BYTE | no xdv |
# timeout), xelatex's and our error counts (log lines beginning "!"), j1 wall. Writes the XDVs and logs beside it
# (xdv/, logs/); docs/note-xetex-corpus.md has the first run's result grouped by first cause.
set -u
export FORCE_SOURCE_DATE=1 SOURCE_DATE_EPOCH=1700000000 NO_COLOR=1
export FONTCONFIG_FILE=$(realpath scripts/xetex/fonts.conf) OSFONTDIR=
cargo build -q --release -p phitex-cli > "$r/build.txt" 2>&1 || { grep -E "^error" -A10 "$r/build.txt" | head -40 > "$r/summary.txt"; exit 1; }
export B=$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys;print(json.load(sys.stdin)["target_directory"])')/release/phitex
export PHITEX_CACHE_DIR=$w/sweep-cache
export repo=$PWD S=$w/sweep
rm -rf "$S"; mkdir -p "$S" "$r/xdv"
fc-cache >/dev/null 2>&1 || true
# the documents: kind|file
{
  for pkg in fontspec unicode-math polyglossia; do
    for f in upstream/$pkg/testfiles/*.lvt; do [ -e "$f" ] && echo "$pkg|$PWD/$f"; done
  done
  for f in crates/output/xdvipdfmx/tools/probe/*.tex tests/e2e/*xelatex*.tex; do [ -e "$f" ] && echo "probe|$PWD/$f"; done
} > "$S/list"
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
  else
    (cd "$d" && timeout 300 "$B" --compat=xelatex -no-pdf -interaction=nonstopmode -threads=${mode#j} "$n.tex" </dev/null >"$d/run.out" 2>&1); e=$?
  fi
  echo "$e $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}')" > "$d/exit"
}
export -f run
for mode in oracle j1 j8; do
  P=16; [ $mode = j1 ] && P=12; [ $mode = j8 ] && P=2
  tr '|' ' ' < "$S/list" | sed "s/^/$mode /" | xargs -P $P -L 1 bash -c 'run "$@"' _
done
printf 'name\txelatex\tj1\tj8\terr_xelatex\terr_j1\terr_j8\tj1_wall\n' > "$r/table.tsv"
while IFS='|' read -r kind f; do
  n=$(basename "$f"); n=${n%.tex}; n=${n%.lvt}; D=$S/$kind-$n
  o=$D/oracle/$n.xdv
  row="$kind-$n\t$(cut -d' ' -f1 "$D/oracle/exit")"
  for m in j1 j8; do
    x=$D/$m/$n.xdv; e=$(cut -d' ' -f1 "$D/$m/exit")
    if [ "$e" = 124 ]; then c=timeout
    elif [ ! -s "$x" ]; then c="no-xdv(exit $e)"
    elif [ ! -s "$o" ]; then c="no-oracle"
    elif cmp -s "$x" "$o"; then c=same
    else c="differs@$(cmp "$x" "$o" 2>&1 | head -1 | sed 's/.*byte \([0-9]*\).*/\1/')"; fi
    row="$row\t$c"
    [ -s "$x" ] && cp "$x" "$r/xdv/$kind-$n-$m.xdv"
  done
  [ -s "$o" ] && cp "$o" "$r/xdv/$kind-$n-oracle.xdv"
  for m in oracle j1 j8; do row="$row\t$([ -f "$D/$m/$n.log" ] && grep -c '^!' "$D/$m/$n.log"; true)"; done
  row="$row\t$(cut -d' ' -f2 "$D/j1/exit")"
  printf "$row\n" >> "$r/table.tsv"
  mkdir -p "$r/logs/$kind-$n"; for m in oracle j1 j8; do cp "$D/$m/$n.log" "$r/logs/$kind-$n/$m.log" 2>/dev/null; cp "$D/$m/run.out" "$r/logs/$kind-$n/$m.out" 2>/dev/null; done
done < "$S/list"
for m in 3 4; do echo "col $m: $(cut -f$m "$r/table.tsv" | tail -n +2 | sed 's/@.*//; s/(.*//' | sort | uniq -c | tr '\n' ' ')" >> "$r/summary.txt"; done
true
