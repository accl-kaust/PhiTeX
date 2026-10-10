#!/bin/bash
# bench/flow-par/pgfrounds.sh: the pgf manual's first pdflatex run (cold, no .aux)
# through `phitex --compat=pdflatex -threads=1`, with PHITEX_SCHED: rounds,
# files discovered (settled runs reading a file), each round's wall and
# setup, the last (inline) round. CAPS: round caps to try (PHITEX_ROUND_CAP).
set -u
R=${r:-/tmp/pgfr-results}; W=${w:-/tmp/pgfr-work}/pgfr; mkdir -p "$R" "$W"
SRC=${SRC:-$PWD/bench/flow-par/pgfman-src}
CAPS=${CAPS:-"300"}; LIMIT=${LIMIT:-2400}
export SOURCE_DATE_EPOCH=0 FORCE_SOURCE_DATE=1
log() { echo "$*" | tee -a "$R/summary.txt"; }
log "commit $(git rev-parse --short HEAD); node $(cat /proc/sys/kernel/hostname); $(ls $SRC | wc -l) source files"
cargo build --release -q -p phitex-cli > "$R/build.log" 2>&1 || { log "build failed"; tail -30 "$R/build.log"; exit 1; }
B=$PWD/target/release/phitex
for cap in $CAPS; do
  rm -rf $W/run; cp -a $SRC $W/run
  f=$R/sched-cap$cap.tsv; rm -f $f
  read s rss st < <(cd $W/run && PHITEX_ROUND_CAP=$cap PHITEX_SCHED=$f python3 $OLDPWD/bench/flow-par/meas.py $R/out-cap$cap.txt timeout $LIMIT $B --compat=pdflatex -threads=1 -interaction=batchmode pgfmanual.tex)
  log "cap $cap: ${s}s maxrss ${rss}KB status $st (124 = timeout); pdf $(ls -l $W/run/pgfmanual.pdf 2>/dev/null | awk '{print $5}') bytes"
  tail -5 $W/run/pgfmanual.log 2>/dev/null | sed 's/^/    log: /' >> "$R/summary.txt"
  python3 - "$f" <<'PY' | tee -a "$R/summary.txt"
import sys, collections
rounds=[]; cur=None
for l in open(sys.argv[1], errors='replace'):
    f=l.rstrip('\n').split('\t')
    if f[0]=='R':
        cur=dict(build=int(f[1]), pieces=int(f[3]), wall=int(f[4])/1e6, setup=int(f[8])/1e6 if len(f)>8 else 0, trig=[]); rounds.append(cur)
    elif f[0]=='E' and cur is not None and f[2]=='8':
        cur['trig'].append(int(f[11]))
by=collections.defaultdict(list)
for r in rounds: by[r['build']].append(r)
for b, rs in by.items():
    files=sum(1 for r in rs for c in r['trig'] if c & 1)
    walls=[r['wall'] for r in rs]
    print(f"  build {b}: {len(rs)} rounds, pieces at the end {rs[-1]['pieces']}, settled file reads (round triggers) {files}; rounds' walls: total {sum(walls):.0f} ms, all but the last {sum(walls[:-1]):.0f} ms (median {sorted(walls[:-1])[len(walls[:-1])//2] if len(walls)>1 else 0:.1f}, max {max(walls[:-1]) if len(walls)>1 else 0:.0f}), the last {walls[-1]:.0f} ms; setups {sum(r['setup'] for r in rs):.0f} ms")
    print('    walls: ' + ' '.join(f"{w:.0f}" for w in walls))
PY
  { echo "  slow nodes (five per run over 1 s):"; zcat -f $f | grep -P "^N\t" | sort -t"	" -k3,3nr | head -15 | cut -c1-300; } | tee -a "$R/summary.txt"
  gzip -f $f
done
