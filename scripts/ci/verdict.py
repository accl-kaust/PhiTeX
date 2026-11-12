#!/usr/bin/env python3
"""The verdict of a corpus shard (gate-shard.sh research and manuals):
each document of shard K of N of LIST (shards.py) against its results at
every -j of JOBS the shard builds it at (sweep.py run's OUT/<id>-j<J>.json).

    verdict.py LIST K/N JOBS OUT [--lane TEXT] [--known FILE --known-out FILE] [--slow FILE]

Prints a red line for each document not identical: a PDF that differs
(with pdfdiff's first differing object), a build that fails, panics or
times out, an oracle that fails, a result that is missing (never a pass
of nothing). A document in --known (ci/manuals-not-identical.tsv: its
id, a tab, why it is not identical) is not red: its line goes to
--known-out with its cause, and one now identical at every -j is said
there too. Listed as `ID -jJ`, only its results at -j J are known: at
another -j it is red as any. A document in --slow (ci/manuals-slow.tsv: its id, a tab,
its measured times) is identical given time but slower than the limit:
a build of it that only ran past its time limit is not red, its line
goes to --known-out as "slow (listed)", and one within the limit at
every -j is said there; a PDF of it that differs, or an error, stays
red (an exact PDF is correct however long it takes: the user, 2026-11-04).
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import manuals  # noqa: E402
import shards  # noqa: E402


def line(d, j, r, lane):
    st = r.get("status")
    note = (r.get("note") or "")[:200]
    first = (r.get("diff") or {}).get("summary") or ""
    extra = f" [{first[:200]}]" if first and first not in note else ""
    if r.get("oracle_rebuilt_md5"):
        extra += f" [the oracle built here has md5 {r['oracle_rebuilt_md5']}, not the pinned {r.get('oracle_md5')}]"
    return f"{d['id']} -j{j}{lane}: {st}: {note}{extra}"


def listed(path):
    """A list's ids and their text (its id, a tab, the text; `#` lines
    left out), or nothing for no list."""
    out = {}
    if path:
        for l in open(path):
            if l.strip() and not l.startswith("#"):
                i, _, why = l.rstrip("\n").partition("\t")
                out[i] = why
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("list")
    ap.add_argument("shard")
    ap.add_argument("jobs")
    ap.add_argument("out")
    ap.add_argument("--lane", default="")
    ap.add_argument("--family", help="the times the shards are dealt by (shards.py): the list's stem unless given")
    ap.add_argument("--prefix", default="")
    ap.add_argument("--known")
    ap.add_argument("--known-out")
    ap.add_argument("--slow")
    a = ap.parse_args()
    known = listed(a.known)
    slow = listed(a.slow)
    lane = f" ({a.lane})" if a.lane else ""
    kout = []
    family = a.family or shards.family_of(a.list)
    # (each document of the shard at the -j it is built at here: a
    # manual's builds at each -j are dealt apart, shards.py)
    for d, js in shards.mine(family, manuals.load_list(a.list), a.shard, a.jobs):
        js = [str(j) for j in js]
        # (each not identical: its status and its line)
        bad, no_oracle = [], None
        for j in js:
            try:
                r = json.loads((Path(a.out) / f"{d['id']}-j{j}.json").read_text())
            except (OSError, ValueError):
                bad.append(("missing", j, f"{d['id']} -j{j}{lane}: no result"))
                continue
            # (documented: differs from the oracle only where
            # ci/manuals-pdftex-bugs.tsv lets it, pdfTeX's bug not copied)
            if r.get("status") in ("exact", "documented"):
                continue
            if r.get("status") == "no-oracle":
                no_oracle = f"{d['id']}{lane}: no oracle: {r.get('note')}"
                continue
            bad.append((r.get("status"), j, line(d, j, r, lane)))
        if no_oracle:
            bad.insert(0, ("no-oracle", None, no_oracle))
        # (one known at a -j alone, `ID -jJ`: its results there go apart, the rest stay)
        for j in js:
            why = known.get(f"{d['id']} -j{j}")
            if why is None:
                continue
            kout.extend(f"{b} (known at -j{j}: {why})" for _, bj, b in bad if bj == j)
            if not any(bj == j for _, bj, _ in bad):
                kout.append(f"{d['id']}{lane}: identical now at -j {j} (known at -j{j}: {why})")
            bad = [x for x in bad if x[1] != j]
        if d["id"] in known:
            kout.extend(f"{b} (known: {known[d['id']]})" for _, _, b in bad)
            if not bad:
                kout.append(f"{d['id']}{lane}: identical now at -j {','.join(js)} (known: {known[d['id']]})")
        elif d["id"] in slow:
            # (a slow one: its timeouts are expected and go apart; a PDF
            # that differs, an error, a missing result stay red)
            for st, _, b in bad:
                if st == "timeout":
                    kout.append(f"slow (listed): {b} (slow: {slow[d['id']]})")
                else:
                    print(a.prefix + b)
            if not bad:
                kout.append(f"{d['id']}{lane}: within its limit now at -j {','.join(js)} (slow: {slow[d['id']]})")
        else:
            for _, _, b in bad:
                print(a.prefix + b)
    if a.known_out:
        Path(a.known_out).write_text("".join(a.prefix + x + "\n" for x in kout))


if __name__ == "__main__":
    main()
