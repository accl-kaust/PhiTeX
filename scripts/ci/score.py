#!/usr/bin/env python3
"""The scoreboard and the performance comparison of a CI run (docs/ci.md).
This is the trusted step: it reads the stages' JSON as data, runs no
document, and writes only under the run and the CI directory.

    score.py --run RUNDIR --ci CIDIR [--main] [--known FILE]

Reads RUNDIR/{bin/commit.txt, manuals/*.json, torture/torture.json,
perf/perf.json}; writes RUNDIR/{scoreboard.json, scoreboard.md, summary.txt}.
With --main (a run of a main commit): CIDIR/main/<commit>.json and
latest.json (the next run's baseline). With perf results: the run's entry
in CIDIR/history.d/<run>.json (one file per run, never rewritten), then
CIDIR/history.jsonl made again from them (ordered by commit date) and
CIDIR/report.html regenerated. --perf-only (the backfill): only that.

Exit status: 0 pass, 1 fail. A run fails when
  - a case fails that ci/known-failing.txt does not list, or
  - a case that passed in the last main run no longer passes, or
  - a perf median is over 5% worse than the last main run's with the two
    ranges apart (a memory peak likewise); within 5%, or overlapping, a
    warning.
"""

import argparse
import datetime
import fnmatch
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
PASS = {"identical", "pass"}
NEUTRAL = {"n/a", "excluded"}
THRESHOLD = 0.05


def load(p, default=None):
    try:
        return json.loads(Path(p).read_text())
    except (OSError, ValueError):
        return default


def md(s):
    """A Markdown table cell: data, never markup."""
    s = str(s if s is not None else "")
    for a, b in (("&", "&amp;"), ("<", "&lt;"), (">", "&gt;"), ("|", "\\|"), ("`", "'"), ("\n", " ")):
        s = s.replace(a, b)
    return s[:240]


def known_failing(path):
    rules = []
    try:
        lines = Path(path).read_text().splitlines()
    except OSError:
        return rules
    for line in lines:
        body = line.split("#", 1)[0].split()
        if len(body) >= 2:
            rules.append((body[0], body[1], line.split("#", 1)[1].strip() if "#" in line else ""))
    return rules


def is_known(rules, suite, cid):
    return any(fnmatch.fnmatchcase(suite, s) and fnmatch.fnmatchcase(cid, i) for s, i, _ in rules)


def cases_of(run):
    cases = []
    for f in sorted((run / "manuals").glob("*.json")):
        m = load(f)
        if not m:
            continue
        suite = m.get("suite", "manuals")
        if m.get("status") in ("excluded", "error") and not m.get("modes"):
            cases.append({"suite": suite + "/oracle", "id": m["id"], "status": m["status"],
                          "detail": (m.get("oracle") or {}).get("reason") or m.get("note", "")})
            continue
        for mode, r in (m.get("modes") or {}).items():
            cases.append({"suite": f"{suite}/{mode}", "id": m["id"], "status": r.get("status"),
                          "detail": r.get("first") or r.get("note") or "",
                          "wall_s": r.get("wall_s"), "rss_kib": r.get("rss_kib")})
    t = load(run / "torture" / "torture.json", {})
    cases.extend(t.get("cases", []))
    return cases


def counts(cases):
    by = {}
    for c in cases:
        s = by.setdefault(c["suite"], {})
        s[c["status"]] = s.get(c["status"], 0) + 1
    return by


def perf_compare(cur, base):
    """[(level, doc, metric, text)]: `fail` or `warn` lines."""
    out = []
    if not cur or not base:
        return out
    if base.get("node") != cur.get("node"):
        out.append(("warn", "-", "-", f"baseline from {base.get('node')}, this run on {cur.get('node')}: "
                    "compared, never failed"))
    same_node = base.get("node") == cur.get("node")
    for doc, metrics in cur.get("docs", {}).items():
        bm = base.get("docs", {}).get(doc, {})
        for k, v in metrics.items():
            b = bm.get(k)
            if not (isinstance(v, dict) and isinstance(b, dict) and "median" in v and "median" in b):
                continue
            if not k.endswith(("_s", "_ms", "_kib")) or not b["median"]:
                continue
            rel = v["median"] / b["median"] - 1
            if rel <= THRESHOLD:
                continue
            apart = v["min"] > b["max"]
            level = "fail" if apart and same_node else "warn"
            out.append((level, doc, k, f"{b['median']:.4g} -> {v['median']:.4g} (+{rel * 100:.1f}%), "
                        f"ranges {b['min']:.4g}-{b['max']:.4g} / {v['min']:.4g}-{v['max']:.4g}"))
    return out


def record(ci, run, commit, date, subject, perf, main, by, backfill=False):
    """The run's perf entry in the history, the history and its report."""
    if not perf or not perf.get("docs"):
        return
    entry = {"commit": commit, "date": date, "subject": subject, "run": run.name, "main": main,
             "backfill": backfill, "node": perf.get("node"), "cpus": perf.get("cpus"),
             "load": perf.get("load"), "docs": perf.get("docs"), "ssa_edits": perf.get("ssa_edits"),
             "scoreboard": by}
    d = ci / "history.d"
    d.mkdir(parents=True, exist_ok=True)
    (d / f"{run.name}.json").write_text(json.dumps(entry, sort_keys=True) + "\n")
    entries = [e for e in (load(f) for f in sorted(d.glob("*.json"))) if e]
    entries.sort(key=lambda e: (e.get("date") or "", e.get("run") or ""))
    tmp = ci / f"history.jsonl.tmp-{run.name}"
    tmp.write_text("".join(json.dumps(e, sort_keys=True) + "\n" for e in entries))
    tmp.replace(ci / "history.jsonl")
    try:
        sys.path.insert(0, str(HERE))
        import report
        report.write(ci / "history.jsonl", ci / "report.html")
    except Exception as e:  # (the report is a view: its failure is not the run's)
        print(f"report: {e}")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--run", required=True)
    ap.add_argument("--ci", required=True)
    ap.add_argument("--main", action="store_true")
    ap.add_argument("--perf-only", action="store_true")
    ap.add_argument("--known", default=str(HERE.parent / "data" / "known-failing.txt"))
    a = ap.parse_args()
    run, ci = Path(a.run), Path(a.ci)
    info = (run / "bin" / "commit.txt").read_text().splitlines() if (run / "bin" / "commit.txt").exists() else []
    commit = info[0] if info else "unknown"
    date = info[1] if len(info) > 1 else None
    subject = info[2] if len(info) > 2 else ""
    perf = load(run / "perf" / "perf.json")
    if a.perf_only:
        record(ci, run, commit, date, subject, perf, True, {}, backfill=True)
        return
    cases = cases_of(run)
    rules = known_failing(a.known)
    baseline = load(ci / "main" / "latest.json", {})
    base_status = {(c["suite"], c["id"]): c["status"] for c in baseline.get("cases", [])}
    new_fail, regress, fixed, still = [], [], [], []
    for c in cases:
        key = (c["suite"], c["id"])
        ok = c["status"] in PASS
        known = is_known(rules, *key)
        c["known"] = known
        if not ok and c["status"] not in NEUTRAL:
            (still if known else new_fail).append(c)
        if base_status.get(key) in PASS and not ok:
            regress.append(c)
        if ok and known:
            fixed.append(c)
    missing = sorted(set(base_status) - {(c["suite"], c["id"]) for c in cases})
    history = []
    for line in (ci / "history.jsonl").read_text().splitlines() if (ci / "history.jsonl").exists() else []:
        try:
            history.append(json.loads(line))
        except ValueError:
            pass
    mains = [h for h in history if h.get("main") and not h.get("imported") and h.get("docs") and h.get("commit") != commit
             and (not date or (h.get("date") or "") <= date)]
    perf_base = max(mains, key=lambda h: h.get("date") or "", default=None)
    pc = perf_compare(perf, perf_base)
    failed = bool(new_fail or regress or any(l == "fail" for l, *_ in pc))
    by = counts(cases)
    board = {"commit": commit, "date": date, "subject": subject, "run": run.name,
             "scored": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
             "baseline": baseline.get("commit"), "counts": by, "cases": cases,
             "new_failures": [[c["suite"], c["id"]] for c in new_fail],
             "regressions": [[c["suite"], c["id"]] for c in regress],
             "fixed": [[c["suite"], c["id"]] for c in fixed], "missing": [list(m) for m in missing],
             "perf": [list(x) for x in pc], "result": "FAIL" if failed else "PASS"}
    (run / "scoreboard.json").write_text(json.dumps(board, indent=1) + "\n")

    L = [f"# CI scoreboard: {md(commit[:12])}", "", f"{md(subject)}", "",
         f"Result: **{board['result']}**; baseline (last main run): {md((baseline.get('commit') or 'none')[:12])}", "",
         "| suite | pass | differs | error | timeout | n/a | excluded | fail |", "|---|---|---|---|---|---|---|---|"]
    for s in sorted(by):
        n = by[s]
        L.append(f"| {md(s)} | {n.get('identical', 0) + n.get('pass', 0)} | {n.get('differs', 0)} | "
                 f"{n.get('error', 0)} | {n.get('timeout', 0)} | {n.get('n/a', 0)} | {n.get('excluded', 0)} | {n.get('fail', 0)} |")
    for title, lst in (("New failures (not in known-failing)", new_fail), ("Regressions against the last main run", regress),
                       ("Fixed (remove from ci/known-failing.txt)", fixed)):
        if lst:
            L += ["", f"## {title}", ""] + [f"- {md(c['suite'])} `{md(c['id'])}`: {md(c['status'])} {md(c.get('detail'))}" for c in lst]
    if missing:
        L += ["", "## In the baseline, not in this run", ""] + [f"- {md(s)} `{md(i)}`" for s, i in missing]
    diff = [c for c in cases if c["suite"].startswith("manuals") and c["status"] in ("differs", "error", "timeout")]
    if diff:
        L += ["", "## Manuals that differ (first differing object)", "", "| manual | mode | status | first difference |", "|---|---|---|---|"]
        for c in sorted(diff, key=lambda c: (c["id"], c["suite"])):
            L.append(f"| {md(c['id'])} | {md(c['suite'].split('/', 1)[1])} | {md(c['status'])} | {md(c.get('detail'))} |")
    fails = [c for c in cases if not c["suite"].startswith("manuals") and c["status"] == "fail"]
    if fails:
        L += ["", "## Engine suites: failing cases", ""] + [
            f"- {md(c['suite'])} `{md(c['id'])}`{' (known)' if c['known'] else ''}: {md(c.get('detail'))}" for c in fails[:400]]
    if perf:
        L += ["", f"## Performance ({md(perf.get('node'))}, {perf.get('cpus')} CPUs; baseline "
              f"{md((perf_base or {}).get('commit', 'none')[:12])})", ""]
        L += ["| document | TeX Live cold | partex plain | build | machine | SSA cold | SSA edit | TeX Live edit | plain/TL | peak RSS plain / TL / SSA |",
              "|---|---|---|---|---|---|---|---|---|---|"]

        def med(m, k, unit=""):
            v = m.get(k)
            return f"{v['median']:.3g}{unit}" if isinstance(v, dict) and "median" in v else "-"
        for doc, m in perf.get("docs", {}).items():
            ratio = "-"
            if isinstance(m.get("plain.cold_s"), dict) and isinstance(m.get("texlive.cold_s"), dict):
                ratio = f"{m['plain.cold_s']['median'] / m['texlive.cold_s']['median']:.2f}"
            rss = " / ".join(f"{m[k]['median'] / 1024:.0f} MB" if isinstance(m.get(k), dict) and "median" in m[k] else "-"
                             for k in ("plain.cold_rss_kib", "texlive.cold_rss_kib", "ssa.rss_kib"))
            L.append(f"| {md(doc)} | {med(m, 'texlive.cold_s', ' s')} | {med(m, 'plain.cold_s', ' s')} | {med(m, 'build.cold_s', ' s')} | "
                     f"{med(m, 'machine.cold_s', ' s')} | {med(m, 'ssa.wall_s', ' s')} | {med(m, 'ssa.edit_ms', ' ms')} | "
                     f"{med(m, 'texlive.edit_s', ' s')} | {ratio} | {rss} |")
        for level, doc, k, text in pc:
            L.append(f"- **{level}** {md(doc)} {md(k)}: {md(text)}")
    (run / "scoreboard.md").write_text("\n".join(L) + "\n")

    summary = [f"ci {commit[:12]}: {board['result']}"]
    for s in sorted(by):
        n = by[s]
        good = n.get("identical", 0) + n.get("pass", 0)
        total = sum(v for k, v in n.items() if k not in NEUTRAL)
        summary.append(f"  {s}: {good}/{total} pass" + "".join(f", {v} {k}" for k, v in sorted(n.items()) if k not in PASS))
    summary.append(f"  new failures {len(new_fail)}, regressions {len(regress)}, fixed {len(fixed)}, "
                   f"perf fail {sum(l == 'fail' for l, *_ in pc)}, warn {sum(l == 'warn' for l, *_ in pc)}")
    (run / "summary.txt").write_text("\n".join(summary) + "\n")
    print("\n".join(summary))

    if a.main:
        (ci / "main").mkdir(parents=True, exist_ok=True)
        (ci / "main" / f"{commit}.json").write_text(json.dumps(board, indent=1) + "\n")
        (ci / "main" / "latest.json").write_text(json.dumps(board, indent=1) + "\n")
    record(ci, run, commit, date, subject, perf, a.main, by)
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
