#!/usr/bin/env python3
"""The corpus tables and rankings from a results tree (only its .json
files are read: rsync --include='*.json' from accl's
~/shared-cache/phitex-corpus/results/RUN).

    report.py RESULTS_DIR [--manifest ARXIV_DIR] [--json OUT]
"""
import argparse
import json
import re
import statistics
from collections import Counter, defaultdict
from pathlib import Path

DIAG = re.compile(r"hint regime|undeclared definition|hints? (?:assumption|bound)", re.I)


def load(p):
    try:
        return json.loads(Path(p).read_text())
    except (OSError, ValueError):
        return None


def norm(msg):
    m = msg.strip()
    m = re.sub(r"`[^']*'", "`X'", m)
    m = re.sub(r'"[^"]*"', '"X"', m)
    m = re.sub(r"\\[A-Za-z@]+", lambda x: x.group(0) if len(x.group(0)) < 3 else "\\CS", m) if "Undefined control" not in m else m
    m = re.sub(r"\d+(\.\d+)?", "N", m)
    return m[:140]


def q(v, p):
    v = sorted(x for x in v if x is not None)
    if not v:
        return None
    k = (len(v) - 1) * p
    lo = int(k)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (k - lo)


def doc_row(d):
    r = {"id": d.name, "kind": "manual" if d.name.startswith("m-") else "arxiv"}
    prep = load(d / "oracle" / "prep.json") or load(d / "phitex" / "prep.json")
    o = load(d / "oracle" / "oracle.json")
    if prep and prep.get("status") != "ok":
        r.update(state="excluded", reason="prefilter: " + prep.get("reason", "?"))
        return r
    if not o:
        r.update(state="pending")
        return r
    if o.get("status") != "ok":
        r.update(state="excluded", reason=o.get("reason", "?"))
        return r
    r.update(oracle_s=o["wall_s"], pages=o.get("pages"), passes=o.get("passes"), pass_walls=o.get("pass_walls"))
    p = load(d / "phitex" / "phitex.json")
    if not p:
        r.update(state="no-phitex")
        return r
    r["walls"] = {f"{x.get('mode', 'exact')}-j{x['threads']}": x.get("cold_process_s") for x in p["runs"]}
    r["rss_kib"] = max(x.get("rss_kib") or 0 for x in p["runs"])
    first = p["runs"][0]
    b = (first.get("builds") or [{}])[0]
    errs = b.get("errors") or []
    if b.get("event") == "failed":
        errs = errs or [b.get("message", "failed")]
    r["tex_errors"] = [e for e in errs if not DIAG.search(e)]
    r["diagnostics"] = [e for e in errs if DIAG.search(e)]
    r["phitex_pages"] = b.get("pages")
    for mode in ("exact", "relaxed"):
        rs = [x for x in p["runs"] if x.get("mode", "exact") == mode]
        if not rs:
            continue
        bb = (rs[0].get("builds") or [{}])[0]
        if any(x.get("timeout") for x in rs):
            st, cause = "failing", "timeout"
        elif not bb.get("output") or rs[0].get("exit") not in (0, 1):
            st = "failing"
            cause = norm(r["tex_errors"][0]) if r["tex_errors"] else (
                norm(r["diagnostics"][0]) if r["diagnostics"] else f"no PDF, exit {rs[0].get('exit')}")
        elif mode == "exact" and all(x.get("first_diff") == -1 for x in rs):
            st, cause = "identical", None
        elif mode == "relaxed" and all(x.get("relaxed_eq") == "equal" for x in rs):
            st, cause = "identical", None
        else:
            st = "different"
            cause = (norm(r["tex_errors"][0]) if r["tex_errors"] else
                     ("page count differs" if bb.get("pages") != o.get("pages") else
                      ("PDF differs, no error" if mode == "exact" else
                       "relaxed: " + norm(next((x.get("relaxed_eq") or "" for x in rs if x.get("relaxed_eq") != "equal"), "")))))
        r[f"{mode}_state"] = st
        r[f"{mode}_cause"] = cause
        r[f"{mode}_first_diff"] = [x.get("first_diff") for x in rs]
        if mode == "relaxed":
            fb = [n for x in rs for bld in (x.get("builds") or []) for n in (bld.get("notes") or []) if "PDF objects" in n]
            r["fallback"] = fb[0] if fb else None
    r["state"] = r.get("exact_state", "failing")
    r["cause"] = r.get("exact_cause")
    t = p.get("timed") or {}
    tb = t.get("builds") or []
    if tb:
        par = tb[0].get("parallel") or {}
        r["critical_path"] = par.get("critical_path")
        r["busy_ms"] = par.get("busy_ms")
        r["engine_ms"] = par.get("engine_ms")
        r["hints"] = par.get("hints")
        r["nodes_run"] = tb[0].get("nodes_run")
    st = t.get("stderr") or {}
    rounds = st.get("rounds") or []
    ste = (t.get("stderr_edit") or {}).get("rounds") or []
    cold_rounds = rounds[: len(rounds) - len(ste)] if ste else rounds

    def sums(rs):
        nodes = runs_ = loops = stops = restarts = 0
        for line in rs:
            g = lambda k: int(re.search(r"(\d+) " + k, line).group(1)) if re.search(r"(\d+) " + k, line) else 0
            nodes += g("nodes run")
            runs_ += g("runs")
            loops += g("loop rounds")
            stops += g("stops")
            restarts += g("restarts")
        return nodes, runs_, loops, stops, restarts
    n, ru, lo, sto, rest = sums(cold_rounds)
    r.update(rounds=len(cold_rounds), loop_rounds=lo, runs_per_node=round(ru / n, 3) if n else None,
             stops=sto, restarts=rest)
    r["ops_ms"] = {k: v["ms"] for k, v in (st.get("ops") or {}).items()}
    r["serial"] = [l for l in st.get("lines", []) if l.startswith(("push_text", "time:", "settle phases", "pdf finish", "output file", "format deserialise", "prelude definitions", "incr walk"))][:20]
    if len(tb) >= 2:
        e = tb[1]
        r["edit_ms"] = e.get("wall_ms")
        r["edit_process_s"] = t.get("edit_process_s")
        r["edit_nodes"] = e.get("nodes_run")
        _, _, elo, _, _ = sums(ste)
        r["edit_rounds"] = len(ste)
        r["edit_loop_rounds"] = elo
    return r


def table(rows, name):
    built = [r for r in rows if r["state"] in ("identical", "different", "failing")]
    c = Counter(r["state"] for r in rows)
    out = [f"## {name}: {len(rows)} documents",
           f"  excluded {c['excluded']}, pending {c['pending'] + c['no-phitex']}, measured {len(built)}"]
    for mode in ("exact", "relaxed"):
        cm = Counter(r.get(f"{mode}_state") for r in built)
        label = "byte-identical" if mode == "exact" else "equivalent (gate 2)"
        out.append(f"  {mode}: {label} {cm['identical']}, different {cm['different']}, failing {cm['failing']}")
    fb = Counter(r["fallback"] for r in built if r.get("fallback"))
    if fb:
        out.append("  relaxed fell back to exact: " + "; ".join(f"{n} {k}" for k, n in fb.most_common()))
    keys = sorted({k for r in built for k in r.get("walls", {})}, key=lambda k: (k.split("-")[0] != "relaxed", int(k.split("j")[-1])))
    for k in keys:
        rat = [r["walls"][k] / r["oracle_s"] for r in built if r.get("walls", {}).get(k) and r.get("oracle_s")]
        ok = [r["walls"][k] / r["oracle_s"] for r in built if r.get("walls", {}).get(k) and r.get("oracle_s")
              and r.get(k.split("-")[0] + "_state") == "identical"]
        if rat:
            out.append(f"  {k}: phitex/pdflatex wall ratio median {q(rat, .5):.2f}, p90 {q(rat, .9):.2f} (n={len(rat)})"
                       + (f"; on correct ones median {q(ok, .5):.2f}, p90 {q(ok, .9):.2f} (n={len(ok)})" if ok else "")
                       + f"; phitex median {q([r['walls'][k] for r in built if r.get('walls', {}).get(k)], .5):.2f} s")
    out.append(f"  pdflatex (all passes) median {q([r['oracle_s'] for r in built], .5) or 0:.2f} s, "
               f"p90 {q([r['oracle_s'] for r in built], .9) or 0:.2f} s")
    em = [r["edit_ms"] for r in built if r.get("edit_ms") is not None]
    if em:
        out.append(f"  edit wall (relaxed) median {q(em, .5):.0f} ms, p90 {q(em, .9):.0f} ms (n={len(em)}); "
                   f"nodes re-run median {q([r.get('edit_nodes') or 0 for r in built if r.get('edit_ms') is not None], .5):.0f}")
    cp = [r["critical_path"] for r in built if r.get("critical_path")]
    rn = [r["runs_per_node"] for r in built if r.get("runs_per_node")]
    if cp:
        out.append(f"  critical path median {q(cp, .5):.0f} nodes, runs/node median {q(rn, .5) or 0:.2f}")
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("results")
    ap.add_argument("--json")
    a = ap.parse_args()
    rows = [doc_row(d) for d in sorted(Path(a.results).iterdir()) if d.is_dir()]
    lines = []
    for kind in ("arxiv", "manual"):
        rs = [r for r in rows if r["kind"] == kind]
        if rs:
            lines += table(rs, kind)
            ex = Counter(re.sub(r"\(.*", "", r["reason"]).strip()[:90] for r in rs if r["state"] == "excluded")
            lines.append("  exclusions: " + "; ".join(f"{n} {k}" for k, n in ex.most_common(25)))
    causes = defaultdict(list)
    for r in rows:
        if r.get("cause"):
            causes[r["cause"]].append(r["id"])
    lines.append("## failure causes (documents affected)")
    for k, v in sorted(causes.items(), key=lambda kv: -len(kv[1]))[:40]:
        lines.append(f"  {len(v):4d}  {k}   e.g. {', '.join(v[:3])}")
    print("\n".join(lines))
    if a.json:
        Path(a.json).write_text(json.dumps(rows, indent=1))


if __name__ == "__main__":
    main()
