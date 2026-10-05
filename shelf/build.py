#!/usr/bin/env python3
"""Build Shelf: TeX Live's input files and font metrics as a static site,
in packs, with an index from each name to its pack.

    python3 build.py [LABEL]          # default tl2026

From TeX Live's own package archives (tlnet), not a local install: every
package with files under tex/latex, tex/generic, tex/plain, fonts/tfm or
fonts/vf is downloaded (cache/, checked against the package database's
SHA-512, kept between runs) and only those files are taken (no documents,
no images: what TeX reads by name).

    public/<LABEL>/p/<pack>.pack   gzip of  u32 n, (u32 len, name, u32 len, bytes) x n
    public/<LABEL>/index.tsv       name <TAB> pack, sorted: what a client looks up
    MANIFEST.tsv                   name, package, revision, path, sha256

A pack is a TeX Live package's files, as they are used together (a .sty
with its .def, .cfg and .fd files). A package bigger than SPLIT gzipped is
cut, so a document fetches what it reads and not the rest: its inputs by
directory, its fonts by family (the metric's name up to the last '-', or
its first five letters). Cloudflare Pages serves at most 20,000 files a
deployment: the build stops above 19,000 packs.

Two files with one name: kpathsea's order (tex/latex, then tex/generic, then
tex/plain; fonts/tfm, fonts/vf apart, by extension), then the path's sort.

Dependencies (TRACE=path/to/trace-deps and TRACE_FMT=its pdflatex.fmt,
from the extension's core-partex): each .sty and .cls is loaded once on the
preview's own engine, its files read from a flat copy of the tree, and the
packs it read (less what a bare article reads) become the index's third
column. A client fetching a name fetches those packs too, so a package and
everything it loads arrive in one round instead of one build per file:

    public/<LABEL>/index.tsv       name <TAB> pack [<TAB> pack,pack,...]

TL_MIRROR overrides the mirror (default: mirror.ctan.org's redirector).
"""
import gzip, hashlib, lzma, os, re, shutil, struct, subprocess, sys, tarfile, urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

LABEL = sys.argv[1] if len(sys.argv) > 1 else "tl2026"
MIRROR = os.environ.get("TL_MIRROR", "https://mirror.ctan.org/systems/texlive/tlnet").rstrip("/")
TREES = ["tex/latex", "tex/generic", "tex/plain", "fonts/tfm", "fonts/vf", "fonts/type1", "fonts/enc", "fonts/map"]  # (search order)
# (not read by name, or not by pdfTeX: documents, images, other engines' files)
SKIP = {".pdf", ".png", ".jpg", ".jpeg", ".gif", ".svg", ".eps", ".ps", ".txt", ".md", ".html", ".log", ".zip", ".dvi"}
SPLIT = 1 << 20  # gzipped bytes above which a package is cut
BIN = 512 << 10  # about how big (gzipped) each part of a cut package is
CACHE = Path("cache")


def get(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=300) as r:
        return r.read()


def wanted(path: str) -> bool:
    if not any(path.startswith(t + "/") for t in TREES):
        return False
    if path.startswith("fonts/map/"):
        return path.endswith(".map") and path.split("/")[2] in ("dvips", "pdftex")
    if path.startswith("fonts/"):
        return path.endswith((".tfm", ".vf", ".pfb", ".enc"))
    name = path.rsplit("/", 1)[1]
    return "." not in name or Path(name).suffix.lower() not in SKIP


def packages(db: str) -> list[dict]:
    """Each package with files TeX reads by name: name, revision, checksum, files."""
    out, p = [], None
    for line in db.splitlines() + [""]:
        if line.startswith("name "):
            p = {"name": line[5:], "rev": "?", "sum": None, "files": [], "maps": []}
        elif p is None:
            continue
        elif line.startswith("revision "):
            p["rev"] = line[9:]
        elif line.startswith("containerchecksum "):
            p["sum"] = line[18:]
        elif line.startswith("execute addMap ") or line.startswith("execute addMixedMap "):
            p["maps"].append(line.split()[-1])
        elif line.startswith(" "):
            path = line.split()[0].removeprefix("RELOC/").removeprefix("texmf-dist/")
            if wanted(path):
                p["files"].append(path)
        elif line == "":
            if p["files"] and "." not in p["name"]:  # (name.arch: binaries)
                out.append(p)
            p = None
    return out


def fetch(p: dict) -> dict[str, bytes]:
    """The package's files, by path, from its (cached, checked) archive."""
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


def pack(files: list[tuple[str, bytes]]) -> bytes:
    b = bytearray(struct.pack("<I", len(files)))
    for n, data in files:
        nb = n.encode()
        b += struct.pack("<I", len(nb)) + nb + struct.pack("<I", len(data)) + data
    return gzip.compress(bytes(b), 9, mtime=0)


def family(name: str) -> str:
    """A font metric's family: its name up to the last '-' (LinLibertineT-tlf-t1.tfm: LinLibertineT-tlf), else its first five letters."""
    stem = name.rsplit(".", 1)[0]
    return stem.rsplit("-", 1)[0] if "-" in stem else re.sub(r"[0-9]+$", "", stem)[:5] or stem


def groups(pkg: str, files: list[tuple[str, str, bytes]], freq: dict[str, int]) -> dict[str, list[tuple[str, bytes]]]:
    """The packs of one package: (path, name, bytes). A package bigger than
    SPLIT gzipped is cut by use: its hot pack (named as the package) holds
    the files some package's load read (the trace's counts, most read first,
    cut at BIN), the rest go in cold packs by directory and font family."""
    whole = {pkg: [(n, b) for _, n, b in files]}
    if len(gzip.compress(b"".join(b for _, _, b in files), 1)) <= SPLIT:
        return whole
    hot = sorted((f for f in files if freq.get(f[1])), key=lambda f: (-freq[f[1]], f[1]))
    out: dict[str, list] = {}
    cur, size, k = [], 0, 0
    for _, n, b in hot:
        z = len(gzip.compress(b, 1))
        if cur and size + z > BIN:
            out[pkg if k == 0 else f"{pkg}.hot{k}"] = cur
            cur, size, k = [], 0, k + 1
        cur.append((n, b))
        size += z
    if cur:
        out[pkg if k == 0 else f"{pkg}.hot{k}"] = cur
    files = [f for f in files if not freq.get(f[1])]
    keyed: dict[str, list] = defaultdict(list)
    for path, n, b in files:
        if path.startswith("fonts/"):
            key = "f-" + family(n)
        else:
            d = path.split("/")[2:-1]  # (under tex/latex/<pkg>/...)
            key = "-".join(d[1:]) if len(d) > 1 else ""
        keyed[key].append((n, b))
    # (neighbouring keys merged up to about BIN gzipped: a pack per family
    # would make tens of thousands of packs; the first key names the pack)
    # (a key bigger than BIN itself, a font family of 60 MB, goes on in
    # packs of its own, name.2, name.3, ...: Cloudflare takes 25 MiB a file)
    cur, size, name = [], 0, None
    for key in sorted(keyed):
        z = len(gzip.compress(b"".join(b for _, b in keyed[key]), 1))
        if cur and size + z > BIN:
            out[name] = cur
            cur, size = [], 0
        if not cur:
            name = f"{pkg}.{key}" if key else f"{pkg}.cold"
        if z <= BIN:
            cur += keyed[key]
            size += z
            continue
        part = 1
        for n, b in keyed[key]:
            zb = len(gzip.compress(b, 1))
            if cur and size + zb > BIN:
                out[name] = cur
                part += 1
                cur, size, name = [], 0, f"{pkg}.{key}.{part}"
            cur.append((n, b))
            size += zb
    if cur:
        out[name] = cur
    return out


def trace(chosen: dict, files: dict) -> dict[str, set[str]]:
    """Each .sty and .cls (and article.cls, the baseline): the files loading it reads (TRACE, TRACE_FMT)."""
    tool, fmt = os.environ.get("TRACE"), os.environ.get("TRACE_FMT")
    if not (tool and fmt):
        print("no TRACE/TRACE_FMT: index without dependencies", file=sys.stderr)
        return {}
    flat = CACHE / "flat"
    if flat.exists():
        shutil.rmtree(flat)
    flat.mkdir()
    for name, path in chosen.items():
        (flat / name).write_bytes(files[path][1])
    names = sorted(n for n in chosen if n.endswith((".sty", ".cls")))
    batches = [names[i : i + 25] for i in range(0, len(names), 25)]

    def run(batch: list[str]) -> list[str]:
        try:
            r = subprocess.run([tool, fmt, str(flat), *batch], capture_output=True, text=True, timeout=300)
            return r.stdout.splitlines()
        except subprocess.TimeoutExpired as e:  # (a job that never ends: its batch's done lines only)
            return (e.stdout or b"").decode(errors="replace").splitlines() if isinstance(e.stdout, bytes) else (e.stdout or "").splitlines()

    read: dict[str, set[str]] = {}
    with ThreadPoolExecutor(int(os.environ.get("TRACE_JOBS", "12"))) as ex:
        for i, lines in enumerate(ex.map(run, [["article.cls"]] + batches)):
            for line in lines:
                name, _, fs = line.partition("\t")
                read[name] = set(filter(None, fs.split(",")))
            if i % 40 == 0:
                print(f"  traced {i}/{len(batches)} batches", file=sys.stderr)
    missing = len(names) - len(read)
    if missing:
        print(f"  {missing} names not traced (a batch timed out)", file=sys.stderr)
    return read


def safe(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", s)


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
    # each name's file: the tree's order, then the path
    rank = lambda q: (next(i for i, t in enumerate(TREES) if q.startswith(t + "/")), q)
    chosen: dict[str, str] = {}
    for path in sorted(files, key=rank):
        chosen.setdefault(path.rsplit("/", 1)[1], path)
    # pdftex.map, as updmap makes it: every package's Map and MixedMap
    # files, in package order (a font named twice: the first wins)
    maps = {path.rsplit("/", 1)[1]: b for path, (_, b) in files.items() if path.startswith("fonts/map/")}
    seen, lines = set(), []
    # (updmap's default, pdftexDownloadBase14: the base 14 fonts embedded,
    # URW's, from pdftex_dl14.map, ahead of everything; partex has no
    # non-embedded fonts)
    for p in [{"maps": ["pdftex_dl14.map"]}] + pkgs:
        for m in p["maps"]:
            for line in maps.get(m, b"").decode("latin-1").splitlines():
                t = line.split()
                if not t or line.lstrip().startswith(("%", "#")) or t[0] in seen:
                    continue
                seen.add(t[0])
                lines.append(line)
    pdftex_map = ("\n".join(lines) + "\n").encode("latin-1")
    print(f"pdftex.map: {len(lines)} fonts, {len(pdftex_map) / 1e6:.1f} MB", file=sys.stderr)
    for path in [q for q in chosen.values() if q.startswith("fonts/map/")]:
        del chosen[path.rsplit("/", 1)[1]]
    chosen["pdftex.map"] = "fonts/map/pdftex/updmap/pdftex.map"
    files["fonts/map/pdftex/updmap/pdftex.map"] = ({"name": "pdftex-map", "rev": "updmap"}, pdftex_map)
    by_pkg: dict[str, list] = defaultdict(list)
    for name, path in chosen.items():
        p, b = files[path]
        by_pkg[p["name"]].append((path, name, b))
    out = Path("public") / LABEL
    if out.exists():
        shutil.rmtree(out)
    (out / "p").mkdir(parents=True)
    index, rows, sizes = [], [], []
    read = trace(chosen, files)
    base = read.get("article.cls", set())
    freq: dict[str, int] = defaultdict(int)
    for fs in read.values():
        for f in fs:
            freq[f] += 1
    for pkg, fs in sorted(by_pkg.items()):
        fs.sort()
        for key, members in groups(pkg, fs, freq).items():
            data = pack(sorted(members))
            (out / "p" / f"{safe(key)}.pack").write_bytes(data)
            sizes.append((len(data), key))
            index += [f"{n}\t{safe(key)}" for n, _ in members]
        for path, name, b in fs:
            p = files[path][0]
            rows.append(f"{name}\t{pkg}\tr{p['rev']}\t{path}\t{hashlib.sha256(b).hexdigest()}")
    pack_of = dict(l.split("\t") for l in index)
    deps = {name: sorted({pack_of[f] for f in fs - base if f in pack_of} - {pack_of.get(name)}) for name, fs in read.items()}
    (out / "index.tsv").write_text(
        "".join(sorted(l + ("\t" + ",".join(deps[l.split("\t")[0]]) if deps.get(l.split("\t")[0]) else "") + "\n" for l in index))
    )
    print(f"{sum(1 for v in deps.values() if v)} names with dependency packs", file=sys.stderr)
    Path("MANIFEST.tsv").write_text(f"# {LABEL} from {MIRROR}\n" + "\n".join(sorted(rows)) + "\n")
    sizes.sort(reverse=True)
    total = sum(s for s, _ in sizes)
    print(f"{LABEL}: {len(chosen)} names in {len(sizes)} packs, {total / 1e6:.1f} MB; largest: "
          + ", ".join(f"{k} {s / 1e6:.1f} MB" for s, k in sizes[:5]), file=sys.stderr)
    if len(sizes) > 19_000:
        raise SystemExit(f"{len(sizes)} packs: over Cloudflare Pages' 20,000 files")
    big = [k for s, k in sizes if s > 25 * 2**20]
    if big:
        raise SystemExit(f"packs over Cloudflare's 25 MiB: {big}")


main()
