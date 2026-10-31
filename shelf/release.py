#!/usr/bin/env python3
"""Make a Shelf release (schema 1) from a build: packs by content, an index
and a pack table for it, and release.json, the one file that changes; then
drop the packs no kept release needs.

    python3 release.py [LABEL] [--releases DIR] [--min-extension X.Y.Z]
                       [--notice TEXT]

From build/<LABEL>/ (build.py's: packs/, index.tsv, meta.json) into
<DIR>/<LABEL>/ (default releases-v1/, git-ignored, kept between builds):

    <DIR>/<LABEL>/h/<pack>-<hash>.pack     every kept pack, named by its content
    <DIR>/<LABEL>/index-<release>.tsv.gz   path <TAB> pack <TAB> deps... (released pack names)
    <DIR>/<LABEL>/packs-<release>.tsv.gz   pack <TAB> sha256 <TAB> bytes: every pack of the release
    <DIR>/<LABEL>/release-<release>.json   that release's release.json, kept
    <DIR>/<LABEL>/release.json             the current release, below

A pack's released name is its build name and a hash of its bytes
(`pgf-3f9a2c41b0de.pack`): a released pack never changes, so a client may
cache it forever. The pack table has each pack's full SHA-256 (of the
.pack file as served: its gzip bytes) and size: a client checks every
pack it fetches against it before using it (a truncated or altered pack,
from a cache, a proxy or a mirror, is refused, never read).

release.json, read by the extension at most once a day:

    {"schema": 1, "release": "tl2026.6", "index": "index-tl2026.6.tsv.gz",
     "pack_table": "packs-tl2026.6.tsv.gz", "packs": 6950,
     "min_extension": "0.6.0", "notice": null,
     "deps": ["pdftex", "xetex"],
     "traced_by": {"pdftex": "phitex <commit> shelf-trace 1", "xetex": ...},
     "search": {"pdftex": {"tex": ["tex/latex", "tex/generic", "tex"], ...}, ...}}

`schema` 1: Shelf's public protocol, the N of vN.shelf.phitex.org,
counted from 1 at PhiTeX's alpha (the development formats before it, 1 to
4, were never released): the index by texmf path, a dependency column per
engine named in `deps`, `search` the engines' search orders per kpathsea
format (build.py's module doc), and the pack table, `pack_table`.
`traced_by`: per column, the engine and commit whose trace made it (null:
an empty column). The columns are hints: a client that fetches only what
its build asks for is still right, so a column traced by another engine
version than the client's costs rounds, never correctness.
`min_extension`: older extensions ask their user to update (given with
--min-extension; else the last release's). `notice`: a line shown to every
user, or null. Release numbers go on from the newest release made in any
releases*/<LABEL>/ here (release.json and the kept release-*.json), so a
release is never numbered below one already made.

Retention: every release's index, pack table and release-<release>.json
stay for ever, and so does every pack a kept index names (a project
pinned to an older release builds again as it did: its index names its
packs, the table their hashes); a pack goes only when no kept index names
it (and keep-packs.txt, one released pack name a line, does not list it).
Content-addressed packs are shared between releases, so a release adds
only the packs TeX Live's updates changed. The files the deploy holds are
counted: past WARN a loud warning, past LIMIT (Cloudflare Pages' free
plan serves 20,000 a deployment, with the site's own few) no release.
README's "Retention" has the numbers and the ways past the limit.
"""
import gzip, hashlib, json, sys
from pathlib import Path

SCHEMA = 1
LIMIT = 19_500
WARN = 15_000


def num(release: str) -> int:
    return int(release.rsplit(".", 1)[1])


def named(index: Path) -> set[str]:
    """The packs an index names: its files' packs and its dependency columns'."""
    out: set[str] = set()
    for line in gzip.decompress(index.read_bytes()).decode().splitlines():
        cols = line.split("\t")
        out.add(cols[1])
        for c in cols[2:]:
            out.update(filter(None, c.split(",")))
    return out


def main() -> None:
    args = sys.argv[1:]
    opt = {}
    for k in ("--min-extension", "--notice", "--releases"):
        if k in args:
            i = args.index(k)
            opt[k] = args[i + 1]
            del args[i : i + 2]
    label = args[0] if args else "tl2026"
    here = Path(__file__).parent
    build, rel = here / "build" / label, here / opt.get("--releases", "releases-v1") / label
    if not (build / "index.tsv").exists():
        print(f"{label}: no build in build/{label}: the current release stands")
        return
    meta = json.loads((build / "meta.json").read_text())
    (rel / "h").mkdir(parents=True, exist_ok=True)

    # each pack by its content: a pack whose bytes a release already has
    # keeps that name
    names, table = {}, {}
    for p in sorted((build / "packs").glob("*.pack")):
        h = hashlib.sha256(p.read_bytes()).hexdigest()
        names[p.stem] = f"{p.stem}-{h[:12]}"
        table[names[p.stem]] = f"{h}\t{p.stat().st_size}"
    # (the files the deploy would hold with this release, counted before
    # anything is written: over LIMIT, no release)
    new = sum(1 for n in names.values() if not (rel / "h" / f"{n}.pack").exists())
    check(sum(1 for p in rel.parent.rglob("*") if p.is_file()) + site_files(here) + new + 3, len(set(names.values())))
    for stem, n in names.items():
        dst = rel / "h" / f"{n}.pack"
        if not dst.exists():
            dst.write_bytes((build / "packs" / f"{stem}.pack").read_bytes())

    rows = []
    for line in (build / "index.tsv").read_text().splitlines():
        cols = line.split("\t")
        row = [cols[0], names[cols[1]]] + [",".join(names[d] for d in c.split(",") if d in names) for c in cols[2:]]
        rows.append("\t".join(row))
    body = ("\n".join(rows) + "\n").encode()
    packs = "".join(f"{n}\t{t}\n" for n, t in sorted(table.items())).encode()

    old = json.loads((rel / "release.json").read_text()) if (rel / "release.json").exists() else None
    want = {
        "min_extension": opt.get("--min-extension", (old or {}).get("min_extension")),
        "notice": opt.get("--notice", (old or {}).get("notice")) or None,
        "deps": meta["deps"],
        "traced_by": {e: meta.get("traced_by", {}).get(e) for e in meta["deps"]},
        "search": meta["search"],
    }
    if not want["min_extension"]:
        raise SystemExit("--min-extension X.Y.Z: the oldest extension that reads schema 1 (no release here to take it from)")
    if old and old.get("schema") == SCHEMA and (rel / old["index"]).exists() \
            and gzip.decompress((rel / old["index"]).read_bytes()) == body \
            and all(old.get(k) == v for k, v in want.items()):
        print(f"{label}: release {old['release']} unchanged")
    else:
        made = [json.loads(f.read_text()).get("release", "") for f in here.glob(f"releases*/{label}/release*.json")]
        n = max((num(r) for r in made if r.startswith(label + ".")), default=0) + 1
        release = f"{label}.{n}"
        index, pack_table = f"index-{release}.tsv.gz", f"packs-{release}.tsv.gz"
        (rel / index).write_bytes(gzip.compress(body, 9, mtime=0))
        (rel / pack_table).write_bytes(gzip.compress(packs, 9, mtime=0))
        # (the build's MANIFEST.tsv, published beside the index, not committed)
        if (here / "MANIFEST.tsv").exists():
            (rel / f"manifest-{release}.tsv.gz").write_bytes(gzip.compress((here / "MANIFEST.tsv").read_bytes(), 9, mtime=0))
        doc = {"schema": SCHEMA, "release": release, "index": index, "pack_table": pack_table, "packs": len(names),
               **{k: want[k] for k in ("min_extension", "notice", "deps", "traced_by", "search")}}
        text = json.dumps(doc, indent=1) + "\n"
        (rel / f"release-{release}.json").write_text(text)
        (rel / "release.json").write_text(text)
        print(f"{label}: release {release}: {len(rows)} paths, {len(names)} packs")

    # GC: every index kept, and every pack one names; keep-packs.txt's
    indexes = sorted(rel.glob(f"index-{label}.*.tsv.gz"), key=lambda f: num(f.name.removesuffix(".tsv.gz")), reverse=True)
    live: set[str] = set()
    for f in indexes:
        live |= named(f)
    pinned = here / "keep-packs.txt"
    if pinned.exists():
        listed = {l.strip() for l in pinned.read_text().splitlines() if l.strip() and not l.startswith("#")}
        gone = sorted(p for p in listed if not (rel / "h" / f"{p}.pack").exists())
        if gone:
            print(f"keep-packs.txt: {len(gone)} packs not in h/ (never released here): {gone[:5]}")
        live |= listed
    dropped = [p for p in (rel / "h").glob("*.pack") if p.stem not in live]
    for p in dropped:
        p.unlink()
    n_files = sum(1 for p in rel.parent.rglob("*") if p.is_file()) + site_files(here)
    print(f"GC: {len(indexes)} releases' indexes and every pack they name kept; dropped {len(dropped)} packs; "
          f"the deploy holds {n_files} files ({len(live)} packs)")
    check(n_files, len(live))


def site_files(here: Path) -> int:
    """The site's own files deployed beside the releases (public/, less its tl* copies)."""
    pub = here / "public"
    return sum(1 for p in pub.rglob("*") if p.is_file() and not p.relative_to(pub).parts[0].startswith("tl"))


def check(n_files: int, packs: int) -> None:
    """The files the deploy holds (every label's releases and the site's
    own): past WARN a loud warning, past LIMIT no release."""
    ways = ("Cloudflare Pages' paid plan (100,000 files a deployment), or the packs in R2 behind a custom "
            "domain at the same URLs (README: Retention)")
    if n_files > LIMIT:
        raise SystemExit(f"{n_files} files ({packs} packs): over {LIMIT}, Cloudflare Pages' free plan serves 20,000 "
                         f"a deployment; no release: {ways}")
    if n_files > WARN:
        print(f"!!! WARNING: {n_files} files ({packs} packs), past {WARN} of the {LIMIT} Cloudflare Pages' free plan "
              f"takes: plan the move now: {ways} !!!", file=sys.stderr)


if __name__ == "__main__":
    main()
