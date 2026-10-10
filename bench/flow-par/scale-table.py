#!/usr/bin/env python3
# scale-table.py RAW.tsv: the scaling table (median wall, spread, speedup,
# Karp-Flatt serial fraction, median peak RSS, outputs identical across -j,
# the median run's stats)
import sys, re, statistics, collections
rows = collections.defaultdict(list)
order = []
for l in open(sys.argv[1]):
    f = l.rstrip('\n').split('\t')
    f += [''] * (10 - len(f))
    doc, j, rep, s, rss, st, h, cold, ser, rounds = f[:10]
    if doc not in order: order.append(doc)
    rows[(doc, int(j))].append(dict(s=float(s), rss=int(rss) if rss.isdigit() else 0, st=st, h=h, cold=cold, ser=ser, rounds=rounds))
def num(pat, t, d='-'):
    m = re.search(pat, t)
    return m.group(1) if m else d
print()
print('doc  -j  wall_med_s  [min-max]  speedup  karp_flatt  rss_med_MB  status  md5_same_as_j1  | busiest%  pieces ran reruns parks abandoned? | reading_ms confirming_ms held_ms | rounds')
for doc in order:
    js = sorted(j for (d, j) in rows if d == doc)
    base = None; h1 = None
    for j in js:
        v = rows[(doc, j)]
        w = [x['s'] for x in v]
        med = statistics.median(w)
        if j == 1: base = med; h1 = v[0]['h']
        sp = base / med if base else float('nan')
        kf = ((1 / sp - 1 / j) / (1 - 1 / j)) if j > 1 and base else float('nan')
        rss = statistics.median(x['rss'] for x in v) / 1024
        sts = ','.join(sorted(set(x['st'] for x in v)))
        same = 'yes' if all(x['h'] == h1 for x in v) else 'NO(' + ','.join(x['h'] for x in v) + ')'
        mid = sorted(v, key=lambda x: x['s'])[len(v) // 2]
        c = mid['cold']
        busiest = num(r'busiest did (\d+)%', c)
        pieces = num(r'; (\d+) pieces', c); ran = num(r'\((\d+) ran', c); rer = num(r'(\d+) re-run', c); parks = num(r'(\d+) parks', c)
        rd = num(r'reading ([\d.]+) ms', mid['ser']); cf = num(r'confirming ([\d.]+) ms', mid['ser']); hd = num(r'\(([\d.]+) ms holding', mid['ser'])
        rnd = mid['rounds'].replace('rounds (ms):', '').strip()
        print(f'{doc:10s} {j:3d}  {med:8.3f}  [{min(w):.3f}-{max(w):.3f}]  {sp:5.2f}  {kf:6.3f}  {rss:8.0f}  {sts}  {same}  | {busiest} {pieces} {ran} {rer} {parks} | {rd} {cf} {hd} | {rnd}')
    print()
