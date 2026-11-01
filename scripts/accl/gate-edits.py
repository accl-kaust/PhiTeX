#!/usr/bin/env python3
"""An edit history (docs/spec-final.md §0, bytes): a watch build, then N
one-word edits at random places, each saved and built in turn, then the
PDF compared with a cold build of the final text.

    gate-edits.py BIN DIR MAIN J N SEED OUT_JSON [FLAG ...]

DIR holds the document (MAIN); FLAGs go to both `watch` and `build`
(`--exact-pdf`, `--program pdftex`). The edits replace one word (letters
only, between blanks, on a line that is no command's) with another of
the same length; the history's PDF and the cold build's must be equal.
"""
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import time

WORD = re.compile(r"(?<=[ ])([a-z]{4,12})(?=[ .,;])")


def candidates(text):
    out = []
    off = 0
    for line in text.split("\n"):
        s = line.lstrip()
        if s and not s.startswith(("\\", "%")) and "\\verb" not in line and "{" not in line:
            for m in WORD.finditer(line):
                out.append((off + m.start(1), m.group(1)))
        off += len(line) + 1
    return out


def other(w, rng):
    # (a word of the same length, never the same)
    while True:
        x = "".join(rng.choice("abcdefghijklmnopqrstuvwxyz") for _ in w)
        if x != w:
            return x


def md5(p):
    try:
        return hashlib.md5(open(p, "rb").read()).hexdigest()
    except OSError:
        return None


def main():
    binary, d, main_, j, n, seed, out = sys.argv[1:8]
    flags = sys.argv[8:]
    n, rng = int(n), random.Random(int(seed))
    path = os.path.join(d, main_)
    pdf = os.path.join(d, os.path.splitext(main_)[0] + ".pdf")
    res = {"j": int(j), "edits": 0, "edit_walls_s": []}
    # (GATE_EDIT_STDERR: the watch's stderr kept there, its timelines
    # with PHITEX_TEX_TIME)
    err = open(os.environ["GATE_EDIT_STDERR"], "w") if os.environ.get("GATE_EDIT_STDERR") else subprocess.DEVNULL
    p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", j, main_], cwd=d,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)

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

    ev, _ = next_build()
    res["cold_event"] = ev is not None
    text = open(path, encoding="latin-1").read()
    for _ in range(n):
        cs = candidates(text)
        if not cs or ev is None:
            break
        at, w = rng.choice(cs)
        text = text[:at] + other(w, rng) + text[at + len(w):]
        tmp = path + ".tmp"
        open(tmp, "w", encoding="latin-1").write(text)
        os.replace(tmp, path)
        ev, wall = next_build()
        if ev is None:
            break
        res["edits"] += 1
        res["edit_walls_s"].append(round(wall, 4))
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    try:
        p.wait(timeout=120)
    except subprocess.TimeoutExpired:
        p.kill()
    res["history_md5"] = md5(pdf)
    # the cold build of the final text, in a directory of its own
    cold = d.rstrip("/") + "-cold"
    shutil.rmtree(cold, ignore_errors=True)
    shutil.copytree(d, cold)
    for f in os.listdir(cold):
        if f.endswith((".pdf", ".aux", ".toc", ".log", ".out", ".idx", ".ind", ".ilg")):
            os.remove(os.path.join(cold, f))
    subprocess.run([binary, "build", *flags, "-j", j, main_], cwd=cold,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800)
    res["cold_md5"] = md5(os.path.join(cold, os.path.splitext(main_)[0] + ".pdf"))
    ok = res["edits"] == n and res["history_md5"] and res["history_md5"] == res["cold_md5"]
    res["verdict"] = "same" if ok else "DIFFERS" if res["edits"] == n else f"stopped after {res['edits']}"
    json.dump(res, open(out, "w"), indent=1)
    print(f"edit history -j {j}: {res['edits']} of {n} edits, history {res['history_md5']} cold {res['cold_md5']} "
          f"{res['verdict']}")


if __name__ == "__main__":
    main()
