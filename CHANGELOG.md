# Changelog

PhiTeX's versions will converge to φ (1.6, 1.61, 1.618, …). Until the
first of them, every crate stays at 0.0.x and nothing is tagged
(`AGENTS.md`). The entries below name milestones, not versions.

## Alpha: plain TeX and LaTeX

The first build of PhiTeX meant for use: pdfTeX's plain TeX and LaTeX,
built by graph reduction, with exact output. `docs/alpha-checklist.md`
lists what has to be green before it reaches `main`.

### What works

- **Plain TeX and LaTeX, with pdfTeX's semantics.** The engines are
  `pdftex` and `pdflatex` (PDF), and `tex`, `etex` and `latex` (DVI).
  Their formats are made on the graph from TeX Live 2026's ini files the
  first time they are needed, then cached.
- **Exact output.** The PDF is byte-identical to pdfTeX 1.40.29's, object
  numbers and compressed streams included. The DVI is identical to TeX's.
  The files the job writes and reads back are identical too. The
  reference document is the AMS's `amsldoc` (44 pages, with a table of
  contents, an index, cross-references and hyperlinks). Its PDF equals
  the one pdflatex makes when latexmk runs it, at 1, 8 and 16 threads.
- **latexmk's job, in one build.** `phitex build` re-reads the files the
  document writes until they settle. It takes no more rounds than
  latexmk's pdflatex runs, and stops after 5 (latexmk's `max_repeat`)
  with the last round's output and a warning that names the files and
  keys still changing.
- **BibTeX, biber and makeindex in process.** A tool runs when the
  document reads the file it makes, on what the job wrote, and again
  only when that input changes. It runs as a memoised step of the build,
  not between runs. Its support files are found through kpathsea.
- **Watch mode.** `phitex watch` rebuilds on each change of the
  document's files or the editor's buffers, with the pages live in the
  browser. On the gate's edit histories each rebuild equals a cold build
  of the same text; since the engine starts each round from the
  previous build's (2026-10-18), that is checked, not promised.
- **Parallel.** The engine's steps run on every logical CPU by default
  (hyperthreads included; `-j N` or `PHITEX_THREADS` to change that).
  The output is the same at any thread count, and so is the number of
  rounds.
- **WebAssembly with threads.** `phitex-wasm` builds the engine for
  `wasm32-wasip1-threads`, with no `unsafe` code. Its threads are Node
  workers on shared memory. `scripts/wasm.sh check` makes the plain TeX
  and LaTeX formats in wasm and builds a document with each, at 1 and 4
  threads, byte-identical to pdfTeX's output.
- **Editors over LSP.** `phitex watch --lsp` is a language server
  (`phitex_live::lsp`, the same for the frontends' WebAssembly core):
  buffers, diagnostics, progress (`$/progress` for the first build and
  any past 400 ms, `phitex/status` after every build), messages for what
  needs the user, the outline, settings (`threads`, `main`, `follow`,
  `files`: a change of the session's starts a new one in the process,
  the viewer kept), and commands: a cold rebuild, stop, save the PDF,
  the caches' statistics and clearing by scope
  (docs/spec-editor-protocol.md). The viewer gets the same progress
  records (`{"event": "progress"}`), and is told when the file on disk
  is exact at every idle point.
- **Shelf.** `--shelf` / `files = "shelf"`: TeX's files from Shelf's
  packs (phitex-shelf), fetched once over HTTP(S), checked against the
  release's pack table, kept in the cache's `shelf/<release>/`; no TeX
  Live needed. A plain pdfTeX document from the tl2026.6 release is
  byte-identical to the one from TeX Live; XeTeX's fonts by name come
  from the release's font index.
- **pdflatex's command line.** Under an engine's name, or with
  `--compat=NAME`, `phitex` takes web2c's options and makes one run, so
  latexmk, editors and scripts can call it as pdflatex.

### Command line

- Exit statuses are pdflatex's: 0 when the document built without errors
  (warnings allowed), 1 otherwise. Usage and file errors, which exited
  with 2, and PhiTeX's own failures, which exited with 3, now exit
  with 1.
- An engine failure is one sentence naming what failed, where in the
  document, and that it is a bug in PhiTeX rather than in the document:
  a panic in an operation's hook or run (with the node and its text),
  read-back files that never settle (with the files), or a step after the
  last round that changed a file the document reads back. `-v` adds the
  engine's whole message. In compat mode these go to standard error.
- The round-cap warning names the files and the keys still changing.
- `-j` defaults to every logical CPU. `--help` describes the rounds, the
  tools, shell escape and the exit statuses.
- `--shell-escape` sets `\pdfshellescape` to 1, as pdflatex does.
  Without it, TeX Live's restricted mode (2) applies. No external program
  runs in either mode.
- `--exact-pdf` is accepted and hidden: the PDF is always exact.
- `--shelf` (or `files = "shelf"`): TeX's files from Shelf, from
  `PHITEX_SHELF_URL`, kept in the cache; `phitex clean --all` empties
  it with the rest.
- SyncTeX is off by default (`--no-synctex` is the default) until PhiTeX
  writes it. Before, every build printed a note that it was not written.

### Known not to work

- **XeLaTeX and LuaLaTeX** (`xetex`, `xelatex`, `luatex`, `lualatex`):
  rejected with an error.
- **SyncTeX:** not written, and the live viewer has no links between
  source and page yet (a double-click on a page opens nothing).
- **Shelf's server:** no hosted server is the default yet (the user's
  choice): `--shelf` needs `PHITEX_SHELF_URL` until the index and the
  packs a document needs are in the cache. XeTeX over Shelf needs a
  release made with its font index (`FONT_INDEX`) and with xdvipdfmx's
  and TECkit's trees (`dvipdfmx/`, `fonts/misc/xetex/fontmapping`,
  `fonts/cmap`, `fonts/sfd`); tl2026.6 has neither, so an XeTeX job with
  `--shelf` is refused there, and its PDF version follows xdvipdfmx's
  defaults instead of `dvipdfmx.cfg`'s.
- **External programs:** `\write18` runs only bibtex, makeindex and,
  with `--shell-escape`, biber, all in process. Every other command is
  refused, so packages that run programs (minted, or xindy- and
  Perl-based index and glossary tools) do not work.
- **Interaction:** a build never stops for input. `-interaction` is
  accepted and checked, but every job runs to its end as in batch mode.
  A first line of TeX commands on the command line (`pdflatex
  '\input x'`) is not supported. `-draftmode`, `-mktex`, `-ipc`, `-enc`
  and `-mltex` are refused.
- **The `.log`:** not byte-identical to pdfTeX's. Errors, warnings and
  pages are reported, but the transcript's details differ.
- **A page that fires inside a group or a display of the text around
  it:** the page is shipped after the rest of that text has run, so the
  rest reads the state from before the output routine. The log's page
  numbers there (a warning's "on page N") can be one too low, and so
  can `\thepage` read in such a spot (repro:
  `bench/r4/docs/page-align.tex`). No PDF of the test documents
  changes. The finisher's check of such a page also misses reads a
  resumed piece took from its continuation (they are preloaded, so
  `read_values` skips them).
- **Speed:** the alpha's bar is exactness. A cold build is not yet
  faster than pdflatex (amsldoc: 5.4 s at 8 threads against pdfTeX's
  3.0 s a run; para4000: 2.3 s against 0.73 s), and the edit bar is not
  met (amsldoc at 8 threads: one letter 22 ms and a line re-break 27 ms
  against 16 ms; a heading 127 ms, a label 574 ms and a preamble
  definition 82 ms against 50 ms). The numbers, their jobs and commits
  are in `docs/alpha-checklist.md`.
- **Scaling:** a LaTeX document's nodes still form long chains. The
  census bar (a cold critical path of at most 6 nodes) is not met: the
  path is 83 nodes on a 30-paragraph article and 138 on the
  environments article, the same on every run (a scaling fact, not a
  schedule's), so the census tests are ignored until it is.
- **Font expansion:** micro-typography's `\pdfadjustspacing` and
  `\pdffontexpand` are stored but not applied: pdfLaTeX breaks the
  paragraph with expanded fonts (and writes the scale), PhiTeX with the
  fonts as they are, so the lines differ. The largest cause of
  difference in the corpus sweep (about 120 documents; sweep job 14495,
  dev b89bb387, 2026-10-20).
  Repro: `bench/corpus-repro/microtype-expansion.tex`.
- **Type 3 (bitmap, PK) fonts in PDF:** pdfTeX embeds them; PhiTeX
  writes a DVI file instead of the PDF.
  Repro: `bench/corpus-repro/type3-font.tex`.
- **pgf/TikZ-heavy documents:** some time out (32 documents in the
  corpus sweep). The cause is not found yet, and there is no minimal
  repro yet.
- **Documents set only in `multicols`:** some ship no page ("No pages
  of output") where pdfLaTeX ships them. No minimal repro yet.
- **A local definition new in a later round:** where a region that
  opens a group runs again in a later round and now defines something
  locally (LaTeX's file-writing environment once its file exists lets
  `\write` gobble its arguments), the definition outlives the group, and
  the `.aux` loses `\begin{document}`'s `\relax`. An engine bug of the
  round core, fixed in the clean core (`docs/alpha-checklist.md`).
  Repro: `bench/corpus-repro/written-text-lost-write.tex`; test
  `corpus_repros::written_text_lost_write` (with the clean core).
- **`\inputlineno` across files:** it counts the lines of every file
  together, not each file's own. Test `etex::io` (ignored).
- **Alignments:** 57 documents in the corpus sweep stop with an
  alignment error (a misplaced `&` or `\cr`) that pdfLaTeX does not
  give. No minimal repro yet.

### How to run it

    scripts/sandbox --net cargo fetch
    scripts/sandbox --net scripts/fetch-mupdf.sh
    scripts/sandbox cargo build --release -p phitex-cli
    target/release/phitex build paper.tex          # to the fixpoint, like latexmk -pdf
    target/release/phitex watch paper.tex          # rebuild on change, pages in the browser
    target/release/phitex --compat=pdflatex paper.tex   # one run, as pdflatex

It needs TeX Live 2026 for the formats' files, the packages and the
fonts (found through kpathsea from the `tex` on `PATH`). The first
build with each engine makes its format and takes longer. The README's
"latexmk compatibility" section describes the rounds, the tools and shell
escape.
