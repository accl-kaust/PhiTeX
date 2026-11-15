#!/usr/bin/env python3
"""Parse a PHITEX_RUN_LOG + PHITEX_INTERMED_LOG stderr log (gzip) into a
pickle: runs, commits, wakes, quiet points, field names.
docs/note-intermediate-reruns.md.  Usage: parse.py LOG.gz OUT.pkl"""
import gzip, pickle, re, sys
from collections import defaultdict

def reads_of(s):
    out = []
    for tok in s.split():
        k, _, v = tok.rpartition('=')
        kind = k[0]
        rest = k[1:]
        if kind in 'FEON':
            f, _, anc = rest.partition(':')
            out.append((kind, int(f), anc, int(v) if v != '-' else -1))
        else:
            out.append((kind, 0, rest, int(v)))
    return out

def main(path, out):
    names = {}
    runs = []          # dicts
    commits = defaultdict(list)   # node -> [done t]
    imc = defaultdict(list)       # node -> [(t, serial, defs{f:v}, contribs[f], outchg, quiets)]
    wakes = []         # (t, reader, state, kind, field, by, wby, how, quiets)
    quiets = []        # (t, pass)
    ops = {}
    rcom = re.compile(r'^commit: node (\d+) done (\d+)')
    rwait = re.compile(r'^run: node (\d+) .*: waits for (\d+) \(([^)]*)\).*\tt (\d+) ')
    waitfor = {}
    gone = []          # (t, node, state)
    vals = []          # (t, reader, field, seen, now, by)
    fronts = []        # (t, by)
    rfr = re.compile(r'^frontier: (\d+) -> (\d+) \(at \d+\) by (\d+) .* t (\d+)')
    rq = re.compile(r'^quiet: (\d+) re-asked (\d+) pass (\w+)')
    with gzip.open(path, 'rt', errors='replace') as fh:
        for line in fh:
            if line.startswith('imr\t'):
                p = line.rstrip('\n').split('\t')
                ck = p[9].split()
                r = dict(node=int(p[1]), op=p[2], pos=int(p[3]), t0=int(p[4]), t1=int(p[5]),
                         cpu=int(p[6]), w=int(p[7]), how=p[8], ckind=ck[0], cfield=int(ck[1]),
                         cby=int(ck[2]) - 1, wby=int(p[10]) - 1, goes_on=p[11] == '1',
                         q0=int(p[12]), epoch=int(p[13]), first=p[14] == '1', settles=p[15] == '1',
                         reads=reads_of(p[16]) if len(p) > 16 else [])
                if len(p) > 19 and p[17].startswith('hw '):
                    h = [int(x) for x in p[17].split()[1:]]
                    r.update(cyc=h[0], ins=h[1], cref=h[2], cmiss=h[3], lw=int(p[18]), cpuid=int(p[19]))
                runs.append(r)
                ops[r['node']] = r['op']
            elif line.startswith('imc\t'):
                p = line.rstrip('\n').split('\t')
                defs = {}
                for tok in p[7].split():
                    f, _, v = tok.partition('=')
                    if f != 'many':
                        defs[int(f)] = int(v) if v != '-' else -1
                contribs = [int(x) for x in p[8].split()] if len(p) > 8 else []
                imc[int(p[1])].append((int(p[3]), int(p[2]), defs, contribs, p[4] == '1', int(p[6])))
            elif line.startswith('imw\t'):
                p = line.rstrip('\n').split('\t')
                ck = p[4].split()
                wakes.append((int(p[1]), int(p[2]), p[3], ck[0], int(ck[1]), int(ck[2]) - 1,
                              int(p[5]) - 1, p[6], int(p[7]), int(p[8]) - 1 if len(p) > 8 else -1))
            elif line.startswith('imv\t'):
                p = line.rstrip('\n').split('\t')
                vals.append((int(p[1]), int(p[2]), int(p[3]), int(p[4]), int(p[5]), int(p[6]) - 1))
            elif line.startswith('frontier: ') and ' -> ' in line:
                m = rfr.match(line)
                if m:
                    fronts.append((int(m.group(4)), int(m.group(3))))
            elif line.startswith('img\t'):
                p = line.rstrip('\n').split('\t')
                gone.append((int(p[1]), int(p[2]), p[3]))
            elif line.startswith('imf\t'):
                p = line.rstrip('\n').split('\t', 2)
                names[int(p[1])] = p[2] if len(p) > 2 else ''
            elif line.startswith('commit: '):
                m = rcom.match(line)
                if m:
                    commits[int(m.group(1))].append(int(m.group(2)))
            elif line.startswith('run: ') and 'waits for' in line:
                m = rwait.match(line)
                if m:
                    waitfor[(int(m.group(1)), int(m.group(4)))] = (int(m.group(2)), m.group(3))
            elif line.startswith('quiet: '):
                m = rq.match(line)
                if m:
                    quiets.append((int(m.group(1)), m.group(3) == 'true'))
            elif line.startswith('WALL'):
                wall = line.strip()
    runs.sort(key=lambda r: r['t0'])
    pickle.dump(dict(names=names, runs=runs, commits=dict(commits), imc=dict(imc), wakes=wakes,
                     quiets=quiets, ops=ops, waitfor=waitfor, vals=vals, fronts=fronts, gone=gone), open(out, 'wb'))
    print(path, len(runs), 'runs', sum(len(v) for v in commits.values()), 'commits', len(wakes), 'wakes')

if __name__ == '__main__':
    main(sys.argv[1], sys.argv[2])
