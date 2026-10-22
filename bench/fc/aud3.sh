#!/bin/bash
# exact edits at j8 under the run audit: every rerun by trigger, wasted or not; @abspage@last's values
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
git log --oneline -1 | cut -c1-80
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/b.log" 2>&1 || { echo build failed; grep -m3 -A8 '^error' "$r/b.log"; exit 1; }
S=$(date +%s); B=$w/a3-$S; mkdir -p "$B"; bin=$B/phitex; cp target/release/phitex "$bin"
export PHITEX_CACHE_DIR=$B/tc
mkdir -p "$B/warm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$B/warm/warm.tex"
(cd "$B/warm" && "$bin" build warm.tex > /dev/null 2>&1)
for k in ${KINDS:-3 4 6}; do
  E=$B/ek-$k; mkdir -p "$E" && cp -a bench/amsldoc/. "$E/"
  echo "== kind $k"
  EK_NO_COLD=1 EK_KINDS=$k PHITEX_RUN_AUDIT=1 PHITEX_ROUND_ACCT=1 PHITEX_RERUN_VALUES=abspage PHITEX_KEYS_SHOW=abspage EDIT_STDERR="$r/a-$k.err" timeout 900 python3 -I bench/fc/editkinds.py "$bin" "$E" amsldoc.tex 8 "$r/a-$k.json" --exact
  awk '/^acct-edit: wall/{n++} n>=2' "$r/a-$k.err" | grep -a '^acct-edit: wall\|^acct-edit: audit\|rerun class\|rerun value\|^keys-read' | cut -c1-200 | head -80
  gzip -f "$r/a-$k.err"
done
