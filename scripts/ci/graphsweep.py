#!/usr/bin/env python3
"""The persisted graph across processes (docs/spec-persisted-graph.md §5,
step 3): each document of ci/research.json built by `phitex build` with
PHITEX_GRAPH=1, one process after another in one directory and cache, the
graph each saved loaded by the next:

1. a clean build, which saves the graph;
2. unchanged: loaded, no node run, the same bytes;
3. a word typed early, midway and late in the main file, each in a
   process of its own;
4. the inputs changed under the graph: a .bib entry's title edited, an
   image replaced by another, the document class shadowed by a copy in
   the document's folder (it says so in the log);
5. the other thread count's process: -j 8 after -j 1, -j 1 after -j 8,
   unchanged (the graph is not keyed by it);
6. the graph file cut, then a bit of it flipped: refused, a clean build;
7. SOURCE_DATE_EPOCH changed (a variable that changes the output):
   refused by its key, a clean build.

Each process's PDF, log, aux and SyncTeX file must be byte-identical to a
clean build (no graph) of the same files in the same directory (the
outputs moved aside, a fresh copy of the document with the same changes
built there, then the history's put back): after 1, 2 and 5 the previous
step's outputs are that. A process that should load and did not, one
that loaded what it should not have, and an unchanged load that ran a
node are red too.

    graphsweep.py --list ci/research.json --doc DIR --bin PHITEX --out DIR
                  [--shard K/N] [--ids a,b] [--par 4] [--timeout 600]
                  [--threads 1] [--extra=--shelf]

Red: one line each on stdout, `graphsweep: ...`; exit 1 if any.
"""

import argparse
import fcntl
import hashlib
import json
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import time
import zlib
from concurrent.futures import ProcessPoolExecutor, as_completed
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import editsweep  # noqa: E402
import manuals  # noqa: E402
import shards  # noqa: E402

WORK = manuals.WORK
OUTS = ("pdf", "log", "aux", "synctex.gz")
IMAGES = (".png", ".jpg", ".jpeg", ".pdf")


def md5(p):
    try:
        return hashlib.md5(Path(p).read_bytes()).hexdigest()[:12]
    except OSError:
        return None


def png(seed):
    """A small PNG of its own: 2x2 pixels, colours from `seed`."""
    rows = b"".join(b"\0" + bytes((seed * 37 + i * 91) % 256 for i in range(6)) for _ in range(2))

    def chunk(t, d):
        return struct.pack(">I", len(d)) + t + d + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)

    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 2, 2, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))


class Doc:
    """One document's history: its directory, cache, the changes made to
    its files (relative path -> bytes), and the processes' records."""

    def __init__(self, args, c):
        self.args, self.c = args, c
        self.tag = f"{c['id']}-j{args.threads}"
        self.src = Path(args.doc) / c["dir"]
        self.d = WORK / "graph" / self.tag / "doc"
        common.fresh_copy(self.src, self.d)
        self.cache = WORK / "graph" / self.tag / "cache"
        shutil.rmtree(self.cache, ignore_errors=True)
        self.cache.mkdir(parents=True)
        os.symlink(WORK / "fcache" / "formats", self.cache / "formats")
        self.main, self.job = c["main"], Path(c["main"]).stem
        self.changes = {}
        self.log = open(WORK / "graph" / self.tag / "steps.txt", "w")
        self.red, self.steps = [], []
        self.sde = None

    def env(self, graph, cache):
        e = manuals.side_env(self.c["engine"], {"PHITEX_CACHE_DIR": str(cache), "NO_COLOR": "1"}, phitex=True)
        if graph:
            e["PHITEX_GRAPH"] = "1"
        if self.sde:
            e["SOURCE_DATE_EPOCH"] = self.sde
        return e

    def run(self, where, graph, cache, threads=None):
        """One `phitex build` process in `where`: its graph lines, the nodes
        it ran, its exit and wall."""
        out = where.parent / f"{where.name}.term"
        cmd = [self.args.bin, "build", "--json", "-v", "--exact-pdf", "--synctex", "--engine", self.c["engine"],
               "-j", str(threads or self.args.threads), *self.args.extra, self.main]
        t = time.monotonic()
        r = common.run(cmd, where, self.env(graph, cache), self.args.timeout, out=out)
        wall = time.monotonic() - t
        graph_lines, nodes = [], None
        for line in out.read_text(errors="replace").splitlines():
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") == "graph":
                graph_lines.append(x.get("message", ""))
            elif x.get("event") == "build":
                nodes = x.get("nodes_run")
        return graph_lines, nodes, r["exit"], wall

    def sums(self, where):
        return {x: md5(where / f"{self.job}.{x}") for x in OUTS}

    def write(self, rel, data):
        self.changes[rel] = data
        p = self.d / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        tmp = p.with_name(p.name + ".graphsweep-tmp")
        tmp.write_bytes(data)
        os.replace(tmp, p)

    def clean(self):
        """A clean build (no graph, a cache of its own) of the files as they
        are now, in the history's directory: the history moved aside
        meanwhile. Its outputs' sums."""
        keep = self.d.with_name("doc.history")
        os.rename(self.d, keep)
        try:
            common.fresh_copy(self.src, self.d)
            for rel, data in self.changes.items():
                p = self.d / rel
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(data)
            cc = WORK / "graph" / self.tag / "clean-cache"
            shutil.rmtree(cc, ignore_errors=True)
            cc.mkdir(parents=True)
            os.symlink(WORK / "fcache" / "formats", cc / "formats")
            _, _, code, wall = self.run(self.d, False, cc)
            sums = self.sums(self.d)
            self.keep_outputs(self.d, "clean")
            shutil.rmtree(cc, ignore_errors=True)
            return sums, code
        finally:
            shutil.rmtree(self.d, ignore_errors=True)
            os.rename(keep, self.d)

    def saved(self):
        """The last save done (a build's worker saves after its process has
        ended): its lock taken and let go."""
        for lk in (self.cache / "docs").rglob("graph.lock"):
            with open(lk) as f:
                fcntl.flock(f, fcntl.LOCK_EX)

    def keep_outputs(self, where, name):
        """The outputs of a build in `where` kept as `name` (the last
        step's, for a difference found)."""
        k = WORK / "graph" / self.tag / "last" / name
        shutil.rmtree(k, ignore_errors=True)
        k.mkdir(parents=True)
        for x in (*OUTS, "bbl", "blg", "toc", "out"):
            if (where / f"{self.job}.{x}").exists():
                shutil.copy(where / f"{self.job}.{x}", k / f"{self.job}.{x}")

    def clean_of(self, before):
        """A clean build (no graph) of folder `before` (the history's as it
        was before a step), in the history's directory."""
        keep = self.d.with_name("doc.history")
        os.rename(self.d, keep)
        try:
            os.rename(before, self.d)
            cc = WORK / "graph" / self.tag / "clean-cache"
            shutil.rmtree(cc, ignore_errors=True)
            cc.mkdir(parents=True)
            os.symlink(WORK / "fcache" / "formats", cc / "formats")
            _, _, code, _ = self.run(self.d, False, cc)
            sums = self.sums(self.d)
            self.keep_outputs(self.d, "clean")
            shutil.rmtree(cc, ignore_errors=True)
            return sums, code
        finally:
            shutil.rmtree(self.d, ignore_errors=True)
            os.rename(keep, self.d)

    def step(self, what, load, ref=None, threads=None, nodes0=False):
        """A process of the history; `load`: whether it must load the graph
        (True), must not (False). Its outputs against `ref` (none: a clean
        build of the files now)."""
        if ref is None and load is False:
            # (refused, a clean build where the history's outputs are: its
            # reference is one of that folder)
            before = WORK / "graph" / self.tag / "before"
            shutil.rmtree(before, ignore_errors=True)
            shutil.copytree(self.d, before, symlinks=True)
        lines, nodes, code, wall = self.run(self.d, True, self.cache, threads)
        sums = self.sums(self.d)
        loaded = any(x.startswith("graph: loaded") for x in lines)
        if ref is None and load is False:
            ref, rcode = self.clean_of(before)
        elif ref is None:
            ref, rcode = self.clean()
        else:
            rcode = None
        same = sums == ref
        self.steps.append({"step": what, "loaded": loaded, "nodes": nodes, "exit": code, "wall_s": round(wall, 3),
                           "graph": lines, "same": same})
        print(f"{what}: exit {code}, {wall:.2f} s, nodes {nodes}, {'same' if same else 'DIFFER'}; "
              + " | ".join(lines), file=self.log, flush=True)
        if not same:
            # (both builds' outputs kept, with the history's terminal)
            self.keep_outputs(self.d, "history")
            keep = Path(self.args.out) / "files" / self.tag / f"{len(self.steps):02d}"
            shutil.rmtree(keep, ignore_errors=True)
            shutil.copytree(WORK / "graph" / self.tag / "last", keep)
            term = self.d.parent / f"{self.d.name}.term"
            if term.exists():
                shutil.copy(term, keep / "history.term")
            diff = [x for x in OUTS if sums.get(x) != ref.get(x)]
            self.red.append(f"{what}: {', '.join(diff)} not the clean build's ({sums} / {ref}, clean exit {rcode})")
        if load is True and not loaded:
            self.red.append(f"{what}: the graph not loaded: {' | '.join(lines) or 'no graph line'}")
        if load is False and loaded:
            self.red.append(f"{what}: the graph loaded where it must not: {' | '.join(lines)}")
        if nodes0 and nodes != 0:
            self.red.append(f"{what}: {nodes} nodes run with nothing changed")
        return sums


def bib_edit(d):
    """A .bib of the document's whose first entry has a title: (path,
    the bytes with a letter put first in the title)."""
    for p in sorted(d.rglob("*.bib")):
        t = p.read_bytes()
        m = re.search(rb"(?i)\btitle\s*=\s*[{\"]", t)
        if m:
            return p, t[:m.end()] + b"Q" + t[m.end():]
    return None


def image_swap(d, job):
    """An image the build read (named in its log): (path, other bytes):
    another image of the same kind of the document's, else a PNG made here."""
    log = (d / f"{job}.log").read_text(errors="replace") if (d / f"{job}.log").exists() else ""
    for p in sorted(d.rglob("*")):
        if p.suffix.lower() not in IMAGES or p.stem == job or not p.is_file():
            continue
        if p.name not in log:
            continue
        others = [q for q in sorted(d.rglob(f"*{p.suffix}")) if q != p and q.is_file() and q.stem != job
                  and q.read_bytes() != p.read_bytes()]
        if others:
            return p, others[0].read_bytes()
        if p.suffix.lower() == ".png":
            return p, png(len(p.name))
    return None


def shadow(d, main):
    """The document class, found by kpathsea in the distribution and not
    in the document's folder: (its name, a copy saying so in the log)."""
    t = (d / main).read_text(encoding="latin-1")
    m = re.search(r"\\documentclass(?:\[[^\]]*\])?\{([^}]+)\}", t)
    if not m:
        return None
    name = m.group(1).strip() + ".cls"
    if (d / name).exists():
        return None
    try:
        p = subprocess.run(["kpsewhich", name], capture_output=True, text=True, timeout=60).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None
    if not p:
        return None
    return name, b"\\AtBeginDocument{\\typeout{PhiTeX graph: this class is the document's own}}%\n" + Path(p).read_bytes()


def one(args, c):
    h = Doc(args, c)
    res = {"id": c["id"], "engine": c["engine"], "threads": args.threads}
    try:
        s0 = h.step("clean, saved", False, ref=None)
        s1 = h.step("unchanged", True, ref=s0, nodes0=True)
        # (words typed, the last place first, as editsweep's)
        # (the main file's body's words; too few, the files its body reads
        # too, then the words before a line's first command: editsweep's)
        path = h.d / h.main
        texts = {path: path.read_text(encoding="latin-1")}
        cand = [(path, at, w) for at, w in editsweep.body(texts[path])]
        if len(cand) < len(editsweep.PLACES):
            read = editsweep.inputs(h.d, texts[path], {path})
            for f, t in read:
                texts[f] = t
                cand += [(f, at, w) for at, w in (editsweep.body(t) if "\\begin{document}" in t else editsweep.words(t))]
            if len(cand) < len(editsweep.PLACES):
                cand = [(path, at, w) for at, w in editsweep.body(texts[path], editsweep.loose)]
        picks = cand if len(cand) <= 3 else list(dict.fromkeys(cand[int(f * (len(cand) - 1))] for f in editsweep.PLACES))
        for f, at, w in sorted(picks, key=lambda x: (str(x[0]), x[1]), reverse=True):
            t = texts[f]
            t = t[:at] + editsweep.other(w) + t[at + len(w):]
            texts[f] = t
            h.write(str(f.relative_to(h.d)), t.encode("latin-1"))
            line = t.count("\n", 0, at) + 1
            s1 = h.step(f"word typed at {f.relative_to(h.d)}:{line}", True)
        b = bib_edit(h.d)
        if b:
            h.write(str(b[0].relative_to(h.d)), b[1])
            s1 = h.step(f"{b[0].name} edited", True)
        im = image_swap(h.d, h.job)
        if im:
            h.write(str(im[0].relative_to(h.d)), im[1])
            s1 = h.step(f"{im[0].name} replaced", True)
        sh = shadow(h.d, h.main)
        if sh:
            h.write(sh[0], sh[1])
            s1 = h.step(f"{sh[0]} shadowed by the document's copy", True)
        other = 8 if args.threads == 1 else 1
        s1 = h.step(f"unchanged at -j {other}", True, ref=s1, threads=other, nodes0=True)
        s1 = h.step("unchanged again", True, ref=s1, nodes0=True)
        h.saved()
        g = next((h.cache / "docs").rglob("graph"), None)
        if g is None:
            h.red.append("no graph file in the cache")
        else:
            rnd = random.Random(c["id"])
            data = g.read_bytes()
            g.write_bytes(data[:rnd.randrange(1, len(data))])
            s1 = h.step("the graph file cut", False, ref=s1)
            h.saved()
            data = g.read_bytes()
            i = rnd.randrange(len(data))
            g.write_bytes(data[:i] + bytes([data[i] ^ (1 << rnd.randrange(8))]) + data[i + 1:])
            s1 = h.step("a bit of the graph file flipped", False, ref=s1)
        h.sde = str(int(common.PINNED["SOURCE_DATE_EPOCH"]) + 86400 * 400 + 3723)
        h.step("SOURCE_DATE_EPOCH changed", False)
    except Exception as e:  # noqa: BLE001 (a step that could not run is red, named)
        h.red.append(f"the history stopped: {type(e).__name__}: {e}")
    res["steps"], res["red"] = h.steps, h.red
    h.log.close()
    if not h.red:
        shutil.rmtree(WORK / "graph" / h.tag, ignore_errors=True)
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
    ap.add_argument("--threads", type=int, default=1)
    ap.add_argument("--extra", action="append", default=[])
    args = ap.parse_args()
    items = shards.docs("edits", manuals.load_list(args.list), args.shard)
    engines, tools = set(args.engines.split(",")), set(args.tools.split(","))
    items = [c for c in items if c["engine"] in engines and not (set(c.get("tools", [])) - tools)]
    if args.ids:
        items = [c for c in items if c["id"] in set(args.ids.split(","))]
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    import sweep  # noqa: E402
    sweep.prewarm(argparse.Namespace(shelf="--shelf" in args.extra, bin=args.bin), sorted({c["engine"] for c in items}))
    print(f"{len(items)} documents, {args.par} at once, -j {args.threads}", flush=True)
    rows, reds = [], []
    with ProcessPoolExecutor(args.par) as ex:
        for f in as_completed([ex.submit(one, args, c) for c in items]):
            r = f.result()
            (out / f"{r['id']}.json").write_text(json.dumps(r, indent=1))
            loads = sum(1 for s in r["steps"] if s["loaded"])
            walls = " ".join(f"{s['wall_s']:.1f}" for s in r["steps"])
            rows.append(f"{r['id']:<34} {r['engine']:<9} {len(r['steps'])} steps, {loads} loaded, walls {walls}")
            reds += [f"graphsweep: {r['id']} ({r['engine']}, -j {r['threads']}): {x}" for x in r["red"]]
    (out / "graphsweep.txt").write_text("\n".join(sorted(rows)) + "\n")
    print("\n".join(sorted(rows)), flush=True)
    for x in reds:
        print(x, flush=True)
    sys.exit(1 if reds else 0)


if __name__ == "__main__":
    main()
