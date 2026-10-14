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
echo "### done $(date +%T)"
