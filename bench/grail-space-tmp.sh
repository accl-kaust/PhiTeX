#!/bin/bash
# grail-tmp.sh: the user's Grail paper (xelatex, auto-detected) under
# phitex watch -j ${J:-16}: cold build, then one word typed into the body
# (line EDIT_LINE of grail.tex): every status line timestamped, the
# viewer's page 1 frame (strips) before and after, the settled builds
set -u
(cargo build --profile fastdev -p phitex-cli --features phitex-tex/profile,phitex-tex/observe 2>&1 | tail -2) || exit 1
bin=$PWD/target/fastdev/phitex
rm -rf $w/g; cp -a $w/bin/grail-src $w/g; cd $w/g
t0=$(date +%s.%N)
stamp() { while IFS= read -r l; do printf '%7.1f %s\n' "$(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{print b-a}')" "$l"; done; }
sleep 1500 | PHITEX_TEX_TIME=1 PHITEX_ROUND_ACCT=1 PHITEX_RUN_AUDIT=1 PHITEX_RERUN_SHOW=1 PHITEX_CACHE_DIR=$w/pc timeout 1400 $bin watch grail.tex -j ${J:-16} 2>&1 | stdbuf -oL tr '\r' '\n' | stamp > $r/watch.out &
for i in $(seq 1 900); do grep -q "ok: *1\|failed" $r/watch.out && break; sleep 1; done
url=$(grep -o 'http://127.0.0.1:[0-9]*/[0-9a-f]*/' $r/watch.out | head -1)
echo "viewer $url"; grep -E "ok:|failed|pages" $r/watch.out | tail -5
fr() { curl -s "${url}frame/$1/1333" | python3 -c 'import json,sys
try:
  f=json.load(sys.stdin); print("bands",len(f["bands"]),"drawn",f["drawn"],"kept",f["kept"],"us",f["us"]["all"])
except Exception as e: print("no frame", e)'; }
echo "page 1 before: $(fr 0)"; echo "page 5 before: $(fr 4)"
n=$(grep -c "ok:" $r/watch.out)
waitok() { for i in $(seq 1 1200); do [ $(grep -c "ok:" $r/watch.out) -gt $n ] && break; sleep 0.25; done; n=$(grep -c "ok:" $r/watch.out); }
mark() { echo "=== $1 at $(awk -v a="$t0" -v b="$(date +%s.%N)" 'BEGIN{printf "%.1f", b-a}')" >> $r/watch.out; }
# (1) a space at the end of line EDIT_LINE (TeX strips it: no token changes)
mark "space at end of line"; te=$(date +%s.%N)
sed -i "${EDIT_LINE:-260}s/\$/ /" grail.tex
waitok; echo "space at end of line: $(awk -v a="$te" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}') s to the build line: $(grep 'ok:' $r/watch.out | tail -1)"
sleep 3
# (2) a word typed in the same line
mark "word typed"; te=$(date +%s.%N)
sed -i "${EDIT_LINE:-260}s/ common / common typed /" grail.tex
waitok; echo "word typed: $(awk -v a="$te" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}') s to the build line: $(grep 'ok:' $r/watch.out | tail -1)"
sleep 3
# (3) a word typed late in the paper (line 1016, the last pages)
mark "late word typed"; te=$(date +%s.%N)
sed -i "1016s/ replays / replays typed /" grail.tex
waitok; echo "late word typed: $(awk -v a="$te" -v b="$(date +%s.%N)" 'BEGIN{printf "%.2f", b-a}') s to the build line: $(grep 'ok:' $r/watch.out | tail -1)"
sleep 3
kill %1 2>/dev/null; wait 2>/dev/null
echo "--- status lines (page counters) ---"
grep -E "page|ok:|settl|build" $r/watch.out | awk '!seen[$0]++' | tail -60
echo "--- the edit's accounting (acct-edit, audit, rerun causes) ---"
awk "/=== space at end of line/,0" $r/watch.out | grep -vE "^ *[0-9.]+ +(warning|Overfull|Underfull)" | cut -c1-400 | head -150
echo "--- the session's phases and the PDF link, each build ---"
grep -E "=== |session phases|xdv pdf:|ok:" $r/watch.out | cut -c1-600 | tail -40
