#!/usr/bin/env python3
"""Every edit kind on one document in one watch session: each edit saved,
timed from the save to the watch's build event, and its PDF compared with a
cold build of the same text.

    gate-editkinds.py BIN DIR MAIN J OUT_JSON [FLAG ...]

DIR holds the document (MAIN, amsldoc's text: the sites are found by
shape, at fractions of the file); FLAGs go to both `watch` and `build`
(`--exact-pdf`). The watch runs with PHITEX_ROUND_ACCT=1 (the caller's
environment): each edit's `acct-edit:` line gives its rounds, runs, nodes
run and cutoffs (nodes run whose output did not change). GATE_EDIT_STDERR
keeps the watch's stderr there.

Red: an edit whose PDF is not the cold build's, or a watch that gives no
build event. Times are recorded against each kind's target (at -j 8, end
to end), never red. OUT_JSON: `{"j", "cold_watch_ms", "verdict": "same" |
"differs" | "watch failed", "edits": [{"kind", "class", "what", "ms",
"target_ms", "same", "rounds", "runs", "nodes_run", "cutoffs", "by_op"}]}`.
"""
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

# (each class's target in ms, at -j 8, from the save to the build event)
TARGETS = {"local": 5, "sentence": 10, "preamble": 10, "heading": 20, "page": 50}


def md5(p):
    try:
        return hashlib.md5(open(p, "rb").read()).hexdigest()
    except OSError:
        return None


def plain(line):
    s = line.strip()
    return bool(s) and not s.startswith(("\\", "%")) and not any(c in line for c in ("\\verb", "{", "$"))


def at_frac(lines, f, pred):
    # (the first line at or after the fraction that fits, else from the top)
    n = len(lines)
    for i in list(range(int(n * f), n)) + list(range(int(n * f))):
        if pred(i):
            return i
    raise SystemExit(f"no site at {f}")


WORD = re.compile(r" ([a-z]{5,9})[ .,]")


def word(lines, f):
    i = at_frac(lines, f, lambda i: plain(lines[i]) and WORD.search(lines[i]))
    m = WORD.search(lines[i])
    w = m.group(1)
    lines[i] = lines[i][: m.start(1)] + w[::-1] + lines[i][m.end(1):]
    return f"line {i + 1}: {w} -> {w[::-1]}"


def sentence(lines, f):
    i = at_frac(lines, f, lambda i: plain(lines[i]) and i + 1 < len(lines) and plain(lines[i + 1]))
    lines[i] += " This sentence was added to make the paragraph longer, so that its lines and perhaps the page break after it move."
    return f"line {i + 1}: sentence appended"


def delpar(lines, f):
    i = at_frac(lines, f, lambda i: lines[i].strip() == "" and i + 3 < len(lines) and plain(lines[i + 1]) and plain(lines[i + 2]))
    k = i + 1
    while k < len(lines) and plain(lines[k]):
        k += 1
    del lines[i + 1 : k]
    return f"lines {i + 2}-{k}: paragraph deleted"


def heading(lines, f):
    i = at_frac(lines, f, lambda i: lines[i].startswith("\\section{"))
    lines[i] = lines[i].replace("\\section{", "\\section{Revised ", 1)
    return f"line {i + 1}: {lines[i][:60]}"


def preamble(lines, _f):
    i = next(i for i, l in enumerate(lines) if l.startswith("\\begin{document}"))
    lines.insert(i, "\\newcommand{\\phitexunusedmacro}{x}")
    return f"line {i + 1}: a macro nothing uses defined before \\begin{{document}}"


# (kind, class, edit, site): in this order, in one session. On amsldoc the
# sentence at 0.3 moves no page break, the one at 0.7 does.
KINDS = [
    ("word", "local", word, 0.5),
    ("word", "local", word, 0.8),
    ("sentence", "sentence", sentence, 0.3),
    ("page-moving sentence", "page", sentence, 0.7),
    ("paragraph delete", "page", delpar, 0.5),
    ("heading", "heading", heading, 0.45),
    ("preamble", "preamble", preamble, 0),
    ("word after", "local", word, 0.6),
    ("sentence after", "page", sentence, 0.7),
]

ACCT = re.compile(
    r"^acct-edit: wall [\d.]+ ms, (\d+) rounds, .*?, (\d+) runs, (\d+) nodes run, (?:(\d+) cutoffs, )?runs by op \[(.*)\]",
    re.M,
)


def main():
    binary, d, main_, j, out = sys.argv[1:6]
    flags = sys.argv[6:]
    path = os.path.join(d, main_)
    stem = os.path.splitext(main_)[0]
    pdf = os.path.join(d, stem + ".pdf")
    errp = os.environ.get("GATE_EDIT_STDERR") or os.path.join(d, "..", f"editkinds-j{j}.err")
    err = open(errp, "w")
    p = subprocess.Popen(
        [binary, "watch", "--no-view", "--json", *flags, "-j", j, main_],
        cwd=d, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err,
    )
    seen = 0  # (the stderr's acct lines read so far)

    def next_build(timeout=900):
        t = time.monotonic()
        for line in p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            if x.get("event") == "build":
                return x, time.monotonic() - t
            if time.monotonic() - t > timeout:
                break
        return None, time.monotonic() - t

    def acct():
        nonlocal seen
        err.flush()
        ms = ACCT.findall(open(errp, errors="replace").read())
        new, seen = ms[seen:], len(ms)
        if not new:
            return {}
        # (the edit's evaluations summed: one, unless the watch built twice)
        by = {}
        for m in new:
            for op, n in re.findall(r'\("(\w+)", (\d+)\)', m[4]):
                by[op] = by.get(op, 0) + int(n)
        return {
            "rounds": sum(int(m[0]) for m in new),
            "runs": sum(int(m[1]) for m in new),
            "nodes_run": sum(int(m[2]) for m in new),
            "cutoffs": sum(int(m[3] or 0) for m in new),
            "by_op": by,
        }

    ev, w0 = next_build()
    res = {"j": int(j), "cold_watch_ms": round(w0 * 1e3, 1), "edits": [], "verdict": "same"}
    if ev is None:
        res["verdict"] = "watch failed"
    else:
        acct()
    lines = open(path, encoding="latin-1").read().split("\n")
    for name, cls, fn, f in KINDS if ev is not None else []:
        what = fn(lines, f)
        tmp = path + ".tmp"
        open(tmp, "w", encoding="latin-1").write("\n".join(lines))
        os.replace(tmp, path)
        ev, wall = next_build()
        e = {"kind": name, "class": cls, "what": what, "target_ms": TARGETS[cls]}
        if ev is None:
            e.update(ms=None, same=False)
            res["edits"].append(e)
            res["verdict"] = "watch failed"
            print(f"{name:21s} watch gave no build event ({what})", flush=True)
            break
        time.sleep(0.3)
        e.update(acct())
        hm = md5(pdf)
        cold = d.rstrip("/") + "-cold"
        shutil.rmtree(cold, ignore_errors=True)
        shutil.copytree(d, cold)
        for x in os.listdir(cold):
            if x.endswith((".pdf", ".aux", ".toc", ".log", ".out", ".idx", ".ind", ".ilg")):
                os.remove(os.path.join(cold, x))
        cenv = {k: v for k, v in os.environ.items() if k != "PHITEX_ROUND_ACCT"}
        subprocess.run([binary, "build", *flags, "-j", j, main_], cwd=cold,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800, env=cenv)
        cm = md5(os.path.join(cold, stem + ".pdf"))
        e.update(ms=round(wall * 1e3, 1), same=hm is not None and hm == cm, ok=ev.get("ok"), pages=ev.get("pages"))
        res["edits"].append(e)
        if not e["same"] and res["verdict"] == "same":
            res["verdict"] = "differs"
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    try:
        p.wait(timeout=120)
    except subprocess.TimeoutExpired:
        p.kill()
    json.dump(res, open(out, "w"), indent=1)
    # (the table: one line an edit)
    print(f"-j {j}: cold watch build {res['cold_watch_ms']} ms; verdict {res['verdict']}")
    print(f"  {'kind':21s} {'ms':>9s} {'target':>6s}  {'pdf':7s} {'rounds':>6s} {'runs':>5s} {'nodes':>5s} {'cutoffs':>7s}  runs by op; edit")
    for e in res["edits"]:
        # (a time over its target marked `!`: recorded, not red)
        ms = "-" if e.get("ms") is None else f"{e['ms']:.1f}" + ("!" if e["ms"] > e["target_ms"] else " ")
        by = " ".join(f"{k}:{v}" for k, v in sorted(e.get("by_op", {}).items()))
        print(f"  {e['kind']:21s} {ms:>9s} {'<=' + str(e['target_ms']):>6s}  {'exact' if e['same'] else 'DIFFERS':7s} "
              f"{e.get('rounds', '-'):>6} {e.get('runs', '-'):>5} {e.get('nodes_run', '-'):>5} {e.get('cutoffs', '-'):>7}  {by}; {e['what']}")


if __name__ == "__main__":
    main()
