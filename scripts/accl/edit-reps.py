#!/usr/bin/env python3
"""The hard-bar edits' walls per binary (scripts/accl/edit-reps runs it).

    edit-reps.py N WORKDIR RESULTS NAME=BIN...
"""
import os, sys, time, shutil, statistics, importlib.util
spec = importlib.util.spec_from_file_location("ec", "scripts/accl/edit-classes.py")
ec = importlib.util.module_from_spec(spec); spec.loader.exec_module(ec)
n = int(sys.argv[1]); work = sys.argv[2]; res = sys.argv[3]; bins = sys.argv[4:]
cache = os.path.join(work, "cache"); os.makedirs(cache, exist_ok=True)
env = dict(ec.ENV, PHITEX_CACHE_DIR=cache)
ec.cold(bins[0].split("=")[1], "bench/amsldoc", os.path.join(work, "warm"), cache, 8, os.path.join(res, "warm.log"))
CL = [c for c in ec.CLASSES if c[0] in ("one letter", "line, re-break")]
walls = {}
ws = []
for nb in bins:
    name, b = nb.split("=")
    d = os.path.join(work, "d-" + name); shutil.rmtree(d, ignore_errors=True); shutil.copytree("bench/amsldoc", d)
    w = ec.Watch(b, d, "amsldoc.tex", 8, env, os.path.join(res, f"w-{name}.err"), ["--exact-pdf"])
    w.build(); ws.append((name, w))
orig = open(os.path.join(work, "d-" + bins[0].split("=")[0], "amsldoc.tex"), encoding="latin-1").read()
# interleaved: each repetition runs every binary's edit, so drift hits all alike
for k in range(n):
    for cname, a, b in CL:
        for kind, text in (("edit", orig.replace(a, b)), ("revert", orig)):
            for name, w in ws:
                t = w.put(text); ev, _ = w.build(); dt = (time.monotonic() - t) * 1e3
                walls.setdefault((name, cname), []).append(dt)
for name, w in ws: w.quit()
for (name, cname), v in sorted(walls.items(), key=lambda x: (x[0][1], x[0][0])):
    print(f"{cname:16s} {name:8s} median {statistics.median(v):6.1f} ms  min {min(v):6.1f}  max {max(v):6.1f}  (n={len(v)})")
