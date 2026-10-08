#!/usr/bin/env python3
"""How many nodes of the pure graph an edit re-evaluates (DESIGN 3.17.6),
from two traces of the pure SSA tracer:

    scripts/pure-diff.py OLD NEW

OLD and NEW are `PARTEX_PURE_TRACE` paths (OLD.nodes, OLD.lines): a plain
build of the source before the edit and one after. A node is re-evaluated
if it is new (the edit changed what it read of the source) or if one of
its operands changed: its operand hash (the versions it read, field-level
for the line breaker, the packs and the page steps; the tokens it read)
differs from the old node's in its place.

The body and the page chain are aligned apart (a page that breaks
elsewhere moves the output routine among the body's nodes, not the body's
nodes among themselves): each by its equal runs of (kind, source hash),
re-synchronised after each difference at the nearest window of 32 equal
keys; the nodes of NEW not aligned are new. Segments: the lines of aligned
`line_break` nodes whose contents changed, and every line of a new one.

Each count is a range: the low end takes the operands the tracer has no
version for (`\\read` and `\\write` streams' state, the font table's
shape, the generator) as unchanged; the high end takes every aligned node
that read one, after the first difference, as re-evaluated.

Plain Python (no numpy): the cluster's image has none.
"""
import struct
import sys
from array import array
from collections import Counter

LINE_BREAK = 512 + 5
W = 32
LOOK = 200_000
REC = struct.Struct("<HBBII")
KEY = struct.Struct("<HI")


class Stream:
    """One class's nodes: keys (kind, source) as 6 bytes each, operand
    hashes, flags, kinds, and the line breaker's nodes."""

    def __init__(self):
        self.keys = bytearray()
        self.ops = array("I")
        self.flags = bytearray()
        self.kinds = array("H")
        self.lb = {}  # index -> its lines


def load(base):
    data = open(f"{base}.nodes", "rb").read()
    raw = array("I")
    raw.frombytes(open(f"{base}.lines", "rb").read())
    lines, i = [], 0
    while i < len(raw):
        n = raw[i]
        lines.append(tuple(raw[i + 1:i + 1 + n]))
        i += 1 + n
    s = (Stream(), Stream())
    lbn = 0
    for kind, cls, flags, src, op in REC.iter_unpack(data):
        t = s[1 if cls else 0]
        if kind == LINE_BREAK:
            t.lb[len(t.ops)] = lines[lbn] if lbn < len(lines) else ()
            lbn += 1
        t.keys += KEY.pack(kind, src)
        t.ops.append(op)
        t.flags.append(flags & 1)
        t.kinds.append(kind)
    return s


def run_equal(ka, kb, i, j):
    """The length of the equal run of keys from i in a and j in b."""
    a, b = memoryview(ka), memoryview(kb)
    n = min(len(ka) // 6 - i, len(kb) // 6 - j)
    if a[6 * i:6 * (i + n)] == b[6 * j:6 * (j + n)]:
        return n
    lo, step = 0, 1
    while step < n and a[6 * i:6 * (i + step)] == b[6 * j:6 * (j + step)]:
        lo = step
        step *= 2
    hi = min(step, n)
    while hi - lo > 1:
        m = (lo + hi) // 2
        if a[6 * i:6 * (i + m)] == b[6 * j:6 * (j + m)]:
            lo = m
        else:
            hi = m
    return lo


def resync(ka, kb, i, j):
    na, nb = len(ka) // 6, len(kb) // 6
    first_b = {}
    for q in range(max(0, min(LOOK, nb - j - W + 1))):
        first_b.setdefault(bytes(kb[6 * (j + q):6 * (j + q + W)]), q)
    best = None
    for q in range(max(0, min(LOOK, na - i - W + 1))):
        r = first_b.get(bytes(ka[6 * (i + q):6 * (i + q + W)]))
        if r is not None and (best is None or q + r < sum(best)):
            best = (q, r)
        if best is not None and q > sum(best):
            break
    return best


def mismatches(oa, ob, i, j, n, out, base=0):
    """Offsets k < n where oa[i+k] != ob[j+k], by halving."""
    if n <= 0 or oa[i:i + n] == ob[j:j + n]:
        return
    if n <= 64:
        out.extend(base + k for k in range(n) if oa[i + k] != ob[j + k])
        return
    h = n // 2
    mismatches(oa, ob, i, j, h, out, base)
    mismatches(oa, ob, i + h, j + h, n - h, out, base + h)


def compare(a, b, name, kinds):
    runs = []  # (i, j, n)
    i = j = 0
    na, nb = len(a.ops), len(b.ops)
    while i < na and j < nb:
        n = run_equal(a.keys, b.keys, i, j)
        if n:
            runs.append((i, j, n))
        i += n
        j += n
        if i >= na or j >= nb:
            break
        r = resync(a.keys, b.keys, i, j)
        if r is None:
            break
        i += r[0]
        j += r[1]
    covered = bytearray(nb)
    for _, j, n in runs:
        covered[j:j + n] = b"\x01" * n
    aligned = sum(n for _, _, n in runs)
    new = nb - aligned
    first = covered.find(0)
    first = nb if first < 0 else first
    changed = []
    for i, j, n in runs:
        off = []
        mismatches(a.ops, b.ops, i, j, n, off)
        changed.extend(j + k for k in off)
    if changed:
        first = min(first, min(changed))
    cset = set(changed)
    unv = 0
    for _, j, n in runs:
        lo = max(j, first)
        if lo < j + n:
            unv += b.flags[lo:j + n].count(1)
    unv -= sum(1 for c in cset if c >= first and b.flags[c])
    for c in changed:
        kinds[(name, b.kinds[c])] += 1
    q = covered.find(0)
    while q >= 0:
        kinds[(name, b.kinds[q])] += 1
        q = covered.find(0, q + 1)
    # segments
    seg = 0
    for jdx, now in b.lb.items():
        if not covered[jdx]:
            seg += len(now)
            continue
        for i, j, n in runs:
            if j <= jdx < j + n:
                old = a.lb.get(i + (jdx - j), ())
                if len(old) != len(now):
                    seg += max(len(old), len(now))
                else:
                    seg += sum(1 for x, y in zip(old, now) if x != y)
                break
    low = new + len(changed)
    print(f"{name}: old {na:,} new {nb:,} nodes; aligned {aligned:,}, new {new:,}; "
          f"re-evaluated {low:,}..{low + unv:,}")
    return low, low + unv, seg


def main():
    old, new = load(sys.argv[1]), load(sys.argv[2])
    kinds = Counter()
    lo = hi = seg = 0
    for cls, name in [(0, "body"), (1, "page chain")]:
        l, h, s = compare(old[cls], new[cls], name, kinds)
        lo, hi, seg = lo + l, hi + h, seg + s
    total = len(new[0].ops) + len(new[1].ops)
    print(f"re-evaluated nodes: {lo:,}..{hi:,} of {total:,}; "
          f"lines whose contents changed (page-stream segments): {seg:,}")
    print("re-evaluated by kind (class, kind, nodes):")
    for (name, k), n in kinds.most_common(25):
        print(f"  {name:<10} {k:>5} {n:>9,}")


if __name__ == "__main__":
    main()
