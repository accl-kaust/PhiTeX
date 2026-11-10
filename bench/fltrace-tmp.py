"""fltrace: from a run+wake log, the finishers' wakes by field and waker op; the wakers' own wakes."""
import re, sys, collections
L = open(sys.argv[1], errors='replace').read().split('\n')
wake = re.compile(r'wake: node (\d+) op .*? field (\S*) (\w+)[ {(].*? by (-|\d+ \w+)\twake\t(\w+)\t')
fin, wk_of = collections.Counter(), collections.defaultdict(collections.Counter)
first = []
for l in L:
    m = wake.match(l)
    if not m: continue
    node, field, kind, by, op = m.groups()
    f = re.sub(r'[0-9:-]+$', '', field)
    byop = by.split(' ')[-1]
    wk_of[op][(kind, f, byop)] += 1
    if op == 'finish' and len(first) < 25: first.append(l[:300])
for op in ['finish', 'pages', 'exec', 'resume']:
    print(f"== wakes of {op}: {sum(wk_of[op].values())}")
    for k, v in wk_of[op].most_common(15): print(f"  {v} {k}")
print("== first finisher wakes")
for l in first: print(l)
