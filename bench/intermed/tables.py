import json
docs=['amsldoc','acm','grail']
def L(d,m,j): return json.load(open(f'log-{d}-{m}-j{j}.json'))
def grp(c):
    out={'kept':[0,0],'a':[0,0],'b':[0,0],'c':[0,0],'d:gone':[0,0],'d:next pass':[0,0],'d:other':[0,0]}
    for k,v in c.items():
        if k=='w': continue
        g = k if k in out else ('c' if k.startswith('c') else 'd:other')
        out[g][0]+=v['runs']; out[g][1]+=v['cpu_ms']
    return out
print('| doc | build | -j | runs | kept | (a) intermediate | (b) final | (c) stopped/parked | (d) claimed or removed | (d) next pass | (d) other |')
print('|---|---|---|---|---|---|---|---|---|---|---|')
for d in docs:
  for m in ['clean','second']:
    for j in [1,8]:
      x=L(d,m,j); g=grp(x['classes'])
      cells=[f"{g[k][0]} / {g[k][1]:.0f}" for k in ['kept','a','b','c','d:gone','d:next pass','d:other']]
      print(f"| {d} | {m} | {j} | {x['runs']} | "+' | '.join(cells)+' |')
print()
print('| doc | build | runs on the path (ms): kept | next pass | (a) | (b) | (c) | other | path end ms |')
print('|---|---|---|---|---|---|---|---|---|')
for d in docs:
  for m in ['clean','second']:
    x=L(d,m,8); cp=x['cp']['run_ms']
    c=sum(v for k,v in cp.items() if k.startswith('c'))
    o=sum(v for k,v in cp.items() if k not in ('kept','d:next pass','a','b') and not k.startswith('c'))
    print(f"| {d} | {m} | {cp.get('kept',0):.0f} | {cp.get('d:next pass',0):.0f} | {cp.get('a',0):.0f} | {cp.get('b',0):.0f} | {c:.0f} | {o:.0f} | {x['cp']['end_ms']:.0f} |")
print()
print('| doc | build | (a) definer unsettled: text / page layer | (a) definer settled: text / page layer | (b) unsettled: text / page | (b) settled: text / page |')
print('|---|---|---|---|---|---|')
for d in docs:
  for m in ['clean','second']:
    x=L(d,m,8)['by_definer']
    g=lambda k: x.get(k,{'runs':0,'cpu_ms':0})
    f=lambda c,s: f"{g(f'{c}/{s}/text')['runs']} ({g(f'{c}/{s}/text')['cpu_ms']:.0f} ms) / {g(f'{c}/{s}/page layer')['runs']} ({g(f'{c}/{s}/page layer')['cpu_ms']:.0f} ms)"
    print(f"| {d} | {m} | {f('a','unsettled')} | {f('a','settled')} | {f('b','unsettled')} | {f('b','settled')} |")
print()
print('| doc | build | rule | runs removed | CPU removed ms | runs added | CPU added ms | delay to the end ms (trace timeline) |')
print('|---|---|---|---|---|---|---|---|')
for d in docs:
  for m in ['clean','second']:
    x=L(d,m,8)
    for v,r in x['replay'].items():
      print(f"| {d} | {m} | {v} | {r['removed']} | {r['removed_cpu_ms']:.0f} | {r['added']} | {r['added_cpu_ms']:.0f} | {r['end_delay_ms']:.0f} |")
print()
for d in docs:
  for m in ['clean','second']:
    x=L(d,m,8)['a']
    print(d,m,'fields', ', '.join(f"`{a}` {b} ({c:.0f})" for a,b,c in x['fields'][:12]))
    print(d,m,'definers', ', '.join(f"{a} {b} ({c:.0f})" for a,b,c in x['definers'][:5]), '; readers', ', '.join(f"{a} {b}" for a,b in x['readers'][:4]))
