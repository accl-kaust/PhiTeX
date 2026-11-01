"""Candidates by why a piece did or did not end there, from PHITEX_CAND_TRACE lines (observe).

    python3 -I cand-classify.py LOG.gz

The last trace line per (node, candidate) counts. Classes, first match wins: ended (cuttable);
nothing contributed since the one before (not fresh); no input left (the slice's end);
inside the run's own group, list or conditional (nest or conds non-empty); other (not a
top-level or document piece). For "inside": the nest depth and conds histogram, and the
longest run of consecutive inside candidates in one piece.
"""

import collections
import gzip
import re
import sys

R = re.compile(r"^cand: node (\d+) op (\w+) k (\d+) newgraf (\w+) fresh (\w+) input (\w+) nest (\[.*?\]) conds (\d+) forced (\w+) cuttable (\w+)")


def main(path):
    last = {}
    with gzip.open(path, "rt", errors="replace") as f:
        for line in f:
            m = R.match(line)
            if m:
                last[(m[1], int(m[3]))] = m
    cls = collections.Counter()
    depth = collections.Counter()
    by_op = collections.Counter()
    pieces_inside = collections.Counter()
    for (node, k), m in last.items():
        nest = m[7]
        levels = nest.count("(")
        if m[10] == "true":
            c = "ended there (cut)"
        elif m[5] == "false":
            c = "nothing contributed since the one before"
        elif m[6] == "false":
            c = "no input left (slice end)"
        elif levels or int(m[8]):
            c = "inside the run's own group/list/conditional"
            depth[(levels, int(m[8]), m[9])] += 1
            pieces_inside[node] += 1
        else:
            c = "other (not a top-level piece)"
        cls[c] += 1
        by_op[(m[2], c)] += 1
    print(f"{path}: {len(last)} candidates")
    for c, n in cls.most_common():
        print(f"  {n:7d}  {c}")
    print("  by op:", dict(by_op))
    print("  inside, by (nest levels, conds, forced):", dict(depth.most_common(12)))
    print(f"  pieces with candidates inside: {len(pieces_inside)}; most in one piece: {max(pieces_inside.values(), default=0)}; histogram:",
          dict(collections.Counter(min(n, 50) // 5 * 5 for n in pieces_inside.values())))


if __name__ == "__main__":
    main(sys.argv[1])
