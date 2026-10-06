#!/usr/bin/env python3
"""The measured runs of the CI (docs/ci.md, "Performance"): a fixed set of
documents, each built REPS times on one node, the runs of a repetition
alternating between TeX Live and partex's modes; medians, ranges and peak
RSS, with TeX Live's own times on the same node as the yardstick.

Per document and repetition:
  texlive.cold     TeX Live's engine from nothing to its fixpoint (every
                   pass and every tool run counted, latexmk's loop)
  plain.cold       partex as the engine, in the same loop
  build.cold       `partex build --no-machine` (pdflatex documents)
  machine.cold     `partex build` (machine mode, an empty store), then
  machine.warm     `partex build` again, nothing changed
  ssa.cold         one `PARTEX_SSA=1` process to its fixpoint (wall), and
  ssa.edit         in it, a word edited and rebuilt to the fixpoint (the
                   process's own `ssa rebuild 1: X ms`)
  texlive.pass     one TeX Live pass over the settled files
  plain.pass       one partex pass over the same
  texlive.edit     the same word edit, TeX Live from the settled files to
                   its new fixpoint (what an editor waits for today)
Each with its peak RSS (`*_rss_kib`). Then ssa-edits' cases (one job at a
time), their rebuild medians.

    perf.py --src DIR --bin PARTEX --formats DIR --doc DOCTREE --out DIR
            [--reps N] [--only ID,...]
"""

import argparse
import os
import re
import shutil
import socket
import statistics
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import manuals  # noqa: E402

W = manuals.WORK
# (id, directory: under the doc tree, or absolute; main file; engine;
# the edited file: its first prose line after \begin{document} that has
# " the " gets "the" -> "this"; repetitions)
DOCS = [
    ("amsldoc", "latex/amsmath", "amsldoc.tex", "pdflatex", "amsldoc.tex", 5),
    ("usrguide", "latex/base", "usrguide.tex", "pdflatex", "usrguide.tex", 5),
    ("beameruserguide", "latex/beamer", "beameruserguide.tex", "pdflatex", "beamerug-introduction.tex", 3),
    ("memman", "latex/memoir", "memman.tex", "pdflatex", "memman.tex", 3),
    ("pgfmanual", "generic/pgf", "pgfmanual.tex", "pdflatex", "pgfmanual-en-tutorial.tex", 3),
    ("polyglossia", "latex/polyglossia", "polyglossia.tex", "xelatex", "polyglossia.tex", 3),
    ("course", "/w/course", "course.tex", "pdflatex", "ch15.tex", 3),
]
COLD = re.compile(rb"partex: ssa build 0: ([\d.]+) ms, calls")
REBUILD = re.compile(rb"partex: ssa rebuild 1: ([\d.]+) ms \(the rebuild")


def edit_line(path):
    """(old, new) for the word edit of `path`, or None."""
    try:
        text = Path(path).read_text(errors="replace")
    except OSError:
        return None
    start = text.find("\\begin{document}")
    for line in text[max(start, 0):].splitlines():
        s = line.strip()
        if " the " in s and not s.startswith(("%", "\\")) and "'" not in s and len(s) > 30:
            return s, s.replace(" the ", " this ", 1)
    return None


def apply_edit(path, old, new):
    p = Path(path)
    text = p.read_text(errors="surrogateescape")
    tmp = p.with_suffix(p.suffix + ".new")
    tmp.write_text(text.replace(old, new, 1), errors="surrogateescape")
    st = p.stat()
    tmp.replace(p)
    # (a new modification time, as an editor's save gives)
    os.utime(p, (st.st_atime, max(st.st_mtime, time.time())))


def fresh(src, name):
    d = W / "perf" / name
    common.fresh_copy(src, d)
    for x in ("cache", "store"):
        shutil.rmtree(W / x, ignore_errors=True)
    return d


def cold_loop(d, cmd, tools, environ, job):
    base = {rel: common.sha256_file(d / rel) for rel in common.tree_files(d)}
    ist = {k for k in base if k.endswith(".ist")}
    return manuals.loop(d, cmd, tools, environ, job, base, ist, []), base


def one_rep(doc, src, binary, formats, rec):
    did, _, main, engine, edit_file, _ = doc
    job = Path(main).stem
    tl_env = manuals.side_env(engine)
    px_env = manuals.side_env(engine, partex=True, formats=formats)

    def put(k, v):
        if v is not None:
            rec.setdefault(k, []).append(v)

    # TeX Live, cold, to its fixpoint; its settled tree kept for the pass
    # and edit runs
    d = fresh(src, "texlive")
    r, base = cold_loop(d, manuals.oracle_cmd(engine, main), manuals.Tools(), tl_env, job)
    put("texlive.cold_s", r["wall_s"])
    put("texlive.cold_rss_kib", r["rss_kib"])
    put("texlive.passes", r["passes"])
    settled = W / "perf" / "settled"
    shutil.rmtree(settled, ignore_errors=True)
    shutil.copytree(d, settled)
    ref_pdf = d / f"{job}.pdf"

    def same_pdf(dd):
        try:
            return (dd / f"{job}.pdf").read_bytes() == ref_pdf.read_bytes()
        except OSError:
            return False

    # partex plain, the same loop
    d = fresh(src, "plain")
    r, _ = cold_loop(d, manuals.partex_compat(binary, engine, main), manuals.Tools(binary), px_env, job)
    put("plain.cold_s", r["wall_s"])
    put("plain.cold_rss_kib", r["rss_kib"])
    put("plain.identical", same_pdf(d))
    if engine == "pdflatex":
        for mode, extra, flags in (("build", {"PARTEX_MACHINE": "0"}, ["--no-machine"]), ("machine", {}, [])):
            d = fresh(src, mode)
            cmd = [binary, "build", *flags, "--no-shell-escape", "--color", "never", main]
            x = common.run(cmd, d, manuals.side_env(engine, extra, partex=True, formats=formats),
                           manuals.MODE_TIMEOUT, out=d / "term-mode.txt")
            if not x["timeout"]:
                put(f"{mode}.cold_s", x["wall_s"])
                put(f"{mode}.cold_rss_kib", x["rss_kib"])
                put(f"{mode}.identical", same_pdf(d))
            if mode == "machine":
                x = common.run(cmd, d, manuals.side_env(engine, extra, partex=True, formats=formats),
                               manuals.MODE_TIMEOUT, out=d / "term-warm.txt")
                if not x["timeout"]:
                    put("machine.warm_s", x["wall_s"])
                    put("machine.warm_rss_kib", x["rss_kib"])
    # SSA: cold, then the word edit rebuilt in the same process
    d = fresh(src, "ssa")
    ed = edit_line(d / edit_file)
    extra = {"PARTEX_SSA": "1"}
    if ed:
        script = W / "perf" / "edit.py"
        script.write_text(
            "import sys\nfrom pathlib import Path\n"
            f"p = Path({str(d / edit_file)!r})\n"
            f"t = p.read_text(errors='surrogateescape')\n"
            f"p.write_text(t.replace({ed[0]!r}, {ed[1]!r}, 1), errors='surrogateescape')\n")
        extra["PARTEX_SSA_REBUILD"] = f"python3 {script}"
    x = common.run(manuals.partex_compat(binary, engine, main), d,
                   manuals.side_env(engine, extra, partex=True, formats=formats), manuals.MODE_TIMEOUT,
                   out=d / "term-ssa.txt")
    err = (d / "term-ssa.txt").read_bytes()
    m = COLD.search(err)
    put("ssa.cold_ms", float(m.group(1)) if m else None)
    put("ssa.wall_s", None if x["timeout"] else x["wall_s"])
    put("ssa.rss_kib", x["rss_kib"])
    m = REBUILD.search(err)
    put("ssa.edit_ms", float(m.group(1)) if m else None)
    # one pass over the settled files, each side
    for side, cmd, environ in (("texlive", manuals.oracle_cmd(engine, main), tl_env),
                               ("plain", manuals.partex_compat(binary, engine, main), px_env)):
        d = fresh(settled, f"pass-{side}")
        x = common.run(cmd, d, environ, manuals.PASS_TIMEOUT, out=d / "term-pass.txt")
        put(f"{side}.pass_s", x["wall_s"])
        put(f"{side}.pass_rss_kib", x["rss_kib"])
    # TeX Live after the word edit, from the settled files to its fixpoint
    if ed:
        d = fresh(settled, "edit-texlive")
        apply_edit(d / edit_file, *ed)
        r, _ = cold_loop(d, manuals.oracle_cmd(engine, main), manuals.Tools(), tl_env, job)
        put("texlive.edit_s", r["wall_s"])
        put("texlive.edit_passes", r["passes"])
    rec["edit"] = [ed[0][:80]] if ed else []


def summarize(rec):
    out = {}
    for k, vs in rec.items():
        nums = [v for v in vs if isinstance(v, (int, float)) and not isinstance(v, bool)]
        if k == "edit":
            out[k] = vs[0] if vs else None
        elif nums and len(nums) == len(vs):
            out[k] = {"median": statistics.median(nums), "min": min(nums), "max": max(nums), "values": nums}
        else:
            out[k] = {"values": vs, "all": all(vs) if all(isinstance(v, bool) for v in vs) else None}
    return out


def ssa_edits(src, out, binary):
    """ssa-edits' cases, one at a time: each case's rebuild times."""
    work = W / "perf" / "ssa-edits"
    common.run(["python3", "scripts/ssa-edits", "--brief", "--jobs", "1", "--bin", str(binary),
                "--work", str(work)], src, dict(os.environ), 7200, out=out / "ssa-edits.log")
    data = common.read_json(work / "results.json", {})
    cases = {}
    for c in data.get("cases", []):
        ms = [s["ms"] for s in c.get("stages", [])[1:] if isinstance(s.get("ms"), (int, float))]
        if ms:
            cases[c["case"]] = {"median_ms": statistics.median(ms), "rebuilds": len(ms), "ok": c.get("ok")}
    if cases:
        all_ms = [v["median_ms"] for v in cases.values()]
        cases["(all cases)"] = {"median_ms": statistics.median(all_ms), "rebuilds": sum(v["rebuilds"] for v in cases.values())}
    return cases


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", required=True)
    ap.add_argument("--bin", required=True)
    ap.add_argument("--formats", required=True)
    ap.add_argument("--doc", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--reps", type=int)
    ap.add_argument("--only")
    ap.add_argument("--no-ssa-edits", action="store_true")
    a = ap.parse_args()
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    result = {"node": socket.gethostname(), "cpus": len(os.sched_getaffinity(0)), "docs": {},
              "load": [os.getloadavg()[0]]}
    for doc in DOCS:
        did, dsrc, *_rest, reps = doc
        if a.only and did not in a.only.split(","):
            continue
        src = Path(dsrc) if dsrc.startswith("/") else Path(a.doc) / dsrc
        if not (src / doc[2]).exists():
            print(f"{did}: not here, skipped", flush=True)
            continue
        rec = {}
        t0 = time.monotonic()
        for i in range(a.reps or reps):
            one_rep(doc, src, a.bin, a.formats, rec)
            print(f"{did}: rep {i + 1} done ({time.monotonic() - t0:.0f} s)", flush=True)
        result["docs"][did] = {"engine": doc[3], "input_hash": None if did == "course" else common.tree_hash(src),
                               **summarize(rec)}
        common.write_json(out / "perf.json", result)
    result["load"].append(os.getloadavg()[0])
    if not a.no_ssa_edits:
        result["ssa_edits"] = ssa_edits(Path(a.src), out, a.bin)
    common.write_json(out / "perf.json", result)


if __name__ == "__main__":
    main()
