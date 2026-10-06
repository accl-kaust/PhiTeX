#!/usr/bin/env python3
"""Make a Shelf release (schema 3) from a build: packs by content, an index
for it, and release.json, the one file that changes; then drop the packs no
kept release needs.

    python3 release.py [LABEL] [--releases DIR] [--min-extension X.Y.Z]
                       [--notice TEXT] [--keep N]

From build/<LABEL>/ (build.py's: packs/, index.tsv, meta.json) into
<DIR>/<LABEL>/ (default releases/, git-ignored, kept between builds):

    <DIR>/<LABEL>/h/<pack>-<hash>.pack     every kept pack, named by its content
    <DIR>/<LABEL>/index-<release>.tsv.gz   path <TAB> pack <TAB> deps... (released pack names)
    <DIR>/<LABEL>/release.json             the current release, below

A pack's released name is its build name and a hash of its bytes
(`pgf-3f9a2c41b0de.pack`): a released pack never changes, so a client may
cache it forever and every kept index stays whole.

release.json, read by the extension at most once a day:

    {"schema": 3, "release": "tl2026.3", "index": "index-tl2026.3.tsv.gz",
     "min_extension": "0.3.0", "notice": null, "packs": 6950,
     "deps": ["pdftex", "xetex"],
     "search": {"pdftex": {"tex": ["tex/latex", "tex/generic", "tex"], ...}, ...}}

`schema` 3: the index is by texmf path, a dependency column per engine
named in `deps`, and `search` the engines' search orders per kpathsea
format (build.py's module doc has the rule). `min_extension`: older
extensions ask their user to update. `notice`: a line shown to every user,
or null. Release numbers go on from <DIR>'s release.json, or, for a new
<DIR>, from releases/<LABEL>/release.json (read only), so a release is never
numbered below one already published.

GC: only the packs the newest KEEP releases' indexes name (default 3), and
those keep-packs.txt lists (one released pack name a line: the packs the
current extension bundles, written by its scripts/bundle-packs.sh), stay in
h/; older indexes go too. At most 19,500 files in all (Cloudflare Pages
serves 20,000 a deployment, with the site's own few).
"""
import gzip, hashlib, json, re, shutil, sys
from pathlib import Path

SCHEMA = 3
LIMIT = 19_500


def main() -> None:
    args = sys.argv[1:]
    opt = {}
    for k in ("--min-extension", "--notice", "--releases", "--keep"):
        if k in args:
            i = args.index(k)
            opt[k] = args[i + 1]
            del args[i : i + 2]
    label = args[0] if args else "tl2026"
    here = Path(__file__).parent
    build, rel = here / "build" / label, here / opt.get("--releases", "releases") / label
    keep = int(opt.get("--keep", "3"))
    if not (build / "index.tsv").exists():
        print(f"{label}: no build in build/{label}: the current release stands")
        return
    meta = json.loads((build / "meta.json").read_text())
    (rel / "h").mkdir(parents=True, exist_ok=True)

    # each pack by its content: a pack whose bytes a release already has
    # keeps that name
    names = {}
    for p in sorted((build / "packs").glob("*.pack")):
        b = p.read_bytes()
        names[p.stem] = f"{p.stem}-{hashlib.sha256(b).hexdigest()[:12]}"
        dst = rel / "h" / f"{names[p.stem]}.pack"
        if not dst.exists():
            dst.write_bytes(b)

    rows = []
    for line in (build / "index.tsv").read_text().splitlines():
        cols = line.split("\t")
        row = [cols[0], names[cols[1]]] + [",".join(names[d] for d in c.split(",") if d in names) for c in cols[2:]]
        rows.append("\t".join(row))
    body = ("\n".join(rows) + "\n").encode()

    old = json.loads((rel / "release.json").read_text()) if (rel / "release.json").exists() else None
    want = {
        "min_extension": opt.get("--min-extension", (old or {}).get("min_extension", "0.3.0")),
        "notice": opt.get("--notice", (old or {}).get("notice")) or None,
        "deps": meta["deps"],
        "search": meta["search"],
    }
    if old and old.get("schema") == SCHEMA and (rel / old["index"]).exists() \
            and gzip.decompress((rel / old["index"]).read_bytes()) == body \
            and all(old.get(k) == v for k, v in want.items()):
        print(f"{label}: release {old['release']} unchanged")
    else:
        # (numbered on from this directory's release, or the published one)
        prev = old or next((json.loads(f.read_text()) for f in [here / "releases" / label / "release.json"] if f.exists()), None)
        n = int(prev["release"].rsplit(".", 1)[1]) + 1 if prev else 1
        release = f"{label}.{n}"
        index = f"index-{release}.tsv.gz"
        (rel / index).write_bytes(gzip.compress(body, 9, mtime=0))
        doc = {"schema": SCHEMA, "release": release, "index": index, **{k: want[k] for k in ("min_extension", "notice")},
               "packs": len(names), "deps": want["deps"], "search": want["search"]}
        (rel / "release.json").write_text(json.dumps(doc, indent=1) + "\n")
        print(f"{label}: release {release}: {len(rows)} paths, {len(names)} packs")

    # GC: the newest `keep` releases' packs, and keep-packs.txt's
    num = lambda f: int(re.search(r"\.(\d+)\.tsv\.gz$", f.name).group(1))
    indexes = sorted(rel.glob(f"index-{label}.*.tsv.gz"), key=num, reverse=True)
    live: set[str] = set()
    for f in indexes[:keep]:
        for line in gzip.decompress(f.read_bytes()).decode().splitlines():
            cols = line.split("\t")
            live.add(cols[1])
            for c in cols[2:]:
                live.update(filter(None, c.split(",")))
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
    for f in indexes[keep:]:
        f.unlink()
    n_files = sum(1 for p in rel.rglob("*") if p.is_file())
    print(f"GC: kept {len(indexes[:keep])} releases' packs and keep-packs.txt's; dropped {len(dropped)} packs, "
          f"{len(indexes[keep:])} older indexes; {n_files} files in {rel}")
    if n_files > LIMIT:
        raise SystemExit(f"{n_files} files: over {LIMIT} (Cloudflare Pages serves 20,000)")


if __name__ == "__main__":
    main()
