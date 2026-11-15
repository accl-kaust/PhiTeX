#!/usr/bin/env python3
"""Classify every run that is not kept, attribute the observed critical
path, name the fields and definers of intermediate re-asks, and replay the
"wait for a settled definer" rule.  docs/note-intermediate-reruns.md.
Usage: classify.py PKL [--json OUT]"""
import bisect, json, pickle, re, sys
from collections import Counter, defaultdict

MS = 1e6

def group(name):
    """A field's family: numbers and node ids folded."""
    if name is None:
        return '-'
    n = re.sub(r'\d+', '#', name)
    return n

def layer_of(op, settles):
    if settles:
        return 'page layer (settling)'
    return op

def main(pkl, jout=None):
    D = pickle.load(open(pkl, 'rb'))
    names, runs, commits, wakes, quiets, ops, waitfor = (D[k] for k in
        ('names', 'runs', 'commits', 'wakes', 'quiets', 'ops', 'waitfor'))
    settles_op = {}
    for r in runs:
        settles_op[r['op']] = r['settles']
    pass_ts = sorted(t for t, p in quiets if p)
    quiet_ts = sorted(t for t, p in quiets)
    def pass_of(t):
        return bisect.bisect_left(pass_ts, t)
    for v in commits.values():
        v.sort()
    byn = defaultdict(list)
    for i, r in enumerate(runs):
        r['i'] = i
        byn[r['node']].append(r)
    # committed: a commit of the node between this run's end and its next run
    for n, rs in byn.items():
        cs = commits.get(n, [])
        for k, r in enumerate(rs):
            nxt = rs[k + 1]['t0'] if k + 1 < len(rs) else float('inf')
            j = bisect.bisect_left(cs, r['t1'])
            r['done'] = cs[j] if j < len(cs) and cs[j] < nxt else None
            r['committed'] = r['done'] is not None
            r['pass'] = pass_of(r['t0'])
    counted = lambda r: r['how'] != 'waitsonly'
    # kept: last committed run in the evaluation; pkept: last committed in its pass
    kept, pkept = {}, {}
    for n, rs in byn.items():
        for r in rs:
            if r['committed']:
                kept[n] = r
                pkept[(n, r['pass'])] = r
    # wakes per reader, sorted
    wk = defaultdict(list)
    for w in wakes:
        wk[w[1]].append(w)
    for v in wk.values():
        v.sort()
    def commits_after(d, t, p):
        cs = commits.get(d, [])
        j = bisect.bisect_right(cs, t)
        return [c for c in cs[j:] if pass_of(c) == p]
    def field_reads(r, f):
        return [(k, f2, a, v) for (k, f2, a, v) in r['reads'] if f2 == f and k in 'FEON']
    cls = {}
    detail = {}
    for n, rs in byn.items():
        for k, r in enumerate(rs):
            if not counted(r):
                cls[r['i']] = 'waitonly'
                continue
            if kept.get(n) is r:
                cls[r['i']] = 'kept'
                continue
            if pkept.get((n, r['pass'])) is r and r['committed']:
                cls[r['i']] = 'd:next pass'
                continue
            if not r['committed']:
                h = r['how']
                cls[r['i']] = {'stopped': 'c:stopped', 'panicked': 'c:stopped', 'waits': 'c:waits',
                               'frontier': 'c:frontier'}.get(h, 'c:superseded')
                continue
            # committed, re-asked in the same pass: the next counted run's wake
            nx = next((x for x in rs[k + 1:] if counted(x)), None)
            if nx is None:
                cls[r['i']] = 'd:no next'
                continue
            f = nx['cfield']
            P = pkept.get((n, r['pass']))
            # definer-based: the wake's definer commits again in this pass after the wake
            D_ = nx['wby'] if nx['wby'] >= 0 else nx['cby']
            tw = None
            for w in reversed(wk.get(n, [])):
                if w[0] <= nx['t0'] and w[0] >= r['t0'] and (w[6] == D_ or w[5] == D_):
                    tw = w[0]
                    break
            unsettled = None
            if D_ >= 0 and tw is not None:
                later = commits_after(D_, tw, pass_of(tw))
                # (the wake is made inside a commit of the definer, which
                # ends after it: a second one after is a later commit)
                unsettled = len(later) >= 2
            vclass = None
            if f and P is not None:
                pk = {(kk, a): v for (kk, f2, a, v) in field_reads(P, f)}
                pfirst = next(iter(pk.values()), None)
                for x in [y for y in rs[k + 1:] if y['t0'] <= P['t0']]:
                    fr = field_reads(x, f)
                    if fr:
                        kk, _, a, v = fr[0]
                        ref = pk.get((kk, a), None)
                        if ref is None:
                            vclass = 'a' if pfirst is None or pfirst != v else 'b'
                        else:
                            vclass = 'b' if ref == v else 'a'
                        break
                if vclass is None:
                    vclass = 'b?'  # (no later run read it before the kept one: taken as final)
            if not f:
                c = 'd:' + nx['ckind']
            elif vclass == 'a':
                c = 'a'
            else:
                c = 'b'
            cls[r['i']] = c
            detail[r['i']] = dict(f=f, by=D_, kind=nx['ckind'], unsettled=unsettled, vclass=vclass,
                                  byop=ops.get(D_, '-'), tw=tw)
    # ---- tables
    out = {}
    tot = Counter(); cpu = Counter(); wall = Counter()
    for r in runs:
        c = cls[r['i']]
        tot[c] += 1; cpu[c] += r['cpu']; wall[c] += r['t1'] - r['t0']
    nruns = sum(v for c, v in tot.items() if c != 'waitonly')
    out['runs'] = nruns
    out['nodes'] = len(kept)
    out['classes'] = {c: dict(runs=tot[c], cpu_ms=cpu[c] / MS, wall_ms=wall[c] / MS) for c in tot}
    # definer-settled split of a and b
    sp = Counter(); spc = Counter()
    for i, d in detail.items():
        key = (cls[i], 'unsettled' if d['unsettled'] else ('settled' if d['unsettled'] is False else 'unknown'))
        sp[key] += 1; spc[key] += runs[i]['cpu']
    out['by_definer_state'] = {f'{a}/{b}': dict(runs=sp[(a, b)], cpu_ms=spc[(a, b)] / MS) for (a, b) in sp}
    # ---- class (a): fields and definers
    fa = Counter(); fac = Counter(); da = Counter(); dac = Counter(); ka = Counter()
    fb = Counter(); fbc = Counter()
    for i, d in detail.items():
        g = group(names.get(d['f'])) if d['f'] else '(' + d['kind'] + ')'
        if cls[i] == 'a':
            fa[g] += 1; fac[g] += runs[i]['cpu']
            dop = d['byop']
            da[dop] += 1; dac[dop] += runs[i]['cpu']
            ka[d['kind']] += 1
        elif cls[i] == 'b':
            fb[g] += 1; fbc[g] += runs[i]['cpu']
    out['a_fields'] = [(g, fa[g], fac[g] / MS) for g, _ in fa.most_common(25)]
    out['a_definers'] = [(g, da[g], dac[g] / MS) for g, _ in da.most_common(15)]
    out['a_kinds'] = dict(ka)
    out['b_fields'] = [(g, fb[g], fbc[g] / MS) for g, _ in fb.most_common(15)]
    # reader ops of class a
    ra = Counter()
    for i, d in detail.items():
        if cls[i] == 'a':
            ra[runs[i]['op']] += 1
    out['a_readers'] = ra.most_common(10)
    # all non-kept by wake field family (every class), for the page layer share
    allf = Counter(); allfc = Counter()
    for n, rs in byn.items():
        for k, r in enumerate(rs[1:], 1):
            if not counted(r):
                continue
            prev = rs[k - 1]
            g = group(names.get(r['cfield'])) if r['cfield'] else '(' + r['ckind'] + ')'
            allf[g] += 1; allfc[g] += prev['cpu']
    out['rerun_wake_fields'] = [(g, allf[g], allfc[g] / MS) for g, _ in allf.most_common(25)]
    # ---- observed critical path
    run_end_by_done = {}
    for r in runs:
        if r['done'] is not None:
            run_end_by_done[(r['node'], r['done'])] = r
    def producing_run(d, t):
        cs = commits.get(d, [])
        j = bisect.bisect_right(cs, t) - 1
        if j < 0:
            return None, None
        done = cs[j]
        return run_end_by_done.get((d, done)), done
    cur = max(runs, key=lambda r: (r['done'] or r['t1']))
    t_end = cur['done'] or cur['t1']
    cp = Counter(); cpn = Counter(); gap = 0; com = 0; steps = 0
    seen = set()
    while cur is not None and cur['i'] not in seen and steps < 100000:
        seen.add(cur['i']); steps += 1
        c = cls[cur['i']]
        cp[c] += cur['t1'] - cur['t0']; cpn[c] += 1
        cands = []
        n = cur['node']
        rs = byn[n]
        k = rs.index(cur)
        if k > 0:
            pr = rs[k - 1]
            wf = waitfor.get((n, pr['t0']))
            if wf is not None:
                x, t = producing_run(wf[0], cur['t0'])
                if x is not None:
                    cands.append((t, x))
            cands.append(((pr['done'] or pr['t1']), pr))
        d = cur['wby'] if cur['wby'] >= 0 else cur['cby']
        if d >= 0:
            x, t = producing_run(d, cur['t0'])
            if x is not None:
                cands.append((t, x))
        cands = [(t, x) for (t, x) in cands if t <= cur['t0']]
        if not cands:
            gap += cur['t0']
            break
        t, x = max(cands, key=lambda z: z[0])
        gap += cur['t0'] - t
        com += t - x['t1']
        cur = x
    out['cp'] = dict(end_ms=t_end / MS, runs_ms={c: cp[c] / MS for c in cp}, runs_n=dict(cpn),
                     queue_ms=gap / MS, commit_ms=com / MS, links=steps)
    # ---- replay: wait for a settled definer
    out['replay'] = replay(runs, byn, cls, detail, kept, pkept, commits, wk, pass_of, counted, ops, names)
    js = json.dumps(out, indent=1, default=str)
    if jout:
        open(jout, 'w').write(js)
    print(js)

def replay(runs, byn, cls, detail, kept, pkept, commits, wk, pass_of, counted, ops, names):
    """Per node and pass: each re-run's wake mapped to an effective time.
    oracle: a wake by a definer that commits again in this pass waits for
    its last commit there; the waits from one definer collapse into one
    re-ask, made only if the reader's value of the field then differs from
    what its run before the first deferred wake read.  The re-asks that
    remain are coalesced when they fall within one run of the node (a
    re-ask while the node is queued or running asks for one run more)."""
    res = {}
    for variant in ('oracle', 'settling-only'):
        removed = 0; removed_cpu = 0; added = 0; delays = []
        for n, rs in byn.items():
            cs = [r for r in rs if counted(r)]
            if not cs:
                continue
            bypass = defaultdict(list)
            for r in cs:
                bypass[r['pass']].append(r)
            for p, prs in bypass.items():
                if len(prs) < 2:
                    continue
                dur = max(1, sorted(r['t1'] - r['t0'] for r in prs)[len(prs) // 2])
                events = []   # (time, run index it stands for or None)
                deferred = {}  # definer -> (T_final, [run], field)
                for k in range(1, len(prs)):
                    r = prs[k]
                    prev = prs[k - 1]
                    d = r['wby'] if r['wby'] >= 0 else r['cby']
                    # an unkept predecessor that waited/stopped: its rerun is a resumption, kept as is
                    if prev['how'] in ('waits', 'frontier') or not prev['committed']:
                        events.append((r['t0'], r))
                        continue
                    dd = detail.get(prev['i'])
                    unsettled = dd is not None and dd['unsettled']
                    if variant == 'settling-only':
                        unsettled = unsettled and r['settles'] is False and \
                            runs_settles(byn, d)
                    if unsettled and d >= 0:
                        lc = [c for c in commits.get(d, []) if pass_of(c) == p]
                        T = lc[-1] if lc else r['t0']
                        e = deferred.setdefault(d, [T, [], r['cfield'], prev])
                        e[1].append(r)
                    else:
                        events.append((r['t0'], r))
                for d, (T, rr, f, before) in deferred.items():
                    P = pkept.get((n, p))
                    need = True
                    if f and P is not None:
                        a = [v for (kk, f2, an, v) in before['reads'] if f2 == f]
                        b = [v for (kk, f2, an, v) in P['reads'] if f2 == f]
                        need = a[:1] != b[:1]
                    # (the kept run among the deferred: it must run, delayed to T)
                    if P in rr:
                        need = True
                        if T > P['t0']:
                            delays.append((T - P['t0'], n, d))
                    if need:
                        events.append((max(T, rr[0]['t0']), None))
                    removed += len(rr)
                    removed_cpu += sum(x['cpu'] for x in rr)
                # coalesce the remaining events
                events.sort(key=lambda e: e[0])
                kept_ev = 0; last = -1e30
                for t, r in events:
                    if t >= last + dur:
                        kept_ev += 1; last = t
                    else:
                        if r is not None:
                            removed += 1; removed_cpu += r['cpu']
                        else:
                            kept_ev += 0
                added += sum(1 for t, r in events if r is None)
        res[variant] = dict(removed=removed, removed_cpu_ms=removed_cpu / MS, added=added,
                            net=removed - added,
                            kept_delay_ms_sum=sum(x[0] for x in delays) / MS,
                            kept_delay_ms_max=max((x[0] for x in delays), default=0) / MS,
                            kept_delayed=len(delays))
    return res

_settles = {}
def runs_settles(byn, d):
    if d not in _settles:
        rs = byn.get(d)
        _settles[d] = bool(rs) and rs[0]['settles']
    return _settles[d]

if __name__ == '__main__':
    a = sys.argv[1:]
    j = None
    if '--json' in a:
        j = a[a.index('--json') + 1]
    main(a[0], j)
