import sys, collections, bisect
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
import classify as C
MS=1e6
D=C.load(sys.argv[1]); C.prepare(D); cls,_=C.classify(D)
runs=D['runs']; T0=min(r['t0'] for r in runs)
byw=collections.defaultdict(list)
for r in runs: byw[r['w']].append(r)
for w in byw: byw[w].sort(key=lambda r:r['t0'])
def during(w,a,b):
    return [r for r in byw[w] if r['t1']>a and r['t0']<b]
cp,chain=C.critical_path(D,cls)
L=[]; tot=collections.Counter()
for i in chain:
    cur=runs[i]; e=C.enabler(D,cur)
    if e is None or e[1] is None: continue
    t,x,kind=e
    if kind=='stale' and cur['k']>0:
        pr=D['byn'][cur['node']][cur['k']-1]; a=max(t,min(pr['done'] or pr['t1'],cur['t0']))
    else: a=t
    g=cur['t0']-a
    if g<=0: continue
    rb=during(cur['w'],a,cur['t0'])
    busy=sum(min(r['t1'],cur['t0'])-max(r['t0'],a) for r in rb)
    idle=sum(g-sum(min(r['t1'],cur['t0'])-max(r['t0'],a) for r in during(w,a,cur['t0'])) for w in byw)
    earlier=sum(1 for r in rb if r['pos']<cur['pos'])
    cat=('runner-busy' if busy>0.5*g else 'runner-free')+('/others-idle' if idle>0.5*g else '/all-busy')
    tot[cat]+=g; tot[cat+'#']+=1
    L.append((g,kind,cur,x,busy,idle,len(rb),earlier))
print({k:(round(v/MS,1) if not k.endswith('#') else v) for k,v in tot.items()})
L.sort(key=lambda z:-z[0])
for g,kind,r,x,busy,idle,nrb,earlier in L[:int(sys.argv[2])]:
    print(f"q {g/MS:.2f} ms {kind} r n{r['node']} {r['op']} pos{r['pos']} w{r['w']} {r.get('took')}/{r.get('tfrom')} start {(r['t0']-T0)/MS:.1f} | x w{x['w']} n{x['node']} {x['op']} | runner busy {busy/g:.0%} with {nrb} runs ({earlier} earlier pos) | idle worker-time {idle/g:.1f}")
