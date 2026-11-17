# cpq.py PKL: the critical path's links: by enabler kind, hop or not, how taken; the queue gaps:
# what the waker's and the runner's workers did in the gap and how many workers were idle.
import sys, collections, bisect
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
import classify as C
MS=1e6
for p in sys.argv[1:]:
    D=C.load(p); C.prepare(D); cls,_=C.classify(D)
    runs=D['runs']; byw=collections.defaultdict(list)
    for r in runs: byw[r['w']].append(r)
    for w in byw: byw[w].sort(key=lambda r:r['t0'])
    nw=len(byw)
    def busy(w,a,b):
        s=0
        for r in byw[w]:
            if r['t1']<=a: continue
            if r['t0']>=b: break
            s+=min(r['t1'],b)-max(r['t0'],a)
        return s
    cp,chain=C.critical_path(D,cls)
    agg=collections.Counter(); n=collections.Counter(); gq=collections.Counter()
    hops=0; links=0; ins=collections.Counter()
    for i in chain:
        cur=runs[i]
        e=C.enabler(D,cur)
        if e is None or e[1] is None: continue
        t,x,kind=e
        links+=1
        hop=x['w']!=cur['w']; hops+=hop
        tk=cur.get('took','?')
        gap=cur['t0']-t
        key=f"{kind}/{'hop' if hop else 'same'}/{tk}"
        n[key]+=1; agg[key]+=gap
        if gap>50000:
            rb=busy(cur['w'],t,cur['t0'])/gap; wb=busy(x['w'],t,cur['t0'])/gap
            idle=sum(1-busy(w,t,cur['t0'])/gap for w in byw)
            gq['runner busy']+=rb*gap; gq['waker busy']+=wb*gap; gq['idle worker-ms']+=idle*gap; gq['gap']+=gap
        if 'ins' in cur:
            k='hop' if hop else 'same'
            ins[k+'cpu']+=cur['cpu']; ins[k+'ins']+=cur['ins']; ins[k+'cyc']+=cur['cyc']
    name=p.split('/')[-1].replace('.pkl','')
    print(f"== {name} end {cp['end_ms']:.0f} ms  run {sum(cp['run_ms'].values()):.0f} (kept {cp['run_ms'].get('kept',0):.0f}) queue {cp['queue_ms']:.0f} commit {cp['commit_ms']:.0f} quiet {cp['quiet_ms']:.0f} stale {cp['stale_ms']:.0f} late {cp['late_ms']:.0f}; links {links}, hop {hops} ({100*hops/max(links,1):.0f}%)")
    for k,v in sorted(agg.items(), key=lambda z:-z[1])[:12]: print(f"   {k:32} n {n[k]:4}  gap {v/MS:7.1f} ms")
    g=gq['gap'] or 1
    print(f"   gaps>50us: {g/MS:.0f} ms; runner busy {100*gq['runner busy']/g:.0f}%, waker busy {100*gq['waker busy']/g:.0f}%, mean idle workers {gq['idle worker-ms']/g:.1f} of {nw}")
    for k in ('same','hop'):
        if ins[k+'ins']: print(f"   on-path {k}: ns/kins {1e3*ins[k+'cpu']/ins[k+'ins']:.0f} IPC {ins[k+'ins']/ins[k+'cyc']:.2f} GHz {ins[k+'cyc']/ins[k+'cpu']:.2f}")
