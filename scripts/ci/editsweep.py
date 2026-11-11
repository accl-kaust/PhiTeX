#!/usr/bin/env python3
"""The research corpus's edits: each document of ci/research.json (its
engine, its tools) under `phitex watch --json -j 1`, a word typed early,
midway and late in its main file, each edit saved and built in turn; then
the history's PDF against a cold build of the final text.

    editsweep.py --list ci/research.json --doc DIR --bin PHITEX --out DIR
                 [--shard K/N] [--engines pdflatex,xelatex] [--tools ...]
                 [--par 4] [--timeout 600]

Red (one line each on stdout, `editsweep: ...`; exit 1 if any):
- the history's PDF is not the cold build's;
- an edit's build never came, or the watch stopped;
- an edit ran more than twice a node a round. At one thread an edit's
  runs still go ahead of their inputs on the last build's predictions,
  so a node may run twice; the research templates' worst edits run 1.8
  times a node (revtex-aip, acm-sigconf-biblatex, mc 78030). Past that is
  a chain of finishers re-asked on the same field (the user's paper: 513
  finisher runs for 154 nodes, an edit of 60 s, a field flipping between
  the last pages' routines).
Each edit's wall, nodes run, runs and rounds go to OUT/<id>.json and the
table OUT/edits.txt, the times reported, not judged (a runner's clock).
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import manuals  # noqa: E402
import pdfdiff  # noqa: E402

WORK = manuals.WORK
WORD = re.compile(r"(?<=[ ])([a-z]{4,12})(?=[ .,;])")
PLACES = (0.15, 0.5, 0.85)


def md5(p):
    try:
        return hashlib.md5(Path(p).read_bytes()).hexdigest()
    except OSError:
        return None


def words(text):
    """(offset, word) of each plain word on a line that is text, not a command's."""
    out, off = [], 0
    for line in text.split("\n"):
        s = line.lstrip()
        if s and not s.startswith(("\\", "%")) and "\\verb" not in line and "{" not in line:
            out += [(off + m.start(1), m.group(1)) for m in WORD.finditer(line)]
        off += len(line) + 1
    return out


def body(text):
    """The words after \\begin{document} (an edit of the preamble is another test)."""
    at = text.find("\\begin{document}")
    return [w for w in words(text) if w[0] > at] if at >= 0 else []


def other(w):
    # (a word of the same length, never the same: its letters rotated by one)
    return "".join(chr((ord(c) - 97 + 1) % 26 + 97) for c in w)


def one(args, c):
    tag = f"{c['id']}"
    d = WORK / "edit" / tag
    common.fresh_copy(Path(args.doc) / c["dir"], d)
    cdir = WORK / "edit-cache" / tag
    shutil.rmtree(cdir, ignore_errors=True)
    cdir.mkdir(parents=True)
    os.symlink(WORK / "fcache" / "formats", cdir / "formats")
    env = manuals.side_env(c["engine"], {"PHITEX_CACHE_DIR": str(cdir), "NO_COLOR": "1"}, phitex=True)
    main, job = c["main"], Path(c["main"]).stem
    res = {"id": c["id"], "engine": c["engine"], "edits": [], "red": []}
    path = d / main
    text = path.read_text(encoding="latin-1")
    cand = body(text)
    if len(cand) < len(PLACES):
        res["skipped"] = f"{len(cand)} plain words in the body of {main}"
        return res
    picks = [cand[int(f * (len(cand) - 1))] for f in PLACES]
    err = open(d.parent / f"{tag}.err", "w")
    p = subprocess.Popen([args.bin, "watch", "--no-view", "--json", "--exact-pdf", "--no-synctex",
                          "--engine", c["engine"], "-j", "1", main], cwd=d, env=env,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)

    def build(timeout):
        t = time.monotonic()
        for line in p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") == "build":
                return x
            if time.monotonic() - t > timeout:
                break
        return None

    try:
        ev = build(args.timeout)
        if ev is None:
            res["red"].append("the watch's first build never came")
        # (the last place first: each edit keeps the offsets before it)
        for at, w in sorted(picks, reverse=True):
            if ev is None:
                break
            text = text[:at] + other(w) + text[at + len(w):]
            line = text.count("\n", 0, at) + 1
            tmp = path.with_suffix(".tmp")
            tmp.write_text(text, encoding="latin-1")
            os.replace(tmp, path)
            t0 = time.monotonic()
            ev = build(args.timeout)
            e = {"line": line, "wall_ms": round((time.monotonic() - t0) * 1e3, 1)}
            if ev is None:
                res["red"].append(f"line {line}: the edit's build never came")
            else:
                e.update(nodes=ev.get("nodes_run") or 0, runs=ev.get("runs") or 0,
                         rounds=ev.get("rounds") or 0, engine_ms=ev.get("wall_ms"), ok=ev.get("ok"))
                if e["runs"] > 2 * max(e["nodes"], 1) * max(e["rounds"], 1):
                    res["red"].append(f"line {line}: {e['runs']} runs for {e['nodes']} nodes in "
                                      f"{e['rounds']} rounds ({e['wall_ms']:.0f} ms)")
            res["edits"].append(e)
    finally:
        try:
            p.stdin.write(b"q\n")
            p.stdin.close()
        except OSError:
            pass
        try:
            p.wait(timeout=60)
        except subprocess.TimeoutExpired:
            p.kill()
        err.close()
    hist = md5(d / f"{job}.pdf")
    # (the final text built cold, in a directory and cache of its own: the
    # document's own files as they came, the main file's final text. Not
    # the watch's directory with its outputs deleted by their extensions:
    # that deleted the documents' own PDF figures too (apa7's Figure1.pdf,
    # asmeconf's sample-figure-*.pdf, acmart's sampleteaser.pdf), and the
    # cold build drew draft boxes where the history had the figures)
    cold = WORK / "edit-cold" / tag
    common.fresh_copy(Path(args.doc) / c["dir"], cold)
    (cold / main).write_text(text, encoding="latin-1")
    ccache = WORK / "edit-cache" / f"{tag}-cold"
    shutil.rmtree(ccache, ignore_errors=True)
    ccache.mkdir(parents=True)
    os.symlink(WORK / "fcache" / "formats", ccache / "formats")
    cenv = manuals.side_env(c["engine"], {"PHITEX_CACHE_DIR": str(ccache)}, phitex=True)
    common.run([args.bin, "build", "--exact-pdf", "--no-synctex", "--engine", c["engine"], "-j", "1", main],
               cold, cenv, args.timeout, out=cold / "term.txt")
    res["history_md5"], res["cold_md5"] = hist, md5(cold / f"{job}.pdf")
    if len(res["edits"]) == len(PLACES) and hist != res["cold_md5"]:
        # (both PDFs kept, with where they first differ, and the files
        # each build read back)
        keep = Path(args.out) / "files" / tag
        keep.mkdir(parents=True, exist_ok=True)
        for src, name in ((d, "history"), (cold, "cold")):
            for x in ("pdf", "aux", "log", "bbl", "toc", "out"):
                if (src / f"{job}.{x}").exists():
                    shutil.copy(src / f"{job}.{x}", keep / f"{name}.{x}")
        (WORK / "tmp").mkdir(parents=True, exist_ok=True)
        where = pdfdiff.diff(d / f"{job}.pdf", cold / f"{job}.pdf", WORK / "tmp") if hist and res["cold_md5"] else {}
        res["diff"] = where
        res["red"].append(f"the history's PDF is not the cold build's: "
                          f"{where.get('summary') or where.get('byte') or 'one is missing'}")
    for x in (d, cold, cdir, ccache):
        shutil.rmtree(x, ignore_errors=True)
    return res


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--list", required=True)
    ap.add_argument("--doc", required=True)
    ap.add_argument("--bin", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--shard")
    ap.add_argument("--ids")
    ap.add_argument("--engines", default="pdflatex,xelatex")
    ap.add_argument("--tools", default="bibtex,biber,makeindex,glossaries")
    ap.add_argument("--par", type=int, default=4)
    ap.add_argument("--timeout", type=int, default=600)
    args = ap.parse_args()
    items = manuals.shard(manuals.load_list(args.list), args.shard)
    engines, tools = set(args.engines.split(",")), set(args.tools.split(","))
    items = [c for c in items if c["engine"] in engines and not (set(c.get("tools", [])) - tools)]
    if args.ids:
        items = [c for c in items if c["id"] in set(args.ids.split(","))]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    # (each engine's format once, shared: sweep.py's prewarm)
    import sweep  # noqa: E402
    sweep.prewarm(argparse.Namespace(shelf=False, bin=args.bin), sorted({c["engine"] for c in items}))
    print(f"{len(items)} documents, {args.par} at once", flush=True)
    rows, reds = [], []
    with ProcessPoolExecutor(args.par) as ex:
        for f in as_completed([ex.submit(one, args, c) for c in items]):
            r = f.result()
            (out / f"{r['id']}.json").write_text(json.dumps(r, indent=1))
            es = " ".join(f"L{e['line']}:{e['wall_ms']:.0f}ms/{e.get('nodes')}n/{e.get('runs')}r/{e.get('rounds')}q"
                          for e in r["edits"])
            rows.append(f"{r['id']:<34} {r['engine']:<9} {r.get('skipped') or es}")
            reds += [f"editsweep: {r['id']} ({r['engine']}): {x}" for x in r["red"]]
    (out / "edits.txt").write_text("\n".join(sorted(rows)) + "\n")
    print("\n".join(sorted(rows)), flush=True)
    for x in reds:
        print(x, flush=True)
    sys.exit(1 if reds else 0)


if __name__ == "__main__":
    main()
