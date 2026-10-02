# partex — design

A TeX engine in Rust that produces exactly the reference engines' output,
built as an incremental, parallel, memoizing document compiler rather than
a translation of `tex.web`.

**This is the archive** (frozen 2026-09-29): the design document as it
grew from the start to 2026-09-29, with the designs replaced on the way
(§7.0–§7.16) and the record of how §7.17 was reached. The design now is
`DESIGN.md`. Code comments that cite `DESIGN 7.x` or `§7.x` of DESIGN
cite this file.

## 0. Principles

1. **The oracle defines correctness.** The reference is the installed
   engine binary (TeX Live 2026). For the same inputs, environment and
   `SOURCE_DATE_EPOCH`, exact mode (§1) produces byte-identical DVI/XDV and
   PDF, identical `\write` files and exit status, and a transcript with the
   same semantic content: errors and their recovery, warnings, `\show` and
   `\showbox` output, tracing.
2. **Port behaviour, not code.** `tex.web`, `etex.ch` and `pdftex.web` are
   specifications of behaviour. Data structures and algorithms are designed
   natively for speed, sharing, snapshots and parallelism. Red flags:
   sentinel tokens inside lists, linked stacks, mirroring `mem` or
   save-stack word layouts. Prefer enums, flags and `Vec` passes.
3. **Internal statistics are not output.** Memory usage, pool, hash, font
   memory, trie and stack counts, `\dump` statistics and exact capacity
   limits describe tex.web's internals (runaway recursion still stops with
   an error, just not at tex.web's exact point). They are not emulated, and nothing
   (tracking, memoization, formats) is designed around them;
   `xtask/src/mask.rs` masks them in oracle comparisons.
4. **Every acceleration is a pure optimization.** Memoization,
   speculation, parallelism and compiled expansion can each be switched
   off at runtime, and output is identical either way.
5. **Deterministic commit.** Work may run in any order on any thread;
   results and effects are committed in program order. Scheduling never
   leaks into output.
6. **Native Rust only.** No external commands and no C libraries at run
   time: TFM, formats, fonts, shaping and compression are ours (or pure
   Rust crates).
7. **Everything is text-dumpable** (§9).
8. **Measure first.** No optimization without a profile, no performance
   claim without a benchmark.
9. **Latency is what users feel.** The number that matters is the time
   from a keystroke to the repainted page: edit, resume, re-typeset, ship
   the changed pages, update the preview. Cold-build speed, compiled
   expansion and parallel line breaking only touch the middle of that
   chain. Re-running everything after an edit, rewriting the whole PDF,
   the viewer reloading it, and LaTeX's repeated runs for the aux file
   dominate it, so they come first (§7.4).

## 1. Output modes

| Mode | Layout | Backend bytes | Transcript |
|---|---|---|---|
| **exact** (default, and the test gate) | identical | identical to the oracle | identical apart from masked statistics |
| **visual** (proposed) | identical | free: object order, compression level, parallel compression, font subsetting, xref streams | free |

There is **one layout engine**. Visual mode changes only the backend and
the transcript. Line breaks, page breaks and every glyph and rule position
stay exact: if layout could drift, there would be two engines, and the
fast one's bugs would hide behind "visually close". Previews that show
stale pages while recomputing are a UI feature on top, and must converge
to the exact layout.

## 2. Compatibility targets

| Stage | Engine | Gate | Status |
|---|---|---|---|
| C1 | TeX82 3.141592653 | trip, byte for byte (statistics masked) | done |
| C2 | e-TeX | etrip against pdfTeX's output | done (18/18) |
| C3 | pdfTeX 1.40.29, DVI mode | plain and LaTeX corpus: logs and DVI identical | in progress: pdfTeX primitives |
| C4 | pdfTeX, PDF mode | PDF byte-identical under deterministic settings | not started |
| C5 | XeTeX 0.999998 | XeTeX corpus (`*.xetex.tlg`) | not started |
| C6 | LuaTeX 1.24 | LuaTeX corpus (`*.luatex.tlg`), embedded Lua | not started |

pTeX, upTeX, Aleph and HiTeX are out of scope unless requested.

Byte-exact PDF (C4) needs a native port of zlib's deflate that reproduces
its exact bitstream at pdfTeX's settings; other deflate implementations
give different bytes. Visual mode can use any compressor.

**pdfTeX's bugs.** Identical output means reproducing pdfTeX's bugs too:
behaviors of its code that depart from what the code evidently means (a
stale pointer, a wrong side). Each one partex reproduces is marked at its
site with a `PDFTEX_BUG(<name>)` comment (what pdfTeX does, and why it is
wrong), listed in `partex_engine::bugs::PdftexBugs`, and switchable:
`PARTEX_PDFTEX_BUGS=none` (or `-<name>,...`) gives the output pdfTeX would
give without it. All are on by default. Found so far:

| Name | pdfTeX | Effect |
|---|---|---|
| `lig_margin_kern_var` | `try_break`'s variations of the marginal kerns: `char_pw` of a ligature's `lig_char` is 0 | a ligature at the margin changes line breaking under `\pdfadjustspacing=2`, `\pdfprotrudechars=2` |

## 3. Test corpus and oracle policy

Sources are pinned in `scripts/fetch-upstream.sh` (shallow clones in
`upstream/`, not committed) and indexed by `cargo xtask corpus`.

| Suite | Location | Covers |
|---|---|---|
| trip | `texlive-source/texk/web2c/triptrap/` | every TeX82 primitive and error path |
| etrip | `texlive-source/texk/web2c/etexdir/etrip/` | e-TeX |
| web2c tests | `texlive-source/texk/web2c/{tests,pdftexdir,xetexdir,luatexdir}/` | engine regressions |
| LaTeX kernel | `latex2e/**/testfiles*/*.lvt` (+ `.pvt`) | ~1.5k l3build tests |
| expl3 | `latex3/**/testfiles*/*.lvt` | ~270 tests, heavy expansion |
| e2e | `tests/e2e/`, `xtask/src/e2e.rs` | whole runs against `tex`/`pdftex`, formats, incremental rebuilds |
| fuzz | generated | differential fuzzing against the oracle |

The ground truth is the installed binary, not upstream `.tlg` files (they
encode the development kernel's logs). Each test runs under the real
engine; its raw outputs go to `refs/<engine>/` (gitignored, regenerated
with `--oracle`) and ours are compared against them.

## 4. Architecture

```text
crates/
  partex-engine  typed nodes, fonts (TFM), back end as pure functions:
                 packaging, line breaking + hyphenation, page builder,
                 math, alignment, TeXXeT; page IR and the DVI writer
  partex-core    front end: input, tokens, expansion, main control, eqtb,
                 save stack, formats, ship-out to page IR; no_std + alloc
  partex-kpse    file lookup with kpathsea semantics
  partex-trace   tracing/profiling facade
  partex-cli     binary; tex/etex/pdftex personalities; -watch sessions
xtask/           corpus, oracle runs, trip/etrip/e2e, masking, checks
```

Planned: a PDF backend (C4), OpenType fonts and shaping (C5), a Lua
runtime (C6), a wasm host crate.

### 4.1 Pipeline

```text
files ─► input (lines, catcodes) ─► expansion ─► main control ─┬─► lists
                                                               │
      ┌───── pure back-end calls (futures, memoized) ◄─────────┘
      ▼
  line breaking · math · alignment · packaging
      ▼
  page builder ─► ship-out ─► page IR ─► backends (DVI, PDF), per page in parallel
```

Expansion cannot be split off ahead of typesetting: it reads layout
results all the time (`\wd` after `\setbox`, `\lastbox`, `\prevgraf`,
`\pagetotal`, `\badness`, marks, whatever the output routine did), and
LaTeX relies on it (`\settowidth`, float placement, TikZ measuring text).
The front end therefore stays sequential and in order. Parallelism comes
from back-end futures and holes (§7.5), per-page backends, and regions
run in parallel rounds, validated by their read sets (§7.7).

### 4.2 WASM

Core crates build for `wasm32-unknown-unknown` (`xtask check` enforces
it). `partex-core` touches no OS: files, time, terminal, interaction and
shell escape go through the `Host` trait (Typst's "World"). Output is
identical native and wasm. Threads are optional: on wasm without threads
the `Executor` runs sequentially. There is no native code generation on
wasm; compiled expansion runs as interpreted IR there.

## 5. Data model

### 5.1 Tokens and names

- Today: a token is `cmd*256 + chr`, or `CS_TOKEN_FLAG + p` for a control
  sequence at hash position `p`. Token lists are immutable vectors with a
  `protected` flag, shared by reference.
- Target: `Token(u32)`: bit 31 set means a control sequence, and the rest
  is a `Sym`, an interned name in an append-only interner with a stable
  name hash. Otherwise `cmd << 21 | chr`, where `chr` holds a full Unicode
  code point (21 bits), as XeTeX and LuaTeX need. Hashing uses names and
  (catcode, char), never table positions. Macro bodies, marks and `\write`
  texts are `Arc<[Token]>`.

### 5.2 Nodes

- `enum Node` in `Vec`s, 24 bytes per node. Boxes are `Arc<BoxNode>`,
  immutable once packed, and cache their content hash (Merkle), so `\copy`
  and shared boxes cost nothing to hash.
- Characters are glyph runs (`Glyphs`): up to 19 same-font characters in
  one node, split canonically so structural equality matches
  character-level equality.
- Unicode: characters are `u8` today. C5/C6 need code points for TFM fonts
  and glyph IDs for OpenType. Plan: `Glyphs` keeps `u8` runs for 8-bit
  fonts, and a native-word node carries an `Arc` shaped run (glyph IDs,
  advances, offsets), like XeTeX's `native_word` node.
- **Source origins** (for SyncTeX, §5.6): boxes, glyph runs, glue, kerns
  and math nodes record the file and line they came from, as pdfTeX's
  SyncTeX does. They must not grow the 24-byte node: a compact origin id
  (file, line) in a side field of `BoxNode` and in the spare bytes of
  small nodes, with glyph runs shortened if needed.
- LuaTeX's node library (C6) lets Lua callbacks walk and rewrite linked
  node lists (`node.next`, direct integer IDs). Plan: when a callback is
  registered, the list is lent to Lua as an arena with stable integer
  handles and links, then turned back into a `Vec<Node>`. Lists pay for
  this only when a callback runs. This must stay possible as the node
  model grows.

### 5.3 State

- eqtb, the hash and the save stack are `JVec`s (`journal.rs`, a
  journaled vector): the running engine reads and writes one flat `Vec`
  (no indirection on the token path), `IndexMut` sets a dirty bit per
  512-word chunk, and a shared base of `Arc` chunks holds the contents
  at the last snapshot. A clone is frozen: it shares the base's clean
  chunks and copies only the dirty ones; `commit` (the engine's
  `snapshot`) makes those the new base, and a checkpoint thaws into a
  flat vector when it resumes. A checkpoint so costs the chunks written
  since the last one, not the whole table. `CowVec`, which shared chunks
  on the live path, is gone: its indirection on every meaning cost a
  plain run 2–3%, so it had been flat unless the run kept checkpoints,
  and then every checkpoint copied `hash_extra`'s head (94k control
  sequences in the PGF subset). Token lists are flat while the
  engine runs (the token path reads them) and frozen in a checkpoint:
  shared chunks of 64 records whose tokens are shared `Arc<[i32]>`s. A
  clone refreezes only the lists changed since the last snapshot (a flag
  set by `get_mut`, found by a scan of the flags and cleared at
  `commit`), and a checkpoint thaws them when it resumes. A list of
  changed ids instead of the scan cost a plain run 1% (its pushes on the
  hot `get_mut`), measured, so the scan stays. Sharing on the live path instead (copy-on-write chunks and
  token vectors) cost 18% of a plain run, from two more dependent loads
  per token. The string pool is only as long as its contents, so a
  clone does not copy its slack. Registers above 255 are a sparse
  map. Glue, boxes and paragraph shapes named by eqtb are ids into
  `objs.rs` stores.
- Target: a persistent, versioned state map (HAMT) keyed by `Sym` and typed
  parameter banks, with a structural hash per node. Snapshots are O(1), a
  group restore swaps roots, two states compare in time proportional to
  their difference, and every read is logged per cell (§7.1). The
  interpreter never walks the trie per token: it reads and writes a flat
  working copy and journals the cells it changes, and the journal is
  committed into the trie at region boundaries (a few node copies per
  changed cell), which is where snapshots, hashes and diffs are taken.
- **Whole-engine state hash.** Early cutoff (§7.4) compares the complete
  state at a boundary with the previous run's: eqtb, hash and save stack,
  the semantic nest, the page builder (current page, contributions,
  insertions, marks), the input stack and open files' read positions,
  open `\openout` streams, fonts loaded and changed, and counters. Every
  part must hash structurally, by content and names, never by addresses.
- **On-disk checkpoints** (done). `Tex::save_state`/`load_state`
  serialize the whole engine through the `Persist` trait
  (`persist.rs`): one generated line per type, destructuring
  exhaustively, so a new field does not compile until it is saved.
  `Rc`/`Arc` values are written once and shared again on load. That
  sharing matters for more than size: the session matches open files
  and font map entries by identity (`Arc::ptr_eq`), so a loaded
  checkpoint must hold the *same* file contents as the loaded read log,
  not equal copies. `PARTEX_PERSIST_CHECK=1` round-trips every
  checkpoint a session keeps and resumes from the loaded copy; e2e
  passes that way. A **pooled** saver writes shared values out of line
  into one pool, so each checkpoint is a separate segment that loads
  alone and in any order, sharing the pool's values (loaded by id on
  first use). A session saved by `-converge` is described in §7.9.
- **What a checkpoint shares** (done). Chunks and frozen token lists are
  `Arc`, and `Tex<H, T>` is `Send` whenever its host and tracker are (a
  compile-time assertion in `tex.rs`); the session host's shared logs
  are an `Arc` behind an `RwLock` (`Locked`), so the session host is
  `Send + Sync` too (asserted in `session.rs`). The string pool, `str_start`, the input buffer and the input and
  parameter stacks are `Flat` (`flat.rs`): flat while running,
  4096-element `Arc` segments in a checkpoint, found by a memcmp against a
  shadow (no write tracking) and thawed on resume; saved as segments, so
  saved checkpoints share them too. Caches are per clone and start empty
  (`search_string`'s index; copying it had cost 1.5 MB per checkpoint) or
  are shared lock-free (`id_lookup`'s shortcut, an `Arc` of tagged atomic
  words); the lazily read font map's lookups live in the engine, not in the
  shared table. Measured on the 2-page article, cold `-converge` with 166
  checkpoints: 2.15 s and 1.15 GB before, 1.35 s and 394 MB after (the
  unshared bytes per checkpoint 2.4 MB → 0.9 MB, now mostly the
  unchunked eqtb and hash heads); PDF and log identical.
- **`Send + Sync` state** (done; roadmap step 5). `Tex` is `Send` and
  `Sync` (asserted in `tex.rs`), so a snapshot can go to a worker thread
  or be read from several at once. What the running engine updated
  through `&self` (the memo's recording state, stamps and logs, the
  tracker's bitsets) is relaxed atomics (`relaxed.rs`: `Flag`, `Word`
  and an append-only `Log`), plain loads and stores on x86 and ARM, so
  the token path pays nothing. A clone never writes to what it clones:
  `Flat` and the token store compare with their shadow without updating
  it, and `commit` updates the shadows at a snapshot (`Tex::snapshot` is
  commit, then clone). Tables that change rarely and were copied by
  every clone (hyphenation words and links, the primitive table) are
  `Arc<Vec>`s, copied on write. A `Rest` snapshot is a checkpoint
  (§7.0), so memory is what a checkpoint does not share: `Flat` segments
  are 4 KiB (the input stack's top changes at every command; 4096
  elements had cost 98 KB per checkpoint) and token chunks 64 records.
  Measured on the small article (`bench/inputs/article.tex`), cold
  `-converge` with 331 checkpoints: 1.58 s and 517 MB before, 1.50 s
  and 368 MB after, unshared bytes per checkpoint 1.05 MB → 0.33 MB
  (a rerun from the saved session, 52 checkpoints: 1.06 s → 0.78 s,
  1.37 MB → 0.64 MB). A plain run of the 295-page course: 330.6 G →
  333.4 G instructions (+0.8%: dirty bits and flags on writes), wall
  +2.8% averaged over 8 alternating pairs on a loaded machine (from
  +0.1% to +5.5%); PDF and log identical.

### 5.4 Fonts and formats

- Large inputs (fonts, images, `pdftex.map`) are memory-mapped instead
  of read, sharing pages with the OS cache (zero-copy, as mold does).
- TFM files are parsed once into shared `Font`s, with each character's
  metrics resolved at load so a lookup is one index.
- Formats are partex's own: native serialization (`codec.rs`) of tokens,
  stores, eqtb, hash, the primitive table, fonts, hyphenation patterns and
  e-TeX state. Target: a memory-mapped snapshot of the §5.3 state, so
  loading is O(1).

### 5.5 Effects

Everything that leaves the engine is an **effect**: log and terminal
text, `\write`, `\openout`, `\closeout`, `\message`, file opens and
ship-out of pages. By default effects are printed directly through TeX's
selector, and the session captures them at the host boundary as logs with
marks, which is what a splice reuses. With `PARTEX_EFFECTS=1` (and always
under `PARTEX_MACHINE=1`) they are values (`effects.rs`): the engine
appends `Effect`s and a link step writes the files (§7.6). The old path
stays the default until the session uses the runtime.

### 5.6 Pages

Ship-out walks a box once into an owned page IR (`pageir::Page`): glue is
set, running rules resolved, `\special`s expanded, and `\write`s performed
in TeX's order. TeXXeT's right-to-left segments are emitted in visual
order with `Edge` items. Backends see only pages, never engine state, so
pages encode in parallel.

- **Page hashes.** Each page's IR has a content hash. Encoded pages
  (DVI bytes, PDF content streams) are cached by it, so an edit re-encodes
  only pages that changed (§7.4, §7.6).
- **Output as values.** The page walk stays at ship-out, where TeX
  observes it (object numbers, `\pdfsavepos`, late `\write`s and
  literals). The bytes do not: content streams stay in memory, are
  compressed in parallel once the build has converged, and the exact
  file is written once, in pdfTeX's object order. Passes before
  convergence (§7.10) encode nothing. Offsets (`/Length`, xref,
  `startxref`) are computed at that write; TeX never observes them.
- **SyncTeX.** Jumping between source and output is core to editing.
  Ship-out already walks every box; with source origins on nodes (§5.2)
  it writes pdfTeX-compatible `.synctex` records at almost no cost.

## 6. Diagnostics

TeX's error report is output and is kept byte for byte. Each error also
yields a structured `Diagnostic` (`crates/partex-core/src/diag.rs`),
handed to `Host::diagnostic`:

| Field | Content |
|---|---|
| `severity` | `Error` (TeX recovers) or `Fatal` (TeX stops) |
| `code` | stable kebab-case id: `undefined-control-sequence`, … |
| `message` | TeX's message without `! ` and the final period |
| `help` | help lines, or the `\errhelp` text |
| `frames` | the whole input stack, innermost first: file lines, terminal and `\read` lines, macro expansions (name, tokens read, tokens left), other token lists; not truncated by `error_line` or `\errorcontextlines` |
| `suggestions` | likely fixes, such as defined names within a small edit distance |

`Diagnostic::render` is the canonical text form:

```text
error[undefined-control-sequence]: Undefined control sequence
  --> paper.tex:12:13
   |
12 | Hello \greet world
   |       ^^^^^^
   = in expansion of \greet: `->\fooo ` | `{}`
   = help: The control sequence at the end of the top line
   =       of your error message was never \def'ed.
   = did you mean `\foo`?
```

Collection: `print_err` starts capturing the message; `error` and
`succumb` build the diagnostic before TeX prints anything else; frames
print through a private `DIAG` selector that changes no output and
restores `tally`, `first_count` and `trick_count`. `Params::diagnostics`
switches it off; the oracle tests check that TeX's output is the same
either way.

Next: where each macro was defined (file and line), a caret on the
offending token inside macro bodies, per-code suggestions (where math mode
or a runaway argument started, mismatched `\begin`/`\end`), and warnings
(overfull boxes, missing characters) as diagnostics.

### 6.1 Command line and terminal

The transcript, the outputs and the exit status are the engine's and stay
byte-identical; only the terminal is free. Which command line an
invocation speaks (`crates/partex-cli/src/compat.rs`):

- **Under an engine's name** (argv[0]: `tex`, `pdftex`, `pdflatex`,
  `etex`, `latex`, … as `target/partex-shim` links them): that engine's
  web2c command line and TeX's own terminal output, so `latexmk`,
  editors and scripts keep working. `xetex`, `xelatex`, `luatex` and
  `lualatex` are reserved: partex refuses them until those engines exist.
- **`partex --compat=NAME …`** (first argument) or **`PARTEX_COMPAT=NAME`**:
  the same, for scripts that call `partex` itself. The xtask harnesses
  (e2e, trip, etrip, bench) run `partex --compat=tex` (`-engine=` still
  picks the engine) or `--compat=pdftex`.
- **Bare `partex`** is the modern command line. A TeX option there
  (`partex -ini …`) is refused with a pointer to `--compat`.

A resident session's server is started under the invoking name
(`arg0`), so it speaks the same command line as its client.

**The modern command line** (`modern.rs`), cargo-style subcommands:
`partex build [file]` runs the `-converge` path (fixpoint, BibTeX and
makeindex in process, persisted session), `watch` a watch loop in
machine mode (below; `--no-machine`, `machine = false` or
`PARTEX_MACHINE=0`: the `-watch` loop), `check` one pass without writing, `why` what the last build did (its
pass reports with `Report::why`, and every warning), `trace` a build
with the Chrome/Perfetto timeline, `clean` the files the last build
wrote and its saved session. What to build: the command line, else
`partex.toml` (here or above: `main`, `engine`, `output-dir`,
`shell-escape`, `viewer`; a small TOML subset read by `config.rs`), and
the file's `% !TEX program` and `% !TEX root` comments; the engine
defaults to `pdflatex` for a LaTeX file, else `pdftex`. The subcommand
turns this into the command line a compat invocation would get
(`pdflatex -interaction=nonstopmode [-output-directory=D] paper.tex`),
so the job, its session key and its outputs are exactly those of
`pdflatex -converge`; e2e case `modern` builds a LaTeX document with
citations, an undefined reference, an overfull box, a font
substitution and an error, against pdflatex and BibTeX run to their
fixpoint: every file identical. Formats: partex cannot load pdfTeX's,
so the first build makes the engine's format as fmtutil would, in the
cache (`formats-<build>`, one set per partex binary; `PARTEX_FORMATS`
moves it), which the native host searches before kpathsea.

**`partex watch` runs in machine mode** (§7.0, `machinehost::Watch`).
The build is recorded as regions (coarse, 131072 commands; refine off),
and an edit re-runs only the regions whose reads it changed, finely,
with coarsening while idle (§7.0, "Memory"). A rebuild runs to its
fixpoint as further rebuilds: after each pass the outputs whose bytes
changed are written (the PDF first, renamed into place), BibTeX and
makeindex run as in `-converge`, and every file the job read that now
differs on disk (its `.aux`, `.bbl`, `.toc`) is an edit for the next
pass; a file looked for and not found is a cell too (its key names what
was looked for), so one appearing re-runs the regions that looked. The
changed inputs are found by modification time every 50 ms (the TeX
tree's and the files not found every tenth time), then by contents.
What the renderer shows comes from effects (`Effect::Diagnostic`,
`Effect::Shipping`), so a region reused brings its warnings and pages
with it. `build` and `check` stay on the session path: a cold machine
build costs +6% cycles and more memory, and a watch has no saved
session yet, so its first build is always cold. e2e case
`modern_watch` watches `modern.tex` through three edits (a word, a
forward reference, its label) against pdflatex and BibTeX run to their
fixpoint: every file identical after each, and the report must show the
machine's lines. On the course a one-word edit takes 88–100 ms when its
region was re-run before; the first edit in a region re-runs it at the
fine grain, 0.25–0.7 s. What would make that cheaper: refining while
idle the region around the last edit, which `Build::refine` cannot do
after the first rebuild (a region recorded in an earlier rebuild keeps
an exit snapshot patched only as a rebuild replays it); refining with
that patch is the next step.

**Copying the PDF.** `--copy-pdf[=DIR]` (`build`, `watch`, and `trace`,
which builds) or `copy-pdf = true | "dir"` in `partex.toml` copies the
final PDF after each successful build or watch rebuild (history below
error; not for `check`) into `DIR`, by default the directory partex was
invoked from. That directory is taken before resolving chdirs to
`partex.toml`'s, so a flag's relative `DIR` is the invocation's and the
key's is the toml's; the flag (or `--no-copy-pdf`) overrides the key.
The copy is atomic (a temporary file in `DIR`, renamed over the old
copy, so a viewer never reads half a PDF), is skipped when the
destination is the PDF itself, and prints one `Copied` status line. It
happens in `finish()`, after the outputs are written; the engine never
sees it.

**Rendering from events, not the log.** The terminal is drawn from
what the engine reports as data (§6): `Host::notes` asks for warnings
(each overfull, underfull, tight or loose box report, with its lines,
amount and `short_display` excerpt) and notes (the text of each
`\message` and of each `\write` to the terminal, one per command), and
`Host::shipping` announces each page. A session logs them next to its
terminal output, with the same marks, so resuming from a checkpoint
rewinds them, early cutoff splices the previous build's tail in, and a
persisted session saves them: the diagnostics of the final pass are
exactly those of a fresh run. `warnings.rs` groups them: boxes by kind
with the worst first, and LaTeX's own warnings, recognized in their
notes by their standard form (`Reference `x' … undefined`, `Citation`,
`Font shape … using …`, any other `LaTeX`/`Package`/`Class … Warning:`),
deduplicated by subject with every location; everything else a note
says is shown with `-v`, and `-vv` adds TeX's raw terminal stream.
Status lines and the live line (pass, pages shipped) go to standard
error, coloured on a terminal unless `NO_COLOR` or `--color=never`;
errors are nonstop (`--interactive` gives TeX's own terminal and
prompt, one pass). Errors are drawn rustc-style from their frames
(`snippet.rs`): the file line with a caret under the token TeX had
just read and the first suggestion, the backtrace folded to the
document's outermost and innermost macros (names with `@` are a
format's internals; `-v` shows every level with its text, and TeX's
help), a LaTeX `\errmessage` split into its headline and notes without
the advice about `H`, and identical errors shown once with a count.

## 7. Execution model: incremental, parallel, lazy

TeX is an interpreter over global mutable state in which code is data:
`\def` stores programs in cells, and catcodes make even the tokenizer
depend on state. No static program exists to analyse ahead of time, so
dependencies are **recorded while the job runs**, and a rebuild
**propagates changes** through what was recorded. This is how language
runtimes handle the same problem, and the design borrows from them
openly:

| From | Idea | partex |
|---|---|---|
| LuaJIT (tracing JIT) | a trace: guards on assumed values, then effects; replay while the guards hold, side exit when one fails | a region trace (§7.3): guards on the cells it read, then its effects; a failed guard re-executes the region |
| Julia (method invalidation, world age) | compiled code keeps backedges to the methods it used; redefining a method invalidates exactly those | each cell keeps backedges to the traces that read it; `\def` invalidates exactly its readers (§7.4) |
| V8 (speculation, deoptimization) | optimize under an assumption, deoptimize when it fails | parallel rounds from speculated entry states, re-running only regions whose guards failed (§7.7) |
| Salsa, Adapton, Jane Street Incremental, self-adjusting computation | versions by content (early cutoff), a dynamically recorded nested graph, change propagation in order | versions are content hashes; propagation in program order stops where a re-executed region produces the same values (§7.4) |
| Haskell thunks, futures and promises, partial evaluation | values stay symbolic until forced; the residual program holds only what could not be computed | holes (§7.5): unresolved values that flow until something observes them |
| Linkers | code refers to symbols; addresses are assigned at link time | PDF object numbers, fonts' numbers and byte offsets are symbols resolved by the link step (§7.6) |
| CPython's `.pyc` cache | compiled source keyed by the source's hash | tokenized lines keyed by bytes and catcode table (§7.2) |
| Bazel, Nix, ccache | a content-addressed action cache shared across builds | traces keyed by their guards' and inputs' hashes (§7.9) |
| Clojure's HAMTs, git's trees | persistent maps with structural sharing, Merkle hashes | state snapshots in O(1), diffs and hashes in O(changes) (§5.3) |
| Cilk, Rayon | work stealing | the round scheduler (§7.7, §7.14) |
| Go's `context`, React's concurrent rendering | cooperative cancellation of stale work | a newer edit cancels regions whose guards it breaks (§7.11) |
| ASan, TSan; QuickCheck, Hypothesis | runtime checkers; property-based testing | the sanitizer mode and edit fuzzing (§7.12) |

The result must be what the sequential engine produces, byte for byte
(§0.1). Every mechanism below is a way of reaching that result faster,
and each can be switched off (§0.4).

The layers, bottom up:

1. **Cells and versions** (§7.1): every piece of state is a cell whose
   value has a content hash; a write makes a new version.
2. **Source text as values** (§7.2): a line's value is the tokens TeX
   reads from it.
3. **Regions and traces** (§7.3): a stretch of execution recorded as
   guards and effects, nested hierarchically, with boundaries found
   dynamically.
4. **Invalidation and propagation** (§7.4): an edit dirties the traces
   that read what changed; propagation in program order replays clean
   regions and re-executes dirty ones.
5. **Holes** (§7.5): values not yet known stay symbolic until observed.
6. **Effects as values and the link step** (§7.6): output is assembled
   from region effects; numbering and offsets are resolved at the end.
7. **Parallel rounds** (§7.7): regions run concurrently from speculated
   or partly unknown entry states, iterating to the sequential fixpoint.

Policies (granularity, what to cache, whether to suspend or speculate on
a hole) are not fixed in advance: they are chosen from traces and
profiles (§7.8).

**Read §7.16 first.** It is the current execution model (2026-09-27):
units cut at clean points, the page builder as a fold, checkpoints as
shared roots. §7.0–§7.15 are the record of what was built and measured
on the way, and their targets hold only where §7.16 does not say
otherwise.

### 7.0 A language-independent runtime

None of the layers above depends on TeX. Cells and versions, traces,
backedges, propagation, holes, rounds and the link step need only a small
interface from the language they accelerate, so they live in their own
crate (`partex-incr`), generic over it, and TeX is one client (as Salsa
and Adapton are generic over the computations they memoize):

```text
trait Machine: Clone + Send {
    type Cell: Ord + Hash;          // a unit of state
    type Value: Hash + Eq;          // a cell's content (its version is its hash)
    type Effect;                    // what leaves the machine, as a value
    type Boundary: Hash + Eq;       // where a region may begin or end
    fn step(&mut self, t: &mut impl Recorder<Self>) -> Step<Self::Boundary>;
    fn get(&self, c: &Self::Cell) -> Option<Self::Value>;    // for validation
    fn set(&mut self, c: &Self::Cell, v: Self::Value);       // for replay, exit patching
    fn at(&self) -> Self::Boundary;                          // current position
}
trait Recorder<M: Machine> {       // the tracker, seen from the machine
    fn read(&mut self, c: &M::Cell, v: &M::Value);
    fn write(&mut self, c: &M::Cell, v: &M::Value);
    fn effect(&mut self, e: M::Effect);
    fn hole(&mut self, c: &M::Cell) -> Hole;                 // an unresolved read
    fn force(&mut self, h: Hole) -> Forced<M::Value>;        // suspend or speculate
}
```

The runtime owns everything else: recording traces, the hierarchy and the
chooser, backedges, program-order propagation, the round scheduler, the
content-addressed store, cancellation, decision records and the
sanitizer. A stub language (a few instructions: assign, read, branch on a
cell, emit, with source lines as cells) drives its tests, so propagation,
holes and rounds are proved correct against a sequential run before TeX
slots in. TeX's side is the adapter: its tracker is the `Recorder`, its
checkpoints are `Clone`, its boundaries are §7.3's candidates, its effects
are §7.6's, and the lexer layer (§7.2) is how its source lines become
cells.

**Built** (`crates/partex-incr`, `no_std` + `alloc`, threads behind a
feature; no TeX code). Traces are recorded at the machine's candidate
points: coarse regions on a cold build, fine ones where a rebuild
re-executes. The runtime keeps backedges and writers per cell, with
program-order keys that leave gaps for inserted regions (renumbered when
a gap runs out). Propagation keeps the difference set `D` between the new
state and the old run at the same point. A cell entering `D` marks its
readers up to its next writer. A dirty region runs until the machine
reaches a later old region's entry; the cells either run wrote are then
compared by version. When no dirty region is left, the old final state
is patched with `D`. The link renders chunks in parallel, lays them out
by prefix sums and copies them into one buffer; symbolic numbering and
forward references resolve there. Rounds sweep in program order over
composed entries, then run the invalid regions in parallel. Cells that
several regions wrote in the last round become holes, and the first
invalid region, whose entry is exact, gets none. A suspended region is
resumed in the sweep once its entry is exact. Every path is switchable.
Property tests over random programs and edits compare all of them with
an independent interpreter, byte for byte, and a sanitizer compares each
rebuild with a fresh one.

The sketch changed where the stub showed why:

- `set` takes `Option<Value>`, because a cutoff must be able to restore
  an absent cell;
- `seek(boundary)` moves the position on replay, since the position is
  not a cell;
- `digest()` is the state hash the sanitizer compares;
- `Step` carries a candidate's level, which the hierarchy uses, and
  `Suspended`;
- the machine renders its own effects, and counts their link-time
  symbols (`render`, `allocs`);
- `Split` finds cold-start positions;
- the recorder has no `hole(c)`. The runtime plants hole *values* in a
  region's entry (`hole_value`), and the machine carries them as
  ordinary values, affine arithmetic included, so copies and stores need
  no hook. `force(h)` is as sketched, and `subst_*` resolves holes.

Measured (`--example measure`, 10k statements, one-line edits,
24 threads):

| Program | Incremental edit | Recorded from scratch | Plain run |
|---|---|---|---|
| document-shaped, paragraph cost 20k units | 0.44 ms | 58 ms | 60 ms |
| cheap statements | 0.28 ms | 5.6 ms | 1.1 ms |

- Rounds on the document-shaped program: 4.8× the plain run with
  speculation (3 rounds), 3.5× with suspension, 0.74× with holes off (it
  hits the round cap).
- A random program, with every variable live everywhere and forcing
  points everywhere, never beats the plain run.
- Recording costs about 5× on cheap statements.
- The cold first round is mostly wasted: nothing guesses its entries yet
  (§7.8).
- **Warm starts** (`rounds::run_warm`, a `Warm` taken from the last
  run's `Outcome` or from a recorded `Build`) seed every region with the
  previous run's trace between the same two boundaries (those still in
  the program, `Split::still_at`). The first sweep validates them against
  the composed entries, so an unchanged region costs a guard check, the
  old writes compose the guesses, and the previous run's volatile cells
  are holes from round 1. The sequential fallback now records one trace
  per region, so a warm start after a capped run still lines up. After a
  one-line edit of the document-shaped program (24 threads, 88 regions):
  cold 12.2 ms (3 rounds, 190 runs), warm 2.4 ms (1 round, 1 run) with
  speculation; 14.7 ms against 2.1 ms with suspension. An unchanged
  program runs nothing. The Build's rebuild (0.39 ms) stays faster for a
  single edit; warm rounds are for what a rebuild cannot do in order:
  cold-ish starts from a stale run, and the aux fixpoint (§7.10).
- **Flat recording** (`build::Config::flat`, on by default;
  `Machine::index` gives a cell a dense number). A first touch is found
  in a bit set instead of an ordered map; a first read keeps its version,
  a first write only the cell, and the values written are read from the
  state at the region's end; the bits are cleared by what the region
  touched. It records exactly what the maps record (property test over
  200 programs and three grains). Recording a build, against a plain
  run: 6.4× with maps, 3.5× flat on the random program; 5.4× and 3.7× on
  the document-shaped one. This is the shape of the session's tracker
  (`intervals.rs`: bits over dense eqtb and family numbers, sets for the
  rest), which is what TeX's `Recorder` becomes.

**TeX's adapter** (built: `crates/partex-core/src/machine.rs`, run by
`PARTEX_MACHINE=1` through `crates/partex-cli/src/machinehost.rs`). TeX
is a real `Machine`, built and rebuilt by `partex_incr::Build`:

| `Machine` | TeX (`TexMachine`) |
|---|---|
| `Cell`, `index` | `MCell`: `Rest` (index 0), `Glyphs` (1), `Classes` (2), `Eqtb(p)` (p + 3); unindexed `Line(file, k)`, one line of an input file, `File(name)` for files read whole, `Sealed(key)` for a sealed line's contents, `Written(id)` for a `\write` file; `Link(p)` (a hash slot's link), `Name(chars)` (where a name is), `Font(slot)`, `FontName(name)`, `FontExpand(base, ratio)` and the accumulators `FontOrder`, `Dests`, `Numbering` (§7.0, "A first use moved") |
| `Value`, version | `Eqtb`: the word and what it names, exported and imported by content, versioned by `cell_content`; written values are lazy (a snapshot and `p`, exported only if a recorder asks). `Rest`: a snapshot of the whole engine (a checkpoint, §5.3), versioned by the memoized `rest_hash` (below). `Glyphs`: the characters used per font (bitsets). `Line`: the line's bytes |
| `Recorder` | `CellTracker`: first reads and writes of eqtb words per interval, in relaxed-atomic bitsets and logs, plus the log of input lines read; reads are reported at the cut (`prepare_cut`), versioned from the region's entry snapshot, not per read |
| `Boundary`, `at` | before a command read from a file (not from a token list), named by a hash of the input position |
| `step` | main control up to the next such command (`set_stop_at_candidate`, `resume`), with its cost; the first also starts the job and reports the format's eqtb as writes |
| `Effect` | the engine's `Effect`s (§7.6), forwarded; `render` is canonical (bytes and xrefs, no marks) so the sanitizer's comparison does not depend on how a run cut them |
| `digest` | the state hash and the host's digest |
| `seek` | the position is in `Rest`, which the replayed region wrote; a snapshot is taken (the next region's entry) |
| `accumulates`, `combine` | `Glyphs` and `Written`: every region's write is applied in replay (a union, an append), and composed guesses combine them |
| `replay_exit` | a clean span becomes the old snapshot at its end, patched with D and with the cells that changed since the snapshot was taken; accumulating cells keep their value plus the span's writes |
| `Clone` | `Tex::snapshot` (`Send + Sync`, §5.3) |

**`Rest`, hashed incrementally.** Everything but eqtb, the glyphs used
and the input files' text (input stack, nest and lists, page builder,
save stack, PDF writer, printing state) is one cell; the hash table, the
string pool and the fonts have left it for cells of their own (§7.0, "A
first use moved"). Its
version hashes the state (`state_hash` in the `Rest` scope) minus what
cells own (eqtb words, `xeq_level`, the PDF fonts' used characters) and
scratch (`dig`, a stale `def_ref`). The hash is a Merkle hash over what
snapshots already share (`hashmemo.rs`): each `JVec` chunk, `Flat`
segment, `Arc<Font>` and hyphenation table is hashed once and memoized
by its `Arc`'s address (the memo holds the `Arc`, so the address is not
reused; unused entries are swept every 64 snapshots). A region start
hashes only the chunks written since the last one: 21 ms became about
1 ms on the course. An open input file enters as (file, lines read), not
by its remaining bytes: its lines are `Line` cells, read only by the
regions that read them, so an edit to line 400 of a chapter invalidates
the regions that read line 400, not every region while the chapter is
open. The per-level input and condition stacks enter only up to their
live depth. `set(Rest)` takes the snapshot but keeps the target's eqtb
and glyphs, and rebases an edited open file to the same line
(`restore_rest`).

**Glyphs as an accumulating cell.** Which characters of each font the
document used (for subsetting at the end) grows in every region; inside
`Rest` it made every later `Rest` version differ after an edit that
changed a glyph set, and early cutoff never came. It is its own cell,
written as the set a region added and read only by font subsetting at
the end: the runtime applies every write of an accumulating cell in
replay (`Machine::accumulates`) and unions them in composed guesses
(`Machine::combine`).

**Regions.** A cold build cuts coarse regions (`PARTEX_MACHINE_GRAIN`,
131072 commands: a cut costs a snapshot and a hash, so fewer is faster);
a rebuild re-runs a dirty region at the fine grain
(`PARTEX_MACHINE_FINE_GRAIN`, 1024), so early cutoff is found soon after
the edit's effect dies out. Replay between dirty regions is lazy
(pending writes applied when the next region runs).

**Refine while idle.** A coarse cold region makes the first edit re-run
up to 131072 commands. `Build::refine` re-runs, in parallel, every
region costlier than four fine grains. Each region starts from its
predecessor's exit snapshot, with what the regions before it added to
accumulating cells, and is re-cut at the fine grain. A region is spliced
in only if it ends where it did, at the same cost and with the same
write versions; a position met twice cannot stop it early. It is opt-in
(`PARTEX_MACHINE_REFINE=1`): on the course, 240 regions become 4589, in
8.2 s on 24 threads (58 s of sequential work, 7.1×), but a fine region
keeps a snapshot, and a whole document of them peaked at 11.6 GB. Without
it, the first edit in a coarse region re-runs it finely (0.25–0.7 s on
the course), and later edits there take 88 ms. `PARTEX_MACHINE_SANITIZE=1`
also checks that the refined regions replay to the same state.

**Memory.** A snapshot is a clone of the engine that shares what did
not change since the one before. What it used to copy although nothing
changed, and what it copies now:

| Part | Copied before | Now |
|---|---|---|
| Token lists | each list whose reference count changed (a macro expanded) | only lists that differ from the last snapshot; unchanged tokens shared |
| eqtb, hash, save stack | each 512-word chunk written | only chunks that differ (a group's local assignments undone are not) |
| Node lists (page, current list, enclosing levels) | whole (a box built over pages: 14 000 nodes) | 64-node chunks, shared where equal |
| State hash memo | the whole memo, keeping replaced chunks alive | not at all (scratch: the running engine's) |
| `\write` files | each file appended to, whole | 4 KB pieces, shared |

A trace's cell is 24 bytes and a value 32 (the sealed-line key split in
halves, the file value boxed). The backedge and writer indexes keep a
sorted vector of region keys per cell, the writers without versions (the
trace holds them). After a rebuild, `Build::coarsen` merges runs of fine
regions no rebuild recorded in the last two (`PARTEX_MACHINE_COARSEN`,
0 turns it off) back into regions of the grain, by composition; a later
edit there re-runs one coarse region finely again. The course, peak RSS:

| | Before | Now |
|---|---|---|
| Plain run | 246 MB | 246 MB |
| Machine cold | 1.43 GB | 913 MB |
| Cold, then indexed | 1.80 GB | 1.08 GB |
| After 20 edits in 20 places | 2.48 GB | 1.32 GB (≈400 regions) |
| After 21 edits in one place | | 1.16 GB |
| Cold, refined (old default) | 11.6 GB | opt-in |

Output after the 20 edits is byte-identical to a plain build of the
edited sources (PDF, log, aux, toc, out). What is left: the traces'
guards and writes (1.7 M and 1.1 M on the course, about 150 MB), the
indexes (about 150 MB), one region's own chunks each (≈1.5 MB), and
about 280 MB with a single region (the format kept, the region's writes).
Twice a plain run needs the eqtb guards and writes of a region kept as
ranges or bitmaps rather than one entry per word.

**Converging after an edit.** The probe (`PARTEX_MACHINE_PARTS=4`) runs
the edited job and compares it with the old run at every boundary. For a
one-word edit in chapter 15:

| Stage | What differs at region boundaries | Regions until equal |
|---|---|---|
| before | the page builder for 5 regions, then the PDF writer for 14 more (a link's `/Rect` waits in the object stream until the stream is written) | 19 |
| symbolic object streams | the page builder only (the page's skeleton is equal) | 5 |
| sealed lines, a boundary before `\shipout` | only the sealed lines the ship region reads | 0 |

- *Symbolic object streams* (`PARTEX_MACHINE_OBJSTM=0` turns them off):
  an object that goes into an object stream becomes output
  (`ObjStmStart`, `ObjStmBytes`), and the stream is an `ObjStm` effect
  that the link renders and compresses as pdfTeX would. The bytes
  waiting in the stream leave the state. So does where each written
  object went. The xref's entry for an object in a stream is
  `XEntry::Placed`, filled in by the link from the effects, and the
  object table hashes a written object as written.
- *Sealed lines* (`seal.rs`, `PARTEX_MACHINE_SEAL=0`): the line breaker
  moves each line box's glue setting and list into a table of cells
  (`Sealed(key)`). The key hashes the paragraph's file, line and
  occurrence with the line's index. The box in the page keeps only its
  dimensions and the key. A word changed without changing a line's
  dimensions leaves the page, and so `Rest`, equal. Only code that looks
  inside a line opens it, and that is a read of its cell: shipping out,
  `\unhbox`, display math and `\leftmarginkern`.
- *A boundary before `\shipout`*: main control stops before `\shipout`,
  so shipping a page is a small region of its own. That region is the
  one that reads the sealed lines. A region before it that shipped the
  page would have to re-run with every sealed-line change.
- *`\write` files as accumulating cells* (`Written(id)`): a region's value
  is what it appended. A file read back (LaTeX's `.aux` at
  `\end{document}`) is read at its entry contents, even after the region
  appended to it. It shadows the input file of the same name: its lines
  are not `Line` cells, and its contents are `Rest`'s.
- *Remembered skips under tracking* (`Classes`, `PARTEX_MACHINE_SKIPS=0`):
  a remembered `pass_text` skip depends on every control sequence's
  class to a skip. That is one cell, versioned by an XOR of per-sequence
  hashes that `note_meaning` updates incrementally.
- *Stale exit snapshots*: a clean span replays as the old snapshot at
  its end, patched with D (`replay_exit`). A snapshot recorded before an
  earlier rebuild holds that run's values of cells that changed since.
  Each trace records the rebuild that made it (`born`), and each cell
  records the last rebuild in which it differed. Those cells are patched
  too, from the span's last write or the span's entry. This was found
  when a sealed line from two edits back came back.
- *Stale accumulator versions*: an accumulating cell's write adds the
  same value in any run, but its version is the whole value after it, so
  a region kept from before the last rebuild in which that value
  differed holds another run's version. After a re-executed span, such a
  version is not compared against: the cell stays in D until a reader's
  guard settles it (`known_old_version_before`), and coarsening does not
  compose a guard from it. Found on a glyph added, then taken back
  together with an edit further on: the font subset kept the glyph. The
  stub now has an accumulator (`mark`, `fonts`), and the property tests
  take edits back.
- *Accumulators that catch up*: a region dirty only because an
  accumulating cell differs runs only if its guard still fails once the
  replay reaches it. The glyphs a `\ref` edit adds usually occur earlier,
  so the font writing at the end stays clean.
- *Derived cells* (Task 14e): a derived cell asks a question about an
  accumulating cell, its source (`Machine::derived`), for example "the
  final number of object `k`" about the numbering. `get` answers it from
  the state; no region writes it, and setting it does nothing. A region
  that only asked questions about a source guards the answers, and also
  keeps its guard on the source, which is how a rebuild finds it when
  the source differs. A region that reads the source whole has no
  derived guards on it.

  The rule: the region is dirty through the source, and only through
  accumulating cells. It is replayed to its entry, as for an accumulator
  that may catch up. If the source's own guard fails there, the region is
  still clean if it has derived guards on that source and every one of
  them holds against `get` at that entry. The source stays in D for later
  readers. The region's source guard takes the value the source has now,
  so every guard keeps its real version, which is the sanitizer's chain
  check. A derived guard therefore can only make a region cleaner than
  its source guard would, never dirtier.

  Composition is conservative. The second region's questions about a
  source the first added to are dropped, and the composed region reads
  that source whole, guarded at the composed entry. The composed region
  keeps questions only if every part that reads the source asked only
  questions. `build::Config::derived_guards` turns the rule off. The stub
  asks `has g`, whether glyph `g` was used, and the property tests cover
  a question that holds (the region replays), one that fails (it runs),
  composition over a writer, and replay of every guard.
- *Rebuild bookkeeping*: restoring a snapshot takes the replaced engine's
  unchanged token lists, by chunk (`TokStore::thaw_from`,
  `PARTEX_MACHINE_THAW=0`). A splice with as many regions as before keeps
  their keys and updates the indexes by difference. Replaced regions and
  states are garbage that the driver drops while it waits.
- *Memos* (`PARTEX_MACHINE_MEMO=0`): Type 1 subsetting and deflate are
  memoized by content in the host. `\pdfglyphtounicode`'s table and
  virtual fonts are hashed by digest.

**The host** (`MachineHost`) is `Send`: the native host behind
`Arc<Mutex<…>>`, input files served from a map shared by all clones,
with their line starts cached (`CellHost::lines`), `\write` files kept
in the host and found again by `./name` as kpathsea names them (the job
reads them back, so they are state), time and epoch fixed. The files are
written once, from the linked effects (§7.6).

**Proof.** `PARTEX_MACHINE=1` writes byte-identical PDF, log and aux on
all 26 e2e documents and the 295-page course (the log's memory
statistic masked). `PARTEX_MACHINE_EDIT=path|from|to[||…]` rebuilds
after one or more edits, and the output equals a plain run of the edited
files. The runtime's sanitizer (`PARTEX_MACHINE_SANITIZE=1`) passes.
What it found: `dig` scratch and `xeq_level` in the hash, the format's
eqtb written untracked, `eq_destroy` on integer words in
`import_cells`, read versions taken at the cut, stale byte counts in
reused regions' text (now a `Length` effect), token lists aliased
across cells in a whole-state digest (the digest is now per cell), and
glyph marks made at the end that the tracker did not see.

Measured before this round (release build, one machine, 24 threads):

| | small book (44 pages) | course (295 pages) |
|---|---|---|
| plain run | 8.9 s | 50–55 s |
| `-watch` today: cold / after a one-word edit | — | 71.8 s / 26.2 s |
| machine cold, full hash per region start (before) | 32.3 s | — |
| machine cold, memoized hash, line cells | 12.8 s | 65–70 s |
| machine rebuild, one-word edit, whole-file cells (before) | 9.6 s | 35.6 s |
| machine rebuild, one-word edit | 1.0 s | 3.3–3.9 s |
| machine rebuild, four edits in four chapters | — | 5.7 s |

The course's one-word edit re-runs 25 regions (2.6 M commands): the
page breaks move up to the chapter's end, which is TeX's work, not the
runtime's (in the profile, snapshots, cuts and replay are under 2.5% of
the rebuild). Cold, the machine is 1.2–1.3× a plain run: the tracker's
hooks, `pass_text` without its skip cache (disabled while reads are
tracked) and the snapshots (`rest_hash_memo` is 4% of a cold build).
That is short of the 10% target, so machine mode is not the default;
its rebuild is 7× faster than `-watch`'s. `PARTEX_MACHINE_PARTS=1|2|3`
dumps per-region diagnostics.

Measured now, on the course. Rebuild times are steady state, and every
output is identical to a plain run of the edited files:

| | before | now |
|---|---|---|
| machine cold, cycles against a plain run (150 G) | +10.7% (166 G) | +6.0% (159 G), +8.6% task clock |
| refine while idle | — | 8.2 s |
| one-word edit (`costly`/`dear`) | 3.3–3.9 s | 86–95 ms |
| one-word edit, first after a cold build | — | 0.13 s |
| edit that adds a glyph (`\ae`), and its undo | — | 0.13 s, 0.10 s |
| `\ref` edit | 0.33 s | 0.14 s |
| `\label` edit | 31–34 s | 0.15–0.16 s |
| a paragraph added (page breaks move to the chapter's end) | 29 s (1608 of 4590 regions) | 2.7–2.9 s (11 of 399 regions); undo, redo 2.4 s |
| a `\Cref` added (a link: every later object renumbered) | 27 s (1605 of 1811 regions) | 1.9–2.0 s (7 regions) |
| an `\href` added | — | 1.8 s (7 regions) |
| a `\label` added | 25 s (86 regions: `\@savsf`, below) | 3.3 s (11 regions) |
| a footnote added (the document's first before chapter 22) | — | 27.6–30 s (every later region, below) |

The paragraph-added edit's 2.7 s is the chapter's tail, whose page
breaks really move: with fine regions (`PARTEX_MACHINE_REFINE=1`) the
rebuild still runs 2.31 M commands (91 fine regions, 2.1 s), against
2.49 M at the coarse grain, so coarse regions add 7%. The rest is TeX
typesetting the pages again, until the `\clearpage` at the chapter's
end, where the state is the old run's again.

The one-word edit costs about 38 ms re-running three regions (the
paragraph, the page's ship region) and 43 ms replaying the rest (two
snapshot restores). A `\ref` re-runs its paragraph (34 k commands). A
`\label` re-runs the `\end{document}` tail, which reads the `.aux` back.
The rest is restoring snapshots and taking new ones: cloning the engine,
thawing its flat arrays, and hashing the PDF state.

**Numbering histories.** Adding a paragraph moves page breaks to the
chapter's end. Before, the state never equalled the old run's again
after that, and every later region re-ran. The probe
(`PARTEX_MACHINE_EDIT`, `converge:` lines) showed what differed after
the chapter: eqtb, the page builder and the lists were equal, but

- the string pool held the same strings in another order (string 86903
  is `FiraSans-Bold-osf-t1--base+8` in one run and `tcolorboxpage@140`
  in the other), so the hash table's names pointed elsewhere;
- PDF objects were numbered in another order (hyperref's `page.153` and
  `paragraph*.265` destinations swap object numbers), and so was the
  list of destination names.

These are numbering histories, not values TeX branches on. Two changes
take them out of `Rest`, with no renumbering at the link:

- *Strings by content* (`Tex::canon_strings`, `PARTEX_MACHINE_STRINGS=0`
  turns it off). `Rest` hashes the format's strings as they are and the
  run's as a multiset (a sum of their hashes, plus the string being
  built). Every reference to a string the run made is hashed by the
  string's characters: the hash table's names (memoized per shared
  chunk, `HashMemo::names`), font names, the input stack's names, the
  file name stacks, the hyphenation exceptions, `\jobname` and the other
  file names. Two runs whose strings differ only in number are in the
  same state. What TeX observes of a number (`\fontname` prints
  characters, not numbers) comes out the same.
- *Object table cells* (`pdf/objtab.rs`: `ObjLog`,
  `PARTEX_MACHINE_OBJCELLS=0` turns it off). Each entry of the object
  table (`MCell::Obj(k)`), each lookup tree entry (`MCell::Tree(type,
  id)`) and each destination name's place (`MCell::Dest(i)`) is a cell.
  `Rest` keeps only the table's length, `obj_ptr`, the list heads and the
  number of destination names. The table logs its reads (`get`, `find`,
  the destination names read whole at the end) and writes (`get_mut`,
  `create`) into a relaxed log drained after each step. A region's
  guards are the entries it read before writing them, at their values
  at the entry. With symbolic object streams, an entry's version leaves
  out where a written object went, but keeps whether it went to a
  stream or straight to the file (the xref's `Placed` or `Byte`).
  After the chapter the two runs differ in 190 entries, 12 destination
  names and 17 tree entries, and only the regions that read those
  re-run: the chapter's pages, and the end (which writes the
  destination names and the xref).

Each change alone leaves `Rest` unequal. Together, the paragraph-added
edit re-runs 11 regions (2.7 s: the chapter's pages at the fine grain)
instead of 1608, and the output is identical to a plain run of the
edited file (and after undo and redo). §7.5's holes are not needed
for this edit: a reused region keeps its old object numbers because
every entry it reads has the same value in both runs.

An edit that adds an object (a `\Cref` or an `\href` makes a link
annotation) shifted every later object's number, and every later object
stream's boundary: pdfTeX numbers objects in allocation order, object
streams included, and a stream takes the next number when its first
object is written. Every later region re-ran (27 s). *Virtual object
numbers* (§7.6) take numbering out of the state: the same edit re-runs
7 regions (2.0 s), identical output.

**Soft reads of the save stack** (`Tracker::soft_read`,
`PARTEX_MACHINE_SOFTREADS=0` turns them off). A `\label` added set
`\@savsf` (LaTeX's saved space factor, a scratch register `\@bsphack`
sets) at the outer level to another value, and from then on every local
assignment to it inside a group read it: the old value may be saved
(and e-TeX compares it with the new one). 86 regions re-ran (25 s),
though none observed it. Either way the value inside the group is the
new one and after it the old one, so the tracker now counts such a read
as *soft*: at the group's end it is dropped, and a restore of the value
the save took at the region's entry leaves the location untouched (no
write either). It becomes a read only if the group is still open when
the region ends (the save stack holds the value), if the value is read
in the meantime, or if `\tracingassigns` or `\tracingrestores` would
print it. The `\label` edit re-runs 11 regions (3.3 s); everything else
is unchanged, output identical.

**A first use moved: names and fonts as cells.** A footnote added before
the first of the course (chapter 15; the edit measured on the course's
copy of 27 September, which has none) cost 27–30 s: every later region
re-ran. The probe (`PARTEX_MACHINE_PARTS=4`) and `PARTS=7` showed three
lasting differences, none of which a later region observes:

- *A control sequence and strings more.* pdfcol makes
  `pdfcolfoot@current` at the first footnote, which the old run never
  does, and so the hash table and the string pool differ for good.
  With `Tex::name_cells` (`PARTEX_MACHINE_NAMES=0` turns it off) a
  control sequence's name is part of its eqtb cell's value, by its
  characters (`CellValue::name`; `Tex::mcell_content` is the version),
  a hash slot's link is a cell of its own (`MCell::Link`, what a lookup
  that enters a name walks), and a lookup that finds a name missing, or
  enters it, reads where that name is (`MCell::Name`, keyed by the
  characters; §259). The tracker gives `Cell::Hash(p)` the byte of
  `Eqtb(p)`, and each link a byte of its own. `Rest` then leaves out the
  table, the count of `hash_extra` places taken (with names placed by
  name, in INITEX not: a format's names must stay packed for §260 of the
  run that loads it), and the run's strings: they matter only through
  what holds them, which `Rest` hashes by characters. A value set imports
  a name as a new string; which number it has TeX never observes. That
  was also a fixed cost of every cut: hashing every string the run made
  (7–9% of a rebuild that re-runs everything).
- *Font numbers.* The footnote's paragraph needs two of pdfTeX's
  auto-expanded fonts (microtype's expansion: `…-12`, `…+18`) that the
  old run never made, and a font takes tex.web's next number (§576), so
  the ~190 fonts loaded after it, mostly expanded ones, were numbered +2:
  the font table, the font identifiers' eqtb words, every character node
  and the PDF writer's per-font state differed for good. Fonts now sit at
  *slots* (`FontArrays::place`), apart from their numbers
  (`FontArrays::number`, the order of loading: pdfTeX's `/F` name and
  DVI's font number). A machine's host gives slots from a registry by
  identity shared by every run of a build and saved with it
  (`Host::font_slot`; `MachineHost`'s `FontSlots`,
  `PARTEX_MACHINE_FONTSLOTS=0` turns it off): a TFM font by its file's
  contents, name, area and size, an expanded one by its base and ratio, a
  copy by its source. A font never seen takes tex.web's next number or
  the slot after the last one given, so a cold build's slots are pdfTeX's
  numbers, and a font keeps its slot however many fonts came before it.
  With `Tex::font_cells` (`PARTEX_MACHINE_FONTCELLS=0`) each slot is a
  cell (`MCell::Font`: identity, parameters, `\hyphenchar`, codes,
  expansion, and what the PDF writer keeps of it; a slot not loaded is
  the same whatever it holds), read where the font's state is read (the
  `Cell::Font` hooks, `pdf_font`) and written where it changes. A font's
  metrics follow from its identity, so reading them needs no cell (fonts
  whose tags changed, `\pdfnoligatures` and `\tagcode`, stay in `Rest`).
  The fonts by name (`MCell::FontName`: what §1260's search and
  `tfm_lookup` find) and pdfTeX's expanded fonts by base and ratio
  (`MCell::FontExpand`; its `pdf_font_elink` chain kept as a map, what it
  is searched for) are cells, logged where searched and made. The order
  of loading accumulates (`MCell::FontOrder`), read where a number is
  observed: DVI's font numbers, §580's last font, the end's walk through
  the fonts.
- *The numbers in the page streams.* pdfTeX prints a font's number in
  `/F` names inside compressed content streams, so a page using a font
  loaded after the change would observe the order. `/F` names are
  relocations (`Effect::FontRef`, `ObjStmFontRef`), and every stream
  `pdf_begin_stream` begins is compressed by the link
  (`Effect::Deflate`, with `Effect::StreamLength` for its `/Length`
  blanks), after it writes in the numbers (`Effect::FontLoad` gives
  them). The engine counts each stream as compressed with the digits it
  had, for "Output written"; the host's deflate memo hands that
  compression to the link when the digits are the same, as in a cold
  build.
- *Destination names* are one accumulating cell (`MCell::Dests`), read
  at the end, which writes them sorted and counts them.

Measured on the course: the footnote 27.2 s → 3.2 s (11 of 282 regions
re-run: the paragraph; a counter above 255 and `\@gtempa`, which the
footnote left different for good and a few regions read; the end), and
removing it again 0.88 s; a `\Cref` added 0.81 s, a `\label` 1.34 s;
every output identical to a plain run of the edited files. Affine holes
(§7.5) were not needed: the counter's readers are few.

**Comparing words that name objects.** `Tex::eqtb_differences` (which
`restore_rest` uses to keep this state's eqtb words) took two words with
the same bits to be the same, but a word names a token list, glue, box
or shape by its store's number, and two runs' stores give one number to
different objects: once, with boundaries placed elsewhere, a `\linespread`
rebuild kept the old run's `\baselinestretch` (list 21663 holding `1.04`
in one store, `1.05` in the other) and its output differed. In chunks the
two states do not share, such words are compared by content.

Since the objects became the entries' values (7.17.12, `objs.rs`), an
entry is its word and the object beside it (`eqtb_obj`), and the word
no longer names the object: its `equiv` is null, or a macro's
`\protected` flag. Two entries with the same word may hold different
lists, and a chunk of words two states share says nothing of the
objects beside it (a `\def` of a new body writes the word back as it
was, and the snapshot shares the chunk again). So the words are
compared by chunk and the objects apart, every one: an object that is
the very value in both states (the same `Arc`, or equal glue) is the
same without a look, any other pair by content (`Tex::cell_content`,
the cell's version). Comparing only the words in chunks not shared,
and only words whose `equiv` was positive, kept a snapshot's
`\@gtempa` over the state's on a restore, and the machine's
sanitizer failed on the course's footnote edit.

The eqtb prototype (`adapter.rs`) remains: `apply_writes` is
`Trace::apply` for eqtb cells, and its round trip holds on all 1068 e2e
regions.

### 7.1 State tracking

**Cells.** A cell is a unit of observable state with a hashable value.
`Cell` has `Eqtb`, `Hash`, `Font(f)`, `FontTable`, `Read(n)`, `Out(n)` and
`Random` today (the next paragraph says what each covers); the table is
the target. Cells are added one at a time, each measured by the
recordings it stops abandoning.

- `Font(f)` is what can change about a loaded font (its `\fontdimen`s,
  `\hyphenchar`, `\skewchar`, pdfTeX's character codes and expansion
  links), read by `font_param`, the interword glue, line breaking's
  hyphen character and expansion, math (every family font of a formula)
  and the font primitives. `FontTable` is which fonts are loaded:
  `\font` looks it up, loading writes it. A font's metrics never change
  once loaded, so packaging and ship-out read them by id without a hook;
  the ids themselves come from eqtb reads.
- `Read(n)` is `\read` stream `n` (open, and how far read): `\openin`,
  `\closein`, `\ifeof`, `\read`. `Out(n)` is whether `\write` stream
  `n` is open: `\openout`, `\closeout`, a `\write` choosing between the
  file and the log, `\showstream`, the end of the job.
- `Random` is pdfTeX's generator: `\pdfuniformdeviate`,
  `\pdfnormaldeviate`, `\pdfsetrandomseed`, `\pdfrandomseed`.
- **Assignments read what they replace when it matters.** Inside a group
  a local assignment saves the old value (restored at the group's end)
  and its level decides whether; e-TeX skips an assignment of the value
  already there, which `\tracingassigns` shows. So `eq_define` and
  `eq_word_define` report a read of their target when `cur_level > 1` or
  `\tracingassigns > 0`, and `unsave` reads each restored cell (whether
  its current value is global decides). At level one without tracing
  e-TeX's check is skipped: the outcome is the new value whatever the old
  one was (the check could only keep an equal value; for a control
  sequence never defined, at level zero instead of one, which nothing
  prints). Before this, a region that assigned a cell locally looked like
  it wrote it without depending on it, and read-set cutoff could keep the
  previous build's saved value (a `\refstepcounter` inside a figure
  saves `\@currentlabel` and restores it at `\end{figure}`).
`\time`, `\day`, `\month` and `\year` are int parameters, so `Eqtb`
cells, constant within a run.

| Cell | Examples | Notes |
|---|---|---|
| `Eqtb(p)` | meaning, catcode, register, parameter, `\time` | level is part of the value |
| `Hash(name)` | whether `\foo` exists | `\csname` of an unknown name is a hidden local write |
| `Font(f)`, `FontTable` | `\fontdimen`, `\hyphenchar`, loaded fonts, protrusion and expansion codes | built, per font (fields later if it pays) |
| `Nest(field)` | mode, `\prevdepth`, `\spacefactor`, `\prevgraf`, the current list | per semantic level |
| `SaveStack` | group level and code, `\aftergroup` tokens | boundaries keep it unchanged |
| `Input` | pending token lists, backed-up tokens, `\afterassignment`, `align_state` | "what the interpreter reads next" |
| `Line(file, line)` | a source line's tokens (§7.2), `\inputlineno` | |
| `File(name)` | content hash of a file the host served, or its absence | cross-run invalidation |
| `Host(query)` | `\pdffilemoddate`, `\pdffilesize`, `\pdfmdfivesum` | the host's answer |
| `Read(n)`, `Out(n)` | `\openin` and `\openout` streams open or closed, how far read | built; writes are effects |
| `Page` | page-builder state, `output_active`, marks, inserts | `\pagetotal`, `\outputpenalty`, … |
| `Pdf(field)` | resources, destinations, annotations, used glyphs | numbering is not state (§7.6) |
| `Misc` | `\lastskip`, `\lastpenalty`, `\badness`, interaction, `\deadcycles` | |
| `Random` | the generator's state | built; its initial seed comes from the clock (impure across builds unless `SOURCE_DATE_EPOCH`) |
| impure | `\pdfelapsedtime`, `\write18`, terminal reads | make a trace uncacheable |

**Counters** are the one exception to plain cells: state that regions only
increment, such as PDF object numbers or `\c@page`. A region records a
delta, and only an explicit read of the value (`\pdflastobj`, `\the`) is a
cell read; with holes (§7.5) even that read can stay symbolic. Nothing
tracks memory or stack statistics: they are not output (§0.3).

**Versions are content.** A write makes a new version of its cell, named
by the hash of the value: dynamic SSA whose version names are hashes,
with hash-consing (global value numbering) making two writes of the same
value the same version, so early cutoff is automatic. For a word-valued
cell the version is the word; for a cell that names an object (a macro, a
`\toks` list, a box, a glue spec, a paragraph shape) it is the object's
content hash with its type. An object stored in a cell is never mutated
afterwards, so it acquires its hash when stored: `intern` in `tok.rs` does
this for `\def` bodies (today a 64-bit FNV-1a) and extends to every
cell-stored list; boxes carry their Merkle hash (§5.2). A recorded write
stores what is needed to reinstate it: `(cmd, Arc<[Token]>, protected)`
for a macro, the `Arc<BoxNode>` for a box, the spec for glue. Validation
compares hashes, never ids.

**Stable hashing.** A fixed 128-bit hash, independent of platform and run
(never `SipHasher`). `glue_set` hashes by `f64::to_bits`: TeX computes it
deterministically, so equal boxes have equal bits. Fonts hash by identity
(TFM content hash, size, changed `\fontdimen`s) through a context that
maps run-local `FontId`s. Tokens hash by name, lists by content.

**The tracker.** The core reads and writes state only through accessors
that call a `Tracker` (`crates/partex-core/src/track.rs`), a generic
parameter of `Tex`, never `dyn`. The reference build uses `Untracked`,
whose hooks are empty and inline away. Every consumer of state
observations is a tracker: the session's read sets (`intervals.rs`), the
`deps` analysis, the TeX-level profiler, memoized calls, region recording,
speculation and the guards of compiled expansion. Hook families sit
behind `const`s on the trait, so a tracker pays only for what it uses:

| Family | Const | Hooks | Called from |
|---|---|---|---|
| state | `VALUES` | `read(cell)`, `write(cell)`, `read_value`, `write_value`, `retract(cell)` | the eqtb, hash, font, nest, page and save-stack accessors |
| lines | `LINES` | `lines_open`, `line_start`, `line_end`, `line_shown` | file opens, `next_line`, `\read`, `show_context` (§7.2) |
| input | `INPUT` | `fetched`, `list_began`, `list_ended`, `param_pushed` | `get_next`, `begin_token_list`, `end_token_list` |
| control | `CONTROL` | `command`, `expand`, `group_begin`, `group_end`, `effect(kind)`, `opaque(what)` | `main_control`, `expand`, grouping, every print and `\write`, every access to state that is not yet a cell |
| profile | `PROFILE` | `macro_call(cs)`, `macro_args(cs, hash)` | `macro_call` |
| structure | always | `file(depth, name)`, `page()`, `blank_line()` | file open and close, ship-out, `get_next` |

Hooks take `&self`; a recording tracker keeps its log behind interior
mutability. A tracker returns plans; it never touches the engine.

**Purity is observed at the access, not declared per command.** Every
access to state that is not yet a cell calls `opaque(what)`; a region that
saw `opaque` is not reusable, with `what` reported. Every effect calls
`effect(kind)`; until effects are values (§7.6) an effect makes the region
opaque, afterwards it is recorded and replayed in order. As cells are
added the opaque set shrinks with no change to region code. Coverage is
guaranteed by construction only once semantic state is private to its
accessors (the state split, §10); until then the defense is the sanitizer
(§7.12).

**Today** `memo.rs` bypasses the tracker: a field of `Tex` with a runtime
flag tested on every token in `get_next`, its own read stamps, raw
`MemoryWord` values with token lists by id and generation, and purity by
three whitelists (`memo_command_ok`, `memo_expand_ok`,
`memo_internal_ok`). Its tables cannot persist (`save_state` refuses a
state with the memo on). The unification: the memo call sites become
tracker calls; values become content hashes; `opaque` replaces the
whitelists; the memo's `Entry` becomes the call-grain instance of the
region trace (§7.3).

**Input is a cell.** Tokens fetched at or below a region's input level are
its consumed input and part of its key; tokens left pending above that
level at exit are its pending output, pushed back on replay.

**Measured.** The session's tracker (exposed reads and writes per
interval between checkpoints, as bit sets over eqtb and hash locations)
costs a watch build about 12%; `PARTEX_READSETS=0` turns it off.

### 7.2 Source text as values

What a line of a file defines is the tokens TeX reads from it, not its
bytes. The line's value is `tokens(bytes, catcode table,
\endlinechar)`, computed by TeX's own tokenizer (§343–357) from state
`new_line`. It is memoized like CPython's `.pyc` files, keyed by the
source and, here, the catcode table. An edit whose lines produce the same
tokens under the codes they were read with creates no new version, so
nothing downstream is dirty: added spaces, reflowed text within a line,
comments, trailing whitespace, and anything else the tokenizer cannot
see. This is not a special case: under `\obeyspaces` or in verbatim the
same edit gives other tokens and is visible.

The engine reports, per line of an `\input` file, where it began, the
tracker's *catcode generation* when it was read and when it was done (the
next line of the file read, or the file closed), and the codes then in
force. The generation counts writes to category codes and
`\endlinechar` in every engine of a session and is never reset, so a line
begun and ended at the same count was read under one table whichever
runs came between. A changed line is invisible if every reading of it:

- was read whole under one table (a line that changes catcodes while it
  is read, such as `\makeatletter`, or one that `\input`s a file that
  does, is visible whenever it changes);
- gives the same tokens under that table, old text and new (`^^`
  notation and invalid characters count as different: TeX rewrites the
  buffer or stops there);
- was never shown in an error context (the context prints the bytes);
- and the file keeps its number of lines (`\inputlineno`, error
  locations).

Lines read by `\read` are always visible. A file's other consumers are
host reads, `Host(query)` cells: modern LaTeX's `\IfFileExists` runs
expl3's `\file_if_exist:n`, which calls `\pdffilesize` on every file it
inputs, and the size changes with any edit. Such a read is backdated at
the level of values: the interval between checkpoints that made it is run
again with the new file, and its end state compared with the previous
build's checkpoint, both with the file's positions moved to the new text.
If they agree, the consumer used the value only in ways that did not
matter (here, to test existence and the name) and the edit stays
invisible (Salsa's backdating, applied to one value).

When every change is invisible, a rebuild runs nothing: the new contents
replace the old in every checkpoint (read positions moved line by line;
a checkpoint whose buffer holds a changed line is dropped), in the
recorded line events and in the read log, and the outputs stand.
`PARTEX_TOKEN_DEPS=0` turns it off (no line events, no generation
count: the old rebuilds exactly).

The fast path refuses whenever it cannot be sure, and says why
(`PARTEX_WATCH_DEBUG`, `-v`). It refuses when the recorded intervals do
not cover the whole job one after another (a session loaded from disk,
whose events were not saved); when a changed file's old contents are held
at another address (a checkpoint or spliced events that got the file from
another lookup: they would neither be checked nor move; a lookup that
found a file unchanged is another file, whatever its contents); for a
changed line longer than 4096 bytes (the shared buffer could overflow
where it did not); and when a value-level rerun of a lookup's interval
reaches the next checkpoint in the same state but with other effects
(lookups, outputs, terminal, pages or diagnostics compared, not just the
state hash, whose scope leaves out what was already written). Files are
named by their contents' address, so the tracker keeps every file events
name alive until no event names it. The generation cache of codes is
forgotten at every interval (two engines can share a count and differ in
codes); a line in a checkpoint moved by `replace_input_before` keeps its
tokens followed only if its bytes are still there. A splice adopts the
new run's copies of the files open where the builds met, and a
successful rerun keeps the checkpoints just before and after the lookups,
so the next edit of the same file reruns only that stretch.

Measured (watch rebuild, the renderer's time; PDF and `.aux` of every
rebuild byte-identical to a cold `partex build` of the same sources; e2e
case `tokens` checks plain TeX edits against Knuth's TeX):

| edit | mini (23 pp.) on / off | course (295 pp.) on / off |
|---|---|---|
| spaces in a paragraph, first time | 147 / 144 ms | 2.65 / 2.98 s |
| the same file again | 35–72 / 92–151 ms | 0.26 / 3.17 s |
| comment only | 34 / 159 ms | 0.24 / 2.95 s |
| spaces after `\makeatletter` | 33 / 83 ms | refused: 2.37 / 2.34 s |
| text in a paragraph (visible) | 132–209 / 133–231 ms | 6.80 / 6.57 s |
| verbatim spaces (visible) | 151 / 132 ms | — |
| a `\ref` (visible) | 217 / 212 ms | 7.03 / 6.64 s |
| a `\label` (visible, 2 passes) | 423 / 420 ms | 118.8 / 117.6 s |

A first invisible edit of an `\input` file costs the rerun of the
interval with its `\IfFileExists` probe (up to the next kept checkpoint:
2.4 M commands, 2.5 s, in the course); later ones rerun only the stretch
around the probe. The preamble is probed before the first checkpoint, so
an edit there is refused. Refusals seen in these runs: lines under
`\obeyspaces` or verbatim, lines that change codes while read, lines an
error context shows, lines read by `\read`, lines added or removed, and
the preamble probe.

The same layer serves the cold path as a **token cache** (§7.13 step 1):
tokenized lines cached by (bytes, catcode table, start state), consumed
lazily so `loc` stays exact; a catcode change mid-line drops the rest of
the cached line.

### 7.3 Regions and traces

A region is the execution between two boundaries. Running it tracked
yields a trace, LuaJIT's recorder applied to TeX:

```text
RegionTrace {
  kind:     Call { def: hash, args: [Arc<[Token]>] } | Span { entry: Boundary }
  input:    { consumed: Arc<[Token]>, pending: Arc<[Token]>, align: i32 }
  guards:   [(Cell, Version)]          // exposed reads, first reads in order
  writes:   [(Cell, Content, Scope)]   // net effect; Global or Local(level)
  counters: [(Counter, delta)]
  holes:    [HoleUse]                  // unresolved values it used (§7.5)
  nodes:    Vec<Node>                  // its contribution, position-independent
  effects:  [Effect]                   // §7.6
  exit:     { boundary, group, mode, cond_depth }
  cost:     { tokens, time }
}
```

A trace is **replayed** where its guards hold: apply the writes and
counters, append the nodes, commit the effects. A failed guard is a side
exit: the region is executed again. A trace is not reusable if the region
read an impure cell (§7.1), consulted the terminal on an error or ran
`\dump`; that makes it a no-op for caching, never an error. Guard sets
stay small by recording first reads only, by covering cells unchanged
since the format was loaded with one `FormatEpoch` guard, and by interning
identical guard sets.

**Boundaries are found dynamically and hierarchically.** Two things are
kept apart: a *candidate* is a point that could be a boundary, cheap and
mostly syntactic; a *region* is a run of consecutive candidate intervals,
chosen from what was observed. All recording is at candidate grain;
choosing is composition over the records, so the choice changes between
passes and builds without rerunning anything.

- Candidates, cold: `\input` and `\include` edges, and every top-level
  blank line (`Tracker::blank_line`), which a worker assumes is read as
  `\par` at `scanner_status = normal` with only file levels on the stack.
  Warm: every candidate a previous build confirmed, paragraph ends in
  outer vertical mode, points where the page builder is empty with no
  output active and no held insertions (`\clearpage`), shipped pages, and
  output-routine invocations.
- A candidate interval's trace records first reads, net writes, counter
  deltas, nodes, effects, cost, and any `opaque`. A region's trace is the
  composition of its intervals: writes in order, guards the union less
  what an earlier interval wrote, counters summed, nodes and effects
  concatenated.
- A boundary's **cut** is the cells written before it and read after it,
  classified:

  | Class | Examples | Costs a cut? |
  |---|---|---|
  | dead scratch | `\@tempa`, `\pgf@x` written before read | no |
  | counter | `\c@page`, PDF object numbers | no: a delta, or a hole (§7.5) |
  | deferred layout | `\prevdepth`, the list's tail, `\pagetotal` | no: a hole resolved at commit (§7.5) |
  | stable definition | first-use `\csname` targets, styles | yes, same value every build |
  | volatile | any cell that differed here in a past pass or build | yes, weighted by how often |

- **The chooser** is a one-dimensional segmentation of the candidate
  sequence, a deterministic function of the records (§0.5), so two runs of
  the same input choose the same regions. It merges an interval into its
  neighbour while the region is below a minimum cost (a snapshot and a
  validation must be amortized), merges across a boundary whose volatile
  weight is high (a region invalid anyway gains nothing from a cut there),
  never exceeds a maximum cost (a wrong guess wastes the whole region),
  and merges an interval with an `opaque` read into its neighbours.
- **The hierarchy** is the same chooser run with larger minimum costs:
  paragraph, page, file, document, each a coarser cut of the same records.
  Macro calls are the leaves (below). Validation goes coarse first: a
  region whose guards hold is replayed in one step (a whole chapter at
  once); only a dirty region is descended into, and usually most of its
  children still hold (Adapton's nested demand; Incremental's nested
  binds).
- **The hierarchy adapts.** After each pass and build, validation says
  which cells mismatched at which boundary: a region that keeps failing
  splits; neighbours that are always clean merge; boundaries that failed
  on order-dependent values merge away; boundaries that were not clean
  leave the candidate set. Per-candidate statistics persist, keyed by a
  content anchor (the hash of the source around the point, not its line
  number, so an edit elsewhere does not orphan it). In a warm rebuild the
  chooser adds edit locality: paragraph grain near the edit, page grain
  further away, file grain far away, under a memory budget.

**The innermost level: memoized macro calls.** A call of a macro that
opens a group and hands its results out of it is a `Call` region keyed by
the macro, its definition and its argument tokens. Selection is by
definition identity, argument repetition and measured saving, as
`memo.rs` does today (recorded after `HOT` = 8 calls and on the second
sighting of a signature, the arguments plus the next 16 tokens; judged
after `WARMUP` = 32 recordings; a definition that keeps failing retried
every `RETRY` = 4096 calls). Measured on the PGF manual subset: 150k hits
saving 102M token reads for 54M recorded, 35.5 s against 35.4 s without:
at call grain the saving is eaten by bookkeeping, because most expensive
calls read state that changes between calls. Content-keyed intrinsics and
compiled definitions (§7.13) are the cheaper leaves.

**Back-end calls are pure leaves.** Packaging, line breaking, page
building, math and alignment are functions of explicit values: they never
read engine state. Everything they may read is in their parameter
structs, filled from eqtb by the front end, plus a small `Env` (for line
breaking: `\lccode`s, `\hyphenchar`s, patterns, exceptions, fonts). The
memo key is the stable hash of the arguments; reports (overfull boxes,
`\tracingparagraphs`) are part of the result and printed by the caller,
so a hit replays them in order. Targets by measured cost: line breaking
(17% of the woven tex.web run, 18,346 paragraphs at about 7 µs each) and
math lists.

**What the same mechanism covers.** Preamble images (the state at
`\begin{document}` is a region exit keyed by the preamble's reads, C++'s
precompiled headers for free); labels and references (`\r@foo` is a
macro cell, and only its readers rerun); unchanged TikZ pictures and
repeated formulas; "pure" files, whose purity is their recorded read and
write sets, not a classification.

**Measured: what a structural edit leaves different.** One `\subsection`
inserted early in the PGF subset: the eqtb differs in 7–8 cells
(hyperref's current anchor, `\@currentlabel`, `\@svsec`); after the next
section only two remain, pgf's resource list and a soft mask's name, both
holding PDF object numbers. The log's column differs until the next
newline. The PDF writer differs for good: named destinations shift and so
does every object number allocated after them. So once the anchor macros
catch up, the rest of such a rebuild differs only in PDF numbering, which
§7.6 takes out of the state.

### 7.4 Invalidation and propagation

**Target.** Every cell keeps backedges to the traces holding a guard on
it (Julia's method invalidation: redefining a method invalidates the
compiled code that called it). An edit changes source-line values
(§7.2) and host cells; their backedges give the dirty traces in
O(dependents), with no scanning. Propagation runs in program order, a
priority queue keyed by position (Incremental's height-ordered heap;
the order-maintenance structure of self-adjusting computation):

- a **clean** region is not run: its new exit is its old exit with the
  upstream differences it did not overwrite patched in
  (`old exit ⊕ D`), O(|D|) on persistent state (§5.3);
- a **dirty** region re-executes from its new entry; its new writes are
  compared with its old ones by version. Equal: propagation stops there
  (early cutoff, Salsa's backdating). Different: the backedges of the
  changed cells mark their readers dirty.

Propagation therefore replays clean regions *between* dirty ones and
stops per value, not once per build.

**Built so far** (`-watch`, `partex watch`, `-converge`), each step a
special case of the target and each measured:

1. **Resume.** Restart from the latest checkpoint the edit leaves valid:
   everything it read is unchanged.
2. **Early cutoff.** At each later checkpoint, compare the whole-engine
   state hash (§5.3) with the previous run's checkpoints at the same input
   position (same line, file level and input left, and the same rest of
   every open file). When they match, the rest of the run would repeat
   itself: stop, and splice in the previous run's outputs, terminal and
   lookups after that checkpoint. The DVI file's later pages are written
   again from their page IR by this run's writer, not copied (where TeX
   flushes its DVI buffer decides some bytes: §611's movement reuse,
   §601's `pop`). The previous run's later checkpoints are kept, moved to
   where their outputs now are, so the next edit can stop early too. In
   PDF mode the splice goes only as far as the previous build's last
   checkpoint (the xref holds every offset), whose writer positions are
   moved, and the job runs on from there and writes its end. What the PDF
   writer keeps only in order to write something (a written mark's place
   on the page) is dropped from the state; keeping it made an edit that
   moved one link never converge (17.2 s, now 4.1 s). Measured on the
   woven tex.web (536 pages): 45–105 ms for a one-word edit anywhere,
   against 650 ms fresh.
3. **Backdating.** A changed read before a checkpoint need not invalidate
   it. LaTeX probes every `\includeonly` file in the preamble
   (`\pdffilesize` through expl3), so an edit anywhere in a chapter
   looked like a change at the start. When the first invalid checkpoint
   is blocked only by reads of files closed again before it, the interval
   before it runs again with the files as they are now and the state
   reached is compared with the checkpoint's; if they match, those reads
   count as unchanged. A trial that does not match is where the rebuild
   goes on from.
4. **Read-set cutoff.** Equality is too strict: an edit of `\title` leaves
   `\@title` different for the rest of the document, though nothing
   after the title page reads it. Sessions record, per interval between
   due checkpoints, the exposed reads and the writes (`intervals.rs`). At
   a checkpoint at the previous build's position:
   - only cells one of the two builds wrote since the checkpoint resumed
     from can differ; those are compared one at a time, giving D;
   - if the rest of the previous build reads no cell of D before writing
     it, the rebuild stops there: the previous build's outputs are
     spliced in, and its later checkpoints take D's cells from the
     rebuild (`Tex::import_cells`), except those written again before
     them — `old exit ⊕ D`, the clean-region rule above;
   - one full state hash confirms it, covering what the tracker does not
     see yet (lists, registers above 255, the PDF writer); after a failed
     confirmation the next waits 2, 4, 8… checkpoints. A cheap input hash
     comes first (LaTeX's `\usepackage` peeks at the next line, so an
     edited line sits in the buffer while a package loads).
   Measured: a one-letter `\title` edit in a 295-page book (66M commands,
   68 s cold) rebuilt in 56 s before and 2.8 s now (1.4M commands run,
   converged after the title page); in a 23-page article, 0.41 s against
   0.94 s without and 1.04 s cold. PDF, log and aux identical to a fresh
   build.
5. **Partial splices, and names as cells.** A `\label` edit changes the
   `.aux` file, and LaTeX reads it back twice: at `\end{document}`
   (pass 1) and at `\begin{document}` of the pass that follows. Both
   reads used to defeat cutoff. That cost 118 s on the 295-page book,
   against about 7 s for a `\ref` edit. The chosen route is not DESIGN's
   general propagation, which does not exist yet. It is the smallest
   sound step towards it, inside the checkpoint model:
   - *Splice up to a reader.* In PDF mode a splice may stop at the
     previous build's last checkpoint before either of two things: the
     first interval that reads something in D, or its first read of an
     output this run wrote differently. The job goes on from that
     checkpoint, moved, with D imported. The previous build is kept
     whole (its checkpoints and pages are copied, not moved), so the job
     can meet it again at a later checkpoint and splice again. D then
     costs only the intervals that read it: in effect, per-label cells.
   - *Names are cells.* The pass after a rename sees a hash table and a
     string pool that differ. One slot holds `r@sec:smooth-lip` instead
     of `r@sec:smooth-lipschitz`, one string differs, and every later
     string moves. `Tex::name_differences` finds the slots whose name or
     link differs, and the strings whose characters differ. It refuses
     when a differing string is also held outside the hash table (a file
     name, a font, the input stack). `Tex::import_names` gives a
     checkpoint the rebuild's slots and strings below the meeting point,
     keeping its own later strings, moved. The full state hash confirms,
     as for eqtb cells.
   - *Finer reads.* A slot's name (`Cell::Hash`) and its link
     (`Cell::HashNext`) are separate cells. A lookup that finds a name,
     or misses it without entering it, is recorded as reading the name
     (`Cell::Str`, keyed by a hash of its characters) and the slot it
     found. It is not recorded as reading every slot it walked past: two
     valid tables that differ only in other names give the same answer.
     Entering a name still reads the walk and the chain's end. Without
     this, a hot macro sharing a chain with the renamed label
     (`\realfootnote`) stopped every splice within a page. web2c's
     `search_string` reads the same `Cell::Str`.
   - `tally` (a count of characters printed, reset before every read)
     left the state hash. A `\write` of another length changed it, and
     so blocked cutoff after every label edit.
   Switches: `PARTEX_PARTIAL_SPLICE=0` and `PARTEX_NAME_CUTOFF=0`. The
   e2e case `readback` covers a changed value, a second edit, and a
   rename. Measured on the book, a label renamed: 118 s before, 78.6 s
   with the pass-1 splice alone, 56.5 s with names but walked slots
   read, and 9.45 s with all of the above. PDF and aux are identical to
   a fresh build.
6. **Convergence after a text edit.** A one-word edit in a chapter of
   the book used to rerun about 13 pages (7.4 s). Five things stopped
   it meeting the previous build at the next page:
   - *Checkpoints per page in PDF mode.* The shipped-page count that
     places checkpoints was only counted for DVI, so PDF jobs had one
     checkpoint per 100-line block. Thinning over budget now drops the
     checkpoint whose neighbours are closest, not every other one
     (`PARTEX_EVEN_CHECKPOINTS=0`: the old spacing). Checkpoints where
     one of the document's own files (found as `./name`) begins or ends
     are kept through thinning (`PARTEX_FILE_CHECKPOINTS=0`: not).
     `\include` looks a chapter up by `\pdffilesize` between two
     chapters, so backdating that lookup reruns a few thousand commands,
     not a 150,000-command interval.
   - *Lines as values for resuming.* A checkpoint stays valid for a
     changed file that it read only as lines before the first
     difference, and that is closed or not yet read past it
     (`LineRule`, `PARTEX_LINE_RESUME=0`: not).
   - *Glyphs used are an accumulating cell.* A font's used-character
     set is read only when the fonts are written at the end. The
     difference (added, removed) is imported into the previous build's
     later checkpoints like eqtb cells. A character the previous build
     used again in between stays (`Interval.chars`, filled from
     `Tex::take_chars_shipped`; `PARTEX_CHARS_CUTOFF=0`: not).
   - *The open object stream is output.* A moved link rectangle changes
     the bytes of objects waiting in the PDF object stream. Those bytes
     are output not yet written, not state. The rebuild's waiting
     objects replace the previous build's in its later checkpoints. A
     partial splice stops before the previous build writes that stream
     (`ObjStmDiffer`, `PARTEX_OBJSTM_CUTOFF=0`: not). Two dead output
     fields (`stream_length`, `last_byte`) left the hash.
   - *Backdating keeps the checkpoint's mark.* `clone_checkpoint`, not a
     plain clone (which takes the current shared mark). Without it,
     effect comparison after a backdating trial always failed.
   Measured on the book, with the edits in sequence (text, `\ref`,
   `\label`, comment, whitespace). Before: 7.4 s, 7.0 s, 9.45 s,
   481 ms, 542 ms. After: 1.71 s, 1.43 s, 5.58 s, 311 ms, 345 ms (with
   the hash memo: comment 238 ms, whitespace 295 ms). PDF
   and aux are identical to a fresh build. What remains is mostly fixed
   cost:
   - about 50 ms to find the changed files;
   - 45–100 ms per full state hash (each confirmation, each backdated
     lookup). It was 90–145 ms before the session shared one
     `StateHashMemo` across its states (master's `hashmemo.rs`,
     `PARTEX_HASH_MEMO=0`: not). The rest is chunks the running job has
     not committed, and parts not in shared chunks;
   - about 100 ms to resume;
   - about 850 ms rerunning up to the next object-stream write after a
     splice, because the stream is written compressed and cannot be
     patched in place. A link step that rebuilds the stream would remove
     this cost.
7. **Object streams rendered at the splice.** The rebuild's waiting
   objects (item 6) used to stop a splice before the previous build wrote
   their object stream. Packing and compressing a stream is output, not
   state. So a session records each object stream as it is written
   (`ObjStmWritten`: its object number, where its object begins and ends
   in the file, and its objects uncompressed; `Tex::record_objstms`, taken
   into `Interval.objstms`). A splice past that write renders the object
   again, with the rebuild's objects first and the previous build's later
   ones moved after them (`ObjStmWritten::with_differ`, `render`). It then
   replaces the old object's bytes, and positions after it move by the
   difference in length (`Tex::relocate_output_past`). Object numbers stay
   the engine's, because TeX observes them. Before trusting the renderer,
   the splice renders the old record and requires the old bytes. If they
   differ, it falls back to stopping before the write
   (`PARTEX_OBJSTM_LINK=0`: always). Only one stream is recompressed per
   splice, so there is nothing to parallelize yet. Measured on the book:
   text 2.17 s → 1.40 s, `\ref` 1.40 s → 715 ms. PDF and aux are
   identical to a fresh build, with `PARTEX_CHECK_OFFSETS` clean.
   The machine's line log (`Tex::line_log`) is now kept only by a
   machine (`log_lines`). Before, a session engine carried every line
   read to the end of the job and cloned it into every checkpoint: 38% of
   a rebuild's CPU went to cloning and dropping it. With that fixed and
   one render per splice: text 1.29 s → 0.82 s, `\ref` 617 → 589 ms.
8. **Where the floor is** (the book, after items 1–7). A text edit
   costs 821 ms:
   - finding the change: 29 ms;
   - trying token equality: 30 ms;
   - the backdating trial of `\include`'s `\pdffilesize` interval,
     which is two state hashes and 8,000 commands: 175 ms;
   - resuming, which reruns the first interval: 105 ms;
   - rerunning to the meeting page, 1–4 pages plus one hash: 274 ms;
   - the tail from the last checkpoint (font subsets, xref): 185 ms.

   A comment edit costs 194 ms: finding the change 29 ms, the token
   check 43 ms, and the backdated lookup 83 ms, of which 47 ms is two
   hashes. A full state hash costs 40–95 ms even with the memo, because
   it visits every token list (`Canon::tok`, numbering by first use).
   Going below that needs a session hash that is incremental over dirty
   chunks, as machine mode's Rest hash is.

   Since then, the state hash takes each token list by the hash its store
   keeps: computed when the list is frozen at a snapshot, and taken from
   the shadow while a running list is unchanged (`TokStore::hash_of`,
   `PARTEX_LIST_HASH=0`: computed). Lists enter by contents, not by
   first-use numbering, because TeX never observes list ids or sharing.
   A confirmation now costs 14–53 ms, down from 40–95 ms. Pruning pinned
   files walks the line events without hashing each one, which saves
   11 ms. The rest of the hash is the scan of every defined eqtb word.

   Type 1 subsets are memoized. `writet1` is a pure function of its job:
   the font file, the glyph set, the encoding codes, slant and extend,
   the subset tags given so far, and pdfTeX's static
   `lastargOtherSubr3`. So its result, with the job fields it changes,
   is kept under a hash of all of those (`Host::cached`); the session's
   host keeps them for the process, as a machine's host does
   (`PARTEX_T1_CACHE=0`: not kept). A rebuild's tail, which writes every
   font at the end, goes from 141 ms to 86 ms.

9. **Resume points between paragraphs.** Checkpoints fall after pages,
   so an edit mid-page first ran again everything from the page's top.
   While a rebuild runs again (from its resume point until it meets the
   previous build), the engine also stops at each new line of a file
   read in vertical mode, that is between paragraphs (`Tex::set_dense`).
   These stops are apart from the ordinary checkpoints, which stay where
   they would be, so cutoff still meets the previous build's. The
   session keeps up to 48 of them and pins them against thinning until
   the next rebuild. The next edit near the last one resumes from its
   paragraph, not from its page (`PARTEX_DENSE_CHECKPOINTS=0`: off). On
   the book, a text edit after another in the same place reruns
   8–10 thousand commands instead of 135 thousand: 589 → about 400 ms.
   This is the first step of sub-page regions. Replaying the unchanged
   paragraphs after an edit is the remaining step. It needs the state
   after each paragraph compared apart from the current page's
   contribution list, so that one paragraph's new lines do not hide an
   otherwise equal state. The unchanged paragraphs' recorded vertical
   material could then be appended to that list again, and the page
   builder and output routine run over it; `\write`, `\mark` and
   inserts are ordered within that material. Not built.

   The engine is not the gap. On the book, a single cold pass takes
   52.5–57 s in partex (185 ms a page) against 58–62 s for TeX Live's
   pdfTeX 1.40.29 (200 ms a page). Its top symbols are `get_token` 17%,
   `macro_call` 12%, `get_x_token` 10%, `get_next_slow` 10% and
   `pass_text` 4%, so the course is bound by expansion. An edit that
   must typeset again even one page therefore costs about 0.2 s. Under
   100 ms needs regions finer than a page.

**Checkpoints** are the snapshots regions will start from. They are
placed where an input file begins or ends, once per block of lines just
after a page is shipped out, and after each page; token lists may wait
below them (LaTeX's `\include` reads its file from inside a macro). At
most 256 are kept (`PARTEX_WATCH_CHECKPOINTS`), thinned evenly (item 6);
convergence is still tested at every due
checkpoint. On the PGF subset: 112 checkpoints instead of 1531, 1.4 GB
peak instead of 19.6 GB, one-word edits 1.2–2.4 s. `PARTEX_WATCH_DEBUG`
prints, at a refused cutoff, which sections of the state hash differ,
which eqtb cells, where two buffers differ, and which interval reads which
cell.

**Off the critical path** (mold's fork trick): the session save (136 MB
per rebuild on the 295-page book) and dropping large sets of checkpoints
happen after `Finished … → paper.pdf` is reported, on a background
thread, so the user never waits for bookkeeping. The outputs are written
PDF (or DVI) first, renamed into place so a viewer never reads half a
file. The others follow, and a file whose bytes and length are what was
last written is not written again, so a viewer does not reload an
unchanged PDF. There is no fsync (`PARTEX_FAST_WRITE=0`: every file,
in place). On the book this takes 1–3 ms per rebuild.

**Seeing an edit.** The watcher polls the document's own files (relative
paths) every 20 ms (`PARTEX_WATCH_POLL_MS`) and the TeX tree's every
200 ms, instead of everything every 200 ms. A rebuild then looks every
lookup up again (`Session::changes`), but it reads a file's contents only
if its stamp changed. The stamp is size, mtime, ctime and inode. It is
compared with the stamp the file had when its contents were last found
equal, and the file must still be found at the same path
(`NativeHost::locate`). Each lookup keeps its own copy of a file, which
line-level dependencies rely on (`PARTEX_STAT_CACHE=0`: every file is
read). On the book: 50 ms → 20–28 ms.

**Gaps, in the order they are removed:** propagation stops only once per
build (no replay between dirty regions); read sets cover eqtb and the
hash only; read sets are not saved with a session; checkpoints are the
only regions; `history` is state, so an edit that adds or removes an error
does not stop early; a line-count change shifts `\inputlineno`.

**Where machine mode replaces this path.** Machine mode (§7.0) is the
target propagation: regions, backedges, replay between dirty regions,
and `old exit ⊕ D` at every clean region. Which pieces it can take
over, and when:

- *Early cutoff and read-set cutoff* (steps 2 and 4): already replaced.
  A machine rebuild compares per cell at every old boundary, and replays
  instead of stopping once. It can take over once machine mode is the
  default for `partex watch`. Two conditions: cold within 10% of a plain
  run (it is at +6% cycles), and memory: 3.7× a plain run cold and 5.4×
  after 20 edits in 20 places, with refine off and coarsening on (§7.0,
  "Memory").
- *Partial splices and names as cells* (step 5): replaced for `\label`
  (0.15 s against 9.45 s) by `Written` cells and the read-back rule. The
  name cells are ported: a name is part of its eqtb cell, a link and a
  name's place are cells (`MCell::Link`, `MCell::Name`), and the string
  pool left `Rest` (§7.0, "A first use moved").
- *Backdating* (step 3): subsumed. Machine regions read `File` and
  `Line` cells, so an `\includeonly` probe is a guard on one cell.
- *Resume from checkpoints* (step 1) and the session save: the machine
  `Build` is persisted for `partex watch` and `partex build` (§7.9,
  "Persisted machine builds"). `-watch` keeps its own session save.
- *The one-shot fixpoint* (§7.10): stays on the session path until
  machine rebuilds run the in-process passes.

### 7.5 Holes and forcing

A region does not need every value to proceed, only the values it
observes. A **hole** is a symbolic value: "cell *c* at the entry of region
*R*", resolved when *R*'s predecessor is known. It is a thunk (Haskell), a
future (Multilisp; JavaScript's promises), a residual value of partial
evaluation (Futamura; PyPy):

- it flows without being forced: stored in a register, copied, passed as
  an argument, written to `.aux` or a `\write` stream;
- `\the`, `\number` and `\romannumeral` of a hole give a **hole token**,
  which prints its value when resolved;
- typeset, it is a **hole glyph run** of unknown width;
- integers stay symbolic under arithmetic as affine forms `h + k`:
  `\stepcounter{section}` on a hole gives the hole plus one (LLVM's scalar
  evolution keeps loop counters as symbolic recurrences the same way).

**Forcing points.** Only a few operations need a concrete value:

| Operation | Forces? |
|---|---|
| `\ifnum`, `\ifodd`, `\ifdim`, `\ifcase`, `\ifx` on a hole | yes |
| arithmetic whose result reaches a forcing point | yes, when it does |
| `\wd`, `\ht`, `\dp` of a box with holes, `\string`, `\meaning`, `\detokenize` of a hole | yes |
| line breaking a paragraph with a hole glyph run | yes, that paragraph only |
| `\hbox to` a fixed width containing holes (headers, footers, page numbers) | no: the outer size is known; the glue set is resolved at link time |
| `\write`, `\edef` into a macro, storing, copying, argument passing | no |

**What a forced hole does**, chosen per case by policy (§7.8):

1. **Suspend.** The dependent piece becomes a continuation (Lua's
   coroutines; async/await); the region goes on with independent work and
   the piece resumes when the value arrives. A paragraph with a `\pageref`
   waits; the rest of the chapter typesets.
2. **Speculate with a guard.** Take last build's value (or a predicted
   one), record the guard "assumed `\c@page = 37`", and go on (V8's
   speculative optimization). A wrong guess deoptimizes only that piece.

**Deferred layout values are holes too.** Every box appended to a
vertical list reads `\prevdepth` for its interline glue (`append_to_vlist`,
§679); `\addvspace` reads the last glue of the list (`\lastskip`, §424);
main control runs the page builder (§994) at every paragraph's start and
end at the outer level, where it reads the page so far. Under any guess
these mismatch. A region therefore emits its nodes as a
position-independent contribution: its first interline glue is a hole, a
read of the list's tail is a read of the tail's value, and the page
builder does not run inside a region: at commit the main line runs
`append_to_vlist` and `build_page` over the committed stream, and output
routines run there as their own regions. A TeX-level read of a deferred
value (`\pagetotal`, `\lastskip`, `\prevdepth` after a deferred point)
forces it.

**Back-end futures.** A back-end call (§7.3) can return a future that the
front end forces when its result is observed (`\prevgraf`, `\lastbox`,
`\unskip` on the lines, the page builder). Measured catch: TeX calls the
page builder at the start of every outer paragraph, so a plain future is
forced almost at once; the lazy page builder above is what makes the
overlap real.

**Where holes stay cheap.** Page numbers in headers and footers resolve at
the link step with no rework. `\ref` and `\pageref` in running text force
their own paragraph's line breaking; if the line count is unchanged the
page does not move (early cutoff). Section and equation counters are
entry value plus a local count. The truly forced cases (`\ifodd\c@page` in
a two-sided output routine, float placement depending on the page) are
rare and local: an output routine's parity choice suspends until the link
step.

### 7.6 Effects as values and the link step

Nothing inside a region writes a file. Everything that leaves the engine
is an effect value: log and terminal text, `\write`, `\openout`,
`\closeout`, `\message`, file opens, pages. A final **link step**
assembles the output in program order, as a linker resolves relocations:

- **Numbering is symbolic.** Font ids become `/F17` (and DVI's `fnt_def`
  numbers), PDF objects get numbers in allocation order: a font takes
  `font_ptr + 1` at its first load (§576), an object the next number at
  `pdf_create_obj`. A region allocates symbols (an allocation id, a font
  identity); the link step assigns pdfTeX's numbers by prefix sums over the
  allocation order the regions recorded. That covers what TeX never
  observes: page objects, content streams, font dictionaries and resource
  names, annotations, destinations, outlines, the names tree, xobject
  references. A number TeX does observe (`\pdflastobj`, `\pdflastxform`,
  `\pdflastximage`, `\pdflastannot`, `\pdflastlink`) is a hole token.
- **Offsets** (`/Length`, xref, `startxref`) are prefix sums of byte
  lengths, computed when the file is written once, in pdfTeX's object
  order. TeX never observes them.
- **Font subsets** are written at the end, as pdfTeX does, from the used
  glyphs of the committed pages.
- **Holes resolve**: affine forms are evaluated with the actual entry
  values, hole glyphs filled in, fixed-width boxes containing holes packed
  again (with web2c's floating-point glue arithmetic, exactly), suspended
  output-routine pieces resumed.
- **Text.** The log and terminal wrap at `max_print_line` by the current
  column (`file_offset`, `term_offset`), and `\write` to the terminal
  shares it, so printed text is not position-independent. A text effect is
  kept unwrapped with the column it began at, and wrapped at the link.
- **`.aux` and other streams** are concatenated. The next pass's
  `\input{doc.aux}` is a set of line values (§7.2), so only readers of
  changed lines are dirty.

**Built** (`effects.rs`, `pdf/xref.rs`; roadmap step 4, first part).
With `PARTEX_EFFECTS=1`, and always in machine mode, the engine appends
`Effect`s instead of writing: `Write` (bytes of the log, the DVI or the
PDF), `Term`, `Close`, `PdfObject{file, num, ahead}` (object `num`
begins `ahead` bytes after the file's bytes so far), `PdfXref` (the
cross-reference table or stream, as data) and `Length` (a printed byte
count). `effects::link` takes the chunks in program order:

1. Layout, one pass over the effects (not the bytes): each file's
   pieces and size, the objects' offsets by prefix sums, and the xref,
   trailer and `startxref` rendered from those offsets, pdfTeX's
   `/Length` padding and all.
2. Byte counts ("Output written on … N bytes"): the engine printed its
   own count, and the `Length` effect holds those printed digits, apart
   from the stream's other bytes. The link writes the real length. A
   count with another number of digits would wrap the log differently:
   `LinkError::LengthDigits`, and the region runs again.
3. One parallel copy of every piece into preallocated files (through
   the `Executor` from 1 MiB).

e2e is identical with `PARTEX_EFFECTS=1`, and so is the 295-page
course. A plain run costs the same with and without it (333.4 G
instructions on the course either way).
Decisions, and what is not symbolic yet:

- **`\write` streams are not effects.** The job reads them back in the
  same run (`texsys.aux`, `\@input{…aux}` after `\closeout`), so they
  are state: written to the host at once, and read through `File`
  cells in machine mode.
- **Object numbers are virtual in machine mode** (`pdf/vnum.rs`;
  `PARTEX_MACHINE_VIRTOBJ=0` turns it off). The engine names each object
  by a *virtual id*: a hash of the position of the step that makes it
  (`TexMachine::at`) and a count within the step, the first free one
  (each id tried is read). Two runs name an object made at the same
  place alike, so a reused region's state and effects stay right when
  an edit before it adds objects. The object table is a map by id
  (`ObjTab::vtab`), and its entries are cells as before (§7.0).
  What pdfTeX's numbers depend on is a log of events in program order
  (`NumEvent`: an object created, an object begun and ended in an object
  stream, the job's end), an accumulating cell (`MCell::Numbering`, its
  version a running hash; a region's value is the events it added), and
  the same events are effects (`Effect::Num`). `Numbering` replays them
  as pdfTeX numbers: each creation takes the next number, an object
  stream takes one when its first object begins and closes after its
  hundredth ends (or at the job's end). The engine no longer fills
  object streams: an object's bytes go out as it is written
  (`ObjStmStart`, `ObjStmBytes`), and the link lays the streams out.
  - *Printing.* Every object number printed goes through
    `PdfOut::objnum`: its digits become a relocation, cut out of the
    bytes when they are handed on (`Effect::ObjRef`, `Effect::ObjStmRef`
    in an object stream). A direct object's mark flushes the writer
    first, so no relocation sits in the bytes its `ahead` counts; an
    object that ends in a stream flushes the file's buffer too, as the
    link may close a stream there. The link replays the numbering
    (`resolve_numbers`), writes each relocation's digits, renumbers the
    objects' marks and closes the streams, then lays out as before.
  - *Observing.* Where TeX sees a number (`\pdflastobj`,
    `\pdflastxform`, `\pdflastximage`, `\pdflastannot`,
    `\pdflastlink`) or gives one back (`\pdfrefobj`, `\pdfrefxform`,
    `\pdfrefximage`, `useobjnum`), the engine replays the log
    (`ObjTab::numbering`, cached) and the region reads
    `MCell::Numbering`: it re-runs whenever numbering before it
    changed. The end of the job observes it too: it lays out the
    cross-reference section by pdfTeX's numbers (`xref_virt`, the free
    list over them), prints the trailer's with them, and counts the
    objects for "PDF statistics".
  The course's cold build costs the same (50.9 s), every output is
  identical, and a `\Cref` added re-runs 7 regions instead of 1605.
- **Fonts' numbers are the link's too.** A font's `/F` name is its
  number in the order of loading (`FontArrays::number`), not its slot:
  printed as a relocation (`Effect::FontRef`, `ObjStmFontRef`) and
  written by the link from `Effect::FontLoad`. Because pdfTeX prints
  those names inside compressed content streams, a stream that
  `pdf_begin_stream` begins is compressed by the link (`Effect::Deflate`
  with its relocations, `Effect::StreamLength` for its `/Length`),
  memoized by content in the host; the engine counts it as compressed
  with the digits it had, for "Output written". An image's stream,
  written as it is, is not deferred.
- **DVI is bytes.** Its offsets (each `bop`'s back pointer, `post`'s
  pointer) are computed by the engine from `dvi_offset`, which is
  state, so a DVI page is already exact where it is written; there is
  nothing to relocate.
- **Text is wrapped by the engine.** The column (`file_offset`,
  `term_offset`) is part of `Rest`, so a region's text is exact for its
  entry. Unwrapped text effects with a starting column come with the
  split of `Rest`.

**Built like mold.** The link step is a linker's problem, and Rui
Ueyama's mold shows how to make it nearly free: every pass a data-parallel
loop, offsets by prefix sum, then an embarrassingly parallel copy into a
preallocated output, concurrent hash maps instead of locks, zero-copy
inputs, and teardown off the critical path. For partex:

1. Encode and compress each page's content stream, each font subset and
   each image in parallel. zlib-exact deflate (§2) works per stream, so
   streams are independent.
2. Object numbers and byte offsets by prefix sums over the sizes.
3. Every object written in parallel into a preallocated, memory-mapped
   PDF; the xref emitted from the same prefix sums.

**A fast full rewrite over splicing.** mold rejects incremental linking:
if a full link is fast enough, splicing is not worth its complexity. The
same holds for the back end. Moving writer positions and splicing the
previous build's PDF tail (§7.4) goes away once the whole file is linked
again, in parallel, from cached page IR and encoded streams keyed by page
hash. Incrementality is kept where it pays (the front end, a minute cold
for a book); brute parallel speed where it is cheap (the back end).

**PDF ship-out onto page IR.** Today only DVI mode goes through page IR
(§5.6); PDF ship-out (`pdf/ship.rs`) walks boxes into PDF bytes as pdfTeX
does, allocating objects and marking used glyphs on the way, which puts
export-only accumulators into engine state. Measured on the PGF subset:
between converge passes, where the state differs only in eqtb and PDF
state, the only PDF difference is one font's used-glyph set (a page number
with other digits); once the index exists, the third pass has 4–5 more
objects from early on, renumbering every later object. Steps, each
byte-identical on e2e, the PGF subset and the manuals:

1. *Record, replay per page.* The walk keeps what TeX observes and what can
   print or fail (writes, late expansion, `\pdfsavepos`, color and link
   stacks, first-use font checks) and records the encoder's calls as
   display items; the content stream is encoded at the page's end. Exact
   because pdfTeX writes no other object while a content stream is open.
2. *Encoder state out of the engine.* The text state machine, glyph marks
   and resource lists live in the encoder; engine state holds display
   lists, not bytes or offsets.
3. *Link.* The file-level sequence is recorded too, and the whole file is
   written at the end, numbering objects and computing offsets there.
4. *Reuse.* Display lists hashed per box and page: a rebuild encodes only
   changed pages; a replayed region contributes its recorded display lists
   and objects.

### 7.7 Parallel rounds

A build is a fixpoint over regions, computed by Jacobi iteration rather
than Gauss–Seidel (chaotic iteration), on a work-stealing scheduler:

1. **Round 1.** Every dirty region starts at once from its entry state:
   in a rebuild, the cached exit of its predecessor with the known
   differences patched in; in a cold build, the cached preamble state,
   with holes for cells predicted volatile and speculated guards for the
   rest (§7.8). Each records its trace.
2. **Round n.** When a predecessor's exit becomes known, holes are
   filled, suspended pieces resume, and guards are checked against the
   actual entry, *only over the cells the region read*. A region whose
   guards hold is final whether or not its predecessors reran: a rerun
   that produces the same values leaves its successors valid. Failed
   guards re-run, now from a known entry.
3. **Fixpoint.** Every region's actual entry agrees with its
   predecessor's exit on everything it read, and every hole is resolved.
   That is exactly the sequential execution, which is why the output is
   byte-identical. The link step then commits in program order (§0.5).
4. **Termination.** The number of rounds is one plus the longest chain of
   corrections that change a value, not the number of regions. A round cap
   with a sequential fallback handles a region that oscillates (a page
   break changes a reference's width, which changes the page break), as
   LaTeX's passes effectively do.

The same loop serves three workloads: a cold build (poor guesses, several
rounds), a warm rebuild (the previous build's states validate nearly
everything in one round) and the aux-file fixpoint (§7.10: a pass's labels
are the next pass's guesses, and only readers of a changed label rerun).

**Why the naive form failed**, measured on the PGF manual (134-page
subset, 1,156 blank-line blocks after `\begin{document}`): workers running
ahead of a sequential main line from a guessed state committed almost
nothing (125 blocks adopted, 275 if scratch macros are disregarded). A
typical boundary has about 6,800 cells live across it, mostly definitions
made on first use and read for the rest of the document, and many blank
lines fall inside unfinished computations (`\@ifnextchar` looking ahead
across them). Each cause has its mechanism: *composed guesses* (the prefix
scan of round 1's writes gives round 2 nearly all first-use definitions,
whose values do not depend on which region made them); *holes* for
deferred layout values and counters (§7.5); *symbolic numbering* (§7.6);
and warm starts from the previous build, where write-once definitions
validate unchanged.

**What stays sequential**: validating guards, applying writes, the page
builder over committed contributions, output routines that force the page,
and the link step. This is the serial fraction and bounds the scheme by
Amdahl's law. Estimate, to be replaced by measurement: for the 295-page
course (≈67 s cold, preamble ≈2 s), round 1 on 20 cores ≈3.3 s and a small
round 2, about 5–7 s cold.

**Shared tables in parallel rounds**: token-list interning, font glyph
sets and PDF resource deduplication go through concurrent hash maps (as
mold's symbol table does), merged in program order where order is
observable.

**Prerequisites**: complete cells (§7.1, or guards are not sound);
effects as values and the link step (§7.6); persistent `Send` state
(§5.3: a worker starts from a composed snapshot, so snapshots and diffs
must be cheap and cross threads; done, the session host included).

**Measured on TeX** (`PARTEX_MACHINE_ROUNDS=N` with `PARTEX_MACHINE_EDIT`,
warm from the build before the edits, regions merged to
`PARTEX_MACHINE_REGIONS`, default 4N; holes off). Byte-identical to the
sequential rebuild. On the course with four edits in four chapters:
9 rounds on 24 threads, 17.4 s, against 5.7 s for the sequential
rebuild and 55.8 s for a plain run. Rounds lose because `Rest` is
coarse: a region's entry guard on `Rest` holds only if its
predecessor's exit is exactly what the warm guess had, so a changed
page count validates the chain one region per round, and each round
re-runs every region behind the change.

**A cold split on TeX.** Split points cost nothing to find. The course's
main file names its chapters on lines 52–94 (`\chapterfile{N}{chNN}`),
and a scan through the lexer (§7.2) finds them before anything runs.
What a cold split lacks is entry states. Two measurements bound what a
guessed entry can achieve:

- Given exact entries, TeX regions run in parallel well. Refine re-runs
  all of the course's regions, 58 s of sequential work, in 8.2 s on 24
  threads (7.1×).
- A state reached by a different history validates only if the history
  leaves no trace. After an added paragraph, the true state at every
  later boundary equals the old run's in eqtb, the lists and the page
  builder, and differs in string-pool order, object numbers and
  destination order. Those were `Rest`'s, and all 1608 later regions
  re-ran (29 s). With strings by content and the object table as cells
  (§7.0, "Numbering histories"), 11 regions re-run (2.7 s).

A scan-predicted entry at `\chapterfile{k}` differs in more than that:
the page counter, the object and string counts, fonts loaded on first
use, the `\write` files' contents and the glyph sets. Every region reads
`Rest`, so round 2 validates nothing past the first chapter, and each
round re-runs the chain behind it. Rounds therefore cost at least a
sequential cold build plus round 1's wasted work, and a cold split
cannot beat sequential cold today.

Two things change that, in this order:

1. The numbering leaves `Rest`: strings compared by content, and the
   object table as cells (done, §7.0). The counts (`str_ptr`,
   `obj_ptr`) are still `Rest`'s; a guessed entry needs them as holes
   (§7.5).
2. The page counter and the page builder become cells with holes.

**Measured: what a guessed entry differs in** (`PARTEX_MACHINE_SPLITPROBE=1`).
The probe runs the course sequentially and stops at each top-level line
of the main file that begins with `\chapterfile`, `\input`,
`\include`, `\part`, `\chapter` or `\section` after
`\begin{document}` (a scan of the source: 38 lines, 31 met at a
boundary). There it compares the true state with two guesses at the
same position (`TexMachine::adopt_position`: the input stack, open files
and buffer taken from the truth): the state at the first split (a cold
split's guess), and the true state at the split before (the best a
round can compose, one chapter of history missing).

| guess | `Rest` parts that differ (of 30 splits) | eqtb words that differ |
|---|---|---|
| the first split's state | 24 at every split (the last positions at 29, the stacks at 2) | 1303–3189 (median 2294) |
| the split before's state | tables, strings and fonts, input conditionals, lists, page, marks, scalars, pdf, objs, out, ship at every split; printing, file names, last link at 29; current value at 21; input stack at 20 | 143–1303 (median 236) |

Even one chapter of history missing leaves 11 parts of `Rest` and about
236 eqtb words different: new control sequences and strings, fonts
loaded on first use, the page builder and the lists (the chapter before
is still on the page at its `\chapterfile` line), marks, the PDF
back end's pages and objects. Every region reads `Rest`, so no guessed
entry validates: round r validates exactly region r, and rounds cost
more than a sequential run. `str_ptr` and `obj_ptr` as holes would not
change that; the numbering part is done anyway (virtual numbers, §7.6),
for the warm case. `TexMachine::split` still returns one region.

What a cold split needs, from the table: `Rest` itself split into cells
(the hash table and strings by name, fonts, the PDF back end's lists),
the page builder deferred (§7.5), and split points after a chapter's
`\clearpage`, not before it. The names and the fonts are cells now
(§7.0), which a warm rebuild needed first; cold parallel rounds are set
aside for the nested units below (§7.15): a warm rebuild that
re-runs little matters more than a cold one that runs in parallel.

Then the scan's entries (the preamble's state plus the chapter counter,
which `\chapterfile` sets itself) validate each chapter's regions,
except those that read the page number or allocate objects. Refine's 7×
is the ceiling. With a previous session, its checkpoints give exact
entries, which is the warm rebuild (0.09–0.16 s).

### 7.8 Policies by measurement

Nothing that could be decided by observation is fixed in advance. The
defaults below are starting points, each with a switch, a counter and a
benchmark:

- **Granularity** is found dynamically and hierarchically (§7.3): the
  chooser's thresholds are functions of recorded costs, and regions split
  and merge from validation outcomes. The floor is whatever pays: macro
  calls where the profile shows repetition, paragraphs or pages elsewhere.
  Measured on the course: a region cut costs about 5 ms, O(state), not
  O(what the region changed): hashing `Rest` (40%), the indexes' B-trees
  keyed by `MCell` (18%), allocation (12%), freezing the token store
  (10%), page faults (7%). A rebuild re-recorded every re-run at the fine
  grain (1024 commands), so a `\linespread` edit, which re-runs
  everything, made 4,399 cuts against 282 cold and took 84 s against
  55 s. `build::Config::grow` (`PARTEX_MACHINE_GROW`, off by default)
  multiplies the grain by it after each cut along one re-execution, up to
  the cold grain: fine where the edit is, where the next edit is likely,
  coarse where its effects run on. At 2: `\linespread` 67.5 s (1,334
  regions), a one-word edit 0.51 → 0.37 s, the next edit there 74 →
  78 ms. Preferring boundaries where the state is small (outside every
  group, between paragraphs: parked, not merged) removes the `\@savsf`
  chain of a `\label` edit (1.34 → 0.46 s) but lengthens a `\Cref`'s
  (0.81 → 1.23 s), as old boundaries then fall less often right after
  a page. The structural fix is cuts that cost O(changes) (§7.3,
  "Next").
- **Cache scope** (per document, or shared across documents, branches and
  machines) follows from traces: which traces hit, in which documents,
  after which edits, at what size. The content-addressed store (§7.9) makes
  either possible; measurements choose.
- **Hole policy**: suspend or speculate, and which cells become holes at
  all. Both are implemented and profiled. A volatility predictor learned
  from previous builds (V8's type feedback applied to cells) marks cells
  the document body writes (LaTeX counters, `\@currentlabel`, page state)
  as holes and speculates the stable ones (definitions made in the
  preamble); with no history, name rules (`\c@…`) and suspension.
  Measured on the stub (a counter read by every region): speculation
  with the previous build's values 4.8× faster than sequential, suspension
  3.5×. Chosen: speculate from the previous build when there is one,
  suspend without history. On TeX holes are off for now (`Holes::Off`):
  the page counter, object numbers and `\ref` values live in `Rest`,
  so a hole needs the split of `Rest` and symbolic integers in the
  engine first.
- **Checkpoint and region budgets** follow memory and edit locality (§7.4).

Every policy decision is recorded (§8) so it can be evaluated after the
fact.

### 7.9 Caching, sessions and persistence

**Content-addressed store.** Traces live in a store keyed by `(entry
guard hashes, input hashes)`, with values stored as deduplicated blobs
(Bazel's action cache, Nix, ccache). A cold build hits traces from
earlier builds, branches or a restarted watch process. Impure cells make a
trace uncacheable (Bazel's `no-cache`). `SOURCE_DATE_EPOCH` pins time.

**Sessions** (`-watch`, `partex watch`) keep a warm process. After an edit
a rebuild resumes as §7.4 describes. At a checkpoint the engine hands every
buffered output to the host, so a splice never meets half-written buffers.

**Resident sessions** (`-resident` or `PARTEX_RESIDENT=1`, done). A plain
invocation gets the same rebuilds without a watcher: the first starts a
session in the background (the same binary, its own process group) and
asks it to build; later invocations with the same binary, arguments,
directory and TeX environment reach it over a Unix socket and get back the
terminal transcript and exit status. The socket's name hashes all of
these, plus the day unless `SOURCE_DATE_EPOCH` fixes it. A session serves
one request at a time and ends after half an hour idle
(`PARTEX_RESIDENT_IDLE`); any failure to reach one falls back to a local
run. On the PGF subset, `pdflatex -resident` took 31.7 s cold, 0.04 s
unchanged and 1.29 s after a one-word edit, against 30.0 s fresh, with
identical outputs.

**Persisted sessions** (`-converge`, done; `PARTEX_PERSIST=0` turns it
off). After a build that ran anything, the session is saved to the host
cache: the read log with contents, outputs, terminal text and pages, and up
to 8 checkpoints chosen by edit locality. The next process with the same
identity loads it and rebuilds like `-watch`. Only the logs are decoded
eagerly; a rebuild tells from each saved checkpoint's read count and open
files, without loading it, which is the latest valid, and loads that one
and the later ones. Measured on the 2-page article: cold 2.15 s, unchanged
rerun 0.08 s, a body edit 0.46 s. The saved file is ~80 MB (file contents
26 MB, hyphenation patterns 13.6 MB, eqtb/hash chunks 12 MB, a font map
5.5 MB; `PARTEX_PERSIST_STATS`). Next: an append-only pool that keeps
unchanged values instead of encoding them again; the format's values
referenced by the format's digest; read sets and traces saved with the
session.

**Persisted machine builds** (machine mode, done; `PARTEX_STORE=0` turns
it off). `partex watch`, and `partex build` in machine mode, keep the
recorded build (§7.3: starting and final states, and every region's
guards, writes and snapshots) in a content-addressed store, so a restart
or a second build costs a load plus the edit, not a cold run.

- *Store* (`store.rs`), under `PARTEX_STORE_DIR`, else `store` in
  `partex.toml`, else `$XDG_CACHE_HOME/partex/store`: blobs named by the
  stable hash of their bytes, in packs named by their own hash (with an
  index of 28 bytes per blob), and one root per job identity naming its
  packs, the hash of its bytes, then the bytes. Every file is written to
  a temporary name and renamed. Size is bounded (`PARTEX_STORE_MAX`, 4 GiB):
  least recently used roots go first, then packs no root names; a pack
  less than half alive is copied into the next save's pack.
- *No mmap.* Blobs are read with positioned reads (`read_exact_at`) of
  what the load asks for. Files are immutable and content-named, so no
  file changes under a reader, and there is no `unsafe`. Reads are
  checked: a blob against its name, a root against its hash. Anything
  missing or different builds cold.
- *Encoding* (`persist.rs`, merkle mode): a shared value (an `Arc`) of 64
  bytes or more is a blob of its own, referenced by hash, so snapshots
  share chunks on disk as they do in memory and a save writes only the
  blobs the store lacks. Smaller ones are inlined in their parent. The
  loader keeps what it loaded by (hash, type), so a value shared on disk
  is one `Arc` again.
- *Identity* (the root's name): executable (path, length, time), directory,
  engine command line and parameters, and the TeX and partex environment.
  Each of these decides how names resolve or what is written, so a change
  re-runs all. Input files are not part of the identity. The loaded build
  compares every file it read with the disk, and each difference is an
  edit that the ordinary rebuild handles.
- *The clock is a read, not identity.* The machine host serves the clock
  as a file under a name no file has (`\0clock`): the day and minute of
  `\time`, `\day`, `\month` and `\year`, the PDF creation date, the start
  of `\pdfelapsedtime`, and the `SOURCE_DATE_EPOCH`/`FORCE_SOURCE_DATE`
  settings. The region that reads it guards on it. A loaded build compares
  it with today, and a new day or new settings is an edit: only the
  regions that read the clock re-run (on the course, 145,494 of 66 million
  commands, 2.4 s, byte-identical to a cold build with that date). The
  start of `\pdfelapsedtime` stays the saved one, since every state holds
  it. For this the engine's `sys_*` date fields are not part of the
  compared state; the log's banner reads the job start from the host.
- *Validation:* the starting and final states are checked against their
  saved digests, and regions must be in order. `PARTEX_STORE_CHECK=1`
  also recomputes every snapshot's state hash. Any failure builds cold.
- *Parts, loaded lazily.* The root holds the host's shared parts and the
  starting state (0.1 MB). The final state is a blob, and the regions
  are saved in runs cut where a region key's hash says (about 8 regions,
  at most 32). The cuts are content-defined, so an edit moves only nearby
  runs. Each run is a blob. A snapshot is loaded only when a rebuild
  resumes from it or validates against it (`Snapshot` holds it `Stored`
  until then; `PARTEX_STORE_LAZY=0` loads all). The guards, writes and
  keys load eagerly, since the index needs them.
- *Parallel load.* The final state loads on one thread, reusing the
  starting state's blob cache. The runs load on the others
  (`PARTEX_STORE_THREADS`, else up to 7). The regions' index is built
  while the final state loads (`partex_incr::Index`). It groups eqtb
  cells by a counting sort over `Machine::dense_cell` and other cells by
  hash.
- *Incremental save.* A run whose fingerprint (keys, boundaries, guards,
  write versions) matches one the last save or load saw is written as a
  reference to its blob. Values the last save wrote, or the last load
  read, are known by address and held alive (`persist::Known`), so they
  are not encoded again. The store records each blob's references
  (`packs/<hash>.kids`) and keeps what the root reaches.
  `PARTEX_STORE_INCREMENTAL=0` encodes all.
- *Compression.* Each blob of 128 bytes or more is kept compressed when
  that saves an eighth (`lz.rs`: LZ4's scheme with LEB128 lengths, safe
  Rust, no dependency). A machine state is about half zeros.
  `PARTEX_STORE_COMPRESS=0` keeps blobs as they are. Blob names hash the
  uncompressed bytes.
- *Off the critical path:* a save starts after the outputs are written,
  on its own thread, over a copy of the build whose regions are merged to
  the grain (`PARTEX_STORE_COARSEN=0` keeps them fine: 4321 regions,
  2.8 GB, 41 s to load, against 281 regions, 0.5 GB, 4 s). A build that
  did not change is not saved again. `partex build` waits for the save
  only after printing its result.

Measured on the course (295 pages; the `.aux` files present; PDFs
byte-identical to a cold build):

| run | before (2b7dfb1) | now |
|---|---|---|
| cold, store empty | 55.9 s (save 2.3 s) | 51-54 s (save 4.0 s, after the result) |
| `build` again, unchanged | 4.7 s | 0.94-1.0 s |
| `build`, one-word edit | 5.1 s (save 5.2 s) | 1.7-1.9 s (save 1.05 s) |
| `build` on another day (`\today` read) | cold | 3.6 s (145,494 of 66 M commands) |

A load takes 0.68-0.75 s: open 25 ms, host 55 ms, root 17 ms, then the
final state (0.66-0.73 s, the critical path: fonts, the font map,
seals) while the runs (0.2 s) and the index (0.3 s) load on the other
threads. A one-word edit re-runs 0.2 to 0.6 million commands in 0.7 to
0.9 s. The store holds 248 MB for one job (blobs 217 MB, reference
lists 28 MB), plus about 13.5 MB per edit until eviction.

Next, toward a restart plus an edit of about the edit plus 0.3 s: load
the final state's large, rarely read parts (the font map, fonts not
used since, seals) lazily as snapshots are; keep the store's liveness
in memory between a session's saves (a save after an edit spends 0.6 s
reading reference lists and walking them, besides 0.43 s encoding).

**No-op restarts** (done; `PARTEX_STORE_QUICK=0` turns them off). A
restart that finds nothing changed does not wait for the saved build.
After a settled build's outputs are written, a record (`quick/<key>`
beside the store's roots, `quick.rs`, written on a thread of its own)
holds what the build read and wrote: every file served (the TeX tree's
too) by its stamp (length, modification and change times, inode, device)
and the hash of its contents, the files looked for and not found, the
clock if the job read it, each output as written, and the result as
reported (terminal text, diagnostics, history). A stamp is kept only if
the file on disk still holds what the build read (read back and
compared, the stamp taken before and after) and its times are over 2 s
old: a later write within the same clock tick could leave them alike
(git's racy entries); such a file is hashed instead. The next process
with the same identity looks at the files before anything else: an input
whose stamp is the same is unchanged, else its contents are hashed; each
file not found is looked for again; the clock is compared by day and
settings (§7.9's clock); each output must hash to what was written (an
output is always racy, so it is hashed). If all hold, the recorded result
is the result: no load, no link, no deflate, no write. `partex build`
ends there; `partex watch` loads the saved build on another thread and
waits for it at the first edit, then compares every file as a load does
(an edit made meanwhile is an edit). Anything else (no record, a missing
or modified output, a new day that `\today` read, a changed file of the
TeX tree) takes the path above, and the record's check costs it a few
milliseconds.

Measured on the course (warm page cache; the same binary with
`PARTEX_STORE_QUICK=0` before; PDFs byte-identical to a cold build):

| restart | before | now |
|---|---|---|
| `build`, nothing changed, to the result | 0.92-0.96 s | 0.07-0.08 s |
| `watch`, nothing changed, to the result | 0.91-0.97 s | 0.06-0.08 s |
| `build` after a one-word edit | 1.50-1.56 s | 1.53-1.54 s |

The check looks at 883 files (the TeX tree's are most of them) in 8 to
12 ms; the rest is the process, the sandbox and the TeX setup. A watch
that restarts with nothing changed has its build about 0.9 s later, in
time for any edit but one made at once.

### 7.10 One-shot builds

A document is built by one command, once. The user never runs `bibtex`,
`biber` or `makeindex`, and never reruns partex: the result is exactly
what the conventional pipeline gives when run to convergence.

**Done: in-process reruns** (`-converge` or `PARTEX_CONVERGE=1`). While a
file the job read has changed since (its own `.aux`, `.toc`), it rebuilds:
each pass resumes from the checkpoint before the first changed read and
stops early where the state meets the last pass's (§7.4); at most five.
e2e `converge` compares one `partex -converge` with Knuth's TeX run until
its `.toc` stops changing: identical, terminal included.

**Done: BibTeX in process** (crate `partex-bibtex`, `no_std` + `alloc`;
`partex -bibtex` or partex invoked as `bibtex`). A port of bibtex.web's
behaviour with TeX Live's bibtex.ch, not of its data structures: strings
are shared values (the pool leaks into behaviour in one place, global
string assignment, which literals carry as one bit); the hash table is an
interned-text map, interning exactly when bibtex.web makes a pool string
so the `.blg` statistics come out exact; `sort` uses the standard sort
(ties broken by position make the order total); web2c's line reading and
every message are faithful. Compared on xampl.bib under eight styles, TeX
Live's bibtex tests, a 1600-entry stress file and hand-written error
cases: `.bbl`, `.blg` (masked), terminal and status identical, about 1.4×
faster. After each `-converge` pass BibTeX runs on each `.aux` with
`\bibdata`, skipped when its lookups find the same. e2e `converge_bibtex`:
every file identical, 0.21 s against 0.69 s.

**Done: makeindex in process** (`partex-makeindex`). A port of makeindexk
2.18's behaviour, including qsort.c's comparison order (the `.ilg`
reports the comparison count and duplicates depend on it). Identical over
makeindexk's tests with every style and the main flags (322 runs) and a
random 30000-entry file (0.10 s against 0.17 s). e2e `converge_makeindex`:
0.23 s against 0.63 s.

**Next: passes are rounds.** Forward references make a document depend on
itself; the text they produce changes widths, so breaks, so the values
again: a fixpoint no single forward pass computes. On the execution model
that fixpoint is §7.7's loop with label and citation cells as holes or
speculated values: no output is encoded until it converges (§7.6), no
files pass between evaluations (the `.aux`, `.toc`, `.bbl` and `.ind`
contents are values), an evaluation re-executes only the regions that read
a changed value, and tools are functions called where their result is
needed (BibTeX when `\bibliography` reads the `.bbl`). When editing, the
previous build's converged values are in memory, so an edit that changes
no label converges at once. biber (a large Perl program with Unicode
collation) is a project of its own; until then biblatex documents need
`backend=bibtex`. Tools that run arbitrary programs (`minted`, shell
escape) stay unsupported.

### 7.11 Watch loop: cancellation and preview

**Cancellation** (done; `PARTEX_WATCH_CANCEL=0` turns it off). While a
watch rebuild runs, it looks at the times of the job's own input files
(not the TeX tree's) between re-executed spans, at most every
`PARTEX_WATCH_POLL_MS` (50 ms), once one span has re-run, so that a
stream of edits still moves on. A newer edit stops it at the next old
region's boundary (`Build::rebuild_or_stop`): what re-ran is kept, and the
build records a *frontier*, the region not reached and the cells that
may differ there between the new prefix and the regions recorded after
the old run. The next rebuild, with the newer edit, goes on from there:
it takes the frontier's cells as changed at the frontier, and compares
one with the old run's writes only once a region from the frontier on
wrote it (before that, the regions before the frontier no longer hold the
old values). A rebuild stops only past the frontier of the one before, so
frontiers never stack. Outputs are written, and a build saved or
coarsened, only from a rebuild that ran to its end (`Build::settled`).
A property test stops rebuilds after random looks, followed by further
edits, by none (a file whose time alone changed), by reverts, with the
accumulator and with coarsening after each settled rebuild, all under
the sanitizer; e2e `machine_edits` stops real LaTeX rebuilds
(`PARTEX_MACHINE_STOP=1`) and compares every file with pdfTeX's.

Measured on the course (a burst of keystrokes typed into one sentence
150-300 ms apart, from the last keystroke to a PDF on disk byte-identical
to a cold build of the final text; medians):

| edit | rebuild | burst, on | burst, off | one save, on | one save, off |
|---|---|---|---|---|---|
| ch05, 8 keystrokes | 0.2 s | 0.19 s | 0.19 s | 0.20 s | 0.21 s |
| ch03, 8 keystrokes | 0.2 s | 0.23 s | 0.22 s | 0.24 s | 0.24 s |
| ch12, 8 keystrokes (a page more) | 31 s | 33.1 s | 33.2 s | 31.4 s | 32.4 s |
| ch12, 36 keystrokes | 1.9 s | 2.7 s | 60.6 s | 1.9 s | 2.0 s |

Most edits rebuild faster than one types, and a watch that converges
already folds the edits that arrive during a rebuild into its next pass,
so a short burst costs about one more rebuild either way. Cancellation
pays when typing goes on for longer than the rebuilds take: typing 36
characters into a ch12 sentence (8 s), each rebuild runs to its end
before it sees the keystrokes since (the first keystrokes' rebuilds move
every later page: 27.7 of 66 million commands), where with cancellation
the result is there 2.7 s after the last keystroke. A first version
looked at every file served, the TeX tree's included (880), every 20 ms,
and made an isolated 31 s rebuild 2 to 4 s slower; the job's own files
every 50 ms cost nothing measurable.

Coarsening counts how recent a region is in rebuilds that ran to their
end: counted in every rebuild, the 30 stopped by a burst made the next
coarsening merge the regions they had just re-run, and a merged region
replayed alone (`Trace::apply`) can give a state its constituents' last
snapshot does not have (a box register differs by content hash after
merging; §7.12). After that, a rebuild on the course read an empty token
list and panicked. Counted in settled rebuilds, the regions a burst
re-ran stay fine as long as without cancellation. The replay of merged
regions is still to be fixed; it predates cancellation.

Next: the watch's idle work (coarsening, dropping replaced regions, the
copy a save takes) runs on its thread and can delay the pickup of the
next edit by up to half a second; the first edit in a coarse region
re-runs it finely whole (0.6 to 2 s where later edits there take 0.2 s).

**Preview, with the exact build behind it.** A watch session serves a
preview the editor shows at once and the exact PDF, the only thing written
as output. The preview may be briefly wrong, like an IDE's quick type
check refined later: pages are shown as soon as they are built, with holes
shown as their last-build values (or `??`) and repainted when resolved; a
region whose guards failed is still shown if its pages match up to what
never reaches the page; later pages may be laid out approximately for a
first look and are replaced when the exact build confirms them. A preview
gets page deltas over our own protocol, with SyncTeX lookups, instead of
reloading the PDF.

### 7.12 Soundness

Guards are only sound if tracking is complete; nothing here is trusted
without a check.

- **Sanitizer mode** (in the spirit of ASan and TSan): run sequentially
  and, after each region, assert that its predicted or replayed exit hash
  equals the real one; report the first differing cell. On in CI over
  trip, etrip, e2e and the manuals.

  **Built for the tracker** (`PARTEX_SANITIZE=1`; `crates/partex-cli/src/sanitize.rs`,
  `crates/partex-core/src/sanitize.rs`). The job runs as usual and stops
  every `PARTEX_SANITIZE_EVERY` commands (`Tex::set_stop_at`); the
  commands between two stops are a region, and the session's own tracker
  (`intervals.rs`) says what each region read before writing and what it
  wrote. Two checks per region:

  - *shadow diff*: every cell whose value changed across the region (eqtb
    chunks shared with the entry's clone are skipped unread) must have
    been reported written. That includes a cell whose word stayed the same
    while what it names changed in place: a token list, glue (with its
    lineage), a shape or a box, compared by value where the two states do
    not share it (`named_differences`; `PARTEX_SANITIZE_NAMED=0` turns it
    off). This costs about a quarter more; with bug B's fix reverted, it
    names `box 0` for a `\wd0=5pt`;
  - *poisoning*: a clone of the entry gets another valid value in every
    cell in use that the region did not report reading (an integer off by
    one, a catcode moved on, a meaning made undefined or `\relax`, a token
    list, box or shape emptied, glue zeroed or made 1sp, a font's
    parameters and characters changed, a stream closed, the seed changed)
    and runs the same commands against a replay of the files, dates and
    times the real run saw. Its output (the log and terminal with TeX's
    memory statistics masked), its cells and the rest of its state (the
    state hash without the cells, less `cur_val` and the current token,
    dead between commands and full of ids that poisoning shifts) must come
    out as the real run's, and the poisoned cells it did not write must
    keep their poison. A difference is a read that bypassed the tracker;
    delta debugging over the poisoned set names the cell.

  The real run's outputs are exactly a normal run's. Findings, all fixed:
  e-TeX's reassignment check and the save stack reading the replaced value
  (the largest class: every `\catcode`, `\uccode`, `\dimen` or meaning
  assigned the value it had); `unsave` deciding by the current level; the
  font state (`font_param` behind `em`, `ex`, math spacing and accents,
  the interword glue, `\hyphenchar` in line breaking, font expansion's
  links); `\closeout`/`\openout` closing an open stream and the job's end
  closing them all; `\ifeof`, `\read`, the random generator. Clean now
  over the e2e corpus at 60-command regions and over the 295-page course
  at 20000 (3304 regions, 2727 cells read and 168k poisoned per region,
  12.8 minutes against 54 s for a plain run). Not cells yet, and so not checked by
  poisoning: the nest and lists, the page builder, marks and inserts,
  alignments, the input stack and conditionals, the PDF writer, file and
  host queries, interaction and `history` (the "state changed" counts the
  sanitizer prints per section say how often regions change each); the
  session's full-hash confirmation covers them until they are.
- **Machine rebuilds of real documents** (`PARTEX_MACHINE_EDIT` with
  several `;;` edits, `PARTEX_MACHINE_STOP`, `PARTEX_MACHINE_SANITIZE=1`;
  e2e `machine_edits`). Found: a file read back (LaTeX's `.aux` at
  `\end{document}`) was refreshed on replay from the file of that name as
  served at `\begin{document}`, so the `.toc` came out with old page
  numbers (fixed). A region merged by coarsening replayed a box register
  as an earlier region had left it (fixed): `\wd`, `\ht` and `\dp`
  (§1247) change the box a register holds in place, so its eqtb word
  stays and no write was reported. A plain replay did not notice, as the
  region's `Rest` snapshot holds the box as changed under the same id; a
  merged region carries the write of the earlier region that set the box
  and applies it after `Rest`, putting back the old dimensions. Setting a
  box dimension now reports a write of the register (the unit test
  `a_box_dimension_set_writes_its_register`). The tracker sanitizer's
  shadow diff then compared eqtb words, not what they name, so it could
  not see this; it now compares the named values too (above).
  Open: `\dump` copies eqtb, the hash and the fonts into the format past
  the tracker, so poisoning finds the dump region's format changing with
  cells it was said not to read (all 17 e2e cases that dump plain or
  e-TeX formats; `PARTEX_SANITIZE=1 PARTEX_SANITIZE_EVERY=60 partex
  --compat=tex -ini '\input plain \dump'`). Reporting reads of every
  eqtb word, register, hash entry and font there removes the cells first
  named but not `\lineskip` and `\baselineskip`, which still show.
  *Bug C* (fixed): in DVI mode, an edit in section 5 of a 47-page document
  and then one in section 2 shipped section 2's page with the old line.
  The DVI writer keeps a page's bytes in its buffer after the page is
  shipped (§597–§598: written out a half at a time), and the state hash
  left the buffer and the file's position out, as if a splice could
  relocate DVI output. It cannot: a page's `bop` points back at the one
  before by its offset (§640), and a movement is reused only while its
  bytes are in the buffer (§611). So the re-run of section 2 met the old
  run at the next boundary with the new page still buffered, and the old
  region that later wrote the buffer out wrote the old page (and the
  whole file stayed consistent with it). In machine mode (outputs as
  effects, reused as they are) the state hash now takes the bytes not
  written yet and the writer's position (`DviWriter::hash_placement`).
  The checkpoint session keeps the position-free hash, since at a cutoff
  it takes the new run's writer by page. A DVI rebuild after a page
  changes length then runs to the job's end, since every later position
  differs: DVI output is not position-independent, as PDF output is.
  e2e `machine_edits_dvi` fails without it.
  *Bug D*: replaying the regions from the starting state no longer gave
  the final state (on the course, and on the first rebuild of
  `edits.tex`), though outputs were byte-identical to cold, since a
  rebuild runs from recorded states and only the replay composes writes.
  `PARTEX_MACHINE_REPLAYCHECK=1` names the first region whose guards fail
  on replay and what in `Rest`, the input stack or eqtb differs from the
  state it really began in; `=2` names the first region after which the
  replayed eqtb differs from the region's own exit, a write the tracker
  lost. Three causes, all fixed (84b0bf3), each with a unit test:
  - *A name taken for a string.* A macro's input record names the
    control sequence it was called by (§390), a location; `Rest` hashed
    it as a string number. Setting `Rest` from cells imports names as new
    strings, so a location fell among the strings in the replayed state
    and past them in the real one.
  - *Sizes that are not state.* The glyphs used were hashed with the
    length of the PDF writer's per-font table, which a state set from
    font cells extends to every font it is given; `font_ptr` after
    setting `FontOrder` counted the null font; loading a format did not
    report the fonts by name as written, so a region beginning with it
    guarded them at their empty value.
  - *A restore that is a change.* Soft reads take a group's end that
    restores the value saved at the region's entry as no change. After a
    global assignment in the same group, a later local assignment saves
    the location again (§279), and the group's end restores that global
    value first; the entry's never comes back. LaTeX's `\@outputbox` went
    void across a ship region with no write recorded. A second save at the
    same level now makes the location written.
  Open: under `PARTEX_MACHINE_SANITIZE=1` the `modern_watch` e2e case
  fails "the rebuild's output differs from a fresh build's" (before these
  fixes it failed earlier, on bug D's `Rest` guard; with fonts as cells
  off it fails the same way, so it is not the link's relocations). The
  case's outputs are byte-identical without the sanitizer.
  The build sanitizer (`PARTEX_MACHINE_SANITIZE=1`) compares a
  rebuild's output with a fresh build's through
  `Machine::comparable_output`. For TeX that masks the digits of the
  statistics lines about its internal tables (the templates of
  `xtask/src/mask.rs`, `partex_core::statistics_line`). A rebuild's
  string pool counts differ from a fresh run's, and partex does not
  reproduce them, so without the mask the watch e2e failed under the
  sanitizer on "29604 strings out of 469503" against "453". Everything
  else is still compared byte for byte (e2e `modern_watch_sanitized`).
- **Check modes per tier**: `PARTEX_MEMO=check` re-runs every memo hit and
  compares; compiled expansion and intrinsics run alongside the
  interpreter and compare state hashes.
- **Property-based edit fuzzing** (QuickCheck, Hypothesis): random edits on
  the e2e corpus, the PGF/TikZ manual and a copy of a large book; every
  incremental result must equal a cold sequential build byte for byte.
- **Differential runs**: tracked against untracked, chunk-parallel against
  sequential, one deferred quantity or hole kind at a time.
- **Construction**: after the state split (§10), semantic state is private
  to accessors that call the tracker, so an access that is not a cell
  cannot go unrecorded.

### 7.13 Faster expansion

About 60% of a run is reading and expanding tokens (`get_next`, `expand`,
`main_control`), spread along the whole per-token path, so tuning that
path is worth little: the gains come from reading fewer tokens. A
TeX-level profile (`PARTEX_TEXPROF`) of the PGF manual subset: 43.5M macro
calls, pgfmath's expression parser 38% (52% with pgf's other parsing
helpers), the LaTeX kernel 28%, pgfkeys 10%, expl3 4%; the parser's
arguments repeat 90–93% of the time. In order:

0. **Memoized calls**: the innermost regions (§7.3). Break-even as built.
1. **Token cache** (§7.2).
2. **Meaning cache.** Resolve a token's meaning once and cache it against
   its cell's version (an inline cache, as in V8 and Smalltalk). It must
   avoid the load of the meaning's `eqtb` word, not add a version check
   beside it.
3. **Compiled definitions.** A macro definition compiles to straight-line
   ops (argument matching, body substitution, conditionals, `\csname`,
   fused `\expandafter` chains) guarded by `(cell, version)` checks on the
   meanings it inlined. Compiled code works on the interpreter's own input
   and parameter stacks, so bailing out is just stopping. It runs only when
   expansion tracing is off, and bails out before any error.
4. **Traces.** A hot call that does not repeat its arguments is recorded as
   the primitive operations it executes and specialized (partial
   evaluation of the interpreter, as PyPy's meta-tracing does):
   conditionals fixed by guards fold, token shuffles disappear, `\csname`
   resolves through an inline cache, argument scanning becomes a native
   loop. A failed guard pushes the unread tokens back and the interpreter
   continues (a LuaJIT side exit).

**Content-keyed intrinsics** (between steps 3 and 4): native code for one
exact definition, keyed by the stable hash of its parameter text and body
*and* of every definition it reaches, validated at the call like an inline
cache. A document that redefines any of them stays interpreted, so no
library is named. An intrinsic performs every observable effect the
interpreter would, so state hashes and early cutoff are unaffected.

**No library is named** anywhere: targets come from live profiles, and
everything is keyed by definition identity and guards. A native JIT
(Cranelift) is last and native-only.

**Built accelerations** (`Params::fast`, one bit each; the CLI's
`PARTEX_FAST=0` turns all off, `PARTEX_FAST_<NAME>=0` one; off, tex.web's
path runs unchanged). The course (295 pages, pgfplots-heavy: 75M macro
calls, 1.69G `get_next`) is the benchmark, cold, one pass:

- `BULK` (`bulk.rs`): where absorbing reads a token list, a run of tokens
  that needs nothing of `get_next` and that the caller would only store
  is copied at once: an argument's group inside and its tokens up to the
  delimiter (§392, §399), a definition's or token list's body (§477, with
  expansion only non-expandable tokens), `\csname`'s characters (§372),
  and skipped conditional text up to the next conditional (§494). The run
  stops before anything else, and the caller reads that token as before,
  so the caller's state (counts, `align_state`, the input position) is
  what token-by-token reading leaves. 0.99G of the 1.69G tokens go in
  runs; instructions −18% (334.7G to 275.0G), cycles about −9%: the reads
  it removes were cheap and well predicted, the loads of each list's
  tokens and each meaning remain.
- `XREGS` (`xregs.rs`): e-TeX's registers above 255 (pgf allocates its
  registers there) are read from vectors by kind and number, in shared
  chunks, kept beside the ordered map (which formats and the save stack
  go on using). Instructions −1.1%, cycles about −2%.

**Parked** (steps 3 and 4, after the gains above: instructions 334.7G to
261.5G, −22%, cycles about −15%, end to end −5.7%). Where the
instructions went then, on the course:

| function | share |
|---|---|
| `get_next` (the slow path) | 11.6% |
| `get_x_token` | 9.7% |
| `macro_call` | 9.1% |
| `get_token` | 8.4% |
| token list allocation and release | 5.5% |
| `BULK` runs | 4.7% |
| `end_token_list` | 4.3% |
| `expand` (non-macro) | 3.5% |

Next, not built (about 5%): **deferred pushes**. 59M macro parameters are
a single token, and 17M+ `back_input`s push back the token just read; both
push a whole input level (and later pop it) to read one token. A pending
single token kept beside the input stack, read first by `get_next` and
made a real level only when something inspects the stack (`\showcontext`,
error context, `\endinput`, a region cut), would remove most of those
pushes and pops. It must leave the input stack as tex.web's wherever
anything can observe it.

### 7.14 Executor

The core never names a threading library: parallel work goes through the
`Executor` trait (`crates/partex-incr/src/exec.rs`: `join`, `map`,
`width`; one implementation, which `partex-core` re-exports), results in
program order. `Sequential` is the reference;
a work-stealing pool (rayon) serves region rounds, line-break batches and
per-page encoding; cancellation is cooperative (§7.11). No async runtime:
the work is CPU-bound.

### 7.15 Where a rebuild's time goes, and nested units

The runtime's stub rebuilds a 10,000-statement program after a one-line
edit in 0.44 ms (§7.0). Driving TeX, the same runtime took 0.4–1.5 s for
edits that should take milliseconds. This section records the
measurement that located the difference, entirely on TeX's side of the
`Machine` interface, and the design that follows from it. The target is
not "a rebuild costs a cold pass": a one-word edit or an added `\label`
takes milliseconds.

**Measured** on 2026-09-27 on the course: a 295-page LaTeX book,
66.06M commands, a cold machine build of 53–55 s in 282 regions at grain
131,072. The runs used master's binary with in-process rebuilds
(`PARTEX_MACHINE_EDIT`), the rebuild's statistics with per-span timings,
`PARTEX_MACHINE_PARTS=7` (the cells that made each region dirty), and
`perf` over the cold and rebuild windows of one process.

| edit | rebuild | commands re-run | spans: old region, what made it dirty |
|---|---|---|---|
| one word, ch15 | 0.37 s | 149,020 | 185: the edited line |
| `\Cref` added | 0.89 s | 523,872 | 185: the line; 235: `Numbering`; 281: `Glyphs`, `Numbering` |
| `\label` added | 1.34–1.46 s | 696,598 | 185: the line; 199: `\@savsf`; 200: `Rest`; 272: `\@savsf`; 273: `Rest`; 281: `Written(2)` |
| footnote added | 27.2 s | 23.2M | font numbers shifted (now fonts at slots and cells, §7.0: 3.2 s, 1.84M) |
| `\linespread` (every region) | 75.7–80.8 s | 65.8M | all |

Each rebuild also pays fixed costs: replaying the clean prefix
(42–144 ms; not one region at a time, measured below); the splice
(0.8–1.7 ms per region recorded, 0.2 ms since dense ids); and the link
of the whole document (47–64 ms).

**Four causes.**

1. **Regions are atomic.** A region is the only unit that has an entry
   snapshot (where execution can start), guards (a cell is recorded as
   read by the region, not where in it) and a sync point (a
   re-execution stops only at an old boundary). One changed cell
   therefore costs a whole region. The word edit's 149,020 commands run
   from region 185's entry to the next old boundary, around a paragraph
   estimated at 1–3k commands. §7.3 describes recording at candidate
   grain, with regions composed over the records; the implementation
   records at the chooser's grain only.
2. **A cut costs O(state).** A rebuild records what it re-executes at a
   fixed fine grain (1,024, against 131,072 cold), and each cut costs
   5–6 ms.
   - The word edit takes 297 ms with 29 cuts and 159 ms with one
     (`PARTEX_MACHINE_FINE_GRAIN=131072`).
   - The `\linespread` rebuild takes 79.7 s with 4,393 cuts and 56.1 s
     with 280, against 55.5 s cold.
   - The profile of the extra 24 s: state hashing 9.7 s; cell-keyed
     B-tree indexes 4.4 s (a `Line` cell carries its file's path bytes,
     and a region has ~860 guards); allocation 2.8 s; freezing the token
     store 2.5 s; page faults 1.8 s; cut bookkeeping 1.3 s; clones 0.9 s.
     TeX's own execution takes 0.9 s more.

   Coarse regions are a compromise forced by this cost, and they make
   cause 1 expensive.

   Since then, measured again on the same rebuild (perf with frame
   pointers, 4,399 cuts, after the hash table's names and the string pool
   left `Rest`): a cut costs about 7 ms of work besides TeX's own. The
   snapshot (`take_snapshot`) is 2.9 ms: the freeze (`Tex::commit`: the
   token store, `JVec` and `Flat` shadows) 1.3 ms, the engine's clone
   1.2 ms, the `Rest` hash itself only 0.36 ms (it was the largest piece
   while every string the run made was hashed at each cut). The guards'
   versions (`mcell_content`, about 860 per region) take 0.7 ms, and the
   splice's index took 2.1 ms before its dense ids (0.21 ms after). Two
   O(state) pieces of the snapshot are O(change) now: the virtual object
   table sits in shared shards (`cow::ShardMap`), and a `Flat` borrowed
   mutably not since its commit is not compared again when cloned; the
   rebuild went 364.9 → 360.4 G instructions. What stays O(state) per cut
   is the freeze's scans: the token store looks at every list's flag, and
   a `Flat` compares every segment with its shadow once. Marking changed
   chunks on the token path was measured and dropped (it cost the cold
   build 1.2%, more than the scans it saved). A checkpoint that costs
   O(changes) therefore needs changes recorded where they are made
   cheaply: the journal's chunk writes (as `JVec` has), a `Flat`'s
   written range, and token lists' reference counts kept apart from
   their contents, so that a macro call, which changes a count, does not
   make its list look changed.
3. **Lump cells.** `Rest`, the numbering log (`Numbering`, versioned by a
   hash of every event) and each `\write` file (`Written(k)`, one byte
   buffer) are single cells, so any change invalidates every reader.
   - The `\Cref`'s one new link object re-runs a region 50 regions
     later. That region observes object numbers through `\pdflast…`
     (pgf's opacity, shading and xform objects) only to write `N 0 R`
     into PDF bytes: 360k of the 524k commands.
   - The `\label` re-runs four coarse regions, through a scratch
     register (`\@savsf`, LaTeX's saved space factor) and `Rest`.
4. **Identities by creation order.** A hash slot, string, destination,
   font or object numbered in creation order renumbers everything after
   one early addition. Two new auto-expanded font instances in the
   footnote's paragraph shifted about 190 later fonts (27.2 s). Hash
   slots, destinations and fonts now take identities independent of
   order; object numbers are virtual (§7.6).

Holes play no part: rebuilds are sequential and plant none (§7.5).

**The target: nested units with dynamic dependencies.** *(Superseded
on 2026-09-27 by §7.16, which keeps the goal and changes the means:
units are clean-point stretches, the page builder is replayed as a
fold, and checkpoints are shared roots. The measurements in this
section remain the baseline.)* This is the
known technique of Adapton, salsa's red-green algorithm, "Build Systems
à la Carte" (dynamic dependencies with early cutoff), Julia's method
backedges and CPython's version-tag guards. The runtime already records
read sets dynamically and keeps backedges. What TeX's side must supply
is the right units and the right cells.

- **Units** are the dynamic extents of TeX computations with a clear
  input and output, and they nest: a region contains pages (the output
  routine), which contain paragraphs (an hlist becoming lines), which
  contain boxes, macro expansions (an `\edef`, a `\write` expanded at
  shipout) and pictures. Today's region is the top of the tree.
- **Regions are hierarchical and chosen dynamically,** not fixed at
  "every chapter", "every page" or "every paragraph".
  - Execution produces the tree of candidate extents: groups, boxes,
    macro expansions, paragraphs, output-routine calls, `\write`
    expansions, pictures.
  - A policy (§7.8) picks which nodes become memoized units and
    checkpoints. It weighs measured cost (running a node, checkpointing
    it, the size of its read set) against observed volatility (where
    edits land, what they invalidate).
  - Nodes split where edits land and merge where nothing changes, as a
    JIT refines hot code. Paragraphs and pages in the walk-throughs below
    are examples of what the policy may pick, not a fixed grain.
- **The page IR is built from functional blocks.** partex owns its page
  IR (§5.6), so it can be designed around blocks with stable identities
  rather than monolithic pages: a paragraph's lines, a display, a float,
  a footnote, a header or footer, a box built once.
  - The page builder places blocks. Shipout emits each block's content,
    and the link assembles pages from blocks.
  - A renumbered folio is then one footer block. A changed paragraph
    re-emits its own block, and its page is re-assembled from the
    others as they were.
- **Each unit records its own reads**, dynamically, over fine cells, and
  its outputs.
- **Revalidation is red-green**, top-down. A unit whose recorded reads
  all hold is reused without running. A unit that read a changed value
  re-executes, and if its outputs come out unchanged, its dependents stay
  valid: early cutoff at every level.
- **Checkpoints cost O(what changed since the last one).** This needs
  incremental hashing, dense integer cell ids for the indexes, and the
  token store frozen incrementally. Fine checkpoints then pay everywhere,
  and granularity becomes a policy (§7.8) instead of a compromise.

  *Dense ids for the indexes (done).* The readers and writers indexes
  hold each cell's region keys by a small id (`build.rs`, `CellIndex`):
  the machine's dense number (`Machine::index`: eqtb words, registers,
  hash links, fonts) looked up in pages of 1,024 allocated when used,
  else the cell interned in an open-addressing table by its hash (a line,
  a file, a PDF object). A splice stages its changes, (id, key, added),
  and applies them at the end by one merge per cell touched, where a cell
  every region reads (`Rest`, the font parameters) had one key shifted in
  for each region put in, O(regions) each time. Cells stay the keys of
  traces and of the store; ids are never shown or saved. On the course
  (the same edits in one process, outputs byte-identical to a cold
  build):

  | | before | dense ids | and staged splices |
  |---|---|---|---|
  | `\linespread`, all re-run: splice per cut | 1.94 ms | 0.81 ms | 0.21 ms |
  | `\linespread` rebuild | 85.8 s | 76.6 s | 71.6 s |
  | one-word edit (28 cuts): splice | 52.8 ms | 18.6 ms | 19.2 ms |
  | one-word edit | 0.515 s | 0.447 s | 0.464 s |
  | second edit nearby (44 cuts): splice | 55.2 ms | 28.5 ms | 24.7 ms |
  | indexes of the loaded build (282 regions) | 0.61 s | 0.35 s | 0.35 s |

  A small rebuild's splice is mostly the removal of the coarse region it
  replaces, O(the cells that region touched: tens of thousands). Of the
  per-cut costs above, state hashing, allocation and the token store's
  freeze remain.
- **A re-execution resumes and stops inside a region.** It resumes at the
  checkpoint before the first read of a changed cell (regions record the
  position of each cell's first read). It stops at the first old
  checkpoint whose state digest matches, and reuses the old suffix's
  effects and writes (effects indexed by checkpoint).
- **What a checkpoint inside a region must provide** (the interface
  between the runtime and whoever makes checkpoints cheap): a position
  (`Machine::at`); a handle to restore the state from, whose cost is
  O(what changed since the last one); a digest of the state, incremental
  in the same way; and the offsets of the region's effect list and write
  set there. A trace then records, per guard, the checkpoint before which
  its cell was first read, and at each checkpoint the cost so far and the
  digest. Paragraph starts in outer vertical mode and page ships are the
  natural places: there the save stack is empty, so a scratch register
  saved in a group (`\@savsf`) is no read across the checkpoint. Placing
  today's coarse boundaries only there (parked on `cheap-cuts-wip`)
  already removes the `\label` edit's `\@savsf` chain (1.34 → 0.46 s),
  but makes a `\Cref` re-run further (0.81 → 1.23 s), since old
  boundaries then fall less often right after a page is shipped: the
  checkpoints must be fine, not only well placed.

**Fine cells.**

- **`\write` streams are entry logs.** Each `\write` executed is an entry
  with a stable identity: its position and a count, as for virtual object
  ids. The file's bytes are generated at the link, identical to
  pdfTeX's, and reach disk only for compatibility (latexmk, BibTeX,
  makeindex, `xr`). Reading a file back (`\@input{\jobname.aux}`, `.toc`,
  `.lof`, `.out`) is one unit per entry. Each has its own reads (the
  meaning of `\newlabel` at that point, which hyperref and cleveref
  redefine, and the catcodes) and writes (`\r@key`). Nothing is
  LaTeX-specific.
- **pdfTeX's numbers are holes resolved at the link.** `\pdflastobj` and
  its relatives give a relocatable value, and its digits reach the PDF as
  a relocation. The real number is forced (a read of the numbering) only
  where TeX uses the integer: arithmetic, `\ifnum`, typesetting, a `\write`
  to a file other than the PDF.
- **Fonts sit at slots by identity** (name, area, size), from a registry
  shared by the whole build and persisted with it. `/F` names are
  relocations. Built (§7.0, "A first use moved"), with the hash table's
  names and the destination names as cells too.
- **`Rest` shrinks to what nothing reads.**

**Stages.** A paragraph's line breaking is a function of its hlist and
parameters. The page builder consumes the resulting items as values, and
shipout and the link consume pages.
- **Token-consumer tracking** records what consumed each source token:
  the main loop appending characters, macro-argument scanning, `\write`,
  `\mark`, verbatim, math.
- **A confined edit** is one whose changed tokens all went to the main
  loop as characters. It re-expands nothing: the paragraph's lines are
  rebuilt, and the page builder re-runs over the existing items until the
  page breaks realign. The output routine runs only for pages whose
  content changed, and only those pages are shipped and relinked.
- **Anything not confined** re-executes from the paragraph's checkpoint.

**Fixed costs follow the change.**
- A rebuild starts from the dirty unit's entry checkpoint instead of
  replaying the clean prefix.
- The splice touches only the changed units.
- The link is incremental: the objects and pages that changed, and the
  xref. A `\label` changes no PDF bytes, so nothing is relinked.

**Walk-throughs (targets).**

- **One word.** One paragraph unit re-executes (1–3k commands). If its
  lines keep their heights the page builder cuts off, and one page is
  re-shipped. Milliseconds.
- **`\label`.** The paragraph unit (one more zero-size whatsit, the same
  lines) and that page's output routine re-run. The `\write` expansion
  adds one entry, and `\end{document}`'s read-back runs one entry unit,
  which defines an `\r@key` that nothing reads. No PDF bytes change.
- **`\Cref`.** The paragraph gains a link annotation: one new object and
  a relocation. Later readers of object numbers see relocations, not
  numbers, and the link renumbers.

**Output modes: exact and fast.** Byte-identical output is a mode, not a
constraint on every build (the user's decision, 2026-09-27). Both modes
typeset identically; they differ only in how the PDF is encoded.

- **exact**: byte for byte what pdfTeX writes. This is the oracle test
  and the default of the `pdflatex`-compatible command line.
- **fast**: identical content, not bytes. The PDF has the same objects,
  and the same decompressed stream bytes, as pdfTeX's. The objects may
  be compressed differently or not at all, packed into object streams
  differently, numbered differently, or written as incremental updates.
  This is the default of `partex watch` and `partex build`; `--exact`
  asks for bytes.
  - Where TeX itself observes a number (`\pdflastobj` and its relatives
    typeset or compared), pdfTeX's semantics still hold.

**Checks** (`scripts/pdfcheck`):
- **`uncompressed DOC`** builds with pdfTeX and with partex, both with
  `\pdfcompresslevel=0` and `\pdfobjcompresslevel=0`, the same output
  path and a fixed `SOURCE_DATE_EPOCH`, and compares the bytes. That
  checks everything but deflate and object-stream packing, `/Length`
  values and xref offsets included. The e2e LaTeX document (171,207
  bytes) and the 299-page course (10,079,539 bytes) are byte-identical
  this way.
- **`same A B`** compares content: qpdf's QDF form, with streams
  decompressed, object streams unpacked, keys sorted, objects renumbered
  in traversal order, and `/ID` ignored.
  - Re-encodings of the course at compression levels 1 and 9, with and
    without object streams, compare equal.
  - A one-byte change in a content stream is caught.

Exact mode stays tested byte for byte against pdfTeX. Fast mode is
tested with `same` against the same oracle, so nothing is loosened.

**What fast mode buys:**
- Deflate leaves the critical path: `watch` writes streams uncompressed
  or at level 1, and can compress the final file in the background.
- Page streams are spliced from blocks at deflate full-flush boundaries.
  The decompressed bytes are unchanged, so `same` holds.
- Object ids stay stable across edits, so an added object does not
  renumber later ones.
- Small edits can be written as PDF incremental updates: the changed
  objects and a new xref section appended.

**Latency targets.**

The rule: latency follows the size of the change in the output (the
pages whose bytes change), never the size of the document. Bookkeeping
(replay, splice, link, store) is O(change).

Some edits cascade by TeX's own rules. A page added early renumbers every
later page, and every later header and footer is typeset again. Then the
work is inherent, but it costs little per page and runs in parallel. The
page in view is published first (§7.11), and the rest streams in.

| edit | what runs | target, any size of document | 2026-09-27, course (299 pages) |
|---|---|---|---|
| whitespace or comment that tokenizes the same | tokenize the line, compare the tokens | < 1 ms; nothing written | the fast path exists (§7.2); unmeasured in machine mode |
| a space or word inside a paragraph | that paragraph's line breaking; its page re-shipped if its lines keep their heights | < 5 ms to the PDF's bytes | 0.37 s |
| `\label` added | the paragraph (one zero-size whatsit), that page's output routine, one `.aux` entry, one read-back entry; no PDF bytes change | < 2 ms | 1.3–1.5 s |
| `\footnote` added, or a paragraph deleted | the paragraph, then the page builder over the existing items until the breaks realign, then the output routine and re-shipping for each changed page. After a deletion, also the numbered items whose digits changed, and their `\ref`s | about 1 ms per changed page, in parallel; typically one chapter, 20–50 ms; the page in view < 20 ms | footnote 3.2 s (27 s before fonts at slots) |
| any edit that renumbers 100,000 later pages | each later page's header and footer typeset again, its stream emitted and deflated again (the bytes equal pdfTeX's, so streams are not patched), the xref rewritten | 0.5–1 ms of CPU per page, so 100k pages take 2–4 s on 24 cores; the pages in view < 20 ms | — |
| restart with nothing changed | the input stamps and the stored output hashes | < 50 ms | 1.0 s |

Today's fixed costs grow with the document. The link is about 0.2 ms
per page. Replaying clean regions does not grow with the edit's
position. A run of clean regions is replayed as one restore of the last
one's exit snapshot, patched with the cells that differ (`replay_span`,
`Machine::replay_exit`). What is left per region is re-adding each one's
share of the accumulating cells, and it does not show:
- On a generated 1,287-page article, one-word edits at 1%, 50% and 99%
  of the document replayed in 392 ms (12 separate replays), 38 ms and
  17 ms. The last replayed 617 regions in one span.
- Taking the accumulating cells from the snapshot instead (when none
  has changed since the snapshot was recorded) gave 16.8 against
  17.3 ms, so it was not kept.

The cost is the number of separate replays, where dirty and clean regions
alternate, times the cost of one restore, 12–33 ms. That restore is
O(state): the token store thawed from the snapshot, the state hash of the
entry, and the font tables cloned. It grows with what a document holds,
not with where the edit is, and making it O(changes) is the checkpoint
work above. On the course, one-word edits in ch01, ch15 and ch28 each
replayed 49–64 ms over 2–3 restores.

Measured on the course since (12 one-word edits made and taken back in
ch15, perf with frame pointers): a replay was 21% of a rebuild. Thawing
the token store was 10%: a restore took the running store's lists only
for chunks it shares with the snapshot by address, and a snapshot of the
old run shares few with the rebuild's store. Hashing the restored
state's `Rest` at the seek was 6%. Cloning and dropping snapshots was
most of the rest. Three of these became O(change) (120de28):
- the running store's lists equal to the snapshot's, by hash and fields,
  are taken as they are where chunks are not shared (143k lists taken,
  25k copied over the 12 rebuilds; `PARTEX_MACHINE_THAW_CONTENT=0`);
- a replay's `Rest` version is the trace's, since the patches after the
  restore change only cells (`PARTEX_MACHINE_KNOWN_REST=0`; checked by
  hashing under the sanitizer or `PARTEX_MACHINE_VERIFY_REST=1`);
- the PDF object table's lookup trees and destination names are shared
  between snapshots and copied when they change.
The replay went 26.3 → 23.4 ms per rebuild, a rebuild 96 → 90 ms. What
stays O(state) per restore: dropping the running engine, and a
snapshot's unique parts, cloned when it is taken and freed when it is
replaced (the font tables, the PDF writer's per-font table: a thousand
entries each on the course). Sharing those, and keeping a snapshot that
a replay seeks to until the region that starts there has used it, are
what is left.

**A `\label` in watch mode** (course, ch15, measured on 88b1952): 4.4–6.9 s
end to end in two passes, against a target of < 2 ms.
- *Pass 1, the edit* (2.4 s): the edited line's region (365k commands,
  0.48 s), four regions dirty only through `Rest` (0.89 s), two through
  the scratch count `\@savsf` with a `Rest` region after each, and the
  last region through the `.aux` read back at `\end{document}`
  (`Written(2)`).
- *Pass 2, the `.aux` changed* (4.3 s):
  - The region that reads the `.aux` at `\begin{document}` is dirty
    through 852 `Line` cells. The new `\newlabel` line shifts every later
    line's number, and a line cell is keyed by it: 132k commands, 1.07 s.
  - Three regions, and one after them through `Rest`, are dirty only
    through `Link(433598)` (2.3 s). This is the hash-table slot that the
    new name `\r@partex:test` was linked into; other names' lookups
    probe through it.
  - The read back at `\end{document}` and the job's end take 85 ms.
- *Virtual ids at the job's end* (fixed): the end-of-job step starts
  while the `.aux` is read back. Its virtual ids were seeded with that
  position, so an `.aux` one line longer renamed all 407 objects the end
  of the job makes. That changed the numbering log and every effect
  naming them, which is one reason links could not be reused. Those
  objects are now seeded by a constant and named by their order (0 of
  543 ids differ, from 407).

The smallest changes that make pass 2 re-run only what reads the new
entry, in order of payoff:
1. *Name lookups guard on the name, not on the probe path* (built,
   86deeeb). The regions read `Link(433598)` because entering a name
   walks its hash chain to the end, and the new `\r@partex:test` had
   been appended to that chain. With `Tex::probe_names`
   (`PARTEX_MACHINE_PROBENAMES=0` turns it off) the names a run makes
   are not chained at all. A name goes to its hash code's slot (§259) if
   that is free, else to the first free place of its probes in
   `hash_extra` (the places its characters pick); names are never taken
   away, so a name is found there or before the first free place, and a
   name missing is missing there. The format's names stay in their
   chains, which the run no longer changes. A lookup reads the names at
   its start slot and at the places it probes, up to the name or a free
   place, and no links: a name made elsewhere changes no lookup of
   another, and a lookup that found a name missing read the place where
   it is made later, so the `Name` cells are not needed. When
   `hash_extra` is full the table overflows (a capacity limit, not
   tex.web's point). Measured under `partex watch` with a fresh store: the
   `\label` 4.47 → 3.17 s, its second pass 2.04 s (133 regions) →
   0.85 s (51); taken back 3.98 → 3.42 s; the label's PDF byte-identical
   to a plain two-pass run. What stays: a name's slot is still part of
   its eqtb cell, so a lookup that reads a start slot depends on that
   slot's meaning too; names in a cell of their own would take that
   away.
2. *Lines of a job-written file keyed by entry, not by number* (1.07 s,
   toward `\write` streams as entry logs). Looked at closely, this is
   neither sound as first stated nor small:
   - *Sequence guards.* A region that read an `.aux`'s entries in order
     depends on the sequence, not on the set. So a guard must read "the
     entry after E is F", and the reader's position must be the last
     entry read, not a line number or byte offset. That position is in
     `Rest` (the input file's position, `statehash.rs`'s served files
     counted by lines read), in the boundary identity (`Machine::at`
     hashes the line), and in how a restore puts an open file back at
     its line (`restore_rest`).
   - *The line counter becomes an additive cell.* A reused region's
     exit holds the old run's absolute line number, one off after an
     insertion. The counter must be a cell that a region adds to, like
     the accumulating cells, and every read of it must be tracked:
     `\inputlineno`, error contexts (`l.N`), box warnings (`at lines
     N--M`), and the rest, 39 uses in 20 engine files.
   - *The first pass after a cold build gains nothing.* The `.aux` read
     at `\begin{document}` is inside one coarse region (131k commands)
     with no boundary inside it to resynchronise at. Only a refined
     region, or a boundary per entry in the cold build (1,735 more cuts
     for the course), lets a re-run stop after the new entry.
   - *It touches machine.rs throughout* (cells, `get`/`set`, `at`,
     `restore_rest`, the tracker), `statehash.rs`, `machine_store.rs`
     (the store layout) and the engine's line counter. That is days of
     work, overlapping the name-lookup tracking.

   Measured instead: growing the re-recording grain (`Config::grow`,
   `PARTEX_MACHINE_GROW=2`, off by default) takes the `\label` from
   4.37 to 3.97 s and its removal from 3.93 to 2.57 s, with PDFs
   byte-identical to cold.

   *The series, in order* (planned 2026-09-27; each step is sound and
   mergeable alone, and says what shows it works):
   1. *A region per input file read* (built first, below). A boundary
      where an input file begins or ends is preferred for a cut
      whatever the grain (candidate level 2; `Config::file_cut`, the
      least cost such a cut needs). The `.aux` read at
      `\begin{document}` is then a region of its own, and a pass that
      re-runs it stops at the file's end, where the state is the old
      run's but for the entries' definitions: the run leaves the read
      at the same line of the main file, so nothing in `Rest` differs.
      Shows: the `\label`'s second pass in watch mode. Built
      (`c543b12`): a boundary is level 2 when the file being read (its
      depth and name) differs from the last boundary's, and the cut
      needs 16,384 commands (`PARTEX_MACHINE_FILE_CUT`, 0 turns it off).
      On the course a cold build has 464 regions for 282 and costs 1.3%
      more instructions (4.3% at 2,048, for about the same edits); under
      `partex watch` the `\label` takes 1.96 s for 3.17 (its pass 2
      0.50 s for 0.85) and its removal 1.66 s for 3.42. Pass 2 still re-runs every region from the new
      entry to the end of the file, because the lines after it moved:
      that is step 3.
   2. *Positions in a job-written file by entry* (tried 2026-09-27 and
      dropped). Counting the lines left instead of the lines read in
      `Machine::at` lets a run past an inserted line find the old run's
      boundaries after it, but the keys of the boundaries *before* it go
      stale, since the file's total moved. A clean region before the
      line is replayed (its guards count from the start and hold), and
      its recorded entry no longer matches the state it is replayed
      onto: the sanitizer's chain check fails (`machine_edits`'s
      sanitized run, which now inserts lines, shows it). Doing the same
      in `Rest` would dirty every region before the line instead, and
      `restore_rest` from the end with `Rest` from the start would put
      those regions' files back at the wrong line. No count is stable on
      both sides of an insertion, and keys by content are not unique
      (blank lines, `\]`), so a sync by content can replay a region at
      another place whose lines its cells did not check.
   3. *Positions renamed through the diff* (the replacement for 2–4,
      not built). When a served file changes, the host knows the old and
      the new contents, so the lines both have map old numbers to new
      ones. Before a rebuild the old build is renamed: every line number
      that names a position (in boundary keys, `Line` cells, the open
      files' positions and `line` in `Rest`) is mapped. Regions that
      read only unchanged lines then mean in the new file what they
      meant in the old one, keys stay unique, and the regions still
      chain. A line number read as a *value* (`\inputlineno`, the `l.N`
      of error contexts, "at lines N--M": 39 reads in 20 engine files)
      is a cell that is not renamed, so a region that printed one re-runs
      when it moved. The parts:
      - `Rest` without absolute positions in served files (built,
        behind `PARTEX_MACHINE_RENAME=1`, off by default). With it on,
        the machine's `Rest` leaves out `line`, the line stack and the
        lines read of every served file (`statehash::POSITION_CELLS`).
        They are `MCell::Positions`, whose value (`Positions`) keeps the
        numbers with the served files' names by slot, so a rename can
        map them. Every region reads it at its start and writes it at
        its end, as it does `Rest`. Setting it (after `Rest`, and in
        `replay_exit` after the snapshot) puts the counters back and
        reads each served file on from its line. Alone it changes no
        reuse: the cell holds exactly what `Rest` left out. On the
        course, same process: a comma in ch15 0.189 s off, 0.181 s on;
        a paragraph split there 1.800 s off, 1.767 s on. Both have the
        same regions and commands, and the outputs are identical to
        plain runs. It enables the rename: the numbers `Rest` hashed
        are now values a rebuild can map and re-version. `Rest` still
        holds line numbers that the engine copies, the nest's
        `mode_line` (§213), `if_line` and `skip_line` (§489, §493) and
        `pack_begin_line` (§661). A region that begins inside a
        paragraph or an open conditional after the inserted lines
        still differs through them.
      - The copies of `line` (above) join `Positions`, tagged with the
        file each was taken in, and the line-number value cell, with
        reads through an accessor. A copy is a position, renamed with
        the rest, while printing one (`\inputlineno`, the `l.N` of
        error contexts, "at lines N--M") reads it as a value, which is
        not renamed.
      - A key made of a hash and the file positions
        (`Machine::Boundary` becomes a struct).
      - `Build::rename(cells, boundaries)`: keys, guards, writes and the
        readers' index renamed for the regions of a changed file
        (partex-incr).
      - `restore_rest` maps a snapshot's positions through the edits
        since its generation.
      - The watch's and `PARTEX_MACHINE_EDIT`'s changed cells become
        the lines that changed plus the mapping (`machinehost.rs`,
        `changed_cells`).
      This covers every file read by lines, not only job-written ones.
      On the course a comma added in ch15 rebuilds in 0.18 s (1 dirty
      region, 76k commands), while splitting the same paragraph in two
      (two lines inserted) takes 1.80 s (13 dirty regions, 1.73M
      commands): the rest of the chapter re-runs because every later
      line moved. The Enter key costs ten times a letter, and the
      `.aux`'s new entry is the same problem. Shows: the new paragraph
      rebuilds like the comma; `machine_edits` sanitized.
   4. *A boundary per entry during the read-back*, once a cut costs
      O(changes) (1,735 cuts on the course cost 8–9 s cold today), or
      at entries only in regions a rebuild re-records. Shows: pass 2
      re-runs the new entry's region and the `\r@key` readers.
3. After these two, pass 2 would re-run the new entry's unit, the
   regions that read `\r@partex:test` (none, for an unreferenced
   label), and the read-back at `\end{document}`. That is a few
   milliseconds plus the link, which is taken again when no page
   changed (below).

**The link.** A watch links the whole document after each rebuild:
25–43 ms on the course (58 ms for a `\Cref`, whose new objects miss the
deflate memo), plus 1.5–3 ms to write the files. Of that:
- Resolving the virtual object numbers takes 20–27 ms. It is one pass
  over every effect that clones them, and it assembles 10.1 MB of stream
  data in 407 streams, whose deflate is memoized by content (3 ms for
  the hashing).
- Object streams and the xref take 2–8 ms (16.5 ms with memo misses).
- Copying into the file buffers takes about 2 ms.

*Taken again (done, `PARTEX_MACHINE_LINK_REUSE=0` turns it off).* A
splice compares the effects it takes out with those it puts in, which
costs what was replaced (`Build::take_effects_changed`). A watch whose
build's effects are unchanged since its last link writes that link's
output again, and only the `\write` files are looked at. It is exact
by construction, since `effects::link` reads nothing but the effects.

It hits less often than hoped. Of the four links a `\label` makes, it
took again only one, 26 ms saved. In the other three the effects did
differ, though the PDF did not:
- The log gains or loses "Label(s) may have changed", so the effects
  differ per file: the PDF's are the same, the log's are not.
- The regions re-run when the edit is taken back create their objects
  under other virtual ids. They resolve to the same pdfTeX numbers.

Next, in order:
- compare per file, up to a renaming of virtual ids, and link only the
  files that changed (the log alone costs next to nothing);
- keep the previous link's per-region resolved bytes and resolved
  numbering, so that resolving touches only the changed regions, with
  offsets and the xref by prefix sums. *Resolving done*
  (`effects::link_cached`, `PARTEX_MACHINE_LINK_CACHE=0` turns it off):
  - The watch keeps each region's effects with pdfTeX's numbers, by
    region key.
  - A region is taken again when the build did not put it in since the
    last link (`Build::take_touched`; after a renumbering, every region
    counts as put in), and its entry is the same: the object-stream
    counters (`sys`, `cur`, `idx`), the file whose stream is being
    filled, and the numbers it writes (objects' and fonts', found by a
    look at its effects, not their bytes).
  - The layout takes the cached regions' effects as they are.
  - Exact by construction: a region's resolution reads nothing else. A
    stream whose length and bytes fall in different regions resolves
    the link whole (not seen).

  On the course, same binary (53b54ef with this; link time, cache off →
  on; PDFs byte-identical to cold, `scripts/pdfcheck same` too):

  | edit | off | on |
  |---|---|---|
  | one word (17 of 479 regions resolved) | 35.7 ms | 16.0 ms |
  | taken back (2 resolved) | 39.4 ms | 13.5 ms |
  | `\label`, both passes | 29.2 + 27.2 ms | 23.2 + 13.0 ms |
  | `\label` taken back | 26.4 ms | 13.2 ms |
  | `\Cref` (467 of 682 resolved: the numbers moved) | 41.7 ms | 35.4 ms |
  | `\Cref` taken back | 27.8 ms | 21.9 ms |

  The rest, about 13 ms, is the layout over every effect (object streams,
  the xref), the copy and the numbering's replay. A `\Cref` moves every
  later object's number, so its regions' bytes do change: exact output
  costs O(document) there, until numbers are relocations the layout
  writes.

**A restart with nothing changed** should answer from the input stamps
and the stored hashes of the outputs as written, without loading the
final state (§7.9). It measured 1.02–1.08 s, of which the final state's
decode was 670–690 ms, CPU-bound rather than waiting on I/O:
- 57,647 reads averaging 1.45 KB each;
- memory growth;
- the whole 42,270-entry font map rebuilt as separate values, for 29
  fonts used.

### 7.16 The unit model

Superseded in its targets by §7.17 (2026-09-28); its measurements
remain the baseline, and its harness and gates remain the method.

The second design of the execution model, made on 2026-09-27 after
§7.15's measurements and two readings of master `41bc504`. It replaces
the *targets* of §7.0–§7.15 where they differ; those sections stay as
the record of what was built and measured, and every number in them is
still the baseline. In one sentence:

> TeX's execution is a sequence of **units** cut at **clean points**;
> units exchange state only through **cells** and through one value
> stream, the page builder's contributions, which the runtime **folds**
> itself; and a **checkpoint is a set of shared roots**, not a clone of
> the engine.

**Why the first design stalled.** The runtime rebuilds its stub after
a one-line edit in 0.44 ms because the stub hands it small cells and a
state that costs nothing to snapshot. TeX's adapter hands it the
opposite, on four counts, each confirmed in the code:

1. *Cuts fall anywhere a command is read from a file, including inside
   a paragraph.* The only test is that the top input level is a file
   (`run.rs:68`, checked at `big_switch`, `maincontrol.rs:111`); nothing
   looks at the mode, the group level, the list or the page. Runs of
   letters never return to `big_switch` (§1034–1040), so inside a
   paragraph the candidates are the interword spaces and the commands.
   A fine cut therefore lands mid-paragraph, where the current list, the
   page, `mode_line` and the buffer differ between two runs, and the
   `Rest` guard at the next boundary fails. Early cutoff happens only
   where *everything* coincides, which in practice is `\clearpage`: a
   footnote re-runs its chapter (1.84M commands), a `\label` four coarse
   regions.
2. *The page builder is one struct hashed whole into `Rest`, and every
   observable of it that TeX code reads is untracked.* `Builder`
   (`partex-engine/src/builder.rs:112`) holds the page list, `so_far`,
   the insertion list, `last` and `contents`; the reads of `\pagegoal`
   … `\pagedepth` (§421, `scan.rs:268`), `\insertpenalties` and
   `\deadcycles` (§419, `scan.rs:259`), `\lastskip` … `\lastnodetype`
   when the list is at its head (§424, `scan.rs:410`), `\prevdepth`,
   `\spacefactor` and `\prevgraf` (§418, §422, `scan.rs:225`) and the
   marks (§386, `expand.rs:61`) go straight to the fields. Only
   `\box255`, the insertion boxes, `\outputpenalty` and the builder's
   parameters are cells. So the runtime cannot tell "this region read
   the page" from "this region appended to the page", and any layout
   change makes every later region dirty. The output routine has no
   boundary of its own: it runs from a token list (§1025,
   `page.rs:439`), where the machine turns every candidate into
   `Continue`.
3. *A cut clones the engine.* `Tex` is `#[derive(Clone)]`
   (`tex.rs:54`), and `Snapshot::of` (`machine.rs:1215`) is commit,
   clone, then a compare of every node on the page (`Lists::share`,
   `machine.rs:1159`). Per cut: two scans of all token lists
   (`tok.rs:187`, `tok.rs:228`) with a memcmp of every macro *expanded*
   since the last cut, because the reference count sits inside the list
   record (`tok.rs:29`) and `macro_call` bumps it through `get_mut`
   (`expand.rs:571` → `input.rs:579` → `tok.rs:628`); a memcmp of each
   `Flat` over its whole allocation, not its used prefix
   (`flat.rs:72`); a copy of every owned container (the object stores,
   the sparse registers, `xeq_level`, twelve per-font vectors, the PDF
   font writer's trees, marks, exceptions, the memo tables); and a
   from-scratch hash of the save stack, the primitive tables, all
   `hyph_word` entries and the PDF writer (`statehash.rs:1520–1824`).
   That is the 5–7 ms of §7.15, and it forced regions to be coarse
   (131k commands), which is what makes cause 1 expensive. Per
   restore: the snapshot is cloned again (`machine.rs:1233`), eqtb, hash
   and the save stack are thawed whole (`journal.rs:127`), the token
   store list by list (`tok.rs:416`), and the tracker's slot arrays,
   18 bytes for each of about a million slots, are reallocated and
   zeroed at the first step after every `set` (`machine.rs:2667`,
   `machine.rs:244`), of which a replay does one per patched cell.
4. *Positions are absolute line numbers* (`MCell::Line(file, k)`,
   compared by index in `changed_cells`, `machinehost.rs:720`), so
   pressing Enter costs ten times a letter.

Where a one-word edit's 0.25 s went (in process, before this design):

| piece | cost | cause |
|---|---|---|
| TeX re-executed | 149,020 commands, about 40 ms | 1: run from the coarse region's entry to the next old boundary; the paragraph is 1–3k commands |
| 29 cuts | 150–200 ms | 3 |
| 2–3 restores | about 50 ms | 3 |
| link | 16 ms | layout over every effect |
| watch loop | +150 ms | poll, diff, idle work |

About 85% is bookkeeping, and the TeX part is fifty times the
paragraph. Holes and rounds play no part: rebuilds are sequential and
plant none. What §7.0 did over the last days (strings, names, fonts,
object numbers, destinations, positions as cells) peeled one thing at a
time out of `Rest` after it was caught cascading. This design replaces
the lump.

**Nothing is keyed on a control sequence's name.** Dependencies are
recorded at the state accessors as the engine runs: a read of
`\lastskip` is tracked because §424 reads the builder's `last` field,
whichever macro got there. Clean points are tested on the mode, the
nest depth, the list and the group level, never on what `\section` is.
The `.aux` read-back becomes one unit per entry because each of its
lines is a clean point, not because the code knows `\newlabel`. Names
in this section (`\@savsf`, `\section`) are examples from measurements.

#### 7.16.1 Clean points

A unit boundary is one of:

- **an outer clean point**: `nest_ptr = 0`, vertical mode, the
  contribution list empty, a file on top of the input stack (token
  lists may wait below it, as today), and no output routine active, in
  a **fixed group context**: any group level qualifies. LaTeX reads the
  `.aux` file inside `\begingroup … \endgroup` (`\document`), and every
  environment (lists, theorems, proofs, figures, beamer frames) is a
  group. The save stack is in `Rest` (`statehash.rs`, section "tables":
  `save_ptr`, the entries by content, then `cur_level`, `cur_group`,
  `cur_boundary`), which every region's entry guards, so a unit that
  begins inside a group is replayed only in the same group context.
  This is the predicate the parked branch `cheap-cuts-wip` tried
  (`cheap_boundary()`, 13e4a16) without its `input_ptr <= in_open` and
  `cur_level = level_one` tests;
- **a paragraph start**: the first candidate after `new_graf` (§1091)
  began a paragraph at `nest_ptr = 1`, its `\everypar` list exhausted,
  with the nest still there (`nest_ptr = 1`, horizontal mode), and no
  output routine active. `new_graf` sets a flag (`Tex::par_start`,
  hashed in `Rest`), as does `resume_after_display` (§1200) where a
  paragraph goes on after a display, and the next candidate outside an
  output routine clears it; if the paragraph ended before (`\par` right after it, from
  `\everypar`, say) there is no start. What the list holds then (the
  indentation box, what `\everypar` appended) is in `Rest`. A paragraph
  that begins with a letter has its first word in the list too: §1090
  backs the letter up, and the main loop (§1034–1040) reads the rest of
  the word without returning to `big_switch`, so the start is the
  candidate after the first word;
- **before a fire**: the page builder has decided to fire (§1005,
  §1012). The fire is deferred to the next `big_switch`, where it is a
  candidate, and `fire_up` is the first action of the unit that
  follows. At every call site of `build_page` (§1076, §1091, §1094,
  §1096, §1100, §1103, §1026, §1054, §812, §1145, §1200) `build_page`
  is the last action and `big_switch` is the next thing that happens,
  so the order of everything observable is tex.web's, including
  `\everypar`'s tokens, pushed before the output routine at §1091, and
  `\everydisplay`'s at §1145. (Corrected 2026-09-29: this named §1145
  an exception, `build_page` followed by `push_math`; tex.web's §1145
  calls `push_math` first and `build_page` last.);
- **before `\shipout`** (built, §7.0).

Every other command read from a file is a candidate only past a large
grain (level 1; a file's beginning or end stays level 2). Exhausted
token lists are popped eagerly at `big_switch` (§357 pops them lazily
in `get_next`, `scanner.rs:231`, so the first file command after a
macro or an output routine was missed).

**A candidate is also taken inside `big_switch`'s fetch** (written
2026-09-29 before the code, after the course's word edit re-ran one
step from ch15's line 26 to line 65, three paragraphs and a pgfplots
figure, 996,361 commands: no paragraph end in it was a candidate).
Since LaTeX's paragraph hooks (2021) `\par` is a macro, `…\tex_par:D
\hook_use:n {para/after}\@kernel@after@para@after`, and its last
token expands to nothing. `big_switch`'s `get_x_token` (§380) expands
it, reads past the exhausted lists (§357) and fetches the next
paragraph's first letter from the file without coming back to
`big_switch`, where the candidate test was: when the test ran, the
`\par` list still held that token. §380's loop is as clean as
`big_switch` each time it returns to `restart` after an expansion. No
local is live there, and what the expansion did (the lists it pushed
and popped, a conditional, a `\csname`'s `\relax`) is state. So
`big_switch`'s fetch tests there too: once the expansion's lists are
exhausted and popped and a file is on top, the command about to be
fetched is read from a file with no list above it, which is the
candidate. A stop there resumes at `big_switch` with the command
counted, and the fetch starts again. Fetches outside `big_switch`
(a scanner's `get_x_token`, the main loop's lookahead) have their
caller's state live and never stop. The expansion the loop made
before the stop is shown by `\tracingmacros` once, as without the
stop.

**The deferred fire's form** (written 2026-09-29 before the code,
after the course's word edit re-ran the step holding ch15's pgfplots
figure, 991,705 commands: `\end@float` puts `\penalty\@floatpenalty`,
at most −10000, in vertical mode, the page builder fires
`\@specialoutput` inside the figure's step, and the fire reads the
page the edited paragraph's lines are on):
- The page step that decides to fire (`Step::FireUp`, §1005) does not
  fire. Its node goes back in front of the contributions, where §1005's
  `fire_up(p)` finds it, a fire is pending, and `build_page` returns.
- At the next `big_switch` the pending fire is a candidate, taken before
  the exhausted token lists are popped. The output routine then goes on
  the input stack above the lists that were exhausted when `build_page`
  returned, as in tex.web, where §311 shows them in an error inside the
  routine. Token lists may be on top at this boundary (`\everypar`'s at
  §1091, the `\end` backed up at §1054). The step's place holds the
  whole input stack, so a step can begin there.
- The step that begins there begins with the fire: `fire_up` on the
  first contribution, then the output routine (§1025). With no output
  routine, it is the default output (§1023) and the rest of
  `build_page`'s loop; a fire decided there is pending again, and so is
  a boundary again. Then the candidate test that `big_switch` made
  before the stop is made again, with the command still counted, so a
  default output that leaves an outer clean point stops there.
- The pending fire is part of the step's place, not a slot. A step's
  result holds it with the input (`InputState`), two places differ if
  one of them has a fire pending, and a step placed there begins with
  the fire. No step reads it: only the page step that decides sets it,
  and the next `big_switch` stops. So a fire deferred where none was,
  or no longer deferred, changes where a step ends, and item 4's rules
  for an end that moved apply.
- A step that begins at a fire is named, like any step, by its file's
  line: that of the topmost file level, under the token lists. The fire
  is part of the name too.
- The deferral is on where the other candidates are: SSA with
  `machine::clean_cuts()` (`PARTEX_MACHINE_CLEAN_CUTS=0` turns it off,
  and the fire is then the deciding page step's own, inside its call,
  as before). Machine mode does not defer (7.17.9 removes it).

*Built (2026-09-28, Task 2–3):* the outer clean points and the paragraph
starts are candidates of level 3 (`Tex::clean_point`, `run.rs`), a file
edge that is also clean included; exhausted token lists are popped at
`big_switch` in machine mode (`Tex::pop_exhausted_lists`); the chooser's
`file_cut` rule (`level >= 2`) cuts at them past 16,384 commands.
`PARTEX_MACHINE_CLEAN_CUTS=0` turns all three off and gives back the
boundaries from before. On the course, the cold build has 568 regions
against 464. Of its candidates, 52,940 are clean points (51,728 outer,
1,212 paragraph starts), against 142,383 of level 1 and 479 of level 2
(with §1200's starts too, Task 14: 53,317 clean, 1,589 of them
paragraph starts, and 569 regions). A
one-space or one-word edit that does not reflow re-runs 32,870 commands
instead of 76,119 (127 ms instead of 194 ms), and a `\label` 73,989
instead of 237,937. Edits that move the page (a word that reflows, a
blank line) barely change: they wait for the fold (§7.16.2). The
deferred fire is built for SSA builds (2026-09-29; its form is above,
and machine mode does not defer).

At a clean point the residual state, what `Rest` still holds, is small
and deterministic: the outer nest's scalars (`prev_depth`, `prev_graf`),
the input stack with one buffer line per open file, the conditional
stack, the printing column, and a few scalars. Everything else is a
cell (eqtb, fonts, names, objects, the marks and the page's observables
below) or a value (the page). The sanitizer's chain check
(`PARTEX_MACHINE_SANITIZE=1`) is the gate for any change here.

*Measured (Task 14c, diagnosis only).* The regions still dirty through
`Rest` after an edit are dirty for three kinds of reason:
- Dead scratch, about 7% of their commands, which canonicalizing at
  candidates would remove: the page's annotation, link and destination
  lists, which the next page ship empties (pdfTeX §752); `scaled_out`;
  `pdf_v`; the scanner's `cur_val`.
- Virtual object names seeded by position: a Pages node's
  (`last_pages`, `head_tab[pages]`). These are live and read at every
  ship.
- The save stack at the stops before `\shipout` inside an output
  routine, about twenty groups deep, where the saved values are live
  until the routine ends. Those stops are not clean points: the
  sentence above holds at clean points, and the ship stops are the
  exception until the deferred fire makes an output routine one unit.

*Built (Task 14f): the dead scratch is canonicalized.* At every cut in
machine mode the running engine's dead scratch is set to its initial
value (`machine::canonicalize_dead`). So the exit snapshot and the run
going on hold the same constants, and `Rest` still hashes them. The
fields, each with where it is written before it is read:
- the page's annotation, link and destination lists: emptied as a page
  ship begins (pdfTeX §752), read at that page's end;
- `pdf_v`: set by `pdf_set_origin` when text begins (pdfTeX §727–§735),
  read only inside text, and a ship ends its text;
- `scaled_out`: written by `pdf_print_bp` and `divide_scaled`, read by
  their callers right after (pdfTeX §690);
- `cur_val` and `glue_origin`: the scanner's results, which a command
  sets by scanning before it uses them (§409–§413, §440–§463).

`PARTEX_MACHINE_CANON=0` turns this off. `PARTEX_MACHINE_POISON=1` sets
them to junk at every candidate instead. That is the test that they are
dead: e2e in machine mode is identical and the sanitizer passes.

#### 7.16.2 The page builder is a fold, not state

`Builder` leaves `Rest` and becomes a value, `MCell::Page`, with a
content hash (the page list in shared 64-node chunks, `so_far`, the
insertion list, `last`, `contents`, `best_break`, and the unconsumed
contribution list, if any). A unit's *write* to it is the list of
items it contributed: `build_page`'s loop (§996–§1000) moves nodes from
the contribution list to the page, and the tracker records the moved
nodes (values: nodes, `Arc` boxes, sealed-line stubs) as the unit's
delta. `Page` is an accumulating cell (`Machine::accumulates`), and its
`combine` is `Builder::contribute` over the delta with the builder's
parameters (`\vsize`, `\maxdepth`, `\topskip`, the insertion `\count`,
`\dimen` and `\skip`, `\holdinginserts`), which the unit already
guards, since `build_page` read them through eqtb (`page.rs:233`).

The builder's observables become cells with read hooks, so a unit
depends on the page only if it read it: `PageLast` (`last`, for §424),
`PageSoFar` (for §421), `Marks` (`cur_mark`, for §386),
`InsertPenalties` and `DeadCycles` (for §419). They are derived by the
fold. `PageLast` is a cell of its own because `\addvspace` reads it in
every `\section` and environment; its value after a paragraph is that
paragraph's trailing glue, the same in both runs unless a page break
moved to exactly that point (`fire_up` resets `last`, §991), which is
then a true dependency.

The same pattern applies outside the page builder, to every value one
command writes and another only reads. pdfTeX §447's `last_item` values
(`\pdflastobj`, `\pdflastxform`, `\pdflastximage` and its pages and
colour depth, `\pdflastannot`, `\pdflastlink`, `\pdflastxpos`,
`\pdflastypos`, `\pdfretval`) are cells `MCell::PdfLast(k)`, one per
value (built in Task 14b; `PARTEX_MACHINE_PDF_LAST=0` puts them back
in `Rest`). Each is written where its object is made, or at
`\pdfsavepos` during shipout, and read only by `last_item` and by
`\immediate` right after the object is made. A value that is a virtual
object number is read through `final_num`, which reads `Numbering`
too. Before, a `\Cref`'s link made `Rest` differ after any edit to its
left on the same line, because the link's virtual number is seeded by
the position. That kept four regions (1.1M commands) of the `word`
edit dirty, though nothing read `\pdflastlink`. Re-seeding by a more
stable position was rejected: it would only move the problem to the
next value derived from a position.

In a rebuild the runtime replays a clean unit by running the fold over
its recorded delta (microseconds) and gets one of three answers:

- *continue*: the page did not fire; the next unit's replay goes on;
- *would fire here*: the old run's next unit must be an output-routine
  unit (one that begins with `fire_up`). Its entry guards, on
  `\box255`, the insertion boxes, `Marks`, `\outputpenalty` and `Page`,
  are checked as for any unit, against the values the fold produced
  and the cells the engine will write in `fire_up`. Held, it is
  replayed; failed, it is re-executed, and the engine does the real
  `fire_up` as its first action;
- *diverged*: the fold fired in the middle of the delta (only possible
  after coarsening or at §1145), or the old run fired at this unit's
  end and this one does not. The unit is re-executed from its entry
  with the fold's page state set (`set(Page)` assigns the `Builder`).

The engine stays eager and exact; only the replay is a fold. After a
footnote is added, the later paragraphs of the chapter replay as
values, and only the output routines and ship-outs of pages whose
content or number changed re-execute, about a millisecond each. A
shift of `\c@page` still re-runs every later output routine; that is
TeX's own rule (headers and `\write`s print the number), and it is
where parallel units come in (7.16.6).

`Build::rebuild` changes in one place: in the loop of
`rebuild_or_stop` (`build.rs:939`), a region found clean by its guards
has its `Page` delta folded before it is passed over, and *would fire*
or *diverged* make it dirty. The stub gets an accumulating fold cell
with the same three answers, and its property tests cover it.

*Built, first step (2026-09-28, Task 14):* `MCell::Page` is a plain
cell, not yet accumulating. It holds the builder's fields (`contents`,
the list in the snapshot's shared 64-node chunks, `so_far`,
`max_depth`, `least_cost`, `best_break`, `best_size`, `ins`,
`insert_penalties`, `last`, `discards`), not the contribution list,
which stays in `Rest` with the nest. `Rest` leaves the builder out, and
`PARTEX_MACHINE_PAGE_CELL=0` restores the old hash. Every engine access
goes through `Tex::page` or `Tex::page_mut`, which call
`Tracker::page_access`. A region that touches any field reads and
writes the whole cell; the observables are split out in Task 15. On
the course the harness counts are unchanged, with the cell on or off.
After a reflow, the regions that stay dirty differ in `Rest`'s PDF
writer: `last_link`, a virtual object number seeded by the position,
which moves with the edited line's columns; and after a ship, `ship`.
The page is not the cause. Phase 4's positions come before the fold
pays off on `word` and `enter`.

#### 7.16.3 A checkpoint is O(changes)

`Tex` splits into the working state and a `Checkpoint` of shared roots;
`Snapshot::of` no longer clones `Tex`. Container by container:

| container | today | checkpoint form |
|---|---|---|
| eqtb, hash, save stack | `JVec` (512-word chunks, dirty bits) | as is: the chunk roots (Task 6b: a dirty bit per 64-word block, the commit compares only the blocks written; smaller chunks wait for a clone that shares the chunk vector, T7) |
| token lists | flag scan of every list, memcmp of every expanded macro | reference counts leave the token path (below); a changed-id list instead of the scan; `free` and `interned` shared |
| `str_pool`, `str_start` | memcmp over the whole allocation | append-only: a high-water mark since the last checkpoint; a truncation is a flag (built, Task 6: a floor at the strings made, lowered when `str_ptr` goes down) |
| `buffer`, `input_stack`, `param_stack` | memcmp over the whole allocation | captured by their live prefix, which at a clean point is one line and a few records (built, Task 6) |
| the object stores, `xregs.cells`, `xeq_level`, the page and nest lists | copied whole | `Chunked` (shared 64-element chunks, copy on write) |
| per-font tables, `pdf.ship.fonts`, the font writer's trees, exceptions, primitives, the font map, memo tables, `cur_mark` | copied whole | `Arc` with `make_mut`: a bump per checkpoint, a copy on the rare write (built, Task 7, for the per-font tables, `pdf.ship.fonts` and the primitives' meaning map: `cow::Shared`; the others clone in under 5 µs each) |
| `pdf.out` buffers, the log and `\write` buffers | copied whole | scratch: flushed to effects at every candidate, absent from the checkpoint |
| the host's maps | cloned | shared |

*Reference counts leave the token path.* tex.web counts references to
a macro's body so that a redefinition during its own expansion keeps
the body alive until `end_token_list`; partex does not need a count per
call for that. A macro call no longer touches its body's record; a list
that `eq_destroy` would free while an input level still references it
(a walk of the input stack, tens of entries, on a redefinition only) is
released when that level ends. Parameter lists, made and freed per call
(59M on the course), live in a stack arena, not in the store.
(Built for the input stack's references, Task 4, 2026-09-28, in
`tok.rs`, `delete_token_ref`. `begin_token_list` and `end_token_list`
no longer count. A list whose last table reference goes while a level
reads it is kept with `refs = -1` and freed when the last such level
ends, which is where the count freed it, so ids are reused in the same
order. On the course's cold machine build a commit now compares 913
lists instead of 2,264; the rest are lists written between two cuts,
parameter lists among them (not broken down). The walk
is not rare: a table drops a list's last reference 21.2M times in a
plain pass of the course, at an average input depth of 23, against 75M
macro calls. So the walk costs about what the count did, and a plain
pass is +0.10% instructions, not the decrease expected. The edit
harness rows did not move.)
(Parameters, Task 5a, 2026-09-28: built as a pool in the store, not as
an arena beside it. An argument takes an empty list from
`TokStore::arg_free`, and `end_token_list` gives it back, cleared but
still allocated (`Tex::arg_new`, `Tex::arg_flush`). Its tokens are
stored without a change mark (`arg_push`), because no snapshot is taken
while `macro_call` scans. Readers still find a parameter by list id, so
the token path, `show_context`, the memo, bulk runs and the state hash
are unchanged, with no branch per token. A plain pass of the course is
-0.47% instructions. A commit compares about as many lists as before,
944 per cut against 913, and freezes 3% fewer anew, because recycled
`back_input` lists and other short-lived lists dominate those numbers.
`PARTEX_PARAM_ARENA=0` turns the pool off.)
(The token store's cut, Task 5b, 2026-09-28. `back_input`'s lists
(§325) join the pool. Pooled lists are kept by liveness, not by change:
no flag and no id when they are taken, filled or freed, and a snapshot
visits every pooled id instead, about 120, most of them empty both
times. The other lists are visited through a list of changed ids,
pushed when a list's flag goes from clear to set (`TokStore::changed`),
so a commit no longer scans the 58k lists. `PARTEX_TOK_CHANGED=0`
brings the scans back. Measured with `PARTEX_CUT_TIMING=1`, the token
store's part of a cold cut fell from 1.16 ms to 0.50 ms; what remains
is refreezing the roughly 470 lists that really changed. Recording the
ids is tracking. `Tracker::CHANGED_IDS`, a constant true only for the
machine's `CellTracker`, selects it at compile time, so a plain pass
stores the flag with no test, as before (−0.08% with the pooling). A
store whose ids are not complete (a new or loaded one, or one under a
tracker without them) is scanned. Recording the ids cost 1% of a plain
pass when it was unconditional; chunk-level dirty bits cost 0.85%.)

*The vectors* (Task 6, 2026-09-28, `flat.rs`, `commit_live`). A snapshot
captures each `Flat` up to its live prefix: `pool_ptr`, `str_ptr + 1`,
`max(first, last)`, `input_ptr` and `param_ptr`, the prefixes the state
hash reads. What lies past the prefix is dead: TeX writes it before it
reads it. The segments wholly below a floor are shared without a
compare. For the strings the floor is the start of the string being
made: a made string is never written, and the floor is lowered where
`str_ptr` goes down (`strings_reopened`). A checkpoint holds the prefix
and the length, and resumed, the dead part reads as zeros. The `Rest`
hash takes an unwritten segment from the shadow uncompared. Per cold
cut the five commits fell from 489 µs to 39 µs; the pool's 2 MB was 296
of them. `PARTEX_FLAT_LIVE=0` gives the whole compares back.

*The clone* (Task 7, 2026-09-28). Timed by field group
(`PARTEX_CUT_TIMING`, `CLONE_GROUPS`), `Snapshot::of`'s clone of the
engine was 407 µs a cold cut. The largest pieces were:
- the per-font tables, 121 µs: twelve arrays of 9,001 font slots, whose
  `Arc`s and `Option`s are copied one by one;
- the `JVec`s' chunk vectors, 131 µs: one `Arc` per chunk, 3,700 of them;
- the token store and the `Flat`s' segments, 55 µs;
- `pdf.ship`, 17–41 µs.

The font tables (`FontArrays` over a `cow::Shared<FontData>`, its
scratch flag outside), `pdf.ship.fonts` and the primitives' meaning map
are now shared. A clone of each is a reference count, and the first
write after a snapshot copies it. The clone fell to 182 µs cold and 171
µs in the rebuild. What remains is mostly the `JVec` spines, which are
O(chunks) either way: sharing a spine moves its copy from the clone to
the next commit. A persistent tree of chunks, copying one path per
changed chunk, would cut both.

*The `JVec`s* (Task 6b, 2026-09-28, `journal.rs`). A write sets a bit
per 64-word block, the same single OR as the chunk's bit it replaced, so
a commit compares only the blocks written of a dirty chunk. Per cold cut
eqtb had 580 dirty chunks of 1231, 187 of them changed. The eqtb, hash
and save-stack commits fell from 1039 µs to 836 µs, the compare from 393
to 178. What remains is the copy of the chunks that changed, about
2.5 µs per 4 KB chunk. 64-word chunks cut the commits to 300 µs, but
tripled `Snapshot::of`: its clone takes one `Arc` per chunk, a cache
miss each. So smaller chunks wait until the clone shares the chunk
vector itself (T7). `PARTEX_JVEC_BLOCKS=0` compares whole chunks.

*The digest* is a Merkle hash over the same roots, memoized by root
identity as `hashmemo.rs` does for chunks today; the tables that are
constant after the format (primitives, `hyph_word`, exceptions) are
hashed once. `Lists::share`'s compare of every node on the page goes,
since the page is a value in shared chunks.

*A restore rebases.* Instead of thawing eqtb, hash, the save stack and
every list into fresh vectors, the running engine compares its roots
with the checkpoint's by identity and copies only the chunks that
differ, as `TokStore::thaw_from` already does for token chunks. The
tracker keeps its slot arrays across `set` and clears the slots it
touched. (Built for the tracker and the object log, Task 1, 2026-09-28,
`PARTEX_MACHINE_KEEP_TRACKER=0` to switch it off. One set of fresh tables
per rebuild remains, from the clone the rebuild starts from.)
(Built for the vectors and the token lists, Task 12, 2026-09-28.
`JVec::thaw_from`, `Flat::thaw_from` and `TokStore::thaw_from` take the
replaced engine's running vectors and records and copy only the chunks,
parts and lists that are not the snapshot's. A restore fell from 9.4 ms
to 4.6 ms on `footnote`, and from 9.7 ms to 6.4 ms on `word`. What
remains is:
- the token store's walk over its 58k records, 1.7–3.2 ms;
- the `JVec` chunks of another lineage, 1–2 ms: a rebuild's engine
  shares few chunks with the old run's snapshots it restores;
- eqtb's differences, 1 ms, when cells are kept;
- the snapshot's clone, 0.23 ms.

`PARTEX_RESTORE_REBASE=0` thaws anew. The 100 µs target needs the clone
gone, which is restoring into the running engine's own containers, and
the walks made O(chunks differing).)

Targets: a cut in tens of microseconds, a restore under 100 µs, cold
overhead under 1% of a plain run. Memory per kept checkpoint is the
chunks its unit dirtied, tens of kilobytes.

#### 7.16.4 Regions, hierarchy and policy

Once cuts cost nothing, a region *is* a clean-point stretch: there is
no separate fine grain and no checkpoint inside a region. The
hierarchy is

- the document: the link over pages;
- pages: an output-routine unit (from the fire to the next clean
  point) and a ship unit;
- units: clean-point stretches, paragraphs mostly;
- leaves: sealed lines, boxes by content hash, memoized calls;

with the fold above the units. The choice stays dynamic through the
chooser and `coarsen` (§7.0): checkpoints are kept dense where edits
land and sparse where the document is idle, with the guards composed,
so memory stays bounded and the first edit in a quiet chapter re-runs
a few paragraphs at most, then that chapter is dense.

*The refinement signal (measured 2026-09-28, the audit,
`PARTEX_MACHINE_AUDIT=1`).* Could the cells themselves be chosen
dynamically, `Rest` split where it causes waste? The audit classifies
every re-executed span of a rebuild (the rebuild compares spans, not
regions):
- *spurious*: it ended as the old run did;
- *carried*: its effects are the same, and whatever differs at its end
  differed at its entry and neither run changed it in the span (by field
  for `Rest`, `Tex::rest_field_differences`; for an accumulating cell,
  both runs added the same);
- *real*: anything else.

The strict test is nearly blind with a lump: every region writes `Rest`,
so a live field that differs passes through each one and every exit
differs. That is the measurement that matters: while `Rest` is one
cell there is no early cutoff below it, whatever the regions are. The
carried test, run with tonight's five lifts off, ranks each row's lift
first, and the regions it flags hold 82% of the commands the lifts
removed. The misses are names seeded by position, which differ in the
representation and not in the output (the naming rule of §7.16.5), and
dead scratch that a region itself makes different (liveness needs
reads, §7.16.4's poison test).

The audit is a diagnostic, not a policy. It does not choose a
granularity: the cells are the engine's values, and a field that
regions only carry is a value that is not yet its own cell. What it
gives is the order in which to make them cells, by the commands they
carry: the save stack's saved values (read where `unsave` restores
them, §281–§283), the destination objects' head, `\@gtempa`. "Carried"
says a region passes a field on, not that no later region reads it, so
each becomes a cell whose reads are tracked, never an omission from
`Rest`: leaving the saved values and the destination head out (a
what-if, measured and then removed) cleans 769k of the footnote's
commands and fails the sanitizer, because `unsave` reads the values
when the output routine's groups end. The tables are in LOG.md.

#### 7.16.5 Positions and streams

Line numbers are renamed through the file's diff (§7.15, step 3; C's
series is unchanged). `\write` streams are entry logs, and the two are
one mechanism: a job-written file is a *served file whose contents come
from the log*. A pass's log is diffed against the log the previous pass
read, and positions in it are renamed exactly as for a source edit, so
regions that read unchanged entries keep their meaning. Each entry has a
stable identity (the writing unit's position and a count, as for virtual
object ids). A non-immediate `\write` is performed at `\shipout`
(`write_out`, §1370, from `out_what`, §1373), so its entry sits in ship
order and is written by the ship unit. The `.aux` is read back inside a
group: TeX Live 2026's `latex.ltx` wraps both `\document`'s
`\@input{\jobname.aux}` and `\enddocument`'s `\@@input\jobname.aux` in
`\begingroup … \endgroup`, so its lines are clean points by the fixed-group-context
rule of 7.16.1, not at level one, and a new label runs one entry unit
per pass. `\enddocument`'s read-back within the same pass is served
from this pass's log. The `.aux`, `.toc`, `.bbl` and the rest are
written to disk by the link, for latexmk, BibTeX and the like.

*Names that cross regions (Task 14d).* "A position and a count" is
refined by a rule. **A name held in state that crosses a region
boundary must come from an identity TeX defines, never from the
position of the step that made it.** A position moves with every edit
before it on the line (a word) or in the file (a new line). A name made
from it then differs in every later region that holds it, though
nothing observes the name itself: TeX sees only the final number,
through `Numbering`. Built for virtual PDF objects
(`objtab::tex_identity`, `PARTEX_MACHINE_TREE_NAMES=0` to turn off):
- a page is named by its number, a destination by its name, a raw
  object, form or image by its count (`pdf_obj_count` and friends), a
  font by its number;
- a leaf of the page tree is named by its place, level 0 and index
  `k` for pages 6k+1…6k+6;
- the upper levels are made at the end of the job, whose seed is fixed.

Still named by position:
- the objects no identity names that die before the next boundary:
  a ship's resources, stream and link annotations;
- the objects no identity names that cross regions: `\pdfstartlink`'s
  and `\pdfannot`'s objects, which their `\pdflast…` cells hold;
  outlines, which `first/last/parent_outline` and their neighbours'
  entries hold; an open object stream's number.

`Numbering`'s version is a hash of its events, and those include every
object's name, so any object named by a moved position made every
reader of `Numbering` dirty.

*The numbering guarded by its answers (Task 14e).* A region that asks
for single numbers (`\pdflastobj` and the other `final_num` readers,
`\pdfrefobj`/`\pdfrefxform` through `of_final`) guards the answers it got.
These are derived cells of `Numbering` (the runtime's rule is §7.0's
"Derived cells"):
- `FinalNum(k)` for an object that existed at the region's entry;
- `OfFinal(n)` for the object numbered `n`;
- `NumState` (the counters: `sys`, `obj_ptr`, the open stream's `cur`
  and `idx`, `streams`) for an object the region made itself, whose
  number depends on how many numbers were taken before it, not on
  any name.

It is exact because pdfTeX numbers objects in order of creation, so an
object's number is fixed once it is made. It changes only if the count
of numbers taken before it changes, which is what the guard sees.

A region that also reads the numbering whole records no answers as
guards, only its chain guard (the implementors' contract of
`Machine::derived`): `pdf_finish_file` and the statistics at the job's
end do. `PARTEX_MACHINE_NUM_ANSWERS=0` guards the chain everywhere. On
the course the `\pdfrefxform` region (360,212 commands) no longer
re-runs after an edit that renames objects before it:
- `word`: 718,176 → 357,964 commands;
- `enter`: 588,495 → 228,283;
- `delete-par`: 1,840,582 → 1,480,370.

It still re-runs, correctly, when the count of objects before it
changes (`footnote`).

*Sync across moved lines: the plan (Task 20).* After an edit that
inserts or deletes lines, a rebuild re-runs the rest of the file:
`delete-par` re-runs 1.2M commands in one span, `enter` 221k. It can
stop early only at an old boundary whose state it reaches again. Four
things stand in the way today, and they have to go together, in this
order:
1. *The old boundary's identity.* `Machine::at` hashed `line` with the
   rest of the position, so after two lines are deleted every later
   position in the file hashes differently and no old boundary is ever
   found. The line has to be a part of the key the rebuild can map
   through the file's diff: a position at new line L′ is the old
   boundary at L = map⁻¹(L′), where the line maps (an unchanged line).
   Built in Task 20: `machine::Pos` (key, file, line).
2. *The changed cells.* `changed_cells` compares a file's lines by
   index. `delete-par` deletes 2 lines of 204, and by index 171 lines
   differ. Every later region reads some of them and is dirty through a
   `Line` cell whatever the sync. With the diff, 2 lines changed. The
   other lines keep their contents at new numbers, so the old build is
   renamed through the diff, not patched:
   - `Build::rename(map)` in `partex-incr` renames the boundary keys,
     `Line(file, k)` in guards, writes and the readers' index, and
     `Positions` values with their versions;
   - the edit's changed cells are then only the lines the diff
     changed.

   Renaming the keys is why they must hold the line apart (1). This
   and 1 are one change for exactness: a kept region whose keys or
   guards were not renamed would chain wrong, and the sanitizer's chain
   check sees it.
3. *The positions in the state.* With `PARTEX_MACHINE_RENAME=1` (part
   1, off by default), `line`, the line stack and the lines read are
   `MCell::Positions`, which the rename maps. `restore_rest` takes an
   open file's place from `Positions`. It must be on for any of this.
   The engine's copies of line numbers (part 2, `machine-converge`
   3ae550d): `mode_line` §213, `if_line` §489, `skip_line` §493,
   `pack_begin_line` §661. At an outer clean point they are the outer
   list's (0), no open conditional, and 0. So a sync at a clean point
   needs part 2 only where a conditional is open. A sync inside a
   paragraph needs all of it.
4. *Line numbers read as values* stay true dependencies:
   - `\inputlineno` (LaTeX's `\@currenvline` at every `\begin`);
   - the "on input line N" of warnings;
   - `l.N` in error contexts.

   Each read becomes a derived cell of `Positions`, "the line number
   this region observed", as the numbering's answers are. A region that
   printed a moved line re-runs, and one that only passed through
   moved lines does not.

Also, the names of the objects a step makes are seeded by its whole
position, line included (`position_hash`, kept as it was). After lines
move, an object made past them gets another name. That is harmless to
the numbering since Task 14e, but the name is held in the page's link
whatsits and in object entries. The rule of "Names that cross regions"
below applies: the seed should be the renamed position, or the
position without the line.

The tasks, about 30 minutes each:
- **T20** (this one): the boundary with the line apart (`Pos`); no
  change in behaviour; the plan.
- **T21**: `changed_cells` by the line diff, and the map old → new per
  changed file (`machinehost.rs`, from the served file's old and new
  contents).
- **T22**: `Build::rename(map)` in `partex-incr`: keys, `Line` cells,
  the readers' index. Stub property tests with inserted and deleted
  lines, which the stub's line identities make natural.
- **T23**: `TexMachine`'s side of the rename: `Positions` values and
  versions, `Line` guards and sealed lines by line, with
  `PARTEX_MACHINE_RENAME=1` on by default. Measure `delete-par` and
  `enter`.
- **T24**: part 2's copies into `Positions` (cherry-pick `3ae550d`'s
  tags, then the cell), for syncs inside paragraphs and conditionals.
- **T25**: line numbers read as values, as derived cells. The seed of
  object names from the position without its line.

*Built (T21–T22), behind `PARTEX_MACHINE_RENAME=1`:*
- `Build::rename` (`build::Renaming`): cells, boundaries, values, guard
  versions and the final state renamed; regions that read a gone cell
  are forced dirty until they run again.
- The changed lines come from one hunk per file (common prefix and
  suffix), and `machine::rename_lines` maps the old build through it.
  The line right after the hunk counts as gone, because the region that
  read it went there from the hunk.

With it on, the rebuild syncs at every old boundary after the edit. On
the course `enter` and `delete-par` still re-run what they did, but now
region by region, each dirty for a reason of its own:
- the reflowed pages (`Page` and sealed lines);
- `Rest`;
- `\@currenvline` (T25).

*Measured (T23a).* A line split that moves every later line by one and
changes nothing in the output costs as much as `enter` (228k commands,
the same regions), so what keeps them dirty is not the reflow. It is
state named or numbered by lines:
- the sealed lines' key seed `seal_at` (file, level, **line**, count):
  it names every later paragraph's `Sealed` cells and the boxes on the
  page that carry the keys;
- `skip_line` (§493), dead at a clean point;
- `mode_line` inside paragraphs (§213);
- e-TeX's group lines in the save stack (§274);
- the object-name seed (T25);
- the ship's per-page scratch (`last_stream`, `pdf_h` and the text
  state).

T24 is re-scoped to begin with the sealed lines' names, by the rule of
"Names that cross regions": a structured key whose line the rename
maps.

*Convergence in one session.* A pass after the first is a rebuild
whose edit is the difference between two logs, as `partex watch`
already runs it (§6.1). `partex build` and `-converge` move onto
machine mode for the same (roadmap below). Pass 2 after a cold build is
not small: every paragraph whose `\ref` or `\cite` turns from `??` into
text reflows, and page breaks move; warm parallel units (7.16.9) are
what make it cheap. BibTeX and makeindex stay whole stages memoized by
content (the `.aux`'s citation set, the `.bst`, the `.bib`; the `.idx`
and its style): a `.bst` is a program with state across entries (alpha
labels depend on neighbours, numeric labels on the sort), so running
one entry is not exact. The TeX side is incremental through the
`.bbl`'s and `.ind`'s diffs.

#### 7.16.6 Output and the loop

- *Exact mode*: the link keeps per-region resolved bytes
  (`link_cached`, §7.15) and adds a per-file comparison up to a
  renaming of virtual ids, so an edit that changes no PDF effect links
  nothing; offsets by prefix sums over cached page bytes.
  (Built, Task 27, 2026-09-28. A watch already skipped the link when no
  region's effects changed (`Build::take_effects_changed`, a comparison
  of the effects spliced out with those put in). It resolved only the
  regions put in (`link_cached`) and wrote only files whose bytes
  changed. The in-process path of `bench/edits.sh` linked in full after
  every rebuild. It now links as a watch does (`machinehost::Linker`).
  The cold build's output is linked before the first edit, and under
  `PARTEX_MACHINE_SANITIZE=1` or in debug builds the result is checked
  against a full link. `PARTEX_LINK_SPLICE=0` gives the full link.
  - `space` links in 0.9 ms instead of 54 ms.
  - `word` links in 23 ms instead of 46 ms: numbering 1.6, resolve 8.2,
    the layout's pass 2.1, the cross-reference stream 4.5 (rendered and
    compressed each time, since the offsets move), copy 1.8, the write
    2.5.
  - `footnote` links in 43 ms instead of 51 ms: a footnote renumbers the
    objects after it, so 330 of 683 regions are resolved again.

  Splicing the buffer would save at most the copy's 1.8 ms. What costs
  is what is O(regions) in `link_cached`: the numbering, each region's
  written numbers compared (`region_inputs`), the layout's pass, and the
  cross-reference stream.)
- *Fast mode* (§7.15): stable object ids, deflate off the critical
  path, incremental updates, checked by `scripts/pdfcheck same`.
- *Invisible edits*: a `Line` cell's version is its tokens under the
  catcodes it was read with (§7.2's rule, ported to machine mode).
- *The watch loop*: inotify or a 5 ms poll; idle work (coarsening, the
  save's copy) never delays the pickup of an edit.
- *The renumbering cascade*: output-routine units run in parallel
  (7.16.9), the page in view streamed first (§7.11).

#### 7.16.7 Targets by edit

Estimates on the course, against the numbers of 2026-09-27; the rule of
§7.15's latency targets holds (latency follows the change in the
output, never the document's size):

| edit | before | after 7.16.1–7.16.4 | with 7.16.5–7.16.6 |
|---|---|---|---|
| a space or comment | 0.18 s | about 5 ms | under 1 ms |
| a word | 0.25 s (0.40 s in watch) | about 5 ms plus the 13 ms link | about 5 ms |
| `\label`, both passes | 1.96 s | about 50 ms | under 10 ms |
| a footnote, or a paragraph deleted | 1.9 s | 30–80 ms (2–4 ms per changed page) | 20–50 ms |
| Enter | 1.8 s | 1.8 s | as a word |

These rows are a first edit after a cold build, the worst case. Typing
is the next edit in the same place, on the fine regions the first
rebuild recorded. `bench/edits.sh`'s `word2`, `space2` and `enter2` rows
measure it (edits joined by `;;`, the row being the second rebuild).
That second number is the latency a user feels while typing, and the
target the table above is about.

*Measured budget per keystroke* (Task 12b, 2026-09-28). The second
rebuild of each typing row, in process, unsanitized, the median of three
runs one at a time (`PARTEX_CUT_TIMING=1`'s keystroke timeline), in ms:

| piece | `space2#2` | `enter2#2` | `word2#2` |
|---|---:|---:|---:|
| the edit applied and diffed | 1.0 | 1.0 | 0.9 |
| TeX re-run (the regions' own commands; spans below) | 5.7 | 28 | 68 |
| its cuts | 0.6 | 2.6 | 3.5 |
| restores: `restore_rest` | 12.6 | 16.3 | 20.6 |
| restores: the rest of `replay_exit` (cells set, its snapshot) | 7.5 | 9.6 | 11.4 |
| other replay | 0 | 5.0 | 0 |
| compare | 0.2 | 1.5 | 6.3 |
| splice | 1.8 | 4.3 | 7.7 |
| the rebuild's other work (the final cutoff) | 5.4 | 0.2 | 0.4 |
| link | 0 | 11.5 | 17.9 |
| write | 0.2 | 2.3 | 2.4 |
| wall | 35.7 | 83 | 137 |

Where the time goes:
- Restores are 58% of `space2#2`, and the largest piece that is not
  TeX's own work. The first restore of a rebuild restores into the
  rebuild's start state, a clone of the job's initial state, which
  shares nothing with a snapshot in the middle of the document. So it
  thaws nearly everything: the token store 7 ms, the `JVec`s 4 ms.
- "TeX re-run" is not TeX's speed. `word2#2`'s 10,015 commands take
  68 ms, where the cold build runs 1.2M a second:
  - the job's last region, 244 commands, takes 32 ms;
  - a span of 2,019 commands takes 14.5 ms.
  So re-running a region costs a fixed amount besides its commands,
  which is not accounted for yet.
- Link and write follow the output, as intended.

#### 7.16.8 The work

Tasks of about thirty minutes each, at most two Opus agents in parallel
on their own branches, each gated by e2e in both modes, the sanitizer
where it applies, and one measured number on `bench/edits.sh`; LOG.md
carries the schedule and the outcomes. **End-to-end optimization comes
before new features** (the user's decision, 2026-09-28): the unit model
changes what every feature plugs into (object numbering, checkpoints,
stages, the page IR), so a feature built earlier would be built twice.

1. *Baseline*: a one-command harness for the canonical edits (done).
2. *Quick wins*: the tracker across `set` (done); clean points as cuts
   (7.16.1, the old T2 and T3 merged).
3. *Cheap cuts* (7.16.3): ten tasks, container by container.
4. *The fold* (7.16.2): six tasks.
5. *Positions and streams* (7.16.5): seven tasks.
6. *Output and the loop* (7.16.6): six tasks.
7. *Warm parallel units and convergence* (7.16.9, 7.16.5): about five
   tasks, including `partex build` on machine mode.
8. *PDF ship-out onto page IR, parallel emission* (7.16.9): about six.
9. *Cold speculation* (7.16.9): four tasks, then a measurement; parked
   unless the course's cold build gains at least 3×; four more if not.
10. *Features*: PNG and PDF inclusion, biber (7.16.10).

Risks: the fold's exactness (the display-start fire stays in its unit;
the sanitizer's chain check and e2e in machine mode gate every step);
memory with a checkpoint per paragraph (the policy of 7.16.4);
`\lastskip` after every `\section` (`PageLast` its own cell);
7.16.3 touches most of `Tex` (mechanical, one container at a time);
cold speculation's payoff is unmeasured (timeboxed).

#### 7.16.9 Parallelism

Three kinds, in order of value; each is exact by validation, never by
assumption.

- **Warm parallel units.** Every unit starts from the previous run's
  recorded entry state for the same unit (`rounds::run_warm`, §7.0),
  with the known changes patched in, and runs on the pool; units are
  validated in program order against the composed exits of the
  accepted prefix, with the fold's three answers (7.16.2) for the page;
  the first unaccepted unit starts the next round from its true entry.
  On TeX this was 3× slower than sequential while `Rest` was one lump
  (§7.7); with small residual state, a unit's predicted entry is almost
  always its true one. It is what makes *global* edits cheap, which
  every other mechanism re-runs whole: a `\linespread`, a preamble
  definition, a font size (65.8M commands, 79 s sequential; about
  55 s of work, so a few seconds on 24 cores), and convergence passes
  2 and 3 (7.16.5).
- **Per-page emission from a page IR.** PDF ship-out writes bytes as
  pdfTeX does (`pdf/ship.rs`); only DVI goes through page IR (§5.6).
  Moving it there (§7.6's four steps) lets each page's content stream
  be encoded and deflated independently, with numbers and offsets by
  prefix sums at the link, byte-identical, and gives a viewer its
  display list. Deflate is already at the link and memoized.
- **Cold speculation.** Without a previous run, units start from the
  state after the preamble. Three things make that fail naively:
  - *A guessed page fires in the wrong places.* Speculative units run
    with the page builder recording their contributions and never
    firing; the fold over the accepted units finds the true fire
    points, and the output-routine units run there.
  - *Counters chain.* A counter each unit increments is validated one
    link per round. Additive writes first (a unit that only adds to a
    counter records `+k`, and composition sums them), full affine holes
    (§7.5) only if measurement asks for them. A counter that is
    typeset or tested is forced and guarded like any read.
  - *Floats wait for output routines*, which write LaTeX's free list of
    float boxes; such units validate a page later.
  Timeboxed (7.16.8).
- **Not pursued: the stage pipeline** (line breaking, page building and
  emission on their own threads behind the front end). Every
  `build_page` call can fire, and a fire must happen before the next
  command, so the front end could run ahead only by speculating "no
  fire" and rolling back; every `\section` reads `\lastskip` through
  `\addvspace`. Its ceiling is the front end's share of a build (about
  1.5× cold), and it saves little on an edit. Future work.

#### 7.16.10 Features after the optimizations: their interfaces

Written down now so that the optimizations leave room for them; none is
built yet (`pdf/image.rs`: JPEG only).

- *PNG* (pdfTeX's `writepng`, including its copying of IDAT data when
  the image allows it) and *JBIG2*: an image is read once, keyed by its
  file's contents, and becomes an effect with a virtual object id, like
  any object; `\pdfximage`'s dimensions are cells read by the region
  that queries them.
- *PDF inclusion* (pdfTeX's `pdftoepdf`): the objects copied from an
  included file are link-time effects keyed by (file contents, page,
  box), numbered by the link in pdfTeX's order, with the copied streams
  passed through as they are. Byte-exact means reproducing the order in
  which pdfTeX walks and writes the included objects.
- *biber*: a stage, a tracked function from the `.bcf`'s entry log and
  the `.bib` files to the `.bbl`'s entry log. Three routes, chosen
  later: the external program as an opt-in, content-memoized stage (an
  exception to §0.6, native only, and unavailable in wasm); a native
  port (Unicode collation, name parsing, `uniquename`, a byte-exact
  `.bbl`: a project of its own); or biblatex documents with
  `backend=bibtex` until then (§7.10).

**Superseded.** §7.15's "nested units with dynamic dependencies", its
"in-region checkpoints" and "stages" paragraphs, and §7.5's lazy page
builder inside regions are replaced by 7.16.1–7.16.4: units are the
clean-point stretches, the fold is the page builder's replay, and the
engine's order of execution never changes.

### 7.17 Dynamic SSA: the build as one graph

The third design of the execution model, made on 2026-09-28 with the
user after the unit model's first week (Tasks 12 and 20–23 and the
audit, LOG 2026-09-28). It is a clean design of the engine, not a
change to §7.16's machinery: nothing below is a switch on the unit
model, and nothing of it is kept for compatibility. §7.16's numbers
stay as the baseline, and its harness (`bench/edits.sh`, the sanitizer,
the gate) is how this design is measured. In one sentence:

> A build is one graph. Every operation is a **call** `v = f(a₁ … aₙ)`,
> every piece of state is an immutable **value** whose version is its
> content, and an operand is a version. A call runs again if and only
> if a version it read changed, and it stops the change where its
> results come out equal. A job that reads what it wrote is a **cycle**
> in the graph, iterated in memory to its fixed point. The **hierarchy
> is the call tree**, and there is no other mechanism.

**What the first week showed.** Two measurements decided it.

- Splitting one line of the course in two at a space moves every later
  line by one and changes nothing in the output. It costs what pressing
  Enter costs, 228k commands in the same eight regions, with the rename
  of Task 21–22 on or off (Task 23a). What kept the regions dirty was
  not the reflow but state named or numbered by lines and carried in
  `Rest`: the sealed lines' key seed above all, then `skip_line`,
  `mode_line`, e-TeX's group lines and the object-name seed.
- The audit (§7.16.4) asked which re-runs were spurious. Its strict
  test found 0.6% of the waste the week's lifts removed, because every
  region writes `Rest`, so a differing field passes through each one
  and every exit differs; only a test by field found 82%.

Both say one thing: a lump of state that every region carries defeats
early cutoff whatever the regions are, and identity by position makes
a moved paragraph a different one. The remedy is not a finer lump, a
policy for splitting it, or a rename through the diff. It is a
representation in which there is no lump, in which every read is of a
value, and in which identity is not position.

**Why "SSA".** In a compiler's SSA form every variable is assigned
once, a use names the assignment it reads, and φ merges values where
control flow joins. TeX has no static program: a macro's meaning is
data, `\csname` (§372) computes an address, and what `\ref` reads
depends on its argument. So the graph here is built as the engine runs,
and it is a *trace*: each write makes a new version of a value, each
read names the version it read, and a job's next trip is a φ at every
load of what the job stored. The construction is known: self-adjusting
computation (Acar; change propagation over a trace, with early cutoff),
demand-driven and by name in Adapton (Hammer et al.), keyed queries
with verified dependencies in Salsa (rust-analyzer's, which also
iterates cycles to a fixed point), monadic builds with early cutoff in
"Build systems à la carte" (Mokhov, Mitchell, Peyton Jones), and
iteration over a lattice of versions in differential dataflow
(McSherry). "Dynamic SSA" is this document's name for the combination.

#### 7.17.1 Values

**All of the engine's state is values**: the equivalents of `eqtb`
(§220), the save stack (§268), the nest's lists (§211), boxes and
their lists, token lists (§289), the page builder's state (§980), the
insertion classes, marks, fonts, hyphenation, the PDF writer's tables
and object list, the streams (below), and the source (7.17.4). A value
is immutable and persistent: a write makes a new version that shares
everything it did not change with the old one. A version is the
value's content hash, computed when the value is made from its parts'
hashes (a Merkle tree), so a new version of a list costs what it
changed, and two versions are equal iff their content is. Equality by
content is what early cutoff compares.

**A read is a read of a field.** A value is a tree, and each subtree
has a version. Page breaking (§994) reads a box's height and depth,
not its list, so a paragraph whose text changed and whose lines kept
their heights leaves every page-builder step after it unchanged. `\wd`
(§420, `box_dimen`) reads one field of a register's box. This second
tree, the *value tree*, is as much of the hierarchy as the call tree
is: the finer the reads, the earlier the cutoff, and both are set by
the representation, not by a policy.

**There is no cell list.** The engine has no way to read state except
through a value, so every read is recorded by construction; §11's
"hidden state" is closed rather than defended. Nothing is keyed on a
control sequence's name (§7.16): `\r@foo` is a slot of the table like
any other, and `.aux` is an address the document chose.

**Positions are data.** A line number is a value: `\inputlineno`
(§424) reads it, an error prints it, and a call that read it depends
on it. It is never a call's identity (7.17.2) and never part of a key.

**Versions for slots.** A version is a content hash for a value that
is built (a list, a box, a token list, a stream). For a slot that
holds a scalar (a table entry, a register, a catcode) a *revision* is
an equivalent version: the slot keeps the revision at which its
content last changed, and a write of an equal value leaves it, so two
reads see the same version iff the content was the same.

**Definitions, not snapshots** (*corrected*, the user, 2026-09-28
21:52: the text of 21:00 kept the state at each call's start as shared
roots, which is a snapshot by another name, and a snapshot has no place
in SSA). A write of a slot is a *definition*: it keeps its value, its
place in program order and its readers (the reads that saw it). The
state at a point is stored nowhere. It is, slot by slot, the
definition that reaches the point, the last one before it. A call that
runs again in the middle of the document reads exactly that: a read of
a slot whose latest definition lies after the call's place resolves,
through the slot's definitions, to the one that reaches the call, and a
write there is a new definition in that place, which the slot's next
definition ends. The engine's flat arrays hold every slot's latest
definition, so a read whose slot was not defined again later (nearly
every read: the preamble's macros, the fonts, the catcodes) is an index,
as in a plain run. Nothing is copied at a boundary, nothing is restored,
and no call walks from the start.

A revision is a version only within one store's life. partex starts
each build from a fresh engine, so a revision says nothing across
builds (step 2, LOG 2026-09-28): the parallel array holds the slot's
*content* version instead, made by the writing accessor once it has
stored the value (step 2d; step 2 computed it lazily at the first read
after a write, a stopgap now gone), so a read costs an index; a writer
that stores a table wholesale past its accessors (the engine's initial
tables, a format's load) versions the whole table. A family with no
content version yet keeps revisions tagged with the build, which never
compare equal across builds (a miss, never a false hit).

#### 7.17.2 Calls

**Every operation is a call**, `v = f(a₁ … aₙ)`, and `f` is an operand.
A macro's meaning is a value, written by `\def` (§1218) and `\let`
(§1221); a call through a redefined macro is a call with a changed
operand. The engine's routines are calls of the same kind: the
tokenizer over a line (§343–§356), `expand` (§366) and `macro_call`
(§389), `hpack` (§649) and `vpack` (§668), `line_break` (§815), each
step of `build_page` (§994), the output routine (§1025), `ship_out`
(§638), `write_out` (§1370), `start_input` (§537), `unsave` (§281),
and the in-process tools (BibTeX and makeindex, §7.10). Primitives and
macros are one category.

**A call's record** holds its name (below), its body's own reads in the
order they happened (a version each; a child's reads stay in the
child's record, and the parent depends on the child by its result), its writes (the new versions), its effects
(the page it shipped, the lines it stored, what it logged), and its
children (the calls its body made, in order). The records are the
graph, and the last build's records are what a session keeps (7.17.7).

**Evaluation.** To evaluate a call: find its records by name and take
the one whose subtree's reads from outside it hold the versions now
current. The test walks the record in program order with the set of
slots the call has written so far: a read of the record's own body is
compared unless its slot is in the set (the body's first write of a
slot is marked in its record where it happened), and a child's record
is walked by the same rule, its writes joining the set when it ends.
This is exact and copies nothing: a read the recorded run made of a
slot the call itself had written is inside the call, its version
following from the call's other reads, and a child's reads stay in the
child. If the walk holds, the call is a *hit*: its writes are put in
place, its effects stay in its record for the link (7.17.10), and its
children are not visited. At the first read that differs it is a
*miss*: the body runs, and every call the body makes is evaluated by
the same rule, so a body that re-runs still reuses each child whose
reads held. The reads are checked in order because a later read's
address can depend on an earlier read's value (`\csname`, a register
number scanned from the input); in order, each read is checked in the
state the recorded run saw it in.

**Nothing is named by where it is.** A call's name is `f` and the
versions of its inputs: for a paragraph, the token list it consumed;
for the `hpack` inside it, the list and the spec; for the tokenizer,
a line's bytes and the catcodes. Versions exist already, computed
when each value was made, so a name costs nothing to form, and a
lookup is one hash. Records with the same name are told apart by
their reads: the lookup keeps a set per name and takes the record
whose recorded reads verify, so the same paragraph in two chapters
has two records (its page number differs), and if the reads are equal
too it has one, which is correct: the same inputs and the same reads
give the same outputs. A record is pure given its reads. No position,
ordinal, line number or identity of the source enters a name, so a
moved paragraph is the same node, a line typed again with two spaces
tokenizes to the same value and everything after it is a hit, and
nothing is renamed through a diff. An effect (an object the PDF
writer makes) is named by its record and its ordinal among that
record's effects; the link orders effects in program order, as it
does now.

**Which calls are recorded** is a performance rule with no correctness
content. A record costs about a hundred nanoseconds (a lookup, the
reads' comparison, the writes' versions), so a call is recorded if its
body usually costs more than that: the routines above are; the
arithmetic inside them is not; `hpack` over a list is one record, never
one per addition. The results are identical at any grain, so the grain
is set once by measurement and never adapted at run time.

**Two rules the runtime's construction fixed** (step 1, LOG
2026-09-28). A call threads its accumulator as an *argument*: the page
builder's page-so-far is an input of each step, so each step has a
name of its own; were it a read, every step over the same box would
share a name and the set per name would grow with the document.
Ambient state (the table, the fonts) is read. And a write of an equal
value is kept in the record although it leaves the state alone:
replayed into a state where that slot differs, it must set it.

#### 7.17.3 Change propagation

An edit replaces one line object of the source (7.17.4). Every value
keeps its readers, and a rebuild runs again the readers of what
changed, then the readers of what their re-runs wrote differently, and
nothing else (*corrected*, the user, 2026-09-28 20:55: this sentence
said the rebuild was evaluated from the root, which made every record
check itself after any edit): the tokenizer call of that line misses and makes new tokens; the
paragraph that consumed them misses and re-runs `line_break`; if its
lines' heights and depths are the old ones, every `build_page` step
that read them hits, the page's `ship_out` misses (the page's bytes
changed, which is the output), and every call after it hits. The work
of a rebuild is the sum of the bodies that ran, and each ran because a
version it read had changed. Nothing runs because a pass does, a
region ends, or a lump differs.

**The rebuild, as built** (2026-09-28). The top level is the fold of
7.17.9, a sequence of steps. Each slot keeps its definitions (7.17.1)
in program order, each with the step that made it and its readers: the
steps whose subtree read it from outside the step, the definition then
having been made before the step began.
1. The edit's lines (7.17.4) are definitions that changed, and their
   readers are the first dirty steps.
2. The dirty steps run in program order, each at its place. Its reads
   resolve to the definitions that reach it (7.17.1), the new build's
   where a step before it ran again. Its input begins where the step
   before it ended, in the new source. It ends at the next clean point.
3. The step's definitions replace its old ones. A definition whose
   value differs from the old one's, or one that only one of the two
   runs made, makes dirty the old definition's readers, up to the
   slot's next definition. One whose value is equal changes nothing:
   that is the cutoff.
4. A step that ended where its old run did goes on to the next dirty
   step. A step that ended elsewhere (a paragraph break deleted, an
   Enter) runs on, a step at a time, until one ends at the start of an
   old step at the same line. The old steps passed over are gone with
   their definitions, and a reader of one of those now reads the
   definition before it, so it is dirty if that one's value differs.
5. An *append* (the page's list, a stream's lines, 7.17.5) is a
   definition that does not end the one before it: a list's value at a
   point is its appends since its last whole definition. A changed
   append makes dirty only the readers of the whole value (the fire, a
   load), never the steps that appended after it. The glyphs the PDF
   writes are one (written 2026-09-29, after the course's word edit
   re-ran every ship after `pdf:30`, since each ship read and wrote the
   fonts' glyph sets): each `pdf_ship_out`, of a page or a form,
   appends the glyphs it used, by font (`Row::Glyphs(n)`, the n-th
   ship's; `pdf::ship::Glyphs`), and the job's end, which writes the
   fonts, reads the whole value (`glyphs_union`, before
   `output_fonts`). A font's version leaves its glyphs out, so a ship
   reads no glyphs, and a page whose glyphs changed makes dirty only the
   job's end.
   The page's list is one too (written 2026-09-29 before the code,
   after the course's word edit re-ran the step holding ch15's pgfplots
   figure: `build_page` put each node on the page by changing the list,
   a read of the whole list, so every step that contributed to a page
   read the list the edited paragraph had appended to). Each node the
   page receives is an append, `Row::PageNode(k)`, the page's `k`-th
   node (`Fam::PageNode`, versioned by the node). The list's length and
   whether its last node precedes a break (`LIST_LEN`, `LIST_TAIL`,
   §1000) are slots of their own, versioned by their values. A page step
   reads those two, never the list. Putting its node on the page
   (§998) reads the length and writes the node, the length and the
   tail. The fire (§1012–§1017), the output routine's end (§1026), the
   default output (§1023) and the page's display (§986) read the whole
   value: the length, and the nodes below it. What changes the list
   whole writes the length, the tail and each node it leaves. So a
   paragraph whose lines changed makes dirty the fire that packs its
   page, and a later contribution only if the page's length or tail
   changed. To place a step, the length, the tail and the nodes the step
   reads are put: the engine's list is made that long, with the nodes
   the step reads in their places and, elsewhere, stand-ins whose kind
   gives the tail (a stand-in is never read: a step that reads node `k`
   has it placed, or runs again with it). A node the step reads is
   always placed, even where no definition after the step's key would
   hide it: the engine holds the list, not the nodes past its length,
   so no array keeps a node's latest definition for it. For the same
   reason, when a dropped run's writes go back to what reaches the step
   and the length is among them, the nodes the step has placed go back
   with it (written 2026-09-29, after `incr`'s first edit shipped its
   page 5 short: the run that fired it read a slot unplaced and was
   dropped, its fire had cut the list to the page's remainder, the
   length put back padded the list with stand-ins, and the next run
   placed only the slot it had missed, so the fire packed stand-ins).

Nothing else is visited. A step that no changed definition reaches is
not looked at, so the work of a rebuild is the dirty steps' bodies and,
for each definition that changed, the lookup of its readers.

*The forms these take* (2026-09-28, written before the code):
- **A read resolves by prediction and validation** (7.17.6's rule, in
  one step). The engine's arrays hold every slot's latest definition.
  Before a dirty step runs, each slot that its last run read from
  outside it, and that a definition at or after its place defines,
  takes the value of the definition that reaches the step (none: the
  slot's value before the build defined it). The old reads predict
  the new ones. When the step ends, its outside reads are known
  (`Runtime::open_step_reads`). If one is of a slot that a definition
  at or after its place defines, and it was not predicted, the step
  read a later value: its run is dropped (`Runtime::abort_step`) and it
  runs again with that slot resolved too. After the step, the arrays
  hold the latest definitions again. A slot the step read at an
  earlier definition, or defined where a later step defines it too,
  takes that later definition's value.
- **The save stack is placed whole below its pointer** (written
  2026-09-29 before the code, after `edits`' second edit panicked in
  `unsave`). The deferred fire at a display's start (§1145) made a new
  step begin inside the display's group. Its `unsave` read the group's
  entries, which no old run had predicted, at their latest definitions,
  which a later group had laid out. A saved value was read as an
  entry's word, and gave an eqtb index past the table.
  - A dropped run works only when its later value is still a value the
    engine can run on, and the save stack's entries are a layout
    (§268: an entry's word, then the value it saves).
  - So before a step runs, these take the values that reach it, as
    predicted reads do, wherever a definition at or after the step's
    place defines them: each entry below `save_ptr`, and the stack's
    other slots (`save_ptr`, `cur_level`, `cur_group`, `cur_boundary`,
    e-TeX's chain of saved registers).
  - The cost is in the entries a later step rewrote: a group's local
    assignments.
- **The input is the step's result.** Where a step left the input is
  its result (7.17.12: the input position is the call's result): the
  input levels, each file's data and where its next line begins, the
  lines in the buffer, the token lists below the file, and the line
  numbers. A dirty step's input is the result of the step before it,
  its positions mapped into the new source through the edit: a
  position after the edit moves by the edit's length. A position
  inside it cannot occur, since the step that ended there read the
  line and is dirty itself. That holds because an edit is found by
  lines (written 2026-09-29 before the code, after `edits-dvi`'s
  `.aux` came back with a page number one digit longer in most of its
  lines, one span of bytes from its 75th to its end with unchanged
  lines inside): the common prefix and suffix are cut at line
  boundaries, and the lines between are diffed as lines (Myers' O(ND)
  diff), so an edit is a list of hunks. In a run of changed lines the
  old and new lines are paired in order, each pair a hunk of its own
  (a line changed), and the lines left over, removed or added, are one
  more hunk: so the place between two changed lines maps too (the
  `.aux` above, every line changed, maps line by line). A position
  between two hunks moves by the hunks before it; a line read is
  dirty if a hunk touches it. A diff costlier than a bound (more than
  1,000 lines changed) is one run of changed lines, paired the same
  way. A step *ended where its old run did*
  (item 4) when its result, so mapped, is the old one's. A `\read`
  stream's value (7.17.12's `read_file` row) is a file's data and a
  position too, and is mapped the same way wherever a step is placed
  at it: each edit applies to the data it replaced, so a value made
  before an edit takes it and one made after does not. A step that
  reads a line of the stream after its `\openin` is dirty by that
  line, and its run reads the new line, not the data the step that
  opened the stream found.
- **Lines are of a data, and an edit is of a data** (written
  2026-09-29 before the code, after `edits-dvi`: LaTeX reads its
  `.aux` file by lines twice, the φ at `\begin{document}` and the
  job's own store at `\end{document}`, and the lines of the two were
  kept as one file's). Each load's contents is one data, the source
  its lines are read from: a file's, a stored name's φ, or a value
  served from the steps' stores. The lines a step read are of a data,
  and an edit turns one data into another: the lines it changed are
  dirty, and every other line and position in it is mapped (the rule
  above). A rebuild makes the edits:
  - at its start, of each data a load found in the host's file, the
    file as it is now: for a stored name, the φ now (what the last
    trip stored). A data served from the stores is not compared, since
    only the stores change it;
  - inside the rebuild, where a step that ran again left a file level
    holding other data than its old run did (a load that found another
    φ or another stored value): the edit from the old data to the new
    one, made at the step's end and applied before its end is compared
    with the old one. The new data may hold the old bytes (a load that
    found the same value again): the edit then has no hunks, and moves
    the lines and the positions read in the old data to the new one,
    so a later edit of the new data reaches them (`edits`' `.aux`, read
    at `\end{document}` by steps that did not run again, kept its old
    page numbers in the `.toc` without it).

  Two loads of one file are two data, each with its edit.
- **A rebuild's file checks cost the files that changed** (written
  2026-09-29 before the code, after the course's word edit spent 13%
  of its rebuild, about 19 ms, reading again every file the job had
  loaded, its packages and fonts too, to find the one edited, and
  looking up again the 365 names it had not found). The host knows
  what a lookup and a file on disk are without making or reading them:
  `Host::unchanged(loads)` says, for each load (its name, its kind, the
  contents `read_file` gave, or none), whether `read_file` would give
  the same again. It is the rule of a watch (§7.4, "Seeing an edit")
  and of a restart (the stamps above), with the lookup kept too:
  - *A lookup keeps its trail*: the candidates it tried and did not
    find before it found the file or gave up (in the output directory,
    in the path's elements no ls-R database covers, and a database's
    candidate that was not there), by directory, each directory with
    its stamp after the search. While no candidate is there, the lookup
    finds what it found. Creating or removing a file changes its
    directory's times, so a directory that has its stamp holds none of
    them. In a directory changed since, each candidate is looked at: an
    editor that saves by renaming changes the document's directory at
    each save, and a stamp of it alone would make every lookup again.
    The databases are read once per process, as kpathsea reads them.
  - *A file found keeps its stamp* (size, modification and change
    times, inode, device), taken just before its contents were read.
    It is as it was while the path has that stamp; the contents then
    equal `last` (compared, since two names can find one file).
  - A stamp, of a file or a directory, is kept only if its times were
    over 2 s old when taken, since a later write within the same clock
    tick could leave them alike (git's racy entries). A file just
    written is read again at each rebuild until it is quiet, and the
    candidates in a directory just written are looked at one by one.
  - One call answers every load, so each directory is looked at once.
  The default answer is false, and each load is made again and
  compared, as before; so is every load with `PARTEX_STAT_CACHE=0`.
- **A line number read is kept by position** (written 2026-09-29 before
  the code, after `edits`' last edit, an empty line inserted, left the
  later steps' box reports with their old `at lines` numbers). A line
  number is a value ("Positions are data"): a step that read one of a
  level open at its start (`\inputlineno`, an error's `l.N`, a box
  report's lines, 7.17.4's `line:j±d`) keeps, in the data that level
  was reading, where the level was at the step's start. An edit that
  changes the number of lines before that position moved the number
  the step read, and makes it dirty; otherwise the position moves
  with the edit, as a line read does.
- **Allocated numbers are not positioned.** The pool's end
  (`str_ptr`), the hash's allocators (`hash_used`, `hash_high`), the
  pool's strings and the hash's slots are numbers the build hands
  out, and no number is observable: a string is read by its
  characters, a control sequence by its name (`Fam::Name`). A step
  that runs again allocates past the arrays' ends. Their definitions
  are never resolved backwards and make no step dirty, and a slot a
  run no longer allocates keeps its value, since a name once made is
  found by content.
- **Output is linked from the steps' records.** Each step's record
  keeps the effects of §7.6 that its run made, gathered at the step's
  end (`Tex::take_effects`: the log's and terminal's bytes, the PDF's
  bytes with its object marks, the cross-reference as data, the byte
  counts as holes, the files opened with their names, and the `\write`
  files' bytes). After a build, the link lays every file out from the
  live steps' effects in program order (`effects::link`) and writes
  it. The files are a view (7.17.5): the build writes only the `\write`
  files as it goes, since the job reads them back. With the link,
  where in the PDF file the writer is (`gone`, and the offsets kept
  from it) is not state. TeX never observes an offset, and the link
  places every object, so the writer's version leaves it out, as §7.6's
  state hash did. The per-call runs (`Tracker::output`) stay in each
  call's record for check mode. Linking a hit applied inside a step
  needs that call's own effects in this form, which comes with the
  applied hits.
  *The link after a rebuild is a watch's link* (§7.16.6; written
  2026-09-29 before the code). On the course's word edit it took 76 ms
  after the rebuild's 185, a third of the whole: every stream was
  deflated again, and every step's records were walked to gather its
  effects.
  - Each step keeps its effects' chunks, one for each of its calls
    that made effects, each with its version (made from its content),
    by step id, as its run ends. A dropped run's chunks are replaced by
    the run after it. Gathering the chunks then costs the live steps,
    not their records.
  - The link is `effects::link_cached`, each chunk keyed by its step's
    id and its place in the step. A chunk is taken from the cache when
    its version is the one last linked under its key and its entry is
    the same: the object-stream counters, the file whose stream is
    filled, and the numbers it writes. A step run again with the same
    effects is taken again.
  - Deflate is memoized by content. A file is written when its bytes
    changed, or when a step run since the last link opened it: the
    host's open makes the file anew (`File::create`), so its bytes on
    disk are no longer the ones last written.
  - `PARTEX_LINK_SPLICE=0` links in full and writes every file, as
    before. A debug build checks each cached link against a full one.
- **Hits applied inside a step that runs again** (written 2026-09-29
  before the code, after the course's word edit spent 31% of its
  rebuild in the job's end step: 7 commands, which write every font
  again though no font's glyphs changed). This is 7.17.2's evaluation,
  made in the rebuild: a step that runs again evaluates each call its
  body makes, and a hit is applied, not run.
  1. *The probe.* A call is looked up by its name, and its records'
     reads are compared with the state its body would read (7.17.2's
     walk): the engine's, as the step's run has placed it.
  2. *The hit.* Its writes are stored (`Store::set`). Its effects come
     back where its body's would be, as chunks of the step: every output
     is an effect in the link's form (the log, the terminal, `\write`
     files, the DVI and PDF files), so its chunks hold all it printed.
     The bytes its record keeps by output are check mode's; a build with
     no effects in the link's form prints them again through their
     outputs. It becomes the running call's child, and its body does not
     run.
     A step's effects in the link's form are cut into chunks at the
     boundaries of the calls that apply (item 4):
     - at such a call's start, what the step made so far is a chunk, an
       effect of the call running then;
     - at its end, before its writes are versioned, what its body made
       is its chunk, an effect of its record;
     - a hit applied gives the step the chunks of its record's subtree,
       in order;
     - the step's end cuts the last chunk.
     The step's chunks, in program order, are the link's (keys
     `step << 32 | k`, "The link after a rebuild is a watch's link").
     A cut finds nothing to take at most calls (an `unsave` rarely
     prints), and an empty cut makes no chunk. Found by the course's word
     edit: with the bytes printed again besides the chunks, each font
     file's name was twice in the log and its stream twice in the PDF.
  3. *Its reads are the step's.* Each read of the hit's subtree from
     outside it (the walk's) is a read of the step too, unless the step
     wrote the slot before. So the step is its reader in the fold, and
     the run's check (item 2's) takes it with the body's own reads. A
     probe that compared a slot a later definition holds, one the run
     did not set, drops the run. The run is made again with that slot
     set, and the probe with it.
  4. *Which calls apply.* A call applies its hit only where its record
     holds everything the call makes: its writes and its effects, with
     nothing handed back to its caller outside them. Those are
     `unsave` (§281) of a group with no token to insert, and the fonts
     written at the job's end (item 5).
     The others run, and probe only in check mode:
     - an `unsave` that inserts tokens (`\aftergroup`'s, §326) puts
       them in the input, which is outside its record, and `back_input`
       may first end finished token lists. It is a routine of its own
       (`unsave_after`), named as `unsave` is. Found by the cold build of
       `incr` with hits applied: `\footnote`'s `\aftergroup\@foot` was
       lost and `\bye` came in internal vertical mode.
     - `hpack` and `vpack` hand back a box, and `line_break` its lines
       through `post_line_break`;
     - a page step hands back what `build_page` does next, and the
       output routine the input it leaves;
     - `ship_out` and `write_out` are written through writer scopes
       that are not yet checked for this.
     Each becomes an item here once its result is a value.
     `PARTEX_SSA_APPLY=0` applies no hit in a rebuild; a cold build
     applies hits only with `PARTEX_SSA_APPLY=1`, as before.
  5. *The fonts are calls* (writefont.c's `writefontstuff`, after
     pdfTeX §794's `do_pdf_font` for each font). The calls:
     - a font descriptor with its font file stream (`FontFile`);
     - an encoding's differences (`Encoding`);
     - a font dictionary with its `ToUnicode` map (`FontDict`).
     Each is named by the entry it writes: its tree's key and the
     entry's content. Each is a writer call (`scoped_call`), so its
     reads are the fields its parts read: the font writer (`FONTW`,
     whose subset tags each font file adds to), the object numbers, and
     the parameters, and the font file is a load. When no font's glyphs
     changed, each is a hit, and the job's end writes none of them
     again.
- **A load reads the store, not the file** (7.17.5; written
  2026-09-29 before the code, after `readback` showed a load's outcome
  kept per name: its two loads of `readback.lab`, before and after the
  job wrote it, were one outcome, the last). A stored name's value at a
  point is its appends since its last open (item 5), and before the
  job's first open of it, the φ: what the last trip stored, which in a
  process's first build is the file on disk. Each step keeps the stores
  its run made, in order (an open of a name, a line appended to it),
  and each load it made: the name, the version of what it found, and
  whether it read the φ. A load of a stored name inside a rebuild is
  served that value from the steps' stores, in program order, and never
  from the host's file, which a step re-run out of order may have
  truncated. One trip is one rebuild, as one `latex` run reads what the
  last one wrote.
  - At a rebuild's start, a load that read the φ is dirty if the φ now
    (what the last trip stored) differs from what it found.
  - A step whose run stored differently to a name (an open added or
    gone, a line changed) makes dirty the loads of that name after it
    that read the build's own store; a step passed over (item 4) stored
    nothing. The loads that read the φ are not dirty for it: the next
    trip reads it.
- **A load is looked up as its kind** (2026-09-29, after the
  `cutoff-pdf` rebuild looked up the font map and the Type 1 fonts as
  TeX sources and found none): each load keeps the kind of file it was
  looked up as (`FileKind`: a source, a TFM, a map, a font, an image),
  and a rebuild looks the name up again as that kind.
- **A stream's version is its file** (2026-09-29, after `incr`'s
  `.toc` came back empty from its third rebuild): `Out(n)`'s version is
  its name and its file handle (`WriteId`), so a file opened again is
  another version, and the job's end, closing every `\write` file
  still open (§1333), writes `Out(n)`.
- **A family's stamps are its own** (2026-09-29, after `incr`'s DVI
  came back short: `Alloc`, `Pdf` and `Dvi` shared one array of the
  per-call read stamps, so a read of one hid the next read of another
  in the same call). The dense stamps that note a table read once per
  call (`SsaTracker::boundary`) are one array per family.
- **A query is asked again** (written 2026-09-29 before the code, after
  `effects` kept a touched file's old `\pdffilemoddate`). The host's
  answer to a query that is not a load (7.17.12's `Clock` reads) is a
  read whose version is the answer, so by the invariant its step runs
  again exactly when the answer changed. Each step keeps the queries
  its run asked, each with the version of its answer, and a rebuild
  asks them again at its start, as it looks up the loads:
  - a file's date (`\pdffilemoddate`) is the host's date for the name
    now;
  - the clock (the job's start printed in the log's banner and in
    `\dump`'s, the creation date) is the host's time now: its date to
    the minute, as TeX reads it, and its creation date, one answer per
    trip (`Tex::clock_answer`);
  - the timer (`\pdfelapsedtime`, `\pdfresettimer`) is the host's
    seconds and microseconds now, which a real clock never answers
    the same, and a reproducible host always does;
  - a line from the terminal (§71) cannot be asked outside a run, so
    its step is dirty in every rebuild.

  A step whose answer differs is dirty. Inside a build, a call that
  read a query is never a hit (the `Clock` slot names no query, so a
  probe cannot ask it again); its step runs whenever the call is
  reached.
- **Not built**: a dirty first step (`Func::Start`: the format's load
  and the job's start). Its definitions are the format's, so the
  initial values come with it. The timer's start at the job's start
  (pdfTeX's, before §1337) is a query of that step, so it is not asked
  again: a rebuild's `\pdfelapsedtime` counts from the first trip's
  start.

State is threaded: a call's writes are the next call's reads. So the
cutoff is by value: a re-run that writes the versions the old one wrote
leaves its successors' reads equal, and they hit. A paragraph inserted
so that a line moves to the next page misses that page's builder steps
and both pages' ships; the headers of later pages hit unless their page
numbers changed. A new page changes `\count0` for every later page, so
every later header call and ship re-run, tens of microseconds each,
and the paragraphs beneath them hit: that is output, not overhead. A
`\pageref` whose number changed re-runs its paragraph (its tokens
changed), and if the number's width is the same the lines are equal
and the change stops there.

#### 7.17.4 The source and the tokens

A file is a persistent sequence of lines, an ordered tree, so an
edit makes O(log n) nodes and shares the rest. A line's number is its
rank, a derived value computed when a call reads it (a field read of
the sequence, never a slot the root writes before each paragraph,
which would put a write per paragraph in the root's record), and
nothing else about a line's place is visible: the tokenizer is a call per line over
its bytes and the catcodes it read (§343–§356), named by them
(7.17.2), so the token stream is persistent too and shared wherever
the bytes are; a changed comment or a doubled space tokenizes to equal
tokens and everything after it hits (§7.2's rule, by construction).
The engine consumes tokens and never sees the file as numbered lines;
`\input` (§537) is a call over a file value keyed by content.

*What building showed* (step 2b, LOG 2026-09-28). A call reads the
source relative to where it started, and per input level: a call that
starts in a chapter can run past the chapter's end into the file that
`\input` it, so a read is (the level open at the start, the rank after
that level's position then, the codes), not (file, rank). A line's
tokens are its value only if the codes held while it was in the
buffer: TeX tokenizes lazily, so `\obeyspaces a  b` on one line makes
the rest of the line depend on the change, and tokens under the codes
at the line's start would give a false hit. Such a line is read by its
bytes. The read carries the codes it was tokenized under, because
verification must tokenize the new line under those, not under the
codes at the call's start. A printed line number whose level is not
known (a paragraph's first line in a box report, a conditional's) is
read against every level open at the start. Which of the engine's
fields are the input state, and which are scratch at a call's
boundary, is 7.17.12's inventory.

Token lists are values: `\edef` makes one, `macro_call` (§389) is a
call whose `f` is the meaning and whose inputs are the arguments'
lists. Boxes are values: `\setbox` (§1241) writes a register, a list
holds them, and their dimensions are fields (7.17.1). The save stack is
a value, and `unsave` (§281) is the call that reads the entries it
restores, which is the read the audit's top candidate lacked
(§7.16.4). The page builder's state is a value, `build_page` (§994) a
call per step, a fire the output routine's call over `\box255`, the
insertion boxes and `\outputpenalty`, and `ship_out` (§638) a call
whose effect is the page. The link (§7.15, §7.16.6) is a call over the
effects.

#### 7.17.5 Stores, loads and the cycle

`\openout`, `\write` and `\closeout` make a **store**: a stream is a
value, a sequence of lines, and `write_out` (§1370), expanding the
list at ship-out, appends to it. `\openin`, `\read` (§482) and
`\input` of the same name are a **load**, and the host forwards the
stored value to it in memory. Whether the name exists is a value too:
the first trip's load of a stream the job has not stored yet sees no
file, as TeX does. No name is special: `.aux`, `.toc`, `.lof`, `.nav`,
`.out` and `.bcf` are addresses the document chose, and the chain from
a store to its readers is ordinary: `write_out` → the stream's lines →
the tokenizer at the load, under the catcodes then in force →
`\newlabel` → the table slot → `\ref`.

**The cycle is a φ.** A load of a name the job itself stores reads, in
trip k, the value trip k−1 stored, and in trip 0 the value the previous
build's last trip stored: `load = φ(previous build, trip k−1)`. The
job has converged when the value each load read is the value the same
trip stored, found by comparing the versions the stores wrote, not
files. Trip k+1 evaluates the build again by 7.17.2's rule, so it runs
exactly the readers of the lines that changed: a moved label re-runs
its `\newlabel`, the `\ref`s of that slot, and what their new widths
push along. "Two passes, then biber, then one pass" is a cycle whose
later trips are nearly all hits. Trips are bounded as latexmk bounds
them (five, then the last trip's output and a report); a document with
two fixed points (a section its own table-of-contents line pushes onto
the next page) reaches the one the previous build's stores lead to,
which is what latexmk reaches from the files on disk.

**Outside tools are calls.** `bbl = biber(bcf, bibs)`: its inputs are
the `.bcf` stream's value and the `.bib` files' contents, its output a
stream `\input` loads, and it runs iff an input's version changed,
never because a recipe says so; it runs in the sandbox, as §7.16.10's
exception to §0.6. BibTeX and makeindex are in-process calls (§7.10).
Which tool reads which stream is fixed in partex, not configured (the
user, 2026-09-28): biber over the `.bcf` stream and the `.bib` files,
BibTeX over the `.aux` stream's `\bibdata`, `\bibstyle` and
`\citation` lines and the `.bib` files, makeindex over the `.idx`
stream; each makes the stream its reader loads. There are no passes:
TeX, BibTeX, biber and makeindex are never run in turn over the whole
document. Each tool is a node of the one graph, and the graph converges
where a version changed: a new `\cite` changes the stream's value, the
tool's call misses, its output stream changes, and the loads of that
stream and what they push along re-run, while every other call hits.

**Files are a view.** The build holds the streams; `partex` writes
`.aux` and the rest to disk only when asked, for outside tools, and
never reads them back. A fresh build with no previous one starts from
empty stores, as a first `latex` run does.

#### 7.17.6 Speculation and parallelism

Trip k+1 needs trip k's stores, so ten independent trips would compute
nothing. What is sound is **speculation with validation**: trip k+1
takes each load's value to be trip k−1's and runs; a store of trip k
that differs invalidates the readers of that value, which run again.
On a keystroke, this is the one-trip case. On a cold build, the trips
overlap in wavefronts, and the PDF is made once, at the fixed point:
until then only the calls that reach a store need to run.

Inside a trip, calls run on the pool with their reads predicted from
the previous build (§7.16.9's warm units, with the reads now exact) and
are validated in program order; effects commit in program order (§0,
principle 5), so scheduling never leaks into output. The hashing of
new values is off the critical path: a version is needed only when a
later call verifies against it, so it is computed on the pool while
the engine runs on. That, and not extra trips, is the cheap parallel
work.

**The cold build** gains three times over latexmk. *Work*: trip 1
runs everything once, with recording; trip 2 runs the readers of the
stores that changed (the `\ref`s and `\cite`s that turned from `??`
into text, the paragraphs they reflow, the table of contents, the
pages whose breaks moved); trip 3 is usually nothing; with slicing
(7.17.11) no PDF bytes exist before the fixed point. About 1.1 runs
instead of three, and bytes once. *Latency*: across trips, `\ref{foo}`
in trip 2 needs only the store from the ship of the page that carries
`foo`, so trip 2 runs a few pages behind trip 1 as a wavefront and each
φ folds as its store becomes final; within a trip, paragraphs run on
the pool with their reads speculated from the state after the
preamble and validated in program order. *Time to the first page*:
page 1 ships in trip 1 and a viewer can paint it, with a `\ref` to
page 40 showing `??` until its φ folds and the page repainting then:
first paint approximate and fast, final paint at the fixed point, from
one graph. The floors: total work is at least one run, and latency is
at least the critical path, the page chain with validation.

**Divide and merge.** Written out, the speculation has the shape of a
parallel divide and conquer. *Divide* at the calls of the tree
(chapters, sections, paragraphs, down to commands, 7.17.13), never at
raw lines, since how a line tokenizes is state. *Conquer*: run every
chunk in parallel from a predicted entry (the previous build's state
there, or the state after the preamble), each run a record of its
reads, writes and effects; a chunk's meaning as a function of its
entry is not computable (TeX is Turing-complete), so the record under
a guessed entry plus validation is what stands in for composing
functions. *Merge*: writes compose associatively (the later write of an
address wins), so the state after chunks 1…k−1 is a parallel prefix
over the deltas, O(log n) deep; each chunk's reads are then verified
against its prefix state, all in parallel; the first chunk that fails
re-runs from its true entry, which is a round, and rebuilds need one.
Counters chain additively: a chunk that only increments records `+k`,
and the prefix sums give each chunk its entry value; a chunk that
typesets or tests the number has read it and is validated like any
read. The sequential spine is the page builder: chunks run with it
recording contributions and never firing, and the fold over the
accepted chunks finds the fire points and runs the output routines
there; the fold consumes boxes it did not make, a few percent of the
work, and its length is the number of pages. `\chapter` clears the
page, so the fold is independent per chapter and only the page number
chains across chapters, additively. This is §7.16.9's warm rounds with
exact reads, which is what makes the validation hold where it failed
while `Rest` was one cell.

#### 7.17.7 Records, memory and sessions

A session (§7.9) is the last build's records and the values they
reference, deduplicated by content. A record is O(its reads, writes,
effects and children), and values are shared, so memory is bounded by
what the build touched, not by the state's size times the number of
records. There are no checkpoints, snapshots or restores: the state is
the slots' definitions (7.17.1), and a call that runs again reads the
ones that reach it. Records the new build did not reach are kept for a
few builds (undo) and then dropped, and their definitions with them.

**The collector** (the user, 2026-09-28). Which records a session keeps
is a policy, and it is split from the mechanism that applies it. The
mechanism marks from the roots of the builds kept and drops every
record none of them reaches; values need no tracing, because they are
immutable, so they form no cycles and reference counting frees them
exactly, and a record keeps alive the values it holds. The policy is a
function that sees each record's age (builds since its last hit), its
recorded cost, its size and its hits, and answers keep or drop within a
memory budget; the budget is the pacing knob (collect when the arena
has grown by a factor, as Go's `GOGC` is), and keeping records by age
alone is the first policy, not the only one: a record whose body is
expensive may outlive its age, so a branch that comes back after many
builds is still a hit. A record kept is still verified by its reads
before it is used, so no policy can make a hit wrong: a record
collected costs a miss. The collector runs on its own thread through
the `Executor` (§0), not on the keystroke's path: records never change
once made and the roots change only when a build ends, so it snapshots
the roots then and marks while the next build runs, with no barrier on
any write; records made while it marks are live; it builds the pruned
lookup table beside the one in use, and the swap is one store at the
next build boundary. Switched off, it runs inline at that boundary,
with identical output. Built with speculation (7.17.10 step 5); today's
runtime collects inline, by age (the last three builds' roots), when
the arena has doubled.

The records are dumpable as text
(§9): the trace is an output of the build, like LLVM's IR is of a
compiler, not a debugging afterthought.

#### 7.17.8 Exactness and gates

Recording off is a plain run, with identical output (§0, principle 4).
Check mode runs every hit again and compares its writes and effects,
which is the sanitizer's successor; differential runs over the corpus
and the course compare recorded builds with plain ones. `bench/edits.sh`
and its rows are unchanged, and §7.16.7's last column is the target:
a space or comment under 1 ms, a word about 5 ms, `\label` with its
second trip under 10 ms, a footnote or a deleted paragraph 20–50 ms,
and Enter as a word.

#### 7.17.9 What this replaces, and what was rejected

| §7.16 | here |
|---|---|
| units cut at clean points, the chooser, `coarsen` | calls; a clean point is where a call ends |
| the page builder replayed as a fold | `build_page` steps as calls |
| a checkpoint as shared roots; restore by rebase | records; no restore |
| `Rest`, guards, read-set cutoff | values, verified reads |
| positions renamed through the diff (Tasks 20–24) | names by content; numbers derived |
| streams as entry logs (Task 25) | stores and loads, the φ |
| the audit's order of cells | none: every value is a value |

**The build's top level is a fold** (the user, 2026-09-28 21:00). A
step is a call from one clean point to the next (§7.16.1's outer
vertical boundaries, where a call ends), named by the tokens it
consumed (7.17.2). The root only folds the steps: it reads nothing and
depends on no step's result, so a rebuild never runs it again, and a
step depends on the steps before it only through the values it reads.
The engine ends a step itself; §7.16's machine does not cut it (its
stepping, `Step::Checkpoint`, goes with the machine). A step's
boundaries are §7.16.1's candidates, including those inside
`big_switch`'s fetch, so a LaTeX paragraph's end, where `\par` is a
macro, ends a step.

**§7.16's machine is removed** (the user, 2026-09-28 20:15). Machine
mode (`PARTEX_MACHINE`, `partex watch`'s default until now) is the
unit model's code: the region runtime (`partex-incr`, but for its
executor, §7.14, the one part the rest of the build uses), the machine
and its adapter over the engine (`machine.rs`, `adapter.rs`,
`journal.rs`'s journal, `machine_store.rs`), the CLI's machine host
and its persisted builds, the `machine` option, the chooser,
checkpoints, restores, guards and the sanitizer. All of it is deleted,
and nothing of it is kept as a fallback:
- `partex watch` keeps a session, the last build's records (7.17.7),
  and each change of the source is a rebuild by readers (7.17.3).
- `bench/edits.sh` keeps its rows and its edits (7.17.8) and runs them
  on the runtime: the cold build, then each edit as a rebuild in the
  same process.
- The gate's second e2e mode is the runtime's, with check mode on
  (7.17.8: check mode is the sanitizer's successor), cold and a
  rebuild, where it was the machine's with its sanitizer.
- The rows of 7.17.12 that are "§7.16's machine's logs and switches"
  go with it.
- §7.16 stays in this document as the record of the second design, and
  its numbers in LOG.md are the baseline 7.17.8's targets are measured
  against.

Rejected, with the reason:

- *Files as the channel between trips.* State has no reason to pass
  through a file; the files are a view.
- *Adaptive granularity as a policy.* The hierarchy is the call tree
  and the value tree; the recorded grain is a measurement, set once.
- *Calls below the engine's routines* (an addition inside `hpack`): a
  record costs more than the body.
- *Guessing the fixed point.* With two fixed points it can differ from
  latexmk's.
- *Identity by position, renamed through the diff*, and its milder
  form, *stable ids for lines*: both put where a thing is into its
  name, and a line typed again gets a new id though its tokens are
  the same. Content names survive every edit that changes no value.

#### 7.17.10 The work

One agent at a time, tasks of about thirty minutes, each gated by
exact output (trip, etrip, e2e in both modes, the course) and a
harness number.

1. **The runtime**: persistent values with Merkle versions, records,
   evaluation with reads verified in order, names by content with a
   set of records per name, stores and loads with the φ, on the stub
   language, with property tests: exactness against sequential runs,
   cutoff, a moved line, a line retyped to the same tokens, and a stub
   job that stores and loads, converging in the trips latexmk needs.
   *Built* (2026-09-28, `crates/partex-ssa`, LOG "§7.17.10 step 1"):
   `Version`, `Value`, the persistent vector (versions that do not
   depend on the tree's shape: a polynomial hash mod 2⁶¹−1), map (HAMT
   with additive node versions) and stack, the `Machine`/`Cx` traits,
   records with the external reads of their subtree found by serials,
   the memo by name, the φ from the previous trip's stores, the trips
   to five, and the LLVM-shaped text form (`Trace::to_text`/`parse`).
   The stub and its oracle are checked by property tests, including
   exact re-run sets. Rebuild bench: about 800 ns per evaluated call,
   of which the runtime's lookup, verification and apply are about
   140 ns; the rest is the stub making values (the target is 100).
2. **The engine's state as values**, type by type, each behind
   accessors: the table, the save stack, token lists, the nest's lists
   and boxes, the page builder, the PDF writer's tables, the streams,
   the source as a persistent sequence. Each step keeps trip, etrip
   and e2e identical with recording off.
   *Built, first cut* (2026-09-28, LOG "§7.17.10 step 2"): the
   runtime opened to an engine that owns its state
   (`partex-ssa/src/open.rs`: the `Store` trait, re-entrant
   `Runtime::call`, `begin`/`end` for a call that spans the main
   loop's steps, the recorder driven by `note_read`/`note_write`), the
   engine's recorder (`partex-core/src/ssa.rs`: `SsaTracker`, slots by
   family and index, `eqtb` versions by content in a parallel array),
   and `PARTEX_SSA=1` with a call per paragraph between clean points,
   named by the tokens of its first line, and a tokenizer call for
   that line. A hit is probed, not applied; check mode
   (`PARTEX_SSA_CHECK=1`) lists the state parts an applied hit would miss:
   on `pages.tex`, the source position (input stack, buffer, files),
   scratch (`cur_val`, the current token) and the families still on
   revisions (hash, strings).
   *Then* (2026-09-28, LOG "§7.17.10 step 2b"): the source a call
   consumes is read relative to its start, per input level open then
   (`source:j.k/c`: level `j`, the `k`-th line after its position at
   the start, versioned by its tokens under the interned codes `c` it
   was read with, or by its bytes if a catcode changed while it was in
   the buffer; a level's end is a read too); line numbers are reads
   `line:j±d` against the level's line at the start; `\input` and
   `\openin` are loads (the contents by name, never equal for a name
   the build opened for writing); the hash is by content (a name's
   characters, a link's layout; a lookup reads the interned name and
   the slot found). Where a call ended is its result. The recorder's
   write and read serials for dense families are arrays
   (`Machine::dense`). Not built: the replay, and with it everything
   of the state a replay must apply (the list, the page builder, the
   PDF tables, the pool); `search_string`, fonts and streams stay on
   build-tagged revisions.
   *Then, the method corrected* (2026-09-28, LOG "§7.17.10 step 2c"):
   steps 2a–2b bolted a recorder onto the mutable engine and used
   check mode to find what it missed, which is the retrofit this design
   rejects. Step 2 is the state as values, by inventory: 7.17.12 lists
   every field of `Tex` and the structs it owns with its class (value,
   scratch, effect, configuration), its value type and address family,
   why each scratch field is dead at a boundary, and the family that
   converts it; the conventions above the table fix how a version is
   made (at the write), how a read is recorded, what a hit is (stores of
   the record's output versions, the position as its result, the
   effects again) and what check mode tests. The families (tables,
   groups, tokens, lists, page, output) convert in parallel from it.
   `Runtime::replay` is renamed `apply` (a hit's writes made current).
   *Then, the tables* (2026-09-28, LOG "§7.17.10 step 2d"): 7.17.12's
   first convention built for its table rows: eqtb with `xeq_level`
   and the registers above 255, and the hash's `text` and `next`, keep
   a version array beside their entries, set by the writing accessor
   after the store from the content written (`Tracker::wrote`), and by
   the table's bulk writers (`Tex::version_tables`: the engine as made,
   a format's load); the lazy cache is gone and a read is an index.
   web2c's `search_string` is a read by content (`search:<name>`, the
   string it found, verified by searching again), so the pool's
   revisions (`str:`) are gone from records. A table read is noted once
   per call through a generation stamp per slot (`SsaTracker::boundary`),
   with no borrow and no map on a repeat. Check mode compares by
   7.17.12's value rows (`Tex::value_rows`, scratch rows left out) and
   tests the arrays: every table read's version against the slot's
   content, silent on e2e and the course. Not built: the structures'
   convention (lists, the page builder, the save stack, the input state,
   the writers' tables, the streams, fonts), so a hit is still probed
   and its body runs; `ship_out`, `write_out` and loads as calls.
   *Then, token lists, the pool and the hash allocator* (LOG "§7.17.10
   step 2e"): each `TokList` carries its polynomial, kept at every
   write, so an eqtb entry naming a list combines it and `\def` hashes
   no tokens; a string is versioned by its bytes when it is made;
   `str_ptr`, `hash_used` and `hash_high` are scalar slots. The hit is
   still probed. That LOG entry lists every remaining row with the type
   that carries its version, its writers and the expected cost: the
   nest's lists and boxes (a running version beside the node list, a
   box's made at packing), the save stack, the page builder by field,
   marks, the writers' tables, the streams, the input state as the
   call's result, the conditionals, fonts, the scalar rows, then the
   hit.
3. **The engine's routines as recorded calls**: the tokenizer,
   `macro_call`, `line_break`, `build_page`, the output routine,
   `ship_out`, `write_out`, `start_input`; a harness row after each.
4. **The cycle**: loads forwarded from stores, convergence by versions,
   the files as an emitted view, BibTeX and makeindex as calls, biber as
   a sandboxed one.
5. **Speculation**: trips overlapped, calls on the pool, validation in
   program order.

*The order, revised* (2026-09-28 13:55, the user: "make every call a
call"). Steps 2 and 3 interleave: a routine of 7.17.2's list becomes a
recorded call as soon as the rows it reads and writes are values
(7.17.12), and the rows still to convert continue after it. Step 2's
first boxes recorded only the root, the tokenizer and the paragraph;
at that grain one call writes every mutable row at once (the input
state, the page builder, the save stack, the streams, the writers'
tables), so no hit could be put in place until the last row was a
value. With 7.17.2's routines as calls, the paragraph's record holds
its children and its own reads, the hierarchy is the call tree, and a
rebuild re-runs exactly the calls whose reads changed.

*Built, the packs and the line breaker* (2026-09-28, LOG "hpack, vpack
and line_break as recorded calls"): `hpack` (§649), `vpack` (§668) and
`line_break` (§815) are recorded calls through `unsave`'s mechanism
(`Tracker::call_begin`/`call_end` with the engine as the view), named
and resulting as 7.17.12's packs' convention says. `line_break`'s
children are its lines' packs: the engine's breaker now returns each
line unpacked with its width and shift (§889), and the core packs it
as an `hpack` call, pdfTeX's expanding packer included, so a paragraph's
record holds its `line_break`, which holds one `hpack` per line. Every
§649 and §668 of main control is a call (boxes, §796's cells and rows,
§1199's display). The SSA report prints calls, probed hits and records
per routine (`ssa::RoutineCount`).
*Then* (LOG "the alignment state as values, and every pack a call"):
the packs inside the other routines are calls too, since the grain rule
makes "`hpack` over a list one record" wherever it is called.
`mlist_to_hlist`'s packs go through its `Env` (`math::Env::hpack`,
`vpack`), which the core answers with the calls; the engine's
`fin_align` is split around its packs (`align::preamble` for §801–§803,
the caller's §804 pack of the preamble, `align::set_rows` for §805 with
§806's rule packs through the caller). A pack inside math now sets
`\badness` as TeX's does (it did not). The page builder's packs,
`vsplit`'s (§977) and `fire_up`'s (§1017), convert with the page
family. The `align` row is a value (7.17.12).
*Then, the page family* (LOG "the page builder's state by field"): the
builder's 21 fields and `split_disc` are values of their own family
(7.17.12's `page` row), and the page builder's packs are calls:
`vsplit`'s two (§977: the rest, then the split), §1021's insertion
boxes, and §1017's box 255, whose report TeX inhibits by storing
`inf_bad` and `max_dimen` in `\vbadness` and `\vfuzz` around it (a
`vpack` whose name says it is quiet, `Tex::vpack_quiet`).
*Then, the page builder's steps and the output routine* (LOG "the
page builder's steps and the output routine as calls"):
- **The step.** Each contribution is a `build_page` step, a recorded
  call (`Func::PageStep`, the engine's `Builder::step`, §996–§1008).
  - It reads the page's fields as TeX's step does. The engine's step
    reports each field it read before assigning it (`builder::Access`),
    so a condition's reads come in only on the paths that make them: a
    line in a started page reads the total, the depth and the depth's
    limit; a breakpoint also reads the goal, the insertion penalties
    and the cost to beat, and, when it is a new best, the index its node
    takes.
  - It is named by the fields it reads on every path from its start,
    its node, and, for a kern, what follows it (§1000). The path is
    known from those, `page_contents` and whether the page's last node
    precedes a break (`step_name`), so the name is part of its reads.
  - It reads the list only by its length and whether its last node
    precedes a break, two views of the list with slots of their own.
    `build_page` links the node in (§998, §999) as a store: an append
    to the page's list that reads nothing of it (7.17.3's appends), so
    the steps of a page never read one another's lines and only the
    fire reads the whole list. A step therefore
    reads and writes neither the list nor the contributions: its
    result gives back the nodes that go in front of the contributions.
  - Its writes are every field it assigns (§987's freeze, §996's
    `last_*`, §1001, §1002–§1010), as the engine reports them, whether
    or not the value changed: 7.17.2's second rule, a write of an equal
    value is kept, since replayed into a state where that field
    differs it must set it. It does not read a field only because it
    may assign it.
- **The fire.** The fire (§1012–§1022) reads the whole page. It is
  deferred (§7.16.1, "The deferred fire's form"): the page step that
  completes the page leaves it pending, and it is the first action of
  the next fold step, in that step's own record. With the deferral off,
  the page step that completes the page does it, inside its call.
- **The output routine.** The output routine is the next call
  (`Func::Output`), named by `\box255`, the insertion boxes the fire
  filled and `\outputpenalty`. The user's routine runs from §1025 until
  §1026 ends the call, with the nodes that go in front of the
  contributions as its result. The default routine is §1023's ship-out.
- **Why a step names what it reads.** After an edit that keeps the
  heights, the steps after it on the page hit. Only the edited lines'
  steps, the page's fire and its output routine run again: 3 of 374
  steps on a seven-page test, where naming the whole page left 52.

*Then, the output family's calls* (LOG "ship_out and write_out as
recorded calls"). `ship_out` (§638) is a call named by the box, by
the version it carries when shared or else from its parts.
  - Its writes are the writers' tables and `dead_cycles`.
  - Its effects are the page's bytes.
  - Its scope is the call itself: the scopes of its parts
    (`fix_pdfoutput`, `pdf_ship_out`, §640) are its own, and their
    fields are versioned when it ends (`Tex::scoped_call`).

`write_out` (§1370) is a call named by the stream and the version its
token list carries, whether from `ship_out`'s `out_what` (§1374) or
from `\immediate` (§1375). Its effects are the line's bytes, and its
store is the line (7.17.5). It is a child of the call it runs in: a
page's `ship_out`, an `\immediate\pdfxform`'s scope, a paragraph.

#### 7.17.11 Optimizations on the trace

The trace is an SSA graph (one assignment per version, uses that name
the version they read, a φ at the cycle), so the compiler's
optimizations apply to it. Three groups.

**By construction.** *Value numbering and CSE* are the memo table: two
calls with the same `f`, inputs and reads are one record, and
hash-consed values are numbered values. *Loop-invariant code motion*
across the cycle is a hit: a node whose reads did not change between
trips is not run; across builds it is the incremental idea itself.
*Sparse conditional constant propagation on the φ* is the convergence
test: a φ whose incoming versions are equal is not a φ, and the trips
end when none is left. *Constant propagation across an edit* is
backdating: a re-run that writes the same value propagates nothing.
*Inlining* is the grain rule of 7.17.2: a call not recorded is inlined
into its parent's record, decided by cost and set once.

**Built explicitly, in this order of value.**
- *Slicing, or dead code by demand.* A trip before the last needs only
  the nodes that reach a store (`write_out`); the PDF's bytes (content
  streams, font subsets, deflate) are dead until the fixed point. So
  `ship_out` is two calls, one that expands the whatsits and reaches
  the stores, and one that emits the page and reaches only the PDF;
  a cold build makes bytes once. Evaluation by demand is what makes
  trace-based dead code safe: a node skipped and demanded later is
  computed then, so nothing skipped is lost.
- *The cycle's strongly connected component.* Tarjan's SCC over the
  trace, through the stores, bounds what can iterate: the `\ref`s, the
  table of contents, the numbers of the pages that carry labels.
  Outside the component the graph is a DAG, evaluated once whatever
  the trip count.
- *Escape analysis for scratch.* A field written and read inside one
  call (`cur_val`, the ship's per-page state, `skip_line`) is a local,
  not state; found over the trace, it leaves the state, with no
  versions and no checks. It is the static form of the poison test
  (§7.16.4), which was the dynamic one.
- *Dead stores on streams.* A `\write` to a stream nothing loads
  (`\@writefile{lof}` without `\listoffigures`) is dead when the file
  view is off, and the chain from the whatsit to the store is skipped.
- *Reassociation where the fold is associative.* The link's offsets are
  prefix sums over cached page bytes already, so the concatenation is
  a parallel prefix. Page breaking is not associative, so the page
  builder stays a fold and its parallelism is 7.17.6's speculation.

**Later: trace compilation.** A recorded trace is what a tracing JIT
consumes, straight-line SSA with guards; the verified reads are the
guards and a miss is a side exit. A hot trace, the paragraph being
typed in, can be compiled with expansion resolved under the catcodes
and meanings its guards fix, folded, dead code removed, and emitted as
native code: §7's compiled expansion with the trace as its input. For
a keystroke it is modest once the overhead is gone (a paragraph is
1–3k commands, about a millisecond at cold speed); for a cold build it
is the 2–5× on the 65.8M-command run. It comes after 7.17.10.

**What does not transfer**: anything that needs every path of a static
program. The trace is one execution, so "dead" means "not read in this
run", and each such fact is guarded by the reads, which is why demand
is the frame that makes the rest sound.

#### 7.17.12 TeX's state

The inventory of step 2 (2026-09-28, LOG "§7.17.10 step 2c"): every
field of `Tex` and of the structs it owns, classified, with the family
that converts it. Steps 2a–2b (LOG "step 2", "step 2b") bolted a
recorder onto the mutable engine and used check mode to find what it
missed; this table replaces that worklist. A difference check mode
finds is an error in this table, fixed here and in the code.

**Conventions shared by the families.**

- *A call boundary* is where a recorded call starts or ends. The
  first calls, the paragraph and the tokenizer, start and end at a
  clean point: main control's top (§1030) before `get_x_token`, in
  outer vertical mode with `nest_ptr = 0` and an empty contribution
  list, the current input level a file, the output routine not active
  (`Tex::clean_point`). A field is *scratch* iff it is dead there: every
  path from the boundary writes it before it reads it. A call added at
  another boundary (`macro_call`, `hpack`, a `build_page` step,
  `ship_out`) re-checks the scratch rows against its own boundary and
  moves a row to *value* where it is live; the rule for those calls is
  7.17.2's: what is live at their boundary is their *argument* (the
  current token for `macro_call`, the list for `hpack`) or their
  *result* (`cur_box` for a pack), never a re-classified field, so the
  table's classification at clean points stands and the finer routines
  are children whose boundaries their arguments and results cover.
  *Applied to the packs* (LOG "hpack, vpack and line_break as recorded
  calls"): `hpack`'s argument is its list, its spec, whether it
  migrates adjust material (`adjust` given or not), its packer (§649's,
  pdfTeX's expansion of a §889 line, or §804's for an alignment's
  preamble, which does not read `\overfullrule`: TeX zeroes it around
  that call) and the *kind* of context its
  report names (`pack_begin_line`'s sign: a paragraph, an alignment,
  none); `vpack`'s is its list, its spec, its depth limit and the same
  kind; `line_break`'s is the paragraph's list and whether it precedes
  a display. The lines `pack_begin_line` holds are not in a name: a
  report that prints them reads them as a position (`line:j±d`, 7.17.4),
  so a pack that prints no report does not depend on them. The result
  is a write to a result slot of its own (`track::scalar`:
  `HPACK_RESULT`, `VPACK_RESULT`, `LINE_BREAK_RESULT`), versioned by
  content: the box's parts, for `hpack` the material that migrated into
  the caller's adjust list, and the glue totals §796 and §1201 use; a
  pack also writes `last_badness`. The rows stay scratch at clean
  points, as the table says. The page builder's calls follow the same
  rule: a `build_page` step's result (`PAGE_STEP_RESULT`) is the node
  it gives to the page or the discards and the nodes it puts back in
  front of the contributions, and the output routine's
  (`OUTPUT_RESULT`) is what §1026 puts in front of them.
  Fonts and hyphenation are owned by the tables family.
- *A value's version is computed when the value is made*, at the write,
  from its parts' versions (7.17.1). There are two conventions, one per
  kind of value, and every value row uses one of them:
  1. *Tables* (eqtb in all regions with `xeq_level` and the registers
     above 255, the hash, the pool, the font tables, hyphenation's
     exceptions): a version array beside the entries, set by the
     writing accessor after it has stored the value, from the content
     written (`Tracker::wrote`); an equal write gives the same version
     (backdating). A writer that stores a table wholesale past its
     accessors versions the whole table the same way
     (`Tex::version_tables`: the engine's initial tables and a format's
     load, step 2d); a store that bypasses both is an error, which check
     mode's array test finds.
  2. *Structures* (the nest's lists and boxes, token lists, the save
     stack, the page builder's state, marks, insertions, the PDF and DVI
     writers' tables, the streams, the input state): a persistent or
     `Arc`-shared value carrying its version, computed from its parts
     when it is made (a box when packed, a list's version per node so an
     append shares the prefix, the builder's state per field so
     `\pagegoal` reads one field's version): `partex-ssa`'s `PVec`,
     `PMap`, `PStack`, or a structure of the family's with the same rule.
  Nothing is hashed at a read. (Step 2's lazy content cache,
  `Versions::content`, was the stopgap; step 2d removed it.)
- *An accessor records a read* by telling the tracker the address and
  the version it holds: `Tracker::read_content(cell, …)` for a table
  entry (the version is the array's; the closure computes the content
  only for check mode's test of the array), `Tracker::read(cell)` for a
  family still on build-tagged revisions, and the source's hooks
  (`line_start`, `line_end`, `file`, `line_number`, `name_lookup`,
  `string_search`, `load`, `store_open`; and a hyphenation exception's
  word, `hyph_word_read`). The
  `SsaTracker` turns each into `Runtime::note_read(loc, version)`; a
  write is `Tracker::write(cell)` → `Runtime::note_write(addr)`, after
  the accessor has stored the value and its version. An effect is
  `Runtime::note_effect`, a stream's line `note_store`, a stream made
  `note_open`. An address is a family and an index (`ssa::Slot`), never
  a name. A family adds its `Fam` variant, its `Store::version` arm in
  `ssa::View`, and its accessors; the `Tracker` hook is only how the
  recorder is told.
- *A hit is nothing but stores*: the state's fields take the record's
  output versions (a pointer store per written field, the value shared,
  never copied or thawed); the input position is the call's result
  (`close_source`'s value: the lines each level consumed, the depth,
  the offset); the call's effects stay in its record, where the link
  finds them in program order (streams and outputs, below). The body
  does not run.
- *A writer scope* (the output family, `pdf::val`). Each routine that
  changes the DVI or PDF writers' tables is a call (7.17.2: every
  operation is a call). These routines are `ship_out` (§640, pdfTeX
  §750's `pdf_ship_out`), a `\pdf…` command (pdfTeX §1537–§1599) and
  the job's end (§642, pdfTeX §794). Each declares the fields it reads
  and the fields it writes.
  - Its reads are the fields at its start, each with the version its
    last writer made.
  - Its writes are the fields it made, versioned from their parts
    when the call ends: a call's writes are its outputs.
  - Inside the call, the writer changes its tables in place, and a
    read of a field the call itself wrote is not an input.

  A field that a few commands store whole is read and written at the
  access instead: `\pdfmatch`'s result, the `\pdflast…` values, and
  `\pdfoutput`'s fixing.

  `ship_out`'s scope is the recorded call itself (7.17.10's output
  family's calls). A call begun inside an open scope, such as
  `write_out` inside `ship_out`, reads through the same rule as any
  other call. First, the fields the scope wrote so far are versioned,
  as the scope's writes before the child. The child then starts with no
  scope open, and the scope reopens when the child ends, so its end
  versions its fields again as they are then. Without that rule, a
  child's read of a field its parent had changed would note the version
  from before the parent's change.
- *Streams and outputs* (the output family, `streams`).
  - Every byte the engine makes for an output (the terminal, the log,
    each `\write` file, the DVI and PDF files) is an effect,
    `Tracker::output(what, bytes)`, never read, where `what` names the
    output (`track::Output`: the log, the terminal, `\write` stream
    `n`, the DVI file's bytes, the page sink's four hand-overs of the
    writer, a page queued, a page written at once and the finish, and
    the PDF file's bytes). The recorder gathers one run per output and
    hands the runs to the innermost call at every call boundary (a
    call's start and end, a paragraph's, the tokenizer's), so each
    call's effects are exactly the bytes its own body made, in order.
    Each record keeps its runs, and every output file is linked from
    the records' runs in program order (7.17.2, 7.17.11's prefix
    sums), so a record a rebuild does not visit keeps its bytes in the
    file and is never replayed (*corrected*, the user, 2026-09-28
    21:00: the merge of 20:30 made a hit emit its runs again into the
    files, which needs every record visited). Check mode compares a
    hit's and its body's bytes output by output. This is the one
    capture point: the log and terminal capture that came with applied
    hits and this one were joined at that merge.
  - `\write` stream `n` is `Out(n)`: which file it stores to (the
    name `\openout` gave, §1374, or none), read by `write_out` (§1370),
    `\closeout`, `\showstream` and the job's end (§1378), written by
    `\openout` and `\closeout`. The file's lines are stores (7.17.5):
    the open makes one to the name's `Load` address
    (`Tracker::store_open`), each line written ends in a store there
    (`Tracker::store_line`), and a load of that name in the same build
    is never a hit. No writer reads the lines, so a line appended late
    makes no later writer miss. `Out(LOG)` is whether the log is open
    (§534), read by `\write` to the log and the job's end.
  - `\openin` stream `n` (§480) is `Read(n)`: its contents' version,
    made at the load (§1275: a load of the name, `Tracker::load`), and
    the position (§483), made again at every line `\read` takes and at
    the close. `\ifeof` reads the stream's `read_open` slot (§501).
  - pdfTeX's generator (pdfTeX §110) is `Random`, versioned by its
    state, made at each draw (pdfTeX §126, §127), at
    `\pdfsetrandomseed` (pdfTeX §1581) and at the job's start.
  - The host's answer to a query that is not a load (the clock for
    `\pdfelapsedtime`, the creation date, `\pdffilemoddate`, a line
    from the terminal, §71) is a read of `Clock` whose version is the
    answer. A probe never finds it equal, so a call that asked is never
    a hit; a rebuild asks each step's queries again (7.17.3, "A query
    is asked again") and runs the steps whose answer changed. The job's start (§241) is such an answer
    where it is printed (the log's banner, §536, and `\dump`'s, §1328),
    and so is the timer's start (`epoch`, at the job's start and at
    `\pdfresettimer`).
- *Check mode* (`PARTEX_SSA_CHECK=1`) is a test: at each hit it runs the
  body as well and compares, field by field over this table's value
  rows, the state the hit's stores leave with the state the body
  leaves, and the effects (every `Out` item of the call and its
  children, in order, by version). With the table complete it is silent; a
  difference names a row whose class or conversion is wrong.

**Classes.** *value*: state a later call can read; *scratch*: dead at
every call boundary (the clause says why); *effect*: output already
made (bytes to the host, the trace), re-emitted by a hit, never read;
*configuration*: fixed for the session (the parameters, the host, the
format's constants), identical in every build it is compared across.
**Families**: *tables* (eqtb, registers, the hash, the pool; also the
fonts and hyphenation, which no family was named for), *groups* (the
save stack), *tokens* (token lists, the input state, the source),
*lists* (the nest, boxes, pack outputs), *page* (the page builder,
marks, insertions), *output* (the DVI and PDF writers' tables,
streams, `ship_out`, `write_out`, the φ). *Converted*: *yes* (a value
by 2's rule), *partly* (versioned by content, but lazily at a read, or
read without its value being a value), *no*.

| field(s) of `Tex` | class | value type → address family | family | converted |
|---|---|---|---|---|
| `host`, `tracker` | configuration | (the caller's) | — | — |
| `params` | configuration | | — | — |
| `xord`, `xchr`, `xprn` | configuration | set by the format and web2c's `-translate-file`, never by a document | — | — |
| `str_pool`, `str_start`, `pool_ptr`, `str_ptr` | value | persistent sequence of strings, each versioned by its bytes; a string number is observable (`\fontname`, `\jobname` reuse), so the pool's additions are writes → `Str` (by number) | tables | yes (step 2e: string `n` versioned by its bytes when made, `make_string` and web2c's raw `str_ptr` moves in `end_name` (§517); `str_ptr` a scalar slot whose version is its value, read and written by `make_string` and `flush_string`; the search a read by content, step 2d; `pool_ptr` above `str_start[str_ptr]` is the string being built, scratch at a boundary) |
| `init_pool_ptr`, `init_str_ptr` | configuration | the format's pool end | — | — |
| `str_index` | value | an index of `str_pool` by contents (`search_string`), derived: versioned with the pool, never read apart. *Corrected* (step 2d): what a caller observes is the search's result, so a search is a read by content, `Search(name)` = the string found, verified by searching again (`Tex::peek_search`), like a name's lookup | tables | yes (the search) |
| `skip`, `skip_tracked`, `class_hash`, `classes_read`, `classes_written` | scratch | a cache of skipped conditional text, derived from eqtb: a hit may leave it cold; it changes only speed | — | — |
| `log_file` (`id`, `buf`) | effect | the log's bytes; `id` (open or not) is a value of *output* → `Out` | output | yes. Built (`streams`): `Out(LOG)`, versioned by whether the log is open, written at the open (§534) and the close (§1333); the bytes are `log` effects |
| `term_buf` | effect | terminal bytes (`term` effects) | output | — |
| `write_file` (16 streams) | value | the stream's lines (7.17.5): a persistent sequence per stream, its bytes an effect → `Out(n)` | output | yes. Built (`streams`): `Out(n)` is the name the stream stores to, versioned by the name; the lines are stores to the name's `Load` address, one per line (`store_open`, `store_line`), never read by a writer; the bytes are `write`*n* effects. The open file and its buffer are the host's |
| `selector` | value | a scalar (§54: where printing goes; `\openout`, the log's opening and `\batchmode` set it) → scalar slot | output | yes (a scalar slot, its version its value; `\dump`'s banner, §1328, sets it through the accessor too) |
| `dig`, `tally`, `trick_buf`, `trick_count`, `first_count` | scratch | printing's work areas (§54, §316): filled before each use, read only after a reset in the same print | — | — |
| `term_offset`, `file_offset` | value | scalars: the column on the terminal and in the log (§54), which decides where the next print breaks its line → scalar slots | output | yes (scalar slots, read before each print moves them and by `print_nl`'s test, §62, written after) |
| `arith_error`, `save_arith_error`, `remainder` | scratch | set by each arithmetic routine before its caller tests it (§104) | — | — |
| `line` | value | the top level's line number: a derived position (7.17.4), read as `line:j±d` | tokens | partly |
| `buffer`, `first`, `last` | value | the lines in the buffer, one per open file level: each level's line is the source's line (by its bytes) and the level's `start`/`limit`; above `first` dead | tokens | partly (read by bytes at a call's end) |
| `max_buf_stack`, `max_nest_stack`, `max_save_stack`, `max_in_stack`, `max_param_stack` | scratch | statistics, not emulated (AGENTS.md) | — | — |
| `interaction` | value | scalar (`\batchmode` …, §1264) | output | yes (a scalar slot; a format's load stores it, then versions it with the tables) |
| `deletions_allowed`, `set_box_allowed` | scratch | true at every boundary: set false only around a scan that restores them (§76, §1241) | — | — |
| `history`, `error_count` | value | scalars: the run's worst message and the errors since the last paragraph end (§76), printed, and the stop at 100 errors reads the count | output | yes (scalar slots) |
| `help_line`, `help_ptr` | scratch | set before each `error` that prints them (§79) | — | — |
| `use_err_help` | scratch | set by `\errmessage` just before its `error` (§1283) and reset by it | — | — |
| `special_printing`, `message_printing`, `no_convert`, `active_noconvert`, `cs_converting` | configuration | encTeX flags at their initial values (encTeX unsupported) | — | — |
| `font_in_short_display` | scratch | set before each `short_display` (§174) | — | — |
| `depth_threshold`, `breadth_max` | scratch | set from `\showboxdepth`/`\showboxbreadth` before each `show_box` (§236) | — | — |
| `nest` | value | empty at a boundary (`nest_ptr = 0`); as a value, a persistent stack of list states → `Nest` | lists | no |
| `cur_list` (`mode`, `list`, `mlist`, `pg`, `ml`, `prev_depth`, `space_factor`, `clang`, `incompleat`, `middle`, `lr_save`, `lr_box`) | value | a list state: the list a persistent sequence of nodes (an append shares the prefix), each node an immutable value versioned when made; the scalars fields → `List` (field) | lists | no (its boxes carry versions, step 2f) |
| `shown_mode` | value | scalar (§213: the mode `\tracingcommands` last showed) | lists | yes (a scalar slot behind `shown_mode`/`set_shown_mode`, 42b576a) |
| `eqtb` | value | table, an entry versioned at its write by its content (the word and the objects it names) → `Eqtb(p)`. *Corrected* (step 2d): a format's load (§1299–§1329) stores eqtb, the hash and `xeq_level` wholesale past the accessors, so it is a writer of the tables and versions them (`Tex::version_tables`); check mode found it (`\newlinechar`, INT_BASE+49, read with the version of a write made before the load) | tables | yes (step 2d) |
| `objs` (`glue`, `boxes`, `shapes`, `lists`, the free lists, `interned`, the pools) | value | the objects eqtb words name: each an immutable shared value (`Arc<BoxNode>` already; glue and shapes by id), part of the naming entry's value, never an address of its own; the id allocator is scratch once entries hold the values | tables | partly (step 2f: a box is a shared value carrying its version, `BoxNode::ver`, made when it becomes one, `BoxNode::share`, from its parts with its boxes by their own versions, and made again after a change in place, `\wd`, a shift, `\vcenter`; a box register's eqtb content combines it; glue and shapes still hashed at the write) |
| `xeq_level` | value | table beside eqtb (§253), versioned with the entry → `Eqtb(p)` (the save stack reads it at each local assignment) | tables | yes (its write makes the entry's version) |
| `eqtb_top` | configuration | eqtb's size | — | — |
| `old_setting` | scratch | saves `selector` around a print and restores it (§245) | — | — |
| `sys_time`, `sys_day`, `sys_month`, `sys_year` | value | the job's start (§241), read from the host where printed: a read of the host's clock (`SOURCE_DATE_EPOCH`), fixed per build → `Clock` | tables | yes. Built: where the date is printed, the log's banner (§536) and `\dump`'s (§1328), the host's answer is a `Clock` read (`Tex::clock_read`, versioned by the answer), so a call that prints it runs again in every build |
| `hash` (`text` = rh, `next` = lh) | value | table: a slot's text is the name (by its characters), its link the layout → `Hash(p)`, `HashNext(p)`; a lookup reads `Name(id)` | tables | yes (step 2d) |
| `hash_used`, `hash_top`, `hash_high` | value | the allocator of new slots (§259), which decides where the next name goes → `HashAlloc` | tables | yes (step 2e: `hash_used` and `hash_high` scalar slots, read and written by `id_lookup`'s insertion, §260, and web2c's extra area; `hash_top` is the table's size, configuration) |
| `no_new_control_sequence` | scratch | true at every boundary: set false only inside `\csname`, a definition's name scan and `\let`, which restore it (§336, §372) | — | — |
| `cs_count` | scratch | statistics | — | — |
| `cur_val`, `cur_glue`, `glue_origin`, `cur_val_level`, `radix`, `cur_order` | scratch | the scanners' results (§410, §438, §447): each set by the scan that its reader called just before | — | — |
| `split_discards` | value | e-TeX's `\splitdiscards` (§1110): a node list, shared → `Page` (field) | page | yes (LOG "the page builder's state by field": a `NodeList`, `Row::Page(SPLIT_DISCARDS)`, changed through `split_discards_mut` by `\vsplit` and taken by `\splitdiscards`) |
| `par_loc`, `par_token`, `write_loc` | configuration | set by `primitive` (§334, §1344) at the format | — | — |
| `mltex_enabled_p`, `enctex_enabled_p` | configuration | set by the format | — | — |
| `interrupt`, `ok_to_interrupt` | configuration | no interrupts in partex (0, true) | — | — |
| `halting_on_error` | configuration | the `-halt-on-error` option | — | — |
| `edit_request` | effect | the `E` response's request, delivered at the end | output | — |
| `save_stack`, `save_eqtb`, `save_ptr` | value | a persistent stack of save entries (§268), each the old eqtb entry's value (not its word) → `Save` | groups | no |
| `cur_level`, `cur_group`, `cur_boundary` | value | scalars with the save stack (§271) → `Save` (field) | groups | no |
| `mag_set` | value | scalar (§286: the magnification first used) | tables | yes (a scalar slot) |
| `cur_cmd`, `cur_chr`, `cur_cs`, `cur_tok` | scratch | overwritten by the `get_x_token` that follows every boundary (§1030) | — | — |
| `input_stack`, `input_ptr`, `cur_input` | value | at a boundary the current level is a file level; the levels below (files, and token lists: LaTeX's `\include` reads its file from inside a macro, whose rest stays on the stack) are the input state: a persistent stack of level records, a file level a position in the source, a token-list level a shared list and a position → the call's result (7.17.4) | tokens | partly (read relative to the start; not advanced by a hit) |
| `in_open`, `open_parens`, `input_file`, `line_stack` | value | the open files: each an immutable file value (`AlphaFile::data`, an `Arc<[u8]>` loaded by name → `Load`), a position (`pos`, `lines`) and its name; `open_parens` counts the `(` printed; `AlphaFile`'s `line_from`, `line_open`, `line_generation` are the recorder's bookkeeping (scratch) | tokens | partly |
| `grp_stack`, `if_stack`, `eof_seen` | value | e-TeX's per-file group and condition depths, `\everyeof` inserted (§1394) | tokens | no |
| `pseudo_files` | value | `\scantokens`' pseudo files (§1485): persistent sequences of lines | tokens | no |
| `source_filename_stack`, `full_source_filename_stack` | value | the file names by level (strings) | tokens | no |
| `scanner_status`, `warning_index` | scratch | `normal` at every boundary: set around a scan (§305) and reset by it | — | — |
| `def_ref` | scratch | the body a definition is building (§473), owned by eqtb once made | — | — |
| `param_stack`, `param_ptr` | value | the parameters of the macro levels below the current file level (§308; empty unless a macro's rest waits below the file, as in `\include`): shared token lists → with the input stack | tokens | no |
| `align_state` | value | the brace count (§309), 1000000 at a boundary outside alignments but read by `get_next` → scalar slot | tokens | no |
| `base_ptr` | scratch | set by `show_context` before use (§311) | — | — |
| `expand_depth_count` | scratch | 0 at every boundary: incremented and decremented around `expand` | — | — |
| `dead_cycles` | value | scalar (§592: `\deadcycles`) → `Page` (field) | page | yes (a scalar slot behind `dead_cycles`/`set_dead_cycles`, 42b576a: a scalar's version is its value, so its address family changes nothing) |
| `out_rtl`, `lr_problems` | scratch | set at each `ship_out` before use (e-TeX §1409) | — | — |
| `last_badness` | value | scalar (`\badness`, §646) | lists | yes (a scalar slot behind `last_badness`/`set_last_badness`: every pack writes it, as §649 and §668 begin, and `\badness` reads it) |
| `output_active` | value | false at every boundary (`clean_point` excludes it), a value of the page builder inside a `build_page`/output call | page | yes (a scalar slot behind `output_active`/`set_output_active`, 42b576a) |
| `read_file`, `read_open` | value | `\openin` streams: a file value (a load, or the φ) and a position, per stream → `Read(n)` | output | yes. Built (`streams`): `Read(n)` is the contents' version, made at the load (§1275), with the position (`pos` and the line counters), made again at each line read (§485, §486) and at the close; `read_open` is a scalar slot per stream, read by `\ifeof` (§501) |
| `cur_name`, `cur_area`, `cur_ext`, `area_delimiter`, `ext_delimiter`, `quoted_filename`, `stop_at_space`, `name_of_file` | scratch | the file-name scanner's results (§512–§519): set by `scan_file_name`/`pack_file_name` before each use | — | — |
| `format_ident` | configuration | the format's banner | — | — |
| `force_eof` | scratch | false at every boundary: `\endinput` sets it and the level's end resets it (§362) | — | — |
| `long_state` | scratch | set by `macro_call` before use (§389) | — | — |
| `cond_stack`, `if_limit`, `cur_if`, `if_line` | value | the open conditionals (§489): a persistent stack; `if_line` a position (read as `line:`) → `Cond` | tokens | partly (step 2f: one slot versioned at the write from its whole contents, at push, pop, `change_if_limit` and `common_ending`, §495–§498, read by every reader; not yet the persistent stack; `if_line` stores the number its `line:` read gave at the push) |
| `skip_line` | scratch | *Corrected* (step 2f, it was in the row above): `pass_text` sets it before its only reader, the runaway error inside the same skip (§494, §336) | — | — |
| `cur_mark` | value | marks by class (§382, e-TeX `\marks`): token lists, shared → `Marks(class)` | page | partly (step 2f: one slot versioned at the write from its whole contents, at `vsplit` and `fire_up`, §977, §1012, read by `\topmark` and friends; not yet shared token lists carrying their version) |
| `job_name`, `log_name`, `output_file_name`, `log_opened` | value | strings and a flag, set once (§527, §534) | output | yes (scalar slots: a name is a string number, its bytes the pool's `Str` value) |
| `name_in_progress` | scratch | false at every boundary: set around `scan_file_name` (§526) | — | — |
| `fmem_ptr`, `font_ptr`, `fonts` (`FontArrays`: metrics, `codes`, `expand`, `order`, `rank`, `expanded`, `ident`, `retagged`, `order_hash`, `order_read`) | value | a font is an immutable shared value (its metrics by identity, `Host::font_slot`), its settable fields (`\fontdimen`, `\hyphenchar`, `\skewchar`, tags) values of their own; the table of loaded fonts a persistent sequence → `Font(f)`, `FontTable`; `order_read` is scratch (a machine's flag) | tables | yes (Built, LOG "§7.17.10: the fonts and hyphenation as values": each font's fields are slots `Font(16f+k)`, versioned by content in the tables' array at the write (`Tex::font_wrote`), a load versioning all of them (`font_made`), the engine as made and a format's load all fonts (`version_fonts`). The metrics are the font's identity (`FontData::idv`: the TFM's content hash, an expanded font's base and ratio, a copy's source), so a read hashes nothing; a font reshaped in place (letterspaced, `\tagcode`, `\pdfnoligatures`: `remade`) is versioned by its metrics' contents. `\fontdimen` is its own field (the params), as are `\hyphenchar`, `\skewchar`, the expansion parameters with the font's chain, the space glue (§1042) and each of the eight code arrays (`\lpcode` … `\knaccode`, a sum of per-character terms kept by `set_code`, so a write costs one term). The table of loaded fonts is `FontOrder`, a `PVec` of (slot, identity), versioned with `font_ptr` and `fmem_ptr`. Every accessor reads the field it uses (`Tex::font_read`; `TrackedFonts` for the packers and alignments). `order_hash` and `retagged` stay machine mode's) |
| `dvi` (`writer`, `file`, `too_long`, the page buffers, fonts listed) | value / effect | the DVI file's bytes are effects; the writer's tables (fonts defined, `total_pages`, `max_v`, `max_h`, `max_push`, `dead_cycles` is above) are values `ship_out` writes → `Dvi` | output | yes. Built (`pdf::val`): four fields, `Row::Dvi`. They are the engine's side (the preamble written, the file, the sink's, the bound, too long), the fonts defined, the totals, and where the file is. The writer is a shared record (`Val<DviWriter>`) whose three parts are versioned when it is shared, at the end of `ship_out`'s scope (§640) or `finish_dvi_file`'s (§642). The bytes flushed (§598) and a page given to the host's sink are effects |
| `write_open` | value | per stream, open or not → `Out(n)` | output | yes (a scalar slot per stream, beside `Out(n)`'s name) |
| `pack_begin_line` | scratch | 0 at every boundary: `line_break` and `fin_align` set it around their packing and reset it to 0 (§815, §800). At a pack's boundary its sign is part of the pack's name and its lines are reads where a report prints them (the packs' convention above) | — | — |
| `adjust` | scratch | `None` at every boundary: an `\hbox`'s adjust material migrates when it is packed (§655). At `hpack`'s boundary whether it is given is part of the name and what migrated is part of the result | — | — |
| `hyph` (`patterns`, `hyph_word`, `hyph_link`, `hyph_count`, `hyph_next`, `exceptions`; INITEX's trie builder `trie_*`, `trie_op_*`, `max_op_used`, live from `\patterns` to `init_trie`) | value | the patterns (shared, fixed once packed) and exceptions (a persistent map by word) → `Hyph` | tables | yes (Built, same LOG entry: three kinds of `Hyph` slot. The patterns are one slot, versioned at each `\patterns` part it stores (a pattern, the hyphenation codes `\savinghyphcodes` stores, `init_trie`'s pack) by folding that part into a running version, and by the packed trie's contents after a format's load. The exceptions are `Exceptions`, a `PMap` from the word with its language (`exception_key`) to its positions, whose version is the map's; that slot, with `hyph_count` and `hyph_next`, is the table as a whole, which an insertion (§940), the format's dump and `\tracingstats` read. A word is its own address, as a name is (`Fam::HyphWord`, interned in the record, `Tracker::hyph_word_read`/`hyph_word_wrote`), versioned by what the map holds for it: its positions' version, made when they are entered, or none. So §930's lookup reads only its word's entry, and `\hyphenation` of another word leaves the paragraph a hit. §939's hash (`hyph_word`, `hyph_link`) addresses nothing) |
| `hyph` (`hc`, `hyf`, `cur_lang`) | scratch | hyphenation's work areas, set by `line_break` before each word (§891, §923) | — | — |
| `page` (`Builder`: `contents`, `list`, `so_far`, `max_depth`, `least_cost`, `best_break`, `best_size`, `ins`, `insert_penalties`, `last`, `discards`) | value | the page so far (§980): its list a persistent sequence, the rest a small record → `Page` (field per part: a `build_page` step reads heights, not the list) | page | yes (LOG "the page builder's state by field": 21 fields, `Row::Page(f)` in a family of its own (`Fam::Page`, `track::page`, numbered by the engine's `builder::field`): `page_contents`, the list (a `NodeList`), each of `page_so_far`'s eight, `page_max_depth`, `least_page_cost`, `best_page_break`, `best_size`, the insertion records, `insert_penalties`, each of `last_glue`, `last_penalty`, `last_kern`, `last_node_type`, and `page_disc`. A field's version is the value's (the lists carry theirs, a scalar its value, the insertion records by their contents, a few per page). The readers and writers outside the builder use one field each (`\pagegoal` reads `SO_FAR+0`, `\lastskip` `LAST_GLUE`, `\insertpenalties` its own). A `build_page` step (§994, a call, LOG "the page builder's steps and the output routine as calls") reads the fields its node's path reads and writes the fields it assigns, as the engine's step reports them (`builder::Access`), and sees the list only through two views with slots of their own, `LIST_LEN` (the index its node takes) and `LIST_TAIL` (whether the last node precedes a break), written whenever the list is; `build_page` links the node in. The fire (§1012) reads and writes every field (`page_all_mut`)) |
| `align` (`cur`, `stack`, columns, tabskips) | value | empty at every boundary (no alignment spans a clean point in outer vertical mode, §768); a value inside alignment calls | lists | yes (LOG "the alignment state as values, and every pack a call": seven fields, `Row::Align` in the lists family: `cur_align`, `cur_span`, `cur_loop`, the row's adjust list, the columns, the tabskips and the stack. The columns are a `PVec` of alignrecords, each carrying a version made when it is made or changed (its templates by theirs, `extra_info` and the widths); the row's adjust list is a `NodeList`; the tabskips an append-only list with a running polynomial; the stack holds each level with the stack's version up to it. Every access is an accessor (`Tex::column`, `edit_column`, `push_tabskip`, …): a read notes the field's version, a change reads and writes it, and push and pop of the stack read and write every field) |
| `etex_mode` | configuration | set by the format | — | — |
| `epoch` | value | `\pdfelapsedtime`'s start, read from the host's clock → `Clock` | tables | yes. Built: the host's clock is a `Clock` read where it is asked, at the job's start, at `\pdfresettimer` (pdfTeX §1582) and at `\pdfelapsedtime`; the start it answered is kept in two scalar slots, which `\pdfelapsedtime` reads |
| `is_in_csname` | scratch | false at every boundary: set inside `\csname` (pdfTeX) | — | — |
| `xregs` | value | table of e-TeX registers above 255, versioned at write like eqtb → `Eqtb(p)` (p ≥ `EXT_BASE`, an array of its own by the offset from `EXT_BASE`; the defaults versioned by `Tex::version_tables`) | tables | yes (step 2d) |
| `max_reg_num`, `max_reg_help_line` | configuration | set by the mode (e-TeX or not) | — | — |
| `toks` (`TokStore`) | value | the token lists eqtb words, marks and the input stack name: each an immutable shared value versioned by its tokens when made, part of its owner's value | tokens | yes for the store's lists (step 2e: each carries a polynomial of its tokens mod 2⁶¹−1 kept at every write, an append one step, an insertion or an edit in place made again at the write, a format's lists in bulk; an eqtb entry combines it); no for the `Tokens` (`Arc<[i32]>`) of mark and whatsit nodes, which carry no version |
| `arg_list`, `preamble_list` | scratch | `NULL` at every boundary: set while an argument or a preamble is scanned (§392, §777) | — | — |
| `prims` | configuration | the primitive table, fixed by the format | — | — |
| `random` | value | pdfTeX's generator state (§110) → `Random` | tables | yes. Built (`streams`): `Random`, versioned by the state, made at each draw, at `\pdfsetrandomseed` and at the job's start; `\pdfrandomseed` reads it |
| `cur_box` | scratch | `None` at every boundary: the box being built by `\setbox`/`box_end` (§1075). A pack's box is its call's result (`HPACK_RESULT`, `VPACK_RESULT`), never this field | — | — |
| `after_token` | value | `\afterassignment`'s token (§1266), live across a clean point in principle → scalar slot | tokens | no |
| `memo` | scratch | the old memo's (`PARTEX_MEMO`) records: a cache, off in SSA mode | — | — |
| `cs_cache` | scratch | a cache of `id_lookup`, derived from the hash | — | — |
| `hash_memo`, `line_log`, `log_lines`, `glyphs_used`, `glyphs_read`, `seals`, `seal_lines`, `seal_at`, `seal_log`, `canon_strings`, `cs_by_name`, `name_cells`, `name_log`, `font_cells`, `font_log`, `probe_names` | scratch | §7.16's machine's logs and switches, off in SSA mode | — | — |
| `record_objstms`, `objstms_written` | effect | object streams written, for the old link | output | — |
| `stop_before_ship`, `ship_stop`, `par_start`, `checkpoint_every`, `stop_at`, `stop_at_candidate`, `at_checkpoint`, `checkpoint_at`, `block_entered`, `dense`, `dense_at`, `dense_last` | scratch | the driver's stopping machinery: where the engine stops, not TeX's state | — | — |
| `fresh_def` | scratch | set by `\def` around the body's interning | — | — |
| `long_help_seen` | value | whether the long help was printed once (§1283) → scalar slot | output | yes (a scalar slot) |
| `cancel_boundary` | scratch | false at every boundary: `\noboundary` sets it for the next character (§1032) | — | — |
| `commands`, `shipped` | scratch | counters for the driver and the report | — | — |
| `tounicode` | value | `\pdfglyphtounicode`'s table (shared map) → `Pdf` (field) | output | yes. Built: a `VMap` (a `PMap` whose version is the sum of its entries' hashes, one made per insertion) in field `TOUNICODE`, written in a scope by `\pdfglyphtounicode` (pdfTeX §1587). The state hash's memo for it is gone |
| `pdf` (`PdfState`: `objs` the object table, `out` the writer, counts `obj_count`, `xform_count`, `ximage_count`, the `last_*` values, `retval`, the `*_toks` for info, catalog, names, trailer, the outlines, `space_font_name`, `font_attr`, `nobuiltin_tounicode`, `stacks` the color stacks, `ship` the page's state, `fontw` fonts and glyphs used) | value / effect | the PDF's bytes are effects; every table is a value `ship_out` and the `\pdf…` commands write: persistent maps and sequences → `Pdf` (field per table: §7.16's `Dests`, `Numbering`, `Glyphs`, `FontOrder`, `Written`); `last_match` too (`\pdflastmatch` reads it any time after `\pdfmatch`) | output | yes. Built (`pdf::val`): 36 fields, `Row::Pdf`, each a value carrying its version. The object table (`Numbering`), the destination names (`Dests`) and the fonts with their glyphs (`Glyphs`) are `VTab`s: shared chunks of 64, copied on write, with each changed element versioned once when its scope ends. The lookup trees, `font_attr`, `nobuiltin_tounicode` and the encodings are `VMap`s. `out`, the `*_toks`, the stacks, `fontw`, `last_match` and the shipping state (the order of first use, `FontOrder`, among its parts) are shared records (`Val`). The counts and the `\pdflast…` values are versioned by their values. Fields are written by writer scopes (above); `\pdflastmatch` reads its field at the access. `Written` belongs to the streams (`Out(n)`). The bytes are `pdf` effects |
| `fontmap`, `fonts_mapped` | value | the font map (loaded from map files: a load) and the entries used → `Pdf` (field) | output | yes. Built: `FONTMAP` is versioned by the map's digest (made at each change, mapfile.c's `process_map_item`, in a scope), with the default map still pending; a map file read is `Tracker::load`. `FONTS_MAPPED` is a `VSet` that shipping writes |
| `diag` | effect | structured diagnostics | output | — |
| `effects` | effect | outputs as values, taken by the host | output | — |

What the table asserts is checked: a value row by check mode's
comparison (by row, `Tex::value_rows`, step 2d), a table row's versions
by check mode's array test (every read's version against the slot's
content: a difference is a store that bypassed its writer), a scratch
row whose clause names a reset value by a check that the field holds it
at each boundary (a field that does not is a classification error; not
built yet).

#### 7.17.13 Evaluation: one flat graph, run to convergence

Written 2026-09-29 with the user, after the course's label test (LOG
2026-09-29): a `\label` nothing refers to re-ran 4.82 M commands, in
steps of a million each, and needed a second rebuild for its `.aux`.
Both come from scaffolding the code still has, not from this design:
the step between clean points as the unit of re-entry, and a store read
back by the next rebuild instead of forwarded as 7.17.5 says. This
section states the evaluation the rest of 7.17 implies, at the grain it
implies, so that the code's scaffolding has a stated replacement. The
IR is MLIR-like: nodes, values and typed edges, with the engine as its
first backend (an interpreter of one node), compiled backends later,
and the canonical text form of §9.

**Nodes.** A node is one evaluation: every command of `main_control`
(§1030) and every call it makes (expansion, a macro call, `hpack`,
`line_break`, a page step, `ship_out`, a font written). A node's
operands are versions and its results are values (7.17.1). There are no
regions and no steps: the graph is flat, and its hierarchy is intrinsic,
each node the child of the node that ran it. A hit on a node skips its
subtree without visiting it; the hierarchy is what makes a hit O(1).

- *Re-entry at any command.* TeX is iterative: groups, boxes and the
  output routine go through the save stack and the nest, not the host's
  call stack, so between two commands of `main_control` everything
  about "where the engine is" is state: the input stack, the nest, the
  save, conditional and alignment stacks. With the input stack a value
  (token lists persistent, each level's position a value), every
  command boundary is a place a node begins. Clean points (§7.16.1) go:
  they were the places where that state was trivial while it was not
  yet a value.
- *Identity.* A node is named by its function's version and its
  operands' versions (7.17.2), and placed by a key between its
  neighbours in program order (7.17.3's keys, at the command's grain).
  A node that runs again may make different children; a child whose
  name and key match an old one is that node, reused, not rebuilt.
- *No chains through a cursor.* Per-slot edges alone do not stop the
  change where it should: state used as a cursor makes each command
  depend on the one before it. So a cursor is never read as a value:
  - the current list is appended to, as the page's list is (7.17.3
    item 5), and a command that appends does not read the tail;
  - the input position is read relative to the node, as source lines
    are (7.17.4), so an edit that makes one command take more input
    does not shift the reads of every later one;
  - allocators (the string pool's end, the hash's, glue lineages) name
    what they allocate by identity, not by count.

**Functions are values.** A control sequence's meaning is a value in
its slot: a macro's parameter text and body (an immutable token list),
or a primitive, whose version is fixed. A call reads the slot (which
function is called now) and runs that function on its arguments, so a
redefinition is a new definition of the slot and nothing else:
- uses before it read the old version and uses after it the new one (the
  reaching definition); an edit of a `\def` re-runs the call sites it
  reaches, up to the next definition;
- a local definition that `unsave` restores gives the old version back,
  and the change stops there;
- memo keys are by content, so two names with one body share results,
  and a macro defined back to an earlier body finds its old ones;
- meanings read as data (`\ifx`, `\futurelet`, `\meaning`,
  `\expandafter`) are reads of the same slot; `\csname` reads the
  name's hash entry, then the slot; `\edef`'s defining node reads what
  it expanded, and a body that comes out equal stops the change;
- accumulated macros (`\g@addto@macro`, hooks) are appends, whose one
  reader is the hook's use;
- a compiled backend specializes a function on its version and is
  invalidated when the slot's version changes, as a JIT deoptimizes.

A call depends on the whole body's version. The value format keeps a
body's spans addressable, so that a call can later depend on the spans
it consumed (a change to a branch it did not take then does not re-run
it); that is added when a measurement shows large macros re-running for
branches they did not take (agreed with the user, 2026-09-29).

**Edges.**
- *Data edges*: a value to each read of it (the readers of 7.17.3).
- *φ edges*: where values from two places merge. A load of a stream the
  job stores is a φ of the stores that reach it (7.17.5): a `\write`
  that changes the `.aux` makes its loads' nodes dirty in the same
  evaluation. There is no second rebuild; a "pass" is not a concept.
- *Ordering edges*: effects carry no value but must come out in program
  order (log lines, the terminal, PDF objects, `\message`). They order
  commits, never evaluation.
- *Sources*: inputs the host answers (file contents and dates, the
  clock, shell escape) are asked again at each evaluation (7.17.3, "A
  query is asked again"); a changed answer is a changed value like any
  other. The random generator is state with a seed, not a source.

**Memo.** Each node keeps its last *k* (operand versions → results)
pairs. An edit and its revert, and a value that alternates between two
states, find their earlier results without running. *k* is small and
fixed at first (2 to 4); a heuristic may grow it for nodes that
alternate.

**Evaluation.** A worklist of ready nodes, run in priority order until
it is empty:
1. a changed value makes the nodes that read it ready;
2. a ready node whose operands match a memo entry is a hit: its results
   are stored, its effects taken again, its subtree not visited;
3. otherwise it runs, and each result compared with the old one wakes
   that result's readers only if its version changed.

- *Priority.* Program order by default: a node runs after the nodes
  whose results it reads have settled, so in the acyclic part each node
  runs once per change. Across a φ, its readers enter the list again at
  their own key.
- *Convergence.* The evaluation ends when the list is empty. A node
  whose operands repeat a memo entry with a different result is
  oscillating; after a bound (latexmk's five trips, per node) the
  evaluation stops, keeps the last results and reports the node.
- *The frame.* On a keystroke, the priority is the viewer's: the nodes
  whose results reach the page on screen come first, that page is
  converged and painted, and the rest (later pages, φ's through stores,
  fonts, the cross-reference) converges in the background. A new
  keystroke cancels it and starts again from the new change. The target
  is 16.7 ms per edit, the page on screen painted (60 frames a second,
  the user, 2026-09-29), even when a paragraph pushes later pages. A
  measurement of a rebuild reports its time to that page and its time
  to convergence.
- *The viewer.* The page on screen is painted from its page node's
  results, not from the PDF file. The whole file's link and write
  follow at their own pace (the course's link today: 9.4 ms for 3,427
  chunks, most of a frame, for a one-page change).
- *Threads.* Many workers take ready nodes from different parts of the
  graph. Their reads are speculated where the operands have not
  settled, and validated when they do (7.17.6); effects commit in
  program order through the ordering edges.

**The oracle.** The result of an evaluation is the fixed point. Its
check is plain partex (or pdfTeX) run until its stores stop changing,
latexmk's rule, compared as §7.15 compares. A switch keeps one
evaluation per trip, the stores read as the last trip left them, so a
stage can still be compared with one plain run.

**Where the code stands** (2026-09-29, `c22f8ff`):

| this section | the code |
|---|---|
| a node per command and per call | records per call (`unsave`, `hpack`, `vpack`, `line_break`, page steps, `output`, `ship_out`, `write_out`, the fonts) inside steps between clean points |
| hits applied | `unsave` without inserted tokens, and the fonts; the others' results are not values yet |
| re-entry at any command | re-entry at a step's start only |
| data edges | the fold's definitions and readers, by step |
| φ through stores | a store is read by the next rebuild |
| memo of *k* entries | one record per name |
| worklist by priority | the dirty steps, in key order |
| threads | none (`Executor` exists) |

**The work, in order.** Each item's form is written here before its
code, and each reports the course's word edit and label test (7.17's
invariant rule):
1. The input stack as a value: token lists persistent, each level's
   position a value, read relative to the node. Its form (written
   2026-09-29 before the code):
   - TeX's buffer (§30) stays, with its indices: it is the stack of
     the open file levels' lines, each at its `start..limit`. Every
     write to it lands in the top file level's line or above `first`:
     the line read (§362, §538 and e-TeX's pseudo lines), `^^`
     reduction (§355), `\endlinechar` put at the limit, `\pausing`'s
     line moved down (§363), and `\csname`'s scratch (§372), which is
     no level's line. So the bytes below the top file level's `start`
     do not change while that level is open: they are one shared
     `Arc<[u8]>`, made once when that level opens or a level above it
     closes, and the value holds it with a copy of the top level's line
     only.
   - The levels below the top are one shared value: each level holds
     its record and the value of the levels below it, so a push shares
     what is below and a pop returns to it (a value, not TeX's stack in
     `mem`: the engine's stack is still the array). Beside the array,
     the value of each prefix is kept while it is so: a push or pop at
     a level forgets that level's, so the known ones are a prefix, and
     taking the value makes only the levels pushed since the last one.
     The input after a command is then that value, the current record,
     the top file level's line and the scalars: O(1) per command, where
     a step's end copied the buffer below `first` and every record.
   - Re-entry at a node writes the levels' lines back into the buffer
     at their places, costing the lines' bytes, and only at a node that
     is re-entered.
   - The other parts are values already or become them: the token
     lists (`Tokens`), the parameter stack, the files' read positions
     (each an `Arc` of the file's contents and an offset), the pseudo
     files of `\scantokens` (their lines shared, and the index of the
     next), and the stacks of line numbers, groups and conditionals per
     file (a number per open file).
2. The current list as appends.
3. Results as values for `hpack`, `vpack`, `line_break`, the page steps
   and the output routine, so their hits apply.
4. The command as the node, with keys at its grain; steps and clean
   points removed.
5. Stores forwarded to their loads as φ's in the same evaluation.
6. Macro calls as nodes, with functions as values.
7. The memo of *k* entries.
8. The worklist by priority, the viewer's page first, and the page
   painted from its node.
9. Threads.

## 8. Performance and observability

- **Spans** on phases (input, expansion, main control, line break, page
  build, ship-out, backend) behind the `trace` feature, zero-cost when
  off, exported as Chrome/Perfetto JSON.
- **TeX-level profile:** per control sequence (calls, tokens produced,
  sampled time), per file and line, per page. This decides what to
  intrinsify.
- **Native profiling:** `valgrind --tool=callgrind` for exact instruction
  and call counts, `perf record`/`perf stat -r` for time (both on the
  development machine now).
- **Benchmarks** against the reference binary on the same machine;
  results as JSON per commit.
- **Decision records.** Incremental and parallel builds explain
  themselves at region granularity, never per token: each region's
  outcome (replayed, re-executed, suspended, speculation held or failed,
  cancelled) with its cause (the cell, its old and new version, the region
  that wrote it) and its cost, in a ring buffer. `partex why <page | line
  | file>` answers from it ("page 37 re-ran because `\c@figure` changed,
  written by chapter 3 ¶12"); `partex trace` exports it with the spans as
  Chrome/Perfetto timelines per core and round. The per-token path pays
  only the read tracking that exists anyway (§7.1). Today `Report::why`
  and `PARTEX_WATCH_DEBUG` carry a first version: the first changed read,
  where cutoff failed and on which cells.

Baseline (2026-09-25, woven tex.web, 1.6 MB of source): Knuth's `tex`
0.46 s and 2.04G instructions; partex 0.64 s and 3.23G (0.69 s and 3.60G
before a first round of allocation and copy fixes). Loading formats is
faster than tex (0.05 s against 0.09 s). By area, partex against tex:
`get_next` 600M/500M, macro calls 174M/150M, line breaking 556M/274M,
math 206M/78M; freeing node lists is cheaper (78M/288M). The remaining
cold-build gap is mostly in line breaking and math.

LaTeX documents (2026-09-26, `perf stat -r 10`, against pdflatex): a
2-page article with amsmath, TikZ and hyperref took 0.59 s (pdflatex
0.65 s): 0.06 s loading the format, 0.33 s the preamble, 0.2 s the body.
Token reading (`get_token`, `get_next_slow`, `macro_call`,
`get_x_token`) is 38% of it and near its floor (§7.13); the preamble's
share is what persisted checkpoints remove (§7.9). Fixed costs every
process paid, removed:

- `search_string` (web2c's reuse of equal file-name strings) scanned
  every string made since the format, 7% of the instructions: an index
  of the pool by contents, truncated wherever strings are removed.
- TeX Live's `pdftex.map` (5 MB, 42 000 lines) was parsed whole, 8%: a
  map file read into an empty table is kept as its contents and each
  entry parsed when looked up (a document looks up a few dozen), its
  first valid line as registering in order would leave it. Its warnings
  come from the host's cache (`Host::cache_get`, keyed by the file;
  `PARTEX_CACHE=0` turns it off) after one full reading; anything that
  changes the table registers every line first. Caching the parsed table
  instead was slower: 7 MB to decode, more than the text.

Result: 2.92G instructions instead of 3.63G, 0.48 s (usrguide 0.38 s
against pdflatex's 0.55 s, amsldoc 0.62 s against 0.75 s), PDFs
identical. Left: kpathsea's start (`cnf_get`, the ls-R table, 4%) and
Type 1 subsetting (eexec decryption and line reading, 7%).

## 9. Text form

Every IR has one canonical, human-readable text form that round-trips to
the binary form: page IR, node lists, cells, region traces, per-candidate
statistics (§7.3), compiled definitions, token-cache entries, images and
memo databases.

- **Round-trip:** `parse(print(x)) == x`, checked by property tests on
  the corpus.
- **Canonical and diffable:** program order for effects and ops, sorted
  keys for sets, one item per line, no addresses, table positions or
  timestamps. Two runs that behave the same dump the same bytes.
- **Names, not numbers:** control sequences by name (`^^xx` escapes as TeX
  prints them), fonts by identity (`cmr10@10pt`), cells by key
  (`eqtb.count[0]`, `meaning\@tempa`, `nest.space_factor`).
- **Tokens as TeX shows them**, with the catcode written out where TeX's
  notation is ambiguous (`{13 ~}`).
- **Loadable:** a dumped trace or definition can be loaded and run, so
  tests can hand-write IR and bug reports can ship a reduced trace.

```text
region para@main.tex:42 {
  entry  files=[main.tex:42] level=1 mode=vertical
  read   meaning\textbf       #3f9a21c0
  read   eqtb.catcode[64]     =12
  write  eqtb.count[1]        =7
  nodes {
    hbox(6.94+0.0)x345.0 { ... }
  }
  effect log     "Overfull \\hbox (1.2pt too wide) in paragraph at lines 42--43"
  effect write 3 "\\newlabel{sec:intro}{{1}{1}}"
  exit   files=[main.tex:44] level=1 mode=vertical
}
```

**The trace** (§7.17, `partex-ssa`'s `Trace::to_text`/`parse`) is
shaped like LLVM IR: one instruction per line, `%vN = call @f(%va, %vb)
; name=#…`, versions named in order of first appearance with their
content hash after `;` so two dumps of equal values compare, the reads,
writes, effects and stores of a call indented beneath it and a nested
call's body as a block, a load as `%vN = phi [%v<previous build>,
%v<trip k−1>] @stream:aux`, a build as `trip k { … }` with the streams
that changed or converged noted, and, on a rebuild, each call marked
`hit`, `new` or `miss=@<the first read that differed>`, so a keystroke
diffs like a compiler's `-print-after-all`. `partex build --emit-ssa`
writes it once the engine runs on the runtime; it replaces the
`PARTEX_MACHINE_PARTS` dumps and the audit as the way to ask why
something re-ran.

## 10. Roadmap

Done: trip (C1), etrip (C2); the engine on typed nodes with `mem` gone;
page IR and a DVI writer on its own thread; native formats; the LaTeX
format from `latex.ini` and `pdflatex.ini`; the exact PDF backend (C4:
ship-out, Type 1 subsetting, virtual fonts, ToUnicode, JPEG, outlines,
object streams; the PGF manual's 1135 pages match pdfTeX byte for byte);
structured diagnostics; the modern command line; incremental `-watch`
with resume, early cutoff, backdating and read-set cutoff (§7.4);
resident and persisted sessions (§7.9); BibTeX and makeindex in process
(§7.10).

**Order: latency first**, in two tracks, each step gated by exact output
(trip, etrip, e2e, the manuals' PDFs and logs) and by a measurement.

**From 2026-09-28 the order is §7.17.10's work**, one agent at a time,
in thirty-minute tasks; the steps below record what was built before
it and remain the baseline.

**Incremental and parallel** (§7), in order:

1. **The runtime crate and its stub** (§7.0): `partex-incr` generic over
   `Machine`, with a stub language whose tests check propagation, holes
   and rounds against sequential runs. TeX's adapter follows each step.
   Built (§7.0), with warm starts for rounds (a one-line edit: 1 run
   instead of 190, 5× faster). The executor is the runtime's, re-exported
   by core (§7.14). Flat recording: 3.5× a plain run instead of 6.4×.
   TeX's adapter is built (§7.0): `PARTEX_MACHINE=1` builds and
   rebuilds a document through `Build`, byte-identical, sanitizer
   clean, on all of e2e and the course. `Rest` is hashed
   incrementally (memoized Merkle hash), input lines and glyph sets
   are their own cells: machine mode is 1.2–1.3× a plain run cold, and a
   one-word edit on the course rebuilt in 3.3–3.9 s (`-watch`: 26 s).
   On 2026-09-27 the course measured:
   - a one-word edit, 0.37 s
   - a `\label`, 1.3–1.5 s
   - an added footnote, 27 s (3.2 s with fonts at slots)
   - a restart with nothing changed, 1.0 s.
   That is far from the milliseconds the runtime makes on its stub, for
   causes measured in §7.15 and located in the code in §7.16.
   Next: §7.16.8's plan, in order: the baseline harness, the quick wins
   (clean points, the tracker across `set`), cheap cuts (checkpoints
   as shared roots), the page builder as a fold, positions and streams,
   output and the loop, warm parallel units and convergence, PDF
   ship-out onto page IR, cold speculation (timeboxed); then the cold
   overhead down to 1% and machine mode as the default. New features
   (PNG and PDF inclusion, biber) come after (§7.16.10).
2. **Tracking coverage and the sanitizer** (§7.1, §7.12): the tracker
   unification (memo call sites as tracker calls, content values,
   `opaque` instead of whitelists), then cells one at a time — nest and
   lists, page builder, save stack, fonts, PDF state — each measured by
   what it stops abandoning. Nothing after this is sound without it.
   Built: the sanitizer (shadow diff and poisoning, §7.12); the save
   stack's reads; cells for fonts, the font table, `\read` and `\write`
   streams and the random generator. Clean over e2e and the 295-page
   course. Next: the nest, the page builder and the PDF writer as cells
   (today covered only by the full-hash confirmation), the memo onto the
   tracker, and `opaque`.
3. **Source text as values** (§7.2): in progress.
4. **Effects as values and the link step** (§7.6): built for offsets
   (log, terminal, DVI and PDF bytes as effects; xref, `startxref` and
   byte counts at the link; parallel copy). Next: text effects with
   columns, PDF ship-out onto page IR, symbolic object numbers (with
   holes).
5. **Persistent `Send` state** (§5.3, E): built for `Send + Sync`
   and O(changed) checkpoints (`JVec` replaces `CowVec`; relaxed
   atomics for what `&self` updates; commit at snapshots), with the
   machine's host `Send`, the session host `Send + Sync`, and the
   memoized state hash at region boundaries (§7.0). Next: the state
   split into semantic, writer, scratch and cache groups.
6. **Region traces, backedges and program-order propagation** (§7.3,
   §7.4), sequential first: interval-grain traces, the chooser with
   per-candidate statistics, coarse-first validation, replay between dirty
   regions, traces saved with sessions.
7. **Parallel rounds** (§7.7): composed guesses, speculation with guards,
   workers through the `Executor`. Built on TeX behind
   `PARTEX_MACHINE_ROUNDS`, warm, byte-identical, but 3× slower than
   the sequential rebuild while `Rest` is one cell. The hash table, the
   string pool and the fonts have left it (§7.0, "A first use moved");
   next are nested units (§7.15).
8. **Holes** (§7.5): hole tokens and glyph runs, affine counters,
   forcing points, suspension, link-time resolution; the lazy page builder.
9. **Cold-start splitting and the volatility predictor** (§7.7, §7.8).
10. **The content-addressed store** (§7.9, built for machine builds) and
    passes as rounds (§7.10).

Cancellation (§7.11) and decision records (§8) are built alongside,
from step 6.

**Cold sequential speed** (§7.13), measured: pgfmath's parser 38% of the
PGF manual, arguments repeating 90–93%, the per-token path spread thin.
Done: per-definition counters; memoized calls (break-even, off by
default); the token path tuned (`get_next`'s common case inlined, `eqtb`
inlined, a pre-sized `\csname` buffer, eqtb and the hash flat unless
checkpoints are kept, a checked shortcut past `id_lookup`'s chains): the
PGF subset takes 28.4 s against pdfTeX's 26.2 s. Tried and dropped:
caching the list being read (5% slower: the token path alternates between
a body and its parameters). Next: §5.1 tokens and the input stack (before
any further tier: inline caches, compiled bodies and traces all sit on
`cur_input.loc` and `CS_TOKEN_FLAG + p`, and 21-bit characters unblock C5
and C6), then meaning caches, content-keyed intrinsics, compiled
definitions and traces, chosen by the profile.

Then: SyncTeX, XeTeX (C5), LuaTeX (C6). A native JIT (Cranelift) comes
last, possibly never.

## 11. Risks and open questions

- **Floating point.** web2c's `glue_ratio` artefacts are observable in
  logs (`glue set 16341.99998fil`); match web2c's arithmetic, not
  tex.web's idealized reals.
- **Byte-exact PDF** needs zlib's exact deflate output (§2).
- **XeTeX shaping.** Byte-exact XDV needs HarfBuzz's exact shaping
  results; a pure-Rust shaper (rustybuzz) must match the pinned HarfBuzz
  version.
- **LuaTeX's surface** (callbacks, node and token libraries, Lua 5.3
  semantics) is very large. At first every Lua call makes a region
  unreusable.
- **biber.** biblatex's default backend is a large Perl program; a
  byte-identical native port is a separate project (§7.10).
- **Hole and deferral exactness.** A hole or deferred layout value is
  exact only if every observation of it is a forcing point (§7.5).
  Defended by forcing at every read of a deferred quantity, and by
  differential runs of parallel against sequential builds over the whole
  corpus, one hole kind at a time (§7.12).
- **Link-time packing.** Boxes packed again at the link step must
  reproduce web2c's floating-point glue arithmetic exactly.
- **Serial fraction.** The page builder over committed contributions,
  forced output routines and the link step bound parallel speedup
  (Amdahl); measured per document before any claim.
- **Trace memory and tracking overhead.** Traces are deduplicated by
  content; the tracker's cost on cold builds (about 12% for read sets
  today) is measured and kept switchable. In §7.17 the cost is per
  record, bounded by 7.17.2's grain rule, and hashing is off the
  critical path (7.17.6).
- **Cutoff completeness.** Early cutoff is only correct if the state
  hash covers everything that influences the rest of the run (§5.3);
  defended by comparing cutoff runs against full runs on the whole corpus.
- **Hidden state.** Memoization is only as good as the cell list. Closed
  by construction in §7.17.1 (state is values, and a read is a read of
  a value). Before that, two defenses, in sequence: until the state split (§10, step 5), check mode
  (`PARTEX_MEMO=check`, every hit re-run and compared) with differential
  runs (tracked against untracked) on the whole corpus and fuzzing; after
  it, construction: semantic state is private to accessors that call the
  tracker, so an access that is not a cell cannot go unrecorded (§7.1).
- **Read-set sizes** for expl3 documents: is first-read-only enough, or
  are coarser per-module epochs needed? Measure on the latex3 corpus.
- **Host-supplied metadata.** `\pdffilemoddate`-style primitives read file
  metadata and the clock; the host must supply them deterministically.
- **Open: visual mode's scope** (§1). Backend and transcript only, or more?
- **Open: C5/C6 exactness and priority.** XeTeX needs HarfBuzz-exact
  shaping; LuaTeX needs Lua 5.3 and the node handle model (§5.2).
