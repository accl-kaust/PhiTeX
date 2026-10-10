#!/usr/bin/env python3
# sched.py FILE...: the scheduler's events (PHITEX_SCHED) of each build round:
# per worker where the time went (reading, parked, waiting for / holding the
# install lock, other tasks, picks, idle), how many workers read at once,
# the ready pieces at each pick, piece sizes (commands per piece: how few
# pieces hold the work), reruns and abandoned runs, install vs run cost.
import sys, collections
PRIO = {'idle': 0, 'pick': 1, 'relock': 2, 'break': 3, 'scan': 3, 'ship': 3, 'log': 3, 'setup': 4,
        'read': 5, 'inst_check': 6, 'inst_lock': 6, 'inst_held': 7, 'park': 8}
TASK = {17: 'break', 18: 'scan', 19: 'ship', 20: 'log'}
def rounds(path):
    cur = None
    for l in open(path):
        f = l.rstrip('\n').split('\t')
        if f[0] == 'R':
            if cur: yield cur
            cur = dict(build=int(f[1]), threads=int(f[2]), pieces=int(f[3]), wall=int(f[4]), lockw=int(f[6]) if len(f) > 6 else 0, lockn=int(f[7]) if len(f) > 7 else 0, setup=int(f[8]) if len(f) > 8 else 0, P=[], E=[])
        elif f[0] == 'P' and cur: cur['P'].append(tuple(map(int, f[1:])))
        elif f[0] == 'E' and cur:
            v = list(map(int, f[1:]))
            if len(v) < 12: v.append(v[6])
            v += [0] * (14 - len(v))
            cur['E'].append(tuple(v))
    if cur: yield cur
def ms(ns): return ns / 1e6
def report(path, r):
    W, T = r['wall'], r['threads']
    iv = collections.defaultdict(list)   # worker -> (start, end, state)
    runs = []; picks = []; seqs = []; inst = []
    for e in r['E']:
        wid, kind, k, t0, t1, t2, t3, t4, a, b, c, t5, dn, dt = e
        if kind == 0:
            runs.append(e)
            iv[wid] += [(t0, t1, 'setup'), (t1, t2, 'read'), (t2, t5, 'inst_check'), (t5, t3, 'inst_lock'), (t3, t4, 'inst_held')]
        elif kind == 5: iv[wid].append((t0, t1, 'idle'))
        elif kind == 6: iv[wid].append((t0, t1, 'park'))
        elif kind == 7: iv[wid].append((t0, t1, 'pick')); picks.append(e)
        elif kind == 8: seqs.append(e)
        elif kind == 9: inst.append(e)
        elif kind in TASK: iv[wid] += [(t0, t1, TASK[kind]), (t1, t2, 'relock')]
        elif kind == 16: iv[wid].append((t1, t2, 'relock'))
    # per worker: the elementary segments' states (highest priority wins)
    tot = collections.Counter(); segs_all = []
    per = {}
    for wid in sorted(iv):
        pts = []
        for s, e, st in iv[wid]:
            if e > s: pts += [(s, 1, st), (e, -1, st)]
        pts.sort(key=lambda x: (x[0], x[1]))
        cnt = collections.Counter(); last = 0; acc = collections.Counter()
        for t, d, st in pts:
            if t > last:
                act = [x for x in cnt if cnt[x] > 0]
                state = max(act, key=lambda x: PRIO[x]) if act else 'none'
                acc[state] += t - last
                segs_all.append((last, t, state))
            cnt[st] += d; last = max(last, t)
        acc['none'] += max(0, W - last)
        per[wid] = acc; tot += acc
    keys = ['read', 'park', 'inst_check', 'inst_lock', 'inst_held', 'setup', 'break', 'scan', 'ship', 'log', 'relock', 'pick', 'idle', 'none']
    print(f'== {path}: build {r["build"]} threads {T} pieces {r["pieces"]} wall {ms(W):.1f} ms, {len(r["E"])} events')
    if inst:
        ts = lambda e: [e[3], e[4], e[5], e[6], e[7], e[11]]
        ph = [sum(ts(e)[i + 1] - ts(e)[i] for e in inst) for i in range(5)]
        print(f'  under the install lock ({len(inst)} installs, {sum(e[8] for e in inst)} reads): ' + ', '.join(f'{n} {ms(v):.1f} ms' for n, v in zip(['revalidate', 'edges', 'exports', 'marks', 'entries'], ph)))
    WHY = {1: 'file', 2: 'join', 4: 'recut', 8: 'endinput', 16: 'ends', 32: 'stop'}
    for e in seqs:
        print(f'  sequence changed by piece {e[2]} at {ms(e[3]):.1f} ms ({ms(W - e[3]):.1f} ms before the round ends): ' + '+'.join(v for b, v in WHY.items() if e[10] & b) + (' (at once)' if e[8] else ''))
    print(f'  round setup (cutting, layout) {ms(r["setup"]):.1f} ms')
    print(f'  scheduler lock: taken {r["lockn"]} times, waited {ms(r["lockw"]):.1f} ms in all ({100*r["lockw"]/(W*T) if W else 0:.1f}% of threads x wall)')
    print('  all workers, % of threads x wall: ' + ', '.join(f'{k} {100*tot[k]/(W*T):.1f}' for k in keys if tot[k]))
    busiest = max(per.values(), key=lambda a: a['read']) if per else collections.Counter()
    print(f'  busiest reader: read {ms(busiest["read"]):.1f} ms = {100*busiest["read"]/W:.1f}% of wall')
    # how many workers read at once (time-weighted)
    pts = []
    for s, e, st in segs_all:
        if st == 'read': pts += [(s, 1), (e, -1)]
    pts.sort(); n = 0; last = 0; h = collections.Counter()
    for t, d in pts:
        h[n] += t - last; n += d; last = t
    h[0] += max(0, W - last)
    avg = sum(k * v for k, v in h.items()) / W if W else 0
    print(f'  readers at once (time-weighted): mean {avg:.2f}; ' + ', '.join(f'{k}:{100*v/W:.1f}%' for k, v in sorted(h.items()) if v > W / 1000))
    # ready pieces at each pick
    if picks:
        rh = collections.Counter(min(p[8], 64) for p in picks)
        got = sum(1 for p in picks if p[10] & 1)
        b = [0, 1, 2, 4, 8, 16, 32, 64, 256]
        hb = collections.Counter()
        for v, c in rh.items():
            for i in range(len(b) - 1):
                if b[i] <= v < b[i + 1] or (v == 64 and i == len(b) - 2): hb[f'{b[i]}-{b[i+1]-1}'] += c; break
        print(f'  picks {len(picks)} ({got} found a task): ready pieces at a pick: ' + ', '.join(f'{k}:{v}' for k, v in hb.items()))
        pt = sum(p[4] - p[3] for p in picks)
        print(f'  time inside pick (lock held) {ms(pt):.1f} ms in all')
    # pieces: commands run per piece, runs, outcomes
    ran = collections.Counter(); nruns = collections.Counter(); oc = collections.Counter(); waste = 0
    rdur = collections.Counter(); held = 0; wait = 0; read = 0; check_t = [0]; idx_n = 0; idx_t = 0
    for e in runs:
        k, a, c = e[2], e[8], e[10]
        ran[k] += a; nruns[k] += 1; oc[c] += 1
        if c in (1, 2): waste += a
        rdur[k] = max(rdur[k], e[5] - e[4])
        idx_n += e[12]; idx_t += e[13]; held += e[7] - e[6]; wait += e[6] - e[11]; chk = e[11] - e[5]; read += e[5] - e[4]; check_t[0] += chk
    total = sum(ran.values())
    if total:
        srt = sorted(ran.values(), reverse=True); acc = 0; cover = {}
        for i, v in enumerate(srt):
            acc += v
            for q in (0.5, 0.9, 0.99):
                if q not in cover and acc >= q * total: cover[q] = i + 1
        big = ran.most_common(3)
        print(f'  commands run {total} in {len(ran)} pieces; pieces holding 50/90/99%: {cover.get(0.5)}/{cover.get(0.9)}/{cover.get(0.99)}; largest: ' +
              ', '.join(f'piece {k} {100*v/total:.1f}% ({nruns[k]} runs)' for k, v in big))
        lk = max(rdur, key=rdur.get)
        print(f'  longest single run: piece {lk} {ms(rdur[lk]):.1f} ms = {100*rdur[lk]/W:.1f}% of wall')
        P = r['P']
        nodes = sorted((p[2] for p in P), reverse=True)
        print(f'  pieces by nodes held: max {nodes[0] if nodes else 0}, median {nodes[len(nodes)//2] if nodes else 0}; runs per piece: ' +
              ', '.join(f'{k}:{v}' for k, v in sorted(collections.Counter(min(p[1], 9) for p in P).items())))
        print(f'  runs {len(runs)}: settled {oc[0]}, stale (run again) {oc[1]}, abandoned {oc[2]}, kept again {oc[3]}; commands in stale/abandoned runs {waste} ({100*waste/total:.1f}%)')
        print(f'  reads of names from before the piece: {idx_n}, {ms(idx_t):.1f} ms ({1000*idx_t/max(idx_n,1)/1000:.0f} ns each), {100*idx_t/max(read,1):.1f}% of reading')
        print(f'  install: validating before the lock {ms(check_t[0]):.1f} ms, waiting for the lock {ms(wait):.1f} ms, holding it {ms(held):.1f} ms, against reading {ms(read):.1f} ms (nested reads counted twice)')
for path in sys.argv[1:]:
    for r in rounds(path):
        report(path, r)
