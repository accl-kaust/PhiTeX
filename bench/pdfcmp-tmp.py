#!/usr/bin/env python3
# pdfcmp-tmp.py OURS THEIRS: md5s with the date and ID taken out, page
# counts, and the first page whose content stream differs (qpdf --qdf).
import hashlib, re, subprocess, sys, os, tempfile

def norm(b):
    b = re.sub(rb"/CreationDate \([^)]*\)", b"/CreationDate ()", b)
    b = re.sub(rb"/ModDate \([^)]*\)", b"/ModDate ()", b)
    b = re.sub(rb"/ID \[<[0-9A-Fa-f]*> <[0-9A-Fa-f]*>\]", b"/ID []", b)
    return b

def pages(path):
    with tempfile.TemporaryDirectory() as d:
        q = os.path.join(d, "q.pdf")
        subprocess.run(["qpdf", "--qdf", "--object-streams=disable", "--stream-data=uncompress", path, q], check=False)
        t = open(q, "rb").read()
    parts = re.split(rb"%% Contents for page (\d+)\n", t)
    out = {}
    for i in range(1, len(parts) - 1, 2):
        body = parts[i + 1]
        s = body.find(b"stream\n")
        e = body.find(b"endstream", s)
        out[int(parts[i])] = body[s + 7:e]
    return out, t

a, b = sys.argv[1], sys.argv[2]
A, B = open(a, "rb").read(), open(b, "rb").read()
print("md5 ours", hashlib.md5(A).hexdigest(), "theirs", hashlib.md5(B).hexdigest())
na, nb = norm(A), norm(B)
print("md5 without date/ID: ours", hashlib.md5(na).hexdigest(), "theirs", hashlib.md5(nb).hexdigest(), "SAME" if na == nb else "DIFFER")
pa, ta = pages(a)
pb, tb = pages(b)
print("pages ours", len(pa), "theirs", len(pb))
for k in sorted(set(pa) | set(pb)):
    x, y = pa.get(k), pb.get(k)
    if x != y:
        print("first differing page", k)
        if x is not None and y is not None:
            i = next((j for j in range(min(len(x), len(y))) if x[j] != y[j]), min(len(x), len(y)))
            print(" at byte", i, "of", len(x), len(y))
            print(" ours  :", x[max(0, i - 300):i + 300])
            print(" theirs:", y[max(0, i - 300):i + 300])
        break
else:
    if na != nb:
        i = next((j for j in range(min(len(na), len(nb))) if na[j] != nb[j]), min(len(na), len(nb)))
        print("pages same; first byte differing", i)
        print(" ours  :", na[max(0, i - 300):i + 300])
        print(" theirs:", nb[max(0, i - 300):i + 300])
