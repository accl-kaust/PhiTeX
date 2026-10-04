# partex — design

partex is a TeX engine in Rust. It produces exactly the reference
engines' output, and it is built as an incremental, parallel document
compiler rather than as a translation of `tex.web`. The goal it is
measured by: a keystroke to a repainted page in one frame, 16.7 ms, on a
real 300-page LaTeX document, with every output byte-identical to
pdfTeX's.

This file states the design as it is, completely enough to implement
it again from `tex.web`, `etex.ch` and `pdftex.web`. It says nothing
about how the design was reached:
- `LOG.md` is the dated record of what was built, measured and decided;
- `DESIGN_ARCHIVE.md` is the old design document, kept as history only
  (nothing here depends on it);
- `AGENTS.md` holds the working rules.

Code cites this file as `DESIGN N.M`. Older comments cite the old
document's numbers (`DESIGN 7.17.3`); appendix A maps each to its
section here. `§N` always means a section of `tex.web`, and pdfTeX's are
"pdfTeX §N".

Contents:
1. Goals and compatibility
2. Architecture and data model
3. The execution model: dynamic SSA
4. Where the code stands, and the work
5. Performance, observability and the text form
6. Risks and open questions
- A. Old section numbers

---

## 1. Goals and compatibility

### 1.1 Principles

1. **The oracle defines correctness.** The reference is the installed
   TeX Live 2026 binaries: pdfTeX 1.40.29, e-TeX, XeTeX, and LuaTeX
   1.24. For the same inputs, environment and `SOURCE_DATE_EPOCH`,
   partex produces byte-identical DVI and PDF, identical `\write` files
   and exit status, and a transcript with the same content. Upstream
   `.tlg` files and our own judgement are not the oracle.
2. **Port behaviour, not code.** `tex.web`, `etex.ch` and `pdftex.web`
   are specifications. Data structures are designed natively: no
   sentinel tokens in lists, no linked stacks in a word array, no
   mirrored `mem` layouts.
3. **Internal statistics are not output.** Memory, pool, hash, font,
   trie and stack counts, `\dump` statistics and exact capacity limits
   are not emulated. Runaway recursion still stops with an error, just
   not at tex.web's exact point. `xtask/src/mask.rs` masks these
   statistics in comparisons, and nothing is designed around them.
4. **Every acceleration is switchable and exact.** Recording,
   memoization, applied hits, the link cache, speculation and threads
   can each be switched off at run time, and the output is identical
   either way.
5. **Deterministic commit.** Work may run in any order on any thread.
   Effects commit in program order, so scheduling never leaks into
   output.
6. **Native Rust only.** No external commands or C libraries at run
   time: TFM, formats, fonts, deflate and shaping are ours or pure Rust.
   The one planned exception is biber, which runs in a sandbox.
7. **Everything is text-dumpable** (5.3).
8. **Measure first.** No optimization without a profile, and no
   performance claim without a benchmark in `bench/results/`.
9. **Latency is what users feel.** What matters is the time from a
   keystroke to the repainted page. Cold-build speed matters less than
   what a rebuild re-runs.

### 1.2 Output modes

- **exact** (the default, and the gate): every file is byte-identical
  to the oracle's. e2e compares bytes.
- **fast**: the same layout and content, but the PDF is compressed,
  packed and numbered freely. Its check is
  `scripts/pdfcheck same OURS.pdf ORACLE.pdf`, and any difference it
  reports is a bug. `scripts/pdfcheck uncompressed DOC.tex` checks
  everything but compression, byte for byte.

There is one layout engine. A mode may change only the backend and the
transcript, never a line break, a page break or a glyph's position.

### 1.3 Targets

| Stage | Engine | Gate | Status |
|---|---|---|---|
| C1 | TeX82 | trip, byte for byte (statistics masked) | done |
| C2 | e-TeX | etrip against pdfTeX's output | done |
| C3 | pdfTeX, DVI mode | plain and LaTeX corpus | in progress: pdfTeX's primitives |
| C4 | pdfTeX, PDF mode | PDF byte-identical | done: the PGF manual's 1,135 pages and the 295-page course match |
| C5 | XeTeX | XeTeX corpus | not started |
| C6 | LuaTeX | LuaTeX corpus, embedded Lua | not started |

Byte-exact PDF needs a native port of zlib's deflate that gives its
exact bitstream at pdfTeX's settings.

**pdfTeX's bugs.** Identical output means reproducing pdfTeX's bugs.
Each one is marked at its site with a `PDFTEX_BUG(<name>)` comment,
listed in `partex_engine::bugs::PdftexBugs`, and switchable with
`PARTEX_PDFTEX_BUGS=none` or `-<name>,…`. Found so far:
`lig_margin_kern_var`.

### 1.4 Tests

- The corpus is fetched from pinned commits by
  `scripts/fetch-upstream.sh` into `upstream/`, which is not committed.
  `cargo xtask corpus` indexes it.
- The suites:
  - trip and etrip;
  - web2c's engine tests;
  - the LaTeX kernel's and expl3's l3build tests;
  - e2e: `tests/e2e/`, run by `xtask/src/e2e.rs` as whole runs against
    the oracle, including rebuilds after edits;
  - SSA mode's edit sequences, `scripts/ssa-edits`: e2e's incremental
    cases as rebuilds of one SSA process, each stage against plain
    partex on the same sources (4.3 item 8).
- Each test runs under the real engine. Its raw outputs go to
  `refs/<engine>/`, which is gitignored and regenerated with
  `--oracle`, and ours are compared against them.
- The gate is `scripts/sandbox cargo xtask check`: fmt, clippy, the
  wasm build, trip, etrip, e2e and SSA mode's edit sequences
  (`ssa-edits`).

---

## 2. Architecture and data model

### 2.1 Crates

```text
crates/
  partex-engine  typed nodes, fonts (TFM), packaging, line breaking and
                 hyphenation, the page builder, math, alignment, TeXXeT,
                 the page IR, the DVI writer, persistence
  partex-core    input, tokens, expansion, main control, eqtb, the save
                 stack, formats, ship-out, the PDF writer, the SSA
                 recorder and rebuild; no_std + alloc
  partex-ssa     the dynamic SSA runtime: values, versions, records,
                 the fold of definitions and readers, the text form
  partex-kpse    file lookup with kpathsea's semantics
  partex-trace   tracing and profiling facade
  partex-cli     the binary: tex/pdftex personalities, the modern
                 command line, watch sessions, the host
  partex-incr    the old region runtime of machine mode (4.1), to be
                 removed but for its Executor
xtask/           corpus, oracle runs, trip, etrip, e2e, masking, checks
```

### 2.2 Constraints

- Core crates build for `wasm32-unknown-unknown`. `partex-core` touches
  no OS: files, the clock, the terminal and shell escape go through the
  `Host` trait. Output is identical native and in wasm.
- Engine state is accessed only through accessors, which tell the
  tracker what they read and wrote (3.2).
- Parallelism goes only through the `Executor` trait, with results
  merged in program order. On wasm without threads it runs
  sequentially.
- Every IR has a canonical text form that round-trips (5.3).

### 2.3 The pipeline

```text
files ─► input (lines, catcodes) ─► expansion ─► main control ─┬─► lists
                                                               │
      ┌──────────────── calls (packs, line breaking) ◄─────────┘
      ▼
  page builder ─► ship-out ─► page IR / PDF writer ─► effects ─► link ─► files
```

Expansion cannot run ahead of typesetting. It reads layout results all
the time: `\wd` after `\setbox`, `\lastbox`, `\prevgraf`,
`\pagetotal`, marks, and whatever the output routine did. So the front
end is one ordered program, and incremental and parallel work come from
the graph of chapter 3, not from a pipeline split.

### 2.4 Data

- **Tokens.** Today a token is `cmd*256 + chr`, or `CS_TOKEN_FLAG + p`
  for a control sequence at hash slot `p`. Token lists are immutable,
  shared `Arc` values (`Tokens`), each carrying a polynomial hash of
  its tokens kept at every write.
  - Planned: `Token(u32)`, with a control sequence as an interned name,
    and 21-bit characters for XeTeX and LuaTeX.
- **Nodes.**
  - `enum Node` is kept in `Vec`s, at 24 bytes a node.
  - Boxes are `Arc<BoxNode>`, immutable once packed, each carrying a
    content version (`BoxNode::ver`).
  - Characters come in glyph runs of up to 19 same-font characters,
    split canonically.
- **Tables.** eqtb, the hash, `xeq_level` and the registers above 255
  are flat arrays. Beside each is a version array, set by the writing
  accessor from the content written (3.2).
- **Fonts and formats.** TFM files are parsed once into shared fonts,
  with each character's metrics resolved at load. Formats are partex's
  own serialization (`codec.rs`).
- **Effects.** Everything that leaves the engine is an effect: log and
  terminal bytes, `\write` lines, file opens, DVI and PDF bytes with
  their object marks. With effects as values (`PARTEX_EFFECTS=1`, and
  always under SSA), the engine appends effects and the link writes the
  files (3.8).
- **Pages.** Ship-out walks a box once into the page IR
  (`pageir::Page`) or through the PDF writer. Backends see only pages,
  never engine state.

### 2.5 Diagnostics and the command line

- **Errors.** TeX's error report is output and is kept byte for byte.
  Each error also yields a structured `Diagnostic` (`diag.rs`): code,
  message, help, the whole input stack as frames, and suggestions. It
  is rendered rustc-style.
- **Command lines.** Under an engine's name (`pdflatex`, `tex`, …) or
  with `partex --compat=NAME`, partex speaks that engine's web2c
  command line. Bare `partex` is the modern command line:
  - `build` runs to the fixpoint, with BibTeX and makeindex in process;
  - `watch` is the watch loop;
  - `check` makes one pass without writing;
  - `why` says what the last build did;
  - `trace` writes a Perfetto timeline;
  - `clean` removes what the last build wrote.

  It is configured by `partex.toml` and `% !TEX` comments.
- **The terminal** is drawn from events, not from the log: warnings
  and notes come as data from the host, and LaTeX's warnings are
  grouped and deduplicated (`render.rs`, `warnings.rs`, `snippet.rs`).
  - *The live line.* On a terminal a thread of its own draws one line
    about twelve times a second: the pass or phase (loading the saved
    build, linking, writing, saving), the pages shipped, the file and
    line being read, the commands run, the time. Its numbers come from
    the engine's *progress board* (`partex_core::progress`): relaxed
    atomics the engine posts every 1024 commands (the commands, the
    file and line) and at each page, which the thread samples. It is
    observability only: no tracker sees it and the engine never reads
    it, so it changes no output. A build from the start shows a bar and
    the time left from the last such build's totals, kept in the cache.
    A build quicker than 150 ms never shows the line.
  - *The result* is one line: `Finished paper.pdf · 12 pages · 84 KB ·
    2 passes · 1.31 s`, the file a hyperlink (OSC 8) where the terminal
    has them.
  - *`partex watch`* logs a line per rebuild (the time, the file and
    line that changed, the result, the share of the commands run
    again), the errors that are new in full and the warnings that are
    new, under a footer (the last builds' times as a sparkline, the
    keys). On a terminal in the foreground it reads keys (`r` rebuild,
    `o` open, `w` warnings, `c` clear, `q` quit) with echo, line
    editing and the signal keys off: Ctrl-C stops the watch after
    saving its build (twice: at once), Ctrl-Z suspends it. A watchdog
    `sh`, waiting on a pipe, restores the terminal's modes and cursor
    if partex dies first. partex takes no dependency for this: the
    terminal's size and modes are `stty`'s.
  - Elsewhere (a pipe, a file, CI) the same lines come out plain, as
    they happen, and nothing is redrawn. `--color`, `NO_COLOR`,
    `CLICOLOR_FORCE` and `CLICOLOR` choose the colours.

---

## 3. The execution model: dynamic SSA

### 3.1 The invariant

> A build is one graph. Every operation is a **call** `v = f(a₁ … aₙ)`.
> Every piece of state is an immutable **value**, whose version is its
> content, and an operand is a version. **A call runs again if and only
> if a version it read changed**, and the change stops wherever a call's
> results come out equal. A job that reads what it wrote is a **cycle**
> in the graph, iterated in memory to its fixed point. The hierarchy is
> the call tree, and there is no other mechanism.

This sentence overrides any other line of this file. A line that
contradicts it is a bug here, fixed here. A rebuild whose checking
grows with the document rather than with the edit is a bug in the
dependency graph, and it is fixed before anything else.

TeX has no static program: a macro's meaning is data, `\csname`
computes an address, and what `\ref` reads depends on its argument. So
the graph is built as the engine runs. It is a trace in SSA form: each
write makes a new version, each read names the version it read, and a
load of what the job stored is a φ. The construction draws on:
- self-adjusting computation (Acar);
- Adapton (demand-driven, by name);
- Salsa (keyed queries with verified dependencies, and cycles iterated
  to a fixed point);
- "Build systems à la carte" (early cutoff);
- differential dataflow.

### 3.2 Values, addresses and definitions

- **All state is values.** The equivalents of eqtb, the nest's lists,
  boxes, token lists, the group frames, the page builder, marks, fonts,
  hyphenation, the PDF and DVI writers' tables, the streams and the
  source. A value is immutable and persistent: a new version shares
  what it did not change.
- **A version is content.** It is a hash made when the value is made,
  from its parts' versions (a Merkle tree), so two versions are equal
  if and only if the contents are. A scalar is versioned by its value.
  Nothing is hashed at a read.
- **A read is of a field.** A value is a tree, and each subtree has a
  version. The page builder reads a box's height and depth, not its
  list, so a paragraph whose lines kept their heights leaves every page
  step after it unchanged. The finer the reads, the earlier the cutoff,
  and the representation sets both.
- **An address is what a read computes.** It can be an eqtb index, a
  hash entry, a register, a field of the page builder, a stream, a
  font's parameter. TeX computes addresses as it runs: `\csname` builds
  a name, `\count\n` reads a register's number, `\ref` reads the entry
  its argument names. So a node cannot hold its operand edges before it
  runs. An address is never a control sequence's spelling: `\r@foo` is
  an address like any other.
- **A definition is a node's output at an address.** It keeps its
  value, its timestamp (its place in program order, 3.9) and its
  readers. The state at a point is stored nowhere. It is, address by
  address, the definition that reaches that point: the last one before
  it that is still in scope. There are no checkpoints, entry states,
  clones or restores, under any name.
- **The definition index** maps an address to its definitions, sorted
  by timestamp. A read at timestamp *t* resolves (address, *t*) to the
  reaching definition, and that resolution becomes the read's data edge.
  It is the current-definition map of SSA construction (Braun et al.,
  "Simple and Efficient Construction of SSA Form", 2013), kept alive
  because construction never ends: a node that runs again may compute
  another address. As a data structure it is a "fat node" persistent
  array (Driscoll, Sarnak, Sleator and Tarjan, 1989), with timestamps as
  versions. A read has three paths:
  - *one definition*, the common case (the format's, the preamble's,
    most catcodes and fonts): one load and a test;
  - *at the frontier*, where a cold build and a worker running ahead
    read: the address's last definition, tested first;
  - *in the middle* of an address with many definitions (counters,
    `\@tempa`): a binary search, or a B-tree for the hottest addresses.
- **The index keeps a span's net definitions.** A *span* is a run of
  nodes that runs again as one. A definition overwritten inside the
  span that made it is reached by no read outside the span, so the
  index does not hold it: it lives in the records of the calls that
  made it and, while the span runs, in the span's local log. A node
  inside a span is re-entered by running the span from its start, with
  the hits of the calls before it applied (3.4). The grain is a policy
  over memory and re-entry, and results are identical at any grain
  (3.3). Measured on the course (the event log, 5.2), commands make
  62.7 M definitions, and 95% are overwritten within 1,024 commands:

  | spans | definitions kept | reads crossing a span |
  |---|---|---|
  | a command each | 62.7 M (100%) | 52.0 M |
  | windows of 256 commands | 16.2 M (26%) | 12.1 M |
  | windows of 4,096 commands | 3.8 M (6.1%) | 2.9 M |
  | steps, from one outer clean point to the next | 703 K (1.1%) | 556 K |
  | the setup as one span, then steps | 638 K (1.0%) | 476 K |

  Kept per command the index alone takes 6 GB, against 180 MB for the
  whole plain build. Every command boundary can begin a span, since
  where the engine is is values (3.9). The spans are:
  - the *setup*, the build before its first `\shipout`: 6.5% of the
    course's commands, making 4.3 M definitions of which 150 K reach
    the body. A setup edit runs the setup again and wakes the body's
    readers of the definitions whose versions changed.
  - after it, *windows* of *N* commands, *N* a few thousand: an edit
    re-runs at most one window's commands before its own, and the index
    keeps about 6% of the definitions. (Outer clean points are too few
    in a body to cut it: 2,100 on the course, 29 K commands apart.)
- **Its representation.**
  - *The frontier*, each address's last definition, is held in the
    engine's tables (eqtb, the hash, the registers), with the key of
    the span that made it. A cold build, and a span after every other
    definition of an address, read there: one load and a test.
  - *Older definitions*: per address, the spans that defined it, in
    order, each naming the record and the write that hold the value.
  - *The running span's local log*: its own definitions, read first. A
    span that runs in the middle of the build never writes the
    frontier. At its end it publishes: its definitions replace its old
    ones, and the frontier takes those that are now last.

  A read in a span at key *k* resolves in its log, else at the
  frontier if that was made before *k*, else to the definition before
  *k*.
- **Every read is recorded by construction.** State is reached only
  through accessors, and each accessor resolves through the index and
  notes the edge. A node that runs again reading the same addresses
  follows its old edges; it resolves only addresses it did not read
  before.
- **A local definition has a scope.** An assignment inside a group
  (§279) is a definition whose scope ends at the group's end. After the
  group, the reaching definition is the one before it, the same node,
  not a copy of its value. So:
  - readers after the group read the outer definition itself, and an
    edit of it wakes them directly;
  - an edit inside the group never reaches past the group's end;
  - `\global` inside the group (§283) is a definition with no scope
    end, which ends the outer definition's range;
  - `eq_level` and `xeq_level` (§277–§283: whether an assignment must
    be saved) are the group level of the reaching definition.
- **The save stack is the stack of group frames.** A frame is real
  state, a value: the group's kind and boundary (`cur_group`,
  `cur_boundary`, e-TeX's line), the saved context of a box, alignment
  or math group (the `saved(k)` words), the `\aftergroup` tokens, and
  pointers to the local definitions made in it. TeX's undo log, copies
  of old eqtb entries, does not exist: at the group's end its local
  definitions end, and `\tracingrestores` prints from the definitions
  (`{restoring …}`, `{retaining …}`, §283, §284).
- **Positions are data.** A line number is a value that `\inputlineno`
  or an error message reads. It is never part of a node's name.
- **Allocated numbers are not positioned.** String numbers, hash slots
  and glue lineages are handed out by the build and never observed: a
  string is read by its characters, a control sequence by its name. A
  node that runs again allocates past the arrays' ends.
- **A font's identifier is positioned.** `font_id_text(f)` (§256) is
  the one hash text that changes: each `\font` that names an already
  loaded font sets it. So those hash slots are placed like eqtb's
  (`rebuild::font_id_slot`). Before, a step run again in a later trip
  read the identifier a later step had set: an overfull box's
  `\OT1/cmr/m/n/12` came out as `\U/cmr/m/n/12`.
- **A name is made where its meaning is first defined.** Names are never
  taken out of the hash, so a step run again in its place, or in a later
  trip, can find a name that a later step entered. A lookup that did not
  find the name (`\ifcsname`) read only the name and the hash slot it
  probed, and those are not positioned. So a step that writes the meaning
  of a name entered by a later step is, in program order, where the name
  is made. Its write is a definition of the name and of its hash slot
  (`SsaTracker::name_defined`), and that definition wakes the lookups in
  between. They are not put back: the table keeps the name. LaTeX's
  `\@ifundefined{b@key}` in a citation is such a lookup. The first trip
  enters `b@key` at `\end{document}` (`\@testdef`). The trip that reads
  the `\bibcite` then defines it at `\begin{document}`. Without this, the
  citations stayed `[?]` in the converged build (the thesis, and a
  three-line document, `names` in the edit harness).

### 3.3 Calls and records

- **Every operation is a call**, and every routine is one: the
  tokenizer over a line, `macro_call`, `hpack`, `vpack`, `line_break`,
  each `build_page` step, the output routine, `ship_out`, `write_out`,
  the fonts written at the job's end, BibTeX and makeindex. Primitives
  and macros are one category. A group's end (`unsave`, §281) is not a
  call: its restores are writes of the step that runs it (4.3 item 2).
- **A record** holds:
  - the call's name;
  - its body's own reads in order, each a version (a child's reads stay
    in the child's record, and the parent depends on the child by its
    result);
  - its writes;
  - its effects;
  - its children, in order.

  The records are the graph.
- **A name is content.** It is `f` plus the versions of the call's
  inputs: for a paragraph, the tokens it consumed; for `hpack`, the list
  and the spec. No position, ordinal or line number enters a name, so a
  moved paragraph is the same call, and a line retyped to the same
  tokens is a hit. Records with one name are told apart by their reads.
- **A call's accumulator is an argument.** For example, the page-so-far
  is an input of each page step. **A write of an equal value is kept**
  in the record: applied where that address differs, it must set it.
- **Evaluation.** Find the call's records by name, and walk one in
  program order, comparing each read with the version now current.
  Reads of addresses the call itself wrote are skipped, and children are
  walked by the same rule.
  - If every read holds, the call is a *hit*: its writes are stored,
    its effects come back, and its children are not visited.
  - At the first read that differs, it is a *miss*: the body runs, and
    each call it makes is evaluated the same way.

  Reads are checked in order because a later read's address can depend
  on an earlier read's value.
- **The grain.** A call is recorded if its body usually costs more than
  a record, about a hundred nanoseconds. The results are identical at
  any grain. Outside check mode (4.3 item 2) only the steps and the pure
  typesetting calls (`line_break`, the packs, the page steps,
  `ship_out`, the fonts) have frames and records; `tokenize`,
  `write_out` and the output routine run in the frame around them. A
  step's record keeps its net writes, effects and children but not its
  own reads: its reads from outside it are the fold's (3.15), and its
  own are kept only in check mode, where it is looked up.

### 3.4 Change propagation

An edit replaces lines of the source. Each value keeps its readers. A
rebuild runs the readers of what changed, then the readers of what
their re-runs wrote differently, and nothing else. For a word typed in
a paragraph:
1. The tokenizer call for that line misses.
2. The paragraph re-runs `line_break`.
3. If its lines keep their heights, every page step that read them
   hits.
4. The page's `ship_out` runs again, because its bytes changed.
5. Everything after it is untouched.

The work of a rebuild is the sum of the bodies that ran, and each ran
because a version it read changed.

- **Readers by definition.** A changed definition wakes its readers:
  the reads resolved to it, up to the address's next definition or its
  scope's end. A definition inserted between an old one and its readers
  redirects their edges, and wakes those whose version changed. An equal
  value changes nothing: that is the cutoff.
- **Appends.** A list that grows by appends (the page's list, a
  stream's lines, the glyphs each ship uses) is a definition that does
  not end the one before it. A changed append makes dirty only the
  readers of the whole value (the fire, a load, the job's end writing
  fonts), never the calls that appended after it. The page's list is
  read by its length and whether its last node precedes a break
  (`LIST_LEN`, `LIST_TAIL`); a page step reads those two, never the
  list.
- **Hits applied inside a call that runs again.** A call made inside a
  re-run body is probed (3.3's evaluation). If it applies (below) and it is a hit:
  - its writes are stored;
  - its effects come back as the chunks of its record's subtree;
  - it becomes the running call's child;
  - its reads from outside it become the running call's reads.

  A call *applies* only if its record holds everything it makes, with
  nothing handed back to its caller outside it. Today those are the
  fonts (`font_file`, `encoding`, `font_dict`). Each other routine
  applies once its results are values (4.2, item 5). A group's end is
  not a call, so the `\aftergroup` tokens it puts in the input (§326)
  come from the step's own run: no hit can lose them.
  `PARTEX_SSA_APPLY=0` switches applied hits off.
- **Queries are sources.** The host's answers that are not file
  contents (a file's date, the clock, the timer, a terminal line) are
  reads whose version is the answer. They are asked again at every
  rebuild, and a call whose answer changed runs.
- **Files by stamp.** `Host::unchanged` says which loads would read the
  same again, from each file's stamp and each lookup's trail of
  candidates it did not find, so a rebuild's file checks cost the files
  that changed. `PARTEX_STAT_CACHE=0` reads every file again.

### 3.5 The source and the input

- **Lines.** A file is a persistent sequence of lines. The tokenizer is
  a call per line, over its bytes and the catcodes it read, so a
  changed comment or a doubled space gives equal tokens and everything
  after it hits. TeX tokenizes lazily: a line whose codes changed while
  it was in the buffer is read by its bytes.
- **Reads relative to the call.** A call reads the source relative to
  where it started, per input level open then: the level, the rank of
  the line after the level's position at the start, and the codes. So
  an edit before a call does not change what the call read.
- **Edits are found by lines.** The common prefix and suffix are cut at
  line boundaries, and the lines between are diffed (Myers). An edit is
  a list of hunks. A position between hunks moves by the hunks before
  it, and a line a hunk touches is dirty. Each load's contents is a
  separate *data* (a file, a φ, or a value served from the stores), and
  an edit is of one data.
- **The input stack as a value.** Where a call leaves the input is its
  result. That value must be cheap enough to take after every command
  (3.9):
  - TeX's buffer (§30) is the stack of the open file levels' lines.
    Every write to it lands in the top file level's line or above
    `first`: a line read (§362, §538, e-TeX's pseudo lines), `^^`
    reduction (§355), `\endlinechar` at the limit, `\pausing`'s line
    moved down (§363), and `\csname`'s scratch (§372). So the bytes
    below the top file level's start are frozen while that level is
    open. They are one shared `Arc<[u8]>`, made once per file level
    opened or closed, and a value holds it plus a copy of the top line.
  - The levels are a shared value: each level holds its record and the
    value of the levels below it (`input::Chain`), so a push shares
    what is below and a pop returns to it. The engine's stack stays an
    array. Beside it, `input::Prefixes` keeps the value of each prefix
    while it holds: a push or pop at a level forgets that level's, so
    taking the value makes only the levels pushed since the last one.
    The parameter stack (§308) is a value the same way.
  - The open file levels below the top one (their files and positions,
    saved line numbers, e-TeX's group and condition depths, `\everyeof`
    flags and names) do not change while a level above them is open.
    They are one shared value (`input::FileLevels`), made once per file
    level opened or closed, beside the top level's own entries.
  - A `\scantokens` pseudo file is a value: its lines, shared, and the
    index of the next one to read (`input::PseudoFile`).
  - Token lists are shared already. The files' read positions are an
    `Arc` of the contents plus an offset.
  - The input's value is a node's operand (3.9). Re-entering the engine
    at a node writes its levels, lines and files back into the engine's
    arrays, costing their size, only where a node is re-entered.
  - Measured on the course (66 million commands), taking the value after
    every command costs about 0.33 µs per command, against 1.9 µs for
    copying the input. Most of what remains is the top line's copy.

### 3.6 Functions are values

A control sequence's **meaning is a value**, defined at the name's
address like any other value. It is one of:
- a **macro**: its parameter text and body, an immutable token list,
  with its flags (`\long`, `\outer`, `\protected`);
- a **primitive**, whose version is fixed by the partex binary, so a
  new build of partex invalidates exactly the records that ran its
  code;
- a **character token** (`\let\bgroup={`);
- **undefined**;
- an **address**: `\countdef\foo=5` makes `\foo` a value that *is* a
  reference to count register 5, and a read through `\foo` resolves
  register 5's reaching definition. So addresses are values too.

**Definitions of meanings are nodes:**
- `\def` and `\edef` are nodes whose output is a function value. An
  `\edef` whose expansion comes out equal has an equal version, and the
  change stops there.
- `\let\a\b` defines `\a` as **the same value node** as `\b`'s reaching
  definition: an edge, not a copy.
- A macro that defines macros is a node whose outputs include function
  values: higher order, with no special case. Hooks built by
  `\g@addto@macro` are appends, whose one reader is the hook's use.

**A call is an apply node.** `apply(f, args)` has as operands:
- the function value, an edge resolved through the name's address at
  the call's timestamp;
- the arguments' token lists;
- whatever the body reads.

Its memo key is the function's version and the arguments' versions,
never the name. So `\a` and `\b` made `\let`-equal share their results,
two names with one body share results, and a macro defined back to an
earlier body finds its old results in the memo (3.9).

- A redefinition is a new definition and nothing else: uses before it
  read the old value, uses after it the new one, and an edit of a `\def`
  wakes the call sites it reaches, up to the next definition or its
  scope's end (3.2).
- `\ifx`, `\futurelet`, `\meaning` and `\expandafter` read the same
  meaning. `\csname` computes the address the function operand is read
  at.

**What a function value carries**, derived from its version and dropped
when a different version reaches:
- a compiled or specialised body, as a JIT deoptimises when its
  assumption breaks;
- its body's spans, so that a call can later depend only on the spans
  it consumed. For now a call depends on the whole body. The spans are
  used when a measurement shows large macros re-running for branches
  they did not take.

A call's body evaluates to a subgraph of children. For one function
version and the same operands its shape is always the same: a
template, which a compiled backend exploits.

### 3.7 Stores, loads and the cycle

- **Stores.** `\openout`, `\write` and `\closeout` make a store: a
  stream is a sequence of lines, and `write_out` appends to it.
- **Loads.** `\openin`, `\read` and `\input` of the same name are a
  load, served the stored value in memory, never the file, which a call
  re-run out of order may have truncated. So is every other read of a
  file by name on TeX's path: `\pdffilesize` (LaTeX's `\IfFileExists`
  calls it, through expl3's `\file_full_name:n`), `\pdfmdfivesum file`,
  `\pdffiledump`, `\pdfobj file` and an image (`Tex::read_source`).
- **No name is special.** `.aux`, `.toc`, `.lof` and `.bcf` are
  addresses the document chose.
- **The cycle is a φ.** A load of a name the job stores reads, on its
  first trip, what the previous build stored (in a process's first
  build, the file on disk), and afterwards what the job stored. The job
  has converged when each load read what the same trip stored, compared
  by versions. A store that changes makes its loads dirty *in the same
  evaluation* (3.9): there is no second rebuild and no "pass". Trips
  are bounded as latexmk bounds them: five, then a report.
- **Outside tools are calls.** BibTeX reads the `.aux` stream's
  `\bibdata`, `\bibstyle` and `\citation` lines and the `.bib` files.
  makeindex reads the `.idx` stream. biber (planned, sandboxed) reads
  the `.bcf` stream and the `.bib` files. Each tool's output is a
  stream its reader loads, and a tool runs if and only if an input's
  version changed. Which tool reads which stream is fixed in partex.
- **Files are a view.** The build holds the streams and writes files
  for outside tools, never reading its own writes back.

**Trips, as built** (`ssa/rebuild.rs`, `rebuild_trips` and `settle`):
- *A build is a sequence of trips.* Trip 1 runs what the edit reaches
  (3.15, "A rebuild"); a cold build's trip 1 is the whole job. A store
  whose lines changed makes dirty, in the same trip, the later loads
  that read the job's own store (3.15, step 4). The loads that read the
  φ wait for the next trip.
- *Trip 1's φ* is the file on disk: what the last build's link wrote,
  which is what its last trip stored, or, in a process's first build,
  what the run before left.
- *A trip's end.* Each stored name has a value: its lines since its
  last open in the fold, or the trip's φ if no live step opens it.
  That value is the next trip's φ, held in memory. The file is never
  read back: a step run again out of order has truncated it (its open
  is the host's `File::create`), and the link writes it only after the
  last trip. A name whose value is the same keeps its φ, the same
  shared bytes.
- *Trip k+1's seeds* are the steps whose load of the φ found other than
  the new φ, compared by versions, and, for a name whose φ changed and
  that is read by lines, the steps that read the lines it changed, as
  an edit of the data (3.15, "Lines are of a data"). Nothing else is
  looked at: no source file and no query (a rebuild asks the host once,
  in its first trip), except the loads again after a tool wrote a file
  (below).
- *Outside tools run between trips.* After each trip, BibTeX runs on
  each stored `.aux` that has a `\bibdata` line, and makeindex on each
  stored `.idx`, unless what the tool's last run read reads the same:
  its stream's lines (BibTeX's `\citation`, `\bibdata`, `\bibstyle` and
  `\@input` lines, makeindex's whole `.idx`), its style and its
  databases. That is a memo of one entry per stream, the one `-watch`
  keeps (`bibtex::Runs`, `makeindex::Runs`). A tool reads the streams
  from the build's stores, served by name, never from the files, and
  writes its outputs (`.bbl` and `.blg`, `.ind` and `.ilg`) as the
  conventional tool does, in the output directory: files the job loads
  by name like any input. A tool that wrote makes the next trip look
  at the job's loads again, as trip 1 does, so the steps that loaded a
  changed output (a `.bbl` that appeared) are seeds.
- *Convergence.* The build has converged after a trip whose end makes
  no seed: every load of the φ read what the same trip stored. The
  bound is `PARTEX_SSA_TRIPS` trips (default 5, latexmk's
  `max_repeat`). A build that reaches it with seeds left keeps the last
  trip's output, as latexmk does, and reports the names whose loads
  differ (`not settled`). The link runs once, after the last trip.
- *The cold build converges too* (decided 2026-10-02): trip 1 is the
  job from its start, and trips 2 on follow as above. A process's first
  build is then what latexmk makes from the files on disk, and every
  stage of the edit harness compares with the oracle run to its fixed
  point (`scripts/ssa-edits --fixpoint`).
- *One trip per build.* `PARTEX_SSA_TRIPS=1` keeps the behaviour before
  trips: one trip per build, no tool run, so a rebuild matches one plain
  pass and a label needs two rebuilds (the harness without
  `--fixpoint`).
- *A rebuild stopped* (decided 2026-10-04). A rebuild stops past its
  deadline (`SsaTracker::deadline`), past its budget of commands
  (`SsaTracker::budget`), or when `SsaTracker::cancel` says so: the
  extension's keystroke cancels the rebuild under way. It stops only
  after a step's run is placed, and it keeps its work: the dirty steps
  it did not reach, each with why (`Dirty`, its `missed` slots too),
  and the φ its trip served (`Steps::pending`). A stop inside a cascade
  that runs on (the step ended elsewhere) marks the old step after it,
  which then runs again from where the step ended, as when an input
  changed (3.15, step 5): the cascade's other state is not kept. With
  no old step after it, the cascade goes on. The report says
  `stopped` and the number of steps `pending` (`ssa::pending`), not
  `unsupported`; only a state the rebuild cannot make is unsupported,
  and then only a cold build is sound.
  - *The next rebuild goes on.* `rebuild`, `rebuild_trips` or `settle`
    takes the pending steps as seeds beside the new edits' (a mark
    runs a step whichever way it came). An edit is found against the
    data each step read, so a pending step that has not run since the
    first edit is diffed against the source it read then, and a step
    the stopped rebuild ran against the source it ran on: an edit
    before, inside or after the pending region marks the steps whose
    lines it changed, and the places each step ends at are mapped
    through all the edits since its run. The φ is the stopped trip's.
    A trip that stopped is not a trip that ended: no store is compared
    and no tool runs until it ends.
  - *The link waits.* While work is pending the program mixes runs of
    two sources, so nothing is linked: the files, the PDF and the
    stores' files are the last complete build's. The φ of trip 1 is
    then still the files on disk. The CLI's rebuilds
    (`PARTEX_SSA_REBUILD`, one per line) with `PARTEX_SSA_REBUILD_MS`
    or `PARTEX_SSA_CANCEL_AFTER=N` (with rebuilds, counted from each
    rebuild's start: the cold build is not cancelled) report `stopped
    (…), N steps pending` and link nothing; after the last line, the
    work left goes on unstopped (`continued`) and is linked.

### 3.8 Output: effects and the link

- Every output byte is an effect, handed to the innermost call. The
  outputs are the log, the terminal, each `\write` file, the DVI file,
  and the PDF bytes with their object marks.
- A call's effects are cut into **chunks** at the boundaries of the
  calls that apply (3.4): at such a call's start, and at its end before
  its writes are versioned. Each chunk carries a version made from its
  content, keyed by `step << 32 | k`.
- **The link costs the changed chunks** (`effects::Splice`, the SSA
  build's link; 4.3 item 4). It keeps the last link's layout:
  - the chunks in program order (by their step's key, then their place
    in the step), each with its pieces of each file (bytes of `Write`
    effects, or pieces rendered at the link: object streams,
    cross-reference sections, byte counts) and the marks of the objects
    it writes, each at its offset in the chunk's bytes of its file;
  - an **offset tree** per file: a Fenwick tree over the chunks' lengths
    in that file, so an object's offset is a prefix sum plus its mark's
    offset in its chunk, `O(log n)`;
  - each object stream as rendered, with the objects it holds, and each
    cross-reference section and byte count as rendered.

  The runtime logs the steps whose chunks changed since the link last
  took them (a run closed, or the step left the fold;
  `ssa::take_step_changes`). A link replaces their chunks: a step with as
  many chunks as before has each replaced in place (its lengths' changes
  into the trees, its marks into the table); chunks inserted or removed
  make the order and the trees again, `O(n)` without reading any other
  chunk's effects. Then, in order:
  1. the object streams a change reached are rendered again: those a
     chunk put in closes, and the first one closed after each chunk put
     in or taken out (its events fed it), by file;
  2. the cross-reference section is rendered from the trees when a byte
     of its file changed at or before it;
  3. the byte counts (`Effect::Length`) are written with the files'
     lengths.

  Each file comes out with its length and the first byte that changed:
  the least prefix sum over the chunks whose bytes or marks of it
  changed (a chunk put in whose bytes and marks of the file are the
  same does not count). A file that is as last written (its length and
  modification time those of the link's last write, no step opened it
  since) is written from that byte on and cut to its length; any other
  is written whole. Deflate is memoized by content, keeping what the
  last eight links used. The diagnostics, pages and closes are not
  copied; a host walks them when it wants them.

  "The edited page ready" is when the changed chunks are placed: their
  bytes and offsets are final, so an edited page's content stream is
  there, before the object streams, the cross-reference section and the
  byte counts that come after it.

  `PARTEX_LINK_SPLICE=0` links everything in full and writes every
  file. A debug build, or `PARTEX_LINK_CHECK=1`, checks every link
  against a full one (files, terminal text, opens, closes, diagnostics,
  pages, and the chunks against the build's). Virtual object numbers
  (SSA mode's, 3.12, and machine mode's) are not laid out by the
  splice: a full link resolves them, at every build. Machine mode keeps `effects::link_cached`: its regions are
  resolved from a cache and laid out in full.
- The writer's position in the file and the objects' offsets are not
  state: TeX never observes them, and the link places every object.
- **Fonts' numbers are the link's.** pdfTeX's `/F` names number the
  fonts in the order the job loaded them (§576). The engine's table keeps
  every font any run loaded, including one that only an older trip
  loaded (the bold `?` of a citation not yet defined). So with virtual
  object numbers each `/F` number is a relocation (`Effect::FontRef`).
  The link numbers the fonts from the `Effect::FontLoad` events of the
  steps' runs as they are now, in program order. A font a step run again
  makes anew in its slot (`remake_font`) is loaded there too.
- **Columns are the link's** (`effects/flow.rs`). `term_offset` and
  `file_offset` (§54), where the terminal's and the log's lines stand,
  decide only what is printed: the wrap at `max_print_line` (§58),
  `print_nl`'s new line (§62), and the space or new line before a page's
  `[` (§638), a file's `(` (§537), a `\message` (§1280) and `\scantokens`'
  `( `. As state, every step that prints read and wrote them, so one
  message a character longer changed them for every later step that
  printed: 261 changed definitions over a thesis's build and one rebuild.
  So in an SSA build they are not slots. The engine records what it
  prints as a *flow* (`Effect::Flow`): characters, new lines, and each
  of those decisions as an op (`NLC`, `SEP`), with no column in it. The
  link renders a step's flow from the columns the step before it left
  (`ssa::resolve_flows`): it renders the steps whose chunks changed, and
  then each next step while the columns at a step's end come out other
  than before, which a new line soon stops. The engine still keeps its
  own columns, as TeX does, untracked: what they decide that is not
  printed (`tally`, the structured diagnostics' copy) is not in a file.
  A byte count's digits are rendered from the flow too (`LEN` …
  `LEN_END` become `Effect::Length`). `PARTEX_SSA_FLOW=0` makes the
  columns slots again.
- **Writer scopes.** Each routine that changes the PDF or DVI writers'
  tables (`ship_out`, a `\pdf…` command, the job's end, a font call)
  declares the fields it reads and writes. The fields are versioned
  when the scope ends; a child call versions its parent's fields first.

### 3.9 Evaluation: a dynamic sea of nodes, run to convergence

The IR is MLIR-like: nodes, values and typed edges. The engine is its
first backend, an interpreter of one node, with compiled backends
possible later.

**In its edges it is a sea of nodes** (Click's IR, as in HotSpot's C2
and Graal):
- there are no blocks and no passes;
- nodes are constrained only by their edges, and the schedule is not
  part of the IR: the worklist derives it at evaluation time;
- memory is split by address (3.2), like memory SSA with exact alias
  information, since each address is known once computed;
- effects are ordered by ordering edges;
- a memo by content plays the part of global value numbering.

It differs in three ways:
- it is **dynamic**: nodes exist for what ran, and the definition
  index keeps the construction going;
- it has a **hierarchy**: each node is the child of the node that ran
  it, so a hit skips a subtree without visiting it;
- function values with their specialisations (3.6) are closer to
  Truffle and Graal's partial evaluation.

**In its layout it is a trace**, as in self-adjusting computation (Acar,
2005): a dynamic dependence graph with timestamps, change propagation by
a priority queue ordered by time, and memoized sub-traces.
- *Nodes* are every command of `main_control` (§1030) and every call a
  command makes: expansion, a macro call, `hpack`, `line_break`, a page
  step, `ship_out`, a font.
- *Storage.* The trace is stored in program order, struct-of-arrays in
  contiguous chunks, as V8's Turboshaft stores operations and as
  automatic differentiation's "tape" does. Each node has:
  - its function value's version;
  - a range in a flat array of reads, each an address and the
    definition read;
  - a range of writes;
  - a range of effects;
  - the index where its span ends.
- *Timestamps are hierarchical*: a node's timestamp is its parent's
  plus its ordinal among the parent's children, compared
  lexicographically (Dewey order). A node that runs again gives its new
  children timestamps inside its own interval, so insertion in the
  middle needs no global order-maintenance lock.
- *Spans are the hierarchy.* A call is the span of its children, and a
  hit on a node skips to its span's end: one jump. The chunks form a
  rope, so a hit reuses an old span's chunks without copying them.

**Re-entry at any node.** TeX is iterative: groups, boxes and the
output routine go through the group frames and the nest, not the host's
call stack. So everything about "where the engine is" is a value: the
input (3.5), the nest, the group frames, the conditional and alignment
stacks. They are a node's operands, shared, and every command boundary
is a place a node begins. A node inside a span is re-entered at the
span's start (3.2).

**Identity.** A node is named by its function's version and its
operands' versions, and placed by its timestamp. A node that runs again
may make different children, and a child whose name and timestamp
match an old one is that node, reused.

**No chains through a cursor.** V8 moved away from a sea of nodes for
JavaScript ("Land ahoy: leaving the Sea of Nodes") because nearly every
operation had effects, so nearly every node sat on one chain and
nothing floated. TeX has the same danger: state used as a cursor makes
each command depend on the one before it. So no cursor is read as a
value:
- the current list is appended to, and a command that appends does not
  read the tail;
- the input position is read relative to the node, so a command that
  takes more input does not shift every later command's reads;
- allocators name what they allocate by identity, not by count.

The SSA report prints the longest chains of consecutive definitions of
one address, which say how much can float.

**Edges.**
- *Data*: a definition to each read that resolved to it.
- *φ*: a load of a stored stream merges the stores that reach it.
- *Ordering*: effects carry no value but come out in program order.
  They order commits, never evaluation.
- *Sources*: the host's answers, asked again at each evaluation. The
  random generator is state with a seed, not a source.

**Memo.** Each node keeps its last *k* (operand versions → results)
pairs, with *k* between 2 and 4 at first. An edit and its revert, or a
value that alternates, find their results without running.

**The worklist.** Dirty nodes are run in priority order until none is
left:
1. a new or changed definition redirects the edges of the reads it now
   reaches, and wakes those readers whose version changed;
2. a woken node whose operands match a memo entry is a hit: its results
   are stored, its effects taken again, its span skipped;
3. otherwise it runs, and each result wakes its readers only if its
   version changed.

Priority is timestamp order by default, so in the acyclic part each
node runs once per change. Across a φ, readers enter the list again at
their timestamp.

**Convergence.** The evaluation ends when the list is empty. A node
whose operands repeat a memo entry with a different result is
oscillating. After a bound (five, per node) the evaluation stops, keeps
the last results and reports the node.

**The frame.** On a keystroke the priority is the viewer's: the nodes
whose results reach the page on screen come first, that page is painted
from its page node's results (not from the PDF file), and the rest
converges in the background. A new keystroke cancels it. The target is
16.7 ms to the painted page, even when a paragraph pushes later pages.
A measurement reports both the time to that page and the time to
convergence.

**The oracle.** The result is the fixed point. It is checked against
plain partex, or pdfTeX, run until its stores stop changing (latexmk's
rule; `scripts/ssa-edits --fixpoint`). A switch keeps one evaluation per
trip (`PARTEX_SSA_TRIPS=1`, 3.7), so a stage can still be compared with
a single plain run.

### 3.10 Threads

Many threads run the evaluation of 3.9 at once. The design is
self-adjusting computation's change propagation run the way Block-STM
(Gelashvili et al., 2022) executes an ordered block of transactions in
parallel: optimistically, over a multi-version memory, validated at
publish, committed in order.

**Why any order is correct.** A node runs again if and only if a
version it read changed. A node run early, with a value an earlier node
has not finished changing, is woken again when that value changes. So
running out of order wastes work but never gives a wrong result. In the
acyclic part each node's final inputs are fixed by the final outputs of
the nodes before it (TeX is deterministic), so every order reaches the
same fixed point, the sequential result. This is chaotic iteration, as
in a dataflow analysis's worklist. The φ's converge as they do with one
thread, under the same bound.

**Shared, read by every thread:**
- the trace's chunks, immutable once published;
- the values, immutable `Arc`s;
- the definition index (3.2), which is Block-STM's multi-version
  memory with timestamps for transaction indices. It is sharded by
  address, reads are lock-free (old entries retired by epochs), and an
  insertion takes the address's lock.

**Per worker:**
- the scratch fields (`cur_cmd`, `cur_val`, the scanners' results) and
  the running node's frame;
- a local log of the node's new trace entries, definitions and reads.
  Nothing in it is seen by another thread until it is published.

There is no working copy of the state: every read resolves through the
index at the node's own timestamp, so a worker can run any node.

**The worklist** is one relaxed concurrent priority queue of dirty
nodes, keyed by (the viewer's page first, then timestamp): a MultiQueue
(Rihani, Sanders and Dementiev, 2015), or per-worker deques of
timestamp ranges with stealing. Strict order is not needed, since order
changes only the wasted work. A node is in it at most once: its state
goes clean → dirty → running by compare-and-swap.

**Running a node:**
1. *Claim* it.
2. *Probe the memo* (3.9). A hit publishes its writes and effects and
   skips its span.
3. *On a miss, run the body.* Children run inline, depth-first, since a
   TeX body needs its children's results in order. A routine declared
   *future-safe*, whose result its caller does not need yet, is spawned
   onto the worklist instead: the packs of a paragraph's lines, a ship's
   bytes, a font written, deflate, a line's tokens ahead of their
   reader.
4. *Publish, then validate.* Insert the node's definitions at its
   timestamp and register it as the reader of each definition it read;
   then resolve each read again. If one now reaches another definition
   (an earlier node published meanwhile), the node runs again. This
   order, publish before validate, is what loses no wake-up. An inserted
   definition wakes the readers whose edges it redirects, if their
   version changed.
5. *Estimates.* A node about to run again first marks its old
   definitions as estimates. A reader that resolves to an estimate waits
   for the re-run, or queues behind it, instead of reading a value about
   to change. This stops cascades of wasted work.

**Commit.** Effects are ordered by timestamp. The *settled prefix* is
everything before a timestamp that is clean, validated and not running;
its effects are final, and the link streams them in order. The page on
screen is painted as soon as its page node lies in the settled prefix.
The evaluation has converged when the worklist is empty and nothing
runs.

**Cancellation.** A new edit bumps an epoch. Running nodes test it at
safe points and drop their local logs; nothing was published, so
nothing is undone.

**Cold builds** use the same machinery, seeded speculatively:
- the previous build's definitions, from the session, are estimates;
- chapters and pages are seeded as parallel ranges, `\count0` chains
  additively, and the page builder is a fold per chapter (`\chapter`
  clears the page);
- slicing (3.12) makes no PDF bytes before the fixed point.

**Guarantees and limits.**
- Output depends only on the fixed point and the timestamps, never on
  the schedule (principle 5).
- One worker is the sequential engine: the same code, never failing a
  validation, and the default on wasm without threads.
- The parallelism is the width of the dirty frontier. A word edit is
  narrow: its frame is met by doing little work, not by threads. A
  reflow over *N* pages, a cold build and a changed preamble macro are
  wide once speculated; their dependencies alone chain them (below).
- The limits are true dependencies only: chains of definitions of one
  address, kept short by the no-cursor rules (3.9), and the page fold.
- The SSA report prints the frontier's width, the re-runs validation
  caused, and the estimates hit, so the waste is measured.

**Measured** on the course (`PARTEX_SSA_DAG`, `scripts/ssa-parallel.py`,
LOG 2026-10-02 "How much of a build could run at once"), with a worker
per step and the steps' boundaries known:
- *Waiting for dependencies gains nothing.* The 58,710 steps (66.06 M
  commands) are one chain: a critical path of 66.04 M, 1.00x, whether a
  step waits for the steps it read from to end or only, at each read, for
  that definition (pipelined). Dropping the reads of local saves and
  restores keeps it at 1.00x, and dropping every read of an address the
  step itself defines (an upper bound for scoped definitions, 3.2) gives
  1.15x. The chains are LaTeX's state that most steps touch near their
  start (`\reserved@a` saved by a local `\def` inside the `document`
  group, `\@currenvir`, `\if@nobreak`, `clubpenalty`, `align_state`,
  `cond`, the save stack, the allocators), not the page fold: pipelined,
  the page builder's edges alone allow 12.4x.
- *Speculation parallelizes.* Run from the last build's entry states and
  validated in order, 99.995% of the steps validate after a word edit,
  99.997% after a TikZ edit and 99.6% after a `\label` (99.8% once a save
  and restore inside a step is not a read, TODO 2); the bound is then
  the costliest step's, 12.4x (8.4x for the `\label`, whose 243 failures
  chain through `\@savsf`): the largest steps are pgfplots figures and the
  cover, 3.6–5.3 M commands each, 26% of the build in four steps, so past
  12x they are cut into windows (4.3 item 1), speculated too.
- *Validation needs versions that are content.* A cache read and written
  as a value (a font's expanded instances, `font:…expand`), an object
  numbering (`pdf.fonts`), a save restored by every output routine
  (`\@savsf`) and a definition a re-run does not make again (a font
  loaded, a name entered) each fail a validation that should hold.

### 3.11 Records, memory and sessions

- A session is the last build's records and the values they reference,
  deduplicated by content. Memory is bounded by what the build touched.
- **The collector** is split into mechanism and policy:
  - the mechanism marks from the roots of the builds kept and drops the
    rest; values are immutable, so reference counting frees them;
  - the policy sees each record's age, cost, size and hits, and keeps or
    drops within a budget.

  A kept record is still verified before use, so no policy can make a
  hit wrong. Today it collects inline, keeping the last three builds'
  roots, when the arena has doubled. Planned: on its own thread.

### 3.12 Optimizations on the trace

- **Free by construction.**
  - Value numbering and CSE are the memo.
  - Loop-invariant motion across the cycle is a hit.
  - Constant propagation on the φ is the convergence test.
  - Backdating is a re-run that writes equal values.
- **Planned, in order of value.**
  - *Slicing*: a trip before the last needs only the nodes that reach a
    store, so `ship_out` splits into the whatsits and the bytes.
  - *The cycle's SCC*, which bounds what can iterate.
  - *Escape analysis* for scratch fields.
  - *Dead stores* on streams nothing loads.
- **Later: trace compilation.** A recorded trace is straight-line SSA
  with guards, a tracing JIT's input.

**As each step closes** (`SsaTracker::end_step`). A step's reads from
outside it and its definitions are its node in the graph. What a step
records here decides how far an edit's change travels: a read the step
did not need makes it rerun, and a definition it did not make makes its
readers rerun. What is built:
- *Copies of the save stack* (soft reads, `PARTEX_SSA_SOFT_READS`). A
  local assignment in a group the step opened saves the slot's entry
  value and the group's end puts it back. That is a copy, so a slot left
  as the step found it is neither read nor defined. An entry value still
  saved at the step's end is a read.
- *Class reads* (`PARTEX_SSA_CLASS_READS`). A lookup that only stores
  the token reads the name's class, not its meaning.
- *A soft read decides nothing* (`PARTEX_SSA_SOFT_PLACE`). A soft read
  is not a read, so a rebuild does not place the slot, and the run finds
  whatever value the arrays hold, possibly a later definition's. Both
  of TeX's decisions on that value are made as a consistent state would
  make them:
  - whether to save it: §277 compares its level with `cur_level`, and an
    entry value's level is below the group's;
  - whether to assign it: e-TeX skips an equal value (`reassigning`).

  So a slot that holds the step's entry value (`Tracker::entry_value`)
  is always saved and assigned. In a plain run the outcome is the same:
  the value is saved and put back, and only the save stack's use
  differs, which nothing observes. An entry value still saved at the
  step's end is a read, so a rebuild places it like one: a run that
  found it at a later definition is dropped and placed again.

  Measured on a 64-page thesis, a section title edit reran 986 steps
  without this and 85 with it. A run had found the frontier's value of
  `\=` (a later step's), skipped the assignment as `reassigning`, and
  saved nothing. That shifted the save stack under every window of the
  long figure that followed.
- *Dead save stack entries* (`PARTEX_SSA_DEAD_SAVES`). An entry at or
  above the pointer at a step's end has been popped, and every command
  writes an entry before it reads one above the pointer (§274's
  `saved(k)` is written, then the pointer moves up). So the step's write
  of such an entry is dead, and it is not a definition. Without those
  definitions the arrays can hold a dead value that no later definition
  shows, so a rebuild places the stack below a step's pointer whole
  (`save_stack_whole`). An entry made a plain value drops any object a
  dead entry left in its place, which its version would otherwise
  carry. The rerun check's changed definitions fall from 3,001 to 680 on
  the thesis, and from 15 to 5 on acro2.
- *e-TeX's saved registers above 255, entry by entry.* The chains of
  registers above 255 saved locally (`sa_chain`, one chain per level
  that saved any) are a shape slot (`save.xchain`: the current chain's
  level and each level's length) and one slot per entry, the chains laid
  end to end (`save.xchain[i]`). An entry never changes once pushed: a
  save reads and writes the shape and writes its entry; a restore reads
  the shape and the entries it restores, and writes them (dropped) and
  the shape. One slot hashing every entry made every later local
  assignment of a register above 255 (pgf's `\dimen261`…) depend on any
  saved value below it. The shape is one structure with its entries, as
  the nest is with its levels' fields: wherever a rebuild puts the shape
  (placed with the stack below its pointer, `save_stack_whole`; put back
  after a run; a dropped run's undone), every entry within it goes with
  it at its own definition (`with_levels`). (Put back alone, a shape a
  placement had shortened came back padded with stand-ins, and a later
  restore read one: `xeq_level` index panic on the thesis.)

- *Virtual PDF object numbers* (`PARTEX_SSA_VOBJ`; `pdf/objtab.rs`,
  `SsaObjs`). pdfTeX numbers objects in the order they are made, so a
  reflow that splits one link annotation fewer numbers every later
  object again. With the table as one slot (`pdf.objs`, `pdf.obj_trees`,
  `pdf.dests`, read whole by every writer scope), every later step that
  made a destination, a link or a page read a changed table and made a
  changed one, through to the job's end. Now:
  - an object's number inside the build is a *virtual id* that does not
    depend on what came before it: an object TeX identifies (a page by
    its number, a destination by its name, a font, a raw object, form or
    image by its count: `tex_identity`) is named by that identity, any
    other by its step's id and its count in the step
    (`1 + (step << 12 | count)`), so a step run again names its objects
    as before. An applied call (a font's) names its objects by its own
    name. The numbers in the bytes are relocations (`Effect::ObjRef`),
    and the numbering events (`Effect::Num`) let the link write
    pdfTeX's numbers in pdfTeX's order (`vnum.rs`, as machine mode had);
  - each object's entry is a slot (`pdfobj:ID`), and each lookup tree
    entry (`pdfname:KEY`, by type and identifier). A scope reads the
    entries it touched, each at the version it had before the scope
    first changed it, and writes those it changed: the table's log
    (`ObjLog`) is turned into reads and writes when a writer scope or a
    call ends (`Tex::obj_flush`);
  - `pdf.objs` is the lists' heads alone; `pdf.obj_trees` and
    `pdf.dests` are neither read nor written (the destinations' names
    are the destination list's, read at the job's end);
  - each step's numbering events are an append (`pdfnum:STEP`), read
    only where TeX observes a number (`\pdflastobj` and its siblings, a
    number given back to `\pdfrefobj` and the like, the job's end):
    the observer reads the events of every live step before it, in
    program order (`Tracker::steps_before`), and is the only step a
    changed count of objects before it runs again.
  The link resolves the numbers in full at every build (the splice lays
  out pdfTeX's numbers only); on the 64-page thesis that is 6 to 9 ms.
  The engine cannot know the PDF's length (the link fills the object
  streams and writes the numbers), so "Output written on … bytes" holds
  its guess; when the link finds a length with another number of
  digits, the text that prints it is rendered again with the length's
  digits (`flow::render`'s lengths, `ssa::set_flow_length`) and linked
  again. Display lists (4.6) name forms and images by pdfTeX's numbers,
  from the steps' numbering events, as the link does.
  Measured on that thesis (its long reflowing insertion, 4 words in
  Chapter 2): 5,712 steps and 11.0 M commands before, 586 steps and
  0.29 M commands after (LOG 2026-10-04).

**Analysed, not built** (LOG 2026-10-04):
- *Positions in the conditionals.* The `cond` slot holds each open
  conditional's absolute `if_line`. An edit that adds a line before a
  conditional that stays open across steps changes `cond` for every
  step until it closes: 101 of 169 steps on the thesis. A line is
  observable only where a message prints it (`print_line_no`), and those
  steps are already seeded by position ("a line number read is kept by
  position"). So `cond`'s version could leave the lines out. But a step
  placed from a definition that was kept would then hold a stale line,
  and placing it would have to map the lines through the edits, as
  `InputState::mapped` maps the input's. Per-level line slots
  (a push reads none of the lines below) are the other way.
- *A relative save pointer.* A step that leaves `save_ptr` where it
  found it, and never pops below it, uses only addresses relative to its
  entry. It could neither read nor define the pointer. In the data so
  far the pointer chained only through the soft read decisions above.

### 3.13 TeX's state

Every field of `Tex` (and of the structs it owns) is one of four
classes:
- **value**: state a later call can read. It is versioned, and read and
  written only through accessors;
- **scratch**: dead at every call boundary, because every path from the
  boundary writes it before reading it (`cur_val`, `cur_cmd`, the
  file-name scanner's results, printing's work areas,
  `scanner_status`). Scratch is never recorded. A field is scratch
  only with a reason, and a field live at some boundary is a value
  there, or the call's argument or result;
- **effect**: output already made (log, terminal and file bytes,
  diagnostics), never read;
- **configuration**: fixed for the session (parameters, the host, the
  format's constants, `xord`).

The value fields fall into six families:

| family | fields | how versioned |
|---|---|---|
| tables | eqtb, `xeq_level`, registers above 255, the hash (`text`, `next`), the pool (a string by its bytes), `str_ptr`, `hash_used`, `hash_high`, fonts' fields (each `\fontdimen`, `\hyphenchar`, code array…, with metrics by identity), hyphenation (patterns as one address, each exception word its own) | a version array beside the entries, set at the write from the content; a format's load versions them whole. A lookup by name reads the name's address, not the hash chain |
| groups | the group frames (3.2), `cur_level`, `cur_group`, `cur_boundary`, e-TeX's saved registers | frames as a shared stack; local definitions scoped |
| tokens | token lists, the input state (3.5), the open files and their positions, the conditional stack, `align_state`, `after_token`, `\read` streams | lists carry their polynomial; the input is the call's result; a file is a load |
| lists | the nest's lists and boxes, the alignment state, pack results, `last_badness`, `shown_mode`, sealed lines' contents | each level's fields are slots of their own by depth, the nest its depth: entering a level reads only the mode and `aux` it inherits (a box built in vertical mode does not read the `\prevgraf` before it), leaving one writes nothing of the level left to, and a field of the current level reads the depth; a box carries its version, made when packed; a pack's result is its node's output; a sealed line's box is its dimensions and key, its contents a slot of their own |
| page | the page builder's fields (contents, list, `page_so_far`'s eight, `page_max_depth`, `least_page_cost`, `best_page_break`, `best_size`, insertions, `insert_penalties`, `last_glue` …), marks, `split_discards`, `dead_cycles`, `output_active` | an address per field; the list by appends (3.4) |
| output | the PDF writer's tables (object numbering, destinations, fonts and glyphs, the font map, `\pdfglyphtounicode`'s table, color stacks, the `\pdflast…` values), the DVI writer's totals, `\write` streams (`Out(n)`: the name each stores to), `selector`, the log's state, `Random` (the generator's state), `Clock` (the host's answers) | writer scopes (3.8); stores and loads (3.7) |

A read of a whole table is never made. For example, `\pagegoal` reads
one field of the page builder, and `\wd` one field of a box.

**Check mode** (`PARTEX_SSA_CHECK=1`) runs each hit's body too, and
compares the state and effects the hit leaves with those the body
leaves, field by field over the value rows (`Tex::value_rows`). It also
checks every table read's version against the address's content. Any
difference is a misclassified field, or a store that bypassed its
accessor.

### 3.14 Switches

| switch | effect |
|---|---|
| `PARTEX_SSA=1` | build with the recorder; rebuilds by readers |
| `PARTEX_SSA_APPLY=0` | apply no hit in a rebuild (`=1`: apply in a cold build too) |
| `PARTEX_SSA_LEAN=0` | record every routine (`tokenize`, `write_out`, the output routine too) and each step's own reads, as check mode does |
| `PARTEX_SSA_CHECK=1` | check mode |
| `PARTEX_LINK_SPLICE=0` | link in full and write every file |
| `PARTEX_STAT_CACHE=0` | read every file again at a rebuild |
| `PARTEX_SSA_REBUILD` | commands run between rebuilds in one process (the harness) |
| `PARTEX_SSA_TRIPS=N` | trips per build at most (default 5, 3.7); `=1`: one trip, a rebuild matching one plain pass, no tool run |
| `PARTEX_SSA_VIEW=FILE` | the build as a program after each build (4.3 item 7) |
| `PARTEX_SSA_VIEW_STEP=ID` | with the view, step `ID`'s calls in the trace's form |
| `PARTEX_SSA_DAG=FILE` | the steps as a dependency graph after each build (`FILE.N` after rebuild `N`): each step's commands, the step whose definition each of its reads reached, its definitions' versions, and how many commands into the step each read and last write came; `scripts/ssa-parallel.py` measures it (3.10, "Measured") |
| `PARTEX_SSA_WINDOW=W` | windows of `W` commands (default 4,096); `0`: clean points (4.3 item 1) |
| `PARTEX_SSA_RERUN_CHECK=1` | after the cold build, run every window again alone and compare (4.3 item 1) |
| `PARTEX_SSA_SOFT_READS=0`, `PARTEX_SSA_CLASS_READS=0` | a local assignment reads the value it replaces; a lookup that stores a token reads its meaning (3.12) |
| `PARTEX_SSA_SOFT_PLACE=0` | a soft-read slot's level and value decide its save and assignment as the arrays hold them (3.12) |
| `PARTEX_SSA_DEAD_SAVES=0` | every save stack entry a step writes is its definition (3.12) |
| `PARTEX_SSA_VOBJ=0` | the PDF object table one slot, pdfTeX's numbers as made (3.12, "Virtual PDF object numbers") |
| `PARTEX_SSA_FONT_REFS=0` | a font's `/F` number is its place in the engine's table, which keeps the fonts an older run loaded, not the link's (3.8) |
| `PARTEX_SSA_FLOW=0` | the log's and the terminal's columns are slots each printing step reads and writes, not the link's (3.8) |

Each is exact: output is byte-identical with it on or off.

### 3.15 The code today

This is how the code builds 3.1–3.8 today, as a guide to reading it.
4.1 lists what of it 3.2–3.10 replace.

**Versions and persistent values** (`partex-ssa`):
- `Version` is a 128-bit content hash (`Version::of`, and
  `Version::node(tag, parts)` for a tree).
- Sequences use a polynomial hash mod 2⁶¹−1 (`Poly`), so a sequence's
  version does not depend on the tree's shape, and an append costs one
  step.
- `PVec` is a persistent vector, `PMap` a HAMT whose version is the sum
  of its entries' hashes, and `PStack` a persistent stack. Each keeps
  its version as it changes.

**The machine.** The runtime is generic over a `Machine` (its `Func`,
`Addr` and `Val` types) and a `Store` that answers a slot's current
version and stores a value. The engine is one machine (`ssa::TexSsa`),
and a stub language with property tests is another.

**Records.**
- A `Record` holds the call's function and name, its argument versions,
  its result, its own reads (location and version, in order), its writes
  (address and value), its items (children and effects, in order), its
  cost, and its content version.
- The memo maps a name to the records with that name.
- `Runtime::begin(f, args)` opens a frame, and `end(result)` closes it
  into a record. `call(f, args, body)` looks up, verifies, applies or
  runs. `probe` looks up without applying.

**Which reads are the body's.** Every write stamps its slot with a
serial. A read in a frame is the frame's own read if the slot's last
write came before the frame began; otherwise the frame, or a child,
wrote the slot, and the read is internal. A read is noted once per call
through per-slot stamps (a *slot* is the code's word for an address).
Families that are dense arrays (eqtb, the hash, the fonts) keep their
stamps in arrays, so the engine's hot path hashes nothing. The arrays
and the serial outlive a trip: a stamp left by an earlier trip is older
than every frame of this one, as a zero is, so the arrays are never
cleared or made again (made again, they were zeroed at each trip as far
as the highest slot it touched).

**The engine's side.**
- `SsaTracker` implements the engine's `Tracker` trait. The accessors
  call its hooks:
  - `read_content`, `read` and `write`;
  - `call_begin` and `call_end`;
  - the source's `line_start`, `line_end`, `file` and `line_number`;
  - `name_lookup`, `string_search`, `load`, `store_open`,
    `store_line` and `output`.
- It turns them into `note_read`, `note_write`, `note_effect` and
  `note_store`.
- The plain engine uses a tracker whose hooks are empty
  (`T::VALUES = false`), so a plain run pays nothing.

**The host.** `Host` answers everything outside the engine:
- `read_file` by name and kind (source, TFM, map, font, image);
- `unchanged`, over a list of loads (3.4);
- the clock and file dates;
- output sinks, the page sink and diagnostics;
- font slots shared across builds.

**The fold** (`partex-ssa/src/fold.rs`). Today the top level is a fold
of *steps*: windows (4.3 item 1), or with `PARTEX_SSA_WINDOW=0` calls
from one clean point to the next.
- A *clean point* is main control's top, in outer vertical mode, with
  an empty nest and contribution list, the input at a file level, and
  no output routine active.
- A page that completes defers its fire to the start of the next step.
- Main control stops before a `\shipout`, and the next step begins with
  it (`CleanPoint::Ship`, inside the output routine as a rule): the
  step that ships a page is apart from the routine's expansion before
  it. (The output routine has no frame, in check mode either: a frame
  does not span steps.)
- So does the command after one that read a file whole by name
  (`CleanPoint::Load`): `\pdffilesize`, which LaTeX's `\IfFileExists`
  asks for every `\input` and only tests for being blank. A typed
  character changes the file's size, so the step that read it runs
  again, but the rest of `\input`'s lookup, which sees only the name it
  found, is a step of its own.
- A paragraph's lines read none of the page builder's state, which
  every paragraph before them on the page changes:
  - the page builder after a paragraph's end (§1094) is deferred to the
    next command, and the step that begins there begins with it and
    ends at the next clean point (`CleanPoint::Page`);
  - a paragraph's start (`CleanPoint::ParStart`: the first candidate
    after `new_graf`, its `\everypar` and LaTeX's restart of the
    paragraph done) is a step boundary when it is on the line the
    paragraph's level began on (`CleanPoint::Graf`). The step that
    begins there takes the level's `mode_line` as its own, the line it
    begins on: the one before kept a number that an edit above it can
    move;
  - `new_graf`'s page builder is deferred too in a step that ran 64
    commands or more before the paragraph began: a picture built in
    vertical mode before its `\leavevmode` is then a step that reads
    none of the page's state. The step that begins at that page builder
    takes the paragraph's `mode_line` as its own as well.
  A step that ends in a line is placed by its offset in it, from its
  start or from its end: where the line ends is the input's, so an edit
  earlier or later in the line (a heading's title, read in `\section`'s
  expansion before its page builder) runs the next step again in its
  place instead of making new ones.
- The line breaker seals each line (`seal.rs`). The line box keeps its
  dimensions and a key, and its glue setting and list go to a table
  under the key, a slot of their own (`Fam::Sealed`, versioned by the
  contents). The key is the step's id, the paragraph's count in the
  step and the line's index: the same at each run of the step. Only
  what looks inside a line reads the slot: shipping out, `\unhbox`,
  `\showbox`, a display's width, `\leftmarginkern`. So a word that
  leaves its line's dimensions changes its paragraph's step and the
  ship step only. The page, `\box255` and the output routine before
  its `\shipout` read what they read before.
- Each step has a fixed id and a key: cold keys are 2³² apart, and a
  step spliced in takes a key between its neighbours, a sixty-fourth of
  the gap from the next one's for the first new step after a step run
  again, from its own predecessor's for the later ones of the run. A
  removed step's index entries go with it.
- A step keeps its records, and the slots it read from outside it.
- Each slot keeps its definitions in key order, each naming the step
  and the run that made it, and each with its readers. A step run again
  bumps its run, so its old entries are dead in place.
- `Fold::reaching(slot, key)` is the definition that reaches a point,
  and `readers_between` gives the readers up to the slot's next
  definition.

**A rebuild** (`ssa/rebuild.rs`):
1. The host says which loads changed. Each changed file is diffed into
   hunks (3.5). The steps that read a changed line, a moved line
   number, a changed φ or a changed query answer are dirty.
2. The dirty steps run in key order. Before each one:
   - the slots its last run read, where a later definition holds the
     arrays, take their reaching values;
   - the save stack below its pointer is placed whole, and so is the
     semantic nest (the nest, each level's fields down to its depth,
     the alignment state): a field placed over a nest that was not is a
     state no run makes, which the run can meet before its reads are
     checked. Wherever the nest is put back (a dropped run's writes, a
     step's end, a step removed), the levels' fields down to the depth
     it is put back at go with it: the levels it holds are as they were
     when it was made, and `cur_list` as the run left it;
     for a window (4.3 item 1), the conditionals and `align_state`
     are placed whole too;
   - the page list's length, its tail, and the nodes the step reads are
     placed;
   - the input is set to the previous step's result (3.5), mapped
     through the edits.
3. The step runs, applying the hits of the calls that apply (3.4). If
   it read a slot a later definition holds and that was not placed, its
   run is dropped and made again with that slot placed. A run that
   read one may never reach the step's end (`\end` firing again and
   again over an output routine left active): past its budget, twice
   its last run's commands and 10,000, it stops at the next command
   that finds such a read.
4. Its definitions replace its old ones. Each definition whose value
   changed, or that only one of the two runs made, marks its old readers
   dirty, up to the slot's next definition. Its stores changed make
   dirty the later loads that read the build's own store. A reader a
   definition marked keeps the slot and the version it read (the old
   definition's, or where the old run made none, the one that reached
   the step; the first change of the slot before it knows, as the
   changes come in key order). When its turn comes it is passed over if
   each such slot reaches it at that version again: a definition that
   went from one step to the next, as when a step run again in its
   place ends elsewhere. Any other mark (an edit, a store, an input
   that changed) runs it.
5. A step that ended where its old run did (its result, mapped, equals
   the old one; in a line, at the same offset from its start or from its
   end, where the line ends being the input's) goes on to the next dirty step, and runs the one after
   it again in its place if the input there is not the same. One that ended elsewhere
   (Enter pressed, a paragraph break deleted) runs on, a step at a time,
   until one ends at an old step's start. Each new step's reads are
   predicted by the old step after it, whose text it runs, and by the
   step just run, whose text is like its own where the run reads text
   the old one never read (a table of contents read for the first
   time). The old steps passed over are removed with their definitions,
   and their readers now read the definitions before them.
6. After the steps, the arrays hold every slot's latest definition
   again. That ends a trip.
7. The trip's stores are the next trip's φ (3.7, "Trips, as built"):
   the outside tools whose input changed run, and the next trip runs
   the loads of a changed φ and the readers of a changed tool output,
   by steps 2–6, until a trip ends with no seed or the bound is
   reached. A cold build goes on in the same way after its first trip.
   Then the link writes the files (3.8).

---

## 4. Where the code stands, and the work

### 4.1 What the code has, and what replaces it

The code today (3.15) positions **one mutable engine** at a *step*, a
call from one clean point to the next, and keeps the latest
definitions in flat arrays. Most of its special cases exist only for
that:

| the code today | why it exists | replaced by |
|---|---|---|
| steps between clean points | the only places the engine can be re-entered | any node, by timestamp (3.9) |
| flat arrays holding the latest definitions | fast reads | reads resolved by timestamp in the definition index (3.2) |
| placing a step: its predicted slots set to their reaching values | the arrays hold another point's values | nothing: a read carries its timestamp |
| dropped runs ("read a later definition, run again") | a prediction missed | nothing: a read resolves right the first time |
| the save stack and the semantic nest placed whole | TeX's undo log and its nest must be consistent where they are re-entered | scoped local definitions and group frames (3.2) |
| the page list placed with stand-ins | the engine holds the list, not its nodes | the list as appends read by timestamp |
| the input copied at a step's end and mapped | "where the engine is" was a copy | the input's value, shared (3.5) |
| a store read by the next rebuild | steps close before their loads are known | the φ in the same evaluation (3.7) |
| hits applied for the fonts only | the other routines' results are not values | every node whose results are values |
| records as heap trees, one per call | the first runtime | the trace: chunks, spans, timestamps (3.9) |
| one record per name | the first memo | the memo of *k* (3.9) |
| the dirty steps in key order | one engine, one position | the worklist; many workers (3.10) |

Machine mode, the design before SSA, is also still in the tree
(`PARTEX_MACHINE`, `machine.rs`, `adapter.rs`, `machine_store.rs`,
`partex-incr`), with its sanitizer in the gate. It cut the build into
units at clean points, carried the rest of the state as one hashed
lump, and restored checkpoints. Every unit wrote the lump, so every
unit's exit differed and cutoff failed. It is removed whole, with SSA
in check mode as the gate's second e2e mode and the edit harness moved
into e2e.

### 4.2 The work, in order

Each item's form is written here before its code. Items that depend on
each other are one component: 2–5 (the index, the command as the node,
scoped definitions and group frames, the current list and results as
values) are written whole, beside the old path behind a switch, run
through e2e until identical, and then measured as the assembled design:
the course's one-word edit rebuilt in the same process (its ms, the
nodes re-run, the readers woken, the reads checked), the label test,
and memory. A measurement checks the whole; it never gates a fragment.
Items 1–9 run with one worker; threads come last, over structures built
for them.

1. **The input's value** (3.5): done.
2. **The definition index** replacing the flat arrays and placement
   (3.2): the frontier in the tables, the older definitions per span,
   the running span's local log, spans of the setup and of windows of
   commands. Measured (LOG 2026-09-29): at command grain the index
   takes 6 GB on the course, and memory, not the read path, decides
   the grain.
3. **The command as the node**: the trace with timestamps and spans,
   steps and clean points removed. First measured: bytes per node and
   reads per node on the course.
4. **Scoped local definitions and group frames** (3.2).
5. **The current list as appends**, and results as values for `hpack`,
   `vpack`, `line_break`, the page steps and the output routine.
6. **The φ in the same evaluation** (3.7).
7. **Function values and apply nodes** (3.6).
8. **The memo of *k*.**
9. **The worklist with the viewer's priority**, the page painted from
   its node.
10. **Threads** (3.10): the concurrent index, publish and validate,
    estimates, the relaxed queue.
11. **Machine mode removed**, SSA as the gate's second mode.

After them:
- SyncTeX (source origins on nodes);
- 21-bit tokens;
- XeTeX (C5) and LuaTeX (C6);
- trace compilation.

### 4.3 partex-PhiTeX: windows (2026-10-02)

This repository is partex at 21edd4e merged with two crates from
PhiTeX (ec3d63a): `phitex-syntax`, the lossless syntax tree cut into
paragraphs and reparsed incrementally, and `phitex-ir`, the SSA
program's text form with its parser and checker. The invariant of 3.1
stands. What changes is the grain, what is recorded, and what a
rebuild may cost. This section overrides 4.2 where they differ.

The measurements behind it (LOG 2026-09-29, the course's one-word
edit): the edit re-runs 4,450 commands, about 3.7 ms of plain TeX, but
the rebuild takes 95–133 ms and the link 12–14 ms. Steps between clean
points average 29 K commands in the body and a pgfplots figure is one
step of a million, so a `\label` nothing refers to re-ran 4.82 M
commands. Recording makes 1.68 M records, 1.51 M of them `unsave`'s; a
cold build with recording takes 3.2–3.4× a plain one and peaks at 24 GB.

1. **The window is the unit of re-execution.** A *window* is a step
   (3.15's fold unit) cut by the rule below. Commands are not nodes:
   the trace is per window, and a window re-runs whole.
   `PARTEX_SSA_WINDOW=W` sets the count (default 4,096); `=0` keeps
   3.15's clean points, byte-identical either way.
   - *Where a window may begin.* At a *boundary*: main control's top
     (§1030's `big_switch`, before the next command is fetched), or
     inside that fetch at §380's loop top once the lists an expansion
     pushed are exhausted and popped and a file is on top (3.15's
     candidate), wherever the output routine is not active. The output
     routine is the one call that spans commands, so no call is open
     at a boundary. Boundaries lie inside groups, inside boxes at any
     nest depth, inside alignments, math and conditionals, and with
     token lists on the input. Every local of `main_control` and of
     the scanners is dead there: what is live is the state of 3.13's
     families and the input (3.5). The input's value gains
     `force_eof` (§362: an `\endinput` waits for its line's end).
   - *Where a window ends.* At the first boundary after any of: a
     paragraph's end (`end_graf`, §1096, back in the enclosing
     vertical mode after `line_break`; not a null paragraph, since
     LaTeX's paragraph hooks begin each paragraph with one); a
     deferred fire (no boundary qualifies until its output routine
     ends); a file opened by `\input` or ended (a `\scantokens` pseudo
     file is not one: expl3 rescans tokens in tight loops); or `W`
     commands since the window began. A boundary where a fire is
     pending ends the window too, and the next one begins with the
     fire (3.15). In LaTeX the first boundary after a paragraph's end
     lies inside the `\par` macro, after its `\tex_par:D`, so a window
     is the rest of one `\par`, the next paragraph and its `\par` up to
     the same point, and a word edit re-runs that one window. The
     engine decides
     (`Tex::window_due`), from the window's own count and the events
     noted while it runs, never from a global counter. So a window run
     again from the same place over the same commands ends at the same
     place, and a run that begins where an old window began meets the
     old run again (3.15, step 5). A text paragraph is one window,
     about a thousand commands; a million-command figure is about 250.
     An edit inside a long box shifts the count's cuts after it, which
     then re-run to the box's next paragraph end or file event: the
     window's slack.
   - *Re-entry.* A window runs from the previous window's end (its
     input, mapped through the edits). Before it runs, the slots that
     say where the engine is are placed whole from the definitions
     that reach the window, wherever a later definition holds the
     arrays, as the save stack is (3.15): the current list's fields
     and the nest's levels (`Fam::List` 0–12), the alignment's fields
     (the current alignment and its stack), the conditionals,
     `align_state`, and the save stack. Placed whole, not only as the
     old run predicted, they keep a dropped run from running on a
     structure laid out by a later point (a `\fi` with no conditional,
     a nest popped past its bottom). Every other slot is predicted and
     validated as in 3.15.
   - *The lists.* The current list and the nest's lists are persistent
     sequences (`NodeList`, a `PVec`: an append shares the prefix, a
     copy is O(1)). A window's record keeps the list it leaves as one
     shared value, at the cost of its own appends, and re-entry places
     it with one store: that value is the list "as the appends made so
     far" (3.4). An append still reads the list it appends to, so an
     edit early in a list re-runs the later windows appending to it (a
     cursor, 3.9). A text paragraph is one window, so this costs only
     inside long boxes, whose pack reads the whole list anyway.
   - *Checked.* `PARTEX_SSA_RERUN_CHECK=1` runs every window again
     alone after the cold build, last first, and compares each run's
     end, definitions, stores and effect chunks with its last run's.
     In reverse order the engine's fields outside the families hold
     another window's leftovers, so any difference is state that a
     boundary leaves outside the families and the input.
2. **Records.** A window's record (its reads from outside it, its net
   writes, its effects, where it began and ended), and records for the
   pure typesetting calls that apply (3.4): `line_break`, `hpack`,
   `vpack`, the page steps, `ship_out`, the fonts, deflate. `unsave` is
   not a call: a group's end is writes of the window that runs it.
   Targets: a cold build with recording at most 1.5× plain (1.2× the
   aim), the course's peak memory at most 4 GB.
3. **A rebuild costs what the edit reaches.** Nothing in a rebuild
   walks every step, record, file, chunk or slot; allocators (string
   numbers, hash slots) are not read as values by the steps that
   allocate (3.9, "No chains through a cursor"). Targets on the course:
   the word edit within 16.7 ms, rebuild and link together, warm; a
   `\label` nothing refers to within 50 K commands re-run.
4. **The link costs the changed chunks.** Byte offsets come from an
   offset tree over the chunks' lengths, so a changed chunk costs its
   bytes and `O(log n)`; the cross-reference table is written from it.
5. **The `.aux` loop in one rebuild** (3.7): a store whose lines changed
   wakes its loads in the same rebuild, until every load read what the
   same trip stored (bound: 5 trips, then a report). The oracle is then
   plain partex run to its fixed point. Built as trips (3.7, "Trips, as
   built"): the cold build converges too, BibTeX and makeindex run
   between trips, and `PARTEX_SSA_TRIPS=1` keeps one trip per build.
6. **The front end** (`phitex-syntax`, and `phitex-doc`, new): the
   static document layer from the syntax tree, without running TeX:
   the outline, labels, references, citations, the `\input` and
   `\include` graph, with the guards it assumes (the meanings of
   `\section`, `\label`, `\ref`, ... are LaTeX's), which a build's
   records confirm or break. It never produces output bytes.
7. **The view** (`phitex-ir`): a build printed as a program, a value
   per window: its imports by name (a control sequence's name for an
   eqtb or hash slot) with the window that defined each, its exports,
   and the text it set. `phitex-ir`'s parse and check round-trip it.
   One window's per-call trace is printed on demand by running that
   window again with the full recorder. As built (`ssa/view.rs`, made
   after a build from the fold, the records and the engine's tables, so
   a build that asks for no view pays nothing):
   `PARTEX_SSA_VIEW=FILE` writes it after the cold build, and to
   `FILE.N` after rebuild `N`; `PARTEX_SSA_VIEW_STEP=ID` adds
   `FILE.stepID` (`FILE.N.stepID`), step `ID`'s calls with their reads,
   writes and effects in the trace's form (5.3). The form:

   ```text
   %0 = format          ; step 0: the job's start, (preloaded format=plain 2026.10.2), 631946 definitions: incr:1
   %1 = file incr
   %71 = window(incr:15-18=%1, \catcode92=%0, ..., \section=%66, \words=%64, \toc=%4, align_state=%68, page.total=%61, ...; page[56], ..., save.level, list.list) ; step 68: incr:15-18 "Opening Lorem ipsum dolor sit amet,"
   %76 = window(..., page[0]=%7, ..., \count1=%71, output=%0, ...; ...) ; step 73: ships [3] "Lorem ipsum dolor sit amet, consectetur"
   ```

   - The job's start, the step that loaded the format and read the
     first command, is the constant `%0`: the format's definitions are
     its, and so is an address no window defined (the engine's initial
     state, a lookup by name, a line number read). A file is a constant
     `file NAME`.
   - Then a window per live step, in program order: an operation
     `window(imports; exports)` (`phitex-ir`'s `Def::Op`). An import is
     an operand `name=%n` (`Operand::Named`): an address the step read
     from outside itself (`Fold::steps`' reads), and the window whose
     definition reached it (`Fold::reaching`), or `%0`. The source it
     read is imports of the file's constant, by runs of lines
     `file:first-last`: the line it began on if the step before left
     some of it, then the lines it read (`Steps`' line reads, the ones a
     rebuild seeds from). Its exports are the addresses its records
     wrote.
   - Names: a control sequence's meaning by its name in the hash
     (`\section`; one of the frozen ones, a font's identifier and
     pdfTeX's primitive copies with a prefix, since another slot has the
     same name); the registers and codes as TeX writes them (`\count12`,
     `\dimen3`, `\toks0`, `\box255`, `\catcode92`, `\textfont1`); a
     parameter without its escape (`baselineskip`, `everypar`, `hsize`:
     `\hsize` is the control sequence, another address); the other
     families by a prefix and a field: `text:\foo` and `next:\foo` (the
     hash), `lookup:\foo`, `font:cmr10.fontdimen`, `page.contents`,
     `page[3]` (the page's nodes), `list.mode`, `nest`, `save[4]`,
     `cond`, `botmark`, `str_ptr`, `string:2031`, `pdf.objs`, `write:3`.
   - Its comment: the step's id (and its run, once a rebuild ran it
     again), its runs of lines, the pages it shipped (by the counts the
     log shows, `[3]`), and an excerpt of text: of the page it shipped
     (the page's nodes it read), else of the nodes it put on the page,
     else of what it added to its list, else of the source it began on.
   - `Program::check` verifies that it is SSA and that each import from
     a window names one of that window's exports; `to_text` and `parse`
     round-trip it (`partex-core`'s `tests/view.rs`, `phitex-ir`'s tests).
   - The per-call trace is today the step's records as they are kept,
     their addresses named as in the view (each call marked `new`: how
     a call was found is kept per trip, not per record). Once records
     are kept only for windows and the pure typesetting calls (item 2),
     it needs the window run again with the full recorder, which is not
     built.
   - Diffing two views shows what a rebuild did: after `edits.tex`'s
     first edit, the paragraph's window, the fire that ships its page
     and the job's end are `(run 2)`, nothing else changes. It also
     shows where a rebuilt graph differs from a cold build's: fonts and
     hyphenation are not placed (3.15), so a paragraph run again reads
     the interword glue and the packed patterns the cold build made
     later, defines them no more, and the next reader's import moves to
     the window before (`font:cmr10.glue=%12` becomes `=%5`).
8. **The gate.** `cargo xtask check` adds `ssa-edits`: every
   incremental e2e case run in SSA mode, one process, each stage
   byte-identical to plain partex on the same sources (logs masked).
   Machine mode stays until SSA passes everything; removing it is last.
   The harness is `scripts/ssa-edits` (in the sandbox):
   - *The cases*: e2e's incremental cases built by `-watch`, read from
     `INCREMENTAL` in `xtask/src/e2e.rs`; e2e's `machine_edits`, with
     a seventh edit whose glyphs the fonts already have, so the job's
     end runs again with every font call a hit; `machine_edits_dvi`;
     and `label`, a `\label` nothing refers to, then two rebuilds with
     no edit. The LaTeX cases start, as e2e's do, from the `.aux` and
     `.toc` of two plain runs of the original text.
   - *The SSA side*: one process per case, its edits as its
     `PARTEX_SSA_REBUILD` lines. Each line saves what the build before
     wrote, then makes the edit as e2e's `edit_file` does: the new text,
     its modification time fixed, renamed over the file.
   - *The oracle*: plain partex (no `PARTEX_*` variable), once per
     stage, in a directory of its own where the files of the runs
     before stay, as the SSA process's do. `--fixpoint` runs each stage
     to its fixed point instead, the oracle for item 5: again while a
     file it wrote, other than its log, DVI and PDF, changed, at most 5
     runs (e2e's `fixpoint`).
   - *The comparison*: every file the job wrote, at every stage, byte
     for byte. Logs are compared without their first line and with
     `mask.rs`'s statistics masked.
   - *The report*: per stage, the rebuild's and the link's ms, the
     steps run and the commands.

   `cargo xtask ssa-edits` runs it on the release binary, and `cargo
   xtask check` runs it beside e2e. The course's numbers come from
   `bench/ssa-course.sh`: one process builds the course cold, then
   rebuilds it after N warm `word` edits and their reverts, then after
   each edit of `bench/edits/course.txt` (or those `--edits` names) and
   after that edit's revert. A rebuild over `--rebuild-timeout` seconds
   ends the run, and the numbers so far are kept. It runs through
   `scripts/heavy`, or in an accl job. Per rebuild it records the counts
   that do not depend on the machine's load: steps, commands, reads
   checked, readers marked and records made. It also records the user
   instructions of each phase (`perf stat -e instructions:u`, attached
   by the rebuild's line), the ms, the peak RSS, and whether the final
   outputs, every edit reverted, are the cold build's.

**The job's end** (from the profile of the course's word edit, LOG
2026-10-02). Each ship writes the glyphs it used, by font (`glyphs:N`,
an append), and the job's end reads every ship's row to make each
font's union: the glyphs its subset holds. A word edit changes its
page's glyphs but almost never a font's union, yet the page's row made
the end dirty, and the end ran again for nothing: the object streams,
the cross-reference stream, the name tree and the outlines, 22% of the
warm word edit. Now the end tells the recorder the version of the
union it made (`Tracker::glyphs_united`). A rebuild that finds a ship's
glyph row changed makes the union again from the ships' latest rows
(`glyph_union_now`, an OR of small bitsets) and marks the end only if
the version differs: an early cutoff on the one value through which the
end depends on the ships' glyphs. The union is made again after each
step closes, so a later ship that changes it again is seen. A ship that
changes it and a later one that changes it back can cost a run of the
end that was not needed, never miss one that was. `scripts/ssa-edits`'
`machine_edits` covers both sides: digits the fonts already have (the
end does not run), then letters no page had, and their removal (it runs
for each).

The work, one agent each, on branches `np/<name>` in worktrees under
`~/code/tmp/`: `windows` (1), `rebuild-cost` (3), `records` (2),
`link` (4), `aux-loop` (5), `front` (6), `view` (7), `gate` (8).

### 4.4 Source and PDF: glyph origins (2026-10-03)

An editor beside the PDF maps a click on a glyph to the source bytes it
came from, and back. Its renderer draws one glyph per character code a
text-showing operator shows, so the origins are defined in that order,
and both sides line up by construction.

**The API** (`partex_core::srcmap`, its module documentation the
reference). `Tex::set_origins(true)` before the cold build; after each
build or rebuild, once linked, `Tex::origins(page) -> Vec<GlyphOrigin>`
(page 0-based in shipping order, glyph `i` the page's `i`-th code) and
`Tex::origin_files()` (file id to the name the job asked `read_file`
for). `GlyphOrigin { file, start, end, synthesized }`: bytes
`start..end` of the file's current text. The CLI's `PARTEX_ORIGINS=1`
writes `<job>.origins.jsonl` beside the PDF after each build or rebuild:
`{"files":[...]}`, then a line per page, `{"page":N,"glyphs":[[file,
start,end,synth],...]}` (N from 1).

**The order.** Every code `Tj`, `TJ`, `'` and `"` show, in content
stream order; a form's codes at each `Do` that draws it, again at each
use, recursively; numbers in a `TJ` array are kerns. A code is a byte in
a simple font, its `CMap`'s bytes in a Type 0 font (an included page's:
`Identity-H`/`-V` two, an embedded `CMap` by its codespace ranges, any
other predefined `CMap` two). Codes no glyph of TeX's made have an entry
with no source (`file = u32::MAX`): literal text, pdfTeX's fake and
interword spaces, an included page's text (its forms too).
`partex_engine::pdftext` is the walk; the `glyphs` e2e job checks every
page's count against it.

**Where a glyph comes from.** A character read from a file: its bytes
(`^^` forms whole). An argument's tokens keep their bytes: a macro's
argument list carries an origin per token (a handle in its
`TokenList`), through parameter substitution, `back_input` and
`\expandafter`. A character from a macro body, `\the`, `\number`, a
counter or `\char` is *synthesized*, its range the call's in the
innermost file: from the start of the last command taken from that file
(an expandable one expanded there, or one main control executed) to the
file's read position (each file level's `call`, kept and mapped through
edits with the input state). A ligature covers its characters; a break's
hyphen is synthesized with the range of the character before it; a box
used again shows the same origins again.

**The side channel.** Origins never enter equality, hashing or any
version, so a build is the same with them on or off, and the off path
reads nothing. An `Org` is 64 bits (a data, i.e. one version of a
file's contents, a start and a length, or an index into long ranges);
an append-only `OrgTable` holds them, and nodes hold `u32` handles: a
glyph run's characters are entries `h..h+n` (copy-on-write when a run
grows away from the table's end), a ligature one entry, a token list
one per token. `Glyphs` gave a byte of its inline characters for the
handle (15 a run, not 16), so nodes stay 24 bytes. The ship walk tags
each drawn item with its handle; the encoder emits a stream's glyph
list (handles, form markers at `Do`, runs of no-source entries) as it
writes the stream: kept per page and form in a plain run, and as the
ship step's effect (`Effect::Origins`, ignored by the link) in SSA mode,
so a reused step keeps its list and a re-run one makes a new one.

**Incremental.** A reused step's origins name the datas of the text it
read. Each rebuild's edits (old and new data, the changed lines' byte
ranges) become maps from a data to its successor, exact outside the
changed lines and inside them where a line kept its start or end
(common prefix and suffix); an origin is mapped through the chain when
asked for and written back, never made again. The `glyphs` job edits
twice (a comment line inserted before a paragraph, then a paragraph) and
checks each rebuild's side file and PDF are a cold build's of its text.

**The outline** (`phitex_doc::Outline`, Overleaf's file outline). The
static layer (4.3.6) knows each heading before any build:
`Project::outline()` gives `Entry { level, title, number, file, start,
end, title_start, title_end, line, page, x, y }`, the command's bytes
and its title's in the file (none for a heading a document macro makes),
LaTeX's number as the layer counts it, and `Outline::section_at(file,
offset)`, the innermost heading at or before a byte in reading order (in
a file read by `\input`, before its first heading: the section it is
read in). `Outline::place(files, origins, position)` adds where the PDF
shows each heading: the first glyph, in page order, not synthesized,
whose origin lies inside the title's bytes (the heading itself; a
running head or the table of contents shows the title from a mark or
the `.toc`, not from its bytes), at that glyph's origin on the page in
points from the bottom left, which `partex_engine::pdftext` computes by
walking the page's content stream as a viewer does (the text and
transformation matrices, `/Widths`, a form's `/Matrix`). The entries
follow edits (`Project::edit`, then `outline()` again) and rebuilds
(`place` again with the new origins). `partex outline FILE.tex --json`
places them from the side file a build wrote beside its PDF
(`PARTEX_ORIGINS=1`), or `--origins PATH`; without one, `page`, `x` and
`y` are `null`.

**Costs** (the PGF subset, `bench/inputs/pgfsub.tex`: four chapters of
the manual, 115 pages, a plain run on settled auxiliary files, against
the branch point built the same way; `perf stat -e instructions:u`).
Off: 146.90 G instructions against 145.42 G, +1.0% (the checks on the
token path; wall time within the machine's noise, 29.7 s against
29.6 s at best), the PDF the same bytes. On: 159.90 G, +10.0%, and the
peak RSS 132 MB to 187–205 MB for 237,566 glyphs, most of it the
origins of argument tokens (every token of every argument read from a
file has an entry); the side file is 4.7 MB. Making argument origins
compact (a run per argument, its tokens' bytes found again by reading
the source) is left to do.

### 4.5 SyncTeX (2026-10-03)

`-synctex=N` (or `-synctex N`) writes `<job>.synctex.gz` as pdfTeX
1.40.29 does, byte for byte: `-synctex=-N` the uncompressed
`<job>.synctex`, `N` with bit 2 the gzipped text under the name without
`.gz`, bit 4 the forms' records, bit 8 the compressed `=` vertical
positions, `0` off for good (with pdfTeX's warning if the document sets
`\synctex`). `Tex::set_synctex(option)` is the library's switch; the
CLI reads the option as web2c does (`strtol(optarg, NULL, 0)`).

**The controller** (`partex_core::synctex`) is `synctex.c` ported: its
context (the last node, the kern recorder, tag, line, point,
`total_length`, `lastv`, `form_depth`, the flags), its record functions
with their `SYNCTEX_IGNORE` variants, anchors, counts and quirks (a
kern's record at the point of the last node that moved it; glue e-TeX's
`hlist_out` turned into a kern recorded as one; `\synctex` read at each
call), `synctex_dot_open`'s file names (`Input:N:` the absolute name,
the working directory before a relative one: the host's
`synctex_name`), `synctexterminate`'s postamble, `SyncTeX written on
NAME.` on the terminal (not in batch mode) between `Output written on`
and `Transcript written on`, and the files of earlier runs removed.
gzip is zlib's `gzopen(name, "wb")`: the host's deflate at level 6 (the
same stream whatever the chunks `gzprintf` gave it) between zlib's gzip
header (no name, time 0, OS 3) and its CRC and length.

**Events.** The engine tells the controller what pdfTeX's hooks tell
it, where they tell it: a file opened (`synctex_start_input`, before its
first line; the tag counter is a scalar row, each file level's tag in
its `AlphaFile`), a sheet or form begun and ended around "Ship box p
out", and the PDF walk's boxes (`[`, `(` before a vlist's height is
taken, `]`, `)`), empty boxes (`v`, `h`; a vlist's between its height
and depth), the end of a run of characters (`x`: a ligature ends one
and begins the next, which goes on over the characters after it), kerns
(`k`, the recorder), glue and rules moved past (`g`, `r` with
`rule_wd`, `rule_ht`, `rule_dp`), math nodes (`$`) and form references
(`f`). A plain run feeds each event to the controller at once.

**Places.** pdfTeX gives every node of `medium_node_size` or more the
tag of the file being read and TeX's `line` in `get_node`. Here boxes,
rules, glue, kerns, math nodes, leaders and unset nodes carry a handle
(`origin::Side`, outside their value) into a table of places. A node
gets its place where it enters a list, a box register or a box being
made (`tail_append`, `box_end`, the line breaker's lines, `hpack` and
`vpack`'s results, an alignment's rows, `set_box_reg`): the engine's
algorithms make nodes with none, and none of them reads input, so the
place then is where they were made. What pdfTeX changes in place keeps
its place: the glue at a break made `\rightskip`, a kern or math node
at a break emptied, an unset node made a box, a rule an alignment
stretches, a kern font expansion resizes. A copy (`\copy`, `\unhcopy`,
`\unvcopy`) keeps its nodes' places but its rules', which pdfTeX does
not copy; a kern hyphenation makes again has none (pdfTeX clears its
tag, "it is too late").

**SSA mode.** Events are the steps' effects (`Effect::Synctex`), and the
build's file is rendered after each build or rebuild is linked
(`Tex::synctex_write`): every step's events in order, through a new
controller, each place's line moved through the edits since it was made
as a rebuild moves a step's reads (`Edit::pos`: a position where bytes
were inserted stays before them). The final step prints the message
from the same render. A node made again by a step that runs again must
not hide behind an equal value: in SSA mode each place is a handle of
its own in `Side::HASHED`'s range, which is hashed, so what holds a node
made again is another version and is made again too (the rest of its
paragraph and page; an edit that moves no line costs the steps of the
page it is on). With `SyncTeX`, no step is taken from another's record.

**A document's own `\synctex`.** With no `-synctex`, the controller is
made when the document first sets `\synctex` nonzero (`assign_int`),
where pdfTeX's first acts: files are counted from the job's start
whatever the setting (`synctex_start_input`; the counter a scalar row),
the first file's name is kept for `Input:1`, and a page shipped before
leaves it off, with pdfTeX's warning at the next sheet. In SSA mode the
controller's flags are a scalar row, so the steps print its warnings
where pdfTeX prints them. pdfTeX gives a node its place in `get_node`
whatever `\synctex` is; here a node made before `\synctex` is set gets
one where it next enters a list, box or register.

**Not done.** The DVI mode (no file is written). The DVI writer
resolves positions in its backend, from the page IR; SyncTeX would need
`build_list`, `node_item`, `leaders_items` and `reflect.rs`'s walk to
track `cur_h` and `cur_v` as tex.web's `hlist_out` and `vlist_out` do
(leader boxes repeated, TeX--XeT's reversed segments) and feed the PDF
walk's events, a sheet begun before "Completed box being shipped out"
and ended after the memory statistics, and `Output:dvi` with offsets of
1in (4736287sp) while pdfTeX's `pdf_output_value` is not positive. A
session or persisted build with `SyncTeX` (saving one is refused):
nodes' `Side` handles in the node codec, the places table, the
controller's state and the steps' events would all have to be saved.
The cost off and on is measured by `scripts/accl/tasks/synctex-ab.sh`
(LOG 2026-10-03).

### 4.6 Display lists: each page's drawing without the PDF (2026-10-03)

A renderer beside the editor (the PhiTeX Overleaf extension) draws pages
itself; it should not need the PDF linked and parsed for that. A page's
display list is what its content stream draws, as items in the
stream's order: `Glyph { font, code, x, y }`, `GlyphMatrix([a b c d])`
(where text is drawn turned, scaled or expanded), `Rule { x, y, w, h,
stroke, ctm }`, `Literal { bytes, mode, ctm, codes }` (a
`\pdfliteral`'s, a color stack's, `\pdfsave`/`\pdfrestore`/
`\pdfsetmatrix`'s text, run with `ctm`) and `XObject { kind, id,
matrix }`. Coordinates are points in the page's default user space (the
media box `[0 0 w h]` pdfTeX writes; `\pdfpageattr`'s if it gives one).

**Made from the stream's bytes, not at ship time.** The list is the walk
of the content stream as written (`partex_engine::pdftext::list`, the
walk glyph origins count with), the fonts' widths those of the PDF's
`/Widths`: each glyph is where a reader of the PDF puts it, bit for bit,
and the walk is the one a check runs over the linked PDF. A list made by
the encoder from pdfTeX's own positions (`cur_h`, `pdf_delta_h`) would
differ from the PDF's by its rounding. The ship keeps only what the walk
cannot see in the bytes: the stream (uncompressed), where each literal's
text is in it and its mode (`emit_literal` notes it), and the resources
(each `/F<n>`'s PDF font with its TFM's widths and map entry, each
`/Fm<n>`'s and `/Im<n>`'s object, a PDF page image's file): `Shipped`,
`displist.rs`. Without a recorder it is kept by the engine; with one it
is the ship step's effect (`Effect::Display`, ignored by the link), so a
reused step keeps its stream and a rebuild needs no link. Lists are
walked when asked for, once per stream. Off (the default) nothing is
kept and the output is the same bytes either way.

**The API** (`partex_core::displist`): `Tex::set_display_lists(true)`
before the cold build; then `display_pages`, `display_hashes` (a hash
per page of its stream, box, fonts, forms and images: redraw the pages
whose hash a rebuild changed), `display_list(page)`, `display_form(id)`,
`display_glyphs(page)` (the codes in glyph origins' order, forms walked
where drawn, a literal's or an included page's `None`),
`display_font(id)` (PS name, size, the whole Type 1 file the map names,
its encoding, the TFM's widths, slant and extend) and
`display_image(id)`. `PARTEX_DISPLAY=1` writes `<job>.display.jsonl`.
The `display` e2e job and the `glyphs` job check every page's list
against `pdftext` over the PDF, and SSA rebuilds against cold builds.

---

## 5. Performance, observability and the text form

### 5.1 Numbers now

The 295-page course (`ch15.tex`, "expensive" → "dear"), release
binary, sandboxed (LOG 2026-09-29):

| | time |
|---|---|
| cold, pdfTeX | 57.6 s |
| cold, plain partex | 55.1 s |
| cold, SSA recording | 178–188 s |
| one-word edit, first rebuild | 164.0 ms (rebuild 150.1, link 13.9) |
| one-word edit, 60 warm rebuilds | mean 116.1 ms, median 106.3 ms (rebuild alone 94.9 ms) |

The word edit re-runs 6 steps and 4,450 commands. It changes 44
definitions, marks 32 readers and checks 6,599 reads. A `\label`
nothing refers to re-runs 4.82 M commands, because re-entry is still
by step (4.1). The target is 16.7 ms.

**Expected with 4.2's work done** (estimates from the costs measured:
about 0.8 µs a command plain, about three times that recording, a
tenth to a fifth of a microsecond per hit; to be replaced by
measurements):

| edit | page on screen | converged |
|---|---|---|
| a space or a comment | < 1 ms | < 1 ms |
| a word, heights kept | 3–10 ms | 10–20 ms |
| Enter, a paragraph split | 3–10 ms | 10–20 ms |
| a reflow over later pages | 5–15 ms | 20–100 ms, 1–5 ms a page |
| a `\label` nothing refers to | 1–5 ms | a few ms |
| a referenced label renumbered | 5–15 ms | about 5 ms per `\ref` |
| a footnote added | 5–15 ms | 20–100 ms |
| a preamble `\def` changed | seconds | up to a cold build |

The unknown is LaTeX's output routine, perhaps 5–30k commands a page:
with macro calls as nodes most of it should hit; if it does not, a word
edit lands nearer 15–30 ms.

### 5.2 Observability

- **Spans** on phases behind the `trace` feature, exported as Perfetto
  JSON. `partex trace` writes them.
- **The TeX-level profiler**: per control sequence, file, line and
  page.
- **The progress board** (`partex_core::progress`, 2.5): the commands
  run, the pages shipped and the file and line being read, as relaxed
  atomics the engine posts every 1024 commands, for the terminal's live
  line.
- **Native profiling**: `perf` and callgrind.
- **The event log** (`PARTEX_EVENTS=DIR`, `partex-cli/src/eventlog.rs`):
  a tracker that observes a plain build and writes a table of every
  definition of eqtb and the hash (the command that made it, the next
  one's, its last reader, its readers, its bits, its group level) and
  of every command (file, line, group level, outer clean point), as
  raw columns. `scripts/events.py DIR summary|setup|grain` answers
  questions over it offline, such as the grain table of 3.2, with no
  change to the engine. The course's log is 2.4 GB.
- **The SSA report**, per build and per rebuild:
  - calls by routine, with hits, misses and records;
  - the longest chains of consecutive definitions of one address
    (3.9), and with threads the frontier's width, the re-runs
    validation caused and the estimates hit (3.10);
  - steps and commands re-run;
  - definitions changed, readers marked, reads checked;
  - hits applied, with the commands they stand for;
  - the link's chunks.
- Benchmarks are recorded as JSON in `bench/results/`. The edit harness
  is `bench/edits.sh`; SSA mode's is `scripts/ssa-edits` (4.3 item 8),
  which writes each stage's numbers to `target/ssa-edits/results.json`.
  The course's edits as the rebuilds of one SSA process are
  `bench/ssa-course.sh`, which writes `bench/results/<commit>-ssa-course-<host>.json`.
  On a loaded machine, edits are compared by counts, instructions and
  peak RSS, not ms.

### 5.3 The text form

Every IR has one canonical, human-readable text form that round-trips:
the page IR, node lists, records and traces. The form is:
- canonical and diffable: program order, sorted keys, and no
  addresses, table positions or timestamps;
- names, not numbers: control sequences by name, fonts by identity,
  addresses by key;
- loadable: a dumped trace can be run.

The trace (`partex-ssa`'s `Trace::to_text` and `parse`) is shaped like
LLVM IR (the shape, illustrated):

```text
%v7 = call @hpack(%v3, %v5)        ; name=#a41f…
  read  eqtb:29992 = %v2           ; #19c0…
  write HPACK_RESULT = %v7
  effect log "Overfull \\hbox (1.2pt too wide) in paragraph at lines 42--43"
%v9 = phi [%v<previous build>, %v<trip k−1>] @stream:aux
```

On a rebuild, each call is marked `hit`, `new` or
`miss=@<first read that differed>`.

The build as a program (4.3 item 7) is `phitex-ir`'s form, `PhiTeX`'s
SSA text: a value per line, `%n = def ; shows`, in program order,
where `def` is a constant, an application `%c(operands)`, a φ, a μ, an
environment, a pending text, or an operation `op(operands; names)`
that defines `names` (a window). An operand is a value `%n`, a use of
it `%n:"text"`, source text, or a name imported from a value
`name=%n`. A name is written as it is unless a character in it would
end it, then quoted (`"\csname a b\endcsname"=%0`). Parsed, the text
gives the same values; printed again, the same text.

---

## 6. Risks and open questions

- **Floating point.** web2c's `glue_ratio` artefacts are observable in
  logs, so partex matches web2c's arithmetic.
- **Exact deflate and shaping.** Byte-exact PDF needs zlib's exact
  deflate. XeTeX needs HarfBuzz-exact shaping (a pure-Rust shaper
  pinned to its version).
- **LuaTeX's surface** (callbacks, node and token libraries) is large.
  At first, every Lua call makes its node unreusable.
- **biber** is a large Perl program: sandboxed as a call, not ported.
- **Recording's cold cost** (3.2–3.3× a plain run today) is measured
  and kept switchable. The frame target is per edit, but a cold build
  must not stay three times slower.
- **Oscillation.** A document with two fixed points reaches the one its
  previous build's stores lead to, as latexmk does from the files on
  disk.
- **Serial fraction.** The page builder and output routines bound the
  parallel speedup (Amdahl), so it is measured per document before any
  claim. Measured on the course (3.10, "Measured"): without speculation
  the steps are one chain (1.00x); speculated from the last build, the
  bound is the largest step's, 12.4x.
- **Read-set sizes** for expl3 documents are to be measured on the
  latex3 corpus.

---

## A. Old section numbers

Comments written before 2026-09-29 cite the old document's numbers.
They map here:

| old | here |
|---|---|
| 7.17 (its first sentence) | 3.1 |
| 7.17.1 values, definitions, versions for slots | 3.2 |
| 7.17.2 calls, records, names, evaluation, the grain | 3.3 |
| 7.17.3 change propagation, appends, the page list, placing a step | 3.4, 3.15 |
| 7.17.3 "the input is the step's result", edits by lines, data | 3.5 |
| 7.17.3 "a line number read is kept by position" | 3.5 (reads relative to the call) |
| 7.17.3 "allocated numbers are not positioned" | 3.2 |
| 7.17.3 "output is linked", "the link after a rebuild" | 3.8 |
| 7.17.3 "hits applied inside a step", the fonts as calls | 3.4, 3.8 |
| 7.17.3 "a load reads the store", "a stream's version is its file" | 3.7 |
| 7.17.3 "a query is asked again", file checks by stamp | 3.4 |
| 7.17.4 the source and the tokens | 3.5 |
| 7.17.5 stores, loads and the cycle | 3.7 |
| 7.17.6 speculation and parallelism | 3.10 |
| 7.17.7 records, memory, the collector | 3.11 |
| 7.17.8 exactness and gates | 1.4, 3.14 |
| 7.17.9 the fold of steps | 3.15 |
| 7.17.10 the work (the runtime, state as values, routines as calls) | 3.15, 4 |
| 7.17.11 optimizations on the trace | 3.12 |
| 7.17.12 TeX's state, writer scopes, streams, check mode | 3.13, 3.8 |
| 7.17.13 evaluation as one flat graph | 3.6, 3.9, 4.2 |
| 7.16.1 clean points, the deferred fire | 3.15 |
| 7.16, 7.16.2–7.16.10 machine mode | 4.1 (being removed) |
| 7.0–7.15 earlier designs (regions, traces, rounds, holes, sessions) | superseded by chapter 3 |
| 5.3 state representation | 2.4 |
| 7.6 effects and the link | 3.8 |
