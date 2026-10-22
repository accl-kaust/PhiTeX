#!/bin/bash
# the predicate read: clippy, cold md5s, exact audit of the page-moving, delete (after it) and preamble edits at j8
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
git log --oneline -1 | cut -c1-80
cargo clippy -q --workspace --exclude defuse --no-deps --all-targets -- -D warnings > "$r/clippy.log" 2>&1; echo "clippy exit $? $(grep -c '^error\|^warning' $r/clippy.log)"; grep -m5 -A12 '^error\|^warning' "$r/clippy.log" | head -60
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/b.log" 2>&1 || { echo build failed; grep -m3 -A12 '^error' "$r/b.log"; exit 1; }
S=$(date +%s); B=$w/pr-$S; mkdir -p "$B"; bin=$B/phitex; cp target/release/phitex "$bin"
export PHITEX_CACHE_DIR=$B/tc
mkdir -p "$B/warm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$B/warm/warm.tex"
(cd "$B/warm" && "$bin" build warm.tex > /dev/null 2>&1)
for j in 1 8; do D=$B/c-$j; mkdir -p $D && cp -a bench/amsldoc/. $D/ && (cd $D && "$bin" build amsldoc.tex -j $j > /dev/null 2>&1); echo "cold amsldoc j$j md5 $(md5sum < $D/amsldoc.pdf | cut -c1-8)"; done
D=$B/p; mkdir -p $D && cp bench/para4000.tex $D/ && (cd $D && "$bin" build para4000.tex -j 8 > /dev/null 2>&1); echo "cold para4000 md5 $(md5sum < $D/para4000.pdf | cut -c1-8)"
E=$B/ek; mkdir -p "$E" && cp -a bench/amsldoc/. "$E/"
EK_KINDS=3,4,6 PHITEX_RUN_AUDIT=1 PHITEX_ROUND_ACCT=1 EDIT_STDERR="$r/a.err" timeout 900 python3 -I bench/fc/editkinds.py "$bin" "$E" amsldoc.tex 8 "$r/a.json" --exact
awk '/^acct-edit: wall/{n++} n>=2' "$r/a.err" | grep -a '^acct-edit: wall\|^acct-edit: audit\|rerun class' | cut -c1-220 | head -90
gzip -f "$r/a.err"
