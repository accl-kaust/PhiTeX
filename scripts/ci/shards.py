#!/usr/bin/env python3
"""The gate's shards dealt by measured time (gate.yml's matrix,
gate-shard.sh): each family's items (a manual at one -j, a research
document, an edited document, a latex test) weigh their wall on the gate's
runners, ci/shard-times.tsv, and go heaviest first to the shard that ends
soonest with them; the same list, times and N give the same shards.

    shards.py collect DIR...                  the times from the gate's result
                                              artifacts (r-<shard>/, downloaded
                                              and unzipped under DIR): the
                                              median of each item, as
                                              ci/shard-times.tsv
    shards.py pick latex K N < NAMES          shard K of N of the latex tests
    shards.py plan FAMILY N [--list L] [--jobs 1,4]   every shard's items and
                                              its estimated wall
    shards.py check [--workflow GATE_YML]     every family of gate.yml's matrix:
                                              its shards K = 0..N-1 each listed
                                              once, and every item in exactly one

A shard's estimated wall is, per phase (a manual's -j, a family's one
phase), the longest item or the phase's sum over its lanes, whichever is
more: the sweep runs a manual's -j1 builds four at once (GATE_MANUALS_PAR's
1:4) and its -j4 builds one at a time; the research sweep its oracle four
at once, then -j1 four at once and -j4 alone; the latex tests and the edits
four at once. A manual's -j1 and -j4 builds are separate items (the oracle
is pinned: nothing is shared between them), so one long manual's two
builds go to two shards. An item without a measured time weighs its
family's fallback: a manual the oracle's wall (pdflatex on the runner,
manuals.json) capped at its time limit, anything else the median of its
family's measured items.
"""

import argparse
import json
import re
import statistics
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
TIMES = ROOT / "ci" / "shard-times.tsv"
WORKFLOW = ROOT / ".github" / "workflows" / "gate.yml"
LISTS = {"manuals": ROOT / "ci" / "manuals.json", "research": ROOT / "ci" / "research.json",
         "edits": ROOT / "ci" / "research.json"}
# (the families whose -j builds are separate items: a pinned oracle)
SPLIT_J = {"manuals"}
# (the lanes of each phase, as the gate's shards run them: sweep.py's
# --par of GATE_MANUALS_PAR and GATE_RESEARCH_SWEEP_PAR, GATE_RESEARCH_PAR,
# GATE_EDITS_PAR, GATE_TEST_PAR; the model only, the shards' runs keep
# their own settings)
LANES = {"j1": 4, "oracle": 4, "edits": 4, "latex": 4}
# (the -j a corpus document not split by -j weighs its builds at: the
# gate's GATE_RESEARCH_JOBS)
MODEL_JOBS = (1, 4)
# (the sweep's time limit: a build past it is stopped there)
LIMIT = 300.0


def load_times(path=TIMES):
    """{family: {item: seconds}} of ci/shard-times.tsv (family, item,
    seconds, samples; `#` lines left out)."""
    out = {}
    try:
        lines = open(path).read().splitlines()
    except OSError:
        return out
    for l in lines:
        if l.strip() and not l.startswith("#"):
            fam, item, s = l.split("\t")[:3]
            out.setdefault(fam, {})[item] = float(s)
    return out


def load_list(path):
    data = json.loads(Path(path).read_text())
    return data.get("manuals") or data.get("documents") or data.get("candidates")


def family_of(path):
    """A list's family: its file's stem (ci/manuals.json manuals,
    ci/research.json research); another list (repro.json) weighs its
    fallback alone."""
    return Path(path).stem


def units(family, items, jobs, times):
    """The family's items as (key, item, j or None, [(phase, seconds)...]).
    A manual at each -j its own item; a corpus document (research, or
    another list in its form) its oracle and its builds at every -j
    together (one oracle for all of them); an edited document or a test
    (a name, not a document) one item."""
    t = times.get(family, {})
    js = [int(x) for x in str(jobs).split(",")] if jobs else list(MODEL_JOBS)
    med = statistics.median(t.values()) if t else 1.0

    def get(k):
        w = t.get(k)
        return med if w is None else w

    out = []
    for c in items:
        if not isinstance(c, dict):
            out.append((c, c, None, [(family, get(c))]))
        elif family in SPLIT_J:
            for j in js:
                w = t.get(f"{c['id']} -j{j}")
                if w is None:
                    # (the oracle's wall, pdflatex's: PhiTeX's is near it)
                    lim = max(LIMIT, float(c.get("time_limit_s") or 0))
                    w = min(float(c.get("oracle_wall_s") or 1.0), lim)
                out.append((f"{c['id']} -j{j}", c, j, [(f"j{j}", w)]))
        elif family == "edits":
            out.append((c["id"], c, None, [("edits", get(c["id"]))]))
        else:
            # (at the gate's -j, whichever -j this run builds at: the
            # oracle's slices (manuals.py, no -j) and the sweep's shard
            # are the same documents)
            ph = [(f"j{j}", get(f"{c['id']} -j{j}")) for j in MODEL_JOBS] + [("oracle", get(f"{c['id']} oracle"))]
            out.append((c["id"], c, None, ph))
    return out


def estimate(phases):
    """A shard's wall from its phases' (sum, longest): each phase the
    longest item or its sum over the phase's lanes."""
    return sum(max(m, s / LANES.get(p, 1)) for p, (s, m) in phases.items())


def deal(us, n):
    """The units dealt to n shards, heaviest (the sum of its phases)
    first, each to the shard that ends soonest with it, ties to the one
    that ends soonest before, then the lowest. (Not into a long -j1
    build's free lanes: the other heavy builds put there beside it ran
    1.6 to 2 times their measured walls, lie-hasse's 1556 s for 991, run
    43117787891; a wall is measured beside the usual mix of light ones.)
    Returns the shards' lists of units and their estimated walls."""
    order = sorted(range(len(us)), key=lambda i: (-sum(w for _, w in us[i][3]), us[i][0]))
    state = [dict() for _ in range(n)]
    est = [0.0] * n
    out = [[] for _ in range(n)]
    for i in order:
        best = None
        for s in range(n):
            ph = dict(state[s])
            for p, w in us[i][3]:
                a, m = ph.get(p, (0.0, 0.0))
                ph[p] = (a + w, max(m, w))
            e = estimate(ph)
            key = (e, est[s], s)
            if best is None or key < best[0]:
                best = (key, s, ph, e)
        _, s, ph, e = best
        state[s], est[s] = ph, e
        out[s].append(us[i])
    return out, est


def spec(s):
    k, n = (int(x) for x in s.split("/"))
    if not 0 <= k < n:
        sys.exit(f"shard {s}: K is not in 0..N-1")
    return k, n


def take(family, items, shard, jobs, times=None):
    """Shard K/N's units, (key, item, j or None), heaviest first."""
    times = load_times() if times is None else times
    k, n = spec(shard)
    return [u[:3] for u in deal(units(family, items, jobs, times), n)[0][k]]


def mine(family, items, shard, jobs, times=None):
    """Shard K/N of a corpus family's documents: [(doc, [j...])] in the
    list's order, the -j each is built at here (every -j of JOBS but for a
    family of SPLIT_J; no JOBS, the gate's). No shard: everything."""
    js = [int(x) for x in str(jobs).split(",")] if jobs else list(MODEL_JOBS)
    if not shard:
        return [(c, js) for c in items]
    by = {}
    for _, c, j in take(family, items, shard, jobs, times):
        by.setdefault(c["id"], set()).update([j] if j else js)
    return [(c, sorted(by[c["id"]])) for c in items if c["id"] in by]


def docs(family, items, shard, times=None):
    """Shard K/N as documents alone, at the gate's -j: those with a build
    in it (a manual's oracle wherever one of its builds is)."""
    return [c for c, _ in mine(family, items, shard, None, times)]


def sub(family, items, spec_, times=None):
    """The I-th of P slices of documents by their oracles' walls (the
    research shard's P oracle processes)."""
    times = load_times() if times is None else times
    k, n = spec(spec_)
    t = times.get(family, {})
    med = statistics.median(t.values()) if t else 1.0
    us = [(c["id"], c, None, [("oracle", t.get(f"{c['id']} oracle", med))]) for c in items]
    mine_ = {u[0] for u in deal(us, n)[0][k]}
    return [c for c in items if c["id"] in mine_]


def order(family, items, j, times=None):
    """The documents heaviest first at -j J (the sweep starts them so)."""
    times = load_times() if times is None else times
    w = {u[1]["id"]: sum(x for _, x in u[3]) for u in units(family, items, str(j), times)}
    return sorted(items, key=lambda c: (-w[c["id"]], c["id"]))


def cmd_pick(a):
    names = [l.strip() for l in sys.stdin if l.strip()]
    for key, _, _ in take(a.family, names, f"{a.k}/{a.n}", None):
        print(key)


def family_items(family, names=None):
    if family == "latex":
        return names if names is not None else sorted(load_times().get("latex", {}))
    return load_list(LISTS[family])


# (the -j list each family's sweep runs: GATE_MANUALS_JOBS and
# GATE_RESEARCH_JOBS's default; none for the edits and the tests)
FAMILY_JOBS = {"manuals": "1,4", "research": "1,4", "edits": None, "latex": None}


def cmd_plan(a):
    jobs = a.jobs if a.jobs is not None else FAMILY_JOBS.get(a.family)
    shards, est = deal(units(a.family, family_items(a.family), jobs, load_times()), a.n)
    for k, (s, e) in enumerate(zip(shards, est)):
        print(f"{a.family} {k}/{a.n}: {len(s)} items, estimated {e:.0f} s; heaviest {s[0][0] if s else '-'}")
    print(f"{a.family}: the longest shard {max(est):.0f} s, the sum {sum(est):.0f} s")


def matrix(path):
    """gate.yml's shards of each family: {family: [(K, N)...]}."""
    out = {}
    for m in re.finditer(r'args:\s*"(manuals|research|edits|latex) (\d+) (\d+)', Path(path).read_text()):
        out.setdefault(m.group(1), []).append((int(m.group(2)), int(m.group(3))))
    return out


def check_family(family, ks, names=None):
    """Red lines: the family's shards one N, each K = 0..N-1 named once,
    and every item of the family (a manual at each -j) in exactly one."""
    ns = {n for _, n in ks}
    if len(ns) != 1:
        return [f"{family}: the shards name more than one N: {sorted(ns)}"]
    n = ns.pop()
    reds = []
    if sorted(k for k, _ in ks) != list(range(n)):
        reds.append(f"{family}: the shards' K are {sorted(k for k, _ in ks)}, not 0..{n - 1} once each")
    items, jobs, times = family_items(family, names), FAMILY_JOBS[family], load_times()
    seen = {}
    for k, us in enumerate(deal(units(family, items, jobs, times), n)[0]):
        for _, c, j, _ in us:
            name = c["id"] if isinstance(c, dict) else c
            for jj in ([j] if j or not jobs else [int(x) for x in jobs.split(",")]):
                seen.setdefault((name, jj), []).append(k)
    want = set()
    for c in items:
        name = c["id"] if isinstance(c, dict) else c
        for j in ([int(x) for x in jobs.split(",")] if jobs else [None]):
            want.add((name, j))
            got = seen.get((name, j), [])
            if len(got) != 1:
                reds.append(f"{family}: {name}{'' if j is None else ' -j%d' % j} is in {len(got)} shards {got}, not one")
    reds += [f"{family}: {x} -j{j} is in a shard but not in the list" for x, j in sorted(set(seen) - want, key=str)]
    if family != "latex":
        # (each user of a shard its same documents: the sweep and the
        # verdict (with the -j) and the oracle (without))
        for k in range(n):
            a = [c["id"] for c, _ in mine(family, items, f"{k}/{n}", jobs, times)]
            b = [c["id"] for c in docs(family, items, f"{k}/{n}", times)]
            if a != b:
                reds.append(f"{family}: shard {k}/{n} is not the same documents with and without the -j: {sorted(set(a) ^ set(b))[:5]}")
    return reds


def cmd_check(a):
    names = [l.strip() for l in open(a.latex_names) if l.strip()] if a.latex_names else None
    reds = []
    m = matrix(a.workflow)
    for family in ([a.family] if a.family else sorted(FAMILY_JOBS)):
        if family not in m:
            reds.append(f"{family}: no shard of it in {a.workflow}")
            continue
        reds += check_family(family, m[family], names if family == "latex" else None)
    for r in reds:
        print("shards: " + r)
    sys.exit(1 if reds else 0)


def collect(dirs):
    """{family: {item: [seconds...]}} from the gate's result artifacts:
    r-manuals-*/manuals/<id>-j<J>.json (wall_s; a timeout weighs its
    limit), r-research-*/research/<id>-j<J>.json and
    r-research-*/oracle/oracle-*.txt (`id: ok ... 12.3s`, the oracle's
    wall), r-edits-*/edits/<id>.json (wall_s), r-latex-*/test-walls.txt."""
    got = {}

    def add(f, k, s):
        got.setdefault(f, {}).setdefault(k, []).append(float(s))

    for d in dirs:
        for p in Path(d).glob("**/r-manuals-*/manuals/*-j*.json"):
            r = json.loads(p.read_text())
            if r.get("wall_s") is not None and r.get("status") != "harness":
                add("manuals", f"{r['id']} -j{r['j']}", r["wall_s"])
        for p in Path(d).glob("**/r-research-*/research/*-j*.json"):
            r = json.loads(p.read_text())
            if r.get("wall_s") is not None and r.get("status") not in ("harness", "no-oracle"):
                add("research", f"{r['id']} -j{r['j']}", r["wall_s"])
        for p in Path(d).glob("**/r-research-*/oracle/oracle-*.txt"):
            for l in p.read_text(errors="replace").splitlines():
                m = re.match(r"^(\S+): \S+ .* ([\d.]+)s$", l)
                if m:
                    add("research", f"{m.group(1)} oracle", m.group(2))
        for p in Path(d).glob("**/r-edits-*/edits/*.json"):
            r = json.loads(p.read_text())
            if r.get("wall_s") is not None:
                add("edits", r["id"], r["wall_s"])
        for p in Path(d).glob("**/r-latex-*/test-walls.txt"):
            for l in p.read_text().splitlines():
                x = l.split()
                if len(x) >= 2:
                    add("latex", x[0], x[1])
    return got


def cmd_collect(a):
    got = collect(a.dirs)
    print("# The gate's items' walls on GitHub's runners (scripts/ci/shards.py: the shards are dealt by them):"
          " family, item (a manual or research document at -jJ, a research document's oracle, an edited"
          " document, a latex test), the median wall in seconds, the runs measured. Made by"
          " `shards.py collect` from the gate's result artifacts; " + a.note)
    for f in sorted(got):
        for k in sorted(got[f]):
            v = got[f][k]
            print(f"{f}\t{k}\t{statistics.median(v):.3f}\t{len(v)}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("collect")
    p.add_argument("dirs", nargs="+")
    p.add_argument("--note", default="")
    p = sub.add_parser("pick")
    p.add_argument("family")
    p.add_argument("k", type=int)
    p.add_argument("n", type=int)
    p = sub.add_parser("plan")
    p.add_argument("family")
    p.add_argument("n", type=int)
    p.add_argument("--jobs")
    p = sub.add_parser("check")
    p.add_argument("--workflow", default=str(WORKFLOW))
    p.add_argument("--family")
    p.add_argument("--latex-names", help="the latex tests the binary lists (else the measured ones)")
    a = ap.parse_args()
    {"collect": cmd_collect, "pick": cmd_pick, "plan": cmd_plan, "check": cmd_check}[a.cmd](a)


if __name__ == "__main__":
    main()
