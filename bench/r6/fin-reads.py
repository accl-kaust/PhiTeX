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


def as_int(ints, e, x):
    """An integer value: inline (tag 2, a sign-extended 61-bit payload) or a big one."""
    if isinstance(x, int) and x >> 61 == 2:
        p = x & ((1 << 61) - 1)
        return p - (1 << 61) if p >> 60 else p
    return ints.get((e, x))


def main(path):
    names, strs, ints, lists = {}, {}, {}, {}
    extra = {}  # (e, node) -> (calls, [pitems units], [fields read by name-less calls])
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
                elif ln["t"] in ("list", "seq") and len(ln["x"]) <= 64:
                    lists[(e, ln["id"])] = ln["x"]
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
                units, ranges = [], []
                for m, a, r in ln["calls"]:
                    if m == "contribute":
                        units.append((raw(a[0]), a[1] if isinstance(a[1], int) else raw(a[1])))
                    elif m in ("acc_range", "acc_range_to"):
                        ranges.append(raw(a[0]))
                extra[(e, json.dumps(ln["node"]))] = (len(ln["calls"]), units, ranges)
    anystr = {i: x for (_, i), x in strs.items()}
    fname = lambda e, f: strs.get((e, f)) or anystr.get(f, f"#{f}")
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

    # longest chains (with the fields along the longest one)
    sys.setrecursionlimit(1000000)

    def longest(keep):
        sub = {k: [(w, n) for w, n in es if keep(n)] for k, es in edges.items()}
        sub = {k: es for k, es in sub.items() if es}
        memo, onstack = {}, set()

        def depth(k):
            if k in memo:
                return memo[k]
            if k in onstack or k not in sub:
                return (0, None)
            onstack.add(k)
            best = (0, None)
            for w, n in sub[k]:
                d = depth(w)[0] + 1
                if d > best[0]:
                    best = (d, (w, n))
            onstack.discard(k)
            memo[k] = best
            return best

        top = max(sub, key=lambda k: depth(k)[0], default=None)
        fields = collections.Counter()
        if top is None:
            return 0, fields
        k, walked = top, set()
        while k not in walked:
            walked.add(k)
            d, step = depth(k)
            if not step:
                break
            fields[step[1]] += 1
            k = step[0]
        return depth(top)[0], fields

    print("\n2. piece -> piece (writers | readers linked to one writer | readers of a value several wrote | longest chain through the field alone)")
    by_field = collections.defaultdict(dict)
    for k, es in edges.items():
        for w, n in es:
            by_field[n].setdefault(k, []).append((w, n))
    chain = {}
    for n in pp:
        saved, edges_n = edges, by_field.get(n, {})
        edges = edges_n
        chain[n] = longest(lambda m: True)[0] if edges_n else 0
        edges = saved
    rows = sorted(pp.items(), key=lambda kv: (-chain[kv[0]], -kv[1][1]))
    for n, (a, b, c) in rows[:45]:
        print(f"   {a:7d} {b:7d} {c:7d} {chain[n]:6d}  {n}")
    d, fs = longest(lambda m: True)
    print(f"   longest chain through all fields: {d}; its links by field: {dict(fs.most_common(15))}")
    d, fs = longest(lambda m: m not in ("pages:resume",))
    print(f"   without the cut's own field (pages:resume): {d}; its links by field: {dict(fs.most_common(15))}")
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
                a, b = as_int(ints, e, xr), as_int(ints, e, x)
                kinds[n][2]["+%d" % (b - a) if a is not None and b is not None else "?"] += 1
            else:
                kinds[n][1] += 1
                b = as_int(ints, e, x)
                kinds[n][2]["=%s" % b if b is not None else "=?"] += 1
    print("\n3. piece writes by kind for the linked fields (read-modify-write | set | most common forms)")
    for n, _ in rows[:45]:
        a, b, c = kinds[n]
        print(f"   {a:7d} {b:7d}  {n}: {dict(c.most_common(6))}")
    tail(path, names, strs, ints, lists, last, extra)


def tail(path, names, strs, ints, lists, last, extra):
    fname = lambda e, f: strs.get((e, f), f"#{f}")
    L = lambda e, x: lists.get((e, x))
    # 4. resumes: what they hold
    hist = collections.Counter()
    sizes = collections.Counter()
    cands = collections.Counter()
    npieces_with = inside_pieces = 0
    for key, (op, reads, writes) in last.items():
        if op not in PIECES:
            continue
        e = key[0]
        ncalls, units, ranges = extra[key]
        pit = None
        for f, u in units:
            if fname(e, f) == "pitems":
                seq = L(e, u)
                pit = L(e, seq[0]) if seq else None
        items = L(e, pit[2]) if pit and len(pit) > 2 else None
        n_items = len(items) if items is not None else -1
        per = L(e, pit[4]) if pit and len(pit) > 4 else None
        last_k = as_int(ints, e, pit[1]) if pit else None
        if op == "resume":
            cls = "no pitems unit" if pit is None else ("empty (0 items)" if n_items == 0 else ("1-2 items" if n_items <= 2 else "3+ items"))
            hist[cls] += 1
            sizes[min(ncalls // 10 * 10, 200)] += 1
        # 5. candidates inside the piece's own groups/conditionals
        if per:
            npieces_with += 1
            if any((L(e, c) or [0, 0])[1:2] and as_int(ints, e, L(e, c)[1]) == 0 for c in per[:-1]):
                inside_pieces += 1
            for i, c in enumerate(per):
                pc = L(e, c)
                if not pc or len(pc) < 2:
                    continue
                cut = as_int(ints, e, pc[1])
                if i == len(per) - 1 and last_k is not None and last_k >= 0:
                    cands["at the piece's end"] += 1
                elif cut == 0:
                    cands["inside (the run could not end there)"] += 1
                else:
                    cands["could end there, did not"] += 1
    print("\n4. resumes by what their items unit holds:", dict(hist))
    print("   resumes by engine calls in their last run (bucket of 10):", dict(sorted(sizes.items())))
    print("   pieces with a candidate inside:", inside_pieces, "of", npieces_with)
    print("\n5. candidates of the pieces (from their check data; a piece with one candidate at its end has none):", dict(cands))
    # 6. page-state readers
    ps = collections.Counter()
    for key, (op, reads, writes) in last.items():
        if op not in PIECES:
            continue
        e = key[0]
        seen = set()
        for f, _ in reads:
            n = fname(e, f)
            if n.split(":")[0] in ("scan", "main", "vlist", "page", "pages") or n in ("deadcycles", "outputpenalty", "insertpenalties"):
                seen.add(n)
        for f in extra[key][2]:
            n = fname(e, f)
            seen.add("range " + n)
        for n in seen:
            ps[n] += 1
    print("\n6. pieces reading page/list state, by field (pieces):", dict(ps.most_common(30)))


if __name__ == "__main__":
    main(sys.argv[1])
