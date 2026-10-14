#!/usr/bin/env python3
"""The TikZ manual as a standing big-document row (agent tikzman).

Runs inside the CI image on accl (`accl submit cmd python3
scripts/accl/tikzman.py ...`; $w and $r from job.sbatch):

    tikzman.py build           release CLI -> $r/phitex
    tikzman.py oracle          the oracle (the harness, scripts/ci/
                               manuals.py) of the manual cold and after
                               the edit -> $r/oracle
    tikzman.py run BIN ORACLE N[,N...] [--format-cold] [--no-edit]
                               phitex watch per thread count: cold, then
                               the one edit; stats -> $r/runs.json

Both sides get the harness's pinned environment (SOURCE_DATE_EPOCH etc.).
Nothing here is committed with outputs: results stay on accl and in
~/code/flinner/phitex-runs.
"""

import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "ci"))
import common  # noqa: E402

DOC = Path(os.environ.get("TIKZMAN_DOC", "/usr/share/doc/texlive"))
REL = "generic/pgf"
MAIN = "pgfmanual.tex"
ID = "generic_pgf_pgfmanual"
EDIT_FILE = os.environ.get("TIKZMAN_EDIT_FILE", "pgfmanual-en-tikz-shapes.tex")
W = Path(os.environ.get("w", "/tmp/tikzman"))
R = Path(os.environ.get("r", W / "results"))
NORM = os.environ.get("TIKZMAN_PDFNORM")
WATCHDOG = 60 * int(os.environ.get("TIKZMAN_WATCHDOG_MIN", "50"))


def edit(tree):
    """One word in a mid-file paragraph of EDIT_FILE: the first word of
    five or more letters on the first plain text line past the file's
    middle, its letters reversed. Returns (line number, old, new)."""
    p = Path(tree) / EDIT_FILE
    lines = p.read_bytes().split(b"\n")
    for k in range(len(lines) // 2, len(lines)):
        l = lines[k]
        if len(l) > 40 and re.match(rb"^[A-Za-z]", l) and b"\\" not in l and b"%" not in l and b"|" not in l:
            m = re.search(rb"\b([a-z]{5,})\b", l)
            if m:
                old = m.group(1)
                new = old[::-1]
                lines[k] = l[: m.start(1)] + new + l[m.end(1):]
                p.write_bytes(b"\n".join(lines))
                return k + 1, old.decode(), new.decode()
    raise SystemExit("no line to edit")


def include_to_input(tree):
    """The manual's chapters read with \\input between \\clearpages
    instead of \\include (label "include->input"): no per-chapter .aux."""
    p = Path(tree) / "pgfmanual-en-main-body.tex"
    t = p.read_bytes()
    t2 = re.sub(rb"\\include\{([^}]*)\}", rb"\\clearpage\\input{\1}\\clearpage", t)
    p.write_bytes(t2)
    return t.count(b"\\include{")


I2I = os.environ.get("TIKZMAN_INCLUDE_TO_INPUT") == "1"
CHAPTER = os.environ.get("TIKZMAN_CHAPTER")


def one_chapter(tree, ch):
    """"pgfman-1ch": the manual's preamble and front matter (the main
    body up to its first chapter), then ONE chapter read with \\input,
    then the index and the end."""
    p = Path(tree) / "pgfmanual-en-main-body.tex"
    t = p.read_bytes()
    k = t.index(b"\\include{")
    p.write_bytes(t[:k] + b"\\clearpage\\input{" + ch.encode() + b"}\\clearpage\n\\printindex\n\\end{document}\n")


def prepare_tree(tree):
    """The variants: one chapter, or every chapter \\input."""
    if CHAPTER:
        one_chapter(tree, CHAPTER)
    elif I2I:
        include_to_input(tree)


class Stderr:
    """A child's standard error: `nodetime:` lines folded (per operation,
    per node: runs per node, the longest single runs), every other line
    written with the seconds since `t0`; and the child's resident set
    every 10 s (killed past `rss_limit_kib`)."""

    def __init__(self, p, path, t0, rss_limit_kib=None):
        import threading
        self.p, self.t0 = p, t0
        self.f = open(path, "w")
        self.ops, self.nodes, self.longest = {}, {}, []
        self.peak = 0
        self.killed = None
        self.lock = threading.Lock()
        self.th = threading.Thread(target=self.drain)
        self.th.start()
        self.rss_limit = rss_limit_kib
        self.rt = threading.Thread(target=self.rss, daemon=True)
        self.rt.start()

    def stamp(self, text):
        with self.lock:
            self.f.write(f"{time.monotonic() - self.t0:9.3f} {text}\n")
            self.f.flush()

    def drain(self):
        for l in self.p.stderr:
            if l.startswith(b"nodetime: "):
                parts = l.split()
                if len(parts) >= 4:
                    op, node, ms = parts[1].decode(), parts[2].decode(), float(parts[3])
                    c = self.ops.setdefault(op, [0, 0.0, 0.0])
                    c[0] += 1
                    c[1] += ms
                    c[2] = max(c[2], ms)
                    k = self.nodes.setdefault(node, [op, 0, 0.0])
                    k[1] += 1
                    k[2] += ms
                    self.longest.append((ms, op, node))
                    if len(self.longest) > 200:
                        self.longest.sort(reverse=True)
                        del self.longest[20:]
                continue
            self.stamp(l.decode("utf-8", "replace").rstrip())

    def rss(self):
        while self.p.poll() is None:
            time.sleep(10)
            try:
                st = Path(f"/proc/{self.p.pid}/status").read_text()
            except OSError:
                return
            kib = next((int(x.split()[1]) for x in st.splitlines() if x.startswith("VmRSS:")), 0)
            self.peak = max(self.peak, kib)
            self.stamp(f"rss-watch: {kib} kB")
            if self.rss_limit and kib > self.rss_limit:
                self.killed = f"rss {kib} kB > {self.rss_limit} kB"
                self.stamp(f"rss-watch: killing ({self.killed})")
                self.p.kill()
                return

    def summary(self):
        self.th.join()
        self.f.close()
        self.longest.sort(reverse=True)
        top = sorted(self.nodes.items(), key=lambda kv: -kv[1][1])[:20]
        total_runs = sum(v[0] for v in self.ops.values())
        return {
            "nodetime": {k: {"n": v[0], "sum_ms": round(v[1], 1), "max_ms": round(v[2], 1)}
                         for k, v in sorted(self.ops.items(), key=lambda kv: -kv[1][1])},
            "runs_timed": total_runs,
            "nodes_timed": len(self.nodes),
            "runs_per_node": round(total_runs / max(1, len(self.nodes)), 3),
            "top_runs_per_node": [{"node": k, "op": v[0], "runs": v[1], "sum_ms": round(v[2], 1)} for k, v in top],
            "longest_runs": [{"ms": round(ms, 1), "op": op, "node": node} for ms, op, node in self.longest[:10]],
            "rss_peak_kib_watch": self.peak,
            "killed": self.killed,
        }


def sh(cmd, **kw):
    print("+", cmd, flush=True)
    return subprocess.run(cmd, shell=True, check=True, **kw)


def build():
    R.mkdir(parents=True, exist_ok=True)
    t = time.monotonic()
    sh("cargo build --release -q -p phitex-cli")
    print(f"cargo build {time.monotonic() - t:.1f}s", flush=True)
    shutil.copy2("target/release/phitex", R / "phitex")


def oracle():
    """The two oracle builds (cold, edited) side by side, one core each."""
    R.mkdir(parents=True, exist_ok=True)
    sh(f"lscpu > {R}/lscpu.txt; pdflatex --version | head -2 >> {R}/lscpu.txt")
    # the oracle: the harness on the doc tree, and on a copy edited
    edited = W / "doc-edit"
    shutil.copytree(DOC / REL, edited / REL, symlinks=True)
    cold = DOC
    if I2I or CHAPTER:
        cold = W / "doc-cold"
        shutil.copytree(DOC / REL, cold / REL, symlinks=True)
        prepare_tree(cold / REL)
        prepare_tree(edited / REL)
    where = edit(edited / REL)
    (R / "edit.json").write_text(json.dumps({"file": EDIT_FILE, "line": where[0], "old": where[1], "new": where[2]}))
    cand = {"id": ID, "path": f"{REL}/{MAIN}", "dir": REL, "main": MAIN, "engine": "pdflatex"}
    lst = W / "list.json"
    lst.write_text(json.dumps({"candidates": [cand]}))
    harness = Path(__file__).resolve().parent.parent / "ci" / "manuals.py"
    ps = []
    for name, doc in (("cold", cold), ("edit", edited)):
        cache = W / f"oracle-{name}"
        env = dict(os.environ, CI_WORK=str(W / f"ci-{name}"))
        (W / f"ci-{name}").mkdir(exist_ok=True)
        cmd = ["python3", str(harness), "oracle", "--list", str(lst), "--doc", str(doc), "--cache", str(cache)]
        ps.append((name, cache, time.monotonic(), subprocess.Popen(cmd, env=env)))
    for name, cache, t, p in ps:
        p.wait()
        print(f"oracle {name} exit {p.returncode} {time.monotonic() - t:.1f}s", flush=True)
        shutil.copytree(cache, R / "oracle" / name, dirs_exist_ok=True)


def npages(pdf):
    try:
        return int(subprocess.run(["qpdf", "--show-npages", str(pdf)], capture_output=True, text=True).stdout.strip())
    except (ValueError, OSError):
        return None


def compare(a, b):
    """None when byte-identical, else the first differing byte and sizes."""
    try:
        x, y = Path(a).read_bytes(), Path(b).read_bytes()
    except OSError as e:
        return {"missing": str(e)}
    if x == y:
        return None
    n = min(len(x), len(y))
    i = next((k for k in range(n) if x[k] != y[k]), n)
    return {"first_diff": i, "size": len(x), "oracle_size": len(y),
            "ours": x[max(0, i - 40): i + 40].decode("latin-1"),
            "oracle": y[max(0, i - 40): i + 40].decode("latin-1")}


def first_error(log):
    try:
        t = Path(log).read_text("latin-1")
    except OSError:
        return None
    i = t.find("\n! ")
    if i < 0:
        return None
    seg = t[i + 1: i + 1200]
    # the innermost file open before the error: the last unmatched "(" name
    opened = re.findall(r"\(([./A-Za-z0-9_-]+\.(?:tex|sty|code\.tex|cfg|def))", t[:i])
    return {"text": seg, "file": opened[-1] if opened else None,
            "line": (re.search(r"\nl\.(\d+)", seg) or [None, None])[1]}


def stderr_summary(path):
    ops, other = {}, []
    with open(path, "rb") as f:
        for l in f:
            if l.startswith(b"nodetime: "):
                parts = l.split()
                if len(parts) >= 4:
                    op = parts[1].decode()
                    ms = float(parts[3])
                    c = ops.setdefault(op, [0, 0.0, 0.0])
                    c[0] += 1
                    c[1] += ms
                    c[2] = max(c[2], ms)
            elif len(other) < 400:
                other.append(l.decode("utf-8", "replace").rstrip())
    return {"nodetime": {k: {"n": v[0], "sum_ms": round(v[1], 1), "max_ms": round(v[2], 1)}
                         for k, v in sorted(ops.items(), key=lambda kv: -kv[1][1])},
            "other": other}


def one(binary, oracle, n, cache, format_cold, do_edit, tag, mode="exact"):
    run = W / f"run-{tag}"
    if run.exists():
        shutil.rmtree(run)
    m = run / "m"
    shutil.copytree(DOC / REL, m, symlinks=True)
    prepare_tree(m)
    if format_cold and (cache / "formats").exists():
        shutil.rmtree(cache / "formats")
    if (cache / "docs").exists():
        shutil.rmtree(cache / "docs")
    (W / "home").mkdir(exist_ok=True)
    env = common.env({"HOME": str(W / "home"), "TEXMFVAR": str(W / "texmf-var"),
                      "PHITEX_CACHE_DIR": str(cache), "NO_COLOR": "1", "PHITEX_TEX_TIME": "1",
                      "TIKZMAN_FIRST_LIMIT_MIN": os.environ.get("TIKZMAN_FIRST_LIMIT_MIN", "30"),
                      "PHITEX_SNAPSHOT_SECS": os.environ.get("PHITEX_SNAPSHOT_SECS", "60")})
    err = R / f"stderr-{tag}.txt"
    res = {"tag": tag, "threads": n, "format_cold": format_cold, "mode": mode,
           "include_to_input": I2I, "chapter": CHAPTER, "builds": []}
    t0 = time.monotonic()
    p = subprocess.Popen([str(binary), "watch", "--no-view", "--json", *(["--exact-pdf"] if mode == "exact" else []), "-j", str(n), MAIN],
                         cwd=m, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE)
    sink = Stderr(p, err, t0)
    import threading
    dog = threading.Timer(WATCHDOG, p.kill)
    dog.start()
    edited_at = None
    out = open(R / f"stdout-{tag}.jsonl", "wb")
    deadline = t0 + 50 * 60
    for line in p.stdout:
        now = time.monotonic()
        out.write(line)
        try:
            ev = json.loads(line)
        except ValueError:
            continue
        e = ev.get("event", {}) if ev.get("kind") == "event" else ev
        if not isinstance(e, dict) or e.get("event") != "build":
            continue
        e = dict(e)
        e["t_s"] = round(now - t0, 3)
        if edited_at is not None:
            e["since_edit_s"] = round(now - edited_at, 3)
        pdf = m / "pgfmanual.pdf"
        k = len(res["builds"])
        e["pdf_pages_qpdf"] = npages(pdf)
        ref = oracle / ("cold" if k == 0 else "edit")
        refpdf = next(ref.glob(f"{ID}-*/files/pgfmanual.pdf"), None)
        e["compare"] = compare(pdf, refpdf) if refpdf else {"missing": "oracle pdf"}
        if mode == "relaxed" and refpdf and NORM and pdf.exists():
            q = subprocess.run([NORM, str(pdf), str(refpdf)], capture_output=True, text=True)
            e["compare_relaxed"] = q.stdout.strip()[:600]
        e["oracle_pages"] = npages(refpdf) if refpdf else None
        e["first_error"] = first_error(m / "pgfmanual.log")
        try:
            lt = (m / "pgfmanual.log").read_text("latin-1")
            e["log_errors"] = sum(1 for l in lt.splitlines() if l.startswith("! "))
            e["log_warnings"] = len(re.findall(r"Warning", lt))
        except OSError:
            pass
        if pdf.exists():
            shutil.copy2(pdf, R / f"pgfmanual-{tag}-{k}.pdf")
        if (m / "pgfmanual.log").exists():
            shutil.copy2(m / "pgfmanual.log", R / f"pgfmanual-{tag}-{k}.log")
        res["builds"].append(e)
        print(f"[{tag}] build {k}: {e['t_s']}s wall_ms={e.get('wall_ms')} pages={e.get('pages')} "
              f"errors={len(e.get('errors') or [])} same={e['compare'] is None}", flush=True)
        if k == 0 and do_edit and not e.get("errors") and e.get("pages"):
            time.sleep(1)
            edited_at = time.monotonic()
            res["edit"] = edit(m)
            # (no build within 10 minutes of the edit: recorded, quit)
            dog.cancel()
            dog = threading.Timer(600, p.kill)
            dog.start()
        else:
            break
        if now > deadline:
            break
    if do_edit and "edit" not in res:
        res["edit"] = "skipped: the cold build failed"
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    try:
        _, status, ru = os.wait4(p.pid, 0)
        res["exit"] = os.waitstatus_to_exitcode(status)
        res["rss_kib"] = ru.ru_maxrss
        res["user_s"], res["sys_s"] = round(ru.ru_utime, 2), round(ru.ru_stime, 2)
    except ChildProcessError:
        p.wait()
        res["exit"] = p.returncode
    dog.cancel()
    res["killed_by_watchdog"] = time.monotonic() - t0 >= WATCHDOG or (
        edited_at is not None and len(res["builds"]) < 2)
    res["total_s"] = round(time.monotonic() - t0, 3)
    res["stderr"] = sink.summary()
    out.close()
    return res


def run(args):
    # ("-": the binary and oracle the job copied in: $w/course, $w/aux)
    binary = W / "course" / "phitex" if args[0] == "-" else Path(args[0]).resolve()
    oracle = W / "aux" / "oracle" if args[1] == "-" else Path(args[1]).resolve()
    ns = [int(x) for x in args[2].split(",")]
    format_cold = "--format-cold" in args
    do_edit = "--no-edit" not in args
    modes = next((a.split("=", 1)[1] for a in args if a.startswith("--modes=")), "exact").split(",")
    R.mkdir(parents=True, exist_ok=True)
    sh(f"lscpu > {R}/lscpu.txt; nproc >> {R}/lscpu.txt; cat /proc/self/status | grep -i cpus_allowed_list >> {R}/lscpu.txt")
    cache = W / "cache"
    results = []
    if format_cold:
        results.append(one(binary, oracle, ns[0], cache, True, False, f"fmtcold-j{ns[0]}"))
        (R / "runs.json").write_text(json.dumps(results, indent=1))
    for n in ns:
        for mode in modes:
            results.append(one(binary, oracle, n, cache, False, do_edit, f"j{n}-{mode}", mode))
            (R / "runs.json").write_text(json.dumps(results, indent=1))
    for x in results:
        for k, b in enumerate(x["builds"]):
            par = b.get("parallel") or {}
            print(f"{x['tag']} build{k}: wall {b.get('wall_ms', 0) / 1e3:.2f}s busy {par.get('busy_ms', 0) / 1e3:.2f}s "
                  f"engine {par.get('engine_ms', 0) / 1e3:.2f}s pages {b.get('pages')} nodes {b.get('nodes_run')} "
                  f"cp {par.get('critical_path')} errors {len(b.get('errors') or [])} rss {x.get('rss_kib')} "
                  f"identical {b['compare'] is None} relaxed {b.get('compare_relaxed')} notes {b.get('notes')}", flush=True)


def phases(args):
    """Any document, timed by phase: `phases BIN SRC MAIN FILE LINE OLD
    NEW N [TAG]`: phitex watch -j N on a copy of SRC (a directory, or a
    .tgz extracted; MAIN `auto`: the .tex with \\documentclass), cold,
    then OLD -> NEW on FILE's LINE (FILE `-`: no edit); stderr lines
    (PHITEX_TEX_TIME, PHITEX_OBS, snapshots) and events stamped with
    seconds since the start; killed past TIKZMAN_RSS_LIMIT_GB."""
    binary, src, main, f, line, old, new, n = args[:8]
    tag = args[8] if len(args) > 8 else "phases"
    binary = Path(binary).resolve()
    R.mkdir(parents=True, exist_ok=True)
    m = W / tag / "m"
    if m.exists():
        shutil.rmtree(m)
    if src.endswith((".tgz", ".gz")) or Path(src).is_file():
        m.mkdir(parents=True)
        subprocess.run(["tar", "xzf", str(Path(src).resolve()), "-C", str(m)], check=False)
    else:
        shutil.copytree(src, m, symlinks=True)
    if main == "auto":
        cands = [q for q in sorted(m.glob("*.tex"))
                 if b"\\documentclass" in q.read_bytes() and b"\\begin{document}" in q.read_bytes()]
        main = cands[0].name if cands else "main.tex"
    env = common.env({"HOME": str(W / "home"), "TEXMFVAR": str(W / "texmf-var"),
                      "PHITEX_CACHE_DIR": str(W / f"pcache-{tag}"), "NO_COLOR": "1",
                      "PHITEX_TEX_TIME": "1", "PHITEX_OBS": "1",
                      "PHITEX_SNAPSHOT_SECS": os.environ.get("PHITEX_SNAPSHOT_SECS", "10")})
    (W / "home").mkdir(exist_ok=True)
    import threading
    t0 = time.monotonic()
    p = subprocess.Popen([str(binary), "watch", "--no-view", "--json", *(["--exact-pdf"] if os.environ.get("TIKZMAN_EXACT") else []), "-j", n, main], cwd=m, env=env,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    lim = os.environ.get("TIKZMAN_RSS_LIMIT_GB")
    sink = Stderr(p, R / f"{tag}-stderr.txt", t0, int(float(lim) * 1024 * 1024) if lim else None)
    sink.stamp(f"===== main {main}, -j {n}")
    dog = threading.Timer(60 * int(os.environ.get("TIKZMAN_WATCHDOG_MIN", "30")), p.kill)
    dog.start()
    builds = 0
    out = open(R / f"{tag}-stdout.jsonl", "w")
    for l in p.stdout:
        now = time.monotonic() - t0
        out.write(f"{now:9.3f} {l.decode('utf-8', 'replace')}")
        out.flush()
        try:
            e = json.loads(l)
        except ValueError:
            continue
        if e.get("event") != "build":
            continue
        builds += 1
        sink.stamp(f"===== build {builds} event: wall_ms {e.get('wall_ms')} nodes_run {e.get('nodes_run')} pages {e.get('pages')} errors {len(e.get('errors') or [])}")
        if builds == 1 and f != "-":
            time.sleep(1)
            q = m / f
            ls = q.read_bytes().split(b"\n")
            k = int(line) - 1
            if old.encode() in ls[k]:
                ls[k] = ls[k].replace(old.encode(), new.encode(), 1)
                sink.stamp(f"===== edit {f}:{line} {old} -> {new}")
                q.write_bytes(b"\n".join(ls))
            else:
                sink.stamp(f"===== edit skipped: {old!r} not on {f}:{line}")
                break
        else:
            break
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    p.wait()
    dog.cancel()
    sink.stamp(f"===== exit {p.returncode}")
    (R / f"{tag}-summary.json").write_text(json.dumps(sink.summary(), indent=1))


if __name__ == "__main__":
    {"build": lambda a: build(), "oracle": lambda a: oracle(), "run": run, "phases": phases}[sys.argv[1]](sys.argv[2:])
