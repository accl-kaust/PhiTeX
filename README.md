<p align="center">
  <img src="assets/banner.png" alt="PhiTeX: incremental TeX, exact output, Rust." width="100%">
</p>

<p align="center">
  <a href="LICENSE"><img alt="License: AGPL-3.0-only" src="https://img.shields.io/badge/license-AGPL--3.0--only-blue.svg"></a>
  <img alt="Rust nightly" src="https://img.shields.io/badge/rust-nightly--2026--08--08-orange.svg?logo=rust">
  <img alt="Edition 2024" src="https://img.shields.io/badge/edition-2024-orange.svg">
  <img alt="Output: byte-identical to pdfTeX 1.40.29" src="https://img.shields.io/badge/output-byte--identical%20to%20pdfTeX%201.40.29-success.svg">
  <img alt="Targets: native and wasm32" src="https://img.shields.io/badge/targets-native%20%7C%20wasm32-informational.svg?logo=webassembly">
  <img alt="unsafe: denied" src="https://img.shields.io/badge/unsafe-denied-critical.svg">
</p>

# PhiTeX

A TeX engine in Rust that compiles a document as a dataflow graph over
content-versioned values (`phitex-flow`): every command is a node, a
macro call a step, a group's close emits nothing, and N equal threads
evaluate the graph. A cold build is an edit from nothing; an edit re-runs
only the nodes whose reads changed, and its output is the cold build's,
byte for byte.

## Status

- **Exactness:** plain TeX, e-TeX, LaTeX (`latex`) and pdfLaTeX
  (`pdflatex`) are byte-identical with TeX Live on the tested documents;
  hyperref, images and colour in PDF mode come next, then XeTeX. The
  `.log` may differ.
- **Edits:** a word edit on a 48-page plain document is about 10 ms
  (`phitex-flow --budgets`).
- **The cross-run fixpoint:** `phitex build` runs the document again with
  the files it wrote (`.aux`, `.toc`, `.bbl`, `.ind`) until they settle,
  BibTeX and makeindex in process between runs.

## Layout

| Crate | What it is |
|---|---|
| `phitex-flow` | the engine: TeX as a suspending dataflow graph (`AGENTS_BRIEF.md`, `crates/phitex-flow/CORE.md`) |
| `phitex-cli` | the `phitex` command |
| `phi` | the φ core |
| `phitex-kpse` | kpathsea file lookup |
| `phitex-bibtex`, `phitex-makeindex` | BibTeX and makeindex |
| `phitex-otf`, `phitex-xdvipdfmx` | OpenType shaping, xdvipdfmx |
| `phitex-syntax`, `phitex-doc`, `phitex-diff`, `phitex-git` | the source CST, the document's structure, latexdiff, git |
| `phitex-draw`, `phitex-codecs` | the viewer's pages, the codecs and PDF reader |

`AGENTS_BRIEF.md` is the design, `DESIGN.md` the crates and the φ core,
and `AGENTS.md` the working rules.

## Building

Run every `cargo` command and built binary through `scripts/sandbox`
(bubblewrap: the repo is writable, `$HOME` is empty, and there is no
network unless `--net`):

    scripts/sandbox --net cargo fetch
    scripts/sandbox cargo build --release
    scripts/sandbox cargo test

`scripts/fetch-upstream.sh` fetches the pinned reference sources (TeX Live's
web2c, LaTeX, pgf) into `upstream/`. A TeX Live installation provides the
fonts and packages.

## Use

`phitex` builds a document to its fixpoint (BibTeX and makeindex
included), or watches it and rebuilds as it is edited:

    phitex build paper.tex
    phitex build --output-format dvi paper.tex
    phitex watch -o out paper.tex
    phitex clean paper.tex

Under an engine's name (`pdflatex`, `latex`, `pdftex`, `etex`, `tex`), or
with `--compat=NAME`, it takes that engine's command line and runs once:

    phitex --compat=latex -interaction=batchmode doc.tex
    phitex --compat=pdftex -ini -etex doc.tex

The formats are made on the graph from TeX Live's ini files the first
time they are needed, and cached (`$XDG_CACHE_HOME/phitex`).

LuaLaTeX is not on the graph: `phitex build --engine lualatex` (or a `%
!TEX program = lualatex` comment) runs the system's `lualatex` as an
external process to the fixpoint (BibTeX, makeindex, biber between runs;
SyncTeX from lualatex), and `phitex watch` runs the whole build again on
each saved change. It is not incremental.

## License

GNU Affero General Public License v3.0 only (`AGPL-3.0-only`); see `LICENSE`.

This license applies to every revision of this repository, including
those committed before `LICENSE` was added. The `license` field in
earlier revisions of `Cargo.toml` (`MIT OR Apache-2.0`, later
`AGPL-3.0-or-later`) was a placeholder and never licensed this code under
those terms.
