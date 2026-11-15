import sys, collections
sys.path.insert(0, '/home/lambda/code/tmp/intermed/bench/intermed')
import classify as C
D = C.load(sys.argv[1]); C.prepare(D)
cls, det = C.classify(D)
runs = D['runs']
print('quiets', [(round(t/1e6,1), p) for t,p in D['quiets']])
prevq = 0
for t, p in D['quiets']:
    seg = [r for r in runs if prevq <= r['t0'] < t]
    if not seg: prevq = t; continue
    kept_end = max(((r['done'] or r['t1']) for r in seg if cls[r['i']] in ('kept','d:next pass')), default=prevq)
    tail = [r for r in seg if (r['done'] or r['t1']) > kept_end]
    last = max(seg, key=lambda r: (r['done'] or r['t1']))
    print('segment', round(prevq/1e6), '-', round(t/1e6), 'runs', len(seg), 'last kept/pass-kept end', round(kept_end/1e6), 'tail ms', round((t-kept_end)/1e6),
          'tail runs by class', dict(collections.Counter(cls[r['i']] for r in tail)), 'last run', last['op'], cls[last['i']], round((last['done'] or last['t1'])/1e6))
    prevq = t
