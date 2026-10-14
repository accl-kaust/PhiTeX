#!/bin/bash
# amscorrect (tooling, not for merge): build phitex, pdflatex's amsldoc to its
# fixpoint, phitex build --exact-pdf at -j1 and -j8; PDFs and logs to $r.
# Extra docs: AMSC_DOCS="name ..." (bench/amsc/NAME.tex) built the same way.
set -u
here=$PWD
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
t0=$(date +%s); cargo build -q --profile fastdev -p phitex-cli 2>&1 | grep -E "^error" -A8 | head -30
echo "build $(( $(date +%s)-t0 )) s"; P=$here/target/fastdev/phitex
cp $P $r/phitex 2>/dev/null
which pdflatex && pdflatex --version | head -1
for doc in ${AMSC_DOCS:-amsldoc}; do
  o=$w/oracle-$doc; mkdir -p $o; cp bench/amsc/$doc.tex $o/
  if [ "${AMSC_ORACLE:-1}" = 1 ]; then
    (cd $o && for i in 1 2 3 4 5; do cp $doc.aux prev.aux 2>/dev/null; pdflatex -interaction=nonstopmode $doc.tex > term-$i.txt 2>&1; cmp -s $doc.aux prev.aux && break; done; echo "oracle $doc: $i passes")
    cp $o/$doc.pdf $r/$doc-oracle.pdf; cp $o/$doc.log $r/$doc-oracle.log
  fi
  for j in ${AMSC_J:-1 8}; do
    d=$w/phi-$doc-j$j; mkdir -p $d; cp bench/amsc/$doc.tex $d/
    (cd $d && PHITEX_CACHE_DIR=$w/cache /usr/bin/time -f "WALL %e BUSY %U+%S RSS %M" timeout 1200 $P build -v -j $j --exact-pdf $doc.tex > $r/$doc-j$j.term 2>&1)
    cp $d/$doc.pdf $r/$doc-j$j.pdf 2>/dev/null; cp $d/$doc.log $r/$doc-j$j.log 2>/dev/null
    echo "== $doc j$j: $(grep -oE 'WALL.*' $r/$doc-j$j.term) | errors $(grep -c '^error' $r/$doc-j$j.term)"
    [ -f $r/$doc-oracle.pdf ] && { cmp $r/$doc-oracle.pdf $r/$doc-j$j.pdf && echo "   IDENTICAL"; }
  done
done
echo done
