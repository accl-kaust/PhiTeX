#!/usr/bin/env python3
"""The edit matrix: one document's edit kinds x places x -j, through the
live watch (`phitex watch --json`, the product's edit path). The speed
scoreboard's runner, kept here for scripts/ci/bench.py's edit timing.

    editmatrix.py REL PO SRCDIR MAIN ENGINE DOC OUT

PHASES (timed:full:po by default; bench.py runs timed:oracle):
  timed   each edit through the watch, REPS tries at each -j of JS
  full    a full `phitex build` of each edited source (-j 8): the md5 the
          edit's PDF must equal
  oracle  a full rebuild of each edited source by TeX Live's own engine
          (manuals.py's loop: passes and tools to the fixpoint, inside
          the CI image), once: its wall time (bench.json's oracle_s) and
          its PDF's md5
  po      a profile+observe build's run log per edit (PO, unless "-")

Per -j in JS (1:8): a watch on a fresh copy, its cold build, then each
edit (kind, place) saved, its build waited for (wall: save to the build
event), reverted and that build waited for; REPS times. The edited PDF's
md5 is kept and compared with a full `build` of the edited source in a
fresh directory (-j 8), the revert's with the cold build's.
Then (PO, a profile+observe build, unless PO is "-") one more watch per
-j with PHITEX_RUN_LOG and PHITEX_TEX_TIME: per edit the runs (by op, by
wake cause and field, their summed time) and the timeline's phases.
Writes OUT/DOC.jsonl (a record per cell) and prints the table.
"""

import hashlib
import json
import os
import queue
import re
import shutil
import statistics
import subprocess
import sys
import threading
import time
from collections import Counter, defaultdict

REL, PO, SRC, MAIN, ENG, DOC, OUT = sys.argv[1:8]
JS = [int(x) for x in os.environ.get("JS", "1:8").split(":")]
REPS = int(os.environ.get("REPS", "3"))
W = os.environ.get("WORK", "/tmp/matrix-work")
KINDS_ENV = os.environ.get("KINDS", "")
TMO = int(os.environ.get("BUILD_TMO", "600"))
# (a cold build is no edit: its own wait, never the edits' cap)
COLD_TMO = int(os.environ.get("COLD_TMO", "1800"))
# (CAP_SKIP=1: an edit past BUILD_TMO is not tried again at the higher thread counts, a new watch costs a cold build)
CAP_SKIP = os.environ.get("CAP_SKIP") == "1"
# (the CI's pinned environment, scripts/ci/common.py PINNED, and its
# xelatex fonts.conf: the cold build's md5 is then comparable with the
# oracle's, ci/manuals.json's oracle_md5 and bench/oracle-md5.json)
ENV = dict(os.environ, SOURCE_DATE_EPOCH="1758800000", FORCE_SOURCE_DATE="1", TZ="UTC", LANG="C.UTF-8", LC_ALL="C.UTF-8",
           shell_escape="f", openout_any="p", openin_any="p", NO_COLOR="1")
if sys.argv[5] == "xelatex" and os.path.exists("scripts/xetex/fonts.conf"):
    ENV["FONTCONFIG_FILE"] = os.path.abspath("scripts/xetex/fonts.conf")
# (no engine deadline on a measurement: a cold build or reference cut short is no reference)
ENV.setdefault("PHITEX_TIME_LIMIT_SECS", "0")
# MODE live (the product's watch: an edit appends an update section, the
# exact file is written when idle, its "written" event waited for and its
# md5 taken) or exact (--exact-pdf: every edit writes the exact file)
MODE = os.environ.get("MODE", "live")
FLAGS = ["--exact-pdf", "--no-synctex", "--engine", ENG]
WFLAGS = (["--exact-pdf"] if MODE == "exact" else []) + ["--no-synctex", "--engine", ENG]

TEXT = ["letter", "word", "sentence", "addpar", "delpar", "caption", "footnote", "math", "tikz"]
XREF = ["heading", "addsec", "delsec", "label", "ref", "cite", "addfloat", "delfloat", "index", "macro"]
KINDS = [k for k in TEXT + XREF if not KINDS_ENV or k in KINDS_ENV.split(":")]


# (user, 2026-11-03: an edit that changes one cross-reference entry is
# held to 100 ms like text; only renumbering edits keep 1 s)
RENUMBER = ["addsec", "delsec", "addfloat", "delfloat"]


def bar(kind):
    return 100.0


# ---- the source, in document order ----------------------------------
VERB = {"verbatim", "verbatim*", "Verbatim", "lstlisting", "minted", "ltxexample", "LTXexample", "codeexample", "filecontents",
        "filecontents*", "comment", "macrocode", "tcblisting", "sourcecode", "example", "examplecode", "BVerbatim", "SaveVerbatim",
        "alltt", "luacode", "pycode", "codeexample*", "lstlisting*", "tikzcdexample", "dispExample", "dispExample*", "pgfmanualentry"}
TIKZ = {"tikzpicture", "tikzcd", "axis", "semilogyaxis", "semilogxaxis", "loglogaxis", "circuitikz", "pgfpicture", "polaraxis", "groupplot"}
MATH = {"equation", "equation*", "align", "align*", "gather", "gather*", "multline", "multline*", "displaymath", "eqnarray", "eqnarray*",
        "flalign", "flalign*", "alignat", "alignat*"}
FLOAT = {"figure", "figure*", "table", "table*"}
TEXTENV = {"document", "frame", "itemize", "enumerate", "description", "quote", "quotation", "abstract", "center", "flushleft",
           "flushright", "block", "exampleblock", "alertblock", "theorem", "proof", "lemma", "remark", "definition", "column", "columns"}
ENVRE = re.compile(r"\\(begin|end)\{([^}]+)\}")
INPUT = re.compile(r"\\(?:input|include|subfile)\{([^}]+)\}")
WORD = re.compile(r"(?<=[ ])([a-z]{5,12})(?=[ .,;])")


def uncomment(s):
    m = re.search(r"(?<!\\)%", s)
    return s[: m.start()] if m else s


class Src:
    def __init__(self, d, main):
        self.d = d
        self.files = {}
        self.split = {}
        self.recs = []  # (file, line index, envs tuple)
        self.seen = set()
        txt = self.load(main)
        ls = txt.split("\n")
        self.latex = "\\begin{document}" in txt
        start = next((i for i, l in enumerate(ls) if "\\begin{document}" in uncomment(l)), -1) + 1
        self.pre = ls[: max(start - 1, 0)]
        self.walk(main, start, ["document"] if self.latex else [])

    def load(self, f):
        if f not in self.files:
            self.files[f] = open(os.path.join(self.d, f), encoding="utf-8", errors="surrogateescape").read()
        return self.files[f]

    def walk(self, f, start, envs):
        self.seen.add(f)
        ls = self.load(f).split("\n")
        verb = None
        for i in range(start, len(ls)):
            line = ls[i]
            if verb:
                if f"\\end{{{verb}}}" in line:
                    verb = None
                    if envs:
                        envs.pop()
                continue
            u = uncomment(line)
            if "\\end{document}" in u:
                return True
            self.recs.append((f, i, tuple(envs)))
            for m in ENVRE.finditer(u):
                e = m.group(2)
                if m.group(1) == "begin":
                    envs.append(e)
                    if e in VERB:
                        verb = e
                        break
                elif envs and envs[-1] == e:
                    envs.pop()
                elif e in envs:
                    while envs and envs.pop() != e:
                        pass
            for m in INPUT.finditer(u):
                n = m.group(1).strip()
                n = n if n.endswith(".tex") else n + ".tex"
                if n not in self.seen and os.path.isfile(os.path.join(self.d, n)):
                    if self.walk(n, 0, envs):
                        return True
        return False

    def line(self, r):
        if r[0] not in self.split:
            self.split[r[0]] = self.files[r[0]].split("\n")
        return self.split[r[0]][r[1]]


def plain(s):
    t = s.strip()
    return bool(t) and t[0] not in "\\%&{}[]|" and "\\verb" not in s and "|" not in s and "&" not in s and not t.startswith("$")


PARBAD = re.compile(r"\\(begin|end|item|label|(sub)*section|chapter|part|paragraph|caption|bibitem|newpage|clearpage|frametitle|input|include)\b")


def parline(s):
    t = s.strip()
    return bool(t) and not t.startswith("%") and not PARBAD.search(uncomment(s)) and "\\verb" not in s


def balanced(b):
    b = re.sub(r"\\[{}$]", "", b)
    d = 0
    for c in b:
        d += (c == "{") - (c == "}")
        if d < 0:
            return False
    return d == 0 and b.count("$") % 2 == 0


def word_at(s):
    u = uncomment(s)
    for m in WORD.finditer(u):
        if u.count("$", 0, m.start()) % 2 == 0:
            return m
    return None


def textenv(envs):
    return all(e in TEXTENV for e in envs)


def edits(src):
    """[(kind, place, file, new_text, note)]"""
    recs = src.recs
    L = lambda r: src.line(r)
    out = []

    order = {(r[0], r[1]): n for n, r in enumerate(recs)}

    def pick(cands):
        """The candidates nearest a tenth and a half of the body."""
        if not cands:
            return []
        near = lambda t: min(cands, key=lambda c: abs(order.get((c[0], c[1]), 0) - t))
        a, b = near(len(recs) // 10), near(len(recs) // 2)
        return [("mid", b)] if a == b else [("start", a), ("mid", b)]

    def put(kind, place, r, fn, note=""):
        ls = src.files[r[0]].split("\n")
        res = fn(ls, r[1])
        t = "\n".join(ls)
        # (an edit that leaves the text as it was starts no build)
        if res is not None and t != src.files[r[0]]:
            out.append((kind, place, r[0], t, note))

    plainrecs = [r for r in recs if textenv(r[2]) and plain(L(r)) and word_at(L(r))]
    # paragraphs: runs of plain lines between blank lines, in one file
    pars = []
    k = 0
    while k < len(recs):
        r = recs[k]
        if L(r).strip() == "" and textenv(r[2]):
            j = k + 1
            while j < len(recs) and recs[j][0] == r[0] and recs[j][1] == recs[j - 1][1] + 1 and textenv(recs[j][2]) and parline(L(recs[j])):
                j += 1
            body = "".join(uncomment(L(x)) for x in recs[k + 1:j])
            if j > k + 1 and j < len(recs) and recs[j][0] == r[0] and L(recs[j]).strip() == "" and balanced(body) and word_at(L(recs[k + 1])) is not None or (j > k + 1 and j < len(recs) and L(recs[j]).strip() == "" and balanced(body) and any(word_at(L(x)) for x in recs[k + 1:j])):
                pars.append((r[0], recs[k + 1][1], recs[j][1]))  # file, first line, blank after
            k = j if j > k + 1 else k + 1
        else:
            k += 1
    parrecs = [(f, a, ()) for f, a, b in pars]
    parend = {(f, a): b for f, a, b in pars}
    heads = [r for r in recs if re.match(r"\s*\\(chapter|(sub)*section)\*?\{[A-Za-z]", L(r))]
    labels = sorted(set(m.group(1) for r in recs for m in re.finditer(r"\\label\{([^}]+)\}", uncomment(L(r)))))
    refs = [r for r in recs if re.search(r"\\(eq|auto|page|c|C)?ref\{[^}]+\}", uncomment(L(r)))]
    CITE = r"~?\\(?:no)?(?:cite[a-z]*|[pP]arencite|[tT]extcite|[aA]utocite|footcite|supercite|smartcite)\*?(?:\[[^\]]*\]){0,2}\{[^}]+\}"
    cites = [r for r in recs if not r[2] or textenv(r[2]) if re.search(CITE, uncomment(L(r)))]
    caps = [r for r in recs if "\\caption{" in uncomment(L(r)) and word_at(L(r)[L(r).index("\\caption{"):])]
    fns = [r for r in recs if "\\footnote" in uncomment(L(r)) and word_at(L(r)[L(r).index("\\footnote"):])]
    idx = [r for r in recs if re.search(r"\\index\{[A-Za-z][^|}]*\}", uncomment(L(r)))]
    NUM = re.compile(r"(?<=[(,])(\s*)(-?\d+(?:\.\d+)?)(?=\s*(?:cm|pt|mm)?\s*[,)])")
    tikz = [r for r in recs if any(e in TIKZ for e in r[2]) and not uncomment(L(r)).lstrip().startswith("\\begin") and NUM.search(uncomment(L(r)))]
    maths = []
    for i, r in enumerate(recs):
        if r[2] and r[2][-1] in MATH and (i == 0 or not recs[i - 1][2] or recs[i - 1][2][-1] not in MATH):
            t = L(r).strip()
            if t and not t.startswith(("\\label", "\\tag", "%", "\\begin", "\\end", "\\intertext", "\\notag", "\\nonumber")) and re.match(r"\s*\\begin\{", L(recs[i - 1])):
                maths.append(r)
    floats = []
    for i, r in enumerate(recs):
        m = re.match(r"\s*\\begin\{(figure\*?|table\*?)\}", L(r))
        if m:
            e = m.group(1)
            for j in range(i, min(i + 200, len(recs))):
                if recs[j][0] != r[0]:
                    break
                if f"\\end{{{e}}}" in L(recs[j]):
                    floats.append((r[0], r[1], recs[j][1]))
                    break
    fl = {(f, a): b for f, a, b in floats}
    newcmd = None
    for l in src.pre:
        m = re.match(r"\s*\\newcommand\{?(\\[A-Za-z]+)\}?\{([^{}#]*)\}\s*(%.*)?$", l)
        if m:
            newcmd = (m.group(1), m.group(2))

    for kind in KINDS:
        if kind == "letter":
            for p, r in pick(plainrecs):
                def f(ls, i):
                    m = word_at(ls[i]); ls[i] = ls[i][: m.start(1) + 2] + "x" + ls[i][m.start(1) + 2:]; return 1
                put(kind, p, r, f)
        elif kind == "word":
            for p, r in pick(plainrecs):
                def f(ls, i):
                    m = word_at(ls[i]); w = m.group(1)
                    ls[i] = ls[i][: m.start(1)] + "".join(chr((ord(c) - 96) % 26 + 97) for c in w) + ls[i][m.end(1):]; return 1
                put(kind, p, r, f)
        elif kind == "sentence":
            for p, r in pick(plainrecs):
                def f(ls, i):
                    ls[i] = ls[i] + " This sentence is added to make the paragraph run one line longer than it was before the edit."; return 1
                put(kind, p, r, f)
        elif kind == "addpar":
            for p, r in pick(parrecs):
                b = parend[(r[0], r[1])]
                def f(ls, i, b=b):
                    ls.insert(b, "\nThis paragraph is added by the edit matrix to see what an added paragraph costs, with a few more words in it."); return 1
                put(kind, p, r, f)
        elif kind == "delpar":
            for p, r in pick(parrecs):
                b = parend[(r[0], r[1])]
                def f(ls, i, b=b):
                    del ls[i: b + 1]; return 1
                put(kind, p, r, f)
        elif kind == "heading":
            for p, r in pick(heads):
                def f(ls, i):
                    ls[i] = re.sub(r"\{([A-Za-z])", r"{X\1", ls[i], count=1); return 1
                put(kind, p, r, f)
        elif kind == "addsec":
            if heads:
                for p, r in pick(parrecs):
                    def f(ls, i):
                        ls.insert(i, "\\section{An added section}"); return 1
                    put(kind, p, r, f)
        elif kind == "delsec":
            for p, r in pick([h for h in heads if re.match(r"\s*\\(chapter|(sub)*section)\*?\{[^{}]*\}\s*(\\label\{[^}]*\})?\s*$", L(h))]):
                def f(ls, i):
                    del ls[i]; return 1
                put(kind, p, r, f)
        elif kind == "label":
            for p, r in pick(heads):
                def f(ls, i):
                    ls.insert(i + 1, "\\label{phitex-matrix-label}"); return 1
                put(kind, p, r, f)
        elif kind == "ref":
            if refs and len(labels) > 1:
                for p, r in pick(refs):
                    def f(ls, i):
                        m = re.search(r"\\(eq|auto|page|c|C)?ref\{([^}]+)\}", ls[i])
                        other = next(x for x in labels if x != m.group(2))
                        ls[i] = ls[i][: m.start(2)] + other + ls[i][m.end(2):]; return 1
                    put(kind, p, r, f, "a \\ref to another label")
            elif labels:
                for p, r in pick(plainrecs):
                    def f(ls, i):
                        ls[i] = ls[i] + f" See~\\ref{{{labels[0]}}}."; return 1
                    put(kind, p, r, f, "a \\ref added")
        elif kind == "cite":
            for p, r in pick(cites):
                def f(ls, i):
                    ls[i] = re.sub(CITE, "", ls[i], count=1); return 1
                put(kind, p, r, f, "a citation removed")
        elif kind == "caption":
            for p, r in pick(caps):
                def f(ls, i):
                    c = ls[i].index("\\caption{"); m = word_at(ls[i][c:])
                    k2 = c + m.start(1) + 2; ls[i] = ls[i][:k2] + "x" + ls[i][k2:]; return 1
                put(kind, p, r, f)
        elif kind == "footnote":
            for p, r in pick(fns):
                def f(ls, i):
                    c = ls[i].index("\\footnote"); m = word_at(ls[i][c:])
                    k2 = c + m.start(1) + 2; ls[i] = ls[i][:k2] + "x" + ls[i][k2:]; return 1
                put(kind, p, r, f)
        elif kind == "math":
            for p, r in pick(maths):
                def f(ls, i):
                    ls[i] = re.sub(r"^(\s*)", r"\1a+", ls[i], count=1); return 1
                put(kind, p, r, f)
        elif kind == "tikz":
            for p, r in pick(tikz):
                def f(ls, i):
                    m = NUM.search(ls[i])
                    v = float(m.group(2)) + 0.5
                    ls[i] = ls[i][: m.start(2)] + (f"{v:g}") + ls[i][m.end(2):]; return 1
                put(kind, p, r, f)
        elif kind == "addfloat":
            if src.latex:
                for p, r in pick(parrecs):
                    b = parend[(r[0], r[1])]
                    def f(ls, i, b=b):
                        ls.insert(b, "\n\\begin{figure}[htbp]\n\\centering\n\\fbox{An added float}\n\\caption{A float added by the edit matrix}\n\\end{figure}"); return 1
                    put(kind, p, r, f)
        elif kind == "delfloat":
            for p, r in pick([(f_, a, ()) for f_, a, b in floats]):
                b = fl[(r[0], r[1])]
                def f(ls, i, b=b):
                    del ls[i: b + 1]; return 1
                put(kind, p, r, f)
        elif kind == "index":
            for p, r in pick(idx):
                def f(ls, i):
                    ls[i] = re.sub(r"\\index\{", r"\\index{x", ls[i], count=1); return 1
                put(kind, p, r, f)
        elif kind == "macro":
            if newcmd and src.latex:
                for p, r in pick(parrecs):
                    def f(ls, i):
                        ls.insert(i, f"\\renewcommand{{{newcmd[0]}}}{{{newcmd[1]}}}"); return 1
                    put(kind, p, r, f, f"{newcmd[0]} redefined as it was")
            else:
                for p, r in pick(parrecs):
                    def f(ls, i):
                        ls.insert(i, "\\def\\phitexmatrixmacro{edited}"); return 1
                    put(kind, p, r, f, "a new \\def nothing reads")
    return out


# ---- the watch --------------------------------------------------------
def md5(p):
    try:
        return hashlib.md5(open(p, "rb").read()).hexdigest()[:10]
    except OSError:
        return "none"


class Watch:
    def __init__(self, binary, d, j, env, errf):
        self.d = d
        self.errf = errf
        self.err = open(errf, "ab")
        pre = []
        self.ctl = None
        if os.environ.get("PERF") and binary == PO:
            # (PERF=out.data: the profile build under `perf record`, enabled
            # around each edit's build only)
            ctl = os.path.join(W, "perf.ctl")
            ack = os.path.join(W, "perf.ack")
            for x in (ctl, ack):
                if os.path.exists(x):
                    os.remove(x)
                os.mkfifo(x)
            pre = ["perf", "record", "-e", "cycles:u", "-F", os.environ.get("PERF_F", "2000"), "--call-graph", os.environ.get("PERF_CG", "dwarf,16384"),
                   "-D", "-1", f"--control=fifo:{ctl},{ack}", "-o", os.environ["PERF"], "--"]
            self.ctl = (ctl, ack)
        self.p = subprocess.Popen([*pre, binary, "watch", "--no-view", "--json", *WFLAGS, "-j", str(j), MAIN], cwd=d, env=env,
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err)
        self.q = queue.Queue()
        self.wq = queue.Queue()
        self.dead = False
        self.last = 0
        threading.Thread(target=self.pump, daemon=True).start()

    def pump(self):
        for line in self.p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            # (the watch's own events carry the build's number; the report's
            # summary event after it does not)
            if x.get("event") in ("build", "failed") and isinstance(x.get("build"), int):
                self.q.put(x)
            elif x.get("event") == "written":
                self.wq.put(x)
        self.q.put(None)

    def build(self, tmo=None):
        if self.dead:
            return None
        while True:
            try:
                x = self.q.get(timeout=tmo or TMO)
            except queue.Empty:
                x = None
            if x is None:
                self.dead = True
                return None
            # (a build begun before the last save: not this one's)
            if x["build"] > self.last:
                self.last = x["build"]
                return x

    def put(self, f, text):
        # (a build event still queued is an earlier save's)
        while not self.q.empty():
            x = self.q.get()
            if x is None:
                self.dead = True
                break
            self.last = max(self.last, x["build"])
        p = os.path.join(self.d, f)
        open(p + ".mtmp", "w", encoding="utf-8", errors="surrogateescape").write(text)
        os.replace(p + ".mtmp", p)
        return time.monotonic()

    def exact(self, ev):
        """The md5 of the exact file of build `ev`: at once with
        --exact-pdf, else once the idle watch says it wrote it."""
        if MODE == "exact" or not ev or ev.get("event") != "build":
            return self.outpdf(ev)
        t = time.monotonic()
        while time.monotonic() - t < 120:
            try:
                x = self.wq.get(timeout=1)
            except queue.Empty:
                continue
            if x.get("build") == ev.get("build"):
                return self.outpdf(x)
            if (x.get("build") or 0) > ev.get("build"):
                break
        return "unwritten"

    def outpdf(self, ev):
        o = ev and ev.get("output")
        return md5(os.path.join(self.d, o)) if o else "none"

    def perf(self, cmd):
        if not self.ctl:
            return
        if not hasattr(self, "cf"):
            self.cf = open(self.ctl[0], "w")
            self.af = open(self.ctl[1], "r")
        self.cf.write(cmd + "\n")
        self.cf.flush()
        self.af.readline()

    def size(self):
        self.err.flush()
        return os.path.getsize(self.errf)

    def quit(self):
        try:
            self.p.stdin.write(b"q\n")
            self.p.stdin.close()
            self.p.wait(timeout=120)
        except Exception:
            self.p.kill()


def fresh(tag):
    d = os.path.join(W, tag)
    shutil.rmtree(d, ignore_errors=True)
    shutil.copytree(SRC, d)
    return d


def evrow(ev):
    ev = ev or {}
    par = ev.get("parallel") or {}
    return dict(engine_ms=par.get("engine_ms"), wall_ms=ev.get("wall_ms"), nodes=ev.get("nodes_run"), runs=ev.get("runs"),
                rounds=ev.get("rounds"), ok=ev.get("ok"), pages=ev.get("pages"), failed=ev.get("event") == "failed" or not ev)


def timed(edl):
    res = defaultdict(dict)
    capped = set()
    for j in JS:
        d = fresh(f"t{j}")
        w = Watch(REL, d, j, ENV, os.path.join(OUT, f"{DOC}-j{j}.err"))
        t = time.monotonic()
        ev0 = w.build(COLD_TMO)
        cold = w.exact(ev0)
        print(f"# {DOC} -j{j} cold {time.monotonic() - t:.1f} s pdf {cold} {evrow(ev0)}", flush=True)
        cells = open(os.path.join(OUT, f"{DOC}-cells.jsonl"), "a")
        cut = ev0 is None or cold in (None, "none", "unwritten") or evrow(ev0)["failed"]
        errs = [str(e)[:400] for e in (ev0 or {}).get("errors") or []][:5] if ev0 and ev0.get("ok") is False else []
        cells.write(json.dumps(dict(cold=j, s=time.monotonic() - t, pdf=cold, deadline=cut, errors=errs, env="ci", **evrow(ev0))) + "\n")
        cells.flush()
        # (no event, a failed build or no PDF: no cold build, so nothing to edit against)
        if cut:
            print(f"# {DOC} -j{j}: no cold build, no edits measured", flush=True)
            w.quit()
            continue
        for kind, place, f, text, note in edl:
            if (kind, place) in capped:
                continue
            orig = open(os.path.join(SRC, f), encoding="utf-8", errors="surrogateescape").read()
            rows = []
            for _ in range(REPS):
                t0 = w.put(f, text)
                ev = w.build()
                ms = (time.monotonic() - t0) * 1e3
                pdf = w.exact(ev)
                t1 = w.put(f, orig)
                rev = w.build()
                rms = (time.monotonic() - t1) * 1e3
                rows.append(dict(ms=ms, pdf=pdf, revert_ms=rms, revert_pdf=w.exact(rev), over=(TMO if ev is None and ms >= TMO * 1e3 else None), **evrow(ev)))
                # (over 5 s: far past the goal, one try says it)
                if ev is None or rev is None or ms > 5000:
                    break
            res[(kind, place)][j] = rows
            cells.write(json.dumps(dict(kind=kind, place=place, j=j, rows=rows)) + "\n")
            cells.flush()
            print(f"  {kind}/{place} j{j}: " + " ".join(f"{r['ms']:.0f}" for r in rows) + f" ms pdf {rows[0]['pdf']} revert {'ok' if all(r['revert_pdf'] == cold for r in rows) else 'DIFFERS'}", flush=True)
            if ev is None or rev is None:
                # (past the limit or gone: this edit has no time; the rest get a new watch, on the original text)
                print("  (no build event: the watch is gone or past the time limit; a new watch for the rest)", flush=True)
                if CAP_SKIP:
                    capped.add((kind, place))
                w.quit()
                d = fresh(f"t{j}")
                w = Watch(REL, d, j, ENV, os.path.join(OUT, f"{DOC}-j{j}.err"))
                w.exact(w.build(COLD_TMO))
        w.quit()
        res["cold"][j] = cold
    return res


def full(edl):
    out = {}
    for kind, place, f, text, note in edl:
        d = fresh("full")
        open(os.path.join(d, f), "w", encoding="utf-8", errors="surrogateescape").write(text)
        t = time.monotonic()
        with open(os.path.join(OUT, f"{DOC}-full.log"), "ab") as lg:
            rc = subprocess.run([REL, "build", *FLAGS, "-j", "8", MAIN], cwd=d, env=ENV, stdout=lg, stderr=subprocess.STDOUT, timeout=7200).returncode
        pdf = os.path.join(d, re.sub(r"\.tex$", "", MAIN) + ".pdf")
        # (a reference that did not finish is no reference: never an md5 an edit can equal)
        out[(kind, place)] = md5(pdf) if rc == 0 else f"failed:{rc}"
        keep = os.path.join(W, "fullpdf", f"{kind}-{place}.pdf")
        os.makedirs(os.path.dirname(keep), exist_ok=True)
        if os.path.exists(pdf):
            shutil.copy(pdf, keep)
        print(f"  full {kind}/{place}: {out[(kind, place)]} ({time.monotonic() - t:.1f} s)", flush=True)
        with open(os.path.join(OUT, f"{DOC}-cells.jsonl"), "a") as fh:
            fh.write(json.dumps(dict(kind=kind, place=place, full=out[(kind, place)])) + "\n")
    return out


RUN = re.compile(r"^run: node (\d+) \(([^)]*)\) at (\d+): (.*?); goes on (\w+), woken by (.*), parked on (.*)\tt (\d+) (\d+) w (\d+) e (\d+)")
RUNF = re.compile(r"^runf: node (\d+) field (.*)$")
TL = re.compile(r"^\s+([\d.]+) ms  (.*)$")


def cause_of(c, field):
    m = re.match(r"Some\((\w+)", c)
    k = m.group(1) if m else c
    return f"{k}:{field}" if field else k


def breakdown(seg):
    runs = []
    last = {}
    tl = []
    intl = False
    for ln in seg.split("\n"):
        m = RUN.match(ln)
        if m:
            r = dict(node=int(m[1]), op=m[2], pos=int(m[3]), how=m[4], goes=m[5], cause=m[6], ns=int(m[9]) - int(m[8]), field="")
            runs.append(r)
            last[r["node"]] = r
            continue
        m = RUNF.match(ln)
        if m and int(m[1]) in last:
            last[int(m[1])]["field"] = m[2][:60]
            continue
        if ln.startswith("timeline ("):
            tl = []
            intl = True
            continue
        if intl:
            m = TL.match(ln)
            if m:
                tl.append((m.group(2)[:70], float(m.group(1))))
            elif not ln.startswith("  "):
                intl = False
    byop = Counter()
    nsop = Counter()
    bycause = Counter()
    nscause = Counter()
    nodes = defaultdict(set)
    for r in runs:
        byop[r["op"]] += 1
        nsop[r["op"]] += r["ns"]
        c = cause_of(r["cause"], r["field"])
        bycause[c] += 1
        nscause[c] += r["ns"]
        nodes[r["op"]].add(r["pos"])
    return dict(runs=len(runs), run_ms=sum(r["ns"] for r in runs) / 1e6,
                byop={k: [byop[k], round(nsop[k] / 1e6, 1), len(nodes[k])] for k in byop},
                causes=[[c, bycause[c], round(nscause[c] / 1e6, 1)] for c, _ in nscause.most_common(8)],
                phases=sorted(tl, key=lambda x: -x[1])[:8],
                first=[(r["op"], r["pos"], cause_of(r["cause"], r["field"]), round(r["ns"] / 1e6, 2)) for r in runs[:12]])


def profiled(edl):
    res = defaultdict(dict)
    env = dict(ENV, PHITEX_RUN_LOG="1", PHITEX_TEX_TIME="1")
    for j in JS:
        d = fresh(f"p{j}")
        errf = os.path.join(W, f"po-{DOC}-j{j}.err")
        open(errf, "w").close()
        w = Watch(PO, d, j, env, errf)
        w.build()
        time.sleep(1)
        for kind, place, f, text, note in edl:
            orig = open(os.path.join(SRC, f), encoding="utf-8", errors="surrogateescape").read()
            a = w.size()
            w.perf("enable")
            t0 = w.put(f, text)
            ev = w.build()
            ms = (time.monotonic() - t0) * 1e3
            w.perf("disable")
            time.sleep(0.5)  # (the timeline follows the event)
            b = w.size()
            w.put(f, orig)
            rev = w.build()
            time.sleep(0.5)
            with open(errf, "rb") as fh:
                fh.seek(a)
                seg = fh.read(b - a).decode("utf-8", "replace")
            bd = breakdown(seg)
            bd["po_ms"] = ms
            res[(kind, place)][j] = bd
            print(f"  po {kind}/{place} j{j}: {ms:.0f} ms, {bd['runs']} runs {bd['run_ms']:.0f} ms; top {bd['causes'][:2]}", flush=True)
            if ev is None or rev is None:
                break
        w.quit()
        shutil.copy(errf, errf + ".keep") if os.path.getsize(errf) < 50e6 else None
        subprocess.run(["gzip", "-1f", errf])
        shutil.move(errf + ".gz", os.path.join(OUT, os.path.basename(errf) + ".gz")) if os.path.getsize(errf + ".gz") < 200e6 else None
    return res


def oracle(edl):
    """Each edited source rebuilt by TeX Live's engine, once: the latexmk
    loop of manuals.py (passes, BibTeX, biber, makeindex to the fixpoint)
    in a fresh copy; the user-visible wall of all its passes and tools."""
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import common
    import manuals
    out = {}
    eng = {"pdftex": "pdftex", "xetex": "xetex"}.get(ENG, ENG)
    job = re.sub(r"\.(tex|ltx)$", "", MAIN)
    for kind, place, f, text, note in edl:
        d = fresh("oracle")
        open(os.path.join(d, f), "w", encoding="utf-8", errors="surrogateescape").write(text)
        try:
            os.unlink(os.path.join(d, job + ".pdf"))
        except OSError:
            pass
        from pathlib import Path
        base = {rel: common.sha256_file(Path(d) / rel) for rel in common.tree_files(d)}
        ist = {k for k in base if k.endswith(".ist")}
        log = []
        envd = manuals.side_env(eng if eng in ("pdflatex", "xelatex") else ENG)
        # (settled when the files the next pass reads stop changing, as
        # latexmk decides: the PDF left out of that test. A document whose
        # PDF carries the time its .log was written (hyperxmp under XeTeX:
        # Grail) changes its PDF every pass and never settles otherwise,
        # running MAX_PASSES passes no user waits for)
        keep = common.NOT_COMPARED
        common.NOT_COMPARED = keep + (".pdf",)
        try:
            r = manuals.loop(Path(d), manuals.oracle_cmd(eng, MAIN), manuals.Tools(), envd, job, base, ist, log)
        finally:
            common.NOT_COMPARED = keep
        pdf = os.path.join(d, job + ".pdf")
        rec = dict(wall_s=r["wall_s"], passes=r["passes"], tools=r["tools"], status=r["status"],
                   settled=r.get("settled"), pdf=md5(pdf) if os.path.exists(pdf) else "none",
                   errors=manuals.errors_in(os.path.join(d, job + ".log")))
        out[(kind, place)] = {"0": rec}
        print(f"  oracle {kind}/{place}: {rec['wall_s']:.2f} s {rec['passes']} passes {rec['tools']} pdf {rec['pdf']}", flush=True)
        with open(os.path.join(OUT, f"{DOC}-cells.jsonl"), "a") as fh:
            fh.write(json.dumps(dict(kind=kind, place=place, oracle=rec)) + "\n")
    return out


def main():
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(W, exist_ok=True)
    src = Src(SRC, MAIN)
    edl = edits(src)
    print(f"# {DOC}: {len(src.recs)} body lines in {len(src.files)} files; {len(edl)} edits: " + " ".join(f"{k}/{p}" for k, p, *_ in edl), flush=True)
    with open(os.path.join(OUT, f"{DOC}-edits.txt"), "w") as fh:
        for kind, place, f, text, note in edl:
            orig = open(os.path.join(SRC, f), encoding="utf-8", errors="surrogateescape").read().split("\n")
            new = text.split("\n")
            i = next((k for k in range(min(len(orig), len(new))) if orig[k] != new[k]), min(len(orig), len(new)))
            fh.write(f"{kind}/{place} {f}:{i + 1} {note}\n  - {orig[i][:160] if i < len(orig) else ''}\n  + {new[i][:160] if i < len(new) else ''}\n")
    if os.environ.get("DRY"):
        print(open(os.path.join(OUT, f"{DOC}-edits.txt")).read())
        return
    phases = os.environ.get("PHASES", "timed:full:po").split(":")

    def stage(name, fn):
        p = os.path.join(OUT, f"{DOC}-{name}.json")
        if name in phases:
            x = fn(edl)
            json.dump([[list(k) if isinstance(k, tuple) else [k], {str(j): v for j, v in d.items()}] for k, d in x.items()], open(p, "w"))
        if not os.path.exists(p):
            return {}
        out = {}
        for k, d in json.load(open(p)):
            out[tuple(k) if len(k) > 1 else k[0]] = {(int(j) if j.isdigit() else j): v for j, v in d.items()} if isinstance(d, dict) else d
        return out

    tm = stage("timed", timed)
    fu = stage("full", lambda e: {k: {"0": v} for k, v in full(e).items()})
    fu = {k: v.get(0) for k, v in fu.items()}
    po = stage("po", profiled) if PO != "-" else stage("po", lambda e: {})
    orc = {k: v.get(0) for k, v in stage("oracle", oracle).items()}
    tm.setdefault("cold", {})
    rows = []
    for kind, place, f, text, note in edl:
        c = (kind, place)
        rec = dict(doc=DOC, kind=kind, place=place, note=note, bar=bar(kind), full_pdf=fu.get(c), oracle=orc.get(c))
        for j in JS:
            rs = tm.get(c, {}).get(j, [])
            if rs:
                rec[f"j{j}"] = dict(med_ms=statistics.median(r["ms"] for r in rs), engine_ms=statistics.median((r["engine_ms"] or 0) for r in rs),
                                    nodes=rs[0]["nodes"], runs=rs[0]["runs"], rounds=rs[0]["rounds"], ok=all(r["ok"] for r in rs),
                                    exact=all(r["pdf"] == fu.get(c) for r in rs), revert_exact=all(r["revert_pdf"] == tm["cold"].get(j) for r in rs),
                                    pdfs=sorted(set(r["pdf"] for r in rs)))
            if po.get(c, {}).get(j):
                rec[f"po{j}"] = po[c][j]
        rows.append(rec)
    with open(os.path.join(OUT, f"{DOC}.jsonl"), "w") as fh:
        for r in rows:
            fh.write(json.dumps(r) + "\n")
    print(f"\n## {DOC}: kind/place | " + " | ".join(f"j{j} ms (engine) runs" for j in JS) + " | exact | top cause (runs, ms) | over bar")
    for r in rows:
        cells = []
        over = []
        for j in JS:
            x = r.get(f"j{j}")
            if not x:
                cells.append("-")
                continue
            cells.append(f"{x['med_ms']:.0f} ({x['engine_ms']:.0f}) {x['runs']}")
            if x["med_ms"] > r["bar"]:
                over.append(f"j{j}")
        ex = "?" if not r.get("full_pdf") else "yes" if all(r.get(f"j{j}", {}).get("exact") for j in JS) else "NO"
        if not all(r.get(f"j{j}", {}).get("revert_exact", True) for j in JS):
            ex += ", revert NO"
        p = r.get(f"po{JS[-1]}") or r.get(f"po{JS[0]}") or {}
        top = p.get("causes", [["-", 0, 0]])[0] if p.get("causes") else ["-", 0, 0]
        print(f"{r['kind']}/{r['place']} | " + " | ".join(cells) + f" | {ex} | {top[0]} ({top[1]}, {top[2]}) | {' '.join(over) or '-'}", flush=True)


main()
