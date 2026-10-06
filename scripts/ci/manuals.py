#!/usr/bin/env python3
"""The manual corpus (docs/ci.md): TeX Live's own documentation sources,
built by TeX Live's engine to a settled fixpoint (the oracle) and by
partex in each of its modes, every output compared byte for byte (the PDF
and the files the job reads back; not the .log).

    manuals.py discover --doc DIR --out CANDIDATES.json
    manuals.py oracle   --list FILE --doc DIR --cache DIR [--shard I/N]
    manuals.py run      --list FILE --doc DIR --cache DIR --bin PARTEX
                        --formats DIR --out DIR [--modes M,...] [--shard I/N]
    manuals.py pin      --list CANDIDATES.json --cache DIR --out ci/manuals.json

`discover` picks the candidates (one main document per package directory,
and the manuals named in FORCED); `oracle` builds each with TeX Live and
keeps the outputs in the cache, keyed by the input tree's hash; `pin`
turns the oracle's results into the pinned list (the inputs' hashes, the
engine, the tools each needs) and the excluded list (with the reason);
`run` checks the pinned hashes, takes the oracle's outputs from the cache
(building them if missing), and runs partex's modes:

  plain    partex as TeX Live's engine (`--compat=tex -engine=…`), in the
           same latexmk-like loop as the oracle, BibTeX and makeindex
           as `partex -bibtex` / `partex -makeindex`, biber TeX Live's
  build    `partex build --no-machine` (one process to the fixpoint,
           BibTeX and makeindex in process): pdflatex documents whose
           tools are BibTeX and plain makeindex
  machine  `partex build` (machine mode, its store in the scratch dir)
  ssa      `PARTEX_SSA=1 partex --compat=tex …`: one SSA process, in
           trips to its fixpoint (DESIGN 3.7), the tools in process

All in one fixed directory (/w/run/<id>, the container's bind of the
job's scratch), so absolute paths are the same on both sides.
"""

import argparse
import json
import os
import re
import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import common  # noqa: E402
import pdfdiff  # noqa: E402

HARNESS_VERSION = "1"
MAX_PASSES = 8
PASS_TIMEOUT = 1200
MODE_TIMEOUT = 5400
WORK = Path(os.environ.get("CI_WORK", "/w"))

# Manuals the corpus must have, by path under the doc tree (the rest is
# one main document per package directory).
FORCED = [
    "latex/base/clsguide.tex", "latex/base/fntguide.tex", "latex/base/usrguide.tex",
    "latex/base/encguide.tex", "latex/base/cfgguide.tex", "latex/base/cyrguide.tex",
    "latex/base/source2e.tex", "latex/base/lthooks-doc.tex", "latex/base/ltmarks-doc.tex",
    "latex/base/ltfilehook-doc.tex", "latex/base/ltnews.tex",
    "latex/l3kernel/interface3.tex", "latex/l3kernel/source3.tex",
    "latex/l3kernel/l3styleguide.tex", "latex/l3kernel/l3syntax-changes.tex",
    "generic/pgf/pgfmanual.tex", "latex/pgfplots/pgfplotsexample.tex",
    "latex/beamer/beameruserguide.tex", "latex/amsmath/amsldoc.tex",
    "latex/amsmath/testmath.tex", "latex/amsmath/technote.tex",
    "latex/hyperref/hyperref-doc.tex", "latex/biblatex/biblatex.tex",
    "latex/biblatex/examples/01-introduction.tex",
    "latex/biblatex/examples/03-localization-keys.tex",
    "latex/biblatex/examples/10-references-per-section.tex",
    "latex/biblatex/examples/20-indexing-single.tex",
    "latex/biblatex/examples/30-style-numeric-bibtex.tex",
    "latex/biblatex/examples/40-style-authoryear.tex",
    "latex/biblatex/examples/70-style-verbose.tex",
    "latex/memoir/memman.tex",
    "latex/koma-script/examples/letter-example-00-en.tex",
    "latex/koma-script/examples/book-remarkbox-patch-en.tex",
    "latex/fontspec/fontspec-example.tex", "latex/polyglossia/polyglossia.tex",
    "xelatex/xecjk/example/xeCJK-example-fallback.tex",
    "xelatex/xecjk/example/xeCJK-example-CJKecglue.tex",
    "xelatex/xecjk/example/xeCJK-example-autofake.tex",
    "latex/unicode-math/unicode-math.ltx", "latex/unicode-math/unimath-symbols.ltx",
    "latex/unicode-math/unimath-example.ltx",
    "latex/tcolorbox/tcolorbox.tex", "latex/tools/tools-overview.tex",
    "latex/graphics/grfguide.tex", "latex/tikz-cd/tikz-cd-doc.tex",
    "latex/forest/forest-doc.tex", "latex/circuitikz/circuitikzmanual.tex",
    "latex/csquotes/csquotes.tex", "latex/enumitem/enumitem.tex",
    "latex/titlesec/titlesec.tex", "latex/tabularray/tabularray.tex",
    "latex/nicematrix/nicematrix.tex", "latex/acro/acro-manual.tex",
    "latex/natbib/natnotes.tex", "latex/moderncv/manual/moderncv_userguide.tex",
    "latex/exam/examdoc.tex", "latex/geometry/geometry-samples.tex",
    "latex/biblatex-chicago/biblatex-chicago.tex",
]

# Packages whose documents need LuaTeX, which partex does not have.
LUA = re.compile(r"\\(directlua|luaexec|luadirect)|\\usepackage(\[[^]]*\])?\{[^}]*\b(luacode|luatexja|luaotfload|luamplib|lua-[a-z]+|luatexbase|ltluatex|lualatex-math|luacolor|selnolig)\b")
XE = re.compile(r"\\usepackage(\[[^]]*\])?\{[^}]*\b(fontspec|polyglossia|xeCJK|unicode-math|xltxtra|xunicode|mathspec|xgreek|bidi|xepersian|zhspacing|xetexko|ucharclasses|realscripts|metalogo|xltabular-x)\b|\\documentclass(\[[^]]*\])?\{(ctexart|ctexbook|ctexrep|ctexbeamer)\}")
MAGIC = re.compile(r"^%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*(\w+)", re.I | re.M)
ARARA = re.compile(r"^%\s*arara:\s*(pdflatex|xelatex|lualatex|latex)", re.M)


def uncomment(text):
    return "\n".join(l.split("%", 1)[0] if not l.lstrip().startswith("%") else "" for l in text.splitlines())


def slug(rel):
    return re.sub(r"[^A-Za-z0-9._-]+", "_", rel.rsplit(".", 1)[0])


def engine_of(text):
    """pdflatex, xelatex or lualatex: a magic comment, else the packages."""
    body = uncomment(text)
    for m in (MAGIC.search(text), ARARA.search(text)):
        e = m.group(1).lower() if m else ""
        if e.startswith(("xe", "xelatex")):
            return "xelatex"
        if e.startswith("lua") or e == "latexmkl":
            return "lualatex"
        if e.startswith(("pdf", "latex")) and not XE.search(body):
            return "pdflatex"
    if LUA.search(body):
        return "lualatex"
    if XE.search(body):
        return "xelatex"
    return "pdflatex"


def discover(args):
    doc = Path(args.doc)
    by_pkg = {}
    for fmt in ("latex", "xelatex", "generic"):
        base = doc / fmt
        if not base.is_dir():
            continue
        for p in sorted(base.rglob("*")):
            if p.suffix not in (".tex", ".ltx") or not p.is_file():
                continue
            try:
                raw = p.read_bytes()[:4 << 20]
            except OSError:
                continue
            text = raw.decode("utf-8", errors="replace")
            body = uncomment(text)
            rel = p.relative_to(doc).as_posix()
            if "\\documentclass" not in body or "\\begin{document}" not in body and rel not in FORCED:
                continue
            parts = rel.split("/")
            pkg = "/".join(parts[:2])
            stem = p.stem.lower()
            name = parts[1].lower()
            score = 0
            if stem in (name, name + "-doc", name + "doc", name + "-manual", name + "-guide",
                        name + "-userguide", name + "-user", name + "-en", "doc", "manual"):
                score += 10
            if any(k in stem for k in ("manual", "guide", "doc", name)):
                score += 4
            if any(d in ("example", "examples", "sample", "samples", "test", "tests", "demo", "demos")
                   for d in parts[2:-1]):
                score -= 6
            if any(k in stem for k in ("example", "sample", "test", "demo", "template")):
                score -= 3
            score += min(len(raw) // 20000, 5)
            cand = {"id": slug(rel), "path": rel, "dir": "/".join(parts[:-1]), "main": parts[-1],
                    "engine": engine_of(text), "score": score}
            by_pkg.setdefault(pkg, []).append(cand)
    out, seen = [], set()
    forced = set(FORCED)
    for pkg in sorted(by_pkg):
        cs = sorted(by_pkg[pkg], key=lambda c: (-c["score"], c["path"]))
        for c in cs:
            if c["path"] in forced:
                out.append(c)
                seen.add(c["path"])
        if not any(c["path"] in seen for c in cs):
            out.append(cs[0])
            seen.add(cs[0]["path"])
    missing = sorted(forced - seen)
    common.write_json(args.out, {"doc": str(doc), "candidates": out, "forced_missing": missing})
    eng = {}
    for c in out:
        eng[c["engine"]] = eng.get(c["engine"], 0) + 1
    print(f"{len(out)} candidates {eng}; forced but not found: {missing}")


# -- the latexmk-like loop -------------------------------------------------

def citations(aux):
    try:
        lines = Path(aux).read_bytes().splitlines()
    except OSError:
        return None
    keep = [l for l in lines if l.startswith((b"\\citation{", b"\\bibdata{", b"\\bibstyle{", b"\\@input{"))]
    return b"\n".join(keep) if any(l.startswith(b"\\bibdata{") for l in keep) else None


class Tools:
    """How the loop runs BibTeX, biber and makeindex for one side."""

    def __init__(self, partex=None):
        self.partex = partex

    def bibtex(self, stem):
        return [self.partex, "-bibtex", stem] if self.partex else ["bibtex", stem]

    def makeindex(self, args):
        return [self.partex, "-makeindex", *args] if self.partex else ["makeindex", *args]

    @staticmethod
    def biber(stem):
        return ["biber", "--quiet", stem]


def loop(run_dir, engine_cmd, tools, environ, job, base, inputs_ist, log):
    """Run `engine_cmd` in `run_dir` until the job's files settle, the
    tools between passes as latexmk runs them: {status, passes, tools,
    runs, wall_s, rss_kib}."""
    res = {"passes": 0, "tools": [], "wall_s": 0.0, "rss_kib": 0, "status": "ok"}
    cited, bcf, indexed = {}, None, {}
    state = common.snapshot(run_dir, base)

    def exec_(cmd, what):
        r = common.run(cmd, run_dir, environ, PASS_TIMEOUT, out=run_dir / f"term-{what}.txt")
        res["wall_s"] = round(res["wall_s"] + r["wall_s"], 3)
        res["rss_kib"] = max(res["rss_kib"], r["rss_kib"])
        log.append({"cmd": what, **r})
        if r["timeout"]:
            res["status"] = "timeout"
        return r

    for n in range(1, MAX_PASSES + 1):
        r = exec_(engine_cmd, f"pass{n}")
        res["passes"] = n
        res["exit"] = r["exit"]
        if res["status"] == "timeout":
            return res
        # BibTeX on every .aux that names a database, when its citations change
        for aux in sorted(run_dir.rglob("*.aux")):
            c = citations(aux)
            rel = aux.relative_to(run_dir).with_suffix("").as_posix()
            if c is not None and cited.get(rel) != c:
                exec_(tools.bibtex(rel), f"bibtex-{slug(rel)}")
                res["tools"].append("bibtex")
                cited[rel] = c
        b = run_dir / f"{job}.bcf"
        if b.exists():
            d = common.sha256_file(b)
            if d != bcf:
                exec_(tools.biber(job), "biber")
                res["tools"].append("biber")
                bcf = d
        for idx in sorted(run_dir.glob("*.idx")) + sorted(run_dir.glob("*.glo")) + sorted(run_dir.glob("*.acn")):
            d = common.sha256_file(idx)
            if indexed.get(idx.name) == d:
                continue
            indexed[idx.name] = d
            stem = idx.stem
            if idx.suffix == ".idx":
                ist = [a for s in (stem, job) if f"{s}.ist" in inputs_ist for a in ("-s", f"{s}.ist")][:2]
                exec_(tools.makeindex([*ist, idx.name]), f"makeindex-{stem}")
                res["tools"].append("makeindex" if not ist else "makeindex-style")
            elif (run_dir / f"{stem}.ist").exists():
                out_ext = {".glo": ("gls", "glg"), ".acn": ("acr", "alg")}[idx.suffix]
                exec_(tools.makeindex(["-s", f"{stem}.ist", "-t", f"{stem}.{out_ext[1]}",
                                       "-o", f"{stem}.{out_ext[0]}", idx.name]), f"glossary-{idx.suffix[1:]}")
                res["tools"].append("glossaries")
        new = common.snapshot(run_dir, base)
        if new == state:
            res["settled"] = True
            break
        state = new
    else:
        res["settled"] = False
    res["tools"] = sorted(set(res["tools"]))
    return res


def oracle_cmd(engine, main):
    return [engine, "-no-shell-escape", "-interaction=nonstopmode", main]


def partex_compat(binary, engine, main):
    flavor, fmt = ("xetex", "xelatex") if engine == "xelatex" else ("pdftex", "pdflatex")
    return [binary, "--compat=tex", f"-engine={flavor}", f"-fmt={fmt}", "-no-shell-escape",
            "-interaction=nonstopmode", main]


def side_env(engine, extra=None, partex=False, formats=None):
    e = {"HOME": str(WORK / "home"), "TEXMFVAR": str(WORK / "texmf-var")}
    if engine == "xelatex":
        e["FONTCONFIG_FILE"] = str(common.HERE.parent / "xetex" / "fonts.conf")
        if partex:
            e["PARTEX_XETEX"] = "1"
    if partex:
        e["PARTEX_CACHE_DIR"] = str(WORK / "cache")
        e["PARTEX_STORE_DIR"] = str(WORK / "store")
        e["NO_COLOR"] = "1"
        if formats:
            e["TEXFORMATS"] = f"{formats}:"
            e["PARTEX_FORMATS"] = str(formats)
    if extra:
        e.update(extra)
    return common.env(e)


def errors_in(log):
    try:
        text = Path(log).read_bytes()
    except OSError:
        return None
    return sum(1 for l in text.splitlines() if l.startswith(b"! "))


def why_failed(run_dir, job, term):
    """A line saying why a build gave no PDF."""
    text = b""
    for p in (run_dir / f"{job}.log", term):
        try:
            text += Path(p).read_bytes()[-200000:]
        except OSError:
            pass
    shell = re.search(rb"runsystem\([^\n]*disabled", text)
    for pat in (
        rb"Not (?:reading from|writing to) [^\n]*",
        rb"panicked at[^\n]*",
        rb"! LaTeX Error: File `[^']*' not found",
        rb"! [^\n]*",
    ):
        m = re.search(pat, text)
        if m:
            what = m.group(0)[:200].decode("latin-1")
            return f"needs shell escape? {what}" if shell else what
    if shell:
        return "needs shell escape"
    return "no PDF"


def prepare(doc, c, base_dir):
    src = Path(doc) / c["dir"]
    run_dir = WORK / "run" / c["id"]
    common.fresh_copy(src, run_dir)
    return run_dir


def oracle_one(doc, c, cache, new_cache=None):
    """The oracle's outputs for candidate `c`: from the cache (`cache`,
    then `new_cache`), or built into `new_cache` (the cache itself when
    there is none)."""
    src = Path(doc) / c["dir"]
    h = common.tree_hash(src)
    name = f"{c['id']}-{h[:16]}"
    for d in (cache, new_cache):
        if d:
            res = common.read_json(Path(d) / name / "result.json")
            if res and res.get("harness") == HARNESS_VERSION:
                return Path(d) / name, res
    key = Path(new_cache or cache) / name
    run_dir = prepare(doc, c, src)
    base = {rel: common.sha256_file(run_dir / rel) for rel in common.tree_files(run_dir)}
    ist = {k for k in base if k.endswith(".ist")}
    job = Path(c["main"]).stem
    log = []
    res = {"id": c["id"], "path": c["path"], "engine": c["engine"], "input_hash": h,
           "harness": HARNESS_VERSION}
    if c["engine"] == "lualatex":
        res.update(status="excluded", reason="needs LuaTeX (partex has none)")
    else:
        r = loop(run_dir, oracle_cmd(c["engine"], c["main"]), Tools(), side_env(c["engine"]),
                 job, base, ist, log)
        res.update(r)
        pdf = run_dir / f"{job}.pdf"
        res["errors"] = errors_in(run_dir / f"{job}.log")
        if r["status"] == "timeout":
            res.update(status="excluded", reason="the oracle timed out")
        elif not pdf.exists():
            res.update(status="excluded", reason="the oracle fails: " + why_failed(run_dir, job, run_dir / f"term-pass{r['passes']}.txt"))
        elif not r.get("settled"):
            res.update(status="excluded", reason=f"the oracle does not settle in {MAX_PASSES} passes")
        else:
            res["status"] = "ok"
    res["log"] = log
    if key.exists():
        shutil.rmtree(key)
    (key / "files").mkdir(parents=True)
    if res["status"] == "ok":
        res["outputs"] = common.snapshot(run_dir, base)
        res["pdf"] = f"{job}.pdf"
        for rel in res["outputs"]:
            (key / "files" / rel).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(run_dir / rel, key / "files" / rel)
    for n in (f"{job}.log", f"term-pass{res.get('passes', 1)}.txt"):
        if (run_dir / n).exists():
            shutil.copy2(run_dir / n, key / n)
    common.write_json(key / "result.json", res)
    shutil.rmtree(run_dir, ignore_errors=True)
    return key, res


def compare(run_dir, base, key, ores):
    """Differences between this run's outputs and the oracle's."""
    mine = common.snapshot(run_dir, base)
    theirs = ores["outputs"]
    diffs = sorted(n for n in set(mine) | set(theirs) if mine.get(n) != theirs.get(n))
    out = {"files": [d for d in diffs if d != ores["pdf"]]}
    pdf = run_dir / ores["pdf"]
    if not pdf.exists():
        out["pdf"] = {"missing": True}
    elif ores["pdf"] in diffs:
        out["pdf"] = pdfdiff.diff(key / "files" / ores["pdf"], pdf)
    else:
        out["pdf"] = {"same": True}
    return out


def mode_applies(mode, c, ores):
    tools = set(ores.get("tools", []))
    if mode == "plain":
        return None
    if mode in ("build", "machine"):
        if c["engine"] != "pdflatex":
            return "partex build has no XeTeX yet"
        if tools - {"bibtex", "makeindex"}:
            return "needs " + ", ".join(sorted(tools - {"bibtex", "makeindex"})) + " (not in partex build)"
    if mode == "ssa" and tools - {"bibtex", "makeindex"}:
        return "needs " + ", ".join(sorted(tools - {"bibtex", "makeindex"})) + " (not in SSA mode's tools)"
    return None


def run_mode(mode, c, key, ores, doc, binary, formats):
    na = mode_applies(mode, c, ores)
    if na:
        return {"status": "n/a", "note": na}
    run_dir = prepare(doc, c, None)
    for d in ("cache", "store"):
        shutil.rmtree(WORK / d, ignore_errors=True)
    base = {rel: common.sha256_file(run_dir / rel) for rel in common.tree_files(run_dir)}
    ist = {k for k in base if k.endswith(".ist")}
    job = Path(c["main"]).stem
    log = []
    eng = c["engine"]
    if mode == "plain":
        r = loop(run_dir, partex_compat(binary, eng, c["main"]), Tools(binary),
                 side_env(eng, partex=True, formats=formats), job, base, ist, log)
    else:
        if mode == "ssa":
            cmd = partex_compat(binary, eng, c["main"])
            extra = {"PARTEX_SSA": "1"}
        else:
            cmd = [binary, "build", "--no-shell-escape", "--color", "never", c["main"]]
            extra = {"PARTEX_MACHINE": "0"} if mode == "build" else {}
            if mode == "build":
                cmd.insert(2, "--no-machine")
        x = common.run(cmd, run_dir, side_env(eng, extra, partex=True, formats=formats),
                       MODE_TIMEOUT, out=run_dir / "term-mode.txt")
        log.append({"cmd": mode, **x})
        r = {"status": "timeout" if x["timeout"] else "ok", "exit": x["exit"],
             "wall_s": x["wall_s"], "rss_kib": x["rss_kib"]}
    res = {k: r[k] for k in ("exit", "wall_s", "rss_kib", "passes", "settled") if k in r}
    term = b"".join(p.read_bytes()[-400000:] for p in sorted(run_dir.glob("term*.txt")))
    if r["status"] == "timeout":
        res["status"] = "timeout"
    elif b"panicked at" in term:
        m = re.search(rb"panicked at[^\n]*\n?[^\n]*", term)
        res.update(status="error", note=m.group(0)[:300].decode("latin-1"))
    elif not (run_dir / ores["pdf"]).exists():
        res.update(status="error", note=why_failed(run_dir, job, run_dir / "term-mode.txt"))
    else:
        cmp = compare(run_dir, base, key, ores)
        res.update(cmp)
        same = cmp["pdf"].get("same") and not cmp["files"]
        res["status"] = "identical" if same else "differs"
        if not same:
            first = cmp["pdf"].get("summary") if not cmp["pdf"].get("same") else None
            res["first"] = first or ("files: " + ", ".join(cmp["files"][:4]))
    res["log"] = log
    shutil.rmtree(run_dir, ignore_errors=True)
    return res


def load_list(path):
    data = common.read_json(path)
    if data is None:
        sys.exit(f"cannot read {path}")
    return data.get("manuals") or data.get("candidates")


def shard(items, spec):
    if not spec:
        return items
    i, n = (int(x) for x in spec.split("/"))
    return [c for k, c in enumerate(items) if k % n == i]


def cmd_oracle(args):
    items = shard(load_list(args.list), args.shard)
    if args.ids:
        items = [c for c in items if c["id"] in args.ids.split(",")]
    for c in items:
        key, res = oracle_one(args.doc, c, args.cache, args.new_cache)
        print(f"{c['id']}: {res['status']} {res.get('reason', '')} passes={res.get('passes')} "
              f"tools={res.get('tools')} {res.get('wall_s')}s", flush=True)


def cmd_run(args):
    items = shard(load_list(args.list), args.shard)
    if args.ids:
        items = [c for c in items if c["id"] in args.ids.split(",")]
    modes = args.modes.split(",")
    out = Path(args.out)
    for c in items:
        h = common.tree_hash(Path(args.doc) / c["dir"])
        res = {"id": c["id"], "path": c["path"], "engine": c["engine"], "suite": f"manuals-{c['engine']}"}
        if c.get("input_hash") and h != c["input_hash"]:
            res.update(status="error", note=f"input tree hash {h[:16]} is not the pinned {c['input_hash'][:16]}")
            common.write_json(out / f"{c['id']}.json", res)
            print(f"{c['id']}: HASH MISMATCH", flush=True)
            continue
        key, ores = oracle_one(args.doc, c, args.cache, args.new_cache)
        res["oracle"] = {k: ores.get(k) for k in ("status", "reason", "passes", "tools", "wall_s", "rss_kib", "errors")}
        if ores["status"] != "ok":
            res["status"] = "excluded"
            common.write_json(out / f"{c['id']}.json", res)
            continue
        res["modes"] = {}
        for m in modes:
            res["modes"][m] = run_mode(m, c, key, ores, args.doc, args.bin, args.formats)
            common.write_json(out / f"{c['id']}.json", res)
        line = " ".join(f"{m}={v['status']}" for m, v in res["modes"].items())
        print(f"{c['id']}: {line}", flush=True)


def cmd_pin(args):
    cands = load_list(args.list)
    pinned, excluded = [], []
    for c in cands:
        hits = sorted(Path(args.cache).glob(f"{c['id']}-*/result.json"))
        res = common.read_json(hits[-1]) if hits else None
        if not res:
            excluded.append({"id": c["id"], "path": c["path"], "reason": "no oracle result"})
            continue
        if res["status"] == "ok":
            pinned.append({**{k: c[k] for k in ("id", "path", "dir", "main", "engine")},
                           "input_hash": res["input_hash"], "tools": res.get("tools", []),
                           "passes": res.get("passes"), "oracle_wall_s": res.get("wall_s"),
                           "oracle_errors": res.get("errors")})
        else:
            excluded.append({"id": c["id"], "path": c["path"], "engine": c["engine"],
                             "reason": res.get("reason")})
    for f in (common.read_json(args.list, {}) or {}).get("forced_missing", []):
        excluded.append({"id": slug(f), "path": f, "reason": "not a standalone document in the doc tree "
                         "(no \\documentclass and \\begin{document}: its source is elsewhere, e.g. a .dtx "
                         "in texmf-dist/source, which the Arch packages leave out)"})
    old = common.read_json(args.out, {})
    doc = {
        "image": args.image or old.get("image"),
        "texlive": "TeX Live 2026.1 (svn r78408), Arch Linux packages of the 2026-09-25 archive snapshot, texlive-doc",
        "harness": HARNESS_VERSION,
        "manuals": sorted(pinned, key=lambda c: c["id"]),
        "excluded": sorted(excluded, key=lambda c: c["id"]),
    }
    common.write_json(args.out, doc)
    by = {}
    for c in pinned:
        by[c["engine"]] = by.get(c["engine"], 0) + 1
    print(f"pinned {len(pinned)} {by}, excluded {len(excluded)}")


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    d = sub.add_parser("discover")
    d.add_argument("--doc", required=True)
    d.add_argument("--out", required=True)
    for name in ("oracle", "run"):
        p = sub.add_parser(name)
        p.add_argument("--list", required=True)
        p.add_argument("--doc", required=True)
        p.add_argument("--cache", required=True)
        p.add_argument("--new-cache")
        p.add_argument("--shard")
        p.add_argument("--ids")
        if name == "run":
            p.add_argument("--bin", required=True)
            p.add_argument("--formats", required=True)
            p.add_argument("--out", required=True)
            p.add_argument("--modes", default="plain,build,machine,ssa")
    p = sub.add_parser("pin")
    p.add_argument("--list", required=True)
    p.add_argument("--cache", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--image")
    args = ap.parse_args()
    {"discover": discover, "oracle": cmd_oracle, "run": cmd_run, "pin": cmd_pin}[args.cmd](args)


if __name__ == "__main__":
    main()
