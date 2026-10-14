#!/usr/bin/env python3
"""The phitex-stage lists from an oracle run: the documents whose oracle
is ok, split by size (over 50 pages: 1, 8 and 16 threads).

    lists.py LIST.tsv RESULTS_DIR OUT_PREFIX   -> OUT_PREFIX-small.tsv, OUT_PREFIX-big.tsv
"""
import json
import sys
from pathlib import Path

lst, res, out = sys.argv[1:4]
small, big = [], []
for line in Path(lst).read_text().splitlines():
    f = line.split("\t")
    if not f or not f[0] or f[0].startswith("#"):
        continue
    try:
        o = json.loads((Path(res) / f[0] / "oracle" / "oracle.json").read_text())
    except (OSError, ValueError):
        continue
    if o.get("status") != "ok":
        continue
    pages = o.get("pages") or 0
    f[4] = "1,8,16" if pages > 50 else "1,8"
    (big if pages > 50 else small).append((o.get("wall_s") or 0, "\t".join(f)))
for name, rows in (("small", small), ("big", big)):
    rows.sort(reverse=True)  # (the longest first: the shards even out)
    Path(f"{out}-{name}.tsv").write_text("".join(r + "\n" for _, r in rows))
    print(name, len(rows), "oracle s", round(sum(w for w, _ in rows)))
