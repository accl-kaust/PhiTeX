#!/usr/bin/env python3
"""(temporary) the bytes of the page files in DIR by the kind of object (qpdf --qdf, streams kept
compressed): font programs, fonts' other objects, images, forms, annotations, content, other.

    stream-pdfkinds-tmp.py DIR
"""
import collections
import os
import re
import subprocess
import sys

d = sys.argv[1]
files = sorted(f for f in os.listdir(d) if f.endswith(".pdf"))
tot = collections.Counter()
cnt = collections.Counter()
for f in files:
    p = os.path.join(d, f)
    try:
        q = subprocess.run(["qpdf", "--qdf", "--object-streams=disable", "--stream-data=preserve", p, "-"],
                           capture_output=True, timeout=60).stdout
    except Exception:
        continue
    for m in re.finditer(rb"\n(\d+) 0 obj\n(.*?)\nendobj", q, re.S):
        body = m.group(2)
        head = body[:600]
        if re.search(rb"/Length1 ", head) or re.search(rb"/Subtype /(Type1C|CIDFontType0C|OpenType)", head):
            k = "font program"
        elif re.search(rb"/Type /(Font|FontDescriptor|Encoding)", head) or b"/CMapName" in head or b"begincmap" in body[:2000]:
            k = "font other"
        elif re.search(rb"/Subtype /Image", head):
            k = "image"
        elif re.search(rb"/Subtype /Form", head):
            k = "form"
        elif re.search(rb"/Type /Annot", head) or re.search(rb"/Subtype /Link", head):
            k = "annot"
        elif re.search(rb"/Type /Page\b", head):
            k = "page"
        elif b"stream\n" in body and b"/Length" in head and not re.search(rb"/Type", head):
            k = "content/other stream"
        else:
            k = "other"
        tot[k] += len(body)
        cnt[k] += 1
print(f"{len(files)} files, {sum(os.path.getsize(os.path.join(d, f)) for f in files) // 1024} KB on disk")
for k, b in tot.most_common():
    print(f"  {k:22s} {b // 1024:8d} KB  {cnt[k]:7d} objects")
