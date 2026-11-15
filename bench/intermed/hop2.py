# hop2.py PKL...: kept links (a kept run and the run whose commit enabled it): same worker or not,
# how the hopping ones were taken, and the per-instruction cost of each; the critical path split.
import sys, collections
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
import classify as C
MS=1e6
for p in sys.argv[1:]:
    D=C.load(p); C.prepare(D)
    runs=D['runs']
    byw=collections.defaultdict(list)
    for r in runs: byw[r['w']].append(r)
    for w in byw: byw[w].sort(key=lambda r:r['t0'])
    import bisect
    starts={w:[r['t0'] for r in L] for w,L in byw.items()}
    links=0; same=0; kinds=collections.Counter(); why=collections.Counter(); cost=collections.defaultdict(lambda:[0,0,0])
    gaps=collections.defaultdict(list)
    for r in D['kept'].values():
        e=C.enabler(D,r)
        if e is None or e[1] is None: continue
        t,x,kind=e
        if kind=='own': continue
        links+=1
        kinds[kind]+=1
        hop = x['w']!=r['w']
        tk=r.get('took','?'); tf=r.get('tfrom',-1)
        if not hop:
            same+=1; key='same:'+tk
        else:
            if tf==x['w'] and tk.startswith('steal'):
                # what the waker's worker ran meanwhile
                L=byw[x['w']]; j=bisect.bisect_right(starts[x['w']], x['t1'])
                nxt=L[j] if j<len(L) else None
                busy = 'idle' if nxt is None or nxt['t0']>r['t0'] else ('earlier' if nxt['pos']<r['pos'] else 'later')
                key='hop:'+tk+':waker-ran-'+busy
            elif tf==r['w']:
                key='hop:queued-on-runner:'+tk
            else:
                key='hop:other:'+tk
        why[key]+=1
        c=cost['hop' if hop else 'same']
        if 'ins' in r: c[0]+=r['cyc']; c[1]+=r['ins']; c[2]+=r['cpu']
        gaps['hop' if hop else 'same'].append(r['t0']-t)
    cls,_=C.classify(D)
    cp,chain=C.critical_path(D,cls)
    name=p.split('/')[-1].replace('.pkl','')
    print(f"== {name}: kept links {links}, same worker {same} ({100*same/max(links,1):.0f}%), hop {links-same} ({100*(links-same)/max(links,1):.0f}%)")
    for k,v in why.most_common(14): print(f"   {k:45} {v}")
    for k,c in cost.items():
        g=sorted(gaps[k])
        print(f"   {k}: ns/kins {1e3*c[2]/max(c[1],1):.1f} IPC {c[1]/max(c[0],1):.2f} GHz {c[0]/max(c[2],1):.2f}  wake->start p50 {g[len(g)//2]/1e3 if g else 0:.0f} us")
    print('   CP', {k:(round(v,1) if isinstance(v,float) else v) for k,v in cp.items() if k not in('enablers','run_n')}, cp['enablers'])
