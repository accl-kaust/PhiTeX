#!/usr/bin/env python3
"""What a page layer seeded from the previous build would save on a second
build (docs/note-intermediate-reruns.md §4): every run not kept, put in the
bucket of what made it.  Usage: q4.py PKL"""
import sys, re, json
from collections import Counter
sys.path.insert(0, __file__.rsplit('/', 1)[0])
import classify as C

PAGE_FIELDS = re.compile(r'^(vprof|pages:|pitems|gdefs|cont:|marks:|wstreams:|fold:)')

def main(pkl):
    D = C.load(pkl); C.prepare(D)
    cls, det = C.classify(D)
    runs, names = D['runs'], D['names']
    laststate = {}
    for t, n, st in sorted(D['gone']):
        laststate[n] = st
    b = Counter(); bc = Counter()
    for r in runs:
        c = cls[r['i']]
        if c in ('kept', 'w'):
            continue
        if c == 'd:gone':
            k = 'claimed (Inert)' if 'Inert' in laststate.get(r['node'], '') else 'removed (Gone)'
        elif c in ('c:frontier',):
            k = 'held at the page frontier'
        elif c in ('d:Structure', 'd:Created', 'd:Output'):
            k = 'structure / placement'
        elif c == 'd:next pass':
            k = 'loop-back pass'
        elif c in ('a', 'b'):
            d = det[r['i']]
            nm = names.get(d['f'], '')
            if d['bysettles'] or PAGE_FIELDS.match(nm):
                k = f'{c}: page-layer value'
            else:
                k = f'{c}: text value'
        else:
            k = c
        b[k] += 1; bc[k] += r['cpu']
    tot = sum(b.values()); totc = sum(bc.values())
    print(json.dumps({k: (b[k], round(bc[k] / 1e6)) for k in sorted(b)}), 'total', tot, round(totc / 1e6))

if __name__ == '__main__':
    main(sys.argv[1])
