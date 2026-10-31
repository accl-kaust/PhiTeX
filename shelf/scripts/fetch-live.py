#!/usr/bin/env python3
"""The releases a host serves, fetched into a releases directory, so a new
release is made after them and a deployment keeps them (a Pages
deployment replaces the whole site).

    fetch-live.py URL LABEL DIR      # https://v1.shelf.phitex.org tl2026 releases-v1

From URL/LABEL/: release.json (the current release), each kept
release-LABEL.N.json (N from 1 to the current one; a missing one was not
kept), each one's index and pack table, every pack those indexes name and
every pack keep-packs.txt pins, each checked against its name's hash.
"""

import gzip
import hashlib
import json
import sys
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def get(url: str) -> bytes | None:
    try:
        with urllib.request.urlopen(url, timeout=300) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def main() -> None:
    url, label, out = sys.argv[1].rstrip("/"), sys.argv[2], Path(sys.argv[3])
    base, rel = f"{url}/{label}", out / label
    (rel / "h").mkdir(parents=True, exist_ok=True)
    cur = get(f"{base}/release.json")
    if cur is None:
        raise SystemExit(f"{base}/release.json: not found")
    current = json.loads(cur)
    (rel / "release.json").write_bytes(cur)
    n = int(current["release"].rsplit(".", 1)[1])
    packs: set[str] = set()
    for i in range(1, n + 1):
        name = f"{label}.{i}"
        b = get(f"{base}/release-{name}.json")
        if b is None:
            continue
        doc = json.loads(b)
        (rel / f"release-{name}.json").write_bytes(b)
        for key in ("index", "pack_table"):
            data = get(f"{base}/{doc[key]}")
            if data is None:
                raise SystemExit(f"{base}/{doc[key]}: named by release-{name}.json, not served")
            (rel / doc[key]).write_bytes(data)
        for line in gzip.decompress((rel / doc["index"]).read_bytes()).decode().splitlines():
            if "\t" in line:
                packs.add(line.split("\t")[1])
        print(f"{name}: kept", file=sys.stderr)
    pinned = Path(__file__).resolve().parent.parent / "keep-packs.txt"
    if pinned.exists():
        packs |= {l.strip() for l in pinned.read_text().splitlines() if l.strip() and not l.startswith("#")}

    def one(p: str) -> int:
        f = rel / "h" / f"{p}.pack"
        if not f.exists():
            data = get(f"{base}/h/{p}.pack")
            if data is None:
                return 0  # (a pinned pack never released here)
            # (a pack is named by its content: <name>-<first 12 of sha256>)
            if hashlib.sha256(data).hexdigest()[:12] != p.rsplit("-", 1)[1]:
                raise SystemExit(f"h/{p}.pack: content does not match its name")
            f.write_bytes(data)
        return f.stat().st_size

    with ThreadPoolExecutor(16) as ex:
        sizes = list(ex.map(one, sorted(packs)))
    print(f"{label}: {sum(1 for s in sizes if s)} packs, {sum(sizes) / 1e9:.2f} GB, current {current['release']}", file=sys.stderr)


if __name__ == "__main__":
    main()
