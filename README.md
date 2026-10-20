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

A TeX engine in Rust that compiles a document by graph reduction of
pending thunks: macros are functions, the PDF is the root, and the files
a job writes and reads back are a loop-back reduced to a fixpoint. A
cold build is an edit from nothing; an edit reduces again only what its
change reaches, and its output is the cold build's, byte for byte.

## Alpha status

The alpha runs plain TeX and LaTeX documents with pdfTeX's semantics.

- **What works:** plain TeX and LaTeX (`pdftex`, `pdflatex`, and `tex`,
  `etex`, `latex` for DVI), with pdfTeX 1.40.29's semantics. The PDF is
  byte-identical to pdfTeX's (TeX Live 2026), object numbers included,
  at every thread count, and so are the files the job reads back; the
  `.log` is not compared. `phitex build` does latexmk's rounds in one
  build, in no more rounds than latexmk's pdflatex runs, with BibTeX,
  biber and makeindex in process. `phitex watch` rebuilds on every
  change with the pages live in the browser. The same engine builds for
  WebAssembly with threads (`scripts/wasm.sh`).
- **How to run it:** build the binary as in [Building](#building), then

      phitex build paper.tex          # to the fixpoint, like latexmk -pdf
      phitex watch paper.tex          # rebuild on change, pages in the browser
      phitex --compat=pdflatex paper.tex   # one run, as pdflatex

- **What is checked, and what is not met yet:** the release bars and
  their measured values are in
  [`docs/alpha-checklist.md`](docs/alpha-checklist.md). What does not
  work yet (XeLaTeX, LuaLaTeX, SyncTeX, font expansion, Type 3 fonts,
  speed, and more) is listed with its cause under "Known not to work" in
  [`CHANGELOG.md`](CHANGELOG.md).

## Layout

| Crate | What it is |
|---|---|
| `phitex-defuse` | the def-use graph evaluator (`defuse`, vendored under `crates/vendor/defuse`, never edited here) and what PhiTeX adds around it without knowing TeX |
| `phitex-tex` | TeX's semantics on the graph: tokens, expansion, macros, registers, groups, modes, pages, pdfTeX's primitives, the cross-run fixpoint, the tools' runs |
| `phitex-typeset` | Knuth's algorithms as pure functions: line breaking, hyphenation, math, alignments, the page builder |
| `phitex-source` | source text as units with identities; the lexer, paragraphs, syntax tree and the syntactic pass |
| `phitex-tfm`, `phitex-dvi`, `phitex-pdf` | font metrics, DVI, pdfTeX's PDF backend |
| `phitex-codecs` | zlib (a port of zlib 1.3's deflate, for pdfTeX's bytes), the image codecs, the PDF reader |
| `phitex-cli` | the `phitex` command |
| `phitex-wasm` | the engine as a WebAssembly program with threads |
| `phitex-kpse` | kpathsea's file lookup |
| `phitex-bibtex`, `phitex-biber`, `phitex-makeindex` | BibTeX, biber and makeindex |
| `phitex-pdftex`, `phitex-xetex`, `phitex-luatex`, `phitex-lua` | (stubs) the other engines, and Lua for LuaTeX |
| `phitex-otf`, `phitex-xdvipdfmx` | OpenType shaping, xdvipdfmx (for XeTeX, not used by the alpha) |
| `phitex-doc`, `phitex-diff`, `phitex-git` | the document's structure, latexdiff, git |

Each crate's docs say what it is. `AGENTS.md` holds the working rules.

## Building

Run every `cargo` command and built binary through `scripts/sandbox`.
This is bubblewrap: the repository is writable, `$HOME` is empty, and
there is no network unless you pass `--net`.

    scripts/sandbox --net cargo fetch
    scripts/sandbox --net scripts/fetch-mupdf.sh    # the live viewer's renderer, embedded in the binary
    scripts/sandbox cargo build --release -p phitex-cli
    scripts/sandbox cargo test

The binary is `target/release/phitex`. It needs a TeX Live 2026
installation for the formats' files, the packages and the fonts, found
through kpathsea from the `tex` on `PATH`. It never runs TeX Live's
programs.

`scripts/fetch-upstream.sh` fetches the pinned reference sources (TeX
Live's web2c, LaTeX, pgf) into `upstream/`. Only the tests use them.

## Use

`phitex build` builds a document to its fixpoint, BibTeX, biber and
makeindex included. `phitex watch` keeps it built as it is edited, with
the pages live in the browser:

    phitex build paper.tex
    phitex build -j 8 --output-format dvi paper.tex
    phitex watch -o out paper.tex
    phitex clean paper.tex

The engine is chosen from `--engine`, a `phitex.toml`, a `% !TEX program
= …` comment, or else the text (`pdflatex` for a LaTeX document, `pdftex`
otherwise). The first build with an engine makes its format from TeX
Live's ini file on the graph and caches it (`$XDG_CACHE_HOME/phitex`).
That first build takes longer.

Under an engine's name (`pdflatex`, `latex`, `pdftex`, `etex`, `tex`, as
a link to `phitex`), or with `--compat=NAME`, it takes that engine's
command line and makes one run, as the engine would:

    phitex --compat=pdflatex -interaction=nonstopmode doc.tex
    phitex --compat=pdftex -ini -etex doc.tex

The exit status is pdflatex's: 0 when the document built without errors
(warnings allowed), 1 otherwise. Standard error says what went wrong. A
failure inside PhiTeX itself is reported as a bug in PhiTeX, naming the
step that failed and the text it was on; `-v` shows the engine's whole
message.

## latexmk compatibility

`phitex build` does latexmk's job without running latexmk, pdflatex or
the tools:

- **Rounds.** A build reads back the files the job writes (the
  auxiliary file, the table of contents, the bibliography, the index,
  ...). It goes round until what the job reads back no longer changes, as
  latexmk reruns pdflatex. It takes no more rounds than latexmk's
  pdflatex runs on the same document. After 5 rounds (latexmk's
  `max_repeat`), the output is the last round's, and a warning names the
  files and keys still changing. A document that never settles under
  latexmk does not settle here either.
- **Tools by reads.** BibTeX, biber and makeindex are not run between
  rounds. When the document opens a file that one of them makes from a
  file the job writes, that read is the tool's output, computed in
  process from what the job wrote in the round before. The tool runs
  again only when its input changed. Bibliography styles and databases
  and index styles are found through kpathsea, as the real programs
  find them. A `\write18` naming one of these tools asks for its output
  the same way, with its arguments.
- **Shell escape.** No external program ever runs. By default
  `\pdfshellescape` is 2, TeX Live's restricted mode. In that mode a
  `\write18` may name bibtex or makeindex (TeX Live's
  `shell_escape_commands`), and any other command is disabled, as pdfTeX
  disables it. `--shell-escape` (or `shell-escape = true` in
  `phitex.toml`) makes it 1, as `pdflatex -shell-escape` does, and biber
  is then allowed too. An allowed tool runs in process. Any other command
  is refused with a diagnostic, so packages that need external programs
  (minted, for example) do not work yet.
- **Directories.** Without a `phitex.toml`, the build runs in the main
  file's directory, as `latexmk -cd` does. `-o DIR` is TeX's
  `-output-directory`. A `% !TEX root = …` comment is honoured.
- **One run, for latexmk itself.** To keep latexmk, give it PhiTeX as
  its pdflatex. Each run is then one run, not a fixpoint:
  `latexmk -pdf -pdflatex='phitex --compat=pdflatex %O %S' doc.tex`.

## WebAssembly

`phitex-wasm` is the engine as a `wasm32-wasip1-threads` program. Files
come through WASI and the engine's threads are Node workers sharing the
module's memory. There is no wasm-bindgen and no `unsafe` code.

    scripts/sandbox scripts/wasm.sh check    # build, then a plain TeX and a LaTeX document at 1 and 4 threads
    cd doc-dir && node .../scripts/wasm-run.mjs .../phitex-wasm.wasm --engine pdflatex -j 4 doc.tex

`check` makes both formats in wasm and compares each PDF with pdfTeX's,
byte for byte. It needs the pinned toolchain's `wasm32-wasip1-threads`
target (`rustup target add wasm32-wasip1-threads --toolchain
nightly-2026-08-08`, once), Node 22 or later, and TeX Live.

## License

GNU Affero General Public License v3.0 only (`AGPL-3.0-only`); see `LICENSE`.

This license applies to every revision of this repository, including
those committed before `LICENSE` was added. The `license` field in
earlier revisions of `Cargo.toml` (`MIT OR Apache-2.0`, later
`AGPL-3.0-or-later`) was a placeholder and never licensed this code under
those terms.
