#!/usr/bin/env python3
"""The corpus sweep (scripts/accl/corpus-sweep): every pinned pdfLaTeX
manual of ci/manuals.json built cold by `phitex build --exact-pdf` at
-j 1 and at -j 8, each against the oracle (TeX Live's pdflatex run to a
settled fixpoint, the manuals.py oracle cache).

    sweep.py run   --list ci/manuals.json --doc DIR --cache DIR --bin PHITEX --out DIR
                   [--ids a,b] [--jobs 1,8] [--par 1:20,8:5] [--timeout 120]
    sweep.py table RESULTS_DIR [--json OUT]

Per build: exit, the PDF's md5 against the oracle's, pages against the
oracle's, rounds (the `round:` lines PHITEX_TEX_TIME prints, a profile
build), wall; for a PDF that differs, where it first differs (pdfdiff.py's
first differing object and page, and the first page whose pdftotext text
differs). Failing builds keep their terminal, log and (small) PDF under
OUT/files/<id>-j<j>/. Runs inside the CI container (corpus-sweep.sbatch):
cold means a fresh run directory and a fresh cache directory per build,
the format alone shared (built once before the pool).
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import manuals  # noqa: E402
import pdfdiff  # noqa: E402

WORK = manuals.WORK


def md5(p):
    try:
        return hashlib.md5(Path(p).read_bytes()).hexdigest()
    except OSError:
        return None


def npages(pdf):
    try:
        r = subprocess.run(["qpdf", "--show-npages", str(pdf)], capture_output=True, timeout=120)
        if r.returncode in (0, 3):
            return int(r.stdout.strip())
    except (OSError, ValueError, subprocess.TimeoutExpired):
        pass
    try:
        r = subprocess.run(["pdfinfo", str(pdf)], capture_output=True, timeout=120)
        m = re.search(rb"^Pages:\s+(\d+)", r.stdout, re.M)
        return int(m.group(1)) if m else None
    except (OSError, subprocess.TimeoutExpired):
        return None


def text_pages(pdf):
    try:
        r = subprocess.run(["pdftotext", "-layout", str(pdf), "-"], capture_output=True, timeout=300)
        if r.returncode != 0:
            return None
        return r.stdout.split(b"\f")
    except (OSError, subprocess.TimeoutExpired):
        return None


def text_diff(oracle, mine):
    """The first page whose pdftotext text differs, and its first
    differing line on each side."""
    a, b = text_pages(oracle), text_pages(mine)
    if a is None or b is None:
        return {"text": "pdftotext failed"}
    for i, (x, y) in enumerate(zip(a, b)):
        if x != y:
            la, lb = x.split(b"\n"), y.split(b"\n")
            for k in range(max(len(la), len(lb))):
                u = la[k] if k < len(la) else b"<end>"
                v = lb[k] if k < len(lb) else b"<end>"
                if u != v:
                    return {"text_page": i + 1, "text_line": k + 1,
                            "text_oracle": u[:200].decode("utf-8", "replace"),
                            "text_phitex": v[:200].decode("utf-8", "replace")}
            return {"text_page": i + 1}
    if len(a) != len(b):
        return {"text_page": min(len(a), len(b)) + 1, "text_oracle": f"{len(a)} pages", "text_phitex": f"{len(b)} pages"}
    return {"text_same": True}


ERR = (
    rb"panicked at[^\n]*\n?[^\n]*",
    rb"^error[^\n]*",
    rb"^! [^\n]*",
    rb"pdfTeX error[^\n]*",
    rb"[Ee]rror: [^\n]*",
)


def first_error(*texts):
    for pat in ERR:
        for t in texts:
            m = re.search(pat, t, re.M)
            if m:
                return m.group(0)[:400].decode("utf-8", "replace")
    return None


def counters(t):
    out = {}
    # (the settle's own count, `rounds: N, ...`; else the round lines)
    m = re.search(r"^rounds: (\d+)", t, re.M)
    out["rounds"] = int(m.group(1)) if m else len(re.findall(r"^round: ", t, re.M))
    m = list(re.finditer(r"(\d+) loop rounds", t))
    if m:
        out["loop_rounds"] = int(m[-1].group(1))
    m = list(re.finditer(r"(\d+) pages", t))
    if m:
        out["said_pages"] = int(m[-1].group(1))
    m = list(re.finditer(r"(\d+) nodes run of (\d+) live", t))
    if m:
        out["nodes_run"], out["live"] = int(m[-1].group(1)), int(m[-1].group(2))
    return out


def keep(res_dir, files):
    res_dir.mkdir(parents=True, exist_ok=True)
    for src, name, cap in files:
        try:
            data = Path(src).read_bytes()
        except OSError:
            continue
        if cap and len(data) > cap:
            if name.endswith(".pdf"):
                continue
            data = data[:cap // 3] + b"\n[... cut ...]\n" + data[-2 * cap // 3:]
        (res_dir / name).write_bytes(data)


def oracle(c, cache, doc):
    h = common.tree_hash(Path(doc) / c["dir"])
    if c.get("input_hash") and h != c["input_hash"]:
        return None, None, f"input tree hash {h[:16]} is not the pinned {c['input_hash'][:16]}"
    key, ores = manuals_oracle(cache, c, h)
    if not ores:
        return None, None, "no oracle in the cache"
    if ores["status"] != "ok":
        return None, None, "oracle excluded: " + str(ores.get("reason"))
    return key, ores, None


def manuals_oracle(cache, c, h):
    key = Path(cache) / f"{c['id']}-{h[:16]}"
    res = common.read_json(key / "result.json")
    if res and res.get("harness") == manuals.HARNESS_VERSION:
        return key, res
    return None, None


def run_one(c, j, args):
    base = {"id": c["id"], "path": c["path"], "tools": c.get("tools", []), "j": j,
            "oracle_passes": c.get("passes"), "oracle_wall_s": c.get("oracle_wall_s")}
    key, ores, why = oracle(c, args.cache, args.doc)
    if why:
        return {**base, "status": "no-oracle", "note": why}
    opdf = key / "files" / ores["pdf"]
    base.update(oracle_md5=md5(opdf), oracle_pages=npages(opdf))
    tag = f"{c['id']}-j{j}"
    run_dir = WORK / "run" / tag
    common.fresh_copy(Path(args.doc) / c["dir"], run_dir)
    job = Path(c["main"]).stem
    (run_dir / f"{job}.pdf").unlink(missing_ok=True)
    cdir = WORK / "cache" / tag
    shutil.rmtree(cdir, ignore_errors=True)
    cdir.mkdir(parents=True)
    os.symlink(WORK / "fcache" / "formats", cdir / "formats")
    env = manuals.side_env(c["engine"], {"PHITEX_TEX_TIME": "1", "PHITEX_CACHE_DIR": str(cdir)}, phitex=True)
    term = run_dir / "term-phitex.txt"
    cmd = [args.bin, "build", "--exact-pdf", "-j", str(j), "--no-synctex", c["main"]]
    r = common.run(cmd, run_dir, env, args.timeout, out=term, cpu_s=args.timeout * (j + 4))
    text = term.read_bytes() if term.exists() else b""
    logb = b""
    try:
        logb = (run_dir / f"{job}.log").read_bytes()
    except OSError:
        pass
    res = {**base, "exit": r["exit"], "wall_s": r["wall_s"], "rss_kib": r["rss_kib"]}
    res.update(counters(text.decode("utf-8", "replace")))
    pdf = run_dir / ores["pdf"]
    res["md5"] = md5(pdf) if pdf.exists() else None
    res["pages"] = npages(pdf) if pdf.exists() else None
    if r["timeout"]:
        res.update(status="timeout", note=f"over {args.timeout} s")
    elif b"panicked at" in text:
        res.update(status="panic", note=first_error(text))
    elif res["md5"] is None:
        res.update(status="error", note=first_error(text, logb) or manuals.why_failed(run_dir, job, term))
    elif res["md5"] == res["oracle_md5"]:
        res["status"] = "exact"
        if r["exit"] != 0:
            res["note"] = "exit {}: {}".format(r["exit"], first_error(text, logb))
    else:
        res["status"] = "differs"
        d = pdfdiff.diff(opdf, pdf)
        res["diff"] = d
        res.update(text_diff(opdf, pdf))
        res["note"] = (f"exit {r['exit']}: " if r["exit"] else "") + (d.get("summary") or f"bytes differ at {d.get('byte')}")
        if r["exit"] != 0:
            res["error"] = first_error(text, logb)
    res["term_tail"] = text[-2500:].decode("utf-8", "replace")
    if res["status"] != "exact" or r["exit"] != 0:
        keep(Path(args.out) / "files" / tag, [
            (term, "term.txt", 400_000),
            (run_dir / f"{job}.log", f"{job}.log", 400_000),
            (pdf, f"{job}.pdf", 8_000_000),
        ])
    shutil.rmtree(run_dir, ignore_errors=True)
    shutil.rmtree(cdir, ignore_errors=True)
    return res


def prewarm(args):
    """The format, built once into the shared format cache."""
    d = WORK / "fcache"
    (d / "formats").mkdir(parents=True, exist_ok=True)
    w = WORK / "run" / "prewarm"
    w.mkdir(parents=True, exist_ok=True)
    (w / "p.tex").write_text("\\documentclass{article}\n\\begin{document}\nx\n\\end{document}\n")
    env = manuals.side_env("pdflatex", {"PHITEX_CACHE_DIR": str(d)}, phitex=True)
    r = common.run([args.bin, "build", "--exact-pdf", "-j", "1", "--no-synctex", "p.tex"], w, env, 600, out=w / "term.txt")
    print(f"prewarm: exit {r['exit']} wall {r['wall_s']} s; formats: {sorted(os.listdir(d / 'formats'))}", flush=True)
    if r["exit"] != 0:
        print((w / "term.txt").read_text(errors="replace")[-3000:], flush=True)


def cmd_run(args):
    items = manuals.load_list(args.list)
    items = [c for c in items if c["engine"] == "pdflatex" and not (set(c.get("tools", [])) - {"bibtex", "makeindex"})]
    if args.ids:
        want = set(args.ids.split(","))
        items = [c for c in items if c["id"] in want]
    items = sorted(items, key=lambda c: (-(c.get("oracle_wall_s") or 0), c["id"]))
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    prewarm(args)
    par = dict((int(a), int(b)) for a, b in (x.split(":") for x in args.par.split(",")))
    print(f"{len(items)} documents, jobs {args.jobs}, parallel {par}, timeout {args.timeout} s", flush=True)
    for j in (int(x) for x in args.jobs.split(",")):
        todo = [c for c in items if not (out / f"{c['id']}-j{j}.json").exists()]
        with ProcessPoolExecutor(max_workers=par.get(j, 4)) as ex:
            fs = {ex.submit(run_one, c, j, args): c for c in todo}
            for f in as_completed(fs):
                c = fs[f]
                try:
                    res = f.result()
                except Exception as e:  # (the harness's own failure, kept as one)
                    res = {"id": c["id"], "path": c["path"], "j": j, "status": "harness", "note": repr(e)[:300]}
                common.write_json(out / f"{c['id']}-j{j}.json", res)
                print(f"j{j} {c['id']}: {res['status']} {str(res.get('note') or '')[:120]} {res.get('wall_s', '')}", flush=True)


def load(d):
    rs = {}
    for p in sorted(Path(d).glob("*-j*.json")):
        r = common.read_json(p)
        if r and "id" in r and "j" in r:
            rs.setdefault(r["id"], {})[r["j"]] = r
    return rs


def cmd_table(args):
    rs = load(args.dir)
    tot = {"docs": 0, "exact1": 0, "exact8": 0, "same18": 0, "exit0_1": 0, "exit0_8": 0, "rounds_le": 0, "rounds_n": 0}
    rows = []
    for i, by in sorted(rs.items()):
        a, b = by.get(1, {}), by.get(8, {})
        if a.get("status") == "no-oracle":
            continue
        tot["docs"] += 1
        tot["exact1"] += a.get("status") == "exact"
        tot["exact8"] += b.get("status") == "exact"
        tot["same18"] += bool(a.get("md5")) and a.get("md5") == b.get("md5")
        tot["exit0_1"] += a.get("exit") == 0
        tot["exit0_8"] += b.get("exit") == 0
        if a.get("rounds") and a.get("oracle_passes"):
            tot["rounds_n"] += 1
            tot["rounds_le"] += a["rounds"] <= a["oracle_passes"]
        rows.append((i, a, b))
    print(f"{'document':<50} {'st j1':<8} {'st j8':<8} {'1=8':>3} {'ex1':>3} {'pg/or':>9} {'rd/or':>5} {'w1':>6} {'w8':>6}  note")
    order = {"panic": 0, "error": 1, "timeout": 2, "harness": 3, "differs": 4, "exact": 5}
    for i, a, b in sorted(rows, key=lambda x: (order.get(x[1].get("status"), 9), x[0])):
        same = "y" if a.get("md5") and a.get("md5") == b.get("md5") else "n"
        print(f"{i[:50]:<50} {str(a.get('status')):<8} {str(b.get('status')):<8} {same:>3} {str(a.get('exit')):>3} "
              f"{str(a.get('pages')) + '/' + str(a.get('oracle_pages')):>9} {str(a.get('rounds')) + '/' + str(a.get('oracle_passes')):>5} "
              f"{str(a.get('wall_s')):>6} {str(b.get('wall_s')):>6}  {(a.get('note') or '')[:110]}")
    print(f"\n{tot['docs']} documents: exact j1 {tot['exact1']}, exact j8 {tot['exact8']}, md5 j1 == j8 {tot['same18']}, "
          f"exit 0 j1 {tot['exit0_1']}, exit 0 j8 {tot['exit0_8']}, rounds <= oracle passes {tot['rounds_le']}/{tot['rounds_n']}")
    if args.json:
        common.write_json(args.json, {"totals": tot, "docs": {i: {"j1": a, "j8": b} for i, a, b in rows}})


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("run")
    p.add_argument("--list", required=True)
    p.add_argument("--doc", required=True)
    p.add_argument("--cache", required=True)
    p.add_argument("--bin", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--ids")
    p.add_argument("--jobs", default="1,8")
    p.add_argument("--par", default="1:20,8:5")
    p.add_argument("--timeout", type=int, default=120)
    p = sub.add_parser("table")
    p.add_argument("dir")
    p.add_argument("--json")
    args = ap.parse_args()
    {"run": cmd_run, "table": cmd_table}[args.cmd](args)


if __name__ == "__main__":
    main()
