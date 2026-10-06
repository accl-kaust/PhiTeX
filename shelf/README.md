# Shelf — shelf.PhiTeX.org

TeX Live's files for the engines PhiTeX runs (pdfTeX, XeTeX; LuaTeX's
inputs carried too), as a static site, so PhiTeX running in a browser (the
Overleaf extension) can load what a document uses without a TeX installation
or a server process.

    GET /tl2026/release.json              the current release (schema 3): its index, the
                                          engines' search orders, the dependency columns
    GET /tl2026/index-<release>.tsv.gz    path <TAB> pack <TAB> deps_pdftex <TAB> deps_xetex
    GET /tl2026/h/<pack>-<hash>.pack      packs, named by content, never changed

- **By texmf path**: every file is keyed by its path in TeX Live
  (`tex/latex/fontspec/fontspec.sty`); a client resolves a name with the
  engine's search order from `release.json`, read from TeX Live's own
  `texmf.cnf` (`build.py`'s header has the rule).
- **Packs**: by TeX Live package; XeTeX's and LuaTeX's own trees and the
  OpenType/TrueType fonts in packs of their own, so a pdfTeX document
  never downloads them.
- **Immutable**: a pack's name is its content's hash; a new TeX Live is a
  new prefix (`tl2027/`). `release.py` keeps the newest 3 releases' packs
  and those `keep-packs.txt` lists (the ones the extension bundles).
- **CORS open**: any origin may read, so a browser extension needs no host
  permission.
- **Static only**: no logs, accounts, or per-user state of its own.

## Build and deploy

    scripts/sandbox --net env TRACE=cache/tools/trace-deps TRACE_FMT=cache/tools/pdflatex.fmt \
        python3 build.py [tl2026]                              # → build/tl2026/, MANIFEST.tsv
    scripts/sandbox python3 release.py tl2026 --releases releases-s3 --min-extension 0.3.0
    scripts/sandbox --net python3 serve.py --root releases-s3 # local, on :8124
    scripts/sandbox --net npx wrangler login                  # once (prints a URL to open)
    RELEASES=releases scripts/sandbox --net ./deploy.sh       # Cloudflare Pages

Heavy steps (build.py, release.py) run inside a memory cap:
`systemd-run --user --scope -q --slice=partex.slice -p MemoryMax=16G -p MemorySwapMax=0 …`.
`scripts/sandbox` passes no environment through: give variables with `env`.

`scripts/sandbox` (bubblewrap) sees only this repository: its `$HOME` is
`.home/` (git-ignored), where npm's cache and wrangler's login token stay.

`build.py` needs no TeX installation: it reads TeX Live's package database
from tlnet, downloads every package with files in the trees `build.py` takes
(about 4,200 archives, each checked against the database's SHA-512,
kept in `cache/` between runs) and keeps only those files. That is all of
TeX Live, not one machine's selection of collections.

At most 19,500 files (Cloudflare Pages allows 20k files, 25 MiB each).
`build/`, `releases*/` and `public/tl*/` are generated, not committed;
`MANIFEST.tsv` is, so a release is reproducible and checkable.

## License

The files served are TeX Live's, unmodified, each under its own license
(mostly the LaTeX Project Public License), as stated in each file. This
repository's own scripts: AGPL-3.0-only, © Ammar Seliaman.
