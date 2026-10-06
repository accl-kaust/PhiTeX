#!/usr/bin/env python3
"""The figures of bench/results/*.json that map onto the CI's perf
metrics, as history entries (docs/ci.md, "Performance"): marked
`imported`, with their host, so the report shows them apart from the
CI's own runs on the perf node. Run once:

    import_bench.py bench/results OUTDIR    # OUTDIR/import-<file>.json
"""

import json
import statistics
import sys
from pathlib import Path


def fig(v):
    return {"median": v, "min": v, "max": v, "values": [v]}


def main():
    src, out = Path(sys.argv[1]), Path(sys.argv[2])
    out.mkdir(parents=True, exist_ok=True)
    for f in sorted(src.glob("*.json")):
        if f.name == "history.jsonl":
            continue
        d = json.loads(f.read_text())
        docs = {}

        def put(doc, k, v):
            if isinstance(v, (int, float)):
                docs.setdefault(doc, {})[k] = fig(v)
        for x in d.get("figures") or []:
            c, m = x.get("case", ""), x.get("median_seconds")
            for doc in ("article", "pgfsub"):
                if c == f"{doc}: pdflatex, plain run":
                    put(doc, "texlive.pass_s", m)
                elif c == f"{doc}: plain run":
                    put(doc, "plain.pass_s", m)
                elif c == f"{doc}: -converge, cold":
                    put(doc, "build.cold_s", m)
                elif c == f"{doc}: -converge, unchanged":
                    put(doc, "machine.warm_s", m)
            if c.startswith(("course: cold build (", "course: machine cold build (")) and "course" not in docs:
                put("course", "machine.cold_s", m)
            if c.startswith("course build, nothing changed: to the result (no-op path on)"):
                put("course", "machine.warm_s", m)
        if d.get("cold", {}).get("ms"):
            put("course", "ssa.cold_ms", d["cold"]["ms"])
            words = [r["ms"] for r in d.get("rebuilds", []) if r.get("name") == "word" and not r.get("revert")
                     and isinstance(r.get("ms"), (int, float))]
            if words:
                put("course", "ssa.edit_ms", statistics.median(words))
        if not docs:
            continue
        entry = {"commit": d.get("commit"), "date": d.get("date"), "subject": f"(imported from bench/results/{f.name})",
                 "run": f"import-{f.stem}", "main": True, "imported": True, "node": d.get("host"),
                 "cpus": d.get("cpus"), "docs": docs}
        (out / f"import-{f.stem}.json").write_text(json.dumps(entry, sort_keys=True) + "\n")
        print(f"{f.name}: {sorted(docs)}")


if __name__ == "__main__":
    main()
