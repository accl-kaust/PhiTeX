#!/usr/bin/env python3
"""How many nodes of the pure graph an edit re-evaluates (DESIGN 3.17,
"Measured"), from two traces of the pure SSA tracer:

    scripts/pure-diff.py OLD NEW

OLD and NEW are `PARTEX_PURE_TRACE` paths (OLD.nodes, OLD.lines, ...): a
plain build of the source before the edit and one after. A node is
re-evaluated if it is new (the edit changed the source text it read) or
if one of its operands changed: its operand hash (the versions it read,
field-level for the line breaker, the packs and the page steps; the
tokens it read) differs from the old node's in its place.

The body and the page chain are aligned apart (a page that breaks
elsewhere moves the output routine among the body's nodes, not the
body's nodes among themselves): each by its equal runs of (kind, source
hash), re-synchronised after each difference at the nearest window of 32
equal keys; the nodes of NEW not aligned are new. The line breaker's
lines are compared in aligned `line_break` nodes: a line whose contents
changed is one page-stream segment to splice.

Each count is a range: the low end takes the operands the tracer has no
version for (`\\read` and `\\write` streams' state, the font table's
shape, the generator) as unchanged; the high end takes every aligned node
that read one, after the first difference, as re-evaluated.
"""
import sys
from collections import Counter
from pathlib import Path

import numpy as np

DT = np.dtype([("kind", "<u2"), ("cls", "u1"), ("flags", "u1"), ("src", "<u4"), ("op", "<u4")])
LINE_BREAK = 512 + 5


def load(base):
    nodes = np.fromfile(f"{base}.nodes", dtype=DT)
    raw = np.fromfile(f"{base}.lines", dtype="<u4")
    lines, i = [], 0
    while i < len(raw):
        n = int(raw[i])
        lines.append(raw[i + 1:i + 1 + n])
        i += 1 + n
    return nodes, lines


W = 32       # a window that re-synchronises the two sequences
LOOK = 200_000  # how far past a difference to look for it


def keys(x):
    return (x["kind"].astype(np.uint64) << np.uint64(32)) | x["src"].astype(np.uint64)


def window_hashes(k):
    """A hash of each window of W keys starting at each position."""
    if len(k) < W:
        return np.zeros(0, dtype=np.uint64)
    h = np.zeros(len(k) - W + 1, dtype=np.uint64)
    with np.errstate(over="ignore"):
        for t in range(W):
            h = h * np.uint64(1_000_003) + k[t:len(k) - W + 1 + t]
    return h


def run_equal(ka, kb, i, j):
    n = min(len(ka) - i, len(kb) - j)
    bad = np.flatnonzero(ka[i:i + n] != kb[j:j + n])
    return int(bad[0]) if len(bad) else n


def align(a, b):
    """Aligned index pairs (pa, pb) of a and b: equal runs of (kind,
    source), re-synchronised after each difference at the nearest
    window of W equal keys; the rest of b is new."""
    ka, kb = keys(a), keys(b)
    ha, hb = window_hashes(ka), window_hashes(kb)
    pa, pb = [], []
    i = j = 0
    while i < len(ka) and j < len(kb):
        n = run_equal(ka, kb, i, j)
        pa.append(np.arange(i, i + n))
        pb.append(np.arange(j, j + n))
        i += n
        j += n
        if i >= len(ka) or j >= len(kb):
            break
        # the nearest re-synchronisation: windows of b after j, by hash
        found = None
        sa = ha[i:i + LOOK]
        sb = hb[j:j + LOOK]
        if len(sa) and len(sb):
            first_b = {}
            for q, h in enumerate(sb.tolist()):
                first_b.setdefault(h, q)
            best = None
            for q, h in enumerate(sa.tolist()):
                r = first_b.get(h)
                if r is not None and (best is None or q + r < best[0] + best[1]):
                    best = (q, r)
                if best is not None and q > best[0] + best[1]:
                    break
            found = best
        if found is None:
            break
        i += found[0]
        j += found[1]
    pa = np.concatenate(pa) if pa else np.zeros(0, dtype=int)
    pb = np.concatenate(pb) if pb else np.zeros(0, dtype=int)
    return pa.astype(int), pb.astype(int)


def main():
    old, old_lines = load(sys.argv[1])
    new, new_lines = load(sys.argv[2])
    # the line breaker's nodes, numbered in each trace's order
    lb_old = np.cumsum(old["kind"] == LINE_BREAK) - 1
    lb_new = np.cumsum(new["kind"] == LINE_BREAK) - 1
    total = {"low": 0, "high": 0}
    kinds = Counter()
    segments = 0
    for cls, name in [(0, "body"), (1, "page chain")]:
        ia = np.flatnonzero(old["cls"] == cls)
        ib = np.flatnonzero(new["cls"] == cls)
        a, b = old[ia], new[ib]
        pa, pb = align(a, b)
        new_b = np.ones(len(b), dtype=bool)
        new_b[pb] = False
        mid = int(new_b.sum())
        changed = a["op"][pa] != b["op"][pb]
        first = np.flatnonzero(changed)
        first = int(pb[first[0]]) if len(first) else len(b)
        if mid:
            first = min(first, int(np.flatnonzero(new_b)[0]))
        unv = ((b["flags"][pb] & 1) != 0) & (pb >= first) & ~changed
        low = mid + int(changed.sum())
        high = low + int(unv.sum())
        total["low"] += low
        total["high"] += high
        for k in b["kind"][pb[changed]]:
            kinds[(name, int(k))] += 1
        for k in b["kind"][new_b]:
            kinds[(name, int(k))] += 1
        # the lines of aligned line_break nodes whose contents changed
        for i, j in zip(ia[pa], ib[pb]):
            if old["kind"][i] != LINE_BREAK:
                continue
            la, lb = old_lines[lb_old[i]], new_lines[lb_new[j]]
            if len(la) != len(lb):
                segments += max(len(la), len(lb))
            else:
                segments += int((la != lb).sum())
        for j in ib[new_b]:
            if new["kind"][j] == LINE_BREAK:
                segments += len(new_lines[lb_new[j]])
        print(f"{name}: old {len(a):,} new {len(b):,} nodes; aligned {len(pb):,}, "
              f"new {mid:,}; re-evaluated {low:,}..{high:,}")
    print(f"re-evaluated nodes: {total['low']:,}..{total['high']:,} "
          f"of {len(new):,}; lines whose contents changed (page-stream segments): {segments:,}")
    print("re-evaluated by kind (class, kind, nodes):")
    for (name, k), n in kinds.most_common(25):
        print(f"  {name:<10} {k:>5} {n:>9,}")


if __name__ == "__main__":
    main()
