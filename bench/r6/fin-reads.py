"""Def-use links among pieces and finishers in a DEFUSE_RECORD recording (round core).

    python3 -I fin-reads.py REC.gz

The last finished run of each node counts. Finishers are nodes of op `finish`; pieces are
nodes of op `exec` or `resume`. A run's reads are its use_field* calls (the field and the
value answered); its writes are its define/define_with/publish calls.

A read is linked to the node that wrote the value it got when exactly one node of the
recording wrote that value to that field. Reported:

1. Finisher -> piece: for each field a finisher writes, the pieces reading it, and of them
   those that got a value that only a finisher wrote (refuted if output were invisible).
2. Piece -> piece: for each field, the pieces writing it, the pieces reading a value one
   other piece wrote (linked) or one of several pieces wrote (ambiguous); the longest
   chain of linked reads through that field alone, and through all fields together.
3. Writes by kind per field: read-modify-write (the run read the field before writing it:
   the difference, if both are integers) or set (no read before): whether a counter's
   writes are all add n / set n.
"""

import collections
import gzip
import json
import sys

PIECES = {"exec", "resume"}
READS = {"use_field", "use_field_at", "use_field_before", "use_field_proj"}
DEFS = {"define", "define_with", "publish"}


def raw(x):
    while isinstance(x, (list, dict)):
        if isinstance(x, dict):
            if "ok" in x:
                x = x["ok"]
                continue
            return None
        if not x:
            return None
        x = x[0]
    return x


def main(path):
    names, strs, ints = {}, {}, {}
    last = {}  # (e, node) -> (op name, reads [(f, x)], writes [(f, x, read before, x read)])
    with gzip.open(path, "rt") as fh:
        for line in fh:
            ln = json.loads(line)
            k = ln.get("k")
            e = ln.get("e")
            if k == "val":
                if ln["t"] == "str":
                    strs[(e, ln["id"])] = ln["x"]
                elif ln["t"] == "int":
                    ints[(e, ln["id"])] = ln["x"]
            elif k == "api" and ln.get("f") == "register":
                names[(e, raw(ln["r"]))] = ln["a"][0]
            elif k == "run" and ln.get("r") != "stop":
                op = names.get((e, ln["op"]), "?")
                if op not in PIECES and op != "finish":
                    continue
                reads, writes, seen = [], [], {}
                for m, a, r in ln["calls"]:
                    if m in READS and r != "stop":
                        f, x = raw(a[0]), raw(r)
                        reads.append((f, x))
                        seen.setdefault(f, x)
                    elif m in DEFS:
                        f = raw(a[0])
                        writes.append((f, raw(a[1]), f in seen, seen.get(f)))
                last[(e, json.dumps(ln["node"]))] = (op, reads, writes)
    fname = lambda e, f: strs.get((e, f), f"#{f}")
    # who wrote (field, value)
    writers = collections.defaultdict(set)
    for key, (op, _, writes) in last.items():
        for f, x, _, _ in writes:
            writers[(key[0], f, x)].add(key)
    fin_fields = {(key[0], f) for key, (op, _, w) in last.items() if op == "finish" for f, _, _, _ in w}
    npieces = sum(1 for v in last.values() if v[0] in PIECES)
    nfin = sum(1 for v in last.values() if v[0] == "finish")
    print(f"{path}: {npieces} pieces, {nfin} finishers")
    # 1. finisher -> piece
    fp = collections.defaultdict(lambda: [0, 0])
    refuted = set()
    # 2. piece -> piece
    pp = collections.defaultdict(lambda: [0, 0, 0])  # field -> [writers, linked readers, ambiguous]
    edges = collections.defaultdict(list)  # (e, reader) -> [(writer, field name)]
    for key, (op, reads, writes) in last.items():
        if op not in PIECES:
            continue
        e = key[0]
        done = set()
        for f, x in reads:
            if f in done:
                continue
            done.add(f)
            ws = writers.get((e, f, x), set()) - {key}
            n = fname(e, f)
            if (e, f) in fin_fields:
                fp[n][0] += 1
                if ws and all(last[w][0] == "finish" for w in ws):
                    fp[n][1] += 1
                    refuted.add(key)
            pws = {w for w in ws if last[w][0] in PIECES}
            if len(pws) == 1 and len(ws) == 1:
                pp[n][1] += 1
                edges[key].append((next(iter(pws)), n))
            elif pws:
                pp[n][2] += 1
    for key, (op, _, writes) in last.items():
        if op in PIECES:
            for f in {w[0] for w in writes}:
                pp[fname(key[0], f)][0] += 1
    print(f"\n1. finisher -> piece: {len(refuted)} pieces read a value only a finisher wrote")
    print("   pieces reading | of them a finisher-only value | field")
    for n, (a, b) in sorted(fp.items(), key=lambda kv: (-kv[1][1], -kv[1][0]))[:40]:
        print(f"   {a:7d} {b:7d}  {n}")

    # longest chains
    def longest(filter_field):
        memo, onstack = {}, set()

        def depth(k):
            if k in memo:
                return memo[k]
            if k in onstack:
                return 0
            onstack.add(k)
            best = 0
            for w, n in edges.get(k, ()):
                if filter_field is None or n == filter_field:
                    best = max(best, 1 + depth(w))
            onstack.discard(k)
            memo[k] = best
            return best

        sys.setrecursionlimit(1000000)
        return max((depth(k) for k in edges), default=0)

    print("\n2. piece -> piece (writers | readers linked to one writer | readers of a value several wrote | longest chain through the field alone)")
    rows = sorted(pp.items(), key=lambda kv: -kv[1][1])
    for n, (a, b, c) in rows[:45]:
        print(f"   {a:7d} {b:7d} {c:7d} {longest(n) if b else 0:6d}  {n}")
    print(f"   longest chain through all fields: {longest(None)}")
    # 3. writes by kind
    kinds = collections.defaultdict(lambda: [0, 0, collections.Counter()])  # field -> [rmw, set, deltas]
    for key, (op, _, writes) in last.items():
        if op not in PIECES:
            continue
        e = key[0]
        for f, x, rb, xr in writes:
            n = fname(e, f)
            if rb:
                kinds[n][0] += 1
                a, b = ints.get((e, xr)), ints.get((e, x))
                kinds[n][2]["+%d" % (b - a) if a is not None and b is not None else "?"] += 1
            else:
                kinds[n][1] += 1
                b = ints.get((e, x))
                kinds[n][2]["=%s" % b if b is not None else "=?"] += 1
    print("\n3. piece writes by kind for the linked fields (read-modify-write | set | most common forms)")
    for n, _ in rows[:45]:
        a, b, c = kinds[n]
        print(f"   {a:7d} {b:7d}  {n}: {dict(c.most_common(6))}")


if __name__ == "__main__":
    main(sys.argv[1])
