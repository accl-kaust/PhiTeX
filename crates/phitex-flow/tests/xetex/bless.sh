#!/usr/bin/env bash
# The XeTeX oracles of tests/xetex_docs.rs, made by TeX Live's xetex and
# xelatex (SOURCE_DATE_EPOCH=0, FORCE_SOURCE_DATE=1, batch mode,
# -recorder), with fontconfig seeing TeX Live's OpenType and TrueType
# fonts only (the font index the graph is given lists the same):
#
#   crates/phitex-flow/tests/xetex/bless.sh OUT
#
# OUT/ini/NAME.{xdv,pdf,log,fls}   xetex -ini -etex on tests/xetex/ini/NAME.tex
# OUT/plain/NAME.{xdv,log}        xetex -ini -etex on plain.tex + tests/plain/NAME.tex
# OUT/docs/NAME.{xdv,pdf,log,fls,aux,...}  xelatex on tests/xetex/docs/NAME.tex
#                                 and tests/latex/docs/NAME.tex (first run)
# OUT/fmt/xelatex.{log,fls}       xetex -ini -etex -jobname=xelatex *xelatex.ini
# OUT/fonts.conf, OUT/fc-list.txt the fonts xetex saw
# OUT/versions.txt
set -uo pipefail
out=$(realpath -m "${1:?OUT}")
here=$(cd "$(dirname "$0")" && pwd)
flow=$(cd "$here/../.." && pwd)
mkdir -p "$out"/{ini,plain,docs,fmt}
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
dist=$(kpsewhich -var-value TEXMFDIST)
cat > "$out/fonts.conf" <<EOF
<?xml version="1.0"?>
<!DOCTYPE fontconfig SYSTEM "fonts.dtd">
<fontconfig>
  <dir>$dist/fonts/opentype</dir>
  <dir>$dist/fonts/truetype</dir>
  <cachedir>$out/fc-cache</cachedir>
</fontconfig>
EOF
export FONTCONFIG_FILE="$out/fonts.conf"
fc-cache -f >/dev/null 2>&1 || true
fc-list -f '%{file}\t%{index}\t%{family}\t%{style}\n' :outline=true | sort > "$out/fc-list.txt"
{ xetex --version | head -1; xdvipdfmx --version 2>&1 | head -1; echo "TEXMFDIST $dist";
  echo "xelatex.ini $(kpsewhich xelatex.ini)"; echo "fonts $(wc -l < "$out/fc-list.txt")"; } > "$out/versions.txt"
tmp=$(mktemp -d)
run() { # dir name engine-args...
  local d=$1 n=$2; shift 2
  (cd "$d" && "$@" < /dev/null > /dev/null 2>&1) || true
}
# IniTeX documents
for f in "$flow"/tests/xetex/ini/*.tex; do
  n=$(basename "$f" .tex)
  for mode in xdv pdf; do
    d=$tmp/ini-$n-$mode; mkdir -p "$d"; cp "$flow"/tests/xetex/ini/* "$d/"
    if [ $mode = xdv ]; then
      run "$d" "$n" xetex -ini -etex -interaction=batchmode -recorder -no-pdf "$n.tex"
      cp "$d/$n.xdv" "$d/$n.log" "$d/$n.fls" "$out/ini/" 2>/dev/null
    else
      run "$d" "$n" xetex -ini -etex -interaction=batchmode "$n.tex"
      cp "$d/$n.pdf" "$out/ini/" 2>/dev/null
    fi
  done
done
# plain TeX's documents (tests/plain.rs's texts)
python3 - "$flow" "$tmp/plain" <<'EOF'
import sys, os
flow, outd = sys.argv[1], sys.argv[2]
os.makedirs(outd, exist_ok=True)
s = open(f"{flow}/tests/dvi/plaintext.tex", encoding="latin-1").read()
at = s.index("\\output={}\n")
h = open(f"{flow}/tests/hyph/hyphen.tex", encoding="latin-1").read()
plain = s[:at].replace("% (\\input hyphen: no hyphenation patterns on the graph yet)\n", h)
for name in ["story", "pages", "display", "table", "hyphens"]:
    b = open(f"{flow}/tests/plain/{name}.tex", encoding="latin-1").read()
    open(f"{outd}/{name}.tex", "w", encoding="latin-1").write(plain + b)
    open(f"{outd}/{name}-default.tex", "w", encoding="latin-1").write(plain + "\\output={}\n" + b)
EOF
for f in "$tmp"/plain/*.tex; do
  n=$(basename "$f" .tex)
  run "$tmp/plain" "$n" xetex -ini -etex -interaction=batchmode -no-pdf "$n.tex"
  cp "$tmp/plain/$n.xdv" "$tmp/plain/$n.log" "$out/plain/" 2>/dev/null
done
# xelatex's format, as fmtutil makes it
d=$tmp/fmt; mkdir -p "$d"
run "$d" xelatex xetex -ini -etex -jobname=xelatex -recorder -interaction=batchmode '*xelatex.ini'
cp "$d/xelatex.log" "$d/xelatex.fls" "$out/fmt/" 2>/dev/null
# LaTeX documents under xelatex (first run, no .aux)
for f in "$flow"/tests/xetex/docs/*.tex "$flow"/tests/latex/docs/*.tex; do
  n=$(basename "$f" .tex)
  [ "$n" = umath-body ] && continue
  src=$(dirname "$f")
  for mode in xdv pdf; do
    d=$tmp/doc-$n-$mode; mkdir -p "$d"
    cp "$f" "$d/"; cp "$src"/"$n"-* "$d/" 2>/dev/null; [ "$n" = umath ] && cp "$src/umath-body.tex" "$d/"
    [ "$src" = "$flow/tests/xetex/docs" ] && cp "$src"/xpic-* "$d/"
    if [ $mode = xdv ]; then
      run "$d" "$n" xelatex -interaction=batchmode -recorder -no-pdf "$n.tex"
      for x in "$d/$n".*; do case $x in *.tex) ;; *) cp "$x" "$out/docs/" ;; esac; done
    else
      run "$d" "$n" xelatex -interaction=batchmode "$n.tex"
      cp "$d/$n.pdf" "$out/docs/" 2>/dev/null
    fi
  done
done
rm -rf "$tmp"
{ echo "xetex oracles in $out"; cat "$out/versions.txt";
  for k in ini plain docs; do echo "$k: $(ls "$out/$k" | grep -c 'xdv$') xdv, $(ls "$out/$k" | grep -c 'pdf$') pdf"; done
  ls "$out/fmt"; } | tee "$out/summary-bless.txt"
