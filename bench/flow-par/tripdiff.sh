#!/bin/bash
# bench/flow-par/tripdiff.sh NAME=REV NAME=REV: TRIP (phitex-flow --trip, 1 thread)
# per revision with the scheduler's events and dependencies; the DVI
# listings' first difference, the rounds, and the reads whose values
# differ between the two (piece, name, reading node, source piece).
set -u
R=${r:-/tmp/tripdiff-results}; W=${w:-/tmp/tripdiff-work}/td; mkdir -p "$R" "$W"
log() { echo "$*" | tee -a "$R/summary.txt"; }
names=()
for spec in "$@"; do
  name=${spec%%=*}; rev=${spec#*=}; names+=("$name")
  git cat-file -e "$rev^{commit}" 2>/dev/null || git fetch -q origin "refs/wip/$rev" 2>/dev/null || git fetch -q origin "+refs/wip/*:refs/wip/*"
  git worktree add -q --detach "$W/wt-$name" "$rev"
  (cd "$W/wt-$name" && CARGO_TARGET_DIR="$W/t-$name" cargo build --release -q -p phitex-flow --bin phitex-flow) > "$R/build-$name.log" 2>&1 || { log "build $name failed"; tail -20 "$R/build-$name.log"; }
  rm -rf $W/trip-$name; cp -a bench/trip $W/trip-$name
  (cd $W/trip-$name && PHITEX_SCHED=$R/sched-$name.tsv PHITEX_SCHED_DEPS=1 timeout 600 "$W/t-$name/release/phitex-flow" --trip $W/trip-$name 1) > "$R/run-$name.txt" 2>&1
  log "$name ($(git rev-parse --short $rev)): status $?; $(grep -c '^R' $R/sched-$name.tsv) rounds over all builds; dvi $(ls -l $W/trip-$name/trip.dvi 2>/dev/null | awk '{print $5}') bytes"
  dvitype -output-level=2 -dpi=72.27 -page-start='*.*.*.*.*.*.*.*.*.*' $W/trip-$name/trip.dvi > $R/typ-$name.txt 2>&1 || log "dvitype failed for $name"
  cp $W/trip-$name/trip.log $R/trip-$name.log 2>/dev/null
done
a=${names[0]}; b=${names[1]}
log "first DVI listing difference:"; diff $R/typ-$a.txt $R/typ-$b.txt | head -20 | tee -a "$R/summary.txt"
log "trip.log difference:"; diff $R/trip-$a.log $R/trip-$b.log | head -30 | tee -a "$R/summary.txt"
python3 - $R/sched-$a.tsv $R/sched-$b.tsv <<'PY' | tee -a "$R/summary.txt"
import sys, collections
def load(p):
    builds=[]; cur=None; rounds=collections.Counter(); b=0
    for l in open(p, errors='replace'):
        f=l.rstrip('\n').split('\t')
        if f[0]=='R': b=int(f[1]); rounds[b]+=1; cur=None
        elif f[0] in 'GDX':
            if cur is None: cur=dict(build=b, G={}, D={}, X={}); builds.append(cur)
            if f[0]=='G': cur['G'][int(f[1])]=f[2:]
            elif f[0]=='D': cur['D'][(int(f[1]), f[4])]=(f[2], f[3], f[6] if len(f)>6 else '', f[8] if len(f)>8 else '')
            else: cur['X'][(int(f[1]), '\t'.join(f[2:-1]))]=f[-1]
    return builds, rounds
A, ra = load(sys.argv[1]); B, rb = load(sys.argv[2])
print('rounds per build:', dict(ra), 'vs', dict(rb))
for x, y in zip(A, B):
    print(f"build {x['build']}: pieces {len(x['G'])} vs {len(y['G'])}")
    ks = sorted(set(x['D']) | set(y['D']))
    diff = [k for k in ks if x['D'].get(k, ('','','',''))[2] != y['D'].get(k, ('','','',''))[2] or x['D'].get(k, ('',))[0] != y['D'].get(k, ('',))[0]]
    print(f"  reads differing (value or source): {len(diff)}")
    for k in diff[:25]:
        print(f"    piece {k[0]} {k[1]}: {x['D'].get(k)} vs {y['D'].get(k)}")
    xd = [k for k in sorted(set(x['X']) | set(y['X'])) if x['X'].get(k) != y['X'].get(k)]
    print(f"  exports differing: {len(xd)}: " + ', '.join(f'{k[0]}:{k[1]}' for k in xd[:25]))
PY
