import json, subprocess, sys, time, os, hashlib, shutil
# editpdf-watch-tmp.py BIN DIR MAIN LINE OLD NEW J SAVE [WAIT_S] [extra watch args...]
# One watch session: the first build, one edit of MAIN at LINE, the edit's build;
# the PDF saved before quitting (SAVE.pre.pdf) and after (SAVE.post.pdf).
binary, d, main, line, old, new, j, save = sys.argv[1:9]
wait = float(sys.argv[9]) if len(sys.argv) > 9 else 0.0
extra = sys.argv[10:]
err = open(save + ".err", "w")
p = subprocess.Popen([binary, "watch", "--no-view", "--json", "-j", j, *extra, main], cwd=d, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
b = []; ev = []
for l in p.stdout:
    try: e = json.loads(l)
    except ValueError: continue
    ev.append(e)
    if e.get("event") != "build": continue
    b.append(e)
    if len(b) == 1:
        time.sleep(1)
        q = os.path.join(d, main); ls = open(q, "rb").read().split(b"\n"); k = int(line) - 1
        assert old.encode() in ls[k]; ls[k] = ls[k].replace(old.encode(), new.encode(), 1); open(q, "wb").write(b"\n".join(ls))
    else: break
if wait: time.sleep(wait)
pdf = os.path.join(d, os.path.splitext(main)[0] + ".pdf")
shutil.copy(pdf, save + ".pre.pdf")
try:
    p.stdin.write(b"q\n"); p.stdin.close()
except OSError: pass
for l in p.stdout:
    try: ev.append(json.loads(l))
    except ValueError: pass
p.wait(timeout=600)
shutil.copy(pdf, save + ".post.pdf")
json.dump(ev, open(save + ".events.json", "w"))
h = lambda f: hashlib.md5(open(f, "rb").read()).hexdigest()[:10]
print("edit_ms %s nodes %s pre %s post %s" % (b[1].get("wall_ms") if len(b) > 1 else None, b[1].get("nodes_run") if len(b) > 1 else None, h(save + ".pre.pdf"), h(save + ".post.pdf")))
