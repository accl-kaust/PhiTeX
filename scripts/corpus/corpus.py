#!/usr/bin/env python3
"""The real-document corpus runner (arXiv papers and TeX Live manuals):
one document's steps, run INSIDE the corpus container (corpus.sbatch):
no network, no home, the environment cleared, only the job's node-local
scratch writable (/w), the harness (/h) and the phitex binaries (/t)
read-only, the TeX tree the image's own.

    corpus.py netcheck
    corpus.py prepare --src FILE --out DIR          (arXiv: extract, prefilter, main file)
    corpus.py oracle  --doc DIR --main F --out DIR [--tools]
    corpus.py phitex  --doc DIR --main F --out DIR --threads 1,8 [--edit] [--oracle-pdf P]

Every step checks the network first and stops if anything is reachable.
The sources are untrusted: nothing in them is executed (no Makefile,
script, latexmkrc); pdflatex runs with shell escape off and kpathsea's
paranoid openin/openout; phitex runs without --shell-escape.
"""

import argparse
import hashlib
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tarfile
import gzip
import time
from pathlib import Path

sys.path.insert(0, "/h/ci")
import common  # noqa: E402

RUN = Path("/w/run")  # (one fixed path for every build: /ID and paths agree)
PASSES = 8


def jdump(p, v):
    Path(p).write_text(json.dumps(v, indent=1, default=str))


# -- the network must be unreachable ---------------------------------------

def netcheck():
    devs = []
    for line in Path("/proc/net/dev").read_text().splitlines()[2:]:
        devs.append(line.split(":")[0].strip())
    probes = {}
    for host, port in (("1.1.1.1", 443), ("8.8.8.8", 53), ("10.0.0.1", 22), ("10.67.24.80", 22)):
        s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        s.settimeout(3)
        try:
            s.connect((host, port))
            probes[f"{host}:{port}"] = "CONNECTED"
        except OSError as e:
            probes[f"{host}:{port}"] = e.strerror or type(e).__name__
        finally:
            s.close()
    try:
        socket.getaddrinfo("export.arxiv.org", 443)
        probes["dns"] = "RESOLVED"
    except OSError as e:
        probes["dns"] = e.strerror or type(e).__name__
    ok = devs == ["lo"] and "CONNECTED" not in probes.values() and probes["dns"] != "RESOLVED"
    return {"ok": ok, "interfaces": devs, "probes": probes}


def require_no_net(out):
    n = netcheck()
    if out:
        jdump(Path(out) / "netcheck.json", n)
    if not n["ok"]:
        print(f"NETWORK REACHABLE, stopping: {n}", flush=True)
        sys.exit(90)


# -- extraction ------------------------------------------------------------

MAX_BYTES = 1 << 30
MAX_FILES = 20000


def extract(src, dest):
    """Extract the e-print `src` into the empty `dest`: a tarball whose
    members are all plain files or directories with relative names and no
    `..`, or one gzip-ed .tex. Returns (kind, reason-if-rejected)."""
    dest.mkdir(parents=True)
    raw = Path(src).read_bytes()
    if raw[:4] == b"%PDF":
        return "pdf", "no source (PDF-only submission)"
    try:
        tf = tarfile.open(src, "r:*")
    except tarfile.TarError:
        tf = None
    if tf is None:
        try:
            body = gzip.decompress(raw) if raw[:2] == b"\x1f\x8b" else raw
        except OSError:
            return "bad", "neither a tar nor a gzip-ed file"
        if len(body) > MAX_BYTES:
            return "bad", "too large"
        (dest / "main.tex").write_bytes(body)
        return "single", None
    with tf:
        members = tf.getmembers()
        if len(members) > MAX_FILES:
            return "bad", f"{len(members)} members"
        total = 0
        for m in members:
            parts = Path(m.name).parts
            if m.name.startswith("/") or ".." in parts or not parts:
                return "bad", f"unsafe member name {m.name!r}"
            if not (m.isreg() or m.isdir()):
                return "bad", f"member {m.name!r} is not a plain file or directory (link/device)"
            if m.mode & 0o6000:
                return "bad", f"setuid/setgid member {m.name!r}"
            total += m.size
        if total > MAX_BYTES:
            return "bad", f"{total} bytes"
        tf.extractall(dest, filter="data")
    for p in dest.rglob("*"):
        if p.is_symlink():
            return "bad", "a symlink after extraction"
        p.chmod(0o755 if p.is_dir() else 0o644)
    return "tar", None


# -- the static prefilter --------------------------------------------------

BANNED = [
    (r"\\write18", "\\write18"),
    (r"\\ShellEscape", "\\ShellEscape"),
    (r"\\directlua|\\luaexec|\\luadirect", "Lua code"),
    (r"\\(?:input|openin)[^\n%]{0,20}\|", "a pipe in \\input/\\openin"),
    (r"\{[^}]*\bminted\b", "minted"),
    (r"\{[^}]*\bpythontex\b", "pythontex"),
    (r"\\usepackage(?:\[[^]]*\])?\{[^}]*\bsvg\b|\\includesvg", "svg (inkscape)"),
    (r"\{[^}]*\b(?:epstopdf|auto-pst-pdf|pst-pdf)\b|auto-convert|\\epstopdfsetup", "epstopdf/auto-convert"),
    (r"\{[^}]*\bgnuplottex\b", "gnuplottex"),
    (r"\{[^}]*\b(?:fontspec|polyglossia|xeCJK|unicode-math|luacode|luatexja)\b", "needs XeLaTeX/LuaLaTeX"),
    (r"\{[^}]*\b(?:sagetex|asypictureB|asymptote|bashful|shellesc|pdfbase|gnuplot-lua-tikz)\b", "external-program package"),
    (r"\\(?:psfig|epsfig|epsfbox)\b", "PostScript figures (latex+dvips)"),
]
MAGIC = re.compile(r"^%\s*!\s*TEX\s+(?:TS-)?program\s*=\s*(\w+)", re.I | re.M)


def uncomment(text):
    out = []
    for line in text.splitlines():
        out.append(re.split(r"(?<!\\)%", line, maxsplit=1)[0])
    return "\n".join(out)


def texts(doc):
    for p in sorted(doc.rglob("*")):
        if p.is_file() and p.suffix.lower() in (".tex", ".sty", ".cls", ".def", ".cfg", ".ltx", ".clo", ".bbl"):
            try:
                yield p, p.read_bytes()[: 8 << 20].decode("utf-8", errors="replace")
            except OSError:
                pass


def prefilter(doc):
    names = {p.relative_to(doc).as_posix() for p in doc.rglob("*") if p.is_file()}
    low = {n.lower() for n in names}
    for bad in ("phitex.toml", "latexmkrc", ".latexmkrc"):
        if any(n.rsplit("/", 1)[-1] == bad for n in low):
            return f"ships {bad}"
    for p, t in texts(doc):
        body = uncomment(t)
        for pat, why in BANNED:
            if re.search(pat, body):
                return f"{why} ({p.relative_to(doc).as_posix()})"
        m = MAGIC.search(t)
        if m and m.group(1).lower().startswith(("xe", "lua")):
            return f"% !TEX program = {m.group(1)}"
    # .eps/.ps figures without a pdflatex-readable sibling
    stems = {n.rsplit(".", 1)[0] for n in low if n.endswith((".pdf", ".png", ".jpg", ".jpeg"))}
    for n in sorted(low):
        if n.endswith((".eps", ".ps")) and n.rsplit(".", 1)[0] not in stems:
            return f"a .eps/.ps figure needing conversion ({n})"
    return None


def readme_main(doc):
    """(main, compiler) from 00README.json or 00README.XXX, if present."""
    j = doc / "00README.json"
    if j.exists():
        try:
            d = json.loads(j.read_text())
            main = next((s["filename"] for s in d.get("sources", []) if s.get("usage") == "toplevel"), None)
            return main, (d.get("process") or {}).get("compiler")
        except (ValueError, KeyError, TypeError):
            pass
    x = doc / "00README.XXX"
    if x.exists():
        for line in x.read_text(errors="replace").splitlines():
            w = line.split()
            if len(w) >= 2 and w[1] == "toplevelfile":
                return w[0], None
    return None, None


def find_main(doc):
    main, comp = readme_main(doc)
    if comp and not re.match(r"(pdf)?latex", comp):
        return None, f"00README compiler {comp}"
    if main and (doc / main).is_file():
        return main, None
    cands = []
    for p, t in texts(doc):
        if p.suffix.lower() != ".tex":
            continue
        b = uncomment(t)
        if "\\documentclass" in b and "\\begin{document}" in b:
            rel = p.relative_to(doc).as_posix()
            score = (0 if "/" in rel else 10) + (5 if p.stem.lower() in ("main", "ms", "paper", "article", "manuscript") else 0)
            cands.append((score, len(t), rel))
    if not cands:
        return None, "no file with \\documentclass and \\begin{document}"
    cands.sort(reverse=True)
    if len(cands) > 1:
        return None, f"ambiguous main file ({len(cands)} candidates: " + ", ".join(c[2] for c in cands[:4]) + ")"
    if "/" in cands[0][2]:
        return None, f"main file {cands[0][2]} is in a subdirectory"
    return cands[0][2], None


def cmd_prepare(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    doc = Path(a.doc)
    res = {}
    kind, why = extract(a.src, doc)
    res["kind"] = kind
    if why:
        res.update(status="dropped", reason=why)
    else:
        why = prefilter(doc)
        if why:
            res.update(status="dropped", reason=why)
        else:
            main, why = find_main(doc)
            if why:
                res.update(status="dropped", reason=why)
            else:
                res.update(status="ok", main=main, input_hash=common.tree_hash(doc),
                           files=len(common.tree_files(doc)))
    jdump(out / "prep.json", res)
    print(json.dumps(res))


def cmd_prepare_local(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    shutil.copytree("/in/doc", a.doc, symlinks=False)
    res = {"status": "ok", "kind": "local", "main": a.main, "input_hash": common.tree_hash(Path(a.doc))}
    jdump(out / "prep.json", res)
    print(json.dumps(res))


def cmd_prepare_manual(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    shutil.copytree(Path("/usr/share/doc/texlive") / a.dir, a.doc, symlinks=False)
    res = {"status": "ok", "kind": "manual", "main": a.main, "input_hash": common.tree_hash(Path(a.doc))}
    jdump(out / "prep.json", res)
    print(json.dumps(res))


# -- the oracle: pdflatex to its fixpoint ----------------------------------

class NoTools:
    """arXiv: the shipped .bbl/.ind are used; no BibTeX, biber, makeindex."""

    @staticmethod
    def bibtex(stem):
        return ["true"]

    @staticmethod
    def makeindex(args):
        return ["true"]

    @staticmethod
    def biber(stem):
        return ["true"]


def fresh_run(doc, job=None):
    """The document copied to RUN; the job's own PDF (a shipped one, as the
    manuals have) removed: it is an output."""
    if RUN.exists():
        shutil.rmtree(RUN)
    shutil.copytree(doc, RUN, symlinks=False)
    if job:
        (RUN / f"{job}.pdf").unlink(missing_ok=True)


def pdf_pages(p):
    try:
        b = Path(p).read_bytes()
    except OSError:
        return None
    m = re.findall(rb"/Type\s*/Pages\b[^>]*?/Count\s+(\d+)", b)
    return max(int(x) for x in m) if m else len(re.findall(rb"/Type\s*/Page\b", b))


def cmd_oracle(a):
    import manuals  # (the CI harness's latexmk-like loop)
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    job = Path(a.main).stem
    fresh_run(Path(a.doc), job)
    base = {rel: common.sha256_file(RUN / rel) for rel in common.tree_files(RUN)}
    ist = {k for k in base if k.endswith(".ist")}
    job = Path(a.main).stem
    log = []
    env = common.env({"HOME": "/w/home", "TEXMFVAR": "/w/texmf-var"})
    tools = manuals.Tools() if a.tools else NoTools()
    r = manuals.loop(RUN, ["pdflatex", "-no-shell-escape", "-interaction=nonstopmode", a.main],
                     tools, env, job, base, ist, log)
    pdf = RUN / f"{job}.pdf"
    res = {**r, "log": log, "errors": manuals.errors_in(RUN / f"{job}.log")}
    if r["status"] == "timeout":
        res.update(status="dropped", reason="the oracle timed out")
    elif not pdf.exists():
        res.update(status="dropped", reason="the oracle fails: " + manuals.why_failed(RUN, job, RUN / f"term-pass{r['passes']}.txt"))
    elif not r.get("settled"):
        res.update(status="dropped", reason=f"the oracle does not settle in {PASSES} passes")
    elif res["errors"]:
        res.update(status="dropped", reason=f"the oracle has {res['errors']} errors: " + manuals.why_failed(RUN, job, RUN / f"term-pass{r['passes']}.txt"))
    else:
        res["status"] = "ok"
    if pdf.exists() and a.tag == "oracle":
        shutil.copy2(pdf, out / "oracle.pdf")
        lt = (RUN / f"{job}.log").read_bytes() if (RUN / f"{job}.log").exists() else b""
        m = re.search(rb"Output written on .*?\((\d+) pages?", lt, re.S)
        res["pages"] = int(m.group(1)) if m else pdf_pages(pdf)
        res["pdf_sha256"] = common.sha256_file(pdf)
    if (RUN / f"{job}.log").exists() and a.tag == "oracle":
        shutil.copy2(RUN / f"{job}.log", out / "oracle.log")
    res["pass_walls"] = [x["wall_s"] for x in log if x["cmd"].startswith("pass")]
    jdump(out / f"{a.tag}.json", res)
    print(json.dumps({k: res.get(k) for k in ("status", "reason", "passes", "wall_s", "pages", "errors")}))


# -- PhiTeX -----------------------------------------------------------------

def phitex_env(extra=None):
    e = {"HOME": "/w/home", "TEXMFVAR": "/w/texmf-var", "PHITEX_CACHE_DIR": "/w/cache",
         "NO_COLOR": "1"}
    e.update(extra or {})
    return common.env(e)


def children_of(pid):
    """The pids under `pid` (the CLI must spawn none)."""
    kids = set()
    try:
        for t in os.listdir(f"/proc/{pid}/task"):
            try:
                kids.update(int(x) for x in Path(f"/proc/{pid}/task/{t}/children").read_text().split())
            except OSError:
                pass
    except OSError:
        pass
    return kids


def summarize_stderr(path):
    """PHITEX_TEX_TIME's lines: per-op node time, rounds, serial pieces."""
    ops, rounds, other = {}, [], []
    try:
        f = open(path, errors="replace")
    except OSError:
        return {}
    with f:
        for line in f:
            if line.startswith("nodetime: "):
                w = line.split()
                if len(w) >= 4:
                    o = ops.setdefault(w[1], [0, 0.0, 0.0])
                    ms = float(w[3])
                    o[0] += 1
                    o[1] += ms
                    o[2] = max(o[2], ms)
            elif line.startswith("round: "):
                rounds.append(line.strip())
            elif len(other) < 400:
                other.append(line.rstrip()[:600])
    return {"ops": {k: {"runs": v[0], "ms": round(v[1], 3), "max_ms": round(v[2], 3)} for k, v in ops.items()},
            "rounds": rounds, "lines": other}


def watch(binary, threads, main, timeout, edit=None, tex_time=False, tag="run", exact=False, save_cold=None):
    """One `phitex watch` session in RUN: the cold build, and with `edit`
    (file, old, new) an edit of that file after it. Returns the events
    and timings."""
    stderr_p = Path("/w/out") / f"stderr-{tag}.txt"
    env = phitex_env({"PHITEX_TEX_TIME": "1"} if tex_time else None)
    cmd = [binary, "watch", "--no-view", "--json", *(["--exact-pdf"] if exact else []), "-j", str(threads), main]
    t0 = time.monotonic()
    # (cold: the document's part of the cache goes; the formats stay)
    shutil.rmtree("/w/cache/docs", ignore_errors=True)
    p = subprocess.Popen(cmd, cwd=RUN, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=open(stderr_p, "wb"), preexec_fn=common._limits(int(32 * timeout + 60), None))
    events, builds, spawned = [], [], set()
    res = {"threads": threads, "mode": "exact" if exact else "relaxed", "timeout": False}
    deadline = t0 + timeout
    import selectors
    sel = selectors.DefaultSelector()
    sel.register(p.stdout, selectors.EVENT_READ)
    want = 2 if edit else 1
    buf = b""
    edited_at = None
    while len(builds) < want:
        spawned |= children_of(p.pid)
        if time.monotonic() > deadline:
            res["timeout"] = True
            break
        if not sel.select(timeout=0.2):
            if p.poll() is not None:
                break
            continue
        chunk = os.read(p.stdout.fileno(), 1 << 16)
        if not chunk:
            break
        buf += chunk
        while b"\n" in buf:
            line, buf = buf.split(b"\n", 1)
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            ev["_t"] = round(time.monotonic() - t0, 4)
            if ev.get("event") in ("build", "failed"):
                ev["notes"] = [n for n in (ev.get("notes") or []) if "PDF objects" in n or "exact" in n][:5]
                builds.append(ev)
                if len(builds) == 1:
                    res["cold_process_s"] = ev["_t"]
                    if save_cold and ev.get("output"):
                        jp = RUN / ev["output"] if not str(ev["output"]).startswith("/") else Path(ev["output"])
                        if jp.exists():
                            shutil.copy2(jp, save_cold)
                    if edit and ev.get("event") == "build":
                        f, old, new = edit
                        time.sleep(0.5)
                        res["stderr_offset_before_edit"] = stderr_p.stat().st_size
                        edited_at = time.monotonic()
                        (RUN / f).write_bytes(new)
                    elif edit:
                        want = 1
                elif edited_at is not None:
                    res["edit_process_s"] = round(time.monotonic() - edited_at, 4)
            elif ev.get("event") in ("error", "warning", "note", "cache") and len(events) < 200:
                events.append(ev)
    if res["timeout"]:
        try:
            os.killpg(p.pid, 9)
        except ProcessLookupError:
            pass
    try:
        p.stdin.close()
    except OSError:
        pass
    ru, status, end = None, 0, time.monotonic() + 60
    while True:
        pid, status, ru = os.wait4(p.pid, os.WNOHANG)
        if pid:
            break
        if time.monotonic() > end:
            try:
                os.killpg(p.pid, 9)
            except ProcessLookupError:
                pass
            pid, status, ru = os.wait4(p.pid, 0)
            break
        time.sleep(0.05)
    p.returncode = os.waitstatus_to_exitcode(status)
    res.update(exit=p.returncode, total_s=round(time.monotonic() - t0, 3), rss_kib=ru.ru_maxrss,
               user_s=round(ru.ru_utime, 3), sys_s=round(ru.ru_stime, 3), builds=builds, events=events,
               spawned=sorted(spawned))
    return res


def pick_edit(doc, main):
    """A word in a mid-document prose line: the file (the main file or one
    it inputs, the one with the most prose), the old and the new bytes."""
    files = [main]
    try:
        mt = (doc / main).read_text(errors="replace")
    except OSError:
        return None
    for m in re.finditer(r"\\(?:input|include|subfile)\s*\{([^}]+)\}", uncomment(mt)):
        n = m.group(1).strip()
        for c in (n, n + ".tex"):
            if (doc / c).is_file() and c not in files:
                files.append(c)
                break
    best = None
    for f in files:
        raw = (doc / f).read_bytes()
        text = raw.decode("utf-8", errors="replace")
        lines = text.split("\n")
        start = next((i for i, l in enumerate(lines) if "\\begin{document}" in l), -1) + 1 if f == main else 0
        end = next((i for i, l in enumerate(lines) if "\\end{document}" in l), len(lines)) if f == main else len(lines)
        prose, env_depth = [], 0
        for i in range(start, end):
            l = lines[i]
            if re.search(r"\\begin\{(verbatim|lstlisting|equation|align|eqnarray|tabular|figure|table|thebibliography|tikzpicture)", l):
                env_depth += 1
            if re.search(r"\\end\{(verbatim|lstlisting|equation|align|eqnarray|tabular|figure|table|thebibliography|tikzpicture)", l):
                env_depth = max(0, env_depth - 1)
                continue
            if env_depth == 0 and len(l) > 50 and "\\" not in l and "%" not in l and "$" not in l \
                    and "{" not in l and l.isascii() and re.search(r"\b[a-z]{5,}\b", l):
                prose.append(i)
        if prose and (best is None or len(prose) > len(best[1])):
            best = (f, prose, lines, raw)
    if not best:
        return None
    f, prose, lines, raw = best
    i = prose[len(prose) // 2]
    m = re.search(r"\b([a-z]{5,})\b", lines[i])
    w = m.group(1)
    nw = ("b" if w[0] != "b" else "c") + w[1:]
    lines[i] = lines[i][: m.start()] + nw + lines[i][m.end():]
    new = "\n".join(lines).encode()
    return {"file": f, "line": i + 1, "old_word": w, "new_word": nw, "new": new}


def first_diff(a, b):
    try:
        x, y = Path(a).read_bytes(), Path(b).read_bytes()
    except OSError:
        return None
    if x == y:
        return -1
    n = min(len(x), len(y))
    for i in range(0, n, 4096):
        if x[i:i + 4096] != y[i:i + 4096]:
            for j in range(i, min(i + 4096, n)):
                if x[j] != y[j]:
                    return j
    return n


def cmd_phitex(a):
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    doc = Path(a.doc)
    job = Path(a.main).stem
    binary = a.binary
    res = {"runs": []}
    # the format, built once into the cache (/w/cache, kept for the shard)
    if not Path("/w/cache/.warm").exists():
        if RUN.exists():
            shutil.rmtree(RUN)
        RUN.mkdir(parents=True)
        (RUN / "warm.tex").write_text("\\documentclass{article}\\begin{document}warm\\end{document}\n")
        w = watch(binary, 8, "warm.tex", 900, tag="warm")
        res["warm"] = {k: w.get(k) for k in ("total_s", "exit")}
        Path("/w/cache/.warm").write_text("")
    timeout = a.timeout
    threads = [int(t) for t in a.threads.split(",")]
    for mode in ("exact", "relaxed"):
        for n in threads:
            fresh_run(doc, job)
            r = watch(binary, n, a.main, timeout, tag=f"{mode}-j{n}", exact=mode == "exact")
            pdf = RUN / f"{job}.pdf"
            if pdf.exists():
                shutil.copy2(pdf, out / f"phitex-{mode}-j{n}.pdf")
                r["pdf_sha256"] = common.sha256_file(pdf)
                if a.oracle_pdf:
                    r["first_diff"] = first_diff(a.oracle_pdf, pdf)
                    if mode == "relaxed" and Path("/t/pdf-relaxed-eq").exists():
                        q = subprocess.run(["/t/pdf-relaxed-eq", a.oracle_pdf, str(pdf)], capture_output=True,
                                           timeout=600)
                        r["relaxed_eq"] = q.stdout.decode(errors="replace").strip()[:2000]
                        r["relaxed_eq_exit"] = q.returncode
            if (RUN / f"{job}.log").exists():
                shutil.copy2(RUN / f"{job}.log", out / f"phitex-{mode}-j{n}.log")
            res["runs"].append(r)
            if r["timeout"]:
                break
    # the timed session: PHITEX_TEX_TIME on, the cold build then one edit
    if a.edit and not any(r["timeout"] for r in res["runs"] if r["mode"] == "relaxed"):
        e = pick_edit(doc, a.main)
        n = threads[-1]
        fresh_run(doc, job)
        if e:
            res["edit"] = {k: v for k, v in e.items() if k != "new"}
            r = watch(binary, n, a.main, 2 * timeout, edit=(e["file"], None, e["new"]), tex_time=True, tag="timed")
            pdf = RUN / f"{job}.pdf"
            if pdf.exists():
                r["edited_pdf_sha256"] = common.sha256_file(pdf)
        else:
            res["edit"] = {"none": "no prose line found"}
            r = watch(binary, n, a.main, timeout, tex_time=True, tag="timed")
        r["stderr"] = summarize_stderr(out / "stderr-timed.txt")
        off = r.get("stderr_offset_before_edit")
        if off is not None:
            p = out / "stderr-timed.txt"
            with open(p, "rb") as f:
                f.seek(off)
                (out / "stderr-edit.txt").write_bytes(f.read())
            r["stderr_edit"] = summarize_stderr(out / "stderr-edit.txt")
            (out / "stderr-edit.txt").unlink()
        res["timed"] = r
        (out / "stderr-timed.txt").unlink(missing_ok=True)
    jdump(out / "phitex.json", res)
    print(json.dumps({"runs": [{"m": r["mode"], "j": r["threads"], "s": r.get("cold_process_s"), "diff": r.get("first_diff"),
                                "exit": r["exit"], "timeout": r["timeout"]} for r in res["runs"]]}))


def cmd_diag(a):
    """`phitex build -v` once (exact, one thread): its terminal output
    (the errors at their file:line with TeX's context), and the source
    lines around the first error places, for the ranking's repros."""
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    doc = Path(a.doc)
    job = Path(a.main).stem
    fresh_run(doc, job)
    shutil.rmtree("/w/cache/docs", ignore_errors=True)
    r = common.run([a.binary, "build", "-v", "--exact-pdf", "-j", str(a.threads), a.main], RUN,
                   phitex_env({"COLUMNS": "200"}), a.timeout, out=out / "diag-term.txt",
                   cpu_s=int(4 * a.timeout + 60))
    term = (out / "diag-term.txt").read_bytes()[:400000]
    (out / "diag-term.txt").write_bytes(term)
    text = term.decode("utf-8", errors="replace")
    places = []
    for m in re.finditer(r"([\w./-]+\.(?:tex|sty|cls|bbl|def|cfg|clo|ltx)):(\d+)", text):
        k = (m.group(1), int(m.group(2)))
        if k not in places:
            places.append(k)
    snips = []
    for f, line in places[:6]:
        p = RUN / f
        if not p.is_file():
            continue
        ls = p.read_bytes().decode("utf-8", errors="replace").split("\n")
        lo, hi = max(0, line - 9), min(len(ls), line + 4)
        snips.append(f"--- {f}:{line}\n" + "\n".join(f"{i + 1:6d}  {ls[i][:300]}" for i in range(lo, hi)))
    (out / "diag-snippets.txt").write_text("\n".join(snips))
    pdf = RUN / f"{job}.pdf"
    if pdf.exists():
        shutil.copy2(pdf, out / "diag.pdf")
        if Path("/in/oracle.pdf").exists():
            r["first_diff"] = first_diff("/in/oracle.pdf", pdf)
    r["errors"] = [l for l in text.splitlines() if l.startswith("error: ")][:30]
    jdump(out / "diag.json", {**r, "places": places[:20]})
    print(json.dumps({**r, "places": places[:6]}))


def qdf_lines(p, out):
    r = subprocess.run(["qpdf", "--qdf", "--object-streams=disable", "--decode-level=generalized", p, out],
                       capture_output=True)
    if not Path(out).exists():
        return None
    return Path(out).read_bytes().split(b"\n")


def pdfdiff(want, got, tag):
    """The first difference of two PDFs: the byte, and in qpdf's QDF form
    of both, the first differing line with its object and page."""
    res = {"first_byte": first_diff(want, got)}
    if res["first_byte"] in (-1, None):
        return res
    a = qdf_lines(want, f"/w/qdf-{tag}-want.pdf")
    b = qdf_lines(got, f"/w/qdf-{tag}-got.pdf")
    if a is None or b is None:
        res["qdf"] = "qpdf failed"
        return res
    i = next((k for k in range(min(len(a), len(b))) if a[k] != b[k]), min(len(a), len(b)))
    res["qdf_line"] = i + 1
    res["want"] = a[i][:300].decode("latin-1") if i < len(a) else "<end>"
    res["got"] = b[i][:300].decode("latin-1") if i < len(b) else "<end>"
    obj = page = None
    for k in range(min(i, len(a) - 1), -1, -1):
        line = a[k]
        if obj is None and re.match(rb"^\d+ 0 obj", line):
            obj = line.decode("latin-1")
        if page is None and line.startswith(b"%% ") and b"age" in line:
            page = line.decode("latin-1")
        if obj and page:
            break
    res["object"] = obj
    res["page_comment"] = page
    lo = max(0, i - 6)
    res["want_context"] = [x[:200].decode("latin-1") for x in a[lo:i + 4]]
    res["got_context"] = [x[:200].decode("latin-1") for x in b[lo:i + 4]]
    res["lines_want_got"] = [len(a), len(b)]
    return res


def cmd_target(a):
    """One document, the iteration target: a cold build and a one-word
    edit in one exact-mode watch session; each PDF compared with
    pdflatex's (the edit's: pdflatex on the edited text) and the first
    difference located."""
    import manuals
    out = Path(a.out)
    out.mkdir(parents=True, exist_ok=True)
    require_no_net(out)
    doc = Path(a.doc)
    job = Path(a.main).stem
    e = pick_edit(doc, a.main)
    res = {"edit": {k: v for k, v in (e or {}).items() if k != "new"}}
    fresh_run(doc, job)
    r = watch(a.binary, a.threads, a.main, a.timeout, edit=(e["file"], None, e["new"]) if e else None,
              exact=True, tag="target", save_cold=str(out / "cold.pdf"))
    if (RUN / f"{job}.pdf").exists() and e:
        shutil.copy2(RUN / f"{job}.pdf", out / "edit.pdf")
    if (RUN / f"{job}.log").exists():
        shutil.copy2(RUN / f"{job}.log", out / "phitex.log")
    res["watch"] = r
    if Path("/in/oracle.pdf").exists() and (out / "cold.pdf").exists():
        res["cold_diff"] = pdfdiff("/in/oracle.pdf", str(out / "cold.pdf"), "cold")
    if e:
        # pdflatex on the edited text
        ed = Path("/w/doc-edited")
        shutil.rmtree(ed, ignore_errors=True)
        shutil.copytree(doc, ed)
        (ed / e["file"]).write_bytes(e["new"])
        fresh_run(ed, job)
        base = {rel: common.sha256_file(RUN / rel) for rel in common.tree_files(RUN)}
        env = common.env({"HOME": "/w/home", "TEXMFVAR": "/w/texmf-var"})
        lr = manuals.loop(RUN, ["pdflatex", "-no-shell-escape", "-interaction=nonstopmode", a.main],
                          manuals.Tools() if a.tools else NoTools(), env, job, base,
                          {k for k in base if k.endswith(".ist")}, [])
        res["edit_oracle"] = {k: lr.get(k) for k in ("passes", "wall_s", "settled", "status")}
        if (RUN / f"{job}.pdf").exists():
            shutil.copy2(RUN / f"{job}.pdf", out / "edit-oracle.pdf")
            if (out / "edit.pdf").exists():
                res["edit_diff"] = pdfdiff(str(out / "edit-oracle.pdf"), str(out / "edit.pdf"), "edit")
    jdump(out / "target.json", res)
    b = r["builds"]
    print(json.dumps({"cold_s": r.get("cold_process_s"), "edit_s": r.get("edit_process_s"),
                      "builds": [(x.get("why"), x.get("pages"), round(x.get("wall_ms") or 0), len(x.get("errors") or [])) for x in b],
                      "cold_diff": (res.get("cold_diff") or {}).get("first_byte"),
                      "edit_diff": (res.get("edit_diff") or {}).get("first_byte")}))


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    sub.add_parser("netcheck")
    p = sub.add_parser("prepare")
    p.add_argument("--src", required=True)
    p.add_argument("--doc", required=True)
    p.add_argument("--out", required=True)
    p = sub.add_parser("target")
    p.add_argument("--doc", required=True)
    p.add_argument("--main", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--binary", default="/t/phitex")
    p.add_argument("--threads", type=int, default=8)
    p.add_argument("--timeout", type=float, default=600)
    p.add_argument("--tools", action="store_true")
    p = sub.add_parser("diag")
    p.add_argument("--doc", required=True)
    p.add_argument("--main", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--binary", default="/t/phitex")
    p.add_argument("--timeout", type=float, default=600)
    p.add_argument("--threads", type=int, default=1)
    p = sub.add_parser("prepare-local")
    p.add_argument("--main", required=True)
    p.add_argument("--doc", required=True)
    p.add_argument("--out", required=True)
    p = sub.add_parser("prepare-manual")
    p.add_argument("--dir", required=True)
    p.add_argument("--main", required=True)
    p.add_argument("--doc", required=True)
    p.add_argument("--out", required=True)
    p = sub.add_parser("oracle")
    p.add_argument("--doc", required=True)
    p.add_argument("--main", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--tools", action="store_true")
    p.add_argument("--tag", default="oracle")
    p = sub.add_parser("phitex")
    p.add_argument("--doc", required=True)
    p.add_argument("--main", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--binary", default="/t/phitex")
    p.add_argument("--threads", default="1,8")
    p.add_argument("--timeout", type=float, default=900)
    p.add_argument("--edit", action="store_true")
    p.add_argument("--oracle-pdf")
    a = ap.parse_args()
    if a.cmd == "netcheck":
        n = netcheck()
        print(json.dumps(n))
        sys.exit(0 if n["ok"] else 90)
    {"prepare": cmd_prepare, "prepare-manual": cmd_prepare_manual, "prepare-local": cmd_prepare_local, "diag": cmd_diag, "target": cmd_target, "oracle": cmd_oracle, "phitex": cmd_phitex}[a.cmd](a)


if __name__ == "__main__":
    main()
