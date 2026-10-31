#!/usr/bin/env python3
"""r3-cuts: one amsldoc edit through `watch --json --exact-pdf` with the
traces asked for in the environment; the edit build's stderr between
markers. edit-trace.py BIN WORKDIR RESULTS CLASS (a name of
scripts/accl/edit-classes.py's CLASSES)."""
import json, os, shutil, subprocess, sys
sys.path.insert(0, os.path.join(os.getcwd(), "scripts/accl"))
import importlib.util
spec = importlib.util.spec_from_file_location("ec", "scripts/accl/edit-classes.py")
ec = importlib.util.module_from_spec(spec); spec.loader.exec_module(ec)
binary, work, res, cls = sys.argv[1:5]
d = os.path.join(work, "et")
shutil.rmtree(d, ignore_errors=True); shutil.copytree("bench/amsldoc", d)
env = dict(ec.ENV, PHITEX_CACHE_DIR=os.path.join(work, "cache"))
errf = os.path.join(res, f"edit-{cls.replace(' ', '_')}.err")
open(errf, "w").close()
w = ec.Watch(binary, d, "amsldoc.tex", 8, env, errf, ["--exact-pdf"])
w.build()
orig = open(w.path, encoding="latin-1").read()
a, b = next((a, b) for n, a, b in ec.CLASSES if n == cls)
w.mark("EDIT")
ev, wall = None, 0
w.put(orig.replace(a, b)); ev, wall = w.build()
w.mark("END")
w.quit()
print(cls, f"{wall*1e3:.0f} ms", ev and ev["notes"][0])
