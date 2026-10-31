"""amsldoc's realised critical chain by link kind (bench/r15/chain.sh's PHITEX_CHAIN_TSV).

    python3 -I chain.py CHAIN.tsv [--eval K] [--top N]

Each line of the dump is a run on the chain, highest first; it waited on the next line's run
for the cause it names. A link is (reader, writer, cause): its time on the path is the
reader's own run (end - start of the run the chain records; all attempts beside it). The
chain is the engine's height chain (which run's result rests on which), not a wall-time path:
a reader may have run on a guess before its writer and been confirmed later. The evaluation taken is the one with the longest chain
(--eval K: the K-th, from 0).
"""

import collections
import sys

COLS = ["i", "h", "node", "op", "cause", "field", "extra", "ready", "start", "end", "commit", "runns", "unit", "parent", "relays", "src", "before", "val", "text"]
PIECES = {"exec", "resume"}


def load(path):
    evals, cur = [], None
    for ln in open(path, encoding="utf-8", errors="replace"):
        if ln.startswith("# eval"):
            cur = {"head": ln.split()[2:], "rows": []}
            evals.append(cur)
            continue
        p = ln.rstrip("\n").split("\t", len(COLS) - 1)
        if len(p) < len(COLS) or cur is None:
            continue
        d = dict(zip(COLS, p))
        for k in ("i", "h", "node", "ready", "start", "end", "commit", "runns", "parent", "relays", "src", "before"):
            d[k] = int(d[k] or -1)
        d["text"] = d["text"].replace("\\n", "\n").replace("\\t", "\t").replace("\\\\", "\\")
        cur["rows"].append(d)
    return evals


def head(f):
    return f.split(":")[0] if ":" in f else f


def infer_op(r):
    """A run freed after the evaluation (no node info): its operation from what it read."""
    if r["op"] != "?":
        return r["op"]
    f = r["field"]
    if f.startswith("rtn:") or f.startswith("scan:") or (r["cause"] == "Range" and f == "vprof"):
        return "scan"
    if r["cause"] == "Created":
        return "finish"
    return "?"


LOAD = ("package record", "package not recorded")


def kind(r, w):
    """The link's kind: reader r waited on writer w (w None: the chain's bottom)."""
    c, f = r["cause"], r["field"]
    op, wop = infer_op(r), (infer_op(w) if w else None)
    h = head(f)
    relay = r["relays"] > 0
    if w is None:
        return "bottom (the first run)", f or c
    if c == "Loop":
        return "aux read-back (rounds)", f
    if any(x in r["text"] for x in LOAD) and op in PIECES:
        return "preamble load reading the one before", f
    if op in ("scan", "finish", "files") or h in ("rtn", "scan", "pages") or (c == "Range" and f == "vprof"):
        return "page node (scan/cont/finish)", f or c
    if wop == "finish" and op in PIECES:
        return "finisher's definitions read by text", f
    if c in ("Raised", "Woken", "Structure") and h != "lines":
        return "engine wait (no data)", c + " " + f
    if h == "count":
        return "counter", f
    if h in ("vlist", "main", "pg"):
        return "main list tail", f
    if "prevgraf" in f or "lastbox" in f:
        return "prevgraf/lastbox after a paragraph", f
    if f == "aftertoken":
        return "other: the pending afterassignment token, empty, defined again unchanged" if relay else "other: the pending afterassignment token", f
    if f == "parpar":
        return "other: the paragraph-shape record (parpar)", f
    if f == "cattab":
        return "other: the catcode record (cattab)", f
    if h == "lines":
        return "other: the input line count (lines, a fold declared statically)", f
    if c in ("Changed", "Rebind", "ChangedStart", "CreatedHint"):
        ff = f or r["extra"]
        return ("macro or register: a same-value redefinition (relay)" if relay else "macro or register def-use (TeX's chain)"), ff
    if c == "Created":
        return "other: creation (a run unfolds the next)", f"{wop}->{op}"
    return "other: " + c, f


def first_line(t, n=90):
    t = t.strip()
    return t.split("\n")[0][:n]


def main():
    args = sys.argv[1:]
    path = args[0]
    top = int(args[args.index("--top") + 1]) if "--top" in args else 10
    evals = load(path)
    if not evals:
        print("no evaluation")
        return
    if "--eval" in args:
        ev = evals[int(args[args.index("--eval") + 1])]
    else:
        ev = max(evals, key=lambda e: len(e["rows"]))
    rows = ev["rows"]
    print(f"{path}: {len(evals)} evaluations; taken: {len(rows)} links, realised path {ev['head'][1]} runs, {int(ev['head'][2]) / 1e6:.1f} ms (longest by run time)")
    span = (rows[0]["commit"] - rows[-1]["ready"]) / 1e6 if rows else 0
    ops = sum(r["end"] - r["start"] for r in rows) / 1e6
    print(f"chain wall (bottom queued -> top committed) {span:.1f} ms; op on the chain {ops:.1f} ms")
    by = collections.OrderedDict()
    fields = collections.defaultdict(collections.Counter)
    links = []
    for k, r in enumerate(rows):
        w = rows[k + 1] if k + 1 < len(rows) else None
        kd, f = kind(r, w)
        wall = r["runns"] / 1e6
        op = (r["end"] - r["start"]) / 1e6
        e = by.setdefault(kd, [0, 0.0, 0.0])
        e[0] += 1
        e[1] += wall
        e[2] += op
        fields[kd][f] += 1
        links.append((wall, op, kd, f, r, w))
    print("\nkind | links | ms op (the run on the path) | ms all attempts | fields (links)")
    for kd, (n, wl, op) in sorted(by.items(), key=lambda kv: -kv[1][2]):
        fs = ", ".join(f"{f or '-'} {c}" for f, c in fields[kd].most_common(8))
        print(f"{kd} | {n} | {op:.1f} | {wl:.1f} | {fs}")
    cc = collections.Counter((r["cause"], head(r["field"]), r["op"], (rows[k + 1]["op"] if k + 1 < len(rows) else "-")) for k, r in enumerate(rows))
    print("\ncause, field head, reader op, writer op: links")
    for (c, h, a, b), n in cc.most_common(40):
        print(f"  {c} {h or '-'} {a}<-{b}: {n}")
    print(f"\nthe {top} longest links (the reader's run):")
    for wall, op, kd, f, r, w in sorted(links, key=lambda x: -x[1])[:top]:
        print(f"  {op:.1f} ms (all attempts {wall:.1f}) [{kd}] {r['cause']} {f} relays {r['relays']}")
        print(f"     reader #{r['i']} {r['op']} n{r['node']}: {first_line(r['text'])!r}")
        if w:
            print(f"     writer #{w['i']} {w['op']} n{w['node']}: {first_line(w['text'])!r}")
    print("\nthe chain in order (bottom first): i op cause field wall op | text")
    for k in range(len(rows) - 1, -1, -1):
        r = rows[k]
        w = rows[k + 1] if k + 1 < len(rows) else None
        wall = r["runns"] / 1e6
        print(f"  {r['i']:4d} {infer_op(r):7s} {r['cause']:12s} {r['field'][:28]:28s} {r['extra'][:16]:16s} {wall:7.2f} {(r['end'] - r['start']) / 1e6:7.2f} | {first_line(r['text'], 70)!r}")


main()
