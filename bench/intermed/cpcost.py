# cpcost.py J1PKL J8PKL: the j8 critical path's kept runs, same worker as their enabler or not,
# against the same nodes' kept runs at j1: ns per kilo-instruction, GHz, IPC; run time on the path.
import sys, collections
sys.path.insert(0, __import__('os').path.dirname(__import__('os').path.abspath(__file__)))
import classify as C
def load(p):
    D=C.load(p); C.prepare(D); return D
A=load(sys.argv[1]); B=load(sys.argv[2])
a={(r['op'],r['pos']):r for r in A['kept'].values()}
cls,_=C.classify(B); cp,chain=C.critical_path(B,cls)
agg=collections.defaultdict(lambda:[0]*7)
for i in chain:
    r=B['runs'][i]
    if cls[i]!='kept' or 'ins' not in r: continue
    e=C.enabler(B,r)
    k='same' if e and e[1] is not None and e[1]['w']==r['w'] else ('hop' if e and e[1] is not None else 'none')
    q=a.get((r['op'],r['pos']))
    if q is None or 'ins' not in q: continue
    g=agg[k]; g[0]+=1; g[1]+=r['cpu']; g[2]+=r['ins']; g[3]+=r['cyc']; g[4]+=q['cpu']; g[5]+=q['ins']; g[6]+=q['cyc']
print(sys.argv[2].split('/')[-1])
for k,g in agg.items():
    print(f"  {k:5} n {g[0]:4} | j8 path run {g[1]/1e6:6.0f} ms  ns/kins {1e3*g[1]/g[2]:5.0f} GHz {g[3]/g[1]:.2f} IPC {g[2]/g[3]:.2f} | j1 {g[4]/1e6:6.0f} ms ns/kins {1e3*g[4]/g[5]:5.0f} GHz {g[6]/g[4]:.2f} IPC {g[5]/g[6]:.2f} | x{g[1]/max(g[4],1):.2f} instr x{g[2]/max(g[5],1):.2f}")
