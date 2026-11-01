#!/usr/bin/env python3
"""The gate's cold accounting and bar table (docs/spec-final.md §0, §1),
from the logs of the ams and para shards (PHITEX_TEX_TIME lines: `acct:`,
`round:`, `incr walk`) and the edit histories (gate-edits.py).

    gate-acct.py acct LOG LABEL           the §1 accounting of one build
    gate-acct.py table KIND DIR           the §0 table of a shard's runs
                                          (KIND ams or para; DIR its results)

Reference numbers are recorded, never measured here (§0): pdfTeX native
para4000 0.734 s, amsldoc converged with pdflatex 3 s.
"""
import glob
import json
import os
import re
import sys

PDFTEX = {"para": 0.734, "ams": 3.0}


def num(s):
    try:
        return float(s)
    except (TypeError, ValueError):
        return None


def acct(text):
    """The accounting lines of a log, summed over its evaluations."""
    r = {"rounds": [], "runs": 0, "live": 0, "pending": 0,
         "kinds": {}, "cp_nodes": 0, "cp_ms": 0.0, "chain": "",
         "env_calls": 0, "overhead_us": None, "threads": None, "hint_ms": 0.0}
    for m in re.finditer(r"^acct: round (\d+): wall ([0-9.]+) ms, op ([0-9.]+) ms, commit ([0-9.]+) ms, "
                         r"check ([0-9.]+) ms, idle ([0-9.]+) ms", text, re.M):
        r["rounds"].append(tuple(float(m.group(i)) for i in range(2, 7)))
    for m in re.finditer(r"^acct: (\d+) rounds, (\d+) runs, (\d+) live nodes, [0-9.]+ runs/node, "
                         r"pending waits (\d+) \([0-9.]+% of runs\), round wall minus op "
                         r"([0-9.]+) us/run at (\d+) threads", text, re.M):
        r["runs"] += int(m.group(2))
        r["live"] = int(m.group(3))
        r["pending"] += int(m.group(4))
        r["overhead_us"] = float(m.group(5))
        r["threads"] = int(m.group(6))
    for m in re.finditer(r"^acct: op time by kind: (.*)$", text, re.M):
        for k, ms, n in re.findall(r"(\w+) ([0-9.]+) ms/(\d+)", m.group(1)):
            a = r["kinds"].setdefault(k, [0.0, 0])
            a[0] += float(ms)
            a[1] += int(n)
    for m in re.finditer(r"^acct: critical path (\d+) nodes, ([0-9.]+) ms; chain kinds (\{[^}]*\}); "
                         r"env calls (\d+)", text, re.M):
        if float(m.group(2)) >= r["cp_ms"]:
            r["cp_nodes"], r["cp_ms"], r["chain"] = int(m.group(1)), float(m.group(2)), m.group(3)
        r["env_calls"] = int(m.group(4))
    for m in re.finditer(r"^incr walk: .*?, analyses ([0-9.]+)(ms|µs|us|s|ns)", text, re.M):
        u = {"s": 1e3, "ms": 1.0, "µs": 1e-3, "us": 1e-3, "ns": 1e-6}[m.group(2)]
        r["hint_ms"] += float(m.group(1)) * u
    return r


def show_acct(log, label):
    a = acct(open(log, encoding="utf-8", errors="replace").read())
    out = [f"§1 {label}: hint pass wall {a['hint_ms']:.1f} ms (Ir: callgrind run, not here); "
           f"{len(a['rounds'])} rounds"]
    for i, (w, o, c, k, idle) in enumerate(a["rounds"]):
        out.append(f"  round {i}: wall {w:.1f} ms, op {o:.1f}, commit {c:.1f}, check {k:.1f}, idle {idle:.1f}")
    kinds = ", ".join(f"{k} {v[0]:.1f} ms/{v[1]}" for k, v in sorted(a["kinds"].items()))
    out.append(f"  op time by kind: {kinds}")
    rpn = a["runs"] / a["live"] if a["live"] else 0
    pr = 100 * a["pending"] / a["runs"] if a["runs"] else 0
    out.append(f"  runs {a['runs']}, live {a['live']}, runs/node {rpn:.2f}; pending {pr:.2f}% "
               f"of runs (bar <= 1%); round wall minus op {a['overhead_us']} us/run (bar <= 10 at j1)")
    out.append(f"  critical path {a['cp_nodes']} nodes, {a['cp_ms']:.1f} ms, chain kinds {a['chain']}; "
               f"env calls {a['env_calls']}")
    print("\n".join(out))


def table(kind, d):
    """The §0 table: each bar against its finish line, from the shard's
    summary lines, logs and edit histories."""
    summary = open(os.path.join(d, f"{kind}.txt"), encoding="utf-8", errors="replace").read() \
        if os.path.exists(os.path.join(d, f"{kind}.txt")) else ""
    rows = []
    md5 = re.findall(r"-j (\d+): exit (\d+) md5 (\S+) \S+ \S+ (same|DIFFERS) wall ([0-9.]+)", summary)
    walls = {int(j): float(w) for j, _, _, _, w in md5}
    same = {int(j): s for j, _, _, s, _ in md5}
    stale = sum(int(x) for x in re.findall(r"stale_reads (\d+)", summary))
    edits = {}
    for f in glob.glob(os.path.join(d, "edits-j*.json")):
        try:
            e = json.load(open(f))
            edits[e["j"]] = e
        except (ValueError, KeyError, OSError):
            pass
    eh = ", ".join(f"j{j} {e.get('verdict', '?')}" for j, e in sorted(edits.items())) or "not run"
    rows.append(("bytes", "cold " + ", ".join(f"j{j} {s}" for j, s in sorted(same.items())) +
                 f"; edit histories {eh}; stale reads {stale}",
                 "identical every j, cold and after edit histories, 0 stale"))
    bar = PDFTEX[kind]
    w8 = walls.get(8)
    rows.append(("cold", f"j8 {w8} s (pdfTeX {bar} s)", f"< {bar} s at j8"))
    logs = {int(re.search(r"-j(\d+)\.log$", f).group(1)): acct(open(f, encoding="utf-8", errors="replace").read())
            for f in glob.glob(os.path.join(d, f"{kind}-j*.log")) if re.search(r"-j(\d+)\.log$", f)}
    sc = []
    for j in sorted(logs):
        a = logs[j]
        busy = sum(v[0] for v in a["kinds"].values()) / 1e3
        if j in walls and j:
            sc.append(f"j{j} wall {walls[j]:.2f} s vs 1.25*busy/N+cp {1.25 * busy / j + a['cp_ms'] / 1e3:.2f} s")
    rows.append(("scaling", "; ".join(sc) or "-", "wall(N) <= 1.25 busy/N + cp"))
    # (the parallel shape: walls, speedups, the SMT step j16 -> j32, the
    # critical path and the j8 round's idle, commit and op bodies)
    par = ", ".join(f"j{j} {walls[j]:.2f} s" for j in sorted(walls))
    sp = []
    for j in (8, 16, 32):
        if 1 in walls and j in walls and walls[j]:
            sp.append(f"j1/j{j} {walls[1] / walls[j]:.2f}x")
    if 16 in walls and 32 in walls:
        sp.append(f"j16->j32 {walls[16]:.2f} -> {walls[32]:.2f} s ({'faster' if walls[32] < walls[16] else 'NOT faster'})")
    a8 = logs.get(8)
    if a8:
        idle = sum(x[4] for x in a8["rounds"])
        commit = sum(x[2] for x in a8["rounds"])
        bodies = a8.get("bodies_ms")
        body = f", op bodies {bodies:.1f} ms" if bodies is not None else ""
        cp = a8.get("cp_ms")
        cps = f"cp {cp:.1f} ms; " if cp is not None else ""
        sp.append(f"{cps}j8 idle {idle:.1f} ms, commit {commit:.1f} ms{body}")
    rows.append(("parallel", par + "; " + "; ".join(sp), "j32 <= j16, then faster"))
    ew = []
    for j, e in sorted(edits.items()):
        ws = sorted(e.get("edit_walls_s") or [])
        if ws:
            ew.append(f"j{j} median {1e3 * ws[len(ws) // 2]:.0f} ms max {1e3 * ws[-1]:.0f} ms")
    rows.append(("edit", "; ".join(ew) or "not run", "<= 16 ms, fixed <= 10 ms (fixed: not split here)"))
    rows.append(("per command", "Ir/token: callgrind run, not in this shard", "<= 100"))
    a8 = logs.get(8)
    if a8:
        rows.append(("rounds", f"{len(a8['rounds'])} cold (j8)", "<= 2 cold, 1 edit"))
        rpn = a8["runs"] / a8["live"] if a8["live"] else 0
        rows.append(("runs", f"{rpn:.2f} runs/node (j8)", "<= 1.1"))
        cpf = (a8["cp_ms"] / 1e3) / w8 if w8 else 0
        rows.append(("critical path", f"{a8['cp_nodes']} nodes, {a8['cp_ms']:.0f} ms = {100 * cpf:.1f}% of j8 wall",
                     "<= 10% of cold wall at j8"))
    print(f"§0 {kind}:")
    for name, now, line in rows:
        print(f"  {name:13} {now}  [finish line: {line}]")


if __name__ == "__main__":
    if len(sys.argv) >= 4 and sys.argv[1] == "acct":
        show_acct(sys.argv[2], sys.argv[3])
    elif len(sys.argv) >= 4 and sys.argv[1] == "table":
        table(sys.argv[2], sys.argv[3])
    else:
        print(__doc__, file=sys.stderr)
        sys.exit(2)
