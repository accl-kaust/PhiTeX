#!/bin/bash
# (temporary) specwin2: the blank-line TikZ series at -j16, runs per node and the wakes behind them.
# $1: base commit for crates (optional); NS: sizes (150 300); LOG=1: a run/wake log of the first size.
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1 PHITEX_TIME_LIMIT_SECS=0
R=$PWD; mkdir -p $R/target/abw
if [ -n "${1:-}" ]; then git checkout "$1" -- crates Cargo.lock || { echo NOBASE; exit 1; }; echo "built from $1"; fi
cargo build -q --release -p phitex-cli --features phitex-tex/profile,phitex-tex/observe 2>&1 | tail -1
B=$R/target/release/phitex
w=$(mktemp -d -p $R/target/abw); cp $R/bench/tikzsizes/tikz200.tex $w/; (cd $w; $B build -j 1 tikz200.tex > /dev/null 2>&1)
for n in ${NS:-150 300}; do for j in ${JS:-16}; do
  w=$(mktemp -d -p $R/target/abw); cp $R/bench/tikzvar/blank$n.tex $w/; cd $w
  s0=$(date +%s%N); $B build -vv -j $j blank$n.tex > o.txt 2>&1; t=$(( ($(date +%s%N)-s0)/1000000 ))
  echo "blank $n j$j ${t} ms: $(grep -o '[0-9]* nodes run of [0-9]* live, [0-9]* runs' o.txt | tail -1) md5 $(md5sum < blank$n.pdf | cut -c1-12)"
  cd $R
done; done
if [ -n "${LOG:-}" ]; then
  n=$(echo ${NS:-150} | cut -d' ' -f1)
  w=$(mktemp -d -p $R/target/abw); cp $R/bench/tikzvar/blank$n.tex $w/; cd $w
  PHITEX_RUN_LOG=1 PHITEX_WAKE_LOG=1 $B build -vv -j 16 blank$n.tex > log.txt 2>&1
  echo "logged: $(grep -o '[0-9]* nodes run of [0-9]* live, [0-9]* runs' log.txt | tail -1)"
  gzip -c log.txt > $r/log.txt.gz; cd $R
fi
