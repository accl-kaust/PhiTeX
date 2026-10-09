#!/usr/bin/env python3
"""case.py NAME DIR: lay out xtask's incremental case NAME (from
xtask/src/e2e.rs's INCREMENTAL) in DIR: DIR/src (its inputs) and
DIR/edits/1.. (each stage's files, edits applied in turn, as edit_file
does: the marker's first occurrence replaced). Prints the job and pdf
flag."""
import os, re, sys

R = os.path.abspath(os.path.join(os.path.dirname(__file__), "../.."))
name, d = sys.argv[1], sys.argv[2]
src = open(os.path.join(R, "xtask/src/e2e.rs")).read()
inc = src[src.index("const INCREMENTAL"):]
m = re.search(r'name: "%s",(.*?)by: By::' % re.escape(name), inc, re.S)
body = m.group(1)

def rust_str(s):
    # a Rust literal: r"..." or "..." with escapes
    if s.startswith('r"'):
        return s[2:-1]
    return bytes(s[1:-1], "utf-8").decode("unicode_escape")

lit = r'(r"(?:[^"])*"|"(?:[^"\\]|\\.)*")'
inputs = [rust_str(x) for x in re.findall(lit, re.search(r"inputs: &\[(.*?)\]", body, re.S).group(1))]
job = rust_str(re.search(r"job: " + lit, body).group(1))
eb = re.search(r"edits: &\[(.*?)\n        \],", body, re.S).group(1)
edits = [tuple(rust_str(x) for x in t) for t in re.findall(r"\(\s*" + lit + r",\s*" + lit + r",\s*" + lit + r",?\s*\)", eb)]
pdf = re.search(r"pdf: (true|false)", body).group(1) == "true"
os.makedirs(os.path.join(d, "src"), exist_ok=True)
files = {}
for i in inputs:
    b = open(os.path.join(R, "tests/e2e", i), "rb").read()
    files[i] = b
    open(os.path.join(d, "src", i), "wb").write(b)
for k, (f, a, b) in enumerate(edits, 1):
    ed = os.path.join(d, "edits", str(k))
    os.makedirs(ed, exist_ok=True)
    if a:
        t = files[f].decode()
        assert a in t, (f, a)
        files[f] = t.replace(a, b, 1).encode()
    open(os.path.join(ed, f), "wb").write(files[f])
print(job, "pdf" if pdf else "dvi", len(edits))
