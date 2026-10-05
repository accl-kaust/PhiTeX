#!/usr/bin/env python3
"""Make a Shelf release from a build: packs by content, an index for it, and
release.json, the one file that changes.

    python3 release.py [LABEL] [--min-extension X.Y.Z] [--notice TEXT]

A pack's released name is its build name and a hash of its bytes
(`pgf-3f9a2c41b0de.pack`), under <LABEL>/h/. A released pack never changes
and is never removed: every index ever published keeps working, whatever a
later build splits differently. They accumulate in releases/<LABEL>/
(git-ignored, kept between builds; build.py rebuilds public/ from scratch),
which deploy.sh copies into public/ before uploading.

    releases/<LABEL>/h/<pack>-<hash>.pack    every released pack, by content
    releases/<LABEL>/index-<release>.tsv.gz  name <TAB> pack [<TAB> deps], released names
    releases/<LABEL>/release.json            the current release, below

release.json, read by the extension at most once a day:

    {"release": "tl2026.3", "index": "index-tl2026.3.tsv.gz", "schema": 2,
     "min_extension": "0.2.0", "notice": null, "packs": 6289}

`schema`: the index and pack format (2: hashed pack names under h/, the
dependency column). An extension that reads an older schema keeps the last
index it could read. `min_extension`: older extensions ask their user to
update (they keep working). `notice`: a line shown to every user, or null.
The build's own p/<pack>.pack names stay too, for extensions that predate
releases.
"""
import gzip, hashlib, json, shutil, sys
from pathlib import Path

SCHEMA = 2


def main() -> None:
    args = sys.argv[1:]
    opt = {}
    for k in ("--min-extension", "--notice"):
        if k in args:
            i = args.index(k)
            opt[k] = args[i + 1]
            del args[i : i + 2]
    label = args[0] if args else "tl2026"
    here = Path(__file__).parent
    pub, rel = here / "public" / label, here / "releases" / label
    (rel / "h").mkdir(parents=True, exist_ok=True)
    # (no build to release: p/ is removed by a deploy; build.py makes it)
    if not (pub / "p").is_dir():
        print(f"{label}: no build in public/{label}/p: the current release stands")
        return

    # each pack by its content: a pack whose bytes a release already has
    # keeps that name
    names = {}
    for p in sorted((pub / "p").glob("*.pack")):
        h = hashlib.sha256(p.read_bytes()).hexdigest()[:12]
        names[p.stem] = f"{p.stem}-{h}"
        dst = rel / "h" / f"{names[p.stem]}.pack"
        if not dst.exists():
            shutil.copyfile(p, dst)

    rows = []
    for line in (pub / "index.tsv").read_text().splitlines():
        cols = line.split("\t")
        if len(cols) < 2 or cols[1] not in names:
            continue
        row = [cols[0], names[cols[1]]]
        if len(cols) > 2 and cols[2]:
            row.append(",".join(names[d] for d in cols[2].split(",") if d in names))
        rows.append("\t".join(row))
    body = ("\n".join(rows) + "\n").encode()

    old = json.loads((rel / "release.json").read_text()) if (rel / "release.json").exists() else {}
    # (an index the same as the current release's: no new release)
    if old.get("index") and (rel / old["index"]).exists() and gzip.decompress((rel / old["index"]).read_bytes()) == body \
            and old.get("min_extension") == opt.get("--min-extension", old.get("min_extension")) \
            and old.get("notice") == opt.get("--notice", old.get("notice")):
        print(f"{label}: release {old['release']} unchanged")
        return
    n = int(old.get("release", f"{label}.0").rsplit(".", 1)[1]) + 1 if old else 1
    release = f"{label}.{n}"
    index = f"index-{release}.tsv.gz"
    (rel / index).write_bytes(gzip.compress(body, 9, mtime=0))
    meta = {
        "release": release,
        "index": index,
        "schema": SCHEMA,
        "min_extension": opt.get("--min-extension", old.get("min_extension", "0.0.0")),
        "notice": opt.get("--notice", old.get("notice")) or None,
        "packs": len(names),
    }
    (rel / "release.json").write_text(json.dumps(meta, indent=1) + "\n")
    print(f"{label}: release {release}: {len(rows)} names, {len(names)} packs, "
          f"{sum(1 for _ in (rel / 'h').iterdir())} released packs in all")


if __name__ == "__main__":
    main()
