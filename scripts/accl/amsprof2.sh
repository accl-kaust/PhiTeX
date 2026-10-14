#!/bin/bash
# amsldoc: (1) the cold build's critical chain (relaxed, -j8);
# (2) callgrind of the cold format (preamble only, -j1).
set -u
D=/usr/share/doc/texlive/latex/amsmath
CARGO_PROFILE_RELEASE_DEBUG=line-tables-only cargo build --release -q -p phitex-cli || exit 1
mkdir -p $w/bin && cp target/release/phitex $w/bin/phitex
echo "### chain $(date +%T)"
PHITEX_T_CHAIN=1 PHITEX_SNAPSHOT_SECS=5 python3 scripts/accl/tikzman-prof.py phases $w/bin/phitex $D amsldoc.tex amsldoc.tex 374 vertically bertically 8 ams-chain
grep -E 'chain by cause|measure|=====|edit phases' $r/ams-chain-stderr.txt | cut -c1-1500
grep -E 'snapshot' $r/ams-chain-stderr.txt | cut -c1-300 | head -8
echo "### callgrind $(date +%T)"
f=$w/fmt; mkdir -p $f $w/home; cp -a $D/. $f/
python3 - $f/amsldoc.tex <<'PY'
import sys
p=sys.argv[1]; s=open(p,encoding='latin-1').read()
i=s.index('\\begin{document}')
open(p,'w',encoding='latin-1').write(s[:i]+'\\begin{document}\nx\n\\end{document}\n')
PY
export SOURCE_DATE_EPOCH=1758800000 FORCE_SOURCE_DATE=1 TZ=UTC NO_COLOR=1
(cd $f && /usr/bin/time -f "plain fmt build %e s %M kB" env HOME=$w/home PHITEX_CACHE_DIR=$w/c0 PHITEX_TEX_TIME=1 $w/bin/phitex build -j 1 amsldoc.tex 2>&1 | grep -E 'time: |prelude:|plain fmt' )
(cd $f && /usr/bin/time -f "callgrind fmt build %e s %M kB" env HOME=$w/home PHITEX_CACHE_DIR=$w/c1 valgrind --tool=callgrind --callgrind-out-file=$r/cg.out $w/bin/phitex build -j 1 amsldoc.tex > $r/cg-term.txt 2>&1; tail -3 $r/cg-term.txt)
callgrind_annotate --inclusive=yes $r/cg.out 2>/dev/null | head -70 | cut -c1-220
echo "--- self"
callgrind_annotate $r/cg.out 2>/dev/null | head -60 | cut -c1-220
echo "### done $(date +%T)"
