#!/usr/bin/env python3
"""The speed benchmark behind phitex.org's proof page (bench.json): each
document of a corpus built cold by TeX Live's engine (the oracle) and by
PhiTeX's release binary, back to back on one machine, and each edit of
the edit matrix timed through the watch beside a full rebuild of the
edited document by TeX Live's engine.

    bench.py cold  --list ci/research.json --corpus research --doc DIR --bin PHITEX --out DIR [--runs 3]
    bench.py edits --bin PHITEX --docs grail:amsldoc:... --src-root DIR --out DIR
    bench.py json  --cold DIR [DIR...] --edits DIR [DIR...] --out bench.json [--commit SHA] [--machine FILE]

`cold` and `edits` run inside the CI image (scripts/accl/bench.sbatch),
as scripts/ci/sweep.py does; `json` runs anywhere.

cold: per document, the oracle and PhiTeX alternate, RUNS times each
(oracle, phitex, oracle, phitex, ...), each in a fresh copy of the
document's directory:
  - the oracle: manuals.py's latexmk-like loop (pdflatex or xelatex, then
    BibTeX, biber, makeindex as the job asks, to the fixpoint), its wall
    the sum of every pass and tool it ran;
  - PhiTeX: `phitex build --exact-pdf -j 1` with an empty cache directory
    (the engine's formats made once before the first document and linked
    in, as TeX Live's formats are made once in the image), its wall the
    whole build's; no engine time limit, the wall limit TIMEOUT_MIN or
    TIMEOUT_X times the oracle's median, whichever is longer (a slow
    document is kept: a run past its limit is recorded as a timeout and
    not tried again).
The PDF's md5 of each run is kept. Documents are claimed one at a time
(an mkdir under OUT/claims), so several jobs on several nodes share one
list, each building one document at a time. One result file per document:
OUT/<corpus>/<id>.json.

edits: per document of the edit matrix (scripts/ci/editmatrix.py, the
speed scoreboard's runner), its edits at -j 1 (REPS tries, the median)
and each edited source once by the oracle.
"""

import argparse
import datetime
import json
import os
import shutil
import statistics
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402

TIMEOUT_MIN = 1800
TIMEOUT_X = 10


def md5(p):
    import hashlib
    try:
        return hashlib.md5(Path(p).read_bytes()).hexdigest()
    except OSError:
        return None


def oracle_run(c, docroot, work):
    import manuals
    run_dir = work / "run" / f"o-{c['id']}"
    common.fresh_copy(Path(docroot) / c["dir"], run_dir)
    job = Path(c["main"]).stem
    (run_dir / f"{job}.pdf").unlink(missing_ok=True)
    base = {rel: common.sha256_file(run_dir / rel) for rel in common.tree_files(run_dir)}
    ist = {k for k in base if k.endswith(".ist")}
    log = []
    r = manuals.loop(run_dir, manuals.oracle_cmd(c["engine"], c["main"]), manuals.Tools(),
                     manuals.side_env(c["engine"]), job, base, ist, log)
    pdf = run_dir / f"{job}.pdf"
    res = {"wall_s": r["wall_s"], "passes": r["passes"], "tools": r["tools"], "status": r["status"],
           "settled": r.get("settled"), "md5": md5(pdf) if pdf.exists() else None,
           "errors": manuals.errors_in(run_dir / f"{job}.log"), "rss_kib": r["rss_kib"],
           "steps": [{"cmd": x["cmd"], "wall_s": x["wall_s"]} for x in log]}
    shutil.rmtree(run_dir, ignore_errors=True)
    return res


def phitex_run(c, docroot, work, binary, limit):
    import manuals
    tag = f"p-{c['id']}"
    run_dir = work / "run" / tag
    common.fresh_copy(Path(docroot) / c["dir"], run_dir)
    job = Path(c["main"]).stem
    (run_dir / f"{job}.pdf").unlink(missing_ok=True)
    cdir = work / "cache" / tag
    shutil.rmtree(cdir, ignore_errors=True)
    cdir.mkdir(parents=True)
    os.symlink(work / "fcache" / "formats", cdir / "formats")
    sdir = work / "store" / tag
    shutil.rmtree(sdir, ignore_errors=True)
    env = manuals.side_env(c["engine"], {"PHITEX_CACHE_DIR": str(cdir), "PHITEX_STORE_DIR": str(sdir),
                                         "PHITEX_TIME_LIMIT_SECS": "0"}, phitex=True)
    term = run_dir / "term-phitex.txt"
    cmd = [binary, "build", "--exact-pdf", "-j", "1", "--no-synctex", "--engine", c["engine"], c["main"]]
    r = common.run(cmd, run_dir, env, limit, out=term, cpu_s=limit * 4)
    pdf = run_dir / f"{job}.pdf"
    res = {"wall_s": r["wall_s"], "exit": r["exit"], "timeout": r["timeout"], "rss_kib": r["rss_kib"],
           "md5": md5(pdf) if pdf.exists() else None}
    if r["exit"] != 0 or res["md5"] is None:
        try:
            res["term_tail"] = term.read_bytes()[-1500:].decode("utf-8", "replace")
        except OSError:
            pass
    shutil.rmtree(run_dir, ignore_errors=True)
    shutil.rmtree(cdir, ignore_errors=True)
    shutil.rmtree(sdir, ignore_errors=True)
    return res


def prewarm(binary, engines, work):
    import manuals
    d = work / "fcache"
    (d / "formats").mkdir(parents=True, exist_ok=True)
    for e in engines:
        w = work / "run" / f"prewarm-{e}"
        w.mkdir(parents=True, exist_ok=True)
        (w / "p.tex").write_text("\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n")
        env = manuals.side_env(e, {"PHITEX_CACHE_DIR": str(d)}, phitex=True)
        r = common.run([binary, "build", "--exact-pdf", "-j", "1", "--no-synctex", "--engine", e, "p.tex"],
                       w, env, 900, out=w / "term.txt")
        print(f"prewarm {e}: exit {r['exit']} {r['wall_s']} s; formats {sorted(os.listdir(d / 'formats'))}", flush=True)


def median(xs):
    return statistics.median(xs) if xs else None


def cmd_cold(args):
    import manuals
    work = Path(os.environ.get("CI_WORK", "/w"))
    items = manuals.load_list(args.list)
    if args.ids:
        # (',' or ':': sbatch's --export values cannot hold commas)
        want = set(args.ids.replace(":", ",").split(","))
        items = [c for c in items if c["id"] in want]
    # (the longest first, so the long tail does not end the run)
    items = sorted(items, key=lambda c: (-(c.get("oracle_wall_s") or 0), c["id"]))
    out = Path(args.out) / args.corpus
    claims = Path(args.out) / "claims"
    out.mkdir(parents=True, exist_ok=True)
    claims.mkdir(parents=True, exist_ok=True)
    prewarm(args.bin, sorted({c["engine"] for c in items}), work)
    host = os.uname().nodename
    print(f"{len(items)} documents of {args.corpus} on {host}, {args.runs} runs each", flush=True)
    for c in items:
        res_p = out / f"{c['id']}.json"
        if res_p.exists():
            continue
        try:
            os.mkdir(claims / f"{args.corpus}__{c['id']}")
        except FileExistsError:
            continue
        t0 = time.time()
        load = open("/proc/loadavg").read().split()[:3]
        o_runs, p_runs = [], []
        limit = None
        for k in range(args.runs):
            if not o_runs or o_runs[-1]["status"] == "ok":
                o_runs.append(oracle_run(c, args.doc, work))
            if limit is None:
                limit = max(TIMEOUT_MIN, int(TIMEOUT_X * o_runs[0]["wall_s"]))
            if not p_runs or not p_runs[-1]["timeout"]:
                p_runs.append(phitex_run(c, args.doc, work, args.bin, limit))
        ok_o = [r for r in o_runs if r["status"] == "ok" and r["md5"]]
        ok_p = [r for r in p_runs if not r["timeout"] and r["md5"]]
        o_md5 = sorted({r["md5"] for r in ok_o})
        p_md5 = sorted({r["md5"] for r in ok_p})
        res = {
            "id": c["id"], "path": c["path"], "engine": c["engine"], "corpus": args.corpus,
            "node": host, "load_at_start": load, "started": datetime.datetime.fromtimestamp(t0).isoformat(timespec="seconds"),
            "oracle_s": median([r["wall_s"] for r in ok_o]), "phitex_s": median([r["wall_s"] for r in ok_p]),
            "oracle_md5": o_md5, "phitex_md5": p_md5, "pinned_oracle_md5": c.get("oracle_md5"),
            "timeout": any(r["timeout"] for r in p_runs), "limit_s": limit,
            "identical": bool(o_md5) and o_md5 == p_md5 and len(p_md5) == 1 and len(ok_p) == len(p_runs),
            "oracle_runs": o_runs, "phitex_runs": p_runs,
        }
        common.write_json(res_p, res)
        r = res["phitex_s"] / res["oracle_s"] if res["phitex_s"] and res["oracle_s"] else None
        print(f"{c['id']} ({c['engine']}): oracle {res['oracle_s']} s, phitex {res['phitex_s']} s"
              f"{f' ({r:.2f}x)' if r else ''}, identical {res['identical']}"
              f"{', TIMEOUT' if res['timeout'] else ''} ({time.time() - t0:.0f} s)", flush=True)


# -- the edit matrix --------------------------------------------------------

EDIT_DOCS = {
    # doc: (manual id or a path under the repo, main, engine)
    "amsldoc": ("bench/amsldoc", "amsldoc.tex", "pdflatex"),
    "para4000": ("bench/para4000.tex", "para4000.tex", "pdftex"),
    "grail": (None, "grail.tex", "xelatex"),
    "tikzcd": ("latex_tikz-cd_tikz-cd-doc", None, None),
    "pgfplots": ("latex_pgfplots_pgfplotsexample", None, None),
    "circuitikz": ("latex_circuitikz_circuitikzmanual", None, None),
    "biblatex": ("latex_biblatex_examples_01-introduction", None, None),
    "beamer": ("latex_beamertheme-metropolis_demo", None, None),
}


def cmd_edits(args):
    import manuals
    work = Path(os.environ.get("CI_WORK", "/w"))
    repo = Path(args.repo)
    byid = {c["id"]: c for c in manuals.load_list(repo / "ci" / "manuals.json")}
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    for doc in args.docs.split(":"):
        src, main, eng = EDIT_DOCS[doc]
        sd = work / "edocs" / doc
        shutil.rmtree(sd, ignore_errors=True)
        sd.parent.mkdir(parents=True, exist_ok=True)
        if doc == "grail":
            shutil.copytree(args.grail, sd, symlinks=False)
        elif src.endswith(".tex"):
            sd.mkdir()
            shutil.copy(repo / src, sd / main)
        elif main:
            shutil.copytree(repo / src, sd)
        else:
            c = byid[src]
            main, eng = c["main"], c["engine"]
            shutil.copytree(Path(args.doc) / c["dir"], sd)
        subprocess.run(["chmod", "-R", "u+w", str(sd)])
        env = dict(os.environ, JS="1", REPS=str(args.reps), PHASES="timed:oracle", PO="0",
                   WORK=str(work / f"m-{doc}"), PHITEX_CACHE_DIR=str(work / f"cache-{doc}"),
                   BUILD_TMO=str(args.build_tmo), CAP_SKIP="1")
        t0 = time.time()
        with open(out / f"{doc}.out", "wb") as fh:
            r = subprocess.run([sys.executable, "-I", str(Path(__file__).resolve().parent / "editmatrix.py"),
                                args.bin, "-", str(sd), main, eng, doc, str(out)],
                               cwd=repo, env=env, stdout=fh, stderr=subprocess.STDOUT)
        print(f"edits {doc}: exit {r.returncode} ({time.time() - t0:.0f} s)", flush=True)
        shutil.rmtree(work / f"m-{doc}", ignore_errors=True)


# -- bench.json ------------------------------------------------------------

DEST_NOTE = ("Inside a rotated or transformed box (\\rotatebox, a landscape page), pdfTeX computes a link "
             "target's position from leftover memory instead of the target itself, so the link lands somewhere "
             "unrelated, often far above the page. PhiTeX sends the link to the target. The pages themselves are "
             "identical; only where these links point differs.")

KIND_WORDS = {
    "letter": "changing a letter", "word": "changing a word", "sentence": "adding a sentence",
    "addpar": "adding a paragraph", "delpar": "deleting a paragraph", "caption": "changing a figure caption",
    "footnote": "changing a footnote", "math": "changing an equation", "tikz": "changing a drawing",
    "heading": "changing a heading", "addsec": "adding a section", "delsec": "deleting a section",
    "label": "changing a label", "ref": "changing a reference", "cite": "changing a citation",
    "addfloat": "adding a figure", "delfloat": "deleting a figure", "index": "changing an index entry",
    "macro": "changing a macro",
}
EDIT_TITLES = {
    "amsldoc": "latex/amsmath/amsldoc.tex", "para4000": "a plain TeX test of 4000 paragraphs",
    "grail": "Grail, an 18-page paper", "tikzcd": "latex/tikz-cd/tikz-cd-doc.tex",
    "pgfplots": "latex/pgfplots/pgfplotsexample.tex", "circuitikz": "latex/circuitikz/circuitikzmanual.tex",
    "biblatex": "latex/biblatex/examples/01-introduction.tex", "beamer": "latex/beamertheme-metropolis/demo.tex",
}


def tsv_rows(path):
    rows = []
    for line in open(path):
        if line.strip() and not line.startswith("#"):
            rows.append(line.rstrip("\n").split("\t"))
    return rows


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return None
    k = (len(xs) - 1) * q
    lo = int(k)
    hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def cmd_json(args):
    repo = Path(args.repo)
    research = json.load(open(repo / "ci" / "research.json"))
    mj = json.load(open(repo / "ci" / "manuals.json"))
    lists = {"research": research["documents"], "manuals": mj["manuals"]}
    res = {"research": {}, "manuals": {}}
    for d in args.cold or []:
        for corpus in res:
            for p in sorted((Path(d) / corpus).glob("*.json")):
                r = json.load(open(p))
                res[corpus][r["id"]] = r
    bugs = {r[0]: r for r in tsv_rows(repo / "ci" / "manuals-pdftex-bugs.tsv")}
    slow_ids = {r[0] for r in tsv_rows(repo / "ci" / "manuals-slow.tsv")}
    corpora, differences, slow, points = [], [], [], []
    measured_all = True
    covers = []
    for corpus in args.corpora.split(","):
        lst = lists[corpus]
        done = res[corpus]
        if len(done) < len(lst):
            measured_all = False
        covers.append(f"{corpus}: {len(done)} of {len(lst)} documents measured")
        engines = {}
        for c in lst:
            e = engines.setdefault(c["engine"], {"total": 0, "identical": 0, "slow": 0,
                                                 "by_class": {"pdftex-bug": 0, "phitex-bug": 0, "not-examined": 0},
                                                 "measured": 0})
            e["total"] += 1
            r = done.get(c["id"])
            is_slow = corpus == "manuals" and c["id"] in slow_ids
            if is_slow:
                e["slow"] += 1
                slow.append({"corpus": corpus, "doc": c["path"], "engine": c["engine"],
                             "note": (f"PhiTeX {r['phitex_s']:.0f} s, {c['engine']} {r['oracle_s']:.0f} s on one machine"
                                      if r and r.get("phitex_s") and r.get("oracle_s") else
                                      "timed out" if r and r.get("timeout") else None)})
            if not r:
                continue
            e["measured"] += 1
            if r["oracle_s"] and r["phitex_s"]:
                points.append({"corpus": corpus, "doc": c["path"], "engine": c["engine"], "oracle_s": round(r["oracle_s"], 3),
                               "phitex_s": round(r["phitex_s"], 3)})
            if r["identical"]:
                e["identical"] += 1
            elif c["id"] in bugs:
                e["by_class"]["pdftex-bug"] += 1
                differences.append({"corpus": corpus, "doc": c["path"], "engine": c["engine"], "cls": "pdftex-bug",
                                    "link": bugs[c["id"]][1], "note": DEST_NOTE if bugs[c["id"]][2] == "dest-under-matrix" else None})
            else:
                e["by_class"]["not-examined"] += 1
                why = ("timed out after {} s".format(r["limit_s"]) if r.get("timeout") else
                       "PhiTeX made no PDF" if not r["phitex_md5"] else
                       "TeX Live made no PDF" if not r["oracle_md5"] else
                       "PhiTeX's PDF differs between runs" if len(r["phitex_md5"]) > 1 else
                       "the PDF differs from TeX Live's (md5)")
                differences.append({"corpus": corpus, "doc": c["path"], "engine": c["engine"], "cls": "not-examined",
                                    "link": None, "note": why})
        for e in engines.values():
            if e["measured"] == e["total"]:
                del e["measured"]
        ex = None
        if corpus == "manuals":
            by = {}
            for x in mj["excluded"]:
                k = x.get("engine") if x.get("engine") in ("pdflatex", "xelatex", "lualatex") else "other"
                by[k] = by.get(k, 0) + 1
            ex = {"total": len(mj["excluded"]), "by_engine": by, "link": "ci/manuals.json"}
        corpora.append({
            "name": corpus,
            "title": "TeX Live's manuals" if corpus == "manuals" else "Research templates",
            "about": ("Every manual in TeX Live that pdfLaTeX or XeLaTeX builds without an error." if corpus == "manuals" else
                      "The templates researchers start from: IEEEtran, acmart, llncs, elsarticle, REVTeX, amsart, beamer, theses and more."),
            "excluded": ex, "engines": engines})
    summary = {}
    for e in sorted({p["engine"] for p in points}):
        rs = [p["phitex_s"] / p["oracle_s"] for p in points if p["engine"] == e]
        summary[e] = {"docs": len(rs), "p50_ratio": round(pct(rs, 0.5), 2), "p90_ratio": round(pct(rs, 0.9), 2),
                      "faster": sum(1 for x in rs if x < 1)}
    by_corpus = {}
    for k in sorted({p["corpus"] for p in points}):
        for e in sorted({p["engine"] for p in points if p["corpus"] == k}):
            rs = [p["phitex_s"] / p["oracle_s"] for p in points if p["engine"] == e and p["corpus"] == k]
            by_corpus.setdefault(k, {})[e] = {"docs": len(rs), "p50_ratio": round(pct(rs, 0.5), 2),
                                              "p90_ratio": round(pct(rs, 0.9), 2), "faster": sum(1 for x in rs if x < 1)}
    cold = {"runs": 3, "summary": summary, "by_corpus": by_corpus, "points": points} if points else None

    edit = None
    eps, missing = [], []
    for d in args.edits or []:
        for p in sorted(Path(d).glob("*.jsonl")):
            if p.name.endswith("-cells.jsonl"):
                continue
            for line in open(p):
                r = json.loads(line)
                j1 = r.get("j1")
                o = r.get("oracle") or {}
                eng = EDIT_DOCS[r["doc"]][2] or next((c["engine"] for c in mj["manuals"] if c["id"] == EDIT_DOCS[r["doc"]][0]), None)
                if not j1 or not o.get("wall_s"):
                    missing.append(f"{r['doc']} {r['kind']}/{r['place']}")
                    continue
                eps.append({"doc": EDIT_TITLES.get(r["doc"], r["doc"]), "engine": eng,
                            "edit": f"{KIND_WORDS.get(r['kind'], r['kind'])} ({r['place']})",
                            "ms": round(j1["med_ms"], 1), "oracle_s": round(o["wall_s"], 3),
                            "same_pdf_as_oracle": bool(o.get("pdf")) and j1.get("pdfs") == [o.get("pdf")]})
    if eps:
        mss = [p["ms"] for p in eps]
        buckets, lo = [], 0
        for le in (10, 30, 100, 300, 1000, None):
            n = sum(1 for m in mss if m > lo and (le is None or m <= le))
            buckets.append({"le_ms": le, "n": n})
            lo = le or lo
        edit = {"cases": len(eps), "under_100ms": sum(1 for m in mss if m <= 100), "p50_ms": round(pct(mss, 0.5), 1),
                "buckets": buckets, "points": eps,
                "worst": [{"doc": p["doc"], "edit": p["edit"], "ms": p["ms"]} for p in sorted(eps, key=lambda p: -p["ms"])[:10]]}
        covers.append(f"edits: {len(eps)} one-thread edit cases measured" + (f", {len(missing)} without a time ({'; '.join(missing[:20])})" if missing else ""))
    else:
        measured_all = False
        covers.append("edits: not measured yet")
    machine = json.load(open(args.machine)) if args.machine else None
    if machine is not None:
        # (the nodes the included measurements ran on)
        machine["nodes"] = sorted({r["node"] for k in args.corpora.split(",") for r in res[k].values()}
                                  | {Path(d).name for d in args.edits or [] if any(Path(d).glob("*.jsonl"))})
    out = {
        # (every point is a measurement; what is not measured yet is left
        # out and named in covers)
        "schema": 1, "sample": False, "complete": measured_all and not missing,
        "covers": args.covers or "; ".join(covers),
        "commit": args.commit, "date": datetime.date.today().isoformat(),
        "texlive": mj["texlive"], "machine": machine,
        "corpora": corpora,
        "timing": {"cold": cold, "edit": edit},
        "differences": differences, "slow": slow,
    }
    if args.note:
        out["note"] = args.note
    if args.method:
        out["method"] = args.method
    common.write_json(args.out, out)
    print(json.dumps({"covers": out["covers"], "cold": summary, "edit": {k: v for k, v in (edit or {}).items() if k not in ("points", "worst")},
                      "differences": len(differences)}, indent=1))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("cold")
    p.add_argument("--list", required=True)
    p.add_argument("--corpus", required=True)
    p.add_argument("--doc", required=True)
    p.add_argument("--bin", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--runs", type=int, default=3)
    p.add_argument("--ids")
    p = sub.add_parser("edits")
    p.add_argument("--bin", required=True)
    p.add_argument("--docs", required=True)
    p.add_argument("--repo", required=True)
    p.add_argument("--doc", default="/usr/share/doc/texlive")
    p.add_argument("--grail")
    p.add_argument("--out", required=True)
    p.add_argument("--reps", type=int, default=5)
    p.add_argument("--build-tmo", type=int, default=600)
    p = sub.add_parser("json")
    p.add_argument("--repo", default=".")
    p.add_argument("--cold", nargs="*")
    p.add_argument("--edits", nargs="*")
    p.add_argument("--out", required=True)
    p.add_argument("--commit")
    p.add_argument("--machine")
    p.add_argument("--note")
    p.add_argument("--corpora", default="manuals,research")
    p.add_argument("--covers")
    p.add_argument("--method")
    args = ap.parse_args()
    {"cold": cmd_cold, "edits": cmd_edits, "json": cmd_json}[args.cmd](args)


if __name__ == "__main__":
    main()
