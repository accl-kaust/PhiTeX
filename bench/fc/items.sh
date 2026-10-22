#!/bin/bash
# preamble edit: the items each rerun finisher reads, before (cold watch) and after
set -u
export SOURCE_DATE_EPOCH=1700000000 FORCE_SOURCE_DATE=1 NO_COLOR=1
cargo build -q --release -p phitex-cli --features phitex-tex/profile > "$r/b.log" 2>&1 || { echo build failed; grep -m3 -A8 '^error' "$r/b.log"; exit 1; }
S=$(date +%s); B=$w/it-$S; mkdir -p "$B"; bin=$B/phitex; cp target/release/phitex "$bin"
export PHITEX_CACHE_DIR=$B/tc
mkdir -p "$B/warm" && printf '%s\n' '\documentclass{article}' '\begin{document}' 'x' '\end{document}' > "$B/warm/warm.tex"
(cd "$B/warm" && "$bin" build warm.tex > /dev/null 2>&1)
E=$B/ek; mkdir -p "$E" && cp -a bench/amsldoc/. "$E/"
EK_NO_COLD=1 EK_KINDS=6 PHITEX_FIN_ITEMS=1 PHITEX_RUN_AUDIT=1 PHITEX_ROUND_ACCT=1 EDIT_STDERR="$r/e.err" timeout 900 python3 -I bench/fc/editkinds.py "$bin" "$E" amsldoc.tex 1 "$r/e.json" --exact
python3 -I - "$r/e.err" <<'PY'
import sys,re,collections
ev=0; last=collections.defaultdict(dict); runs=collections.defaultdict(list)
for line in open(sys.argv[1],errors="replace"):
    if line.startswith("acct-edit: wall"): ev+=1; continue
    m=re.match(r"fin-items: node (\d+) unit (-?\d+) item (\d+) ([0-9a-f]+) (.*)",line)
    if not m: continue
    node,unit,k,h,txt=m.groups()
    runs[(ev,node)].append((unit,int(k),h,txt))
parts=collections.defaultdict(dict)
for line in open(sys.argv[1],errors="replace"):
    pass
edited=[n for (e,n) in runs if e==1]
print("finishers reading in the edit:",len(set(edited)))
shown=0
for n in sorted(set(edited)):
    before={ (u,k):(h,t) for (u,k,h,t) in runs.get((0,n),[]) }
    after={ (u,k):(h,t) for (u,k,h,t) in runs[(1,n)] }
    diff=[x for x in after if before.get(x,("",""))[0]!=after[x][0]]
    gone=[x for x in before if x not in after]
    print(f"node {n}: {len(after)} items, {len(diff)} differ, {len(gone)} gone, units before {sorted(set(u for u,k in before))} after {sorted(set(u for u,k in after))}")
    for x in diff[:3]:
        if shown<12:
            shown+=1
            print("   before:", before.get(x,("","-"))[1][:300]); print("   after: ", after[x][1][:300])
PY
python3 -I - "$r/e.err" <<'PY2'
import sys,re,collections
ev=0; seen=collections.defaultdict(dict)
for line in open(sys.argv[1],errors="replace"):
    if line.startswith("acct-edit: wall"): ev+=1; continue
    for tag in ("fin-parts","fin-part3","fin-locals"):
        if line.startswith(tag+":"):
            m=re.match(tag+r": node (\d+) (.*)",line)
            if m: seen[(tag,m.group(1))].setdefault(ev,[]).append(m.group(2))
n=0
for (tag,node),evs in sorted(seen.items()):
    if 1 in evs and 0 in evs and evs[1][-1]!=evs[0][-1]:
        n+=1
        if n<=12:
            a=evs[0][-1]; b=evs[1][-1]
            i=next((j for j in range(min(len(a),len(b))) if a[j]!=b[j]),min(len(a),len(b)))
            print(f"{tag} node {node} differs at {i}:\n   before ...{a[max(0,i-150):i+150]}\n   after  ...{b[max(0,i-150):i+150]}")
print("differing part lines:",n)
PY2
grep -a "rerun class" "$r/e.err" | cut -c1-160
gzip -f "$r/e.err"
