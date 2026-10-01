# Shelf — shelf.PhiTeX.org

TeX Live's input files (`.sty`, `.cls`, `.def`, `.tex`, …) as a static
site, so PhiTeX running in a browser (the Overleaf extension) can load the
packages a document uses without a TeX installation or a server process.

    GET /tl2026/<name>        the file, as TeX finds it by name
    GET /tl2026/names.txt     every name served (clients ask only for these)

- **Flat by name**: TeX asks for `graphicx.sty`, not a path. Where two
  files share a name, `build.py` keeps kpathsea's LaTeX choice
  (`tex/latex`, then `tex/generic`, then `tex/plain`).
- **Versioned and immutable**: a new TeX Live is a new prefix (`tl2027/`);
  files under one never change, so they cache forever (`public/_headers`).
- **CORS open**: any origin may read, so a browser extension needs no host
  permission.
- **Static only**: no logs, accounts, or per-user state of its own.

## Build and deploy

    python3 build.py [tl2026]   # → public/tl2026/, MANIFEST.tsv
    ./deploy.sh                 # Cloudflare Pages

`build.py` needs no TeX installation: it reads TeX Live's package database
from tlnet, downloads every package with LaTeX/generic/plain input files
(about 4,000 archives, 1.1 GB, each checked against the database's SHA-512,
kept in `cache/` between runs) and keeps only those files. That is all of
TeX Live, not one machine's selection of collections.

About 19k files (Cloudflare Pages allows 20k files, 25 MiB each).
`public/tl*/` is generated, not committed; `MANIFEST.tsv` is, so a
release is reproducible and checkable.

## License

The files served are TeX Live's, unmodified, each under its own license
(mostly the LaTeX Project Public License), as stated in each file. This
repository's own scripts: AGPL-3.0-only, © Ammar Seliaman.
