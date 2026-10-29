# Shelf — shelf.PhiTeX.org

TeX Live's files for the engines PhiTeX runs (pdfTeX, XeTeX; LuaTeX's
inputs carried too), as a static site, so PhiTeX running in a browser (the
Overleaf extension, VS Code) or a native `phitex` that opts in can load
what a document uses without a TeX installation or a server process.

    GET /tl2026/release.json                the current release (schema 1): its index, pack
                                            table, the engines' search orders, the dependency
                                            columns and what traced them
    GET /tl2026/release-<release>.json      every release's release.json, kept
    GET /tl2026/index-<release>.tsv.gz      path <TAB> pack <TAB> deps_pdftex <TAB> deps_xetex
    GET /tl2026/packs-<release>.tsv.gz      pack <TAB> sha256 <TAB> bytes
    GET /tl2026/h/<pack>-<hash>.pack        packs, named by content, never changed

- **By texmf path**: every file is keyed by its path in TeX Live
  (`tex/latex/fontspec/fontspec.sty`); a client resolves a name with the
  engine's search order from `release.json`, read from TeX Live's own
  `texmf.cnf` (`build.py`'s header has the rule).
- **Packs**: by TeX Live package; XeTeX's and LuaTeX's own trees and the
  OpenType/TrueType fonts in packs of their own, so a pdfTeX document
  never downloads them.
- **Checked**: the pack table has every pack's full SHA-256 (of the
  `.pack` file as served, its gzip bytes) and size; a client checks each
  pack it fetches against it and refuses one that differs.
- **Dependency columns are hints**: per engine, the packs loading a .sty or
  .cls reads (traced on PhiTeX; `traced_by` names the engine's commit), so
  a client fetches them in one round. A client that ignores them, or a
  column traced by another engine version, costs rounds, never
  correctness: the build asks for every file it reads.
- **Immutable, reproducible**: a pack's name is its content's hash; every
  release's index, pack table and `release-<release>.json` stay for ever,
  so a project pinned to an older release can say which packs it builds
  from (whether those packs are still served: Retention, below). A new
  TeX Live is a new prefix (`tl2027/`).
- **CORS open**: any origin may read, so a browser extension needs no host
  permission.
- **Static only**: no logs, accounts, or per-user state of its own.

## Protocols and hosts

The public protocol is counted from 1 at PhiTeX's alpha, and the host
names it: schema 1 is the Pages project `shelf-phitex-v1` at
`v1.shelf.phitex.org`, the URL PhiTeX builds in. A schema older clients
can't read gets the next project and host (`shelf-phitex-v2`,
`v2.shelf.phitex.org`) beside it, never in its place. A separate project
rather than a prefix: a Pages deployment replaces its project's whole
site (a prefix would mean uploading the old files again with every new
deploy, and a mistake would take them away), and each project has its own
20,000-file budget. The development sites (`shelf-phitex` at
`shelf.phitex.org`, schema 3, and `shelf-phitex-v4`, schema 4) were never
released; `deploy.sh` deploys to `PROJECT` (default `shelf-phitex-v1`)
and refuses them. PhiTeX's docs/spec-distribution.md has the rules.

## Build and deploy

    # the traces and the build: on the accl cluster (below); or here, with a built tracer
    scripts/sandbox --net env TRACE=trace/target/release/shelf-trace TRACE_FONT_INDEX=fontindex.pxfi \
        python3 build.py [tl2026]                              # → build/tl2026/, MANIFEST.tsv
    scripts/sandbox python3 release.py tl2026 --min-extension 0.6.0   # → releases-v1/tl2026/
    scripts/sandbox --net python3 serve.py --root releases-v1 # local, on :8124
    scripts/sandbox --net npx wrangler login                  # once (prints a URL to open)
    scripts/sandbox --net ./deploy.sh                         # Pages project shelf-phitex-v1

Heavy steps (build.py, release.py) run inside a memory cap:
`systemd-run --user --scope -q --slice=partex.slice -p MemoryMax=16G -p MemorySwapMax=0 …`.
`scripts/sandbox` passes no environment through: give variables with `env`.

`scripts/sandbox` (bubblewrap) sees only this repository: its `$HOME` is
`.home/` (git-ignored), where npm's cache and wrangler's login token stay.

`build.py` needs no TeX installation: it reads TeX Live's package database
from tlnet (or `TLPDB=`, a saved one; the one used is kept in
`cache/texlive.tlpdb.xz`), downloads every package with files in the trees
`build.py` takes (about 4,500 archives, each checked against the
database's SHA-512, kept in `cache/` between runs) and keeps only those
files. That is all of TeX Live, not one machine's selection of
collections.

### The dependency traces (trace/)

`trace/` is `shelf-trace`, a small Rust program on PhiTeX's
`phitex_tex::Build`; the engine is the `trace/phitex` submodule (PhiTeX's
`dev`, at the commit the columns are traced with: `git submodule update
--init trace/phitex`). For each name it runs one job, as the old
`trace-deps` did (`\documentclass{article}\usepackage{X}` for a .sty,
`\documentclass{X}` for a .cls, a body that typesets the usual shapes),
on pdfTeX (pdflatex.ini's format) or XeTeX (xelatex.ini's), the format
made of Shelf's own files as a client makes it, and prints
every file the build's finders gave it: the document's finder over a flat
directory of what that engine finds by each name on Shelf, and XeTeX's
font files (a face found by name through phitex-otf's font index, made
over Shelf's own OpenType/TrueType fonts). Those are exactly the files a
client serving the build from Shelf is asked for, from the build's own
queries. (Not `phitex build -recorder`: PhiTeX's CLI runs no XeTeX yet,
and its `.fls` lists what its own session tracks, not fonts.) The
format's own reads are in no column (every document has them); XeTeX's
XDV is not made a PDF, so what xdvipdfmx reads is not in its column.

`build.py` runs it (TRACE) in batches of 25 names, TRACE_JOBS at once,
and keeps each column's trace in `cache/trace-<engine>.json` with its
`traced_by`, by its inputs; release.json's `traced_by` comes from there.

The trace is thousands of jobs: run it on the accl cluster, not on a
shared desktop. `scripts/accl-trace.sbatch` (its header has the commands)
does the whole build in PhiTeX's cluster image: the fetch (with the
network; the archives kept in `~/shared-cache/shelf/archives`), then
without the network the tracer, the font index and `build.py` with TRACE.
The results (`build/`, `MANIFEST.tsv`, the traces, the database) land in
`~/code/flinner/shelf-runs/<mode>-<job>/`, copied back here before
`release.py`.

At most 19,500 files a project (Cloudflare Pages allows 20k files,
25 MiB each). `build/`, `releases*/` and `public/tl*/` are generated, not
committed; `MANIFEST.tsv` is, so a release is reproducible and checkable.

## Retention

`release.py` keeps every release's index, pack table and
`release-<release>.json` for ever, and every pack a kept index names: a
pack goes only when no kept index names it. Packs are named by content,
so a release adds only the packs whose bytes changed. The files a deploy
holds (the site's own 4, every pack, 3 a release) are counted before a
release is written: past 15,000 a loud warning, past 19,500 no release
(Cloudflare Pages' free plan serves 20,000 files a deployment).

Measured (packs as served, gzip; MANIFEST.tsv and the releases built):

| release | packs | MB | new packs vs all earlier | MB |
|---|---|---|---|---|
| tl2026.3 (schema 3, a new layout) | 6,919 | 1,673 | 6,919 | 1,673 |
| tl2026.4 (xetex column) | 6,919 | 1,673 | 0 | 0 |
| tl2026.5 (columns only) | 6,919 | 1,673 | 0 | 0 |
| tl2026.6 (22 days of tlnet, new traces) | 6,969 | 1,684 | 450 | 179 |

tl2026.6's new packs: 218 (90 MB) from the 161 packages TeX Live
updated (118) or added (43) between 2026-10-06 and 2026-10-28, and 232
(89 MB) re-cut hot packs of big font packages (fira, cm-super, lm,
libertine, ...), whose split into hot and cold packs follows the pdfTeX
trace's counts: a trace by another engine version moves some files
between them. A release whose columns alone
change adds none (tl2026.4, tl2026.5).

Growth: TeX Live's updates come to about 300 new packs a month (about
3,600 and 1.5 GB a year); a trace by a new engine version about 230
more. From tl2026.6's 6,977 files, monthly releases of one TeX Live reach
15,000 files after about 26 months and 19,500 after about 41. But each
TeX Live is a new prefix with a whole new set (about 7,000 packs, 1.7 GB)
in the same project: with half a year of tl2026's updates, TeX Live 2027
(spring 2027) brings the deploy to about 15,800 files, past the warning,
and its own updates reach the limit about a year later, before TeX Live
2028 at the latest. So Pages' free plan holds tl2026 and tl2027, not more.

Past the limit, the packs move; indexes, pack tables and release.json
stay on Pages:
- **Cloudflare Pages' paid plan**: 100,000 files a deployment, the same
  layout and deploy: about ten TeX Live years.
- **R2 behind a custom domain** at the same URLs (`/tl2026/h/...`, a
  Worker route or the bucket's own domain), one object a pack, no file
  limit. Each TeX Live year is about 3.2 GB of packs (1.7 GB and 1.5 GB of
  updates): inside R2's free 10 GB-month for about three years, then
  $0.015 a GB-month (20 GB: $0.30 a month); reads free up to 10 million a
  month; no egress fees (Cloudflare's published prices as known when
  written: check them before deciding). The pack table's SHA-256 is what
  clients check, wherever a pack is served from.

Not volumes (a release's new packs appended to large files, a pack read
with an HTTP Range request): Cloudflare Pages ignores Range on static
assets (shelf-phitex.pages.dev, 2026-10-28: `curl -r 0-99` on a 1.5 KB
and an 11.7 MB pack, cache-busted too, answered 200 with the whole
file), so a client would download whole volumes.

## License

The files served are TeX Live's, unmodified, each under its own license
(mostly the LaTeX Project Public License), as stated in each file. This
repository's own scripts: AGPL-3.0-only, © Ammar Seliaman.
