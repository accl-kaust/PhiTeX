#!/usr/bin/env python3
"""r3-cuts: a document built by `watch --json`, one replacement saved,
its build awaited; stderr (with the traces asked for in the environment)
in RESULTS/watch-<name>.err between the markers.
watch-edit.py BIN WORKDIR RESULTS SRC_DIR MAIN OLD NEW [watch flags...]"""
import os, shutil, sys, importlib.util
spec = importlib.util.spec_from_file_location("ec", "scripts/accl/edit-classes.py")
ec = importlib.util.module_from_spec(spec); spec.loader.exec_module(ec)
binary, work, res, src, main_, old, new, *flags = sys.argv[1:]
d = os.path.join(work, "we")
shutil.rmtree(d, ignore_errors=True); shutil.copytree(src, d)
env = dict(ec.ENV, PHITEX_CACHE_DIR=os.path.join(work, "cache"))
errf = os.path.join(res, f"watch-{os.path.splitext(main_)[0]}.err")
open(errf, "w").close()
w = ec.Watch(binary, d, main_, 1, env, errf, flags)
w.build()
# (EDIT_FILE: another file of the document to edit than the main one)
path = os.path.join(d, os.environ["EDIT_FILE"]) if os.environ.get("EDIT_FILE") else w.path
w.path = path
text = open(w.path, encoding="latin-1").read()
assert text.count(old) == 1
w.mark("EDIT")
w.put(text.replace(old, new)); ev, wall = w.build()
w.mark("END")
w.quit()
print(main_, f"{wall*1e3:.0f} ms", ev and ev["notes"][0])
