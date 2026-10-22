#!/bin/bash
# exact page-moving sentence, delete (after it), preamble at j8: timelines
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/b.log" 2>&1 || { echo build failed; exit 1; }
S=$(date +%s); B=$w/tl-$S; mkdir -p "$B"; bin=$B/phitex; cp target/release/phitex "$bin"
export PHITEX_CACHE_DIR=$B/tc
mkdir -p "$B/warm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$B/warm/warm.tex"
(cd "$B/warm" && "$bin" build warm.tex > /dev/null 2>&1)
E=$B/ek; mkdir -p "$E" && cp -a bench/amsldoc/. "$E/"
EK_NO_COLD=1 EK_KINDS=3,4,6 PHITEX_TEX_TIME=1 PHITEX_ROUND_ACCT=1 EDIT_STDERR="$r/e.err" timeout 900 python3 -I bench/fc/editkinds.py "$bin" "$E" amsldoc.tex 8 "$r/e.json" --exact
awk '/^timeline \(build [234]\)/,/clock:/' "$r/e.err" | grep -v "^  *\. link\|canonical\|^  *0.0[0-9] ms" | cut -c1-150
gzip -f "$r/e.err"
