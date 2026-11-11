#!/usr/bin/env python3
"""cut1: an edits list for cut1-edits.py: a unique 30-byte stretch from
the middle of FILE with one word in it lengthened, N times there and back."""
import json, re, sys
path, n = sys.argv[1], int(sys.argv[2])
t = open(path, encoding="latin-1").read()
for at in range(len(t) // 2, len(t) - 40):
    m = re.match(r" ([a-z]{6,}) ", t[at:])
    if m and t.count(t[at:at + 30]) == 1:
        a = t[at:at + 30]
        print(json.dumps([[path.rsplit("/", 1)[-1], a, a.replace(m.group(1), m.group(1) + "x", 1), n]]))
        break
