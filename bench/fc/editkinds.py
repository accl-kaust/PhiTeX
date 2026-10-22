#!/usr/bin/env python3
"""Edit kinds on amsldoc under one watch: each edit timed save -> build event,
then the watch's PDF compared with a cold build of the same text.
    editkinds-tmp.py BIN DIR MAIN J OUT_JSON [FLAG ...]"""
import hashlib, json, os, re, shutil, subprocess, sys, time
def md5(p):
    try: return hashlib.md5(open(p, "rb").read()).hexdigest()
    except OSError: return None
def plain(line):
    s = line.strip()
    return s and not s.startswith(("\\", "%")) and "\\verb" not in line and "{" not in line and "$" not in line
def at_frac(lines, f, pred, start=0):
    n = len(lines)
    for i in list(range(int(n * f), n)) + list(range(start, int(n * f))):
        if pred(i): return i
    raise SystemExit("no site")
def word(lines, f):
    i = at_frac(lines, f, lambda i: plain(lines[i]) and re.search(r" ([a-z]{5,9})[ .,]", lines[i]))
    m = re.search(r" ([a-z]{5,9})[ .,]", lines[i]); w = m.group(1)
    lines[i] = lines[i][:m.start(1)] + w[::-1] + lines[i][m.end(1):]
    return f"line {i+1}: {w} -> {w[::-1]}"
def sentence(lines, f):
    i = at_frac(lines, f, lambda i: plain(lines[i]) and i + 1 < len(lines) and plain(lines[i + 1]))
    lines[i] += " This sentence was added to make the paragraph longer, so that its lines and perhaps the page break after it move."
    return f"line {i+1}: sentence appended"
def delpar(lines, f):
    i = at_frac(lines, f, lambda i: lines[i].strip() == "" and i + 3 < len(lines) and all(plain(lines[k]) for k in (i + 1, i + 2)))
    k = i + 1
    while k < len(lines) and plain(lines[k]): k += 1
    del lines[i + 1:k]
    return f"lines {i+2}-{k}: paragraph deleted"
def heading(lines, f):
    i = at_frac(lines, f, lambda i: lines[i].startswith("\\section{"))
    lines[i] = lines[i].replace("\\section{", "\\section{Revised ", 1)
    return f"line {i+1}: {lines[i][:60]}"
def preamble(lines, f):
    i = next(i for i, l in enumerate(lines) if l.startswith("\\begin{document}"))
    lines.insert(i, "\\newcommand{\\phitexunusedmacro}{x}")
    return f"line {i+1}: unused macro defined before \\begin{{document}}"
KINDS = [("word", word, 0.5), ("word", word, 0.8), ("sentence", sentence, 0.3), ("sentence", sentence, 0.7),
         ("delete paragraph", delpar, 0.5), ("heading", heading, 0.45), ("preamble", preamble, 0), ("word after", word, 0.6), ("sentence", sentence, 0.7)]
def main():
    global KINDS
    if os.environ.get("EK_KINDS"):
        KINDS = [KINDS[int(x)] for x in os.environ["EK_KINDS"].split(",")]
    binary, d, main_, j, out = sys.argv[1:6]; flags = sys.argv[6:]
    path = os.path.join(d, main_); pdf = os.path.join(d, os.path.splitext(main_)[0] + ".pdf")
    err = open(os.environ["EDIT_STDERR"], "w") if os.environ.get("EDIT_STDERR") else subprocess.DEVNULL
    p = subprocess.Popen([binary, "watch", "--no-view", "--json", *flags, "-j", j, main_], cwd=d,
                         stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=err)
    def next_build(timeout=900):
        t = time.monotonic()
        for line in p.stdout:
            try: x = json.loads(line)
            except ValueError: continue
            if x.get("event") == "build": return x, time.monotonic() - t
            if time.monotonic() - t > timeout: break
        return None, time.monotonic() - t
    ev, w0 = next_build(); res = {"j": int(j), "cold_watch_s": round(w0, 3), "edits": []}
    lines = open(path, encoding="latin-1").read().split("\n")
    for name, fn, f in KINDS:
        what = fn(lines, f)
        tmp = path + ".tmp"; open(tmp, "w", encoding="latin-1").write("\n".join(lines)); os.replace(tmp, path)
        ev, wall = next_build()
        if ev is None: res["edits"].append({"kind": name, "what": what, "wall_s": None}); break
        time.sleep(0.3)
        hm = md5(pdf)
        if os.environ.get("EK_NO_COLD"):
            res["edits"].append({"kind": name, "what": what, "wall_s": round(wall, 4)})
            print(f"{name:17s} {wall*1000:9.1f} ms  (no cold check)  {what}", flush=True)
            continue
        cold = d.rstrip("/") + "-cold"; shutil.rmtree(cold, ignore_errors=True); shutil.copytree(d, cold)
        for x in os.listdir(cold):
            if x.endswith((".pdf", ".aux", ".toc", ".log", ".out", ".idx", ".ind", ".ilg")): os.remove(os.path.join(cold, x))
        subprocess.run([binary, "build", *flags, "-j", j, main_], cwd=cold, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1800)
        cm = md5(os.path.join(cold, os.path.splitext(main_)[0] + ".pdf"))
        e = {"kind": name, "what": what, "wall_s": round(wall, 4), "same_as_cold": hm == cm and hm is not None}
        res["edits"].append(e)
        print(f"{name:17s} {wall*1000:9.1f} ms  {'exact' if e['same_as_cold'] else 'DIFFERS'}  {what}", flush=True)
    try: p.stdin.write(b"q\n"); p.stdin.close()
    except OSError: pass
    try: p.wait(timeout=120)
    except subprocess.TimeoutExpired: p.kill()
    json.dump(res, open(out, "w"), indent=1)
if __name__ == "__main__": main()
