#!/usr/bin/env python3
"""The arXiv half of the real-document corpus (docs: scripts/corpus/README
in the module doc of corpus.py): pick recent papers stratified by primary
category and size, then download their e-print sources as raw bytes.

    fetch.py list  --out DIR [--per N]     arXiv API listing -> DIR/candidates.json
    fetch.py fetch --out DIR [--limit N]   e-prints -> DIR/<id>v<n>/src (+ sha256)

Nothing is extracted or run here: the sources are untrusted and are only
opened inside the corpus runner's container (corpus.py). Polite: one
request every 3.5 s, a generic User-Agent. Run under scripts/sandbox --net.
The sources are third-party: DIR is outside every commit (target/ or
outside the repository).
"""

import argparse
import hashlib
import json
import random
import re
import sys
import time
import urllib.parse
import urllib.request
import xml.etree.ElementTree as ET
from pathlib import Path

UA = "PhiTeX-corpus/0.0 (TeX engine research benchmark)"
GAP = 3.5
OAI = "https://oaipmh.arxiv.org/oai"
EPRINT = "https://export.arxiv.org/e-print/"
NS = {"o": "http://www.openarchives.org/OAI/2.0/", "x": "http://arxiv.org/OAI/arXivRaw/"}

# (OAI set, primary-category prefixes, how many to keep): the strata
STRATA = [
    ("math", ("math.",), 30), ("cs", ("cs.",), 34),
    ("physics:hep-th", ("hep-th",), 6), ("physics:hep-ph", ("hep-ph",), 6),
    ("physics:hep-ex", ("hep-ex", "hep-lat"), 6), ("physics:astro-ph", ("astro-ph",), 14),
    ("physics:cond-mat", ("cond-mat",), 16), ("physics:quant-ph", ("quant-ph",), 10),
    ("physics:gr-qc", ("gr-qc", "nucl-th", "nucl-ex"), 6), ("physics:physics", ("physics.",), 10),
    ("q-bio", ("q-bio.",), 6), ("q-fin", ("q-fin.",), 6), ("stat", ("stat.",), 10),
    ("eess", ("eess.",), 10), ("physics:nlin", ("nlin.", "math-ph"), 4),
]
# (datestamp windows of a week: one OAI page each)
WINDOWS = [(f"{y}-{m:02d}-03", f"{y}-{m:02d}-09") for y in (2024, 2025, 2026) for m in range(1, 13)
           if (y, m) <= (2026, 9)]

_last = [0.0]


def get(url):
    """GET with the polite gap; on 429/503 wait (Retry-After, else 60 s, doubling) and retry."""
    import urllib.error
    pause = 60
    for attempt in range(6):
        wait = _last[0] + GAP - time.monotonic()
        if wait > 0:
            time.sleep(wait)
        req = urllib.request.Request(url, headers={"User-Agent": UA})
        try:
            with urllib.request.urlopen(req, timeout=180) as r:
                return r.read(), r.headers.get("Content-Type", "")
        except urllib.error.HTTPError as e:
            if e.code not in (429, 503) or attempt == 5:
                raise
            ra = e.headers.get("Retry-After")
            delay = int(ra) if ra and ra.isdigit() else pause
            print(f"  {e.code}, waiting {delay}s", file=sys.stderr, flush=True)
            time.sleep(delay)
            pause = min(pause * 2, 600)
        finally:
            _last[0] = time.monotonic()
    raise RuntimeError("unreachable")


def pages_of(comment):
    m = re.search(r"(\d+)\s*(?:pages|pp\b|page)", comment or "", re.I)
    return int(m.group(1)) if m else None


def cmd_list(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    rng = random.Random(20261014)
    picked, seen = [], set()
    for oset, prefixes, want in STRATA:
        want = round(want * args.scale)
        pool = []
        for lo, hi in rng.sample(WINDOWS, 3):
            url = OAI + "?" + urllib.parse.urlencode(
                {"verb": "ListRecords", "set": oset, "metadataPrefix": "arXivRaw", "from": lo, "until": hi})
            try:
                body, _ = get(url)
            except Exception as e:  # noqa: BLE001
                print(f"list {oset} {lo}: {e}", file=sys.stderr, flush=True)
                continue
            for r in ET.fromstring(body).iter("{%s}record" % NS["o"]):
                m = r.find(".//x:arXivRaw", NS)
                if m is None:
                    continue
                aid = m.findtext("x:id", "", NS)
                cats = (m.findtext("x:categories", "", NS) or "").split()
                if not re.fullmatch(r"2[456](0[1-9]|1[0-2])\.\d{5}", aid) or not cats:
                    continue
                if not cats[0].startswith(prefixes):
                    continue
                comment = (m.findtext("x:comments", "", NS) or "").strip()
                vers = [v.get("version") for v in m.findall("x:version", NS)]
                pool.append({"id": aid, "version": vers[-1] if vers else "v1", "category": cats[0], "categories": cats,
                             "submitted": (m.find("x:version/x:date", NS).text if m.find("x:version/x:date", NS) is not None else ""), "comment": comment[:200],
                             "pages_claimed": pages_of(comment), "stratum": oset})
        rng.shuffle(pool)
        longs = [p for p in pool if (p["pages_claimed"] or 0) >= 50]
        rest = [p for p in pool if p not in longs]
        n = 0
        for p in longs[: max(1, want // 5)] + rest:
            if n >= want or p["id"] in seen:
                continue
            seen.add(p["id"])
            picked.append(p)
            n += 1
        print(f"{oset}: {n} of {len(pool)} listed", flush=True)
    (out / "candidates.json").write_text(json.dumps(picked, indent=1))
    print(f"{len(picked)} candidates")


def cmd_fetch(args):
    out = Path(args.out)
    cands = json.loads((out / "candidates.json").read_text())
    if args.limit:
        cands = cands[: args.limit]
    for c in cands:
        d = out / f"{c['id']}{c['version']}"
        if (d / "sha256").exists():
            continue
        d.mkdir(parents=True, exist_ok=True)
        try:
            body, ctype = get(EPRINT + c["id"] + c["version"])
        except Exception as e:  # noqa: BLE001
            (d / "fetch-error").write_text(str(e))
            print(f"{c['id']}: {e}", flush=True)
            continue
        (d / "src").write_bytes(body)
        (d / "content-type").write_text(ctype)
        (d / "meta.json").write_text(json.dumps(c, indent=1))
        (d / "sha256").write_text(hashlib.sha256(body).hexdigest() + "\n")
        print(f"{c['id']}{c['version']}: {len(body)} bytes {ctype}", flush=True)


def main():
    ap = argparse.ArgumentParser()
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("list")
    p.add_argument("--out", required=True)
    p.add_argument("--scale", type=float, default=1.6)
    p = sub.add_parser("fetch")
    p.add_argument("--out", required=True)
    p.add_argument("--limit", type=int)
    a = ap.parse_args()
    {"list": cmd_list, "fetch": cmd_fetch}[a.cmd](a)


if __name__ == "__main__":
    main()
