#!/usr/bin/env python3
"""Build Shelf (schema 4): TeX Live's files for the engines PhiTeX runs, in
packs, with an index from each file's texmf path to its pack.

    python3 build.py [LABEL]          # default tl2026; then release.py

From TeX Live's own package archives (tlnet), not a local install: every
package with files in TREES is downloaded (cache/, checked against the
package database's SHA-512, kept between runs) and only those files are
taken (no documents, no images: what an engine, BibTeX or makeindex reads
by name). The build writes build/<LABEL>/ (git-ignored); release.py makes
a release of it (content-named packs under h/, an index, release.json):

    build/<LABEL>/packs/<pack>.pack   gzip of  u32 n, (u32 len, path, u32 len, bytes) x n
    build/<LABEL>/index.tsv           path <TAB> pack <TAB> deps_pdftex <TAB> deps_xetex
    build/<LABEL>/meta.json           {"deps": ["pdftex", "xetex"], "search": {...}}
    MANIFEST.tsv                      path, package, revision, sha256

Every file is keyed by its texmf path (tex/latex/fontspec/fontspec.sty,
fonts/tfm/public/lm/rm-lmr10.tfm, fonts/opentype/public/lm/lmroman10-regular.otf),
in the index and inside packs. The one file not from a package, pdftex.map,
is made as updmap makes it (every package's Map and MixedMap files, in
package order) at the path updmap gives it,
fonts/map/pdftex/updmap/pdftex.map; the map files it is made from are not
served (dvipdfmx's own, fonts/map/dvipdfmx/, are: its dvipdfmx.cfg names
them). Beyond what TeX reads, what xdvipdfmx and XeTeX read is taken:
dvipdfmx/ (dvipdfmx.cfg), fonts/cmap/ and fonts/sfd/ (CJK), and TECkit's
compiled mappings (fonts/misc/xetex/fontmapping/, .tec).

Search (release.json's `search`): for each engine (pdftex as pdflatex,
xetex as xelatex, luatex as lualatex) and each kpathsea format (tex, tfm,
vf, type1, enc, map, opentype, truetype, bst, bib, ist, cmap, sfd, misc;
lua for luatex),
the ordered path prefixes, read from TeX Live's own texmf.cnf (the kpathsea
package's web2c/texmf.cnf: TEXINPUTS.pdflatex, TEXINPUTS.xelatex,
TFMFONTS, ...): its $TEXMF/... entries brace-expanded, with $progname and
$engine put in. A name resolves to the path whose basename is the name and
whose first matching prefix comes earliest in the list; between paths
under the same prefix, the lexicographically smallest path wins (kpathsea
itself takes ls-R's order there; this is deterministic). The empty
alternative of tex/{latex,generic,} is the prefix `tex`: pdflatex finds
tex/xelatex's files too, last, as kpathsea does.

Packs: by TeX Live package, as its files are used together (a .sty with its
.def, .cfg and .fd files). Engine-specific trees go apart, so a pdfTeX
document never downloads them: tex/xelatex and tex/xetex in `<pkg>.x`,
tex/lualatex and tex/luatex in `<pkg>.l` (each cut at SPLIT gzipped:
`.x.2`, ...), OpenType and TrueType fonts in `<pkg>.o-<family>` (by family,
the file's name up to its last '-', neighbours merged up to FONT_BIN
gzipped, a bigger file alone). The rest of a package bigger than SPLIT
gzipped is cut by use: its hot packs hold the files some package's load
read (the pdfTeX trace's counts, most read first, cut at BIN), the rest go
in cold packs by directory and font family. No pack over 20 MB (Cloudflare
takes 25 MiB a file).

Dependencies, a column per engine (meta.json's `deps`): for each .sty and
.cls, the packs loading it reads on PhiTeX, less what a bare article reads
and its own pack, so a client fetching it fetches those too, in one round
instead of one build per file. The columns are hints: a client that
ignores them, or meets a wrong one, still asks for each file the build
needs; they only save rounds.
- TRACE=path/to/shelf-trace (trace/: phitex_tex::Build at the `phitex`
  submodule's commit, a job per name, every file its finders gave
  printed), run for pdftex (pdflatex.ini's format) and xetex (xelatex.ini's,
  with TRACE_FONT_INDEX, phitex-otf's font index of Shelf's
  OpenType/TrueType fonts (FONT_TREE below), for fonts by name), each over
  a flat directory of what that engine finds by each name (the search
  above). A TeX Live on PATH gives the formats and TECkit's mappings.
- TRACE_ENGINES (default `pdftex,xetex`): the columns traced with TRACE;
  TRACE_JOBS: batches at once (default 12); TRACE_NAMES=a.sty,b.cls: only
  those names (a check, kept apart in cache/trace-<engine>-check.json).
A trace is kept in cache/trace-<engine>.json with the tracer's version
(`shelf-trace --version`: the engine's commit, meta.json's `traced_by`),
by its inputs (the names and the flat directory): the same inputs are
traced once, and a build without TRACE takes a kept trace whose inputs
match (else the column is empty).

TLPDB=path: the package database to build from (a `texlive.tlpdb.xz`)
instead of tlnet's current one; the one used is kept as
cache/texlive.tlpdb.xz, so a build can be repeated (on another machine:
that file and cache/'s archives). FETCH_ONLY=1: fetch the database and
the archives, then stop (a networked step before an offline build).
FONT_TREE=dir: the OpenType/TrueType fonts also written there at their
texmf paths (dir/fonts/opentype/...), for the tracer's font index (made by
phitex-otf's otf-index with a fonts.conf naming those two directories:
scripts/accl-trace.sbatch).

The engine's font index (FONT_INDEX=path: the PhiTeX engine's builder over
fonts/opentype and fonts/truetype) is served, when given, at the path
FONT_INDEX_PATH, in a pack of its own.

PACK_JOBS: packages packed at once (default: every core); the packs are
the same whatever it is. TL_MIRROR overrides the mirror (default:
mirror.ctan.org's redirector).
"""
import gzip, hashlib, json, lzma, os, re, shutil, struct, subprocess, sys, tarfile, time, urllib.request
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

LABEL = sys.argv[1] if len(sys.argv) > 1 else "tl2026"
MIRROR = os.environ.get("TL_MIRROR", "https://mirror.ctan.org/systems/texlive/tlnet").rstrip("/")
TREES = [
    "tex/latex", "tex/generic", "tex/plain", "tex/xelatex", "tex/xetex", "tex/lualatex", "tex/luatex",
    "fonts/tfm", "fonts/vf", "fonts/type1", "fonts/enc", "fonts/map", "fonts/opentype", "fonts/truetype",
    "bibtex/bst", "bibtex/csf", "bibtex/bib", "makeindex",
    # (what xdvipdfmx and XeTeX read beyond the fonts: dvipdfmx.cfg, the
    # CJK CMaps and subfont definitions, dvipdfmx's own maps, TECkit's
    # compiled mappings)
    "dvipdfmx", "fonts/cmap", "fonts/sfd", "fonts/misc/xetex/fontmapping",
]
OPEN = ("fonts/opentype/", "fonts/truetype/")
XETEX = ("tex/xelatex/", "tex/xetex/")
LUATEX = ("tex/lualatex/", "tex/luatex/")
SPLIT = 1 << 20  # gzipped bytes above which a package is cut
BIN = 512 << 10  # about how big (gzipped) each part of a cut package is
FONT_BIN = 4 << 20  # about how big (gzipped) a pack of OpenType/TrueType fonts is
CACHE = Path("cache")
PDFTEX_MAP = "fonts/map/pdftex/updmap/pdftex.map"
FONT_INDEX_PATH = "fonts/misc/partex/fontindex.pxfi"
# (engine: kpathsea's progname for its LaTeX, and $engine)
ENGINES = {"pdftex": ("pdflatex", "pdftex"), "xetex": ("xelatex", "xetex"), "luatex": ("lualatex", "luatex")}
FORMATS = {
    "tex": "TEXINPUTS", "tfm": "TFMFONTS", "vf": "VFFONTS", "type1": "T1FONTS", "enc": "ENCFONTS",
    "map": "TEXFONTMAPS", "opentype": "OPENTYPEFONTS", "truetype": "TTFONTS",
    "bst": "BSTINPUTS", "bib": "BIBINPUTS", "ist": "INDEXSTYLE",
    "cmap": "CMAPFONTS", "sfd": "SFDFONTS", "misc": "MISCFONTS",
}
DEPS = ["pdftex", "xetex"]  # (the index's dependency columns, in order)


def get(url: str) -> bytes:
    with urllib.request.urlopen(url, timeout=300) as r:
        return r.read()


def wanted(path: str) -> bool:
    if not any(path.startswith(t + "/") for t in TREES):
        return False
    if path.startswith("fonts/map/"):
        return path.endswith(".map") and path.split("/")[2] in ("dvips", "pdftex", "dvipdfmx")
    if path.startswith(("dvipdfmx/", "fonts/cmap/")):
        return True
    if path.startswith("fonts/sfd/"):
        return path.endswith(".sfd")
    if path.startswith("fonts/misc/xetex/fontmapping/"):
        return path.endswith(".tec")
    if path.startswith(OPEN):
        return path.lower().endswith((".otf", ".ttf", ".ttc"))
    if path.startswith("fonts/"):
        return path.endswith((".tfm", ".vf", ".pfb", ".enc"))
    if path.startswith("bibtex/"):
        return path.endswith((".bst", ".csf", ".bib"))
    if path.startswith("makeindex/"):
        return path.endswith(".ist")
    # (under tex/ every file is an input: a package reads it by name, an
    # image as well as a macro file: mwe's example-image.png/.jpg/.pdf, a
    # logo, LineBreak.txt; a package's documentation is under doc/, which
    # is not in TREES)
    return True


def packages(db: str) -> tuple[list[dict], dict[str, dict]]:
    """Each package with files we take (name, revision, checksum, files, maps), and every package by name."""
    out, every, p = [], {}, None
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
            every[p["name"]] = p
            if p["files"] and "." not in p["name"]:  # (name.arch: binaries)
                out.append(p)
            p = None
    return out, every


def fetch(p: dict, want: set[str] | None = None) -> dict[str, bytes]:
    """The package's files (or `want`), by path, from its (cached, checked) archive."""
    f = CACHE / f"{p['name']}.r{p['rev']}.tar.xz"
    if not f.exists():
        data = get(f"{MIRROR}/archive/{p['name']}.tar.xz")
        if p["sum"] and hashlib.sha512(data).hexdigest() != p["sum"]:
            raise SystemExit(f"{p['name']}: checksum mismatch")
        tmp = f.with_suffix(".part")
        tmp.write_bytes(data)
        tmp.replace(f)
    want, got = want or set(p["files"]), {}
    with tarfile.open(f, "r:xz") as t:
        for m in t:
            name = m.name.removeprefix("texmf-dist/")
            if m.isfile() and name in want:
                got[name] = t.extractfile(m).read()
    return got


def expand(s: str) -> list[str]:
    """Brace expansion, as kpathsea's: a{b,c}d -> abd, acd (nested too)."""
    i = s.find("{")
    if i < 0:
        return [s]
    depth, j, parts, start = 0, i, [], i + 1
    while j < len(s):
        if s[j] == "{":
            depth += 1
        elif s[j] == "}":
            depth -= 1
            if depth == 0:
                break
        elif s[j] == "," and depth == 1:
            parts.append(s[start:j])
            start = j + 1
        j += 1
    parts.append(s[start:j])
    return [x for part in parts for x in expand(s[:i] + part + s[j + 1 :])]


def search(cnf: str) -> dict[str, dict[str, list[str]]]:
    """Each engine's path prefixes per format, from texmf.cnf (see the module doc)."""
    var: dict[str, str] = {}
    for line in re.sub(r"\\\n", "", cnf).splitlines():
        line = line.split("%", 1)[0].strip()
        m = re.match(r"([A-Za-z0-9_]+(?:\.[A-Za-z0-9_-]+)?)\s*=\s*(.*)", line)
        if m and m.group(1) not in var:  # (kpathsea: the first definition wins)
            var[m.group(1)] = m.group(2).strip()
    out: dict[str, dict[str, list[str]]] = {}
    for engine, (prog, eng) in ENGINES.items():
        formats = dict(FORMATS, lua="LUAINPUTS") if engine == "luatex" else FORMATS
        out[engine] = {}
        for fmt, v in formats.items():
            value = var.get(f"{v}.{prog}", var.get(v))
            if value is None:
                raise SystemExit(f"texmf.cnf: no {v}")
            value = value.replace("$progname", prog).replace("$engine", eng)
            prefixes: list[str] = []
            for el in value.split(";"):
                for x in expand(el):
                    if not x.startswith("$TEXMF/"):  # (., $OSFONTDIR, $VARTEXFONTS: not TeX Live's tree)
                        continue
                    # (a // inside, scripts//lua: any directory between; kept as scripts/lua)
                    x = re.sub(r"/+", "/", x.removeprefix("$TEXMF/")).rstrip("/")
                    if x and x not in prefixes:
                        prefixes.append(x)
            out[engine][fmt] = prefixes
    return out


def kind(name: str) -> str:
    """The kpathsea format a file is looked up as, by its extension."""
    base = name.rsplit("/", 1)[-1]
    ext = base.rsplit(".", 1)[-1].lower() if "." in base else ""
    return {"tfm": "tfm", "vf": "vf", "pfb": "type1", "enc": "enc", "map": "map", "otf": "opentype",
            "ttf": "truetype", "ttc": "truetype", "bst": "bst", "csf": "bst", "bib": "bib", "ist": "ist",
            "sfd": "sfd", "tec": "misc"}.get(ext, "tex")


def resolver(paths, order: dict[str, list[str]]):
    """(name -> the path the engine finds for it, by the module doc's rule; every name's paths)."""
    by_name: dict[str, list[str]] = defaultdict(list)
    for p in paths:
        by_name[p.rsplit("/", 1)[1]].append(p)

    def resolve(name: str) -> str | None:
        prefixes = order.get(kind(name), [])
        found = []
        for p in by_name.get(name, []):
            r = next((i for i, x in enumerate(prefixes) if p.startswith(x + "/")), None)
            if r is not None:
                found.append((r, p))
        return min(found)[1] if found else None

    return resolve, by_name


def pack(files: list[tuple[str, bytes]]) -> bytes:
    b = bytearray(struct.pack("<I", len(files)))
    for n, data in files:
        nb = n.encode()
        b += struct.pack("<I", len(nb)) + nb + struct.pack("<I", len(data)) + data
    return gzip.compress(bytes(b), 9, mtime=0)


def family(path: str) -> str:
    """A font's family: its name up to the last '-' (LinLibertineT-tlf-t1.tfm: LinLibertineT-tlf), else its first five letters."""
    stem = path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
    return stem.rsplit("-", 1)[0] if "-" in stem else re.sub(r"[0-9]+$", "", stem)[:5] or stem


def groups(pkg: str, files: list[tuple[str, bytes]], freq: dict[str, int]) -> dict[str, list[tuple[str, bytes]]]:
    """The packs of one package, (path, bytes) each: the engine-specific
    trees and the OpenType/TrueType fonts apart, the rest by use."""
    parts = [
        opentype(pkg, [f for f in files if f[0].startswith(OPEN)]),
        apart(f"{pkg}.x", [f for f in files if f[0].startswith(XETEX)]),
        apart(f"{pkg}.l", [f for f in files if f[0].startswith(LUATEX)]),
        common(pkg, [f for f in files if not f[0].startswith(OPEN + XETEX + LUATEX)], freq),
    ]
    out: dict[str, list] = {}
    for part in parts:
        clash = out.keys() & part.keys()
        if clash:
            raise SystemExit(f"{pkg}: two packs named {sorted(clash)}")
        out.update(part)
    return out


def apart(name: str, files: list[tuple[str, bytes]]) -> dict[str, list[tuple[str, bytes]]]:
    """Files in packs `name`, `name.2`, ..., each cut at SPLIT gzipped."""
    out: dict[str, list] = {}
    cur, size, k = [], 0, 1
    for p, b in files:
        z = len(gzip.compress(b, 1))
        if cur and size + z > SPLIT:
            out[name if k == 1 else f"{name}.{k}"] = cur
            cur, size, k = [], 0, k + 1
        cur.append((p, b))
        size += z
    if cur:
        out[name if k == 1 else f"{name}.{k}"] = cur
    return out


def opentype(pkg: str, files: list[tuple[str, bytes]]) -> dict[str, list[tuple[str, bytes]]]:
    """One package's OpenType and TrueType fonts in packs `<pkg>.o-<family>`:
    by family, neighbours merged up to FONT_BIN gzipped, a bigger file alone."""
    fam: dict[str, list] = defaultdict(list)
    for p, b in files:
        fam[family(p)].append((p, b))
    out: dict[str, list] = {}
    cur, size, name = [], 0, None
    for key in sorted(fam):
        for p, b in fam[key]:
            z = len(gzip.compress(b, 1))
            if cur and size + z > FONT_BIN:
                out[name] = cur
                cur, size = [], 0
            if not cur:
                name, k = f"{pkg}.o-{key}", 2
                while name in out:
                    name, k = f"{pkg}.o-{key}.{k}", k + 1
            cur.append((p, b))
            size += z
    if cur:
        out[name] = cur
    return out


def common(pkg: str, files: list[tuple[str, bytes]], freq: dict[str, int]) -> dict[str, list[tuple[str, bytes]]]:
    """A package's files every engine reads. Bigger than SPLIT gzipped, cut by
    use: its hot packs (named as the package, then .hot1, ...) hold the files
    some package's load read (the trace's counts, most read first, cut at
    BIN), the rest go in cold packs by directory and font family."""
    if not files:
        return {}
    if len(gzip.compress(b"".join(b for _, b in files), 1)) <= SPLIT:
        return {pkg: files}
    hot = sorted((f for f in files if freq.get(f[0])), key=lambda f: (-freq[f[0]], f[0]))
    out: dict[str, list] = {}
    cur, size, k = [], 0, 0
    for p, b in hot:
        z = len(gzip.compress(b, 1))
        if cur and size + z > BIN:
            out[pkg if k == 0 else f"{pkg}.hot{k}"] = cur
            cur, size, k = [], 0, k + 1
        cur.append((p, b))
        size += z
    if cur:
        out[pkg if k == 0 else f"{pkg}.hot{k}"] = cur
    keyed: dict[str, list] = defaultdict(list)
    for p, b in files:
        if freq.get(p):
            continue
        if p.startswith("fonts/"):
            key = "f-" + family(p)
        else:
            d = p.split("/")[2:-1]  # (under tex/latex/<pkg>/...)
            key = "-".join(d[1:]) if len(d) > 1 else ""
        keyed[key].append((p, b))
    # (neighbouring keys merged up to about BIN gzipped: a pack per family
    # would make tens of thousands of packs; the first key names the pack;
    # a key bigger than BIN itself goes on in packs of its own, name.2, ...)
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
        for p, b in keyed[key]:
            zb = len(gzip.compress(b, 1))
            if cur and size + zb > BIN:
                out[name] = cur
                part += 1
                cur, size, name = [], 0, f"{pkg}.{key}.{part}"
            cur.append((p, b))
            size += zb
    if cur:
        out[name] = cur
    return out


def trace(tool: str | None, flat_files: dict[str, bytes], names: list[str], what: str, check: bool = False) -> tuple[dict[str, set[str]], str | None]:
    """Each name (and article.cls, the baseline): the names loading it reads
    (a font found by path: its texmf path), on `tool` (shelf-trace) as
    `what`, from a flat directory of `flat_files`; and the tracer's
    version."""
    # (kept in cache/trace-<engine>.json, by its inputs: the names and the
    # flat directory's files; the same inputs trace once, so a rebuild for
    # another column leaves this one as it was, and a build without TRACE
    # takes it)
    h = hashlib.sha256()
    h.update(hashlib.sha256("\0".join(names).encode()).digest())
    for name in sorted(flat_files):
        h.update(name.encode() + b"\0" + hashlib.sha256(flat_files[name]).digest())
    key = h.hexdigest()
    saved = CACHE / f"trace-{what}{'-check' if check else ''}.json"
    version = subprocess.run([tool, "--version"], capture_output=True, text=True, check=True).stdout.strip() if tool else None
    if saved.exists():
        s = json.loads(saved.read_text())
        if s.get("key") == key and s.get("traced_by") and (version is None or s["traced_by"] == version):
            print(f"  {what}: trace from {saved}, by {s['traced_by']}", file=sys.stderr)
            return {n: set(fs) for n, fs in s["read"].items()}, s["traced_by"]
    if not tool:
        print(f"no {what} trace (TRACE, or a kept one for these inputs): its dependency column is empty", file=sys.stderr)
        return {}, None
    flat = CACHE / f"flat-{what}"
    if flat.exists():
        shutil.rmtree(flat)
    flat.mkdir()
    for name, b in flat_files.items():
        (flat / name).write_bytes(b)
    # (the format's state: made by the first batch, alone, read by the rest)
    prelude = CACHE / f"trace-{what}.prelude"
    prelude.unlink(missing_ok=True)
    args = [tool, "--engine", what, "--flat", str(flat), "--prelude", str(prelude)]
    if what == "xetex":
        if not os.environ.get("TRACE_FONT_INDEX"):
            raise SystemExit("xetex trace: TRACE_FONT_INDEX (phitex-otf's otf-index) is needed")
        args += ["--font-index", os.environ["TRACE_FONT_INDEX"]]
    batches = [names[i : i + 25] for i in range(0, len(names), 25)]
    log = open(CACHE / f"trace-{what}.log", "w")

    def run(batch: list[str]) -> list[str]:
        try:
            r = subprocess.run([*args, *batch], capture_output=True, text=True, timeout=600)
            log.write(r.stderr)
            if r.returncode:
                log.write(f"batch {batch[0]}..: exit {r.returncode}\n")
            return r.stdout.splitlines()
        except subprocess.TimeoutExpired as e:  # (a job that never ends: its batch's done lines only)
            out = e.stdout.decode(errors="replace") if isinstance(e.stdout, bytes) else e.stdout or ""
            log.write(f"batch {batch[0]}..: timed out\n")
            return out.splitlines()

    def done(lines: list[str]) -> None:
        for line in lines:
            name, _, fs = line.partition("\t")
            read[name] = set(filter(None, fs.split(",")))

    read: dict[str, set[str]] = {}
    done(run(["article.cls"]))
    with ThreadPoolExecutor(int(os.environ.get("TRACE_JOBS", "12"))) as ex:
        for i, more in enumerate(ex.map(run, batches), 1):
            done(more)
            if i % 40 == 0:
                print(f"  {what}: traced {i}/{len(batches)} batches", file=sys.stderr)
    # (the names of a batch that timed out or was killed, its memory cap
    # reached, once more, a process each, two at once)
    again = [n for n in names if n not in read]
    if again:
        print(f"  {what}: {len(again)} names again, one a process", file=sys.stderr)
        with ThreadPoolExecutor(2) as ex:
            for more in ex.map(run, [[n] for n in again]):
                done(more)
    log.close()
    missing = sorted(set(names) - read.keys())
    if missing:
        print(f"  {what}: {len(missing)} names not traced (timed out or killed): {' '.join(missing)}", file=sys.stderr)
    saved.write_text(json.dumps({"key": key, "traced_by": version, "read": {n: sorted(fs) for n, fs in sorted(read.items())}}))
    return read, version


def safe(s: str) -> str:
    return re.sub(r"[^A-Za-z0-9._-]", "_", s)


def main() -> None:
    t0 = time.time()
    CACHE.mkdir(exist_ok=True)
    # (the database used kept beside the archives: TLPDB=it repeats the build)
    raw = Path(os.environ["TLPDB"]).read_bytes() if os.environ.get("TLPDB") else get(f"{MIRROR}/tlpkg/texlive.tlpdb.xz")
    if not (CACHE / "texlive.tlpdb.xz").exists() or (CACHE / "texlive.tlpdb.xz").read_bytes() != raw:
        (CACHE / "texlive.tlpdb.xz").write_bytes(raw)
    db = lzma.decompress(raw).decode()
    pkgs, every = packages(db)
    print(f"{len(pkgs)} packages", file=sys.stderr)
    files: dict[str, tuple[dict, bytes]] = {}
    with ThreadPoolExecutor(8) as ex:
        for i, (p, got) in enumerate(zip(pkgs, ex.map(fetch, pkgs)), 1):
            for path, b in got.items():
                files[path] = (p, b)
            if i % 500 == 0:
                print(f"  {i}/{len(pkgs)}", file=sys.stderr)
    print(f"fetched in {time.time() - t0:.0f} s, database sha256 {hashlib.sha256(raw).hexdigest()[:16]}", file=sys.stderr)
    if os.environ.get("FONT_TREE"):  # (the fonts the tracer's font index is made of)
        for path, (_, b) in files.items():
            if path.startswith(OPEN):
                (Path(os.environ["FONT_TREE"]) / path).parent.mkdir(parents=True, exist_ok=True)
                (Path(os.environ["FONT_TREE"]) / path).write_bytes(b)
    if os.environ.get("FETCH_ONLY"):
        fetch(every["kpathsea"], {"web2c/texmf.cnf"})
        return

    # the search orders, from TeX Live's own texmf.cnf
    cnf = fetch(every["kpathsea"], {"web2c/texmf.cnf"})["web2c/texmf.cnf"].decode()
    order = search(cnf)
    for engine in order:
        for fmt, prefixes in order[engine].items():
            print(f"search {engine} {fmt}: {prefixes}", file=sys.stderr)

    # pdftex.map, as updmap makes it: every package's Map and MixedMap
    # files, in package order (a font named twice: the first wins)
    merged = ("fonts/map/dvips/", "fonts/map/pdftex/")
    maps = {path.rsplit("/", 1)[1]: b for path, (_, b) in files.items() if path.startswith(merged)}
    seen, lines = set(), []
    # (updmap's default, pdftexDownloadBase14: the base 14 fonts embedded,
    # URW's, from pdftex_dl14.map, ahead of everything)
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
    # (dvipdfmx's own maps, fonts/map/dvipdfmx/, stay: its dvipdfmx.cfg names them)
    for path in [q for q in files if q.startswith(merged)]:
        del files[path]
    files[PDFTEX_MAP] = ({"name": "pdftex-map", "rev": "updmap"}, pdftex_map)
    if os.environ.get("FONT_INDEX"):
        files[FONT_INDEX_PATH] = ({"name": "partex-fonts", "rev": "build"}, Path(os.environ["FONT_INDEX"]).read_bytes())

    # what each engine finds by each name, and where tex/xelatex's and
    # tex/xetex's names are in another tex tree too
    res = {e: resolver(files, order[e]) for e in ("pdftex", "xetex")}
    by_name = res["pdftex"][1]
    clashes = []
    for name, paths in sorted(by_name.items()):
        xe = [p for p in paths if p.startswith(XETEX)]
        other = [p for p in paths if p.startswith("tex/") and not p.startswith(XETEX)]
        if xe and other:
            clashes.append((name, res["pdftex"][0](name), res["xetex"][0](name), sorted(paths)))
    xe_names = {p.rsplit("/", 1)[1] for p in files if p.startswith(XETEX)}
    print(f"tex/xelatex, tex/xetex: {len(xe_names)} names, {len(clashes)} also in another tex tree:", file=sys.stderr)
    for name, pd, xe, paths in clashes:
        print(f"  {name}: pdftex finds {pd}, xetex {xe} (of {', '.join(paths)})", file=sys.stderr)

    # the dependency columns: each engine's trace over what it finds by name
    # (a name the tracer prints with a '/' is a texmf path: a font found by path)
    flats, reads, traced_by = {}, {}, {}
    only = [n for n in os.environ.get("TRACE_NAMES", "").split(",") if n]
    for engine, own in (("pdftex", XETEX + LUATEX), ("xetex", LUATEX)):
        resolve, names = res[engine]
        flats[engine] = flat = {n: p for n in names if (p := resolve(n))}
        # (another engine's own packages: this one can't load them)
        traced = sorted(n for n, p in flat.items() if n.endswith((".sty", ".cls")) and not p.startswith(own))
        if only:
            traced = [n for n in traced if n in only]
        tool = os.environ.get("TRACE") if engine in os.environ.get("TRACE_ENGINES", "pdftex,xetex").split(",") else None
        t1 = time.time()
        r, traced_by[engine] = trace(tool, {n: files[p][1] for n, p in flat.items()}, traced, engine, bool(only))
        print(f"{engine} trace: {len(r)} of {len(traced)} names in {time.time() - t1:.0f} s", file=sys.stderr)
        reads[engine] = {flat[n]: {q for f in fs if (q := f if "/" in f else flat.get(f)) in files} for n, fs in r.items() if n in flat}
    if only:
        return
    freq: dict[str, int] = defaultdict(int)
    for fs in reads["pdftex"].values():
        for f in fs:
            freq[f] += 1

    by_pkg: dict[str, list] = defaultdict(list)
    for path, (p, b) in files.items():
        by_pkg[p["name"]].append((path, b))
    out = Path("build") / LABEL
    if out.exists():
        shutil.rmtree(out)
    (out / "packs").mkdir(parents=True)
    # (each package packed apart, PACK_JOBS at once (default: every core;
    # zlib lets go of the GIL): a worker writes its packs and returns only
    # their names, sizes and paths, merged in package order, so the build
    # is the same whatever the count)
    def pack_one(item):
        pkg, fs = item
        fs.sort()
        made = []
        for key, members in groups(pkg, fs, freq).items():
            data = pack(sorted(members))
            (out / "packs" / f"{safe(key)}.pack").write_bytes(data)
            made.append((safe(key), len(data), [path for path, _ in members]))
        rows = [f"{path}\t{pkg}\tr{files[path][0]['rev']}\t{hashlib.sha256(b).hexdigest()}" for path, b in fs]
        return made, rows

    t1 = time.time()
    pack_of, rows, sizes = {}, [], []
    with ThreadPoolExecutor(int(os.environ.get("PACK_JOBS", "0")) or os.cpu_count()) as ex:
        for made, prows in ex.map(pack_one, sorted(by_pkg.items())):
            for key, size, paths in made:
                sizes.append((size, key))
                for path in paths:
                    pack_of[path] = key
            rows += prows
    print(f"packed in {time.time() - t1:.0f} s", file=sys.stderr)
    deps: dict[str, dict[str, list[str]]] = {}
    for engine in DEPS:
        r = reads[engine]
        base = {pack_of[f] for f in r.get(flats[engine].get("article.cls", ""), set()) if f in pack_of}
        deps[engine] = {path: sorted({pack_of[f] for f in fs if f in pack_of} - base - {pack_of.get(path)}) for path, fs in r.items()}
    index = "".join(
        f"{path}\t{pack_of[path]}\t" + "\t".join(",".join(deps[e].get(path, [])) for e in DEPS) + "\n" for path in sorted(pack_of)
    )
    (out / "index.tsv").write_text(index)
    (out / "meta.json").write_text(json.dumps({"deps": DEPS, "traced_by": traced_by, "search": order}, indent=1) + "\n")
    Path("MANIFEST.tsv").write_text(f"# {LABEL} from {MIRROR}: path, package, revision, sha256\n" + "\n".join(sorted(rows)) + "\n")

    for engine in DEPS:
        print(f"{sum(1 for v in deps[engine].values() if v)} paths with {engine} dependency packs", file=sys.stderr)
    sizes.sort(reverse=True)
    total = sum(s for s, _ in sizes)

    def part(test, what):
        ks = [(s, k) for s, k in sizes if test(k)]
        print(f"  {what}: {len(ks)} packs, {sum(s for s, _ in ks) / 1e6:.1f} MB", file=sys.stderr)

    part(lambda k: re.search(r"\.x(\.\d+)?$", k), "tex/xelatex, tex/xetex (.x)")
    part(lambda k: re.search(r"\.l(\.\d+)?$", k), "tex/lualatex, tex/luatex (.l)")
    part(lambda k: ".o-" in k, "OpenType/TrueType (.o-)")
    print(f"{LABEL}: {len(pack_of)} paths in {len(sizes)} packs, {total / 1e6:.1f} MB, in {time.time() - t0:.0f} s; largest: "
          + ", ".join(f"{k} {s / 1e6:.1f} MB" for s, k in sizes[:5]), file=sys.stderr)
    if len(sizes) > 19_000:
        raise SystemExit(f"{len(sizes)} packs: over Cloudflare Pages' 20,000 files")
    big = [k for s, k in sizes if s > 20 * 10**6]
    if big:
        raise SystemExit(f"packs over 20 MB (Cloudflare takes 25 MiB a file): {big}")


if __name__ == "__main__":
    main()
