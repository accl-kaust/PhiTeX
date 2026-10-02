#!/usr/bin/env python3
"""How much of an SSA build could run at once: the step dependency graph
that `PARTEX_SSA_DAG=FILE` writes (`partex_core::ssa::dag`), measured.

    scripts/sandbox python3 scripts/ssa-parallel.py cold DAG [--view VIEW] [--log LOG ...]
    scripts/sandbox python3 scripts/ssa-parallel.py rebuild DAG.K DAG.N [--view VIEW.N] [--log LOG ...]

A step (a window of the fold) costs the commands its last run ran. It
depends on step A if it read an address whose definition reaching it is
A's (the dump's `R A at name` lines; a file it loaded, `L`, is a φ of
the trip before and makes no edge). Step boundaries (where each step's
input begins) are taken as known, as a rebuild knows them from the last
build: found by running, they would chain every step to the one before
it. Two schedules, each with as many workers as steps:

- *whole steps*: a step begins once every step it read from has ended.
  Program order is a topological order, so one pass gives each step's
  earliest end; the longest is the critical path, and total/critical
  the speedup any schedule that respects the edges can reach;
- *pipelined*: a step may begin before the steps it reads from end. It
  runs its commands in order, each read from outside it waiting for its
  definition, which is there once the defining step last wrote the
  address (the dump's times, in commands into each step).

Four models of the reads (artificial chains, TODO item 2):
- *all reads*, as recorded;
- *soft definitions*: a definition equal to the one before it (a local
  assignment saved and restored inside the step, `\\@savsf` around an
  output routine) is passed through to that one;
- *soft reads*: in addition, a step's read of an address it so passes
  through is dropped (the incoming value saved and restored unobserved);
- *blind writes*: a step's read of any address it defines is dropped, an
  upper bound for DESIGN 3.2's scoped definitions (an assignment in a
  group reads no old value; a step that really read the value first is
  counted too).

`cold` measures one build: the bounds, what the critical path is made of
(the addresses its edges carry, by class), how the bound grows when a
class's edges are ignored (greedily, the class whose removal helps most
first), the bound with only some classes' edges kept, a parallelism
profile, and list schedules on P workers.

`rebuild` compares the graph after a rebuild (DAG.N) with the one before
it (DAG.K): the steps whose run changed are the ones re-run. It gives
their dependency-respecting bound, and speculation's: every re-run step
run at once from its recorded entry state (the definitions reaching it in
DAG.K), then validated in order; a step whose every read finds the same
version at its place in both graphs validates, any other runs again
behind the steps it reads from. The same for a cold build with the last
build's records: every step speculated, the ones not re-run validating.

`--view` names steps by the view's comments (`PARTEX_SSA_VIEW`); `--log`
reads register names from LaTeX's allocation lines (`\\c@page=\\count0`)
in the format's and the job's logs.
"""
import argparse
import bisect
import collections
import heapq
import re
import sys

CLASSES = [
    "page builder", "current list (nest)", "save stack", "allocators",
    "log and terminal", "PDF writer and fonts", "\\write streams", "\\read streams",
    "call results", "conditionals", "marks", "counters (\\count)",
    "registers (\\dimen, \\skip, \\toks, \\box)", "hash table", "codes",
    "meanings (macros)", "hyphenation", "source", "unknown", "engine scalars",
    "parameters",
]
BIT = {c: 1 << i for i, c in enumerate(CLASSES)}
EVERY = (1 << len(CLASSES)) - 1
MODELS = ("all reads", "soft definitions", "soft reads", "blind writes")


def klass(n):
    """An address's class (an index in CLASSES), by its name in the view."""
    if n.startswith(("page.", "page[")) or n in ("page_step_result", "splitdiscards"):
        return 0
    if n.startswith(("list.", "align.")) or n == "nest":
        return 1
    if n.startswith(("save.", "save[")):
        return 2
    if n in ("str_ptr", "hash_used", "hash_high", "glue_lineage") or n.startswith(
        ("string:", "strings:")
    ):
        return 3
    if n in (
        "file_offset", "term_offset", "selector", "error_count", "history", "write:log",
        "log_opened", "interaction", "long_help_seen", "open_parens", "log_name",
        "job_name", "output_file_name", "shown_mode",
    ):
        return 4
    if n.startswith(("pdf.", "glyphs:", "dvi.", "font:", "fontid:")) or n in (
        "fonts", "pdfinfo", "pdfcatalog", "pdfnames", "pdftrailer", "pdftrailerid",
    ):
        return 5
    if n.startswith(("write:", "write_open[")):
        return 6
    if n.startswith(("read:", "read_open[")):
        return 7
    if n in ("hpack_result", "vpack_result", "line_break_result", "output_result",
             "last_badness"):
        return 8
    if n == "cond":
        return 9
    if re.fullmatch(r"(top|first|bot|splitfirst|splitbot)marks?\d*", n):
        return 10
    m = re.fullmatch(r"\\(count|dimen|skip|muskip|toks|box)(\d+)", n)
    if m:
        return 11 if m.group(1) == "count" else 12
    if n.startswith(("text:", "next:", "lookup:", "hash:", "search:")):
        return 13
    if re.fullmatch(r"\\(catcode|lccode|uccode|sfcode|mathcode|delcode)\d+", n):
        return 14
    if n.startswith(("\\", "active:", "frozen:", "primitive:", "undefined")):
        return 15
    if n.startswith("hyph"):
        return 16
    if n.startswith(("source:", "line:")):
        return 17
    if n.startswith("unknown:"):
        return 18
    if n in ("output_active", "dead_cycles", "after_token", "align_state", "mag_set",
             "random", "sys_time", "sys_day", "sys_month", "sys_year"):
        return 19
    return 20


class Dag:
    """A dump, by position in program order: each step's id, run, cost,
    reads (from position or -1, name index) and definitions {name index:
    version}; timed, how many commands into the step each read was made
    (`rat`, beside `reads`; -1 untimed) and each definition last written
    (`wat`)."""

    def __init__(self, path):
        self.names, self.ix = [], {}
        self.ids, self.run, self.cost = [], [], []
        self.reads, self.writes, self.rat, self.wat = [], [], [], []
        reads = writes = rat = wat = None
        id_pos = {}
        with open(path, errors="replace") as f:
            for line in f:
                kind, a, rest = line.rstrip("\n").split("\t", 2)
                if kind == "S":
                    run, cost = rest.split("\t")
                    id_pos[int(a)] = len(self.ids)
                    self.ids.append(int(a))
                    self.run.append(int(run))
                    self.cost.append(int(cost))
                    reads, writes, rat, wat = [], {}, [], {}
                    self.reads.append(reads)
                    self.writes.append(writes)
                    self.rat.append(rat)
                    self.wat.append(wat)
                    continue
                if kind == "L":
                    continue
                t, b = rest.split("\t", 1)
                n = self.ix.get(b)
                if n is None:
                    n = self.ix[b] = len(self.names)
                    self.names.append(b)
                if kind == "R":
                    reads.append((-1 if a == "-" else int(a), n))
                    rat.append(-1 if t == "-" else int(t))
                else:
                    writes[n] = None if a == "-" else a
                    if t != "-":
                        wat[n] = int(t)
        self.timed = any(t >= 0 for ts in self.rat for t in ts[:1])
        self.pos = id_pos
        # (reads by position: a reaching definition is a live step's)
        for rs in self.reads:
            for k, (a, n) in enumerate(rs):
                if a >= 0:
                    rs[k] = (id_pos[a], n)
        self.cls = [klass(n) for n in self.names]
        # (each address's definitions in program order, by position)
        self.defs = collections.defaultdict(list)
        for p, w in enumerate(self.writes):
            for n in w:
                self.defs[n].append(p)
        self.n = len(self.ids)
        self._soft = None

    def total(self):
        return sum(self.cost)

    def version_at(self, n, pos):
        """The version of the definition of name `n` reaching position
        `pos` (the last before it), or None."""
        d = self.defs.get(n)
        if not d:
            return None
        i = bisect.bisect_left(d, pos)
        return self.writes[d[i - 1]][n] if i else None

    def soft(self):
        """For each (position, name) it defines: the position whose
        definition is the effective one, a definition equal to the one
        before it passed through to that one; and the set of (position,
        name) that pass through."""
        if self._soft is None:
            eff, through = {}, set()
            for n, ps in self.defs.items():
                last_v, last_eff = None, -1
                for p in ps:
                    v = self.writes[p][n]
                    if v is not None and v == last_v:
                        eff[(p, n)] = last_eff
                        through.add((p, n))
                    else:
                        last_eff = p
                    last_v = v
            self._soft = (eff, through)
        return self._soft

    def model(self, name):
        """A model's (effective definitions, the (position, name) reads
        dropped): MODELS."""
        if name == "all reads":
            return None, None
        eff, through = self.soft()
        if name == "soft definitions":
            return eff, None
        if name == "soft reads":
            return eff, through
        return eff, {(p, n) for p, w in enumerate(self.writes) for n in w}

    def kept(self, p, model):
        """Position `p`'s reads in a model: (from position, name, the
        read's index), each read's definition followed through `eff`."""
        eff, drop = model
        for k, (a, n) in enumerate(self.reads[p]):
            if a < 0 or (drop is not None and (p, n) in drop):
                continue
            if eff is not None:
                a = eff.get((a, n), a)
                if a < 0:
                    continue
            yield a, n, k

    def edges(self, model):
        """Per position, its dependencies {from position: class mask}."""
        cls = self.cls
        out = []
        for p in range(self.n):
            e = {}
            for a, n, _ in self.kept(p, model):
                e[a] = e.get(a, 0) | (1 << cls[n])
            out.append(e)
        return out

    def carried(self, p, a, model):
        """The names by which position `p` depends on position `a`."""
        return [n for b, n, _ in self.kept(p, model) if b == a]

    def lags(self, model):
        """Per position, its reads for the pipelined schedule: (from
        position, the definition's time in its step less the read's time
        in this one, class bit, name)."""
        cls, wat, cost = self.cls, self.wat, self.cost
        out = []
        for p in range(self.n):
            rat = self.rat[p]
            out.append([(a, wat[a].get(n, cost[a]) - max(rat[k], 0), 1 << cls[n], n)
                        for a, n, k in self.kept(p, model)])
        return out


def longest(cost, edges, skip=0, among=None):
    """Earliest ends of whole steps with unbounded workers, in program
    order, edges whose classes are all in mask `skip` ignored; `among`:
    only those positions run (the others' definitions are there at the
    start). Returns (end by position, the predecessor that set it)."""
    n = len(cost)
    fin = [0] * n
    pred = [-1] * n
    keep = ~skip
    inside = None if among is None else set(among)
    for p in range(n) if among is None else among:
        best, who = 0, -1
        for a, m in edges[p].items():
            if m & keep and (inside is None or a in inside):
                f = fin[a]
                if f > best:
                    best, who = f, a
        fin[p] = best + cost[p]
        pred[p] = who
    return fin, pred


def pipelined(cost, lags, skip=0, among=None, ok=None):
    """Earliest ends of pipelined steps: each begins as late as its
    latest-bound read needs (so it never waits once begun, and ends as it
    would if it began at once and waited at each read). Classes in `skip`
    are ignored; `among` as in `longest`; with `ok`, a step whose
    speculation validates (`ok[p]`) begins at once. Returns (end by
    position, the (from position, name) of the read that set each
    start)."""
    n = len(cost)
    start = [0] * n
    fin = [0] * n
    why = [None] * n
    inside = None if among is None else set(among)
    for p in range(n) if among is None else among:
        s, w = 0, None
        if ok is None or not ok[p]:
            for a, lag, bit, name in lags[p]:
                if bit & skip or (inside is not None and a not in inside):
                    continue
                need = start[a] + lag
                if need > s:
                    s, w = need, (a, name)
        start[p] = s
        fin[p] = s + cost[p]
        why[p] = w
    return fin, why


def path_of(fin, pred, among=None):
    end = max(range(len(fin)) if among is None else among, key=lambda p: fin[p])
    path = [end]
    while pred[path[-1]] >= 0:
        path.append(pred[path[-1]])
    return path[::-1]


def binding_chain(fin, why):
    """The reads that set the starts, back from the last step to end: the
    chain's positions and each binding read's (reader, name)."""
    p = max(range(len(fin)), key=lambda q: fin[q])
    chain, reads = [p], []
    while why[p] is not None:
        a, n = why[p]
        reads.append((p, n))
        chain.append(a)
        p = a
    return chain[::-1], reads[::-1]


def schedule(cost, edges, workers, skip=0):
    """Greedy list scheduling of whole steps on `workers` workers, in
    program order, each on the worker free first, after its
    dependencies: the makespan."""
    free = [0] * workers
    fin = [0] * len(cost)
    keep = ~skip
    for p in range(len(cost)):
        ready = max((fin[a] for a, m in edges[p].items() if m & keep), default=0)
        w = heapq.heappop(free)
        fin[p] = max(w, ready) + cost[p]
        heapq.heappush(free, fin[p])
    return max(fin, default=0)


def profile(cost, fin, buckets=10):
    """Concurrency: the most steps running at once, and the mean number
    running in each of `buckets` slices of the critical path."""
    cp = max(fin)
    ev = []
    for p, c in enumerate(cost):
        if c > 0:
            ev.append((fin[p] - c, 1))
            ev.append((fin[p], -1))
    ev.sort()
    run = peak = 0
    for _, e in ev:
        run += e
        peak = max(peak, run)
    width = cp / buckets
    busy = [0.0] * buckets
    for p, c in enumerate(cost):
        a, b = fin[p] - c, fin[p]
        k = int(a // width)
        while a < b and k < buckets:
            hi = min(b, (k + 1) * width)
            busy[k] += hi - a
            a = hi
            k += 1
    return peak, [x / width for x in busy]


def widest_level(edges):
    """The steps at each depth (unweighted) form an antichain: the widest,
    and the number of depths."""
    lv = [0] * len(edges)
    for p, e in enumerate(edges):
        lv[p] = 1 + max((lv[a] for a in e), default=0)
    c = collections.Counter(lv)
    return max(c.values()), max(lv)


def register_names(logs):
    """`\\countNN` -> `\\c@page` etc., from LaTeX's allocation lines."""
    out = {}
    for path in logs or []:
        with open(path, errors="replace") as f:
            for line in f:
                m = re.match(r"^(\\\S+)=\\(count|dimen|skip|muskip|toks|box)(\d+)\s*$", line)
                if m:
                    out.setdefault(f"\\{m.group(2)}{m.group(3)}", m.group(1))
    return out


def comments(path):
    """Step id -> the view's comment (`; step S: ...`)."""
    out = {}
    if path:
        with open(path, errors="replace") as f:
            for line in f:
                m = re.search(r"; step (\d+)(.*)$", line)
                if m:
                    out[int(m.group(1))] = m.group(2).strip(" :")[:100]
    return out


def fmt(x):
    return f"{x:,}"


def ratio(a, b):
    return f"{a / b:.2f}" if b else "-"


def mask_names(m):
    return [CLASSES[k] for k in range(len(CLASSES)) if m >> k & 1]


def shown(n, regs):
    r = regs.get(n)
    return f"{n}={r}" if r else n


def steps_shown(d, ps, shows, k=4):
    return "; ".join(f"step {d.ids[p]} {fmt(d.cost[p])} [{shows.get(d.ids[p], '')[:48]}]"
                     for p in sorted(ps, key=lambda p: -d.cost[p])[:k])


def composition(d, edges, fin, pred, model, regs, shows, top=12):
    """What a whole-step critical path is made of: its steps, and the
    classes and names its edges carry, by edges and by the commands of
    the steps the edges lead to."""
    path = path_of(fin, pred)
    by_class, alone, weight = collections.Counter(), collections.Counter(), collections.Counter()
    by_name, name_weight = collections.Counter(), collections.Counter()
    for a, b in zip(path, path[1:]):
        cs = mask_names(edges[b][a])
        for c in cs:
            by_class[c] += 1
            weight[c] += d.cost[b]
        if len(cs) == 1:
            alone[cs[0]] += 1
        for n in set(d.carried(b, a, model)):
            by_name[d.names[n]] += 1
            name_weight[d.names[n]] += d.cost[b]
    print(f"  the path: {fmt(len(path))} steps, {fmt(sum(d.cost[p] for p in path))} commands; "
          f"its edges by class (edges, carried by the class alone, commands of the steps "
          f"they lead to): " + "; ".join(
              f"{c} {fmt(k)} ({fmt(alone[c])}, {fmt(weight[c])})"
              for c, k in by_class.most_common(10)))
    print("  the names on most of its edges: " + ", ".join(
        f"{shown(n, regs)} {fmt(k)}" for n, k in by_name.most_common(top)))
    print("  the names before the most commands: " + ", ".join(
        f"{shown(n, regs)} {fmt(k)}" for n, k in name_weight.most_common(top)))
    print(f"  its costliest steps: {steps_shown(d, path, shows)}")


def chain_composition(d, fin, why, regs, shows, top=12):
    """What a pipelined critical path is made of: the binding reads, by
    the commands of the steps they hold back."""
    chain, reads = binding_chain(fin, why)
    by_class, by_name = collections.Counter(), collections.Counter()
    for p, n in reads:
        by_class[CLASSES[d.cls[n]]] += d.cost[p]
        by_name[d.names[n]] += d.cost[p]
    print(f"  the chain: {fmt(len(chain))} steps, {fmt(sum(d.cost[p] for p in chain))} commands; "
          f"its binding reads by class (commands of the steps they hold back): " + ", ".join(
              f"{c} {fmt(k)}" for c, k in by_class.most_common(8)))
    print("  by name: " + ", ".join(
        f"{shown(n, regs)} {fmt(k)}" for n, k in by_name.most_common(top)))
    print(f"  its costliest steps: {steps_shown(d, chain, shows)}")


def peel(total, big, label, compute, rounds=8):
    """Classes ignored one after another, each time the one (or pair) on
    the current critical path whose removal shortens it most. `compute`
    gives, for a mask of classes ignored, the critical path and the mask
    of the classes on it."""
    print(f"  classes ignored one after another ({label}):")
    skip = 0
    cur, on = compute(0)
    for _ in range(rounds):
        cands = [1 << k for k in range(len(CLASSES)) if (on & ~skip) >> k & 1]
        best = None
        for c in cands:
            v, o = compute(skip | c)
            if best is None or v < best[0]:
                best = (v, c, o)
        if best is not None and best[0] >= cur:
            # (no one class shortens it: an edge carried by two)
            for i, c1 in enumerate(cands):
                for c2 in cands[i + 1:]:
                    v, o = compute(skip | c1 | c2)
                    if v < best[0]:
                        best = (v, c1 | c2, o)
        if best is None or best[0] >= cur:
            print("    (no class on the path shortens it)")
            break
        cur, skip, on = best[0], skip | best[1], best[2]
        print(f"    + {' + '.join(mask_names(best[1]))}: critical path {fmt(cur)}, "
              f"bound {ratio(total, cur)}x")
        if cur <= big:
            break


def keep_only(total, compute):
    """The critical path with only some classes' edges kept."""
    page, nest = BIT["page builder"], BIT["current list (nest)"]
    count = BIT["counters (\\count)"]
    cursors = (BIT["allocators"] | BIT["log and terminal"] | BIT["engine scalars"]
               | BIT["conditionals"] | BIT["save stack"] | BIT["hash table"])
    for label, keep in (
        ("the page builder's alone", page),
        ("the page builder's and the nest's", page | nest),
        ("the page builder's, the nest's and the counters'", page | nest | count),
        ("all but the cursors' (allocators, log, engine scalars, conditionals, save stack, "
         "hash)", EVERY & ~cursors),
    ):
        v, _ = compute(EVERY & ~keep)
        print(f"    edges kept: {label}: critical path {fmt(v)}, bound {ratio(total, v)}x")


def whole_compute(d, edges):
    def compute(skip):
        fin, pred = longest(d.cost, edges, skip)
        path = path_of(fin, pred)
        on = 0
        for a, b in zip(path, path[1:]):
            on |= edges[b][a]
        return max(fin), on

    return compute


def piped_compute(d, lags):
    def compute(skip):
        fin, why = pipelined(d.cost, lags, skip)
        _, reads = binding_chain(fin, why)
        on = 0
        for _, n in reads:
            on |= 1 << d.cls[n]
        return max(fin), on

    return compute


def cold(args):
    d = Dag(args.dag)
    regs = register_names(args.log)
    shows = comments(args.view)
    total, big, n = d.total(), max(d.cost), d.n
    costs = sorted(d.cost, reverse=True)
    first = next((p for p in range(n) if "ships [" in shows.get(d.ids[p], "")), None)
    print(f"steps {fmt(n)}, commands {fmt(total)} (each step's last run); the costliest step "
          f"{fmt(big)} commands (alone, a bound of {ratio(total, big)}x), the mean "
          f"{total / n:,.0f}, the median {fmt(costs[n // 2])}; reads from outside a step "
          f"{fmt(sum(map(len, d.reads)))}, definitions {fmt(sum(map(len, d.writes)))}")
    body = None
    if first is not None:
        setup = sum(d.cost[:first])
        body = list(range(first, n))
        print(f"the setup (before the first step that ships, step {d.ids[first]}): {fmt(first)} "
              f"steps, {fmt(setup)} commands ({100 * setup / total:.1f}%); the body "
              f"{fmt(n - first)} steps, {fmt(total - setup)} commands")
    print(f"the costliest steps: {steps_shown(d, range(n), shows, 6)}")
    readers = collections.Counter(nm for rs in d.reads for a, nm in rs if a >= 0)
    rows = sorted(((len(ps), readers[nm], d.names[nm]) for nm, ps in d.defs.items()),
                  reverse=True)
    print("the addresses defined by the most steps (definitions/reads of another step's): "
          + ", ".join(f"{shown(nm, regs)} {fmt(k)}/{fmt(r)}" for k, r, nm in rows[:14]))
    _, through = d.soft()
    by_cls = collections.Counter(d.cls[nm] for p, nm in through)
    print(f"definitions equal to the one before them: {fmt(len(through))} of "
          f"{fmt(sum(map(len, d.writes[1:])))} after the start's ("
          + ", ".join(f"{CLASSES[c]} {fmt(k)}" for c, k in by_cls.most_common(6)) + ")")
    models = {m: d.model(m) for m in MODELS}
    print("\n== whole steps (the input chain alone, every step after the one before it, "
          "is 1.00x)")
    edges = {}
    for m in MODELS:
        e = edges[m] = d.edges(models[m])
        fin, _ = longest(d.cost, e)
        line = (f"  {m}: edges {fmt(sum(map(len, e)))}, critical path {fmt(max(fin))}, bound "
                f"{ratio(total, max(fin))}x")
        if body:
            bf, _ = longest(d.cost, e, among=body)
            bt = sum(d.cost[p] for p in body)
            line += f"; the body alone {ratio(bt, max(bf[p] for p in body))}x"
        print(line)
    for m in ("all reads", "blind writes"):
        e = edges[m]
        fin, pred = longest(d.cost, e)
        print(f"\n{m}:")
        composition(d, e, fin, pred, models[m], regs, shows)
        peak, prof = profile(d.cost, fin)
        width, depth = widest_level(e)
        print(f"  profile: at most {fmt(peak)} steps at once; the widest depth {fmt(width)} "
              f"steps (of {fmt(depth)}); mean steps running per tenth of the critical path: "
              + " ".join(f"{x:.1f}" for x in prof))
        print("  list schedules (program order, P workers): " + ", ".join(
            f"P={k} {ratio(total, schedule(d.cost, e, k))}x" for k in (2, 8, 64)))
        peel(total, big, f"whole steps, {m}", whole_compute(d, e))
        print(f"  classes kept (whole steps, {m}):")
        keep_only(total, whole_compute(d, e))
    del edges
    if not d.timed:
        return
    print("\n== pipelined (a step may begin before the steps it reads from end)")
    for m in MODELS:
        lags = d.lags(models[m])
        fin, why = pipelined(d.cost, lags)
        line = f"  {m}: critical path {fmt(max(fin))}, bound {ratio(total, max(fin))}x"
        if body:
            bf, _ = pipelined(d.cost, lags, among=body)
            bt = sum(d.cost[p] for p in body)
            line += f"; the body alone {ratio(bt, max(bf[p] for p in body))}x"
        print(line)
        if m in ("soft reads", "blind writes"):
            chain_composition(d, fin, why, regs, shows)
            peak, prof = profile(d.cost, fin)
            print(f"  profile: at most {fmt(peak)} steps at once; mean steps running per tenth "
                  f"of the critical path: " + " ".join(f"{x:.1f}" for x in prof))
        if m == "blind writes":
            peel(total, big, f"pipelined, {m}", piped_compute(d, lags))
            print(f"  classes kept (pipelined, {m}):")
            keep_only(total, piped_compute(d, lags))


def validated(old, new, p, model):
    """Whether the step at position `p` of `new`, run from its entry state
    in `old`, would have read what it reads in `new`: each address it read
    (in the model) finds the same version at its place in both (its source
    lines are read as they are now). A definition that reached it in
    `old` and reaches it in `new` from no step is not a change: a step run
    again does not define again what the build allocates once (a font
    loaded, a name entered: the engine keeps them, DESIGN 4.3 item 7's
    leftover). Returns the first read that differs (its name), or None."""
    q = old.pos.get(new.ids[p])
    if q is None:
        return "(a new step)"
    _, drop = model
    for a, n in new.reads[p]:
        if a < 0 or (drop is not None and (p, n) in drop):
            continue
        on = old.ix.get(new.names[n])
        if new.writes[a].get(n) != (None if on is None else old.version_at(on, q)):
            return new.names[n]
    return None


def rebuild(args):
    old, new = Dag(args.old), Dag(args.new)
    regs = register_names(args.log)
    shows = comments(args.view)
    R = [p for p in range(new.n)
         if new.ids[p] not in old.pos or old.run[old.pos[new.ids[p]]] != new.run[p]]
    Rs = set(R)
    total, T = sum(new.cost[p] for p in R), new.total()
    gone = len(set(old.pos) - set(new.pos))
    print(f"steps re-run {fmt(len(R))} ({fmt(total)} commands; new "
          f"{sum(new.ids[p] not in old.pos for p in R)}, removed {gone}); the build: "
          f"{fmt(new.n)} steps, {fmt(T)} commands, the costliest step {fmt(max(new.cost))}")
    if not R:
        return
    for m in ("all reads", "soft reads", "blind writes"):
        model = new.model(m)
        e = new.edges(model)
        differs = {p: validated(old, new, p, model) for p in R}
        ok = {p: differs[p] is None for p in R}
        print(f"\n{m}:")
        for p in R[:args.show]:
            deps = [a for a in e[p] if a in Rs]
            why = "; ".join(f"{new.ids[a]}: " + ",".join(
                shown(new.names[nm], regs) for nm in new.carried(p, a, model)[:3])
                for a in deps)
            verdict = "validates" if ok[p] else f"fails ({shown(differs[p], regs)} differs)"
            print(f"  step {new.ids[p]} (run {new.run[p]}): {fmt(new.cost[p])} commands, "
                  f"{verdict}; reads from re-run steps: {why or '-'}"
                  f" [{shows.get(new.ids[p], '')[:56]}]")
        if len(R) > args.show:
            print(f"  ... {len(R) - args.show} more")
        reasons = collections.Counter(differs[p] for p in R if differs[p] is not None)
        if reasons:
            print("  the first read that differs, by name: " + ", ".join(
                f"{shown(nm, regs)} {k}" for nm, k in reasons.most_common(8)))
        fin, pred = longest(new.cost, e, among=R)
        path = path_of(fin, pred, among=R)
        cp = fin[path[-1]]
        k = sum(ok.values())
        spec = {}
        for p in R:
            spec[p] = new.cost[p] if ok[p] else max(
                (spec[a] for a in e[p] if a in Rs), default=0) + new.cost[p]
        sp = max(spec.values())
        shown_path = " -> ".join(str(new.ids[p]) for p in path[:10]) + (
            " -> ..." if len(path) > 10 else "")
        print(f"  whole steps: dependency-respecting, critical path {fmt(cp)} over "
              f"{len(path)} steps ({shown_path}), bound {ratio(total, cp)}x; speculated "
              f"(every re-run step at once from its recorded entry state): {k} of {len(R)} "
              f"validate ({fmt(sum(new.cost[p] for p in R if ok[p]))} of {fmt(total)} "
              f"commands), critical path {fmt(sp)}, bound {ratio(total, sp)}x")
        cfin = [0] * new.n
        for p in range(new.n):
            cfin[p] = new.cost[p] + (max((cfin[a] for a in e[p]), default=0)
                                     if p in Rs and not ok[p] else 0)
        fails = sum(not ok[p] for p in R)
        print(f"  a cold build speculated from the last build's records: "
              f"{fmt(new.n - fails)} of {fmt(new.n)} steps validate "
              f"({100 * (new.n - fails) / new.n:.3f}%); critical path {fmt(max(cfin))}, bound "
              f"{ratio(T, max(cfin))}x")
        if new.timed:
            lags = new.lags(model)
            a, _ = pipelined(new.cost, lags, among=R)
            b, _ = pipelined(new.cost, lags, among=R, ok=ok)
            everyone = {p: p not in Rs or ok[p] for p in range(new.n)}
            c, _ = pipelined(new.cost, lags, ok=everyone)
            a, b, c = max(a[p] for p in R), max(b[p] for p in R), max(c)
            print(f"  pipelined: dependency-respecting {fmt(a)}, bound {ratio(total, a)}x; "
                  f"speculated {fmt(b)}, bound {ratio(total, b)}x; a cold build speculated "
                  f"{fmt(c)}, bound {ratio(T, c)}x")


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    sub = p.add_subparsers(dest="what", required=True)
    c = sub.add_parser("cold")
    c.add_argument("dag")
    c.add_argument("--view")
    c.add_argument("--log", nargs="*")
    r = sub.add_parser("rebuild")
    r.add_argument("old")
    r.add_argument("new")
    r.add_argument("--view")
    r.add_argument("--log", nargs="*")
    r.add_argument("--show", type=int, default=30, help="re-run steps listed")
    args = p.parse_args()
    if args.what == "cold":
        cold(args)
    else:
        rebuild(args)


if __name__ == "__main__":
    sys.exit(main())
