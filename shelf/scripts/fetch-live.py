"""The releases a host serves, fetched into a releases directory, so a new
release is made after them and a deployment keeps them (a Pages
deployment replaces the whole site).

    fetch-live.py URL LABEL DIR [--clean]   # https://v1.shelf.phitex.org tl2026 releases-v1

From URL/LABEL/: release.json (the current release), each kept
release-LABEL.N.json (N from 1 to the current one; a missing one was not
kept), each one's index and pack table, every pack those indexes name,
every format pack their `formats` name and every pack keep-packs.txt
pins, each checked against its name's hash. A format the host no longer
serves (deployed before the formats' packs were fetched here: each
formats deploy dropped the earlier identities' packs while release.json
still named them) is taken out of its release's `formats` and pack table,
said loudly: a release never names a pack the site does not hold.

`--clean`: the current release alone, without its formats and without
keep-packs.txt's pins; the deployment made from it serves only that
release and the formats added after (a clean start: the earlier
releases and identities are no longer served).
"""

import gzip
import hashlib
import json
import sys
import time
import urllib.error
import urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path


def get(url: str, fresh: bool = False) -> bytes | None:
    """`url`'s bytes, none: not served. `fresh`: the origin's, past every
    cache (a query of its own): what is not named by its content (the
    release files, a pack table before 11-11, rewritten in place under
    `immutable` while the CDN kept the first copy: the v1.0 clean
    deploy's table, without the formats it named, served stale for days)."""
    if fresh:
        url += f"?fresh={time.time_ns()}"
    # (a user agent of its own: Cloudflare answers urllib's default,
    # "Python-urllib/3.x", with 403 Forbidden, and the release stopped
    # before it began, GitHub run 41293613277)
    req = urllib.request.Request(url, headers={"User-Agent": "phitex-shelf-release"})
    try:
        with urllib.request.urlopen(req, timeout=300) as r:
            return r.read()
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None
        raise


def write_table(rel: Path, release: str, table: str) -> str:
    """`table` written as the release's pack table, named by its content
    (`packs-<release>-<sha256 12>.tsv.gz`): the site's files are cached
    as `immutable`, so a table that changes is a new file, never the old
    name rewritten (release.py's `write_table`)."""
    data = gzip.compress(table.encode(), 9, mtime=0)
    name = f"packs-{release}-{hashlib.sha256(data).hexdigest()[:12]}.tsv.gz"
    (rel / name).write_bytes(data)
    return name


def drop_formats(rel: Path, doc: dict, gone: set[str]) -> None:
    """`gone` (format names) out of the release `doc` (its
    release-<release>.json, and release.json when it is the current one)
    and out of its pack table."""
    packs = {doc["formats"][f] for f in gone}
    doc["formats"] = {f: p for f, p in doc["formats"].items() if f not in gone}
    lines = [l for l in gzip.decompress((rel / doc["pack_table"]).read_bytes()).decode().splitlines() if l and l.split("\t")[0] not in packs]
    doc["pack_table"] = write_table(rel, doc["release"], "".join(l + "\n" for l in lines))
    doc["packs"] = len(lines)
    text = json.dumps(doc, indent=1) + "\n"
    (rel / f"release-{doc['release']}.json").write_text(text)
    cur = json.loads((rel / "release.json").read_text())
    if cur["release"] == doc["release"]:
        (rel / "release.json").write_text(text)


def main() -> None:
    url, label, out = sys.argv[1].rstrip("/"), sys.argv[2], Path(sys.argv[3])
    clean = sys.argv[4:] == ["--clean"]
    base, rel = f"{url}/{label}", out / label
    (rel / "h").mkdir(parents=True, exist_ok=True)
    cur = get(f"{base}/release.json", fresh=True)
    if cur is None:
        raise SystemExit(f"{base}/release.json: not found")
    current = json.loads(cur)
    (rel / "release.json").write_bytes(cur)
    n = int(current["release"].rsplit(".", 1)[1])
    packs: set[str] = set()
    docs: list[dict] = []
    for i in range(n if clean else 1, n + 1):
        name = f"{label}.{i}"
        b = get(f"{base}/release-{name}.json", fresh=True)
        if b is None:
            continue
        doc = json.loads(b)
        (rel / f"release-{name}.json").write_bytes(b)
        for key in ("index", "pack_table"):
            data = get(f"{base}/{doc[key]}", fresh=True)
            if data is None:
                raise SystemExit(f"{base}/{doc[key]}: named by release-{name}.json, not served")
            (rel / doc[key]).write_bytes(data)
        for line in gzip.decompress((rel / doc["index"]).read_bytes()).decode().splitlines():
            if "\t" in line:
                packs.add(line.split("\t")[1])
        if clean and doc.get("formats"):
            print(f"{name}: --clean: its formats dropped: {' '.join(doc['formats'])}", file=sys.stderr)
            drop_formats(rel, doc, set(doc["formats"]))
        docs.append(doc)
        print(f"{name}: kept", file=sys.stderr)
    if clean and not docs:
        raise SystemExit(f"{base}/release-{current['release']}.json: not served")
    pinned = Path(__file__).resolve().parent.parent / "keep-packs.txt"
    if pinned.exists() and not clean:
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
        for doc in docs:
            fmts = doc.get("formats") or {}
            got = dict(zip(fmts, ex.map(one, fmts.values())))
            gone = {f for f, size in got.items() if not size}
            if gone:
                print(f"{doc['release']}: NOT SERVED, taken out of its formats: {' '.join(sorted(gone))}", file=sys.stderr)
                drop_formats(rel, doc, gone)
            sizes += [s for s in got.values() if s]
    print(f"{label}: {sum(1 for s in sizes if s)} packs, {sum(sizes) / 1e9:.2f} GB, current {current['release']}", file=sys.stderr)


if __name__ == "__main__":
    main()
