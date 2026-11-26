#!/usr/bin/env python3
# wdrive.py BIN DIR MAIN J OUT EDIT... : run `phitex watch` in DIR, after the first build
# run each EDIT (a shell command, in DIR) and wait for its build. Per build: OUT/b<k>.json
# (the build event), OUT/b<k>.pdf (the PDF it wrote). stderr -> OUT/err.txt. Prints a table.
import json, subprocess, sys, time, os, shutil, hashlib
binary, d, main, j, out = sys.argv[1:6]
edits = sys.argv[6:]
os.makedirs(out, exist_ok=True)
args = [binary, "watch", "--no-view", "--json"] + (["-j", j] if j != "0" else []) + os.environ.get("WARGS", "").split() + [main]
p = subprocess.Popen(args, cwd=d, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=open(os.path.join(out, "err.txt"), "w"))
k = 0
rows = []
for l in p.stdout:
    try: e = json.loads(l)
    except ValueError: continue
    if e.get("event") != "build": continue
    json.dump(e, open(os.path.join(out, "b%d.json" % k), "w"))
    o = e.get("output")
    md5 = "none"
    if o:
        src = os.path.join(d, o); shutil.copy(src, os.path.join(out, "b%d.pdf" % k))
        md5 = hashlib.md5(open(src, "rb").read()).hexdigest()[:10]
    par = e.get("parallel") or {}
    rows.append("b%d wall_ms %.0f nodes %s runs %s rounds %s pages %s changed %s ok %s md5 %s" % (k, e.get("wall_ms", 0), e.get("nodes_run"), e.get("runs"), e.get("rounds"), e.get("pages"), e.get("pages_changed"), e.get("ok"), md5))
    print(rows[-1], flush=True)
    if k >= len(edits): break
    time.sleep(1.0)
    subprocess.run(edits[k], shell=True, cwd=d, check=True)
    k += 1
try:
    p.stdin.write(b"q\n"); p.stdin.close()
except OSError: pass
try: p.wait(timeout=120)
except subprocess.TimeoutExpired: p.kill()
open(os.path.join(out, "table.txt"), "w").write("\n".join(rows) + "\n")
