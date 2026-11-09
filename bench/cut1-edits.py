#!/usr/bin/env python3
"""cut1 bench: a watch build, then scripted edits, each timed from the
save to the watch's build event; the final PDF compared with a cold build
of the final text; the watch's peak RSS (VmHWM) printed.

    cut1-edits.py BIN DIR MAIN J EDITS_JSON OUT_JSON [FLAG ...]

EDITS_JSON: [[file, old, new], ...] applied in turn (each old must occur
once in the file as it then stands). An entry [file, old, new, n] repeats
the pair n times, alternating old->new and new->old.
"""
import hashlib
import json
import os
import shutil
import subprocess
import sys
import time


def hwm(pid):
    try:
        for l in open(f"/proc/{pid}/status"):
            if l.startswith("VmHWM:"):
                return int(l.split()[1]) // 1024
    except OSError:
        pass
    return None


def main():
    binary, d, main_, j, edits_json, out = sys.argv[1:7]
    flags = sys.argv[7:]
    edits = json.load(open(edits_json))
    p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", j, main_], cwd=d,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=open(os.environ["EDITS_STDERR"], "w") if os.environ.get("EDITS_STDERR") else subprocess.DEVNULL)

    def next_build():
        t = time.monotonic()
        for line in p.stdout:
            try:
                x = json.loads(line)
            except ValueError:
                continue
            e = x.get("event", {}) if x.get("kind") == "event" else x
            if isinstance(e, dict) and e.get("event") == "build":
                return e, time.monotonic() - t
        return None, time.monotonic() - t

    ev, w = next_build()
    res = {"cold_s": round(w, 3), "cold_wall_ms": (ev or {}).get("wall_ms"), "edits": []}
    print(f"cold: {w:.3f} s", flush=True)
    steps = []
    for e in edits:
        f, a, b = e[0], e[1], e[2]
        n = e[3] if len(e) > 3 else 1
        for k in range(n):
            steps.append((f, a, b) if k % 2 == 0 else (f, b, a))
    for f, a, b in steps:
        path = os.path.join(d, f)
        text = open(path, encoding="latin-1").read()
        if text.count(a) != 1:
            print(f"edit {a!r}: {text.count(a)} matches, skipped", flush=True)
            continue
        text = text.replace(a, b)
        time.sleep(0.4)
        tmp = path + ".tmp"
        open(tmp, "w", encoding="latin-1").write(text)
        os.replace(tmp, path)
        ev, w = next_build()
        ms = round(w * 1000, 1)
        res["edits"].append({"old": a[:40], "new": b[:40], "ms": ms, "wall_ms": (ev or {}).get("wall_ms")})
        print(f"edit {a[:30]!r} -> {b[:30]!r}: {ms} ms (wall_ms {(ev or {}).get('wall_ms')})", flush=True)
    res["watch_hwm_mb"] = hwm(p.pid)
    pdf = os.path.join(d, os.path.splitext(main_)[0] + ".pdf")
    try:
        watched = hashlib.md5(open(pdf, "rb").read()).hexdigest()
    except OSError:
        watched = None
    try:
        p.stdin.write(b"q\n")
        p.stdin.close()
    except OSError:
        pass
    try:
        p.wait(60)
    except subprocess.TimeoutExpired:
        p.kill()
    cold = d.rstrip("/") + "-cold"
    shutil.rmtree(cold, ignore_errors=True)
    shutil.copytree(d, cold, symlinks=True)
    for x in os.listdir(cold):
        if x.endswith((".pdf", ".aux", ".log", ".toc", ".out")):
            os.remove(os.path.join(cold, x))
    subprocess.run([binary, "build", *flags, "-j", j, main_], cwd=cold,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        c = hashlib.md5(open(os.path.join(cold, os.path.basename(pdf)), "rb").read()).hexdigest()
    except OSError:
        c = None
    res["watch_md5"], res["cold_md5"] = watched, c
    res["verdict"] = "same" if watched and watched == c else "DIFFERENT"
    ms = sorted(x["ms"] for x in res["edits"])
    res["median_ms"] = ms[len(ms) // 2] if ms else None
    print(f"verdict {res['verdict']} median {res['median_ms']} ms hwm {res['watch_hwm_mb']} MB", flush=True)
    json.dump(res, open(out, "w"), indent=1)


main()
