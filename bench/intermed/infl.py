import sys, collections
sys.path.insert(0, '/home/lambda/code/tmp/intermed/bench/intermed')
import classify as C
def kept(p):
    D=C.load(p); C.prepare(D)
    return {(r['op'], r['pos']): r for r in D['kept'].values()}, D
a, Da = kept(sys.argv[1]); b, Db = kept(sys.argv[2])
common = set(a) & set(b)
print('kept j1', len(a), 'j8', len(b), 'common', len(common))
by = collections.defaultdict(lambda: [0,0,0])
for k in common:
    by[k[0]][0]+=a[k]['cpu']; by[k[0]][1]+=b[k]['cpu']; by[k[0]][2]+=1
for op,(x,y,n) in sorted(by.items(), key=lambda z:-z[1][1]):
    print(op, n, round(x/1e6), round(y/1e6), round(y/max(x,1),2))
sa=sum(v[0] for v in by.values()); sb=sum(v[1] for v in by.values()); print('all', round(sa/1e6), round(sb/1e6), round(sb/sa,2))
# reads count equal?
same=sum(1 for k in common if len(a[k]['reads'])==len(b[k]['reads']))
print('same #reads', same)
