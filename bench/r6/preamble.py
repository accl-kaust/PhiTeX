"""Preamble load dependencies and format-value guesses from a recording (see docs/note-preamble.md).

    python3 -I preamble.py REC.gz LOAD,NODE,IDS
"""
import gzip, json, sys, collections
path = sys.argv[1]
loads = set(int(x) for x in sys.argv[2].split(","))
strs, lists, names, last = {}, {}, {}, {}
with gzip.open(path, "rt") as f:
    for line in f:
        ln = json.loads(line)
        k = ln.get("k")
        if k == "val":
            if ln["t"] == "str": strs[ln["id"]] = ln["x"]
            elif ln["t"] == "list" and len(ln["x"]) <= 16: lists[ln["id"]] = ln["x"]
        elif k == "api" and ln.get("f") == "register":
            names[ln["r"]] = ln["a"][0]
        elif k == "run" and ln.get("r") != "stop" and names.get(ln["op"]) in ("exec", "resume"):
            reads, writes = [], []
            for m, a, r in ln["calls"]:
                if m.startswith("use_field") and r != "stop" and isinstance(a, list) and a:
                    x = r.get("ok") if isinstance(r, dict) else None
                    while isinstance(x, (dict, list)):
                        x = next(iter(x.values())) if isinstance(x, dict) else (x[0] if x else None)
                    reads.append((a[0], x))
                elif m in ("define", "define_with", "publish"):
                    writes.append((a[0], a[1]))
            last[ln["node"]] = (names[ln["op"]], ln["a"], reads, writes)
def text(node):
    op, a, _, _ = last[node]
    try:
        seg = lists[lists[a[0]][0]]
        return strs.get(seg[1], "?")[:60].replace("\n", " ")
    except Exception:
        return "?"
small = sorted(n for n in last if n < 100000)
writers = collections.defaultdict(set)
for n, (op, a, r, w) in last.items():
    for f, x in w:
        writers[(f, x)].add(n)
# document order: small ids are top-level slices in order; others are resumes/children (unknown position)
begin = next((n for n in small if "begin{document}" in text(n)), None)
print("load nodes:", sorted(loads), "begin-document node:", begin)
for n in sorted(loads):
    print(f"  load {n}: {text(n)}")
# load -> load deps
def deps(n):
    d = collections.Counter()
    for f, x in last[n][2]:
        ws = writers.get((f, x), set()) - {n}
        if len(ws) == 1:
            w = next(iter(ws))
            if w in loads:
                d[w] += 1
    return d
for n in sorted(loads):
    print(f"  load {n} reads values only these loads wrote: {dict(deps(n))}")
# body pieces: everything not a load and not before begin (small ids < begin are preamble)
lat = collections.Counter()
nodes_dep = 0
for n in last:
    if n in loads or (n < 100000 and begin is not None and n < begin):
        continue
    d = deps(n)
    m = max(d) if d else None
    lat[m] += 1
print("body and other pieces by the latest load they read a value of:", dict(sorted(lat.items(), key=lambda kv: (kv[0] is None, kv[0] or 0))))
# does every piece depend on begin-document?
if begin:
    dep_begin = 0
    for n in last:
        if n in loads or n == begin or (n < 100000 and n < begin):
            continue
        if any(len(writers.get((f, x), set()) - {n}) == 1 and next(iter(writers[(f, x)] - {n})) == begin for f, x in last[n][2]):
            dep_begin += 1
    print("pieces reading a value only the begin-document node wrote:", dep_begin, "of", len(last))
    print("begin-document node reads values only these loads wrote:", dict(deps(begin)))
# fields body pieces read from the begin-document node and from the last load
for src in (begin, 170):
    fc = collections.Counter()
    for n in last:
        if n in loads or n == src or (n < 100000 and n < begin):
            continue
        seen = set()
        for f, x in last[n][2]:
            ws = writers.get((f, x), set()) - {n}
            if ws == {src} and f not in seen:
                seen.add(f)
                fc[strs.get(f, f"#{f}")] += 1
    print(f"fields body pieces read from node {src} (pieces):", dict(fc.most_common(25)))
# guesses from the format's values: a body piece's reads of fields the preamble defines,
# compared with the format environment's value
env = {}
with gzip.open(path, "rt") as f:
    for line in f:
        if '"set_environment"' in line:
            d = json.loads(line)
            if len(d["a"]) > len(env):
                env = {fv[0]: fv[1] for fv in d["a"]}
pre_nodes = set(loads) | {n for n in last if n < 100000 and begin is not None and n <= begin}
pre_fields = set()
for n in pre_nodes:
    for fld, x in last[n][3]:
        pre_fields.add(fld)
ok = bad = 0
diff = collections.Counter()
for n in last:
    if n in pre_nodes:
        continue
    wrong = set()
    for fld, x in last[n][2]:
        if fld in pre_fields and env.get(fld) != x:
            # the value read came from somewhere: is it a preamble node's?
            ws = writers.get((fld, x), set())
            if ws & pre_nodes and not (ws - pre_nodes - {n}):
                wrong.add(strs.get(fld, f"#{fld}"))
    if wrong:
        bad += 1
        for w in wrong:
            diff[w] += 1
    else:
        ok += 1
print(f"body pieces whose reads of preamble-defined fields all equal the format's values: {ok}; not: {bad}")
print("fields that differ (pieces):", dict(diff.most_common(30)))
