import json, subprocess, sys, time, os, hashlib
# watchedit.py BIN DIR MAIN FILE LINE OLD NEW J -> prints cold_ms edit_ms nodes_cold nodes_edit
binary, d, main, f, line, old, new, j = sys.argv[1:9]
err = open(sys.argv[9], "w") if len(sys.argv) > 9 else subprocess.DEVNULL
p = subprocess.Popen([binary, "watch", "--no-view", "--json"] + ([] if os.environ.get("NOEXACT") else ["--exact-pdf"]) + ["-j", j, main], cwd=d, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
b = []
t0 = time.monotonic()
for l in p.stdout:
    try: e = json.loads(l)
    except ValueError: continue
    if e.get("event") != "build": continue
    b.append(e)
    if len(b) == 1:
        time.sleep(1)
        q = os.path.join(d, f); ls = open(q, "rb").read().split(b"\n"); k = int(line) - 1
        assert old.encode() in ls[k]; ls[k] = ls[k].replace(old.encode(), new.encode(), 1); open(q, "wb").write(b"\n".join(ls))
    else:
        o = e.get("output")
        if o: import shutil; shutil.copy(os.path.join(d, o), os.path.join(d, "edited.pdf"))
        break
try:
    p.stdin.write(b"q\n"); p.stdin.close()
except OSError: pass
p.wait(timeout=600)
# (the PDF the last build wrote, by the path its event names, relative to the watch's directory)
def md5(e):
    o = e.get("output")
    if not o: return "none"
    try: return hashlib.md5(open(os.path.join(d, o), "rb").read()).hexdigest()[:10]
    except OSError: return "missing"
print("cold_ms %s edit_ms %s nodes_cold %s nodes_edit %s pages %s ok %s/%s pdf %s" % (b[0].get("wall_ms"), b[1].get("wall_ms") if len(b) > 1 else None, b[0].get("nodes_run"), b[1].get("nodes_run") if len(b) > 1 else None, b[-1].get("pages"), b[0].get("ok"), b[-1].get("ok"), md5(b[-1])))
