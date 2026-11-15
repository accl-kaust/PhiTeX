#!/usr/bin/env python3
"""Classify every run that is not kept, attribute the observed critical
path, name the fields and definers of intermediate re-asks, and replay the
"wait for a settled definer" rule.  docs/note-intermediate-reruns.md.
Usage: classify.py PKL [--json OUT]

Terms (one evaluation, the log of PHITEX_RUN_LOG + PHITEX_INTERMED_LOG):
- pass: the stretch between two quiet points that start a pass (loop-back);
- kept: a node's last committed run of the evaluation;
- a run not kept is classed by what ended it:
  a  re-asked (committed, then asked again in the same pass) by a value of
     the field that is not the value the node's last run of the pass reads
     (intermediate);
  b  re-asked by that final value;
  c  stopped, parked (waits for a definer / held at the frontier) or
     superseded before committing;
  d  other: the next pass re-asked it (loop-back), or a cause with no field
     (Structure, Created, Output).
- the definer of a re-ask: the node whose commit made the wake (the wake's
  site, else the node committing on the thread); it is *unsettled* when it
  commits again later in the same pass.
"""
import bisect, json, pickle, re, sys
from collections import Counter, defaultdict

MS = 1e6


def group(name):
    if name is None:
        return '-'
    return re.sub(r'\d+', '#', name)


def load(pkl):
    D = pickle.load(open(pkl, 'rb'))
    return D


def prepare(D):
    runs, commits, quiets = D['runs'], D['commits'], D['quiets']
    pass_ts = sorted(t for t, p in quiets if p)
    D['pass_of'] = lambda t: bisect.bisect_left(pass_ts, t)
    for v in commits.values():
        v.sort()
    byn = defaultdict(list)
    for i, r in enumerate(runs):
        r['i'] = i
        byn[r['node']].append(r)
    for n, rs in byn.items():
        cs = commits.get(n, [])
        for k, r in enumerate(rs):
            nxt = rs[k + 1]['t0'] if k + 1 < len(rs) else float('inf')
            j = bisect.bisect_left(cs, r['t1'])
            r['done'] = cs[j] if j < len(cs) and cs[j] < nxt else None
            r['committed'] = r['done'] is not None
            r['pass'] = D['pass_of'](r['t0'])
            r['k'] = k
    D['byn'] = byn
    kept, pkept = {}, {}
    for n, rs in byn.items():
        for r in rs:
            if r['committed']:
                kept[n] = r
                pkept[(n, r['pass'])] = r
    # a node gone (or taken by a claim) after its last run: none of its runs is kept
    lastg = {}
    for t, n, st in D.get('gone', []):
        lastg[n] = max(lastg.get(n, -1), t)
    D['gone_nodes'] = set()
    for n, t in lastg.items():
        rs = byn.get(n)
        if rs and t >= rs[-1]['t1']:
            D['gone_nodes'].add(n)
            kept.pop(n, None)
            for k in [k for k in pkept if k[0] == n]:
                del pkept[k]
    D['kept'], D['pkept'] = kept, pkept
    wk = defaultdict(list)
    for w in D['wakes']:
        wk[w[1]].append(w)
    for v in wk.values():
        v.sort()
    D['wk'] = wk
    iv = defaultdict(list)
    for v in D.get('vals', []):
        iv[v[1]].append(v)
    for v in iv.values():
        v.sort()
    D['iv'] = iv
    # the run whose commit ends at each commit time
    rbd = {}
    for r in runs:
        if r['done'] is not None:
            rbd[(r['node'], r['done'])] = r
    D['rbd'] = rbd
    settles = {}
    for r in runs:
        settles[r['node']] = r['settles']
    D['settles'] = settles
    fr = sorted(D.get('fronts', []))
    D['fronts'] = fr
    D['quiet_ts'] = sorted(t for t, p in quiets)


def counted(r):
    return r['how'] != 'waitsonly'


def producing_run(D, d, t):
    """The run of node d whose commit is the last to end at or before t, or
    the one committing at t; with that commit's end."""
    cs = D['commits'].get(d, [])
    j = bisect.bisect_left(cs, t)
    # (committing at t: the commit that ends first after t, if its run ended before t)
    if j < len(cs):
        x = D['rbd'].get((d, cs[j]))
        if x is not None and x['t1'] <= t:
            return x, cs[j]
    if j > 0:
        x = D['rbd'].get((d, cs[j - 1]))
        return x, cs[j - 1]
    return None, None


def enabler(D, r):
    """What let run r begin: (time, the run before it on the chain or None,
    kind)."""
    byn = D['byn']
    n = r['node']
    rs = byn[n]
    k = r['k']
    cands = []
    pr = rs[k - 1] if k > 0 else None
    if pr is not None:
        cands.append(((pr['done'] or pr['t1']), pr, 'own'))
        wf = D['waitfor'].get((n, pr['t0']))
        if wf is not None:
            x, t = producing_run(D, wf[0], r['t0'])
            if x is not None and t is not None:
                cands.append((t, x, 'waited'))
        if pr['how'] == 'frontier':
            j = bisect.bisect_right(D['fronts'], (r['t0'], 1 << 62)) - 1
            if j >= 0:
                t, by = D['fronts'][j]
                x, _ = producing_run(D, by, t)
                if x is not None:
                    cands.append((t, x, 'frontier'))
    # the last wake before it began (after its last run began)
    lo = pr['t0'] if pr is not None else -1
    ws = D['wk'].get(n, [])
    j = bisect.bisect_right(ws, (r['t0'], 1 << 62)) - 1
    if j >= 0 and ws[j][0] >= lo:
        w = ws[j]
        c = w[9] if len(w) > 9 else -1
        h = w[10] if len(w) > 10 else -1
        if (c < 0 or c == n) and h >= 0 and h != n:
            # (a re-ask with no node of its own, by the node's own commit
            # check: the definer whose commit moved a read of the run in
            # flight; the run's tail and its commit lie between)
            x, t = producing_run(D, h, w[0])
            if x is not None and t is not None:
                cands.append((t, x, 'stale'))
        elif c >= 0:
            x, _ = producing_run(D, c, w[0])
            if x is not None:
                cands.append((w[0], x, 'wake'))
        else:
            cands.append((w[0], None, 'quiet'))
    if r['ckind'] == 'Created' and r['cby'] >= 0:
        x, t = producing_run(D, r['cby'], r['t0'])
        if x is not None and t is not None:
            cands.append((t, x, 'created'))
    # quiet points release held runs
    q = D['quiet_ts']
    j = bisect.bisect_right(q, r['t0']) - 1
    if j >= 0 and q[j] >= lo:
        cands.append((q[j], None, 'quiet'))
    cands = [c for c in cands if c[0] <= r['t0']]
    if not cands:
        return None
    # (a re-ask by the node's own commit check follows the definer that
    # moved the read, not the node's own run before, which it refutes)
    st = [c for c in cands if c[2] == 'stale']
    if st and max(cands, key=lambda c: c[0])[2] in ('own', 'wake'):
        return max(st, key=lambda c: c[0])
    return max(cands, key=lambda c: c[0])


def classify(D):
    runs, byn, kept, pkept, names = D['runs'], D['byn'], D['kept'], D['pkept'], D['names']
    ops = D['ops']
    pass_of = D['pass_of']
    cls, detail = {}, {}
    for n, rs in byn.items():
        for k, r in enumerate(rs):
            if not counted(r):
                cls[r['i']] = 'w'
                continue
            if n in D['gone_nodes']:
                cls[r['i']] = 'd:gone'
                continue
            if kept.get(n) is r:
                cls[r['i']] = 'kept'
                continue
            if not r['committed']:
                h = r['how']
                cls[r['i']] = {'stopped': 'c:stopped', 'panicked': 'c:stopped', 'waits': 'c:waits',
                               'frontier': 'c:frontier'}.get(h, 'c:superseded')
                continue
            if pkept.get((n, r['pass'])) is r:
                cls[r['i']] = 'd:next pass'
                continue
            nx = next((x for x in rs[k + 1:] if counted(x)), None)
            if nx is None:
                cls[r['i']] = 'd:none'
                continue
            f = nx['cfield']
            # the wake that refuted r: the first after r began, of field f if any
            ws = [w for w in D['wk'].get(n, []) if r['t0'] <= w[0] <= nx['t0']]
            wf = [w for w in ws if w[4] == f] or ws
            w = wf[0] if wf else None
            dfn = -1
            if w is not None:
                dfn = w[6] if w[6] >= 0 else (w[9] if len(w) > 9 else -1)
            if dfn < 0:
                dfn = nx['wby'] if nx['wby'] >= 0 else nx['cby']
            unsettled = None
            if dfn >= 0 and w is not None:
                cs = D['commits'].get(dfn, [])
                p = pass_of(w[0])
                later = [c for c in cs[bisect.bisect_right(cs, w[0]):] if pass_of(c) == p]
                unsettled = len(later) >= 2
            P = pkept.get((n, r['pass']))
            vclass = None
            if f and P is not None:
                pk = {}
                for (kk, f2, a, v) in P['reads']:
                    if f2 == f and kk in 'FEON':
                        pk.setdefault((kk, a), v)
                # the value it was asked by (imv), compared to the final read at its own place
                iv = [v for v in D['iv'].get(n, []) if v[2] == f and r['t0'] <= v[0] <= nx['t0']]
                if iv and ('F', 'm') in pk:
                    vclass = 'b' if iv[0][4] == pk[('F', 'm')] else 'a'
                    via = 'imv'
                else:
                    via = 'run'
                    for x in [y for y in rs[k + 1:] if y['t0'] <= P['t0']]:
                        fr = [(kk, a, v) for (kk, f2, a, v) in x['reads'] if f2 == f and kk in 'FEON']
                        if fr:
                            kk, a, v = fr[0]
                            ref = pk.get((kk, a))
                            vclass = 'b' if ref is not None and ref == v else 'a'
                            break
                    if vclass is None:
                        vclass = 'b'
                        via = 'none'
            if not f:
                c = 'd:' + nx['ckind']
            else:
                c = vclass
            cls[r['i']] = c
            detail[r['i']] = dict(f=f, by=dfn, kind=nx['ckind'], unsettled=unsettled,
                                  byop=ops.get(dfn, '-'), bysettles=D['settles'].get(dfn),
                                  tw=w[0] if w else None)
    return cls, detail


def critical_path(D, cls):
    runs = D['runs']
    cur = max(runs, key=lambda r: (r['done'] or r['t1']))
    t_end = cur['done'] or cur['t1']
    cp = Counter(); cpn = Counter(); how = Counter()
    seen = set(); queue = 0; commit = 0; quiet = 0; stale = 0; links = 0
    chain = []
    while cur is not None and cur['i'] not in seen:
        seen.add(cur['i']); links += 1
        c = cls[cur['i']]
        cp[c] += cur['t1'] - cur['t0']; cpn[c] += 1
        chain.append(cur['i'])
        e = enabler(D, cur)
        if e is None:
            # (no wake, park or placer let it go: queued until a worker took
            # it; on from the run that ended last before it began)
            e = (cur['t0'], None, 'queued')
        t, x, kind = e
        how[kind] += 1
        if x is None:
            # a quiet point: on from the last run that ended before it
            prev = [r for r in runs if (r['done'] or r['t1']) <= t and r is not cur]
            if not prev:
                queue += cur['t0']
                break
            x = max(prev, key=lambda r: (r['done'] or r['t1']))
            if kind == 'quiet':
                quiet += cur['t0'] - (x['done'] or x['t1'])
            else:
                queue += cur['t0'] - (x['done'] or x['t1'])
            cur = x
            continue
        if kind == 'stale' and cur['k'] > 0:
            # the node's run in flight when the definer committed, then its
            # own commit, which re-asked it: wasted, then queued
            pr = D['byn'][cur['node']][cur['k'] - 1]
            pe = max(t, min(pr['done'] or pr['t1'], cur['t0']))
            stale += pe - t
            queue += cur['t0'] - pe
        else:
            queue += cur['t0'] - t
        commit += max(0, t - x['t1'])
        cur = x
    return dict(end_ms=t_end / MS, run_ms={c: cp[c] / MS for c in cp}, run_n=dict(cpn),
                queue_ms=queue / MS, commit_ms=commit / MS, quiet_ms=quiet / MS, stale_ms=stale / MS, links=links,
                enablers=dict(how)), chain


def replay(D, cls, detail, variant):
    """The rule: a reader re-asked by a definer that commits again later in
    this pass (oracle) [and, variant 'pages', only where that definer is of
    the settling layer] does not run then; it is re-asked once, at that
    definer's last commit of the pass, and only if its value then differs
    from what it read before the first such re-ask. Re-asks left are
    coalesced when they fall within one run of the node. Returns the runs
    removed and added, their CPU, and the delay to the end of the build,
    propagated along each run's enabler (no credit for the freed CPU)."""
    runs, byn, pkept, commits, pass_of = D['runs'], D['byn'], D['pkept'], D['commits'], D['pass_of']
    removed = set(); added = 0; add_cpu = 0; defer = {}
    for n, rs in byn.items():
        bypass = defaultdict(list)
        for r in rs:
            if counted(r):
                bypass[r['pass']].append(r)
        for p, prs in bypass.items():
            deferred = {}
            for k in range(len(prs) - 1):
                r, nx = prs[k], prs[k + 1]
                d = detail.get(r['i'])
                if d is None or not d['unsettled'] or d['by'] < 0:
                    continue
                if variant == 'pages' and not d['bysettles']:
                    continue
                if variant == 'text' and d['bysettles']:
                    continue
                lc = [c for c in commits.get(d['by'], []) if pass_of(c) == p]
                T = lc[-1] if lc else nx['t0']
                e = deferred.setdefault(d['by'], [T, [], d['f'], r])
                e[1].append(nx)
            for dfn, (T, rr, f, before) in deferred.items():
                P = pkept.get((n, p))
                rr = [x for x in rr if x['i'] not in removed]
                if not rr:
                    continue
                # the deferred runs vanish; one run at T if the value moved
                a = [v for (kk, f2, an, v) in before['reads'] if f2 == f][:1]
                b = [v for (kk, f2, an, v) in P['reads'] if f2 == f][:1] if P is not None else a
                need = a != b
                if P is not None and P in rr:
                    # (its kept run among them: it runs, at T at the earliest)
                    rr = [x for x in rr if x is not P]
                    if T > P['t0']:
                        defer[P['i']] = max(defer.get(P['i'], 0), T - P['t0'])
                    need = False
                for x in rr:
                    removed.add(x['i'])
                if need and not any(x['t0'] >= T for x in prs if x['i'] not in removed):
                    added += 1
                    add_cpu += P['cpu'] if P is not None else 0
    rem_cpu = sum(runs[i]['cpu'] for i in removed)
    # the delay to the end, propagated in time order along the enablers
    # (a run let go by a quiet point waits for every run before it: the
    # largest delay of a run ended before the quiet point; one queued, for
    # the run that ended last before it began)
    delay = {}
    ends = []   # (end, delay) in order of end, with a running max
    order = sorted(runs, key=lambda r: r['t0'])
    import heapq
    pend = []   # (end, i) of runs begun, not yet folded into the running max
    runmax = 0
    lastend = (-1, 0)
    for r in order:
        while pend and pend[0][0] <= r['t0']:
            end, i = heapq.heappop(pend)
            runmax = max(runmax, delay[i])
            if end >= lastend[0]:
                lastend = (end, delay[i])
        e = enabler(D, r)
        if e is None or e[2] == 'queued':
            dl = lastend[1]
        elif e[1] is None:
            dl = runmax
        else:
            dl = delay.get(e[1]['i'], 0)
        dl = max(dl, defer.get(r['i'], 0))
        delay[r['i']] = dl
        heapq.heappush(pend, ((r['done'] or r['t1']), r['i']))
    last = max(runs, key=lambda r: (r['done'] or r['t1']))
    rem_by = Counter(cls[i] for i in removed)
    return dict(removed=len(removed), removed_cpu_ms=rem_cpu / MS, added=added,
                added_cpu_ms=add_cpu / MS, removed_by_class=dict(rem_by),
                kept_deferred=len(defer), end_delay_ms=delay.get(last['i'], 0) / MS,
                max_delay_ms=max(delay.values(), default=0) / MS)


def report(D, cls, detail):
    runs, names = D['runs'], D['names']
    out = {}
    tot = Counter(); cpu = Counter(); wall = Counter()
    for r in runs:
        c = cls[r['i']]
        tot[c] += 1; cpu[c] += r['cpu']; wall[c] += r['t1'] - r['t0']
    out['runs'] = sum(v for c, v in tot.items() if c != 'w')
    out['nodes'] = len(D['kept'])
    out['cpu_ms'] = sum(cpu[c] for c in cpu if c != 'w') / MS
    out['classes'] = {c: dict(runs=tot[c], cpu_ms=round(cpu[c] / MS, 1), wall_ms=round(wall[c] / MS, 1))
                      for c in sorted(tot)}
    sp = Counter(); spc = Counter()
    for i, d in detail.items():
        st = {True: 'unsettled', False: 'settled', None: 'unknown'}[d['unsettled']]
        lay = 'page layer' if d['bysettles'] else ('text' if d['bysettles'] is False else '?')
        key = f"{cls[i]}/{st}/{lay}"
        sp[key] += 1; spc[key] += runs[i]['cpu']
    out['by_definer'] = {k: dict(runs=sp[k], cpu_ms=round(spc[k] / MS, 1)) for k in sorted(sp)}

    def tab(sel):
        fc = Counter(); fcc = Counter(); dc = Counter(); dcc = Counter(); rc = Counter()
        for i, d in detail.items():
            if cls[i] != sel:
                continue
            g = group(names.get(d['f'])) if d['f'] else '(' + d['kind'] + ')'
            fc[g] += 1; fcc[g] += runs[i]['cpu']
            dop = d['byop'] + ('*' if d['bysettles'] else '')
            dc[dop] += 1; dcc[dop] += runs[i]['cpu']
            rc[runs[i]['op']] += 1
        return dict(fields=[(g, fc[g], round(fcc[g] / MS, 1)) for g, _ in fc.most_common(25)],
                    definers=[(g, dc[g], round(dcc[g] / MS, 1)) for g, _ in dc.most_common(12)],
                    readers=rc.most_common(10))
    out['a'] = tab('a')
    out['b'] = tab('b')
    out['cp'], chain = critical_path(D, cls)
    out['replay'] = {v: replay(D, cls, detail, v) for v in ('oracle', 'pages', 'text')}
    # page-layer share of every rerun's cause (for the persisted page layer)
    pl = Counter(); plc = Counter()
    for r in runs:
        if not counted(r) or r['k'] == 0:
            continue
        prev = D['byn'][r['node']][r['k'] - 1]
        d = detail.get(prev['i'])
        lay = cls[prev['i']]
        if d is not None:
            lay = 'page layer' if d['bysettles'] else 'text'
        elif r['ckind'] in ('Range',):
            lay = 'page layer'
        pl[lay] += 1; plc[lay] += prev['cpu']
    out['rerun_cause_layer'] = {k: dict(runs=pl[k], cpu_ms=round(plc[k] / MS, 1)) for k in pl}
    return out


def main(pkl, jout=None):
    D = load(pkl)
    prepare(D)
    cls, detail = classify(D)
    out = report(D, cls, detail)
    js = json.dumps(out, indent=1, default=str)
    if jout:
        open(jout, 'w').write(js)
    print(js)


if __name__ == '__main__':
    a = sys.argv[1:]
    main(a[0], a[a.index('--json') + 1] if '--json' in a else None)
