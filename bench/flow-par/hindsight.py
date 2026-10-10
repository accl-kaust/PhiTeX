#!/usr/bin/env python3
# hindsight.py FILE [CHAIN-REGEX]: the critical path with hindsight of a -j 1
# build's PHITEX_SCHED file: pieces weighted by their last run's reading time
# (scaled to the whole piece: ns * nodes / ran), their line breaking after
# them; the page scan a chain over the pieces in order (each scan task's
# time spread over its pieces by nodes); edges: a read answered by another
# piece's export (that piece), by a definition between a piece's nodes (the
# scan past that piece), an entry state not the one assumed (the piece
# before). T1 = all the work; T_inf = the longest path. Then again without
# the edges of names matching CHAIN-REGEX (what Speculate would predict
# right: counters, \@currentlabel...).
import sys, re, collections
path = sys.argv[1]
chain_re = re.compile(sys.argv[2] if len(sys.argv) > 2 else r'^(Count\(\d+\)|\\@current\w*|\\@currentlabel|\\c@\w+)$')
rounds = []; G = {}; D = collections.defaultdict(list); X = collections.defaultdict(list)
cur = None
for l in open(path, errors='replace'):
    f = l.rstrip('\n').split('\t')
    if f[0] == 'R':
        cur = dict(wall=int(f[4]), E=[]); rounds.append(cur)
    elif f[0] == 'E' and cur is not None:
        v = list(map(int, f[1:])); cur['E'].append(v)
    elif f[0] == 'G':
        k = int(f[1]); ent = [int(x) for x in f[2].split(',') if x]
        G[k] = dict(entry=ent, ran=int(f[3]), nodes=int(f[4]), ns=int(f[5]) if len(f) > 5 else 0, lran=int(f[6]) if len(f) > 6 else 0)
    elif f[0] == 'D':
        D[int(f[1])].append((int(f[2]), int(f[3]), f[4], int(f[5]) if len(f) > 5 else 0, int(f[6]) if len(f) > 6 else 0, int(f[7]) if len(f) > 7 else 0, f[8] if len(f) > 8 else ''))
    elif f[0] == 'X' and len(f) >= 4 and f[-1].isdigit():
        X['\t'.join(f[2:-1])].append((int(f[1]), int(f[-1])))
if not G:
    sys.exit(f'{path}: no dependencies (G/D lines)')
n = max(G) + 1
last = rounds[-1]
def cost(k):
    g = G.get(k)
    if not g: return 0, 0
    # (the piece's rate over all its runs, times its nodes)
    return (g['ns'] * g['nodes'] / g['lran'] if g['lran'] else 0), g['nodes']
w = [cost(k)[0] for k in range(n)]; nodes = [cost(k)[1] for k in range(n)]
brk = [0.0] * n; scan = [0.0] * n; ship = 0.0; ship_max = 0.0; logt = 0.0; log_max = 0.0
for v in last['E']:
    kind, k, t0, t1, b = v[1], v[2], v[3], v[4], v[9]
    d = t1 - t0
    if kind == 17 and k < n: brk[k] += d
    elif kind == 18:
        to = max(b, k + 1); tot = sum(nodes[k:to]) or 1
        for p in range(k, min(to, n)): scan[p] += d * (nodes[p] or 0) / tot if tot else d / (to - k)
    elif kind == 19: ship += d; ship_max = max(ship_max, d)
    elif kind == 20: logt += d; log_max = max(log_max, d)
T1 = sum(w) + sum(brk) + sum(scan) + ship + logt
import bisect
for v in X.values(): v.sort()
def content_src(k, d, name, vh, ih):
    # (the first piece of the run of exporters before k that left `name` at
    # this value; None: the value before any definition, no edge)
    ex = X.get(name)
    if not ex or not vh: return d
    i = bisect.bisect_left(ex, (k, -1)) - 1
    if i < 0 or ex[i][0] != d or ex[i][1] != vh: return d
    while i > 0 and ex[i - 1][1] == vh: i -= 1
    if i == 0 and ih == vh: return None
    return ex[i][0]
def longest(mode):
    F = [0.0] * n; S = [0.0] * n; why = [None] * n; cmds = [0] * n; swhy = [None] * n
    for k in range(n):
        best, by, bc = 0.0, None, 0
        for q in G.get(k, {}).get('entry', []):
            if q < k and F[q] > best: best, by, bc = F[q], ('entry', q, ''), cmds[q]
        for d, inner, name, init, vh, ih, cmd in D.get(k, []):
            if d >= k or (mode >= 1 and chain_re.match(name)) or (mode >= 2 and init):
                continue
            if mode >= 3 and 'Some(Outer)' in cmd:
                continue  # (a read of outer-ness only: \outer names are rare, any other value satisfies it)
            if mode >= 3 and not inner:
                d = content_src(k, d, name, vh, ih)
                if d is None: continue
            t, c = (S[d], cmds[d]) if inner else (F[d], cmds[d])
            if t > best: best, by, bc = t, ('fold' if inner else 'read', d, name, cmd), c
        F[k] = best + w[k] + brk[k]; cmds[k] = bc + nodes[k]; why[k] = by
        prev = S[k - 1] if k else 0.0
        S[k] = max(prev, F[k]) + scan[k]; swhy[k] = 'scan' if prev >= F[k] else 'piece'
    end = S[n - 1] + ship_max + log_max
    # walk back from the end
    chain = []; k = n - 1; on_scan = True
    while k is not None and k >= 0:
        if on_scan and swhy[k] == 'scan' and k > 0:
            chain.append(('scan', k)); k -= 1; continue
        chain.append(('piece', k, why[k]))
        on_scan = False
        if why[k] is None: break
        kind, d = why[k][0], why[k][1]
        on_scan = kind == 'fold'; k = d
    return end, chain, max(cmds)
tot_nodes = sum(nodes)
LABEL = ['Suspend (every read)', 'chain names predicted', '+ values = the format\'s predicted', '+ by content (first equal definer)']
for mode in (0, 1, 2, 3):
    skip = mode
    Tinf, chain, cc = longest(mode)
    pieces_on = [c for c in chain if c[0] == 'piece']
    reasons = collections.Counter(c[2][0] if c[2] else 'start' for c in pieces_on)
    names = collections.Counter(c[2][2] for c in pieces_on if c[2] and len(c[2]) > 2 and c[2][2])
    scans = sum(1 for c in chain if c[0] == 'scan')
    print(f"{LABEL[mode]:36s} T1 {T1/1e6:9.1f} ms  T_inf {Tinf/1e6:9.1f} ms  T1/T_inf {T1/max(Tinf,1):6.2f}  pieces {n}  commands {tot_nodes} (path {cc})")
    print(f"    path: {len(pieces_on)} pieces ({dict(reasons)}), {scans} scan steps; names on it: {names.most_common(6)}")
    rc = collections.Counter((c[2][2], c[2][3]) for c in pieces_on if c[2] and len(c[2]) > 3)
    if rc: print('    readers on it (name, reading node): ' + '; '.join(f'{n} by {r!r} x{v}' for (n, r), v in rc.most_common(8)))
    heavy = sorted(((w[c[1]] + brk[c[1]], c[1]) for c in pieces_on), reverse=True)[:3]
    print(f"    heaviest pieces on it: " + ', '.join(f'piece {k} {t/1e6:.1f} ms ({nodes[k]} nodes)' for t, k in heavy))
print(f"    work: pieces {sum(w)/1e6:.1f} ms, line breaking {sum(brk)/1e6:.1f}, scan {sum(scan)/1e6:.1f}, ship {ship/1e6:.1f}, log {logt/1e6:.1f}; largest piece {max(w)/1e6:.1f} ms ({100*max(w)/T1:.1f}% of T1)")
allnames = collections.Counter(e[2] for k in D for e in D[k] if e[0] < k)
print(f"    names read across pieces (edges): {allnames.most_common(8)}")
