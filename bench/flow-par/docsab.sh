#!/bin/bash
# bench/flow-par/docsab.sh NAME=REV...: latex_docs tests TESTS one at a time per
# revision (timeout each), and tikz's CLI output on each
set -u
R=${r:-/tmp/docsab-results}; W=${w:-/tmp/docsab-work}/dab; mkdir -p "$R" "$W"
TESTS=${TESTS:-"doc_ams doc_href"}
log() { echo "$*" | tee -a "$R/summary.txt"; }
for spec in "$@"; do
  name=${spec%%=*}; rev=${spec#*=}
  git cat-file -e "$rev^{commit}" 2>/dev/null || git fetch -q origin "refs/wip/$rev" 2>/dev/null || git fetch -q origin "+refs/wip/*:refs/wip/*"
  git worktree add -q --detach "$W/wt-$name" "$rev"
  for t in $TESTS; do
    (cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" timeout 900 cargo test --release -p phitex-flow --test latex_docs -- $t --exact --test-threads=1) > "$R/$name-$t.log" 2>&1
    log "$name ($(git rev-parse --short $rev)) $t: status $? $(grep -E '^test result' $R/$name-$t.log | tail -1)"
    grep -E 'panicked' -A4 "$R/$name-$t.log" | head -8 | sed 's/^/    /' | tee -a "$R/summary.txt"
  done
done
# tikz on the last revision: the CLI's output (the panic) and each round's
# triggers (PHITEX_SCHED)
(cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" cargo build --release -q -p phitex-flow --bin phitex-flow) > "$R/build-bin.log" 2>&1
B=$W/t-$name/release/phitex-flow
fmt=$W/latex.fmt
"$B" $(kpsewhich latex.ini) $(bench/flow-par/dirsof.sh sections) --jobname latex --terminal latex.ini -j 8 --dump-fmt $fmt -o $W/fmt.dvi > "$R/format.out" 2>&1
PHITEX_SCHED=$R/sched-tikz.tsv J=1 timeout 900 bench/flow-par/rundoc.sh "$B" tikz $fmt $W/docs > "$R/tikz.out" 2>&1
log "tikz on $name: status $?"; grep -E 'panicked|stalled|did not settle|^cold|rounds' "$R/tikz.out" | cut -c1-600 | tee -a "$R/summary.txt"
python3 - $R/sched-tikz.tsv <<'PY' | tee -a "$R/summary.txt"
import sys
WHY={1:'file',2:'join',4:'recut',8:'endinput',16:'ends',32:'stop'}
r=0; out=[]
for l in open(sys.argv[1], errors='replace'):
    f=l.rstrip('\n').split('\t')
    if f[0]=='R': r+=1; out.append(f"R{r}(p{f[3]},{int(f[4])/1e6:.0f}ms):")
    elif f[0]=='E' and f[2]=='8': out[-1]+=f" k{f[3]}:{'+'.join(v for b,v in WHY.items() if int(f[11])&b)}{'!' if f[9]=='1' else ''}"
print('  ' + ' | '.join(out))
PY
