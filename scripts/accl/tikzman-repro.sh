#!/bin/bash
# Repro candidates (agent tikzman): each DIR/*.tex built by phitex (-v,
# --exact-pdf) and by pdflatex (one pass each), in the harness's pinned
# environment; prints errors and whether the PDFs are identical.
#   tikzman-repro.sh BIN DIR
set -u
bin=$1 dir=$2
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC LANG=C.UTF-8 LC_ALL=C.UTF-8 \
  shell_escape=f openout_any=p openin_any=p NO_COLOR=1 HOME=$w/home PHITEX_CACHE_DIR=$w/cache
mkdir -p $HOME
for f in "$dir"/*.ini.tex "$dir"/[!k]*.tex; do [ -f "$f" ] || continue; case $f in *in.tex) continue;; esac
  n=$(basename "$f" .tex)
  d=$w/repro/$n; rm -rf $d; mkdir -p $d/o $d/p
  cp "$f" $d/o/; cp "$f" $d/p/; for x in "$dir"/${n%.ini}in.tex; do [ -f "$x" ] && cp "$x" $d/o/ && cp "$x" $d/p/; done
  case $n in
    *.ini) # (an -ini job: pdfTeX's -ini -etex on both sides)
      (cd $d/o && timeout 300 pdftex -ini -etex -interaction=nonstopmode $n.tex > term.txt 2>&1)
      (cd $d/p && timeout 600 "$bin" build --ini --etex --output-format=pdf -v --exact-pdf -j 4 $n.tex > term.txt 2>&1; echo "exit $?" >> term.txt)
      echo "  oracle messages: $(grep -o '\[[^]]*\]' $d/o/$n.log | head -5 | tr '\n' ' ')"
      echo "  phitex messages: $(grep -o '\[[^]]*\]' $d/p/$n.log | head -5 | tr '\n' ' ')" ;;
    *)
      (cd $d/o && timeout 300 pdflatex -interaction=nonstopmode $n.tex > term.txt 2>&1)
      (cd $d/p && timeout 600 "$bin" build -v --exact-pdf -j 4 $n.tex > term.txt 2>&1; echo "exit $?" >> term.txt) ;;
  esac
  oe=$(grep -c '^! ' $d/o/$n.log 2>/dev/null); pe=$(grep -c '^! ' $d/p/$n.log 2>/dev/null)
  if cmp -s $d/o/$n.pdf $d/p/$n.pdf; then same=identical; else same=DIFFERS; fi
  echo "=== $n: pdflatex errors ${oe:-?}, phitex errors ${pe:-?}, PDF $same"
  grep -A3 '^! ' $d/o/$n.log 2>/dev/null | head -8 | sed 's/^/  oracle| /'
  grep -v '^ *$' $d/p/term.txt | head -40 | sed 's/^/  phitex| /'
  mkdir -p $r/repro/$n; cp $d/p/term.txt $r/repro/$n/; cp $d/p/$n.log $r/repro/$n/phitex.log 2>/dev/null; cp $d/o/$n.log $r/repro/$n/oracle.log 2>/dev/null
done
