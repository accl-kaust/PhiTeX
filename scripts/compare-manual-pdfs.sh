#!/usr/bin/env bash
# Build a settled oracle PDF, then compare partex's PDF from the same inputs.
# Each TeX/Lua/Perl run occurs inside scripts/sandbox; latexmk output stays in
# the sandbox's /tmp/tex and is copied out only after the run completes.
set -euo pipefail

repo="$(cd "$(dirname "$0")/.." && pwd)"
name="${1:-}"
case "$name" in
  lwarp)      source_dir=upstream/lwarp-ctan;             subdir=.;   input=lwarp.dtx ;;
  source3)    source_dir=upstream/latex3/l3kernel;       subdir=doc; input=source3.tex ;;
  source2e)   source_dir=upstream/latex2e/base;          subdir=doc; input=source2e.tex ;;
  pdftex)     source_dir=upstream/pdftex-manual;         subdir=.;   input=pdftex.tex ;;
  pgfmanual)  source_dir=upstream/pgf/doc/generic/pgf;   subdir=.;   input=pgfmanual.tex ;;
  usrguide)   source_dir=upstream/latex2e/base;          subdir=doc; input=usrguide.tex ;;
  clsguide)   source_dir=upstream/latex2e/base;          subdir=doc; input=clsguide.tex ;;
  fntguide)   source_dir=upstream/latex2e/base;          subdir=doc; input=fntguide.tex ;;
  amsldoc)    source_dir=upstream/latex2e/required/amsmath; subdir=.; input=amsldoc.tex ;;
  *) echo "usage: $0 {lwarp|source3|source2e|pdftex|pgfmanual|usrguide|clsguide|fntguide|amsldoc}" >&2; exit 2 ;;
esac

for cmd in latexmk pdflatex bwrap; do
  command -v "$cmd" >/dev/null || { echo "missing $cmd" >&2; exit 2; }
done
source_path="$repo/$source_dir"
test -f "$source_path/$subdir/$input" || { echo "fetch upstream sources first" >&2; exit 2; }
shim="$repo/target/partex-shim"
test -x "$shim/pdflatex" && test -f "$shim/formats/pdflatex.fmt" || {
  echo "build target/release/partex and its pdflatex format first" >&2
  exit 2
}

work="$repo/upstream/manual-pdf-check/$name"
rm -rf "$work"
mkdir -p "$work/src" "$work/oracle" "$work/partex"
cp -a "$source_path/." "$work/src/"
job="${input%.*}"
src="$work/src/$subdir"
output="/tmp/tex/manual-$name"

run_latexmk() {
  local engine="$1" stage="$work/$1" path="/usr/bin"
  local -a environment=(SOURCE_DATE_EPOCH=1700000000 TZ=UTC TEXINPUTS=.:..:)
  if [ "$engine" = partex ]; then
    path="$shim:/usr/bin"
    environment+=("TEXFORMATS=$shim/formats:")
  fi
  environment+=("PATH=$path")
  (
    cd "$src"
    "$repo/scripts/sandbox" env "${environment[@]}" \
      bash -c '
        set -euo pipefail
        mkdir -p "$1"
        if [ "$3" = partex ]; then
          cp -a "$4/oracle/." "$1/"
          rm -f "$1/$5.pdf" "$1/$5.log" "$1/$5.fdb_latexmk" "$1/$5.fls"
        fi
        if ! latexmk -pdf -quiet -recorder- -halt-on-error -interaction=batchmode \
          "-outdir=$1" "$2" >"$4/$3-run.log" 2>&1; then
          cp -a "$1/." "$4/$3/"
          tail -80 "$4/$3-run.log" >&2
          if [ -f "$1/$5.log" ]; then tail -40 "$1/$5.log" >&2; fi
          exit 1
        fi
        test -s "$1/$5.pdf"
        cp -a "$1/." "$4/$3/"
      ' bash "$output" "$input" "$engine" "$work" "$job"
  )
  echo "$name $engine: $(wc -c < "$stage/$job.pdf") bytes"
}

run_latexmk oracle
run_latexmk partex
if cmp "$work/oracle/$job.pdf" "$work/partex/$job.pdf"; then
  echo "$name: byte exact"
else
  echo "$name: PDF differs; outputs and logs: $work" >&2
  exit 1
fi
