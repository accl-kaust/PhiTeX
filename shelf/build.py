#!/usr/bin/env python3
"""Build Shelf: TeX Live's input files as a static site, one flat URL per name.

    python3 build.py [LABEL]          # default tl2026

From TeX Live's own package archives (tlnet), not a local install: every
package with files under tex/latex, tex/generic or tex/plain is downloaded
(cache/, checked against the package database's SHA-512, kept between runs)
and only those files are taken. Writes public/<LABEL>/<name> for each,
public/<LABEL>/names.txt (every name, sorted, one per line: a client asks
only for names in it), and MANIFEST.tsv (name, package, revision, path,
sha256). Two files with one name: kpathsea's order for LaTeX (tex/latex,
then tex/generic, then tex/plain), then the path's sort order.

TL_MIRROR overrides the mirror (default: mirror.ctan.org's redirector).
"""
import hashlib, lzma, os, shutil, sys, tarfile, urllib.request
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

LABEL = sys.argv[1] if len(sys.argv) > 1 else "tl2026"
MIRROR = os.environ.get("TL_MIRROR", "https://mirror.ctan.org/systems/texlive/tlnet").rstrip("/")
EXTS = {".sty", ".cls", ".clo", ".def", ".cfg", ".fd", ".ldf", ".ltx", ".tex", ".dfu", ".lua"}
TREES = ["latex", "generic", "plain"]  # (search order; context, luatex, ... are other engines')
CACHE = Path("cache")


def get(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=120) as r:
        return r.read()


def packages(db: str) -> list[dict]:
    """Each package with input files: name, revision, checksum, its files (tex/... paths)."""
    out, p = [], None
    for line in db.splitlines() + [""]:
        if line.startswith("name "):
            p = {"name": line[5:], "rev": "?", "sum": None, "files": []}
        elif p is None:
            continue
        elif line.startswith("revision "):
            p["rev"] = line[9:]
        elif line.startswith("containerchecksum "):
            p["sum"] = line[18:]
        elif line.startswith(" "):
            path = line.split()[0].removeprefix("RELOC/").removeprefix("texmf-dist/")
            parts = path.split("/")
            if len(parts) > 2 and parts[0] == "tex" and parts[1] in TREES and Path(path).suffix in EXTS:
                p["files"].append(path)
        elif line == "":
            if p["files"] and "." not in p["name"]:  # (name.arch: binaries)
                out.append(p)
            p = None
    return out


def fetch(p: dict) -> dict[str, bytes]:
    """The package's input files, by tex/... path, from its (cached, checked) archive."""
    f = CACHE / f"{p['name']}.r{p['rev']}.tar.xz"
    if not f.exists():
        data = get(f"{MIRROR}/archive/{p['name']}.tar.xz")
        if p["sum"] and hashlib.sha512(data).hexdigest() != p["sum"]:
            raise SystemExit(f"{p['name']}: checksum mismatch")
        tmp = f.with_suffix(".part")
        tmp.write_bytes(data)
        tmp.replace(f)
    want, got = set(p["files"]), {}
    with tarfile.open(f, "r:xz") as t:
        for m in t:
            name = m.name.removeprefix("texmf-dist/")
            if m.isfile() and name in want:
                got[name] = t.extractfile(m).read()
    return got


def main() -> None:
    CACHE.mkdir(exist_ok=True)
    db = lzma.decompress(get(f"{MIRROR}/tlpkg/texlive.tlpdb.xz")).decode()
    pkgs = packages(db)
    print(f"{len(pkgs)} packages", file=sys.stderr)
    files: dict[str, tuple[dict, bytes]] = {}
    with ThreadPoolExecutor(8) as ex:
        for i, (p, got) in enumerate(zip(pkgs, ex.map(fetch, pkgs)), 1):
            for path, b in got.items():
                files[path] = (p, b)
            if i % 250 == 0:
                print(f"  {i}/{len(pkgs)}", file=sys.stderr)
    # (search order: tree, then path)
    chosen: dict[str, str] = {}
    for path in sorted(files, key=lambda q: (TREES.index(q.split("/")[1]), q)):
        chosen.setdefault(path.rsplit("/", 1)[1], path)
    out = Path("public") / LABEL
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    rows = []
    for name, path in sorted(chosen.items()):
        p, b = files[path]
        (out / name).write_bytes(b)
        rows.append(f"{name}\t{p['name']}\tr{p['rev']}\t{path}\t{hashlib.sha256(b).hexdigest()}")
    (out / "names.txt").write_text("".join(n + "\n" for n in sorted(chosen)))
    Path("MANIFEST.tsv").write_text(f"# {LABEL} from {MIRROR}\n" + "\n".join(rows) + "\n")
    size = sum(f.stat().st_size for f in out.iterdir())
    print(f"{LABEL}: {len(chosen)} files, {size / 1e6:.1f} MB", file=sys.stderr)


main()
