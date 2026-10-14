#!/usr/bin/env python3
"""The arXiv manifest: id, version, category, main file, tarball sha256,
pages, oracle status (and why dropped), from the fetch directory's
meta.json/sha256 and the oracle run's JSON.

    manifest.py FETCH_DIR RESULTS_DIR OUT.json
"""
import json
import re
import sys
from collections import Counter
from pathlib import Path

fetch, res, out = map(Path, sys.argv[1:4])
rows = []
for d in sorted(fetch.iterdir()):
    if not (d / "meta.json").exists():
        continue
    m = json.loads((d / "meta.json").read_text())
    i = f"a-{d.name}"
    prep = {}
    o = {}
    try:
        prep = json.loads((res / i / "oracle" / "prep.json").read_text())
        o = json.loads((res / i / "oracle" / "oracle.json").read_text())
    except (OSError, ValueError):
        pass
    if prep.get("status") != "ok":
        status, why = "dropped (prefilter)", prep.get("reason", "not run")
    elif o.get("status") != "ok":
        status, why = "dropped (oracle)", o.get("reason", "not run")
    else:
        status, why = "ok", None
    rows.append({"id": m["id"], "version": m["version"], "category": m["category"], "stratum": m["stratum"],
                 "pages_claimed": m.get("pages_claimed"), "main": prep.get("main"),
                 "sha256": (d / "sha256").read_text().strip(), "pages": o.get("pages"),
                 "oracle_status": status, "reason": why, "oracle_wall_s": o.get("wall_s"),
                 "passes": o.get("passes"), "pass_walls": o.get("pass_walls")})
out.write_text(json.dumps(rows, indent=1))
ok = [r for r in rows if r["oracle_status"] == "ok"]
print(f"{len(rows)} fetched, {len(ok)} ok")
print("by stratum (ok/fetched):", ", ".join(f"{s} {sum(1 for r in ok if r['stratum'] == s)}/{n}"
                                          for s, n in Counter(r["stratum"] for r in rows).items()))
sz = Counter("1-9" if r["pages"] < 10 else "10-24" if r["pages"] < 25 else "25-49" if r["pages"] < 50 else "50-99" if r["pages"] < 100 else "100+" for r in ok)
print("pages:", dict(sorted(sz.items())), "max", max(r["pages"] for r in ok))
w = sorted(r["oracle_wall_s"] for r in ok)
print(f"pdflatex to fixpoint: median {w[len(w)//2]:.2f} s, p90 {w[int(len(w)*.9)]:.2f} s, max {w[-1]:.2f} s, total {sum(w):.0f} s")
print("passes:", dict(Counter(r["passes"] for r in ok)))
pre = Counter(re.sub(r"\s*\(.*", "", r["reason"]) for r in rows if r["oracle_status"] == "dropped (prefilter)")
ora = Counter(re.sub(r"`[^']*'", "`X'", r["reason"])[:80] for r in rows if r["oracle_status"] == "dropped (oracle)")
print("prefilter drops:", "; ".join(f"{n} {k}" for k, n in pre.most_common()))
print("oracle drops:", "; ".join(f"{n} {k}" for k, n in ora.most_common()))
