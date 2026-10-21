"""Which pieces read what an output routine defines (from a DEFUSE_RECORD recording).

    python3 -I fin-reads.py REC.gz

The last finished run of each node counts. Finishers are nodes of op `finish`. Pieces are
nodes of op `exec` or `resume`. A finisher's definitions are its define/define_with/publish
calls. For each field that some finisher defines, the script reports how many pieces read
it, and how many of those reads got a value that only finishers define (no other node
defines that value): those pieces would be refuted if they ran with output invisible.
"""

import collections
import gzip
import json
import sys

PIECES = {"exec", "resume"}
READS = {"use_field", "use_field_at", "use_field_before", "use_field_proj"}
DEFS = {"define", "define_with", "publish"}


def lines(path):
    with gzip.open(path, "rt") as f:
        for line in f:
            if '"k":"val"' in line and '"t":"str"' not in line:
                continue
            yield json.loads(line)


def raw(x):
    """A field/value as serialized: a number, or a one-element wrapper."""
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


def run_ok(ln):
    return ln.get("r") != "stop"


def main(path):
    names = {}  # (e, op id) -> name
    strs = {}  # (e, value id) -> str
    fin_defs = {}  # (e, node) -> [(field, value)] of its last finished run
    for ln in lines(path):
        k = ln.get("k")
        e = ln.get("e")
        if k == "val":
            strs[(e, ln["id"])] = ln["x"]
        elif k == "api" and ln.get("f") == "register":
            names[(e, raw(ln["r"]))] = ln["a"][0]
        elif k == "run" and run_ok(ln) and names.get((e, ln["op"])) == "finish":
            d = []
            for m, a, _ in ln["calls"]:
                if m in DEFS:
                    d.append((raw(a[0]), raw(a[1])))
            fin_defs[(e, json.dumps(ln["node"]))] = d
    fin_fields = collections.defaultdict(set)  # (e, field) -> values finishers define
    for (e, _), d in fin_defs.items():
        for f, x in d:
            fin_fields[(e, f)].add(x)
    other_vals = collections.defaultdict(set)  # (e, field) -> values non-finishers define
    piece_reads = {}  # (e, node) -> [(field, value)]
    piece_ops = {}
    for ln in lines(path):
        if ln.get("k") != "run" or not run_ok(ln):
            continue
        e = ln["e"]
        op = names.get((e, ln["op"]), "?")
        node = json.dumps(ln["node"])
        rd = []
        for m, a, r in ln["calls"]:
            if m in DEFS and op != "finish":
                f = raw(a[0])
                if (e, f) in fin_fields:
                    other_vals[(e, f)].add(raw(a[1]))
            elif m in READS and op in PIECES:
                f = raw(a[0])
                if (e, f) in fin_fields and r != "stop":
                    rd.append((f, raw(r)))
        if op in PIECES:
            piece_reads[(e, node)] = rd
            piece_ops[(e, node)] = op
    by_field = collections.defaultdict(lambda: [0, 0])  # name -> [pieces reading, only-finisher value]
    refuted = set()
    pieces = len(piece_reads)
    for (e, node), rd in piece_reads.items():
        seen = set()
        for f, x in rd:
            if f in seen:
                continue
            seen.add(f)
            name = strs.get((e, f), f"#{f}")
            by_field[name][0] += 1
            if x in fin_fields[(e, f)] and x not in other_vals[(e, f)]:
                by_field[name][1] += 1
                refuted.add((e, node))
    nfin = len(fin_defs)
    print(f"{path}: {pieces} pieces, {nfin} finishers, {len(fin_fields)} fields finishers define")
    print(f"pieces reading a value only a finisher defines (refuted with output invisible): {len(refuted)}")
    print("by field (pieces reading it, of them reading a finisher-only value):")
    for name, (n, o) in sorted(by_field.items(), key=lambda kv: (-kv[1][1], -kv[1][0])):
        print(f"  {n:6d} {o:6d}  {name}")
    print("fields finishers define that no piece reads:", ", ".join(sorted(
        strs.get(k, f"#{k[1]}") for k in fin_fields if strs.get(k, f"#{k[1]}") not in by_field))[:2000])


if __name__ == "__main__":
    main(sys.argv[1])
