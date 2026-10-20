#!/usr/bin/env python3
"""The edit-time table (scripts/accl/edit-classes runs it in a job).

    edit-classes.py REL_BIN PROF_BIN WORKDIR RESULTS

amsldoc's six edit classes at -j 8 through `watch --json --exact-pdf`:
each edit saved, its build awaited (the wall: save to build event, on
the users' build), then reverted and that build awaited too. Then the
same on the profile build with PHITEX_TEX_TIME and PHITEX_ROUND_ACCT:
the edit's timeline split into analysis (the phases before the
evaluation), engine (the evaluation's wall), op bodies (the runs' time
summed over threads), the PDF link (the link op's runs) and the rest,
and the runs by op. Then para4000's one-word edit, amsldoc's -j 1 md5,
pages and node count, and four -j 8 md5s: all printed as one table.
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

CLASSES = [
    ("one letter", "permissiveness. When", "permissivenesz. When"),
    ("line, re-break", "pagebreak after a given line, as usual.",
     "pagebreak after a given line, as usual. This added sentence makes the paragraph one line longer "
     "than it was before, so the breaks after it move."),
    ("page added", "\\section{Single equations}", "\\clearpage\\mbox{}\\clearpage\n\\section{Single equations}"),
    ("label", "\\label{displays}", "\\label{displayz}"),
    ("heading", "\\section{Single equations}", "\\section{Single equationz}"),
    ("preamble def", "\\newcommand{\\nfn}[1]{\\texttt{#1}}", "\\newcommand{\\nfn}[1]{\\textsf{#1}}"),
]
ENV = dict(os.environ, SOURCE_DATE_EPOCH="1700000000", FORCE_SOURCE_DATE="1", NO_COLOR="1")
WORD = re.compile(r"(?<=[ ])([a-z]{4,12})(?=[ .,;])")


def md5(p):
    try:
        return hashlib.md5(open(p, "rb").read()).hexdigest()
    except OSError:
        return "none"


def fresh(src_dir, d, cache):
    shutil.rmtree(d, ignore_errors=True)
    shutil.copytree(src_dir, d)
    env = dict(ENV, PHITEX_CACHE_DIR=cache)
    return env


class Watch:
    def __init__(self, binary, d, main_, j, env, errf, flags):
        self.err = open(errf, "a")
        self.events = open(errf + ".events.jsonl", "w")
        self.errf = errf
        self.p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", str(j), main_], cwd=d,
                                  env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=self.err)
        self.path = os.path.join(d, main_)

    def build(self, timeout=1800):
        t = time.monotonic()
        for line in self.p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") in ("build", "failed"):
                self.events.write(line.decode("utf-8", "replace"))
                self.events.flush()
                return x, time.monotonic() - t
            if time.monotonic() - t > timeout:
                break
        return None, time.monotonic() - t

    def mark(self, s):
        self.err.write(f"=== {s}\n")
        self.err.flush()

    def put(self, text):
        open(self.path + ".tmp", "w", encoding="latin-1").write(text)
        os.replace(self.path + ".tmp", self.path)
        return time.monotonic()

    def timelines(self):
        return len(re.findall(r"^timeline \(", open(self.errf, encoding="utf-8", errors="replace").read(), re.M))

    def wait_timeline(self, n, timeout=10):
        t = time.monotonic()
        while self.timelines() < n and time.monotonic() - t < timeout:
            time.sleep(0.05)

    def quit(self):
        try:
            self.p.stdin.write(b"q\n")
            self.p.stdin.close()
        except OSError:
            pass
        try:
            self.p.wait(timeout=120)
        except subprocess.TimeoutExpired:
            self.p.kill()


def classes(binary, d, j, env, errf, timed):
    """Per class: (wall s, event) of the edit's build. With `timed` the
    stderr carries a timeline per build (waited for, so the markers fall
    between them)."""
    open(errf, "w").close()
    w = Watch(binary, d, "amsldoc.tex", j, env, errf, ["--exact-pdf"])
    ev0, cold = w.build()
    if timed:
        w.wait_timeline(1)
    orig = open(w.path, encoding="latin-1").read()
    out = []

    def step(text):
        c = w.timelines()
        t = w.put(text)
        ev, _ = w.build()
        wall = time.monotonic() - t
        if timed:
            w.wait_timeline(c + 1)
        return wall, ev

    for name, a, b in CLASSES:
        assert orig.count(a) == 1, name
        w.mark(f"CLASS {name}")
        out.append(step(orig.replace(a, b)))
        w.mark(f"REVERT {name}")
        step(orig)
    w.quit()
    return (cold, ev0), out


def split(errf):
    """The edit builds' timelines by class: {name: dict}."""
    text = open(errf, encoding="utf-8", errors="replace").read()
    res = {}
    for name, _, _ in CLASSES:
        m = re.search(re.escape(f"=== CLASS {name}\n") + r"(.*?)" + re.escape(f"=== REVERT {name}\n"), text, re.S)
        if not m:
            continue
        seg = m.group(1)
        tl = seg[seg.rfind("timeline ("):] if "timeline (" in seg else ""
        r = {"total": None, "analysis": 0.0, "engine": None, "bodies": 0.0, "link": 0.0, "runs": {}}
        mt = re.search(r"timeline \([^)]*\): ([\d.]+) ms", tl)
        if mt:
            r["total"] = float(mt.group(1))
        before = True
        for line in tl.split("\n")[1:]:
            lm = re.match(r"\s+([\d.]+) ms  (.*)", line)
            if lm:
                ms, ph = float(lm.group(1)), lm.group(2)
                if ph.startswith("engine: evaluate"):
                    r["engine"] = ms
                    before = False
                elif before and not ph.startswith("("):
                    r["analysis"] += ms
                continue
            nm = re.match(r"\s+\. runs: (\S+) ×(\d+) \(summed over threads\) ([\d.]+) ms", line)
            if nm:
                k, cnt, ms = nm.group(1), int(nm.group(2)), float(nm.group(3))
                r["runs"][k] = cnt
                r["bodies"] += ms
                if k == "link":
                    r["link"] += ms
        am = re.search(r"acct-edit: wall [\d.]+ ms, \d+ rounds, .*runs by op \[(.*)\]", seg)
        if am:
            r["runs"] = {k: int(v) for k, v in re.findall(r'\("(\w+)", (\d+)\)', am.group(1))}
        res[name] = r
    return res


def cold(binary, src, d, cache, j, log):
    env = fresh(src, d, cache)
    t = time.monotonic()
    with open(log, "w") as f:
        rc = subprocess.run([binary, "build", "--exact-pdf", "-j", str(j), "amsldoc.tex"], cwd=d, env=env,
                            stdout=f, stderr=subprocess.STDOUT, timeout=1800).returncode
    wall = time.monotonic() - t
    pdf = os.path.join(d, "amsldoc.pdf")
    text = open(log, errors="replace").read()
    nodes = re.findall(r"(\d+) nodes run of (\d+) live", text)
    pg = re.findall(r"^(?:finished|failed): .*?(\d+) pages", text, re.M)
    errs = re.findall(r"^error: (.*)", text, re.M)
    if j == 1 and os.path.exists(pdf):
        shutil.copy(pdf, log[:-4] + ".pdf")
    return rc, md5(pdf), pg[-1] if pg else "?", wall, (nodes[-1][1] if nodes else "?"), "; ".join(errs)[:120]


def para_edit(binary, src, d, cache, errf):
    env = fresh(src, d, cache)
    w = Watch(binary, d, "para4000.tex", 8, env, errf, ["--program", "pdftex"])
    w.build()
    text = open(w.path, encoding="latin-1").read()
    ms = [m for m in WORD.finditer(text) if m.start() > len(text) // 3]
    walls = []
    for k in range(3):
        m = ms[len(ms) // 4 * (k + 1) % len(ms)]
        a = m.start(1)
        new = "".join(chr((ord(c) - 97 + 1 + k) % 26 + 97) for c in m.group(1))
        text = text[:a] + new + text[a + len(new):]
        t = w.put(text)
        w.build()
        walls.append(time.monotonic() - t)
    w.quit()
    return walls


def main():
    rel, prof, work, res = sys.argv[1:5]
    os.makedirs(work, exist_ok=True)
    root = os.getcwd()
    ams = os.path.join(root, "bench/amsldoc")
    cache = os.path.join(work, "cache")
    os.makedirs(cache, exist_ok=True)
    # (the format made once, untimed)
    cold(rel, ams, os.path.join(work, "warm"), cache, 8, os.path.join(res, "warm.log"))
    j1 = cold(rel, ams, os.path.join(work, "j1"), cache, 1, os.path.join(res, "ams-j1.log"))
    j8 = [cold(rel, ams, os.path.join(work, f"j8-{k}"), cache, 8, os.path.join(res, f"ams-j8-{k}.log"))
          for k in range(4)]
    d = os.path.join(work, "cls")
    env = fresh(ams, d, cache)
    (cold_rel, ev_cold), walls = classes(rel, d, 8, env, os.path.join(res, "classes-rel.err"), False)
    hist_rel = md5(os.path.join(d, "amsldoc.pdf"))
    d = os.path.join(work, "clsp")
    env = dict(fresh(ams, d, cache), PHITEX_TEX_TIME="1", PHITEX_ROUND_ACCT="1")
    (pcold, pev_cold), pwalls = classes(prof, d, 8, env, os.path.join(res, "classes-prof.err"), True)
    sp = split(os.path.join(res, "classes-prof.err"))
    pd = os.path.join(work, "para")
    shutil.rmtree(pd, ignore_errors=True)
    os.makedirs(pd)
    shutil.copy(os.path.join(root, "bench/para4000.tex"), pd)
    para_src = os.path.join(work, "para-src")
    shutil.rmtree(para_src, ignore_errors=True)
    shutil.copytree(pd, para_src)
    pw = para_edit(rel, para_src, pd, cache, os.path.join(res, "para.err"))

    f = lambda x: "-" if x is None else f"{x:.0f}"
    print(f"commit {subprocess.run(['git', 'rev-parse', '--short=8', 'HEAD'], capture_output=True, text=True).stdout.strip()}"
          f", node {os.uname().nodename}, -j 8 watch --json --exact-pdf")
    print("| class | wall ms | engine ms (event) | ok | pages | prof: total | analysis | engine | op bodies (sum) | link | rest | runs by op |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|")
    for (name, _, _), (wall, ev) in zip(CLASSES, walls):
        s = sp.get(name, {})
        eng = ev and (ev.get("parallel") or {}).get("engine_ms")
        tot, an, en = s.get("total"), s.get("analysis"), s.get("engine")
        rest = tot - an - en if None not in (tot, an, en) else None
        runs = ", ".join(f"{k} {v}" for k, v in sorted(s.get("runs", {}).items(), key=lambda x: -x[1]))
        print(f"| {name} | {wall * 1e3:.0f} | {f(eng)} | {ev and ev.get('ok')} | {ev and ev.get('pages')} | {f(tot)} | {f(an)} | {f(en)} | "
              f"{f(s.get('bodies'))} | {f(s.get('link'))} | {f(rest)} | {runs} |")
    print(f"| para4000 one word (3 edits) | {' / '.join(f'{x * 1e3:.0f}' for x in pw)} | | | | | | | | | | |")
    print()
    def errs(ev):
        return "; ".join(str(e.get("message", e) if isinstance(e, dict) else e) for e in (ev or {}).get("diagnostics") or [] if "rror" in str(e))[:160]
    print(f"watch cold (rel) {cold_rel:.1f} s ok {ev_cold and ev_cold.get('ok')} pages {ev_cold and ev_cold.get('pages')} {errs(ev_cold)}; "
          f"prof cold ok {pev_cold and pev_cold.get('ok')} pages {pev_cold and pev_cold.get('pages')}; history after the reverts md5 {hist_rel}")
    for (name, _, _), (wall, ev) in zip(CLASSES, walls):
        if ev and not ev.get("ok"):
            print(f"  {name}: not ok: {errs(ev) or str(ev.get('errors'))[:160]}")
    print(f"amsldoc -j 1: exit {j1[0]} md5 {j1[1]} pages {j1[2]} wall {j1[3]:.1f} s nodes {j1[4]} {j1[5]}")
    for k, x in enumerate(j8):
        print(f"amsldoc -j 8 #{k + 1}: exit {x[0]} md5 {x[1]} pages {x[2]} wall {x[3]:.1f} s {x[5]}")


if __name__ == "__main__":
    main()
