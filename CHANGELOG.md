# Changelog

PhiTeX's versions converge to φ, a digit a milestone: 1.0 is the first
release (the alpha's milestones, the user's decision of 2026-10-31), then
1.6, 1.61, 1.618; a milestone's updates are 1.0.1, 1.0.2, … (`AGENTS.md`). A release's entry is a `## <version> (<date>)` heading
whose first paragraph is its summary: `phitex` says it once on the first
run of that version, `phitex news` prints the whole entry, and the
GitHub release's notes are the entry. The entries before 1.0 name
milestones, not versions.

## 1.0 (unreleased)

The first build of PhiTeX meant for use: pdfTeX's plain TeX and LaTeX,
and XeLaTeX, built by graph reduction, with exact output.
`docs/alpha-checklist.md` lists what has to be green before it reaches
`main`.

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
- **XeLaTeX.** `xelatex` and `xetex`: OpenType and TrueType fonts by
  name or file (fontspec, polyglossia, unicode-math), shaped by
  HarfBuzz's Rust port, right-to-left scripts included. The PDF is
  written from the pages directly, with no xdvipdfmx pass, and equals
  xelatex's (its XDV through `xdvipdfmx -q -E`): 131 of 131 documents of
  the XeTeX corpus (mc 76440). The same engine runs in the browser and
  the editors' WebAssembly engine, its fonts from Shelf.
- **SyncTeX.** `--synctex` writes `JOB.synctex.gz`, which TeX Live's
  `synctex` tool reads. In the live viewer a double-click on a word opens
  its place in the editor, and the editor's cursor and selection are
  highlighted on the page. Its places are approximate (below).
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
  Without it, TeX Live's restricted mode (2) applies. BibTeX, biber and
  makeindex run in process; another allowed command runs as a process in
  a scratch directory holding only the files PhiTeX hands it.
- `--exact-pdf` is accepted and hidden: the PDF is always exact.
- `--shelf` (or `files = "shelf"`): TeX's files from Shelf
  (v1.shelf.phitex.org, else shelf-phitex-v1.pages.dev where a network
  blocks the first; `PHITEX_SHELF_URL` for a mirror), kept in the cache;
  `phitex clean --all` empties it with the rest.
- `--synctex` (or `synctex = true`) writes the SyncTeX file; off by
  default.
- `--help` shows the common commands, `--help-all` every option.
- `--report` writes `JOB.phitex-report.txt` for a bug report, with no
  text of the document (`--report=full` adds the log and the sources).
- `phitex news` prints this file's entry for the running version, said
  once on its first run. The update check is off until the user sets
  `update-check` (`phitex config update-check on`); every run says so
  until then. `phitex upgrade` installs a newer release, its signature
  (minisign) and checksum verified.

### Known not to work

- **LuaLaTeX** (`luatex`, `lualatex`): rejected with an error.
- **XeLaTeX's edit speed** (the user's decision of 2026-10-30: recorded,
  not gated, for 1.61): on a two-column paper with line numbers (the
  user's, 18 pages, 16 threads, release build, medians of 20) a space
  takes 4.3 ms, a letter 37 ms and a line break 68 ms against the 16 ms
  bar, and a word 56 ms against 50 ms. Such layouts take every line
  apart on each page, so a letter rebuilds its page. Each edit's PDF
  equals a cold build of the edited file (80 of 80, mc 77010). Its cold
  build takes 41 s at 16 threads against latexmk -xelatex's 37 s.
- **Tests recorded red for 1.61** (the user's decision of 2026-10-31):
  tests that ran in no CI until the alpha's gate ran every test binary,
  red before the alpha's work as well (mc 77044). The engine core's
  review harness ends in a runaway node in two tests
  (`next_readers_follow_their_stops_across_splits`,
  `a_next_past_many_holes_goes_on_from_each`); five count runs, the
  speed that is recorded, not gated (`chain::chains`,
  `chain::census_plain`, `cuts`'s moved fire, `fatal`'s read-on,
  `pages2`'s word edit); two need a font the CI image lacks
  (`xetex::ini_features`, `ini_bidi`). `scripts/accl/gate-known-reds.txt`
  names each with its cause.
- **XeLaTeX's first build in the browser** makes the format there (a
  minute or more) until the web build ships it prebuilt, as it does
  pdfLaTeX's.
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
- **SyncTeX's places** (the user's decision of 2026-10-30: recorded,
  not gated, for 1.61): a glyph's source is found by matching its words
  to the text, so text a macro makes is credited to the macro's
  definition, where pdflatex names the line that used it. Against
  pdflatex's file, TeX Live's `synctex` gives the same place for 55% of
  the lines of amsldoc and the LaTeX suite's documents (forward) and 29%
  of the points on their pages (reverse); text typed in the document
  itself is mostly right (accl 22194).
- **Speed** (the user's decision of 2026-10-29: recorded, not gated;
  release build, accl 22194 on acclnode01, 48 CPUs, medians of 3):
  a cold build is not yet faster than pdflatex. amsldoc takes 2.46 s at
  1 thread and 2.95 s at 8, against latexmk's three pdflatex runs at
  2.17 s; para4000 takes 2.13 s at 1 thread and 1.01 s at 8, against
  pdfTeX's 0.65 s. Edits on amsldoc at 8 threads: one letter 9.7 ms and
  a line re-break 15.2 ms (bar 16 ms); a preamble definition 37 ms, a
  heading 78 ms and a label 328 ms (bar 50 ms); a page added 585 ms. A
  first build, the LaTeX format made, takes 9.2 s and 1.6 GB against
  `pdftex -ini` and pdflatex's 6.2 s and 64 MB.
- **Scaling:** amsldoc is slower at 8 threads than at 1 and slower
  again at 16 (3.20 s); para4000 is 2.1 times faster at 8 threads than
  at 1, and no faster at 16 or 32. Speculative runs are wasted: amsldoc
  runs each node 4.5 times on average at 8 threads.
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
