#!/usr/bin/env python3
"""The verdict of a corpus shard (gate-shard.sh research and manuals):
each document of shard K of N of LIST against its results at every -j
(sweep.py run's OUT/<id>-j<J>.json).

    verdict.py LIST K/N JOBS OUT [--lane TEXT] [--known FILE --known-out FILE]

Prints a red line for each document not identical: a PDF that differs
(with pdfdiff's first differing object), a build that fails, panics or
times out, an oracle that fails, a result that is missing (never a pass
of nothing). A document in --known (ci/manuals-not-identical.tsv: its
id, a tab, why it is not identical) is not red: its line goes to
--known-out with its cause, and one now identical at every -j is said
there too.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import manuals  # noqa: E402


def line(d, j, r, lane):
    st = r.get("status")
    note = (r.get("note") or "")[:200]
    first = (r.get("diff") or {}).get("summary") or ""
    extra = f" [{first[:200]}]" if first and first not in note else ""
    if r.get("oracle_rebuilt_md5"):
        extra += f" [the oracle built here has md5 {r['oracle_rebuilt_md5']}, not the pinned {r.get('oracle_md5')}]"
    return f"{d['id']} -j{j}{lane}: {st}: {note}{extra}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("list")
    ap.add_argument("shard")
    ap.add_argument("jobs")
    ap.add_argument("out")
    ap.add_argument("--lane", default="")
    ap.add_argument("--prefix", default="")
    ap.add_argument("--known")
    ap.add_argument("--known-out")
    a = ap.parse_args()
    known = {}
    if a.known:
        for l in open(a.known):
            if l.strip() and not l.startswith("#"):
                i, _, why = l.rstrip("\n").partition("\t")
                known[i] = why
    lane = f" ({a.lane})" if a.lane else ""
    kout = []
    for d in manuals.shard(manuals.load_list(a.list), a.shard):
        bad, no_oracle = [], None
        for j in a.jobs.split(","):
            try:
                r = json.loads((Path(a.out) / f"{d['id']}-j{j}.json").read_text())
            except (OSError, ValueError):
                bad.append(f"{d['id']} -j{j}{lane}: no result")
                continue
            # (documented: differs from the oracle only where
            # ci/manuals-pdftex-bugs.tsv lets it, pdfTeX's bug not copied)
            if r.get("status") in ("exact", "documented"):
                continue
            if r.get("status") == "no-oracle":
                no_oracle = f"{d['id']}{lane}: no oracle: {r.get('note')}"
                continue
            bad.append(line(d, j, r, lane))
        if no_oracle:
            bad.insert(0, no_oracle)
        if d["id"] in known:
            kout.extend(f"{b} (known: {known[d['id']]})" for b in bad)
            if not bad:
                kout.append(f"{d['id']}{lane}: identical now at -j {a.jobs} (known: {known[d['id']]})")
        else:
            for b in bad:
                print(a.prefix + b)
    if a.known_out:
        Path(a.known_out).write_text("".join(a.prefix + x + "\n" for x in kout))


if __name__ == "__main__":
    main()
