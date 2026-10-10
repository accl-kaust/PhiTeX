#!/bin/bash
# bench/flow-par/accept.sh BEFORE AFTER: clippy and the suite on this head, then
# tikz and biglatex at -j 1 before/after (rounds), pinned
set -u
R=${r:-/tmp/accept-results}; mkdir -p "$R"
log() { echo "$*" | tee -a "$R/summary.txt"; }
t0=$(date +%s)
cargo clippy -q -p phitex-flow --all-targets -- -D warnings > "$R/clippy.log" 2>&1; log "clippy status $? ($(( $(date +%s) - t0 )) s)"
grep -E '^(error|warning)' -A6 "$R/clippy.log" | head -30 >> "$R/summary.txt"
for t in input trip etrip flow dvi design order io output; do
  cargo test --release -q -p phitex-flow --test $t -- --test-threads=4 > "$R/test-$t.log" 2>&1
  log "test $t status $? $(grep -E '^test result' $R/test-$t.log | tail -1) ($(( $(date +%s) - t0 )) s)"
done
for t in fuzz latex_docs; do
  timeout 2400 cargo test --release -q -p phitex-flow --test $t -- --test-threads=2 > "$R/test-$t.log" 2>&1
  log "test $t status $? $(grep -E '^test result' $R/test-$t.log | tail -1) ($(( $(date +%s) - t0 )) s)"
done
grep -hE 'panicked' -A3 $R/test-*.log | head -30 | tee -a "$R/summary.txt"
# TRIP's rounds on this head, with the round triggers
cargo build --release -q -p phitex-flow --bin phitex-flow > "$R/build-bin.log" 2>&1
rm -rf ${w:-/tmp}/tripr; cp -a bench/trip ${w:-/tmp}/tripr
(cd ${w:-/tmp}/tripr && PHITEX_SCHED=$R/sched-trip.tsv timeout 600 $OLDPWD/target/release/phitex-flow --trip ${w:-/tmp}/tripr 1) > "$R/trip-run.txt" 2>&1
log "TRIP (1 thread): status $?, $(grep -c '^R' $R/sched-trip.tsv) rounds"
python3 - $R/sched-trip.tsv <<'PY' | tee -a "$R/summary.txt"
import sys
WHY={1:'file',2:'join',4:'recut',8:'endinput',16:'ends',32:'stop'}
r=0; out=[]
for l in open(sys.argv[1], errors='replace'):
    f=l.rstrip('\n').split('\t')
    if f[0]=='R': r+=1; out.append(f"R{r}(b{f[1]},p{f[3]},{int(f[4])/1e6:.0f}ms):")
    elif f[0]=='E' and f[2]=='8': out[-1]+=f" k{f[3]}:{'+'.join(v for b,v in WHY.items() if int(f[11])&b)}{'!' if f[9]=='1' else ''}"
print('  ' + ' | '.join(out))
PY
DOCS="tikz biglatex" JS="1" REPS=1 bench/flow-par/abdocs.sh before=$1 after=$2
