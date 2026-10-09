# partex — design

partex is a TeX engine in Rust. It produces exactly the reference
engines' output, and it is built as an incremental, parallel document
compiler rather than as a translation of `tex.web`. The goal it is
measured by: a keystroke to a repainted page in one frame, 16.7 ms, on a
real 300-page LaTeX document, with every output byte-identical to
pdfTeX's.

This file states the design as it is, completely enough to implement
it again from `tex.web`, `etex.ch` and `pdftex.web`. It says nothing
about how the design was reached; `AGENTS.md` holds the working rules.

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
7. The φ core
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
| C5 | XeTeX | XDV byte-identical to `xetex -no-pdf`; PDF to piped `xelatex` | in progress (4.7) |
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
- The CI (`docs/ci.md`, `scripts/accl/accl ci`) adds, on the accl
  cluster, the manual corpus (TeX Live's own documentation sources,
  TeX Live's engine to a settled fixpoint against partex's plain,
  `build`, machine and SSA modes, the PDF and the files read back byte
  for byte), the l3build suites, escape tests, and measured runs against
  TeX Live on a pinned node with their history; its scoreboard fails on
  a new failure, a regression against the last main run, or a perf
  regression.

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
  phitex-diff    latexdiff, natively: two versions in, the marked-up
                 document and the change list out (4.10)
  phitex-git     past versions read from git (gitoxide), for phitex-diff
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
  - An input level that reads a list holds it, with two exceptions that
    count no reference: a level that backs up one token (§325, the
    commonest level of all) holds the token itself, and a parameter
    level (§359) reads its parameter in place on the parameter stack,
    which outlives it (`InStateRecord::holds_token`, `holds_param`).
    Both are read through `InStateRecord::tokens_in`, and a state hash
    sees them as the lists they stand for.
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
  accessor from the content written (3.2). An eqtb entry's is only
  marked at the write and made from the content when it is first
  wanted (a read from outside the call, a step's definitions): the
  content then is the content written, since every change of it comes
  through the accessor (check mode makes the version at the write, and
  tests that).
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
  with `phitex --compat=NAME`, PhiTeX speaks that engine's web2c
  command line. Bare `phitex` is the modern command line:
  - `build` runs to the fixpoint, with BibTeX and makeindex in process,
    and writes `<job>.synctex.gz` (4.5; `--no-synctex` not);
  - `watch` is the watch loop (`--ssa`: on the SSA runtime, 4.9);
  - `check` makes one pass without writing;
  - `why` says what the last build did;
  - `trace` writes a Perfetto timeline;
  - `clean` removes what the last build wrote and saved (`--all`:
    everything saved, for every document).

  It is configured by `phitex.toml` and `% !TEX` comments; the engine
  is pdfLaTeX unless they or `--engine` say `xelatex` (XeTeX: its format
  made in the cache as fmtutil makes it, `xetex -ini -etex
  xelatex.ini`; xdvipdfmx in process; fonts found through fontconfig's
  list as `xelatex` finds them, `FONTCONFIG_FILE` honoured).
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
  - *The passes* are a line each, with few numbers: `Pass 2  316
    pages  1:14  (paper.toc changed)`, the files the pass before
    changed being why it ran (a pass that ran part of the job says the
    pages it made again). `-v` adds the commands run (again) and why.
    A first pass that ran nothing says `Restored the saved build
    (nothing changed)`.
  - *The result* is one line: `Finished paper.pdf · 12 pages · 1.3 s`,
    the file a hyperlink (OSC 8) where the terminal has them; `-v` adds
    the size and the passes.
  - *`phitex watch`* logs a line per rebuild: `18:51:14  ch05.tex:5
    ✓ 228 ms · 2 warnings (w)`, the time, the file and line that
    changed, the result, its time, and the errors and warnings counted
    when they are not as many as before; each new error on a line of
    its own, as TeX's `-file-line-error` puts it (`ch05.tex:7:
    Undefined control sequence \foo`). `w` shows them all, rustc-style.
    A save while a rebuild runs supersedes it (4.1, "A save during a
    rebuild"): `18:52:01  superseded by ch05.tex:31`, and the one line
    when the newest save's build is in.
    Below the log a status line, `── watching paper.tex → paper.pdf ·
    ? help`, is drawn in place (`-v`: the last builds' times as a
    sparkline). The cursor waits at the start of the status line, not
    at its end, so that clearing it before a line is printed above
    takes all of it even after it wrapped (a terminal made narrower
    reflows it); it is cut to the terminal's width, looked at once a
    second. On a terminal in the foreground the watch reads keys from
    its start, the first build included (`?` help, `r` rebuild, `o`
    open, `w` errors and warnings, `c` clear, `q` quit), with echo,
    line editing and the signal keys off: a key pressed while a build
    runs is not echoed into the live line, and keys pressed meanwhile
    are answered once, not once a press (help is not printed again
    while it is the last thing shown; the viewer is not started again
    while it runs). Ctrl-C stops the watch after saving its build
    (twice: at once), Ctrl-Z suspends it. A watchdog `sh`, waiting on a
    pipe, restores the terminal's modes and cursor if PhiTeX dies
    first. PhiTeX takes no dependency for this: the terminal's size and
    modes are `stty`'s.
  - *`phitex clean`* removes the job's outputs, its saved session, and
    its saved build and no-op record in the store (the packs only it
    named go with the store's collection), so the next build starts
    over; it says how much the store keeps for other documents.
    `phitex clean --all` removes the whole store and every saved
    session and record (the formats stay).
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
  - The engine's expansion-scoped flags are part of the value too:
    `\csname`'s and `\ifcsname`'s in-csname flag, `scanner_status` and
    `warning_index`, the expansion depth, `no_new_control_sequence`,
    `name_in_progress`, `OK_to_interrupt`, `deletions_allowed` and
    `set_box_allowed`. A run stopped in the middle of an expansion
    (a dropped run) leaves them set; re-entering at the step writes
    back what they were there. Without that, a run dropped inside
    `\csname` made the next run start "in a csname", and LaTeX's
    active `~` (`\ifincsname`) typeset a literal `~`.
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
- **Outside tools are nodes** (3.16). BibTeX reads the `.aux` stream's
  `\bibdata`, `\bibstyle` and `\citation` lines, the `.bst` and the
  `.bib` files. makeindex reads the `.idx` stream and its style. biber
  (planned, sandboxed) reads the `.bcf` stream and the `.bib` files.
  Each tool's output is a stream its reader loads, defined by the tool,
  and a tool's node runs if and only if a version it read changed.
  Which tool reads which stream is fixed in partex.
- **Files are a view.** The build holds the streams and writes files
  for outside tools, never reading its own writes back. In an SSA build
  (effects on) no output file on the host is opened, emptied or written
  while a build or rebuild runs: `\openout` takes a handle the host has
  not opened (`Host::open_write_later`), `\write` bytes are effects and
  the stores' lines, and only the link writes files. So a rebuild that
  stops halfway (cancelled, past its deadline) leaves every file as the
  last link wrote it, never a step's truncated `.aux`.
  - *Only a trip that ended publishes its streams.* What a later trip's
    loads read of a name the job writes (the φ), what the tools read and
    what the link writes is what the last trip that reached the job's end
    stored: a trip stopped halfway (deadline, cancel, budget) publishes
    nothing, whether its work is resumed or dropped (below, "A rebuild
    stopped"); and, with `SsaTracker::keep_complete`, a trip that ended
    fatally publishes nothing either (below, "A trip that ended
    fatally").
  - *A load of a name the job writes* is served from the stores in every
    trip, the cold build's first included (`Steps::serve_cold`): the lines
    since the last open before it, or, with no open before it, the φ (in a
    process's first trip, the host's file). It is named as the host's
    lookup names a file where the job writes it (`Host::written_name`:
    `./name`, or the output directory's).
  - *A rebuild's φ of a name the job writes* is the build's own value, the
    one its link wrote (the stores where the last trip left them), unless
    the host says the file was edited since that write
    (`Host::output_edited`: the native host keeps the stamp of each file
    its link wrote): a user's edit of an `.aux` is an edit as before; a
    file another process truncated is read as an edit too, and the build
    converges from it.
  - *A producer outside the steps* (a tool between trips, a node that
    makes a stream) defines a named stream's bytes with
    `ssa::define_stream`: a stored name no step opens, whose φ is the
    value given, read by loads as a job-written stream is, and written by
    the link (`ssa::produced_streams`).
  - *What a load is served is the whole stream*, every live step's lines
    since the last open, a step run again contributing its new run's
    lines beside the kept steps' (never a copy that a stopped or running
    trip half wrote): after each link it is byte for byte the file the
    link wrote. The edit harness checks that at every stage
    (`PARTEX_SSA_STREAMS=DIR`: the CLI dumps the streams as the stores
    hold them after each link, `ssa::stream_values`).
  for outside tools, never reading its own writes back.
- **Commands** (`\write18`, `crate::shell`, `Host::system`). Which
  command runs is the engine's, the same on every host: web2c's
  `runsystem` and `shell_cmd_is_allowed` (texmfmp.c). Unrestricted
  (`-shell-escape`) it runs as written; restricted (`shell_escape = p`,
  TeX Live's default, or `-shell-restricted`) only if its first word is
  in `shell_escape_commands`, each argument quoted with `'` (a user's
  `"…"` becoming `'…'`; a `'`, or a `"` not closed or not followed by a
  space, is a quotation error; a `|` in the quoted command is refused).
  The log says `runsystem(CMD)...executed.`, `...executed safely
  (allowed).`, `...disabled (restricted).`, `...disabled.` or
  `...quotation error in system command.`, whatever the status, as
  pdfTeX's does; a status not 0 goes to the standard error. The
  command's standard output is handed back and put on the terminal at
  once, before what TeX has buffered and not yet flushed (`term_buf`,
  pdfTeX's stdio buffer), where pdfTeX's child writes it: an effect like
  any terminal output, so a linked or replayed build keeps its place. The host
  only runs the command it is handed: the native host with `/bin/sh -c`
  and kpathsea's variables in the environment (`SELFAUTOLOC`, by which
  latexminted's `latexrestricted` finds TeX Live; `TEXMF_OUTPUT_DIRECTORY`
  with `-output-directory`), a wasm host with whatever it has (Pyodide
  for latexminted, `tools/minted-pyodide`).

  In an SSA build a command is a call of the step that runs it, like
  BibTeX between trips but in program order, since the job reads what
  it wrote at once (minted `\input`s the highlighted code after
  latexminted wrote it):
  - *It reads* its command string (the `\write`'s tokens, the call's
    name) and the job's own files, those stored with `\openout` that a
    store reaches at that point and that are closed there: each is a
    load of the build's store, and the host is handed their contents,
    so a command run in a step run again out of order sees what the job
    wrote before it, not what a later step left on disk. A file still
    open is not an input (pdfTeX's command finds of it whatever stdio
    flushed). So minted's `batch` runs again when a code block's data,
    written to `_<md5>.data.minted`, changed, and not otherwise.
  - *It writes* each file it made or changed: the host finds them
    (natively by the output directory's stamps before and after, a
    wasm host by its file system's) and each is a store of the step, as
    an `\openout` and a `\write` per line would make, but its value is
    its bytes as made (a last line without its end is served so). A file it
    removed is a store too, of nothing (`StoreEv::Remove`): a load after
    it finds no file, and the link, which writes the job's `\openout`
    files from their effects, removes again those whose last store is a
    removal (`ssa::removed_files`; minted removes its data file after
    reading it). A load after it
    reads the store, a load before it the φ, and a store that changes
    wakes its loads, in the same trip or the next, as any store does: a
    cold build of a minted document converges where `pdflatex` run
    twice does (the first run writes the cache, the second reads it).
  - *A run that will be dropped runs no command.* A step run in a
    rebuild that read a definition it was not placed at is dropped and
    made again (3.15, "A rebuild", step 3); before a command, the run's
    reads so far are checked as a run past its budget is
    (`Tracker::run_doomed`), and a doomed run skips the command: it would
    see the wrong files (minted wrote `\minted@tmpdatabufferline39`
    unexpanded into its data file) and leave what it wrote.
  - *A command runs again only if what it reads changed* (the native
    host's `Commands`, on in an SSA build), as a BibTeX node: a step
    run again in a later trip or rebuild runs its commands again, and
    each is a node keyed by its command line, the job's files it is
    handed and the files commands made, changed or removed, as they are
    on disk then (latexminted's cache and configuration). A key that ran
    before is not run: its files are put back as that run left them and
    its `Ran` (status, files, standard output) is answered again. So
    minted's `cleanconfig`, at the document's end, which every rebuild
    runs again, runs only when a code block changed.
  - *A host that runs no commands* (`Host::runs_commands` false: the
    wasm extension's, the resident session's) logs every command
    `disabled (restricted)` with shell escape on, as partex did before
    commands ran.
  - What it does not model: a command's reads of files that neither the
    job stores nor a step loads (an edit of such a file alone runs
    nothing again; minted's `\inputminted` reads its file in TeX too,
    `\pdfmdfivesum`), and the files other commands made (not inputs, or
    each code block's change would run every later command again).
  - Machine mode's host keeps the job's files in memory until its link:
    it puts each name's last opened file on disk before a command, and
    its link leaves the files a command removed since their last open
    removed. The resident session's host runs no commands.

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
- *The tools are nodes at each trip's end* (3.16). After each trip,
  the build's BibTeX node of each stored `.aux` that has a `\bibdata`
  line, and its makeindex node of each stored `.idx`, run if a version
  they read changed, on the stores as the trip left them. Each defines
  a stream (the `.bbl`, the `.ind`): a name the job stores whose value
  the node makes, not a step, so it is the next trip's φ of that name,
  compared and diffed by lines like any φ. Its loads are served from
  memory; the link writes it, with the tool's log, as a file. The trips
  end when no load of the φ read other than what the trip stored *or
  a tool defined*: the tools are in the cycle. At the bound the tools
  run once more, as after a last pass, and with one trip a build
  (`PARTEX_SSA_TRIPS=1`) they run after it, as after a pass, for the
  next build's first trip to read.
- *The outside path* (`PARTEX_SSA_TOOLS=outside`, the compatibility and
  reference path; `PARTEX_SSA_TOOLS=0`: no tool runs). The host's
  closure (`Trips::tools`) runs the conventional tools between trips,
  each unless what its last run read reads the same (its stream's
  lines, its style and its databases: the memo of one entry per stream
  that `-watch` keeps, `bibtex::Runs`, `makeindex::Runs`), reading the
  streams from the stores and writing its outputs as files in the
  output directory, which the job loads like any input: a tool that
  wrote makes the next trip look at the job's loads again.
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
- *A trip that ended fatally* (decided 2026-10-05). A trip whose job
  ends with `history` at `fatal_error_stop` (no legal `\end` in
  nonstop mode, an emergency stop, a capacity exceeded, 100 errors) is
  pdfTeX's run that wrote a `.aux` cut short: by default its stores are
  the streams, the next trip reads the cut `.aux` and the trips go on
  from it, as `pdflatex` run again does (the fixed point the edit harness
  compares with; plain partex's messages and cut `.aux` are pdfTeX's on
  `tests/e2e/petals.tex`). An editor that compiles each keystroke must
  not: one key (`\petals{}` emptied, a `\frac{\p` not closed) ends the
  job fatally, and the next keystroke's single trip read a `.aux` without
  the later pages' `\newlabel` lines, its page 1 showing `??` until the
  idle settle. `SsaTracker::keep_complete` (off by default; the CLI's
  `PARTEX_SSA_KEEP_COMPLETE=1`; the extension turns it on) withholds such
  a trip's stores (`rebuild::trip_ended`, at a cold build's and each
  rebuild's end): each stored name keeps the value the last complete
  trip left (`Steps::complete`: the φ the first fatal trip read, a cold
  build's the host's file, which no build writes before its link), and
  that is the next trip's φ (its trip end makes no seed: the trips end),
  what the tools read, the next rebuild's own φ, what `stream_values`
  serves, and what the link writes over the steps' cut files
  (`ssa::withheld_streams`). The next trip that reaches the job's end
  normally publishes its stores again. The harness's oracle for it
  (`keep_complete` cases: `petals_keys`, `display_keys`) puts the files
  a fatal run wrote back as the run before left them. Either way the job
  leaves no PDF, as pdfTeX's `remove_pdffile` does: a plain run removes
  the file it began, and an SSA build's link, which writes the files
  from the effects after the job ran, removes it afterwards
  (`Tex::fatal_pdf`).
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
    A trip that stopped is not a trip that ended: no store is compared,
    no tool runs and nothing is published until it ends. Its stores,
    written by the steps it ran before the stop (a `.aux` cut before the
    later pages' lines), are never a φ: the resumed trip's φ is the one
    the stopped trip served (`Pending::phi`, the last complete trip's
    streams), and a stopped rebuild dropped (a cold build instead) leaves
    nothing behind. Whether the trip that resumes it ends fatally is then
    as for any trip (above). The harness's `petals_stop` case cancels
    the fatal keystroke's rebuild after 3 steps
    (`PARTEX_SSA_CANCEL_AFTER=3,0`: a list gives each rebuild its own)
    and checks that it wrote no file and that the next keystroke's one
    trip has page 1's numbers; its oracle runs nothing at the stopped
    stage (the stopped work published nothing), so that one trip a
    build (`PARTEX_SSA_TRIPS=1`) compares with one pass from the files
    before it.
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
  makes anew in its slot (`remake_font`) is loaded there too: every
  search that comes before a load finds a font the program has not made
  by now (an older run's, a later step's, a step gone's) as not loaded
  and makes it there (`found_font`): §1260's, and pdfTeX's `tfm_lookup`
  of a VF's local fonts (`vf_def_font`, at the shipout that first uses
  the VF) and of an expanded font's own TFM file.
  DVI's font numbers cannot be relocations: their opcodes' lengths
  depend on them, and §611's movement optimization depends on where
  bytes fall. So a font's number is a value the program makes: each font
  made reads the number of the last one (`scalar::FONT_COUNT`, set to
  the format's last font at its load) and takes the next, its
  `font::NUMBER` field (`FontData::num`). The ship reads that field. A
  step that no longer makes a font defines the count otherwise, so the
  next font's maker runs again and makes its number again, and so on
  while the numbers move. That wakes exactly the ships that use a moved
  number, even one whose page is unchanged.
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
2026-10-02 "How much of a build could run at once"), with a worker
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

#### Parallel builds, as built

`PHITEX_SSA_WORKERS=N` (default 1: the build in turn, byte for byte).
`ssa/par.rs` holds the workers, `ssa/rebuild.rs` the commits, and
`ssa/cold.rs` the cold build.

**A worker** runs on a view of the engine: a copy-on-write fork
(`Tex::fork_with`; eqtb, objects, hash and save stack shared by chunks).
It has a tracker of its own whose tables read through the build's (a
`Base` lent for the round), and its interned ids are renamed at the
commit (`Remap`). Its host asks the build's thread for file reads,
deflate and memos. Opening a file for writing, running a command and an
output's edit *taint* the run, which is then not taken. So do loading a
font, a budget, a runaway expansion and the job's end. Panics are
caught: the step runs in turn. The threads are scoped, each with a 32 MB
stack.

**A: a rebuild's dirty steps on workers.** When a dirty step is popped, a
round runs it and the next dirty steps at once, each from the entry its
old run predicts. Commits go in program order, in `commit`. A run is
taken only if all of these hold:
- its start is the step's (the end tag and the renumbering are unchanged);
- the fonts' and names' makers answer as they did, compared by
  visibility at the step's key (keys are made again as steps go in);
- its writes can be taken;
- once placed as `run_step` would place it, every read from outside the
  step finds the version it read.

The taken run's records are imported and `after_close` marks the readers
of the definitions that changed. Any other run is dropped, and the step
runs in turn. Early cutoff stands. A step's result depends only on what
it read, so the schedule changes the wasted work, never the outputs.

**B: a cold build's paragraphs on workers** (`PHITEX_SSA_COLD=1`, off by
default).
1. The build runs in turn to the end of the main file's line after
   `\begin{document}` (the base).
2. The rest of the main file is cut at its CST paragraphs
   (`phitex-syntax`), each a run from the base, its input moved to the
   paragraph's line.
3. *Discovery*, round 1, is wide and shallow: every run, 12 steps at
   most. The TFM fonts the runs load are loaded in the base's step, as
   `\font` there would load them, so that a run's `\font` finds them
   loaded (§1260) as a run after the one that loaded them does. The PDF
   is opened there too: a run's first page would open it.
4. Then every run goes to its paragraph's end, in batches of 2N, each
   batch taken in as it ends: records wait, never views.
5. Each step goes into the fold in source order (`cold_guess`). It is
   dirty if any of these holds:
   - its start is not where the step before it ended (a gap: a run cut
     short);
   - a read finds otherwise where the build is (wrong numbers are
     allowed);
   - a maker moved;
   - a write cannot be taken.

   A dirty step that only read otherwise keeps those reads and its run's
   host events. Once the steps before it are exact, `run_dirty` keeps it
   if each read reaches it at the version it read, and says its events
   then. A rebuild's run through a gap stops where the next run began
   (`Dirty::meet_start`).
6. The rebuild (`run_dirty`, with call hits applied) corrects the fold to
   its fixed point, which is the build's.

**Relocation** (`Remap::shift_strings`, `shift_names`). A run's
strings, made from the pool's end where it began, are numbered from the
build's pool's end. The number moves with the string in each place that
holds it:
- the pool's slots;
- its end (`str_ptr`, read and written, decoded from its version);
- a name's text (whose version is its bytes');
- a search's answer;
- the input's file names (`InputState::shift_strings`).

With names placed by name (`PARTEX_SSA_NAMES=1`, DESIGN 3.9), the
count of the extra region's names (`hash_high`) is a count only, and it
moves too. That mode is off by default: where a name goes shows in the
DVI's and the PDF's font order and in the log (ssa-edits' `names`,
`fontnum`, `includeonly` and `bibtex` cases differ under it), so a run
that made a name in the extra region is not taken unless the counts
agree. TeX shows a string by its bytes; nothing reads its number but to
copy or compare it.

**A worker's nest** (`patch_nest`). The nest's value holds the enclosing
levels whole, but each level's fields are slots of their own, which
pushing a level does not read. A run made elsewhere left the fields it
did not touch as it found them there. At the commit those fields are
the build's, as a run here would leave them; its own writes stand.
Without this a paragraph's interline glue came from the base's
`\prevdepth`.

**pdfTeX's interword glue** (`font_space_glue`) is read through the
`\fontdimen`s it is made from: a run that made it does as one that
found it made. XeTeX reads whether it was made.

**Output contract.** A parallel build's output may differ from a build in
turn's in:
- the PDF's object numbers and their order, and the xref's layout;
- the resource names (`/F…`, `/Im…`, `/Fm…`), since the fonts discovery
  loads are numbered in another order;
- the order of objects.

It must match exactly in:
- page rendering: glyphs, positions, paths, images, links and their
  targets;
- `ToUnicode`, the outline and the destinations;
- every non-PDF output (`.aux`, `.toc`, `.out`, `.bbl`, `.idx`, …);
- the log's errors and warnings.

The check is `phitex-draw`'s `same` example, which compares every
page's draw list and the outline, plus the other files byte for byte.
N=1 stays byte-identical. The log's statistics (strings, names, fonts
loaded) may differ.

**Measured** (2026-10-07, local, loaded box, fastdev). On an 8-chapter
book (474 steps), B is exact: pages alike, `.aux` and `.toc` the same.
It is not faster: N=1 builds in 2.44 s, N=8 in about 6 s. Of 732 steps
taken in, 722 are dirty, almost all because a read of the nest found
otherwise: the contribution list, `\prevdepth` and the page builder's
state. A paragraph's step reads the vertical list it appends to. The
next step for B is the layering 4.1 describes: a paragraph's lines as
a contribution appended (an effect), and the page builder as its own
steps that take the contributions in order. Then a paragraph's validity
does not depend on where the page broke before it. On the course before
relocation, A's rounds made the label edit's rebuild slower than in
turn: 27 s against 9.5 s. After it, at N=8 on the same box, the
rebuilds were still slower than in turn:
- label: 18.3 s against 9.5 s;
- ch00: 7.4 s against 4.8 s;
- section: 46.9 s against 31.8 s.

Most of the runs not taken were "its start moved" and "a font loaded".
A worker's run that reads a sealed line its placing did not give it is
tainted (`Tracker::seal_missing`). The build's own engine still asserts
that every line is in the table.

**Memory.** A worker's view is a CoW fork: the tables are shared
chunks, and a view costs little. What a run keeps is its tracker. Each
tracker carries stamp arrays of the tables' size: the eqtb's, the
hash's twice, the classes', the save stack's, and its recorder's dense
slots, grown to the highest slot touched (32 bytes a slot). That comes
to about 30 MB a tracker. A run finished early waits in `Spec` for its
step's turn, tracker and all. Nothing bounded those runs. A round at a
step's turn sent the next dirty steps not yet held, so the held runs
went on growing. A run whose step was kept, met or retired, and so
never had a turn, stayed until the rebuild's end. On the book with
every dirty step sent, 54 runs were held at N=2 and 113 at N=8. The
course's section edit held hundreds (accl: 44.8 GB at N=8 against
3.98 GB at N=1). The cure has three parts:
- the held runs and a new round together are at most four a worker,
  and a round is at most two;
- at each turn, a held run whose step is no longer dirty is dropped
  ("its step not run again");
- a finished run's stamp arrays go back to a pool (`Par::stamps`, at
  most two a worker), so only running trackers hold them. A pooled
  array has stamps of another tracker's generation and serial, so the
  tracker taking it moves its generation and serial past both, and every
  stamp is stale.

On the book (every dirty step sent), the peak was:
- N=2: 2.10 GB before, 0.66 GB after;
- N=8: 3.97 GB before, 1.16 GB after;
- N=16: 1.86 GB after;
- N=1: 0.41 GB.

The course's par.txt edits at N=8 (local) peaked at 5.17 GB. What is
left grows with the runs in flight, about 45 MB each. Stamp arrays
allocated in chunks as touched would cut that further. They would add
a branch to every noted read on the build's own engine, so this was not
done.

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
  roots, when the arena has doubled, and when a cascade goes cold, right
  after it retired the old steps after it (the rest of the job, run as a
  cold build, makes their records again). Not more often: a collection
  in the middle of a build leaves the heap full of holes among live
  data, and every later allocation pays for them. The records' writes
  live in chunks (`Writes`), a collection copying the kept ones into new
  chunks and freeing the old ones whole; the native CLI has the C
  library sort and return its free memory once the build settles
  (`malloc_trim`). A lean record (a step's, never looked up) is kept only while
  a live step of the fold holds it: a kept build's roots keep its
  children, which a later call may hit, not it. Planned: on its own
  thread.

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
- *Skips remembered* (`PARTEX_SKIPCACHE`, `skipcache.rs`). A false
  branch skipped in a macro body is remembered by the list and the
  position. While no name's class changes, the skip ends in the same
  place with the same terminating token. A remembered skip makes again
  the reads the skip makes token by token: each name it meets, once, in
  the order it first meets them, and `align_state`'s read and write. So
  the steps' reads are the same as a token-by-token skip's, and the
  tokens are not walked again. Each build and rebuild starts with none
  remembered.
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
  A chain entry is a copy of the save stack's kind, under the rules
  above: a local assignment of a register above 255 in a group the step
  opened saves its entry value in the entry (`Tracker::save_entry` names
  entry `i` by `-1 - i`, apart from the stack's entries), and the
  group's end gives it back (`Tracker::restore_entry`). So the restored
  register is not the step's definition, and an entry value still saved
  at the step's end is a read of it, placed by a rebuild. (Before, the
  chains were left out: the run saved whatever the arrays held, a later
  definition's when a later trip ran the step again, and the restore
  wrote it back as the step's definition. On the thesis pgf's `\pgf@x`
  family came out of a figure's group with values of the picture after
  it, and one arrow of the converged build was drawn 5pt off.)

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
  0.29 M commands after (measured 2026-10-04).

**Analysed, not built** (measured 2026-10-04):
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
| `PARTEX_SSA_LAZY_VERSIONS=0` | an eqtb entry's version made at each write, not when wanted (2.4) |
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
through per-slot stamps (a *slot* is the code's word for an address),
and an eqtb entry's version, made when wanted, only for a read that is
recorded (not for one of a slot the call wrote).
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
- A step keeps its records, and the slots it read from outside it, each
  as a slot id: the fold numbers each slot read once (`Fold::slot`), and
  a slot's readers are kept by its id. A removed step's reads and
  records go at the end of the rebuild that passed over it.
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
     placed, the tail and the nodes always with the length, put before
     them (`place_values`): the tail is a stand-in of its kind at the
     list's end, so put alone on the arrays' list (the job's end's,
     empty) it was lost, and a length put after it, for a run that read
     the length later, made the tail a stand-in's. A step that fires
     the page on a glue reads the tail and not the length; run again
     where the glue no longer fires, it found no break there and broke
     the page a line early (`ssa-edits` case `pagetail`; the course's
     section added and taken away left a page a line short);
   - the input is set to the previous step's result (3.5), mapped
     through the edits, with the flags of where it stopped, a
     paragraph's first boundary still to come (`par_start`) among them:
     a step that ended before it (the page builder `new_graf`
     deferred) leaves it set for the next one, and a run on a worker's
     view, which another step left it set on, ended at a paragraph's
     start it was not at (`par_edits`).
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
   until one ends at an old step's start: any later one, not only the
   next few (an index of the old steps' starts by place; SyncTeX's tags
   are compared only where the input is, and only with SyncTeX on).
   Each new step's reads are
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

### 3.16 BibTeX and makeindex are nodes

The build's BibTeX and makeindex are not passes run between builds:
they are nodes of its program, on the edge of the job's cycle (3.7).
Their inputs are slots the build tracks, their outputs are definitions
of the streams the job loads, and an edit wakes exactly the parts of
them that read what it changed (3.1). `partex-bibtex` and
`partex-makeindex` are the programs; `ssa/tools.rs` makes them nodes.

**Where they sit.** A trip's end makes each stored name's value, the
next trip's φ (3.7). The tools read those values: BibTeX the `.aux`
stream (and the `.aux` files its `\@input` lines name, from the stores
or the host), makeindex the `.idx` stream. Each defines a stream: the
`.bbl` (`ssa::define_stream`), the `.ind`. Such a name is a name the job
stores that no step opens, so a load of it reads the φ, which the tool
defined; a trip's end compares it with what the loads read, and a
change is an edit of the data its readers read by lines (3.15, "Lines
are of a data"): a `\bibitem` block that changed wakes the steps that
read its lines, not the bibliography. The build holds the value
(`last_phi`, the φ the next trip or rebuild starts from), so a rebuild's
first trip reads the tool's latest output from memory, as it reads its
own `.aux` (3.7, "Files are a view"); the link writes the files
(`ssa::produced_streams`: the `.bbl` and `.blg`, the `.ind` and
`.ilg`, in the output directory, each only when it changed).

**What they read.** A tool's reads are of three kinds, each checked the
way the build checks its own:
- *the stream's commands*: BibTeX reads an `.aux` file's `\citation`,
  `\bibdata`, `\bibstyle` and `\@input` lines only (bibtex.web §116
  skips any other line whole), so a `\newlabel` that changed is not a
  change of what BibTeX read; makeindex reads each `\indexentry`;
- *its files*: the style and the databases, looked up by the host
  (`FileKind::Bst`, `Bib`, `Ist`: kpathsea's formats, from the working
  directory as the CLI's `bibtex` and `makeindex` look them up) and
  checked by stamp once per rebuild, before the job's loads
  (`tools::look`), as every load is (3.4, "Files by stamp");
- *their parts*: a database's entries, an index's entries (below).

**Exactly the dirty work.** A tool's run is itself a small program of
calls, and it runs as the build does: a call runs again if and only if
a version it read changed.
- *BibTeX.* The `.bst` is a stack machine. `READ` turns the databases
  into entries (each cited key's type and fields, the crossrefs'
  inherited fields, the cite order); `EXECUTE`, `ITERATE`, `REVERSE`
  and `SORT` then run functions, `ITERATE f` one call of `f` per entry
  in the order. A call's reads are slots: an entry's field, type and
  key (`READ`'s outputs), an entry variable (`sort.key$`, alpha's
  `label`), a global variable (alpha's `last.label`, `next.extra`), the
  preamble, and the output buffer (the line `write$` has not ended).
  Its writes are entry and global variables and the output buffer; its
  effects are the `.bbl` bytes it wrote (a `\bibitem` block), the `.blg`
  lines (warnings) and its built-in call counts (the `.blg`'s
  statistics are their sums). Each call is placed by its command and
  its entry's key in the order that command iterates, and each slot
  keeps its definitions in that order, as the fold's do (3.15): a
  changed field wakes the calls of that entry that read it; a call
  whose writes come out the same stops the change there; one whose
  global changed (alpha's `last.label` after a new citation that
  collides with an older label) wakes the call after it in the order,
  which reruns only if what it read differs, so the "a"/"b" suffixes
  are recomputed as far as they changed and no further. `SORT` reads
  every entry's `sort.key$`; it is incremental (a sorted map from the
  key and the entry's place in the cite order, bibtex.web's tie-break):
  an entry whose key changed moves, and its calls in the commands that
  follow move with it, their definitions and reads re-placed, each
  run again only if what reaches it differs. The `.bbl` is the calls'
  bytes in order, the `.blg` the parse's lines and the calls' lines in
  order, with its statistics summed: byte for byte bibtex's.
- *makeindex.* Each `\indexentry` is parsed alone (its sort and actual
  keys, its page, its encapsulator); the sort reads every entry's keys
  (makeindex's comparison count, in the `.ilg`, and its duplicate
  marks are the sort's behaviour, so the sort is one node over all
  keys); each output block (an entry's line with its merged page list,
  a letter group's heading) reads its entries and its neighbour's keys
  (whether it is a `\subitem`, a new group), and is made again only if
  they changed.

**Switches.** `PARTEX_SSA_TOOLS=outside` runs the conventional tools
between trips instead (the reference); `PARTEX_SSA_TOOLS=0` none.

**As built** (2026-10-04).
- *The node* (`ssa/tools.rs`): one per `.aux` stream asking for BibTeX.
  It runs when one of the four commands it reads in the `.aux` files
  changed, or one of its files by stamp; its `.bbl` and `.blg` are
  streams it defines (`ssa::define_stream`).
- *Its calls* (`partex_bibtex::Session`, stage 2): a run parses the
  `.aux` files, the style and the databases again (`READ` is one call
  over the databases, linear in their size; its outputs are per entry),
  diffs `READ`'s outputs against the last run's (entries added, removed,
  changed field by field; the cite order kept where it can be, by a
  longest increasing run of the old keys), and then visits only the
  places a change reached. The style's execution commands are deferred
  while it is parsed (their places in the parse's `.blg` and terminal
  output kept) and run as calls afterwards. A fatal error drops the
  session and runs bibtex.web's program. Checked against bibtex.web's
  run on TeX Live's `plain`, `alpha`, `abbrv`, `unsrt` and `IEEEtran`
  styles over `xampl.bib` with random edits (citations added, removed,
  swapped, `\citation{*}`, fields, strings, the preamble, crossrefs,
  types, keys: `crates/partex-bibtex/tests/session.rs`): the same `.bbl`,
  `.blg` and terminal at every step.
- *Measured* (alpha, the 25 `xampl` keys cited, 105 calls): a note
  edited runs 1 call; a citation added runs 17 (its 4 calls, the
  `forward.pass`/`reverse.pass` chains as far as `last.sort.label`,
  `next.extra` and the longest label changed); taken away, 13. The
  thesis (`IEEEtran`, 48 entries, 103 calls): a title edited runs 1
  call (and its `.bbl` comes out the same: IEEEtran lower-cases it, so
  no step runs); an uncited entry edited runs 0 calls; a citation added
  runs 20 of 105.
- *makeindex* (stage 3): one node per `.idx` stream, run as `makeindex
  NAME.idx` when the stream or one of its files (a style, a `.mst`)
  changed; its `.ind` and `.ilg` are streams it defines.
  `partex_makeindex::Session` scans and sorts as makeindex does (the
  sort is one call over every entry: its comparison count, dots and
  duplicate marks are the `.ilg`'s), then makes the output block by
  block: a block (genind.c's `make_entry` for one sorted entry) is keyed
  by what it reads (its entry and the entries genind.c's state names,
  all but their input lines; the level, the open line, the range and
  encapsulator flags, the indent) and taken as it was when that is the
  same. A block that warned is always made again (its warning reads the
  input and output lines). Checked against makeindex's run with random
  edits of an `.idx` (entries added, removed, re-paged; ranges,
  encapsulators, sub-entries, cross-references; the default style and
  one with headings and a short `line_max`:
  `crates/partex-makeindex/tests/session.rs`). 300 entries: one added
  makes 2 of 301 blocks; one entry's page changed, 2.
- *Left*: `READ` and makeindex's scan are not split by entry (an edit
  of one entry parses the databases or the `.idx` again, linear and a
  few milliseconds); makeindex's sort is whole by its nature.

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

Until then it is what `phitex watch` and `phitex build` run, so its
memory is the watch's:
- Each region keeps a snapshot of the engine. The large tables are
  journaled vectors (`journal.rs`): a snapshot shares the chunks not
  written since the last one and copies the others whole, so a region
  costs the chunks it wrote. Its writes are scattered over the hash's
  places, a few per chunk, so chunks are small: 512 words (4 KB) for
  eqtb, the hash and the save stack, 128 objects (6 KB) for eqtb's
  objects. Smaller word chunks save little and make the store's save
  slower (more blobs, less compressible).
- A rebuild records at the fine grain where it re-executes, and holds
  the regions it replaces until it splices them out. A pass that runs
  the whole job again (the course's second, from no `.aux`) takes the
  build from 2.0 to 6.4 GB.
- The job's own output files are not edits: the watch records what it
  wrote and rebuilds only for an edit; a job whose files never settle
  stops at the fifth pass with an `Unsettled` line.
- Every region reads the lump (`Rest`), so state in it that differs
  makes every later region run again, though few read it. The PDF
  writer's object lists' heads, the outlines' first, last and parent and
  the catalog's open action are cells of their own (`MCell::PdfWord`):
  hyperref makes the outlines from the `.out` file at
  `\begin{document}`, and one more outline shifts the outline objects'
  numbers. Measured (2026-10-07) on the course, one `\section` added in
  chapter 15: the second pass re-ran 70.4M commands of 70.4M (81 s), now
  7.0M (13 s); the first pass re-runs 23.5M (35 s), from the section to
  where the page marks agree again.
- The current marks (`cur_mark`, every class) are a cell of their own
  (`MCell::Marks`), read where TeX reads them (`\topmarks` and its
  siblings, `fire_up`, `\vsplit`, which change them all and so read
  them first) and not `Rest`'s.
- The save stack is hashed as TeX can still use it (`SaveCanon`,
  `statehash.rs`): a location saved, assigned `\global` and assigned
  locally again in one group is saved again at the same level (§279);
  the group's end restores the topmost entry first, which leaves the
  location at `level_one`, so the entries below it are retained, their
  values thrown away (§282–§283): dead. LaTeX does that at the
  document's level for every `\color`, label or size change, and an
  added `\section` left 32 more entries for the rest of the document.
  The hash takes the stack frame by frame (the boundaries' chain): dead
  `restore_old_value` entries left out, a frame's live restore entries
  as a set by location (restores of distinct places commute), every
  other word in order, positions (`save_ptr`, `cur_boundary`, a
  boundary's link, `grp_stack`) counted by the live words below them.
  Two states that hash alike behave alike but for `\tracingrestores`
  lines in the log.
- At each cut, the machine also makes the save stack canonical in the
  state (`Tex::canonicalize_save_stack`, off while `\tracingrestores`
  is on): dead entries are removed, and so are *no-op* entries, which
  saved the value their location holds now while the location is at
  the entry's level (changed in the group and changed back): the
  location is set to the saved level instead. A later local
  assignment at that level saves the same word again, a `\global` one
  leaves `level_one` either way, and the group's end puts back the same
  value at the same level. The level is part of the location's eqtb
  cell, so this cannot be a hash-only form: the cell and `Rest` must
  agree. A construction's `saved(k)` words are written whole (only their
  `int` is read; the rest kept the slot's earlier contents, which
  differ between runs). Measured (2026-10-07) on the course's added
  `\section`: the edited run kept five such entries for the rest of the
  document (`\current@color`, `\@currsize`, `\delayed@f@adjustment`,
  `\par`, `\reset@equation`, each changed and set back at the
  document's level), and raw words with stale halves.
- *Relocatable values* (built: `reloc.rs`, `machine_reloc.rs`,
  `partex-incr`'s `Machine::origin_number` and its siblings). Some
  counters only number things: LaTeX's mark ids (`\g__mark_int`, in
  every `\marks` text and in the `\g__mark_…_tl` the output routine
  keeps). An added `\section` adds two marks, so every later mark id is
  2 higher and every later page's output routine state differs, though
  no page prints an id. A rebuild reuses a region whose values differ
  from its old run's only by such a shift, with what it wrote shifted
  the same way; every value it reads otherwise must equal its old run's
  (the read set, as before).
  - *Origins.* Every count register is an origin. The digits `\the` and
    `\number` make of one read as it is are *tagged* tokens (category
    12, a character code past Unicode: `TAG_BASE`, the origin, whether
    the digit is a number's first, the digit). `tok_chr` reads them as
    their digits, so TeX runs as with plain digits; only a machine turns
    them on (`Tex::set_tags`, never in INITEX, and a format holds plain
    digits).
  - *Copies keep them.* `get_next` keeps the token as it is in its list
    (`cur_raw`); a macro's arguments, a body or text scanned, a preamble
    and a token backed up store it as it is. A digit read for any other
    use is an *observation* of its origin (the job used the number), at
    the next token or at the region's end. Lists read without
    `get_next` observe themselves: printing (`\write`, `\message`,
    `\meaning`, `\detokenize`, a `\special`, a PDF string: anything that
    shows a value reads it), `\ifx` on macros where a tagged digit faces
    a plain one or another origin's, a delimiter of digits, a case
    change. A count register read as a number, or assigned other than
    by `\advance` by a constant, is observed.
  - *Answers.* `\ifnum` on a number of one origin and a constant
    records its answer (`MCell::IntCmp`); on two numbers of one origin
    nothing (the shift keeps their order). `\advance` by a constant
    and `\numexpr` sums with constants keep the origin and record that
    `x` and `x + c` must move alike (`reloc::SHIFT`). A number written
    in tagged digits of one origin, scanned by a caller that takes
    origins, keeps its origin.
  - *Guards.* A region's observed origins (`MCell::Origin`) and answers
    are guards that always hold: they never make a region dirty, and
    are checked only to relocate it.
  - *The rebuild.* After a re-run span, an origin cell that holds a
    larger number than the old run's there has moved: numbers above its
    old value at the span's start move by the difference (`Shift`; a
    positive shift keeps numbers distinct and in order). A dirty region
    whose differing guards each hold the old value relocated, whose
    other guards hold relocated (no moving origin observed, every
    answer the same with `x` relocated), and whose writes all relocate,
    is reused: its trace relocated (writes shifted and re-versioned,
    guards re-versioned, answers renamed), applied as a clean region
    would be (`replay_span` takes the relocated trace), the cells whose
    values moved entering D and marking their readers. Its exit state
    kept is the old run's, whose own writes then are patched as well
    (`Trace::moved`). The machine relocates eqtb words (a count
    register's number, a macro's or token register's list, a box
    register's box), the marks and the page's list; `Rest`, sealed
    lines and PDF objects relocate only if they hold no moving number
    (checked: the input stack, lists being built, the save stack and
    its saved count values, alignments). Validation is by value: a
    region is relocated only if the relocated old values are the new
    ones, so a wrong base or an origin used in a way the engine does not
    see as a copy costs a re-run, never a wrong result, as long as every
    use of a number that is not a copy is observed.
  - The stub machine has ids too (`next`), and the property tests
    relocate under random edits (`rebuilds_with_relocated_ids_match_scratch`).
- Measured (2026-10-07), the course, one `\section` and a paragraph
  added in chapter 15, `phitex build` from the saved build (commands
  re-run; the edit build's PDF identical to a cold build's):

  | | pass 1 | pass 2 |
  |---|---|---|
  | main 85e7d32 | 23.52M, 142 regions | 6.97M |
  | marks a cell, save stack canonical | 8.34M, 68 regions | 1.57M |
  | relocatable values off (`PARTEX_MACHINE_RELOCATE=0`) | 8.34M, 68 regions | 1.57M |
  | relocatable values | 6.47M, 55 regions (13 relocated) | 1.57M |

  What is left of pass 1 with relocatable values: chapter 15's rest up
  to the next file (0.70M), the pages after the section to the chapter's
  end (0.48M), one region that reads the PDF numbering whole (0.30M), the
  last region (0.07M), and 42 regions (4.92M) that each ship a page with
  a bookmark: hyperref writes every bookmark's sequence number
  (`\c@bookmark@seq@number`) into `course.out` (`% <n>` after
  `\BOOKMARK`), expanded at the shipout, and an added `\section` moves
  every later one. That is a value shown, so a read: those regions run,
  though only their `\write`'s bytes change; a region of its own for the
  shipout would re-run only that (4.2's page builder layer). Taking chapter 0's
  first words out re-runs 0.38M (4 regions), no second pass; both edits'
  PDFs are identical to a cold build's.
- *The page builder and the PDF writer as layers* (2026-10-07).
  - *A shipout is a region of its own.* The machine stops before a
    `\shipout` and, with `stop_after_ship`, right after it (`ship_stop`
    2, then 3 at the checkpoint): a candidate of level `LAYER` (4), cut
    always, whose key's low bit marks it (`Machine::layer_edge`) so the
    store's coarsening never merges across it. A page's shipout region
    reads its box tree and the few values it shows (page number,
    bookmark sequence numbers, marks); the regions that only built the
    boxes keep their guards, so when only a shown value moves the
    shipout alone runs again.
  - *`Rest` relocates.* A shipout run again leaves `Rest` with shifted
    tagged digits (the lists, the save stack and its saved counts,
    `save.rs`' `map_saved_counts`): the old snapshot is relocated
    (`machine_reloc.rs`' `shift_snapshot`), versioned by the machine's
    hash memo, and compared as any relocated value is.
  - *Object numbers are an origin.* `NUM_ORIGIN` (the ext register
    below the tag origins), held by `MCell::ObjCount` (the numbering's
    `sys` counter, written at a cut of a region that numbered):
    `\pdflastobj`, `\pdflastxform`, `\pdflastximage`, `\pdflastannot`
    and `\pdflastlink` give tagged digits, and an object number given
    back (`\pdfrefobj`, `\pdfrefxform`, …) scanned with its origin is
    a copy, not an observation. The numbering's answers relocate
    (`Machine::relocate_answer`): `FinalNum` and `NumState` keep only a
    hash, so the machine moves the answer it gives now back (each
    number the shift may have taken there) and compares the hash;
    `OfFinal(n)` is renamed and must name the same virtual id.
    `NumState(starts)` holds `sys`, `obj_ptr` and, of the open object
    stream, only whether there is one and, if one of the `starts`
    objects written to streams before the region's last answer can fill
    it, its index: a stream boundary elsewhere does not move the numbers
    the region saw.
  - *Font map entries compare by value* (`pdf_init_font`): pointer
    identity made a relocated region's font differ from a re-run's
    (chapter 0's edit re-ran 42M commands, with 90 more objects).
  - *The link's compressed streams persist.* The link already numbers
    objects, names font resources and assigns subsets at link time in
    pdfTeX's order from the virtual ids, and takes a region's resolved
    effects from the last link when they and the numbers they write are
    the same (`LinkCache`). What a new process paid was compression:
    every content stream again. `Host::deflate` keeps each compressed
    stream by its contents' hash, and `Watch` keeps those the process
    used in the store (`deflated/<key>`, written at the end of the
    process, `PARTEX_DEFLATED=0` off): an edit build compresses only the
    pages whose bytes changed (the first link 740 ms → 54 ms).
  - Measured, the course, `phitex build` from the saved build (commands
    re-run; each edit build's PDF identical to a cold build's):

    | | pass 1 | pass 2 | wall |
    |---|---|---|---|
    | `\section` added, before (5d9b894) | 6.47M (13 relocated) | 1.57M | 26.2 s |
    | `\section` added, layers | 1.10M (147 relocated) | 1.20M | 18.8 s |
    | chapter 0's first words, before | 0.38M, 4 regions | — | 5.53 s |
    | chapter 0's first words, layers | 0.26M, 2 pages | — | 5.33 s |
    | cold build, before | | | 183–216 s |
    | cold build, layers | | | 178–214 s |

    (The layers' row is with main's expansion speedup merged; the edit
    builds spend little of their time running commands. Chapter 0's edit
    renames font resources, `/F329` → `/F331` as a cold build does, so
    every page's bytes change and its link compresses them all: 620 ms.)

    Pass 1 of the section edit: the edited section (0.63M, two regions,
    and the last region), the output routine of the 13 pages to the
    chapter's end, whose breaks moved with the added lines (0.47M), and
    104 shipouts of one command each (every later page with a bookmark
    or a moved mark id: its shipout alone). Pass 2: the table of contents' pages, and two regions of pgf
    shadings (0.56M) whose `\pdflastxform` answer moved by a shift that
    is not the one the rebuild holds (an object stream opened in one run
    and not the other moves later numbers by one less): checked, so
    they run.
- The store's save writes each blob to its pack as the saver that made
  it compresses it. A pack's references are in frames. A save holds the
  build and its copy, not every blob as encoded, as kept and as packed
  at once.
- *A saved snapshot shares what it shares in memory* (2026-10-07).
  Before, each of the course's 3466 snapshots was saved nearly whole:
  522 KB each, 1810 of the save's 2764 MB. The PDF writer's tables
  (VTab, VMap, Val, the object table's shards) and
  `\pdfglyphtounicode`'s map (4.5k names, set once) went out flat in
  every snapshot. Every chunked vector went out as one reference per
  chunk: eqtb's objects alone have 5000 chunks. That came to 31M
  references, 527 MB of hashes in the blobs, and as many again in the
  `.kids` (542 MB beside a 1.29 GB pack). Now each is saved as it is
  kept:
  - a list of shared chunks (JVec, Flat, the object log, ShardMap,
    VTab) is a tree of nodes of 32 (`persist::save_seq`), each node
    named by the addresses under it, so a snapshot that wrote a few
    chunks costs those chunks and their paths;
  - a map is saved as its trie's nodes;
  - a record is saved as its `Arc`;
  - a table copied whole when one entry changes (the fonts' arrays, a
    font's metrics) is saved in chunks of 64 named by their contents
    (`save_chunked`).
  Chunks named by content inside JVec chunks (eight blocks of 64)
  were tried: 29% fewer raw bytes, but only 5% fewer once compressed,
  and 38% more blobs. Not kept.
  A pack refers to its own blobs by their place in its index, not by
  hash.
  The runs of regions are encoded by savers forked from the save's
  (`Saver::merkle_fork`), on up to 8 threads that take parts of
  consecutive runs from a queue. A blob's bytes depend on its value
  only, so the store is the same with any number of threads. Each
  saver compresses its blobs, and a writer thread only writes them.
  The copy's regions are merged without indexing it: only the
  accumulating cells' writers are looked up. Each run is composed in
  maps kept from one region to the next (`Composer`).
  Measured on accl, the course (299 pages there), against main
  baceb6f; the PDFs are identical:

  | | before | after |
  |---|---|---|
  | cold: merge before the save | ~4.5 s | 1.7 s |
  | cold: save | 18.4 s | 3.5 s |
  | cold: raw / kept | 2536 / 1171 MB | 934 / 357 MB |
  | `.kids` | 500 MB | 11 MB |
  | store after cold build | 1.68 GB | 0.38 GB |
  | ch00 edit: save (+ merge) | 1.10 s | 0.81 s (+0.23 s) |
  | ch00 edit: build wall | 4.37 s | 3.85 s |
  | load | 0.43 s | 0.43 s |

  On the user's 323-page copy (local, loaded machine), the merge and
  save after a cold build took 2.0 + 6.0 s; the user had seen 48.5 s.
  The store is 0.44 GB, down from 2.0 GB. The ch00 edit build takes
  4.6 s in all, with 1.2 s of save after its result.
- *A save during a rebuild* (2026-10-07) supersedes it. The rebuild asks
  whether an input it watches changed on disk (by modification time, at
  most every `PARTEX_WATCH_POLL_MS`, 50 ms) between re-executed spans and,
  now, inside a span after each region it cuts (`record_span`'s `stop`):
  a span is the whole cascade from a dirty region to where the state
  meets an old region again, which on the course is a chapter's page
  numbering (35 s). A stop inside a span gives that span up (what it ran
  is dropped: the newer edit may change it) and stops the walk at the old
  region it began at, the frontier, exactly as a stop between spans does;
  the spans before it are kept, and the next rebuild goes on from there
  with the newer edit too. Not while the last stop's frontier is still
  ahead (its cells are compared where the walk reaches it). The watch
  then reports the rebuild `superseded` (the terminal's `superseded by
  ch05.tex:31`, the viewer's `superseded` event) and starts pass 1 again.
  Each pass writes its outputs (the PDF renamed into place: always a
  complete PDF of some state) and is shown (`settling`); the passes that
  settle the job's own files (`.aux`, `.toc`, BibTeX, makeindex) are
  preempted the same way, a save between them making the next pass the
  first of a new rebuild (the pass bound counts from it, so the result is
  still the fixpoint, a cold build's). The checkpoint sessions
  (`--no-machine`) take a save at the next pass only: a pass stopped
  halfway would have to put the session back as it was before it.
  Measured (2026-10-07, fastdev builds, this machine, from the save to the
  stop that starts the new rebuild): a paragraph added at the start of a
  280-page one-file document (e2e's `preempt.tex`), saved again 0.6 s
  later, 3.07 s on 71a348e, 51 ms now. Most of those 3 s were spent before
  the first span: the changed lines' cells compared old against new, each
  finding the whole file's line starts again, as the host's line cache
  held one contents per file and the rebuild asks for the old and the new
  in turn (now it keeps the last two: the same edit's rebuild is 2.6 s
  shorter). On the course, whose chapters are files of a few hundred
  lines, a section added in ch05 or ch15 and a word saved 5, 10 or 20 s
  later: the spans are short, and the stop came 0.22–0.76 s after the save
  before, 0.23–0.36 s now; the settled PDF, `.aux`, `.toc` and `.out` are
  a cold build's byte for byte.

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
   commands. Measured (measured 2026-09-29): at command grain the index
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
- XeTeX (C5, 4.7) and LuaTeX (C6);
- trace compilation.

### 4.3 partex-PhiTeX: windows (2026-10-02)

This repository is partex at 21edd4e merged with two crates from
PhiTeX (ec3d63a): `phitex-syntax`, the lossless syntax tree cut into
paragraphs and reparsed incrementally, and `phitex-ir`, the SSA
program's text form with its parser and checker. The invariant of 3.1
stands. What changes is the grain, what is recorded, and what a
rebuild may cost. This section overrides 4.2 where they differ.

The measurements behind it (measured 2026-09-29, the course's one-word
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
checkpoint session with `SyncTeX` (saving one is refused; `phitex
build` and `watch` give it none). The cost off and on is measured by
`scripts/accl/tasks/synctex-ab.sh` (measured 2026-10-03).

**Machine mode** (2026-10-07). `phitex build` and `phitex watch` write
`<job>.synctex.gz` by default, as `pdflatex -synctex=1` does (the
engine command line gets `-synctex=1`; `--no-synctex` or `synctex =
false` in `phitex.toml` leave it out, `--synctex` puts it back; the
checkpoint sessions, `--no-machine`, never get it). `phitex clean`
removes it.
- *Places are values.* In machine mode a node's place is no table's
  handle but the place itself, an *inline* `Side`
  (`Side::INLINE`: `0xC000_0000`, the file's tag in 10 bits, the line
  in 20, each saturated). It is equal, hashed (a box's own place in its
  version too) and saved as the value it is: by `Persist for Side` and
  by the node codec (tag 16 and the place before a rule, glue, leaders,
  kern, math or unset node; a box's in its glue sign's top bit), so a
  snapshot, a region's writes and the store carry it. A node made at
  another line is another value: a region that made it again differs
  from its old run until the page that holds it is shipped. A box
  placed after it was versioned is versioned again. SSA's handles
  (`Side::HASHED` below `INLINE`) are as before.
- *No line moves.* SSA moves a reused step's lines through the edits
  (`Edit::pos`); the machine needs no such map. A region depends on the
  `Line` cells it read, by absolute line number, so a line inserted or
  removed makes every later region of that file run again, and a node
  placed at a moved line is a different value wherever it is held; a
  region reused read the same lines at the same numbers, and its places
  are right as they are. `PARTEX_MACHINE_RENAME=1`, which renames lines
  so that such regions are reused, is off with `-synctex` (their nodes
  would keep the lines they were placed at).
- *Events are the regions' effects* (`Effect::Synctex`, as SSA's steps
  have them), with the controller's flags a scalar row as in SSA mode.
  The walk numbers its nodes from each ship's start (only told apart
  within a ship), so a shipout region run again makes equal events. A
  page whose boxes were reused but whose shipout region ran again (the
  shipout layer, 4.1) reads its boxes from the state, places included.
  The final region prints `SyncTeX written on …` from the flags.
- *After each link* (each build and each settled pass), the regions'
  events in program order are fed to a new controller
  (`synctex::render`), the text gzipped as zlib's `gzopen` writes it
  (`zlib::deflate_once`: no state kept to resume from), and written
  renamed into place whole; again only when a region's effects changed
  (an inline place is part of an event's equality). The job's name is
  its log's (the final state's own strings may not be loaded yet). The
  compat path (`PARTEX_MACHINE=1 phitex --compat=pdftex -synctex=N`)
  writes it the same way.
- *The store.* A machine state with `SyncTeX` from the command line is
  saved (its places are in its nodes and its events in the traces): the
  state's option is saved with it and the controller made again on load;
  `phitex build` after a restart writes the same file. The events cost
  the store a few bytes each, in the traces' blobs, not in the
  snapshots. A document that turns `\synctex` on itself, with no
  `-synctex`, works in a process, but its build is not saved.
- *Found on the way*, in every mode: e-TeX's `hlist_out` makes a math
  node a kern once it is out ("Adjust the LR stack"), so in a leader
  box output again (a table of contents' dots) pdfTeX records it as a
  kern; partex recorded `$` each time.
- *Byte identity.* e2e `modern_synctex`: `include.tex` (three chapter
  files) built by `phitex build` and by `pdflatex -synctex=1` in one
  directory, run to its fixpoint: cold, then from the saved build after
  a word typed, a comment line inserted (every later line of the chapter
  moves, its boxes the same), a paragraph added (later pages move) and
  the line taken out again: the `.synctex.gz` and the PDF identical each
  time. The other `modern` cases' oracles run `-synctex=1` too (their
  `.synctex.gz` compared with the directory masked), and `synctex` passes
  with `PARTEX_MACHINE=1`. On the user's 323-page course (local), the
  machine's file is a plain run's byte for byte, cold and after an edit
  in `ch00.tex` from the saved build (the PDF too); against `pdflatex
  -synctex=1` the plain run's and the machine's differ in 156 of 620k
  lines, from one engine bug that is not SyncTeX's (below).
- *Found on the way*, machine mode only: a `\copy` of a box holding
  sealed lines (`seal.rs`) shares their contents, so their rules kept
  their places; a copy opens the sealed lines whose contents hold a rule
  (each read). A form's reference (`f`) is the link's object number, not
  the virtual one (`numbering_of`).
- *Cost* (the course, local, a loaded machine, `perf stat` and alternate
  runs): a cold `phitex build` 441.3G instructions against 434.0G with
  `--no-synctex` (+1.7%; wall within the machine's noise, 74–93 s both);
  the `ch00.tex` edit from the saved build 26.2G against 22.4G
  instructions, 4.2 s against 3.9 s (rendering 620k records and
  compressing 16.5 MB, on a thread while the link runs); the store 321 MB
  against 313 MB (+2.6%); `course.synctex.gz` 3.3 MB.
- *Not done.* A line whose characters font expansion stretched exactly
  to its width but for a few scaled points can be packed with `glue set
  0.0001` where pdfTeX's `hpack` leaves the glue unset (seen in a
  `tcolorbox` paragraph of the course with `microtype`; a box of
  `\hsize=407.9008pt` with newpxtext, the paragraph's characters
  expanded by +1, kerns between them and a right margin kern). The PDF is
  the same (the difference is below a `TJ` unit), but e-TeX's
  `hlist_out` then turns that line's glue into kerns, and the
  `.synctex.gz` records `k` for pdfTeX's `g`, 16 sp further on per glue.
  A pdfTeX-mode engine bug in `expand.rs`/`pack.rs` (the expanded widths'
  sum), not SyncTeX's; the repro is the course's (private) text.

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

### 4.7 XeTeX (C5) (2026-10-05)

XeTeX is e-TeX 2.6 with Unicode inside and its fonts outside TeX. Its
oracle is TeX Live 2026's `xetex` 3.141592653-2.6-0.999998 for the XDV
file (`xetex -no-pdf`) and the transcript, and `xdvipdfmx` 20260113 for
the PDF made from the XDV (without `-no-pdf`, `xetex` pipes its XDV
into `xdvipdfmx`). Its specification is `xetexdir/xetex.web` with the
change files of `xetexdir/am/xetex.am`, which `scripts/merge-web.sh`
merges into `target/web/xetex-merged.web`.

**The size, measured.** `scripts/align-web.py` aligns it with e-TeX
merged with web2c's change files (`target/web/etex-w2c.web`). Of
XeTeX's 1,740 sections:
- 1,184 are e-TeX's;
- 440 are changed and 116 added (6 of e-TeX's are gone);
- 9,808 lines differ: 7,168 in changed sections, 2,640 in added ones.

For scale, pdfTeX differs from `tex.web` by 17,821 lines (376 sections
changed, 526 added). Beside the WEB are 7,582 lines of C and C++
without the Mac files: `XeTeX_ext.c`, the layout, font and font-manager
glue, OpenType math, pictures. They call seven libraries. The oracle's
are the system's (Arch's `texlive-bin` 2026.0-2, here and in the accl
image alike):
- HarfBuzz 14.5.0 (compiled against 14.2.0);
- ICU 78.3, FreeType 2.14.3, Graphite2 1.3.15, fontconfig 2.18.3,
  libpng 1.6.58;
- linked in: TECkit and pplib 2.2.

**Primitives.** XeTeX has 506, e-TeX with web2c's changes 396, pdfTeX
568. Of the 110 XeTeX has and e-TeX does not:
- 16 are pdfTeX's, by the same names: `\expanded`, `\ifincsname`,
  `\pdfsavepos`, `\pdflastxpos`, `\pdflastypos`, `\pdfpagewidth`,
  `\pdfpageheight`, `\leftmarginkern`, `\rightmarginkern`, `\lpcode`,
  `\rpcode`, `\ignoreprimitiveerror`, `\partokenname`,
  `\partokencontext`, `\showstream`, `\synctex`.
- 15 are pdfTeX's without the `pdf`: `\strcmp`, `\mdfivesum`,
  `\filesize`, `\filemoddate`, `\filedump`, `\creationdate`,
  `\elapsedtime`, `\resettimer`, `\uniformdeviate`, `\normaldeviate`,
  `\randomseed`, `\setrandomseed`, `\shellescape`, `\primitive`,
  `\ifprimitive`. One more is new: `\suppressfontnotfounderror`.
- 24 are Unicode character and math codes: 13 `\U…` (`\Uchar`,
  `\Ucharcat`, `\Umathcode`, `\Umathcodenum`, `\Umathchar`,
  `\Umathcharnum`, `\Umathchardef`, `\Umathcharnumdef`, `\Udelcode`,
  `\Udelcodenum`, `\Udelimiter`, `\Uradical`, `\Umathaccent`), and 11
  older names for them (`\XeTeXmath…`, `\XeTeXdel…`, `\XeTeXradical`).
- 54 are the other `\XeTeX…`:
  - 23 need no native font: the version and revision, the character
    classes and inter-character tokens, the input encodings and
    normalization, the line-break locale, penalty and skip, dash
    breaking, the hyphenatable length, protrusion, upwards mode, glyph
    metrics, interword shaping, actual text, font tracing, and of any
    font its type, first and last character and glyph count;
  - 3 read pictures: `\XeTeXpicfile`, `\XeTeXpdffile`,
    `\XeTeXpdfpagecount`;
  - 28 ask a native font: 5 about glyphs, 6 about OpenType scripts,
    languages and features, 17 about Graphite's features, selectors
    and variations (AAT's on a Mac).

156 of pdfTeX's primitives do not exist in XeTeX: `\pdfoutput`,
`\pdfstrcmp`, `\pdfprimitive`, `\quitvmode`, `\letterspacefont`, …,
all `undefined` to the oracle. So XeTeX mode's table is XeTeX's,
generated from `xetex-merged.web` in its order, not pdfTeX's with more.

**Characters.** A character is a Unicode scalar value, up to 0x10FFFF
(`number_usvs`).
- *Tokens*: `cmd*0x200000 + chr`; a control sequence is
  `0x1FFFFFF + p`.
- *Code tables*: `\catcode`, `\lccode`, `\uccode`, `\sfcode`,
  `\mathcode`, `\delcode` and the active characters run to 0x10FFFF.
  Single-character control sequences run to 0xFFFF only: above, the
  name is a multi-letter one in the hash. `\XeTeXcharclass` (0–4095,
  4096 for ignored) is kept in `\sfcode`'s bits from 16 up.
- *Math*: 256 families (`\fam` and `\textfont` up to 255). A math
  code holds a class, a family and a 21-bit character (`\Umathcode`);
  `\mathcode` gives and takes its old 15-bit form.
- *Lines*: `input_line` reads code points. The encoding is sniffed from
  the first two bytes: a byte-order mark or a NUL means UTF-16 (BE or
  LE); otherwise UTF-8, decoded leniently (an ill-formed sequence
  becomes U+FFFD, with "Invalid UTF-8 byte or sequence at line N
  replaced by U+FFFD." in the log). LF, CR or CRLF ends a line.
  `\XeTeXinputencoding` and `\XeTeXdefaultencoding` choose UTF-8,
  UTF-16, `bytes`, or an ICU converter by name;
  `\XeTeXinputnormalization` applies NFC or NFD (ICU). `^^^^xxxx` and
  `^^^^^^xxxxxx` name characters.
- *Strings*: the pool's units are UTF-16 code units, so a character
  above 0xFFFF takes two, and `print` joins a surrogate pair again.
  `print_char` writes a character:
  - from 0xA0 up as UTF-8, to the terminal, the log and `\write` files,
    counting it one column;
  - below 0x20, at 0x7F and from 0x80 to 0x9F in `^^` notation
    (unless `-8bit`);
  - into a string (`new_string`) as UTF-16, and inside a `\special`
    raw.
- *Hyphenation*: the word's characters go up to 0x10FFFF and the
  trie's to 0xFFFF; `max_hyph_char` grows with the patterns; a word
  has at most `\XeTeXhyphenatablelength` (≤ 4,095) letters, which a
  format keeps (XeTeX dumps e-TeX's state variables). A trie character
  is a UTF-16 unit: a pattern's character past 0xFFFF is cut to 16
  bits but still widens the trie. An exception's letters past 0xFFFF
  are surrogate pairs; its key is its pool string's bytes, so a word
  of a TFM font matches it through `\lccode`s that are surrogates.
  Words of TFM fonts are hyphenated as XeTeX does (its native-word
  branches, §944–§946, §949, §956–§957, wait for native words), but
  for two differences. First, XeTeX marks a left boundary in `hu` with
  `max_hyph_char`, not `non_char`, and so makes it a character node in
  a post-break text when the font has a left-boundary program; partex
  leaves it out. Second, `\savinghyphcodes`' codes are kept beside the
  trie, not packed into it, so the trie's length differs (as in
  pdfTeX mode); the hyphens are the same.
- *Inter-character tokens*: `\XeTeXinterchartoks` holds a list for a
  pair of classes, in an e-TeX sparse array keyed by
  `class₁*4096 + class₂`. While `\XeTeXinterchartokenstate > 0`, main
  control inserts it before a character whose class follows the
  previous one's, in TFM text and native text alike.

**Native fonts.** `\font\x="Name/B/I:features"` or
`"[file.otf]:features"`.
- *Lookup*: a name through fontconfig's font list (1,060 fonts on this
  machine, none of them TeX Live's) and XeTeX's own matching of family,
  style, full and PostScript names (`XeTeXFontMgr*.cpp`, 1,026
  lines). A file in brackets is found through kpathsea (the OpenType
  and TrueType paths). LaTeX's default font goes by file: `TU/lmr` is
  `"[lmroman10-regular]:mapping=tex-text;"` (`tuenc.def`, `tulmr.fd`).
- *Loading*: FreeType, unscaled: the advances (`FT_Get_Advance`), the
  character map (`FT_Get_Char_Index`, with variation selectors), the
  extents and bounds from unscaled outlines, the italic angle, the cap
  and x heights. Metrics are computed in `float` (`unitsToPoints`) and
  `Fixed`.
- *Shaping*: HarfBuzz's `ot` shaper (Graphite only when `/GR` asks),
  with HarfBuzz's own Unicode functions, the font scaled to its units
  per em with no ppem, and XeTeX's FreeType callbacks. A word goes in
  as UTF-16 with its context, the script and language taken from the
  features. ICU's bidi first splits a word into directional runs, and
  `mapping=` runs a TECkit mapping first: `tex-text`, on every LaTeX
  default font.
- *Nodes*: whatsits `native_word_node` (40; 41 with ActualText: the
  text in UTF-16, and each glyph's id, x and y), `glyph_node` (42),
  `pic_node` (43), `pdf_node` (44). They go through packaging and line
  breaking (a native word hyphenated is split and measured again),
  `\XeTeXinterwordspaceshaping` and letter spacing.
- *OpenType math*: the MATH table's constants, variants, assemblies,
  italic corrections and kerns (`XeTeXOTMath.cpp`, and about 1,100
  lines of WEB in parts 35 and 36).

**XDV.** DVI with `id_byte` 7 and three more commands:
- `define_native_font` (252): size, flags, path, face index, then
  color, extend, slant and embolden as the flags say;
- `set_glyphs` (253): the width, then each glyph's x, y and id;
- `set_text_and_glyphs` (254): the text first
  (`\XeTeXgenerateactualtext`).

Every page begins with the special `pdf:pagesize default`, or the
width and height of `\pdfpagewidth` and `\pdfpageheight`. Pictures are
`pdf:image` specials. A TFM font never gets a character above 255: the
log says "Missing character: There is no ü ("FC) in font cmr10!". A
10 pt native font's size is 657,818 sp in its definition.

**The PDF.** `xdvipdfmx` is another program (`texk/dvipdfm-x`, not in
our sparse checkout yet). For a plain document it writes:
- PDF 1.7, with object streams and an xref stream, Flate at level 9;
- an `/ID` from MD5, the XDV's comment as `/Creator`,
  `xdvipdfmx (20260113)` as `/Producer`, and the date from
  `SOURCE_DATE_EPOCH`;
- a TFM font's Type 1 program (found through `pdftex.map`) converted
  to CFF (`/FontFile3`, `/Type1C`), with a ToUnicode CMap made from
  the glyph names (the AGL files);
- an OpenType font as a Type 0 font with Identity-H: a subset
  `CIDFontType0` (CFF) or `CIDFontType2` (TrueType), with a ToUnicode
  CMap made from the `cmap` and GSUB.

Of partex's PDF writer (pdfTeX's), the exact deflate, MD5, PDF values
and parsing, PNG and JPEG can be used again. The object layout, the
content streams, the font conversion and subsetting, and the specials
are xdvipdfmx's own.

**What partex has.** e-TeX 2.6 whole (XeTeX's is the same), pdfTeX's
31 primitives above, TFM fonts, DVI, the exact deflate, MD5, kpathsea,
PDF reading, images. Nothing of XeTeX's yet: `xetex` and `xelatex` are
reserved names that refuse to run (`compat.rs`).

**The shaper.** DESIGN 6 asks for a pure-Rust shaper pinned to the
oracle's HarfBuzz.
- rustybuzz 0.20.1 (and Tectonic's fork of it, 0.20.2) matches
  HarfBuzz 10.0.1, and is no longer developed.
- HarfRust 0.13.3 is the HarfBuzz project's port, grown from rustybuzz:
  `#![forbid(unsafe_code)]`, `no_std` with `alloc`, fonts read by
  `read-fonts`. It matches HarfBuzz 14.3.1, two releases short of the
  oracle's 14.5.0. The changes between them are to be read from
  HarfBuzz's history, and ported or shown not to matter.
- HarfRust has no Graphite (XeTeX uses it only when asked). Its font
  functions are its own, and they must agree with XeTeX's FreeType ones:
  the character map chosen, the advances, the extents.
- Arch updates HarfBuzz on its own schedule. So the corpus records the
  oracle's version, and the accl image's snapshot fixes it for the
  gate.

**Room for LuaTeX (C6).** XeTeX's pieces are cut so LuaTeX reuses them,
not rewrites them:
- `partex-otf`'s core is neutral: load a face, its metrics, shape a run
  into glyphs with positions. XeTeX's own semantics (native words, the
  feature strings, `Fixed` conversions) are a layer on it; LuaTeX
  shapes glyph nodes in node lists (luaotfload, luahbtex).
- The font index keeps each face's raw name records; the matching rules
  are separate (XeTeX's fontconfig order and `XeTeXFontMgr` now,
  luaotfload's name database later, which can pick another face).
- Strings go through one per-flavor layer (pool, `print`, the string
  functions): XeTeX's UTF-16 units kept as CESU-8; LuaTeX's UTF-8, Lua
  strings being any bytes.
- The Unicode code tables and `\U…` math codes are built once for both,
  sized past XeTeX's limits (LuaTeX has more math families and dozens of
  `\Umath…` parameters).
- Glyph data is plain per-glyph values, never only packed inside a
  native word, so a node library could expose nodes later.
- PDF writing varies by engine: XeTeX is XDV through xdvipdfmx; LuaTeX
  writes its PDF itself, a backend descended from pdfTeX's (other object
  order, font embedding, `\pdfextension`), so partex's writer is
  parameterised for it, not forked.
- The oracle script takes the engine as a parameter, LuaTeX's with its
  name cache and font list fixed as `fonts.conf` fixes XeTeX's.

**The work.** Written whole, then brought to byte-identity against the
oracle. Three parts with fixed interfaces between them, each checkable
on its own:
1. *The engine* (`partex-engine`, `partex-core`): `Flavor::XeTeX`, the
   names `xetex` and `xelatex`, `-no-pdf`; tokens `cmd*0x200000 + chr`
   in every flavor (tokens are `i32`, so pdfTeX runs the same code and
   its output does not change); XeTeX's primitive table generated from
   `xetex-merged.web`; the wide code tables past eqtb (as `xregs.rs`),
   256 math families, `\U…` math codes, inter-character tokens;
   input decoding, the pool's UTF-16 units and printing; the native
   nodes through packaging, line breaking, hyphenation and the page
   builder; OpenType math; the XDV writer. Checked against
   `xetex -no-pdf`'s XDV.
2. *The fonts* (`partex-otf`, `no_std`): the font list (one static
   index of TeX Live's faces, given to the oracle as its `fonts.conf`
   and to the Host as its lookup), XeTeX's name matching, faces read by
   `read-fonts`/`skrifa` with FreeType's unscaled numbers, shaping by
   HarfRust, TECkit's runtime (`tex-text`), bidi runs. Shaping is a pure
   function of the face's version and the call's arguments, kept in a
   content-keyed memo that can be switched off. Checked against
   `\XeTeXglyphbounds`, the fontdimens and the shaped glyphs of every
   face in the index.
3. *The PDF* (`partex-xdvipdfmx`, `no_std`, on the Host): XDV in, PDF
   out, xdvipdfmx's behaviour: positioning, objects, object streams
   and the xref stream, Type 1 to CFF, CFF and TrueType CID subsets,
   ToUnicode, the specials of hyperref, color and graphics, images.
   Checked from the oracle's own XDV against `xdvipdfmx`, before the
   engine writes any.

Then whole jobs against piped `xelatex` (`FORCE_SOURCE_DATE=1`,
`SOURCE_DATE_EPOCH`, the controlled `FONTCONFIG_FILE`): `refs/xetex`,
the fontspec, unicode-math and polyglossia test suites, the course in
fontspec; the SSA edits on XeTeX documents; pdfTeX's gates unchanged.
In the SSA, a native font is a value made by an applied call (like a
TFM font), its lookup a Host query versioned by its answer (3.4);
xdvipdfmx's pages and its end are nodes (3.16), its subset tags values
from first-use order.

**Status (2026-10-06).** The three parts are in, and the gate runs
XeTeX against TeX Live 2026: plain `-ini -etex` with `-no-pdf` (the XDV)
and without (the PDF), and xelatex (its format built by both) on
fontspec with TeX Gyre and Latin Modern faces, hyphenation, hyperref,
polyglossia's Greek, Russian, Arabic and Hebrew, beamer with TikZ,
graphicx's pictures (PNG, JPEG, BMP, PDF pages), and unicode-math with
one and with several OpenType math fonts: the logs, aux files, XDV
and PDFs byte for byte, in plain partex and in machine mode. In SSA
mode a fonts document and an edit to it give xelatex's PDF; a cold SSA
build costs about what a plain one does (4.4 s against 4.2 s, most of
it the font index). The PDF is made in process: the hosts keep the XDV
(`FileKind::XdvPipe`) and run xdvipdfmx where XeTeX closes its pipe;
a fatal error of the driver leaves its partial file and, in plain
partex, the log's "Error 256 (driver return code)". The driver's fatal
error (C's `ERROR`) is a value, never a panic: `Fatal`, passed up with
`?` from where C exits to `api::Session`, so a broken `\special`, image
or font cannot abort a wasm worker built with `panic=abort`; the
errors C leaves undefined (a read past a table) are `Fatal` too.
Input encodings
are XeTeX's own and ICU's stateless single-byte converters and UTF-8,
by any ICU name (`icu_tables.rs`, generated from `uconv`).

The upstream fontspec (94 tests) and polyglossia (292, the generated
ones included) suites run as l3build runs them (`-no-pdf`, two runs for
polyglossia), and to PDF, with xelatex's format built by each engine:
the logs, aux files and PDFs match but for fontspec's Graphite test.
They found kpathsea's case-insensitive search missing, e-TeX's
`\tracingstacklevels` and effective tail (`\unskip` past a final
`\endM`), and pdfTeX-only gates on what XeTeX shares (display boxes'
`dlist`, `\primitive`, `\special shipout`, `\vadjust pre`, which now
goes before its line). Tests whose fonts TeX Live lacks (CODE2000,
DavidCLM, Fandol, Junicode.ttf, NotoSerif-VF, Times LT Std) fail alike
on both sides.

**Glyph runs and origins.** Beside each page's PDF bytes, xdvipdfmx
gives its glyph runs (`partex_xdvipdfmx::glyphrun`, a side output that
never touches the PDF): every glyph it draws, in the content stream's
order, a native word's glyphs and a TFM font's characters alike, a
virtual font's characters as its base fonts' glyphs. A run has its
source (a native font's glyph id; a Type 1 font's file and glyph name
from the map entry's encoding or the built-in one; a TrueType or CFF
OpenType map entry's glyph id, as the driver picks it), its place on
the page (TeX's position through the driver's current matrix, `ctm`
and the text matrix beside), the graphics state's fill colour, and its
text as the PDF gives it: a word's `/ActualText` on its first glyph
(`\XeTeXgenerateactualtext`), else the glyph's `ToUnicode` text (a
native font's CMap made as `otf_create_ToUnicode_stream` makes it, every
glyph taken as used; a TFM font's glyph name through the AGL). Under
XeTeX, `Tex::origins` gives one origin per run, in order: the XDV
writer logs the glyph items it writes (native words and glyphs,
characters), and a character counts the glyphs xdvipdfmx draws for it
(one, or a virtual font's packet's, read as `dvi_locate_font` decides:
no map entry and a `.vf` file), each with the character's origin. The
`xelatex_runs` job checks a run per origin and per code the PDF shows,
where the PDF puts it (xdvipdfmx starts a string at TeX's position, a
viewer advances through it by the font's `/Widths`, which differ from
the TFM's by under a thousandth of an em a glyph), with the PDF's
`ToUnicode` and `/ActualText` text.

**Saved builds.** A machine-mode XeTeX build is saved to the store and
loaded by the next process like a pdfTeX one (2026-10-07; before, a
state holding a native font was refused, and every edit between two
processes built cold). A native font is saved by reference to its
files, as a TFM font's bytes are: the face's and the mapping's bytes
are the shared values the host served, in the store the
content-addressed blobs of the files served, kept once however many
states and fonts hold them; the face's index and content key and what
loading computed (features, sizes, colour, fontdimens) are values.
Loading parses the face and compiles the mapping again. A font file
changed on disk is an edit like any input: the regions that read it
run again, and the states after them hold the new face. The font
manager and the shaper's caches are not state, made again as a clone of
the engine makes them. A font's slot as a machine cell (`FontSlot`)
carries the native font and its direction state: without them a region
run again from a pass's start lost the default font of `xelatex`
(`[lmroman10-regular]`), and every character was missing from it.

Not yet: Graphite (`/GR`, fontspec's `Renderer=Graphite`: graphite2's
shaping, its features and queries), `\XeTeXlinebreaklocale` (ICU's line breaking), ICU's
multi-byte converters (GBK and the like), `\XeTeXinterwordspaceshaping`
above 1, math codes with families of 128 and up printed as XeTeX's
64-bit integers, the driver's status in machine mode's log, CJK
(no CJK fonts in the test set). TeX Live's xetex (Arch, ICU 78) itself
crashes on any ICU converter in e-TeX mode, so ICU input can only be
checked against `uconv`.

### 4.8 The live viewer (2026-10-06)

`partex watch` shows the document in the browser as it is built, instead
of an external PDF viewer reloading the whole file. It is the Overleaf
extension's viewer, not a second one.

**When.** On by default where standard output is a terminal and `CI` is
not set; `--view` turns it on anywhere, `--no-view` off (the old
behaviour: `--open` opens the PDF once). The browser is opened once
(`BROWSER`, else `xdg-open`/`open`) where interactive; the address is
printed in any case (`Viewing http://127.0.0.1:PORT/TOKEN/`). The PDF and
every other output are written as before: the viewer reads the PDF back
after the build, and no build reads anything of it.

**Server** (`view.rs`, `ws.rs`, `json.rs`; `std` only). A thread accepts
on 127.0.0.1 at a free port (`PARTEX_VIEW_PORT` to choose), a thread per
connection: HTTP/1.1, and a WebSocket (RFC 6455; SHA-1 and base64 for the
handshake) on `/TOKEN/ws`. Every path begins with a random token (128
bits from `/dev/urandom`), so another page in the browser cannot read the
document. `TCP_NODELAY`: the small frames go at once.

**Pages** (`phitex_draw`). After each build the PDF is opened
(`partex_engine::pdfread`, any PDF: compressed streams, object streams)
and each page hashed: its decoded content and size, kept for the pages
whose content streams lie wholly before the first byte that changed
(`Pdf::hashes_since`). A page's draw list is made when the browser asks
for it and kept by hash. The draw list is the extension's v2 (`Draws2`
in its `page2.ts`): glyphs as `<use>`s of their embedded Type 1 outlines,
the text over them for selecting, TikZ's paths. `phitex-draw` is the
extension core's `pdfdraw.rs` and `type1.rs` reading through `pdfread`
(theirs read only an uncompressed PDF); the extension is to depend on it
instead of its copies, so both draw from the same functions.

Images (`phitex-draw`'s `image.rs`): an image XObject or an inline image
(`BI … ID … EI`) becomes a `data:` URI in the list's `"I"` (by content
hash, kept across pages) placed by `"r"` (`["id",a,b,c,d,e,f]`: the SVG
unit square, row 0 on top, to the page from its top left). A JPEG passes
through; a Flate stream with PNG predictors is wrapped as a PNG as it is;
anything else is decoded and encoded as a PNG (`miniz_oxide`), `/SMask`
as its alpha, an `/ImageMask` in the fill colour. `"o"`
(`[[list,first,count]]`, list 0 paths, 1 images, 2 text) is the paint
order when it is not paths, images, text; `"x"` counts what is not drawn
yet (shadings). Clips (`W`, `W*`, a form's `/BBox`) are `"C"`: `{"c0":
["d", even-odd 0 or 1, the clip it is inside?]}`, each once, its path in
page coordinates; an entry drawn inside one names it in a last field (a
path's fifth, an image's eighth, a text run's eighth after its outlined
flag and colour or `null`, a glyph run's tenth after its colour and
matrix or `null`). `XeTeX`'s glyph runs, from outside the PDF, are not
clipped. A form `XObject` is drawn in place through its
`/Matrix`, with its own resources (its fonts join the page's `F`, one ref
for a font both name) or its parent's, 16 deep at most (`matplotlib`'s
figures, `\includegraphics` of a PDF). A page's hash covers the bytes of
the `XObject`s it can paint, so an image or form changed under the same
name changes the page.

**Protocol.** The extension core's requests (`session.ts`'s `CoreReq`,
`CoreRes`) as JSON: `{"id":N,"op":…}` answered `{"id":N,"ok":…,"json":…,
"draws":…}`. Ops: `open` and `status` (the page count and hashes),
`pages` (the hashes), `png` (a page's draw list; the extension's name),
`edit` (answered with the last build, the page asked drawn: the files are
watched on disk), `origins` (a page's glyphs and their sources, as the
extension core's `Session::origins` gives them: `{"files":[…],"g":[[x,
y, file, start, end, synthesized],…]}`, empty when the build records
none), and the CLI's `source` (a double-click's source: the editor opens
there). Events, unasked:
`{"event":"preparing","on":…}` when a rebuild begins and ends,
`{"event":"settled"}` when a build is in, `{"event":"page",…}` while it
runs, `progress` and `diagnostics` (below). The browser then asks for the
hashes, draws the page in view first if it changed, then the other pages
it shows whose hash changed; the others keep their drawing, and the
scroll stays where it is.

**Pages while the build runs** (2026-10-07). A cold build of a long
document (a 323-page course book: 50 s) showed nothing until its PDF was
in; now each page appears as it is shipped. The PDF cannot be read before
it ends: its fonts are subset and written at the job's end, its page tree
and cross-reference table last, and a machine-mode build links the whole
file only then. Its parts that draw a page exist at shipout, though: the
ship keeps them for display lists (4.6), the stream's bytes as the PDF
holds them and what its resources name (`Shipped`). A host that asks
(`Host::wants_streams`: the watch's, with a viewer) gets each stream as it
ends (`Host::stream_shipped`: a page once its page object is written, as
page `\pdftotalpages − 1`; a form at once), and draws it from a PDF of its
own (`partex_core::pagepdf::page_pdf`: the stream, its forms, its JPEG
images, its fonts with the whole Type 1 programs and encodings their map
entries name) through the same `phitex-draw` that draws the finished PDF.
No second renderer, nothing the build reads or writes changes (the record
is the one display lists make, kept on the side, never in a snapshot).
The engine only queues the stream; a viewer thread hashes it (the reader's
page hash, of the page's own PDF) and, if it differs from the last build's
page, pushes `{"event":"page","k":…,"hash":…,"pages":…}` with a
provisional hash (never a built page's), at once in a cold build, after
250 ms in a rebuild (one that is in by then shows only its PDF's pages: an
edit's page is not drawn twice). The browser adds the page (`Viewer.
shipped`, slots added, none removed: a second pass ships its pages again
over the first's, without a flicker) and asks for it when it is in view;
the build's PDF, when it is in, replaces the pages by its hashes. Drawn
from the shipped stream, a page has the PDF's glyphs (the same outlines
from the whole programs), places and paths; it lacks its links (the
annotations are written apart), its PNG and PDF images, `\pdfpageresources`'
graphics states, and the `/ToUnicode` text of symbol fonts (selection
text), which the PDF's page brings. `XeTeX` (xdvipdfmx after the job)
shows its pages when the build is in, as before. A 59-page `article`
(debug build, 3 passes, 36 s): pages arrive one by one from 13 s on, the
last of pass 1 10 s before the build is in.

**Status** (for an in-page status and error overlay to come). While a
build runs, `{"event":"progress","pass":N,"pages":K,"phase":…,"ms":T}` (its
pass, the pages it shipped in it, `typesetting`, `finishing`, `loading`,
`linking`, …, the time since it began), when it changed, ten a second at
most; when it is in, `{"event":"diagnostics","items":[…]}` before
`settled`, its errors and warnings as `snippet::json` gives them (severity,
code, message, notes, help, suggestions, file, line, col, the excerpt and
the span the carets mark, the macro context, the files it was included
from, a box warning's report), the structured diagnostics the terminal
shows, not its text; the `diagnostics` op gives the last build's again.

A save while a build runs supersedes it (4.1, "A save during a rebuild"):
`{"event":"superseded"}`, the progress's phase `superseded`, then the new
pass's `progress` and `page` events. A pass after which the job's own
files still change is shown as a build is, with `{"event":"settled",
"settling":true,"pass":N}`: the browser lays the pages out again and stays
`building` (its status says `settling (pass N+1)…`) until the plain
`settled`.

**Source and page.** A double-click finds the glyph nearest it (the
extension's `sync.ts`, `nearest`) and asks `source` with its file and
bytes; the watch shows `source: ch1.tex:12:5` and opens the editor there:
`--editor CMD`, else `PHITEX_EDITOR`, `VISUAL`, `EDITOR` (`editor.rs`). A
command with `{file}`, `{line}` and `{col}` runs as written; a known
editor alone gets its preset (`code -g {file}:{line}:{col}`, `emacsclient
-n +{line}:{col} {file}`, `nvim --server $NVIM --remote-send …`, Sublime,
Zed, Kate, gedit, JetBrains'); a terminal editor (`vi`, `nano`) cannot
open beside the watch, which holds the terminal, so the place is only
shown. Forward search: `partex sync FILE:LINE[:COL]` finds the watch of
its directory (or one above) by the address it keeps in
`$XDG_RUNTIME_DIR/partex-view/` (a file named by the directory's hash),
and asks `GET /TOKEN/sync?file=…&line=…`; the watch sends the line's bytes
to the pages (`{"event":"sync",…}`), which highlight the glyphs that came
from them (`sync.ts`'s `from`, `lineAt`, `boxes`), the page in view first.
Both need the build's glyph origins.

**XeTeX.** Its pages' glyphs come from xdvipdfmx's glyph runs (the
extension core's `xetex::extra`, in `phitex_draw::xetex`: OpenType and
TrueType outlines through skrifa, a TFM font's Type 1 from its `.pfb`, a
transformed glyph alone with its matrix), the PDF giving the paths and
rules: with a viewer on, the in-process driver keeps each page's runs for
it (`dpxfiles::keep_glyph_runs`), which changes nothing it writes.

**Page** (`crates/partex-cli/viewer/`). `index.html`, and `viewer.js`:
the extension's `viewer.ts`, `page2.ts` and `sync.ts` at a pinned tag with the CLI's
host for them (`cli.ts`, a `ViewerHost` over the WebSocket), bundled by
esbuild (`scripts/viewer-bundle.sh`, which takes the extension's files
from its repository at the tag; the bundle, 13 KB, is committed and
embedded, so a build needs neither node nor that repository). The text
fonts `page2.ts` asks for (Latin Modern) are served from kpathsea. When
the extension tags its embeddable bundle, `cli.ts` gives way to its
`PreviewSession` over the same protocol.

**Measured** (a three-page article with a second file, machine-mode
watch, sandboxed, this machine): a word edited on disk is built, hashed
(0.3 ms) and settled 125–200 ms after the write, and the changed page's
draw list (4 ms, 16 KB) is in the client 4 ms after the settle; only the
changed page is asked for and sent (`PARTEX_VIEW_LOG=1` logs it).

**Origins in the watch** (2026-10-07). `watch --ssa` records glyph
origins (4.4) when a viewer runs and gives them to the viewer after
each link (`Origins::of`). The machine watch places the glyphs by its
`SyncTeX` file (4.5, `Origins::from_synctex`, `synctexfile.rs`): each
glyph the PDF shows (`glyph_places`) has the line of the record that
ends its run of characters (the first `x`, `g`, `k`, `$`, box or rule
record at or after it on its baseline, else the last before it), its
bytes the line's as the file is on disk. Line-level, as every SyncTeX
editor has it: a double-click opens the line, `phitex sync` highlights
the glyphs of the line. Both are made per page when first asked, on a
thread after the build (`switched` tells the browser to ask again).

### 4.9 `phitex watch --ssa` (experimental, 2026-10-07)

`phitex watch --ssa` (or `ssa = true` in `phitex.toml`) rebuilds on the
dynamic-SSA runtime (3, `partex_core::ssa`), the engine the Overleaf
extension runs, instead of machine mode (`modern/ssawatch.rs`). Everything
around the runtime is the machine watch's: the terminal's lines (a rebuild
line per save, `-v`'s `SSA built in …`/`SSA rebuilt in …` reports), the
keys, the viewer and its events (4.8), the contents sidebar (read from the
PDF), `--copy-pdf`, `-o`, `phitex why`'s record. It keeps no store: each
`watch --ssa` starts with a cold SSA build, and says so in one line.

- *The cold build* is the job on the runtime (`ssa::run_applying`), then
  the trips that settle the job's own files (3.7, BibTeX and makeindex as
  the build's nodes), driven one at a time (`ssa::settle` bounded to one
  more trip, called again until it settles or `PARTEX_SSA_TRIPS` trips,
  5): each trip is linked, written and shown as a machine watch's pass is
  (`Progress::Settling`, the viewer's `settled` with `settling`). A trip's
  end is idempotent (`trip_end` compares with the φ the trip read), so the
  trips one at a time are the trips `settle` would run at once.
- *An edit* is one trip (`ssa::rebuild_trips` with one trip at most, as
  the extension's keystroke): its files are written and its pages shown
  at once; the trips that follow run after it while nothing newer is
  saved, as the extension's idle settle (`settle_idle`).
- *Edits* are found by the files the build read (`NativeHost::read_paths`,
  each with the stamp and bytes it was read with, `read_as`), polled every
  `PARTEX_WATCH_POLL_MS` (50 ms; the TeX tree's files and the files not
  found that appear every tenth look); the job's own files as the link
  wrote them are not edits (`NativeHost::as_written`). The rebuild itself
  finds what changed, by the host's checks of the loads (3.7).
- *A newer save* stops the trip under way at the next step boundary:
  `SsaTracker::cancel` is a poll of the project's inputs' stamps (at most
  every poll period), and the stopped trip's work is pending (3.7, "A
  rebuild stopped"); the watch says `superseded by FILE:LINE` and the next
  rebuild goes on with that work and the new edit. Between trips a save is
  looked for before the next begins. A cold build's first trip is not
  stopped (a cancelled cold build would have to start over); a save during
  it is found by its stamp afterwards.
- *Cold again.* A rebuild the runtime cannot make (`unsupported`), or a
  trip past its deadline (the last cold build's time, at least a second;
  `PARTEX_SSA_DEADLINE_MS`, `0`: none), is built cold.
- *SyncTeX.* The steps' events are rendered after each link
  (`Tex::synctex_file`) and `<job>.synctex.gz` renamed into place whole;
  with a viewer, glyph origins are recorded for double-click and
  `phitex sync` (4.8).
- *Files.* The SSA link (`SsaLinker`) writes them, each renamed into place
  whole in the watch (`atomic`: the bytes before the first change read
  back, the link's after): the PDF on disk is always complete. The
  terminal's text and the diagnostics are captured from the link
  (`NativeHost::capture`), the pages counted from it.
- *Tests.* e2e's `modern_watch_ssa` (three edits during the watch, each
  settled against `pdflatex` run to its fixpoint) and
  `modern_watch_ssa_preempt` (a save 0.6 s into a long rebuild supersedes
  it within 2 s; the PDF never torn; settled as a cold build).
- *Known SSA-runtime issue* (not the watch's; open). On the user's
  323-page course, deleting "This course exists" from `ch00.tex` line 3
  and rebuilding gives a PDF whose font numbers (`/F326` for `/F329`,
  `/F336` for `/F370`) differ from one plain pass from the same files;
  the `.aux`, `.toc` and `.out` are the same. The CLI's harness does the
  same without the watch (`PARTEX_SSA=1`, `PARTEX_SSA_TRIPS=1`,
  `PARTEX_SSA_REBUILD="sed -i '3s/This course exists//' ch00.tex"`,
  compared with one plain pass), also with `PARTEX_SSA_FONT_REFS=0`, on
  main 86eaad7. Until it is fixed, `watch --ssa`'s settled PDF can differ
  from a cold build's in its font numbers after such an edit.

**Measured** (2026-10-07, accl job 7355, one node of 48 cores to itself,
release build, the user's 323-page course with its settled `.aux`; two
sessions of each, alternating; times from the save, renamed into place,
to the rebuild's line; peak RSS of the watch after each step):

| | machine `watch` | `watch --ssa` |
|---|---|---|
| cold build (to `Watching`) | 59.6 / 46.1 s, 2.8 GB | 82.1 / 74.8 s, 3.0 GB |
| ch00: "This course exists" deleted (1 pass) | 2.35 / 2.18 s (rebuild 0.67 / 0.61 s) | 5.44 / 4.89 s (rebuild 4.3 / 3.9 s) |
| ch15: a `\section` added: first pages | 14.2 / 12.5 s | 3.2 / 3.0 s |
| ch15: settled (2 passes) | 20.6 / 18.4 s, 4.4 GB | 22.7 / 21.9 s, 3.5 GB |

The machine watch's cold build saves no store here (a fresh cache);
`--ssa`'s first trip runs the job once more slowly than a plain run (71M
commands recorded). An edit's first pages come from one trip: the ch00
edit, which moves no page, costs 789 steps (666k commands) on SSA against
29 regions on the machine; the `\section`, which moves every later page
of the chapter, shows its pages after 3 s on SSA (one trip),
where the machine runs its first pass to the end of the cascade (12–14 s)
before it writes a page. The settling trip (the `.aux`, `.toc` and `.out`
changed) then costs both about as much. The settled files equal a cold
`phitex build` of the final source, but for the font numbers above in
`--ssa`'s PDF.

### 4.10 latexdiff, natively (phase 1, 2026-10-08)

`phitex-diff` compares two versions of a project and writes what latexdiff
writes: the new version, flattened, with the old one's differences marked
(`\DIFadd{…}`, `\DIFdel{…}`, `\DIFaddbegin`/`\DIFaddend`, the `FL` forms in
floats and alignments), and latexdiff's definitions in the preamble; and
the list of changes (kind; file and byte range on each side; the text on
each side; the section of the new version it is in; where its markup is
in the output). It depends on `phitex-syntax` alone (wasm, the extension,
and the CLI alike); `phitex-git` reads past versions from git (gitoxide,
`default-features = false`, features `sha1` and `revision`: rev-parse,
refs, commits, trees, the index; no checkout, no network).

- *Flattening.* Each side follows its own `\input`/`\include` graph (as
  latexdiff `--flatten`), with a map back to each file's bytes: a file the
  new version no longer inputs is deleted text where it was input, a new
  one added.
- *Tokens.* The CST's paragraphs are read again into a tree of tokens:
  words, commands with the arguments their signature says, math as a unit,
  environments with their bodies, alignment rows. The signature table is
  latexdiff's lists (`SAFECMD`, `TEXTCMD`, `CONTEXT1/2CMD`, `MATHENV`,
  `MATHARRENV`, `FLOATENV`, `PICTUREENV`, `LISTENV`, `COUNTERCMD`, the
  cite family it puts in `\mbox`), extended from the preambles
  (`\newcommand` and friends: arity, and safe if the body is), and by the
  caller. A command defined only by the old preamble is unsafe deleted.
- *Diff.* Paragraphs (runs to a blank line) are aligned first; each run of
  paragraphs that differ is diffed by tokens, patience diff over Myers'
  (linear space, an edit budget past which a run is a rewrite). In a run
  of changes, nodes with the same prefix (a command and its other
  arguments, an environment's `\begin`, a row's columns) are paired and
  diffed inside. Common runs under three words between long changes are
  merged into them (latexdiff's `MINWORDSBLOCK`).
- *Markup only where it compiles.* Unsafe deleted text is commented out
  (`%DIFDELCMD <`), unsafe added text stays outside the markup; a deleted
  counting command steps its counter back; a deleted `\caption` shows its
  text only; display math is coarse (old struck out as an unnumbered
  `displaymath`/`align*`/`eqnarray*`, labels commented out; new added
  cell by cell); a deleted row of a kept table is shown, struck out;
  verbatim and pictures are never marked inside; comments are left as
  they are.
- *Tests.* Unit tests and `tests/corpus.rs` (16 synthetic pairs: word
  edits, paragraphs, sections, cites and refs, inline and display math,
  floats and tables, lists, footnotes, multi-file, verbatim, comments,
  macros, hyperref, a rewrite, a new document) always run.
  `tests/compile.sh` compiles each pair's diff with stock pdflatex and
  runs Perl latexdiff on it to compare; `tests/fuzz.sh` diffs random
  edits of the corpus and compiles them.

**Measured** (2026-10-08, this machine): all 16 pairs compile (latexdiff:
15, it breaks a booktabs table with `\DIFaddendFL \bottomrule`); the text
marked is latexdiff's in 5, and elsewhere differs in granularity only
(latexdiff marks letters in a one-word change, `Intro[+duction+]`, and
inside equations; the old side of a changed verbatim is not shown). Of
300 random pairs, every diff compiles where both versions do (278); 3 of
the 289 whose new version compiles fail, each with an old version that
does not compile (a second seed: 280 of 280, and 4 of 294, the same).
A 3 MB document diffs in 0.6 s (release, this machine, flattening and
I/O included). The two crates add 87 crates to `partex-cli`'s clean
release build (48 to 135 units; 664 s before, 673 s after, fresh target
directory, no sccache, this loaded machine: within noise, gix building
beside the long `partex-core` and LTO chain).

*Markup options* (2026-10-09). `Options` has latexdiff's `--type`
(`Markup`: UNDERLINE, CTRADITIONAL, TRADITIONAL, CFONT, FONTSTRIKE,
CCHANGEBAR, CFONTCHBAR, CULINECHBAR, CHANGEBAR, INVISIBLE, BOLD) and
`--subtype` (`Subtype`: SAFE, COLOR, MARGIN, LABEL, ZLABEL,
ONLYCHANGEDPAGE; DVIPSCOL, dvips only, is not there), their definitions
copied from latexdiff.pl (checked against it); changebar's `--driver`
(`Driver`); and colors for added and deleted text (`Color`: an xcolor name
or expression, or `#RRGGBB`, nothing else), which load xcolor where
latexdiff loads color and make the type's and subtype's blue and red
`DIFaddcolor` and `DIFdelcolor` (no colors: latexdiff's preamble as it
is). In a float, TRADITIONAL's deleted text is shown small, not in a
footnote. `tests/styles.sh` compiles every type, subtype and some colors
on five corpus pairs with stock pdflatex: 95 of 100 compile. The five:
LABEL and ONLYCHANGEDPAGE before a booktabs rule (`\DIFaddendFL` is a
`\label` there, before `\bottomrule`: latexdiff's own failure), and
(C)TRADITIONAL's footnote in deleted display math.

*Phase 2: the live diff* (`live.rs`, 2026-10-09). `Live` keeps a
`Baseline` and the last new version, and diffs each new flattened version
again only where it changed: the edit (where the text differs from the
last) applied to the new body's CST, the paragraphs its splice changed
read into tokens again with those read together (a *unit*, which no token
crosses; an opener with no closer reads on to one that appears), the
alignment kept as segments (matched chunks, or runs with their markup and
changes) and aligned again only around the edit, between the matched
chunks beside it. An edit outside the body, or one that can move where it
ends, is diffed whole. A one-word edit in a 200-paragraph document reads
one paragraph and diffs one chunk (`Stats`, asserted); 2880 random edits
of the corpus give the whole diff's text and changes each time. `places`
puts each change at its markup's first glyph (glyph origins or SyncTeX),
`changes_json` is the list the watch and the extension send. Not built
yet: the watch's second job and keys, the viewer's Diff panel.

### 4.11 A build streamed: pages as they are shipped (2026-10-08)

The Overleaf extension's `ph_open` answered only after the whole first
pass: page 1 waited for page 62 (0.8–6.6 s with every cache warm on six
arXiv papers). Now the open can stream: it starts the build and returns,
and the host runs the build on in slices, drawing each page as it is
shipped and answering its requests in between. The build is the same
build; only where the host gets control back changes.

**Slices, where the engine stops anyway.** A slice ends only at a
checkpoint between two commands (`Tex::start`/`resume`, 3.15), where the
whole state is in the engine:
- *plain* (the first paint, worker A): `Tex::set_stop_at` every N
  commands (the extension's core: 2048, the clock looked at between);
- *SSA, trip 1*: `ssa::ColdRun`, `run_applying` cut in three
  (`start`, `step(tex, budget)`, `finish`), the loop's state (the open
  step, the report, the parallel cold plan) in the struct. `run_applying`
  is `start`, `step` to the end, `finish`: one code path, so a trip in
  slices is the trip by construction. Trip 1 is not preempted: an edit
  made meanwhile waits for it and is then rebuilt as a one-trip
  keystroke. The slices make a preemptible trip 1 possible later (stop
  at a step boundary, keep the `ColdRun`, rebuild the edit's steps
  against it), which a cold build's cancel today cannot (it throws the
  build away);
- *SSA, the settle's trips*: each stopped at a step boundary when the
  slice is over (`SsaTracker::cancel`, 3.7 "A rebuild stopped"): the
  work left is pending and the next slice's `settle` goes on with it; the
  link waits until the trips end. An edit while they run stops them and
  its rebuild takes their work with it, as `watch --ssa`'s keystroke.

The tests (`partex-core/tests/stream.rs`): a plain run in slices of 1, 2
and 7 commands writes the run's PDF byte for byte; an SSA trip in slices
of 5 commands links the trip's PDF.

**Pages as they are shipped.** A host that asks (`Host::wants_streams`)
gets each content stream at its end (4.8), and keeps them in
`pagepdf::Shipments` (each page's last stream, each form by number; a
page shipped again by a later trip replaces it). Page k is drawn from its
own PDF (`Shipments::page_pdf`, `page_pdf`, through `phitex-draw`, the
reader of the finished PDF), its fonts' whole Type 1 programs read as the
build reads them, and their `/ToUnicode` maps made as the job's end will
make them (`\pdfgentounicode` on: the same `\pdfglyphtounicode` entries,
the encoding's or the program's own glyph names, `tounicode::cmap_for`),
so the text layer (selection, search) is the PDF's too.

*Stable hashes.* A page's hash in the finished PDF covers its content
stream, its size and the bytes of the `XObject`s it can paint
(`phitex_draw::page_hash`); its fonts, annotations and resources are not
in it. A stream is *whole* (`ShippedStream::whole`) when its own PDF draws
what the finished PDF will: its images are JPEGs (a PNG's or a PDF
page's bytes are written later), and, a page, it has no annotations or
links (a link's target is resolved at the end) and no
`\pdfpageresources` (graphics states), a form no resources of its own,
no font is `\pdfnobuiltintounicode`'s; a page is whole when it and every
form it draws are. A whole page shown before the build is in has its
PDF's hash, so the host keeps its drawing when the PDF comes (the test:
whole pages' hashes of the shipped streams equal the linked PDF's, plain
and SSA); any other page gets a hash no built page has and is drawn again
once. Drawn the same means: the same paths, glyph outlines, images and
text; only the fonts' names differ (`"F"`: the PDF's subset tag is made
from the glyphs of the whole document), which name the glyphs' `<use>`
ids and draw nothing. On papers with `hyperref` most pages have links,
and so are drawn twice. In SSA mode the link renumbers fonts
(`PARTEX_SSA_FONT_REFS`), so a page with more than one font has another
`/F` name in its stream than in the PDF, and is drawn again too.

**Font files named ahead** (`Host::wants_hints`, `Host::will_need`). In
the browser each file the job lacks is fetched when read, one pack and
one round trip at a time (0.5 s each from Shelf when cold; 15 fetches,
8.4 s, for one paper), and most of them are fonts: their Type 1 programs
and encodings, read only when the PDF's fonts are written at the job's
end. The engine names them when it first knows them: at each `\font`
once a map has been read, and for every font loaded so far when the
default map is read (the first page); the map entry's `.pfb` and `.enc`.
A host that fetches from far away fetches them in the background between
slices, so the job's end finds them. A hint is pure: the map is peeked
at (`FontMap::peek`: nothing kept among the lookups found, which a
loaded table must hand out again), no tracker sees it, and the test
builds the same bytes with a host that records hints and one that does
not.

**The extension's core** (`phitex-overleaf/core-partex`, its own repo):
`ph_open` with a trailing `stream` = 1 runs the setup and a first slice
and answers `{"done":false,"building":true,"handle",…}`; `ph_step(h,
budget_ms)` runs slices for about `budget_ms` and answers the pages
shipped so far (`"pages"`, `"hashes"`), the phase (`typesetting`,
`settling`), the pass, the time, and `"want"` (the hinted files resolved
to `engine TAB key`, those the session has not got), or, done, what
`ph_open` answered before with `first_page_ms` and `pages_at_first`.
While it streams, `ph_png` draws a shipped page, `ph_pages` and
`ph_status` answer from the pages so far, and nothing else builds. The
worker runs one queue: a request is served whole between slices, a slice
when no request waits (a `MessageChannel` turn, not a timer); an edit
or a file set while trip 1 runs waits, and is answered once its own build
has painted; at the end the PDF goes to the drawer before `settled`.

**Measured** (native, the extension's core on this machine, release,
every file given: the open with every cache warm; median of 3, alternating
blocking and streamed opens in fresh sessions; page 1 = drawn):
| project (pages) | plain: page 1, blocking → streamed | plain build | SSA: page 1, blocking → streamed |
|---|---|---|---|
| ieeetran (10) | 414 → 294 ms | 405 → 420 ms | 1360 → 781 ms |
| revtex (20) | 1199 → 721 ms | 1184 → 1222 ms | 6568 → 2204 ms |
| article (41) | 1046 → 561 ms | 1040 → 1076 ms | 4534 → 1531 ms |
| acmart (26) | 2869 → 1153 ms | 2828 → 2846 ms | 21000 → 3300 ms |
| longtail (62) | 3407 → 2058 ms | 3401 → 3524 ms | 8465 → 5150 ms |

Every streamed build's PDF is the blocking build's, byte for byte, the
clock pinned (`SOURCE_DATE_EPOCH`: unpinned, two sessions a minute apart
differ in the XMP date, as acmart's did once in four rounds). The
plain build is the extension's first paint (worker A); a streamed build
costs 1–4% more (the slices' clock and the page hashes). The `xelatex`
project needs XeTeX (or the extension's stand-ins) and is left out;
longtail's numbers are of a harness that missed `lmbx6.pfb` (its
discovery read the `.aux` of earlier builds, so a fresh build's bold `??`
was never asked for): the job ends fatally at the end, both ways alike.
A form not `\immediate` is shipped after the page that draws it (at its
first use, after the page object): a page not whole is hashed again when
forms come (`Shipments::forms`).

XeTeX's pages still come with its PDF (xdvipdfmx runs after the job).
`phitex watch` already streams (4.8): its viewer shows each page as it is
shipped, machine and `--ssa`, and now gives a whole page the hash its PDF
page will have.

---

## 5. Performance, observability and the text form

### 5.1 Numbers now

The 295-page course (`ch15.tex`, "expensive" → "dear"), release
binary, sandboxed (measured 2026-09-29):

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
  JSON. `phitex trace` writes them.
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
  Lua is a source of effects in the SSA (3.1): at first, every
  `\directlua` or callback makes its step unreusable; later, the Lua
  state is a versioned value like any other; all of Lua's I/O (`io`,
  `os.time`, `kpse`) goes through the Host, for wasm and determinism.
  Open: the interpreter must be LuaTeX's Lua 5.3 to the bit, float
  printing included (a port of its own, or a pure-Rust Lua made exact).
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

## 7. The φ core (2026-10-08, design for review)

`crates/phi` (package `phi`): a dataflow core that knows nothing of TeX.
The engine will be ported onto it (branch `pure-ssa` designs the TeX
side: ops, value kinds, node counts). It exists because chapter 3, as
built, records reads and writes around opaque chunks of an imperative
engine, and every attempt to make it finer drifted back there, because
the engine could reach around the model. In φ, **an op receives its
operand values and returns its results; it has no `&mut` to anything**,
so an impure op cannot be written against the API (7.9 says exactly
what the type system does and does not exclude).

The core is chapter 3's invariant made structural:

> A node runs again if and only if a version it read changed; the change
> stops at a node whose read results come out equal. The graph is built
> by evaluating it, and an edit rebuilds only what it reaches.

It depends on no crate of the workspace (a test fails if it does,
7.12), builds for `wasm32-unknown-unknown` with one thread, and has no
`unsafe`.

### 7.1 Overview

```text
 Seq (persistent, Poly-versioned) ──► input nodes ─┐
                                                   ▼
 Graph<L>: one arena of nodes, struct of arrays    scan / unfold nodes
   leaf  = op(operands) → value                    (steps emit children,
   step  = op(state, operands, names read) →        read and define names)
           (children, defs, next state)                  │
   effect / barrier / publish / cross                    ▼
 the name store (reaching definitions, scopes)    chains (ordered effects)
 the worklist (push, by position) ◄── edits        cross-run slots (μ)
 Executor: sequential | threads (rounds, validate, commit in order)
 Memo: content-addressed, Codec + Evict hooks      passes: DVE, CSE, fold
```

A client implements `Lang` (its value enum and op enum). The core
evaluates; the client never holds the graph during an op.

### 7.2 Values and fields (primitive 1)

```rust
#[repr(C, packed(8))] pub struct Ver(pub u128);    // partex-ssa's, copied
pub trait Value: Clone + Send + Sync + 'static {
    fn ver(&self) -> Ver;                           // O(1): cached in composites
    fn field(&self, f: u32) -> Option<Self> { None }
    fn field_ver(&self, f: u32) -> Ver;             // default: field(f)'s ver
}
pub enum Sel { Whole, Field(u32), Name(NameId) }    // what an operand reads
```

- A version is the content's: scalars by value, composites as a Merkle
  hash of their parts' versions made when the value is made (the stable
  128-bit hasher of `partex-ssa::hash`, copied, not depended on).
- A derived value's *memo key* is `Ver::node(op, operand versions)`
  (7.8). Its *version*, which cutoff compares, is its content's: a node
  whose operands changed but whose result is equal stops propagation.
- An operand reads `(node, Sel)`. When a node's value changes from `old`
  to `new`, each reader is woken only if `old.field_ver(f) !=
  new.field_ver(f)` for the field it reads (inside a region by the
  in-order sweep, across regions by reader lists, 7.11). The old value
  is held only during that comparison.
- Forces it: the page builder reads a box's height and depth, not its
  list; a glyph's metrics, not its identity; a step reads `\foo`, not
  the whole environment.

### 7.3 Nodes and purity classes (primitive 2)

```rust
pub trait Lang: Sized + 'static {
    type Val: Value + Default + Debug;
    type Op: Copy + Eq + Hash + Debug + Default + Send + Sync + 'static;
    fn eval(op: Self::Op, args: &Args<'_, Self>) -> Self::Val;                // leaf
    fn step(op: Self::Op, st: &Self::Val, args: &Args<'_, Self>,
            cx: &mut StepCx<'_, Self>) -> Step<Self::Val>;                     // 7.4
    fn scan(op: Self::Op, st: &Self::Val, x: &Self::Val,
            args: &Args<'_, Self>) -> (Self::Val, Self::Val);                 // 7.6
    fn scan_result(op: Self::Op, st: &Self::Val, outs: &Seq<Self::Val>, args: &Args<'_, Self>) -> Self::Val;
    fn as_seq(v: &Self::Val) -> Option<&Seq<Self::Val>>;                     // an input
    fn chain_val(items: Seq<Self::Val>) -> Self::Val;                         // a chain or family
    fn entries(op: Self::Op, input: &Self::Val, args: &Args<'_, Self>) -> Vec<Entry<Self::Val>>; // 7.4
    fn memo(op: Self::Op) -> bool { false }                                   // 7.8
    fn op_tag(op: Self::Op) -> u64;                                           // memo keys
    fn fmt_op / parse_op / fmt_val / parse_val                                // 7.14
}
pub enum Class { Pure, Effect(Chain), Barrier, Publish(Slot), Entry(Fam, Slot) }
pub struct Args<'a, L> { /* the node's operands, read through the core */ }
impl<'a, L: Lang> Args<'a, L> { pub fn get(&self, i: usize) -> Proj<'a, L::Val>; pub fn len(&self) -> usize; }
```

- A node is an op, a class, and operands. Kinds: *leaf* (`eval`),
  *input* (a value set by the client between runs), *scan* (7.6),
  *unfold* (a scan whose steps are nodes, 7.4), *step* (one step of an
  unfold).
- Classes:
  - `Pure`.
  - `Effect(c)`: the value is a payload on chain `c`, ordered within the
    chain by position (7.7). Evaluation is still pure; only the commit
    is ordered.
  - `Barrier`: orders every chain's effects around it.
  - `Publish(s)` and `Entry(f, s)`, with the `Cross` and `Family`
    kinds that read them: the cross-run slots (7.7).
- Dispatch is `L::eval`, a `match` over the client's op enum, so the
  engine is monomorphised over `L` and the hot path has no `dyn`.
- Forces it: `\write` and the PDF's bytes are effects on chains; a
  `\ref` reads what `\label` wrote last run; `hpack` and `line_break`
  are pure.

### 7.4 Dynamic construction and stable keys (primitive 3)

The graph is built by evaluating it. Only steps create nodes.

```rust
pub enum Step<V> { Next { st: V, key: u64 }, Done(V) }
pub struct StepCx<'s, L> { /* the step's emission buffer; brand 's */ }
impl<'s, L: Lang> StepCx<'s, L> {
    pub fn next(&mut self) -> Option<&L::Val>;          // consume an input element
    pub fn peek(&self, k: usize) -> Option<&L::Val>;    // consume() must follow a peek it depends on
    pub fn read(&mut self, n: NameId) -> Option<&L::Val>;   // a name at this position (7.5)
    pub fn emit(&mut self, op: L::Op, class: Class, args: &[Arg<'s>]) -> Local<'s>;
    pub fn emit_unfold(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s>;
    pub fn emit_scan(&mut self, op: L::Op, input: Arg<'s>, init: Arg<'s>, args: &[Arg<'s>]) -> Local<'s>;
    pub fn key(&mut self, k: u64);                      // the next emit's key (default: its ordinal)
    pub fn define(&mut self, n: NameId, v: Arg<'s>, global: bool);
    pub fn open_group(&mut self);  pub fn close_group(&mut self);
    pub fn name(&mut self, spelling: &[u8]) -> NameId;  // content-addressed (7.5)
    pub fn cancelled(&self) -> bool;
}
pub enum Arg<'s> { Local(Local<'s>), Up(u16), Name(NameId), Field(Local<'s>, u32), Lit(/* constant node */) }
```

- **The unfold.** An unfold node has an input (a `Seq`), an initial
  state and operands. Its first step runs on the initial state; each
  step consumes input elements (`next`), reads names, emits children,
  defines names, and returns the next state and the next step's key,
  or `Done(result)`. The steps are nodes, in a linked list under the
  unfold; a step's children are its own region. A macro call is a step
  that emits nothing: it rewrites the input, which is the state (the
  pending tokens) plus the core's cursor in the `Seq`.
- **Regions and scoping.** A step can only refer to what it holds: its
  unfold's operands (`Up(i)`), names (resolved by the core), and the
  `Local`s it emitted in this call. `Local<'s>` carries an invariant
  brand lifetime, so a handle cannot escape the call (it cannot be put
  in a `'static` value nor passed to another step). A node in a region
  is therefore read from outside only through names or through its
  step's state, so replacing a region never leaves a dangling edge.
- **Keys.** A node's identity is `(parent, key)`. A step's key is the
  client's (TeX: the stable id of the element where the step starts,
  with the expansion it came from); a child's is its ordinal unless the
  step gives one. When a step runs again, its new emissions are matched
  to its old children by key: a match keeps the old node's id, value
  and readers; it is re-evaluated only if its op or operands differ or
  an operand's version changed. Unmatched old children are removed.
- **Resync and convergence.** When a step runs again and returns
  `Next { st, key }`, the core compares with the old successor: if its
  key is the same, its input cursor maps to the same old element and
  `st` has the same version, the old successor and everything after it
  are kept (convergence: the rest of the region is not visited). If not,
  the core finds the old step with that key (a map per unfold), removes
  the old steps in between with their regions, and runs the new step.
  An unmatched key inserts a new step. So a flipped conditional, a
  macro that now takes another argument, or a paragraph merged with the
  next replaces exactly the steps between the edit and the point where
  the state comes out equal.
- **Input edits.** When an unfold's input `Seq` changes, the core diffs
  old and new (7.6: equal subtrees skipped by version), finds the step
  that consumed the first changed element (each step keeps the count it
  consumed and the core keeps a map from a step's first element to the
  step), and runs it again. Steps before it read nothing that changed
  and are not visited.
- **Keys decide reuse, never results.** A wrongly matched node is
  re-evaluated because its op or operands differ, and its readers are
  woken by version as always. So a key collision (keys are hashed to 32
  bits in the node table) costs work, not correctness, and the random
  edit tests check that.
- **Why this keying, against the alternatives.**
  - *Adapton* names thunks explicitly and globally, and dirties
    demand-driven: an edit marks every transitive dependent dirty
    before re-evaluating, with no cutoff during the marking. A preamble
    `\def` would mark the document. φ propagates by push in position
    order with cutoff at each node (self-adjusting computation's change
    propagation), and names are scoped to a parent, so uniqueness is
    local and an ordinal is a safe default.
  - *Salsa* keys queries by interned arguments and verifies by pulling
    from what is demanded: each revision walks the dependencies of the
    outputs (the PDF), O(document) per keystroke. Durability only
    coarsens that. It has no ordered program, no scoped names and no
    sequence reuse. φ takes from it backdating (early cutoff),
    interning (names), and the iterated cycle (7.7).
  - *Self-adjusting computation* (Acar) re-executes a changed
    function's interval and matches memoized calls inside it in order.
    φ's resync is that matching, with keys instead of call equality,
    bounded by convergence of the state instead of by the end of the
    interval, which is what TeX's sequential main loop needs.
- Forces it: macro expansion rewrites the input; `\if` reads one arm;
  `\csname` creates a name; an edit that flips a conditional replaces a
  subgraph and the rest of the document must be found again, not
  rebuilt.

**The state stays small.** Anything addressable by name is a name,
never part of a step's state. The state holds only what has no name:
TeX's mode, the pending input, the conditional and group depth. A
small state is what makes convergence come early and speculative entry
(below) predict exactly. `Config::debug` reports each unfold's state
size per step (`Value::bytes`, mean and max), so the client can be
held to the rule.

**Transient interiors** (checkpoint 3a, as built). A step's pure
leaves and constants are not nodes. The step runs into an emission
buffer, and only *big* emissions become its members:
- the sources of its definitions;
- whatever is not pure (effects, barriers, publishers, entries);
- creators (nested unfolds and scans), and the members they read;
- cross, chain and family reads.

The rest is evaluated in the buffer (most of it eagerly, as it is
emitted) and dropped. The step's own operands are its state before it,
the names it read, and every read its interior made from outside it,
each with the field it read. Any change to them runs the step again,
whole.

- *The rule on `Lang::step`:* a step's interior is bounded by
  construction. Anything that can grow with the input (a paragraph's
  words, a box's contents, a list) is a nested unfold, a scan or a
  sequence value, each a node with its own incremental behaviour.
  `Config::debug` reports each op's interiors (`Graph::interior_sizes`:
  steps, max, p99), so a client is held to it. The toy's and the bench
  client's interiors do not grow with paragraph length (test
  `interiors_are_reported_and_bounded`: 10, 100 and 1,000 words a
  paragraph, same max).
- *Nothing is lost at a step's edges.* Definitions' sources are members
  with values, so field-level versions hold across a step's boundary.
  A reader that wants one field of a name reads it with
  `Arg::NameField(n, f)`. Its operand's selector has two levels (the
  definition's own field, then the reader's), and re-resolution keeps
  the reader's level. The test `an_equal_width_page_change_stops_at_the_width`:
  a `\pageref`-like slot changes a digit at equal width. The step that
  shows the page runs again; a step two definitions on that reads only
  the width does not, with or without sealing.
- *Readers of what a step defines* are resolved again after its sweep,
  when a new source has its value. Before that, a new source read as
  the default value and woke its readers.
- *Check mode with interiors kept.* `Config::keep_interior` makes every
  emission a member (the text form then shows them). Check mode runs
  every live step again, dry, from the state before it. It compares the
  output state, the members in order (kind, op and value) and the
  definitions, so with interiors kept every leaf is compared (test
  `kept_interiors_check_every_leaf`, 12 random-edit seeds).
- *A step woken through its state* compares the new state with the one
  it ran from (`in_ver`), not with the old value of the node before it.
  So a step inserted before it that ends in the same state changes
  nothing for it.

**Speculative entry** (the unfold's parallelism). One chain is
sequential: on a cold build its critical path would be the whole
document. So the chain is also started at later points from predicted
states, and the real chain accepts each speculated suffix when it
arrives with the same state. That is the convergence check above,
reused.

```rust
pub struct Entry<V> { pub at: usize, pub key: u64, pub guess: Option<V> }
// in Lang:
fn entries(op: Self::Op, input: &Self::Val, args: Args<'_, Self>) -> Vec<Entry<Self::Val>>;
```

- *Proposals.* For an unfold built cold, or whose input changed past
  its old steps, the core asks `L::entries`: element indices where a
  step may start (TeX: paragraph or block boundaries of the source),
  each with the key the step there would have.
- *Predictions,* in order:
  - the state of the old step with that key (in a session the old
    steps are there; across sessions the memo store keeps
    `(unfold key, step key) → state`, 7.8);
  - else the client's `guess`;
  - else none, and the entry is not used.

  With names out of the state, the state at a paragraph boundary is
  small and often exactly the guess.
- *Segments.* Each used entry starts a *segment*: a chain from its
  predicted state that runs until its cursor reaches the next used
  entry's element. Workers run segments in parallel, each into a
  private buffer of steps and children (the graph is only read). A
  segment ends with the state it reached at the next entry, a better
  prediction for that entry than the guess.
- *Names read in a segment* resolve first in the segment's own
  definitions, else against the index as committed when the segment
  started (the last build's, or the segments committed so far). Each
  read records the definition it reached and its version (or
  "undefined").
- *Arrival.* Segments are committed in order. When the real chain (the
  committed segments before) reaches an entry:
  - same cursor and same state version: the segment is accepted whole.
    Its recorded name reads are checked again at their positions in
    the committed index. A read that now reaches a definition with
    another version wakes the step that made it, as a rebuild would.
  - otherwise the prediction was wrong: the segment's steps become the
    "old" steps of a rebuild. The first one runs again from the arrival
    state; children are matched by key, and the chain stops at the
    first step whose state converges with the segment's. A guess wrong
    only in what a paragraph's end resets costs one or two steps.
- *Rounds.* After a round, every segment whose arrival state is now
  known and differs from its prediction is run again, in parallel. The
  steps that the real chain reached decide the result, so the result
  is the sequential one (7.12). The test: a cold build of many
  independent paragraphs at W workers is faster than at one, and equal.

### 7.5 Names: reaching definitions with scopes (part of primitives 3 and 5)

A step's state is not an environment threaded through every step:
that is the cursor chain V8 left the sea of nodes for (3.9). Names are
an index of definitions by position, chapter 3.2's definition index made
generic.

```rust
pub struct NameId(u32);         // interned from a 64-bit content hash of the spelling
```

- `define(n, v, global)` at a step is a definition of `n` at that
  position: the value of node `v`. `open_group`/`close_group` are scope
  events at positions. A definition is *alive* at position `t` if it is
  global or its group has not closed before `t`.
- A read of `n` at `t` resolves to the latest definition before `t`
  that is alive at `t`: TeX's save stack, including `\global` inside
  groups (checked against §279–§283 in the tests). A group's end makes
  no node and no definition; it only ends lives.
- A read is an edge to the defining node with `Sel::Name(n)`, so it is
  woken when that value changes (field-level, 7.2). Inserting or
  removing a definition, or moving a group's close, re-resolves the
  readers of `n` in the affected range: those whose old definition's
  range it cuts.
- **The cost of a new definition** (a `\def` added early, shadowing
  readers of `\foo` across the document).
  - A definition's readers are kept as a run sorted by position, and a
    reader reaches its definition through the run.
  - Inserting a definition of `n` at `p` splits the runs of the
    definitions alive at `p`: at most one per open group level, so in
    practice one. The part after `p`, up to the next definition of `n`
    alive there, moves to the new definition. That is O(log R) for R
    readers, not O(R).
  - Readers are woken only if the new value's version differs from
    the old one's. A definition with an equal value costs the split
    and nothing else.
  - A different value wakes exactly the readers after `p` that it
    shadows. Each now reads something else, which is the edit's
    inherent reach, not overhead.
  - Removing a definition merges its run back into the one before.
    Moving a group's close splits or merges at the close.
  - *As built (checkpoint 2):* a name's readers are a vector sorted by
    their *anchor*, the top-level step or root node they are under,
    whose order never changes. A change at `p` binary-searches to the
    anchor of `p` and visits the readers after it, each re-pointed one
    by one: O(log R + readers after `p`). Splitting runs, so that
    readers beyond the next definition are not visited either, comes
    with checkpoint 3's memory work. Before this, every new definition
    visited every reader of its name: a cold build of 800 paragraphs
    took 5.6 s, and 0.25 s after.
- Name ids are interned from a stable 64-bit hash of the spelling. The
  table is the core's, never seen by an op, so the numbering does not
  affect results.
- Children read names statically: `Arg::Name(n)` resolves at the
  child's position. So `\the\count1` makes a node that depends on
  `\count1`, while the step that made it does not.
- Memory: the index holds every live definition. Folding steps (7.10)
  keeps only a run's net definitions (3.2's measurement: 95% are
  overwritten within 1,024 commands).
- Forces it: `\def`, `\let`, registers, catcodes as names; groups that
  open in one paragraph and close in another; `\global`.

**The name index, as built (checkpoint 3b).**
- *One record per definition.* `NameTab::recs` holds 32-byte records:
  name, step, ordinal in the step, source, selector, group and global.
  A step's definitions are a range of it, and each name's list holds
  4-byte indices into it, in position order. A step whose definitions
  change gets a new range; the old one stays until compaction rebuilds
  the records and remaps the lists. An import (a segment's name read
  from before it) is a record at the root that no step holds.
- *Lists in runs.* Each name's definition and reader lists are blocks
  of at most 256 entries with their start indices (`graph/runs.rs`).
  Finding a place is a binary search over the blocks and then inside
  one. Inserting or removing moves at most one block's entries plus
  the block starts after it. So a name read or defined a million times
  costs O(log R + 256) per change, not O(R).
- *Bounded re-resolution.* After a definition of `m` changes at `p`, the
  only readers that can resolve differently are those before the next
  definition of `m`, in a later anchor, that is alive to the end
  (global, or in no group): every reader past it reaches it or a later
  one. Only that run of readers is visited. A name with no reader after
  `p` costs one comparison.
- *Reader entries leave where they are.* When a node's operands change
  or it is removed, its entries are found by anchor and removed, rather
  than the whole list being pruned at the run's end.

### 7.6 Persistent sequences and scans (primitives 4 and 5)

```rust
pub struct ElemId(pub u64);
pub trait Measure<T>: Clone + Send + Sync { type S: Copy + Send + Sync; fn of(x: &T) -> Self::S; fn op(a: Self::S, b: Self::S) -> Self::S; const ID: Self::S; }
pub struct Seq<T, M = ()> { /* Arc'd B-tree, B = 32 */ }
impl<T: Value, M: Measure<T>> Seq<T, M> {
    pub fn splice(&self, at: usize, del: usize, ins: impl IntoIterator<Item = (ElemId, T)>) -> Self;
    pub fn get(&self, i: usize) -> Option<(ElemId, &T)>;
    pub fn ver(&self) -> Ver;                       // Poly: independent of shape
    pub fn measure(&self) -> M::S;  pub fn prefix(&self, i: usize) -> M::S;  // O(log n)
    pub fn diff(&self, old: &Self) -> Hunk;         // common prefix/suffix, equal subtrees skipped
    pub fn from_par<E: Executor>(items: Vec<(ElemId, T)>, e: &E) -> Self;
}
```

- The tree is `partex-ssa::pvec`'s (32-way, `Arc` nodes, the two-lane
  polynomial hash `Poly`, so a sequence's version is its content's
  whatever its edit history). Elements carry an `ElemId` the client
  assigns (deterministically), which is the identity steps and scans
  key by. The version does not include ids.
- Each tree node caches its measure, a monoid. Prefix sums are
  O(log n), splices update O(log n) summaries, and building the tree
  bottom-up over chunks is the parallel prefix (each subtree on a
  worker).
- **Scan** (`emit_scan`): `fold(L::scan, init, input)` with a state at
  every element. The scan node keeps checkpoints (the state after each
  element, every `k`th with a configurable stride) and its outputs, a
  `Seq` keyed by the input's element ids. After its input changes it
  diffs old and new, resumes at the checkpoint before the first changed
  element, and after the last changed element stops at the first
  checkpoint whose state has the old run's version at the same element;
  the rest of the outputs is the old run's. Its value is
  `(final state, outputs)`. The count of elements stepped is reported
  (the tests assert it).
- An unfold (7.4) is the same algorithm whose steps are nodes, consume
  any number of elements and emit children.
- Forces it: line breaking is a DP over a paragraph whose state (the
  active breakpoints) comes out equal a few lines after an edit; the
  page builder is a scan over the vertical list; `\count0`, footnote and
  section numbers are prefix sums; the main loop over the source is an
  unfold.

### 7.7 Effects, barriers and the cross-run fixed point (primitive 6)

- **Chains.** An `Effect(c)` node's value is a payload. The chain is
  the payloads of its live effect nodes in position order, kept as a
  sorted index per chain. Chains are unordered with respect to each
  other except at barriers: a `Barrier` node splits every chain into
  segments, and a sink receives segment by segment. Evaluation is never
  ordered by effects; only commit is (`Graph::chain(c)`,
  `Graph::segments()`).
- **Cross-run slots are per entry.** One slot per label, citation key,
  or TOC, LOF or index entry, keyed by the client (`Slot(u64)`, a hash
  of what it names), holding a value with fields (number, page, …). A
  book has 10⁴–10⁵ of them.
  - `Publish(s)` nodes give slot `s` its value for the next run: the
    last in position order, or absent.
  - A `Cross(s)` node reads slot `s`. Its readers read it by field
    (`Arg::Field`), so a changed page number wakes only the readers of
    the page.
  - `Entry(f, s)` nodes publish slot `s` and list it in *family* `f`.
    A `Family(f)` node reads the family as a `Seq` in the last run's
    order, each element keyed by its slot, and a scan over it resumes
    at the changed entry. So renaming one TOC entry runs one TOC line
    again; the test asserts exactly one.
  - A run checks only the slots and families whose publishers changed
    in it (`dirty_slots`, `dirty_fams`): an edit that touches no
    `\label` costs the cross-run loop nothing, however many slots
    there are.
  - Files like `.aux` are a write chain, committed at the end. They
    are not slots.

  A run:
  1. predicts each slot from the last run (in the session, or the memo
     store's persisted copy, 7.8);
  2. propagates to quiescence;
  3. compares each read slot's prediction with what was published;
  4. for each mismatch, sets the `Cross` nodes' value and wakes their
     readers: only the slice that reads the slot runs again;
  5. repeats until no slot changes, at most `max_iters` (default 5).
     If a slot still changes after that, the run ends with a
     diagnostic, `Report::oscillating`: each slot with the versions it
     took, in order, and the nodes that published them. A run that did
     not converge says so, and the result is never silently
     truncated.

  A correct prediction costs no extra iteration, and the tests count
  them.
- Forces it:
  - `\write` to `.aux`, `.toc` and `.idx`, and the log, are chains.
  - `\openout` truncation and `\shipout`'s order across streams are
    barriers.
  - Each `\ref`, `\pageref` and `\cite` target is a slot.
  - The `.toc` is a family of slots, one per entry.
- **Append lists, as built (3b).** LaTeX's hooks (`\AddToHook`) and
  expl3's `\tl_put_right` / `\clist_put_right` append to a list
  without reading it. They are effects on a chain whose identity is the
  list: the append's operand is the payload alone, not the list's
  version, so an append wakes nothing before it and reads nothing. A use
  reads the chain with `StepCx::chain_before(c)`: the payloads before
  the reader's position, in position order. (A `ChainRead` with the
  `BEFORE` bit reads up to its position; a plain one reads the whole
  chain.)
  - Chains are kept in position order (`chains: Map<u32, Runs<u32>>`,
    the same blocked runs as the name index, so an insert or a removal
    is O(log n)).
  - A changed payload wakes only the bounded readers after it
    (`wake_chain_at`). So editing one append re-runs that append's step
    and the uses after it, never another append and never a use before
    it.
  - A segment's chain reads (7.15) are marked dirty at the graft, since
    the segment saw only its own payloads.
  - Test: `an_append_list_reruns_only_what_follows_the_edit`. Forty
    appends with a use in the middle and one at the end; an edit to the
    30th append runs one hook evaluation and at most two steps, and
    equals a fresh build. The toy's `\addto{h}{w}` and `\usehook{h}`
    are in the random-edit test's snippets.
- No relocatable values. The core has no offset or relocation
  primitive. Positions are node identities (7.4), and what a client
  would relocate it recomputes from the identities.

### 7.8 The memo store (primitive 8)

```rust
pub trait Codec<V> { fn encode(&self, v: &V, out: &mut Vec<u8>); fn decode(&self, b: &mut &[u8]) -> Option<V>; }
pub trait Evict { fn touch(&mut self, k: Ver, bytes: usize); fn victims(&mut self, budget: usize) -> Vec<Ver>; }
pub struct Memo<L: Lang, E: Evict = Lru> { /* Ver -> (L::Val, bytes) */ }
impl<L: Lang, E: Evict> Memo<L, E> {
    pub fn get(&mut self, k: Ver) -> Option<&L::Val>;  pub fn put(&mut self, k: Ver, v: L::Val, bytes: usize);
    pub fn save<C: Codec<L::Val>>(&self, c: &C, out: &mut Vec<u8>);
    pub fn load<C: Codec<L::Val>>(&mut self, c: &C, b: &[u8]) -> Result<(), Error>;
}
```

- Keys are content: `Ver::node(L::ver_op(op), operand versions)` for a
  leaf, `(op, init version, element version, state version)` for a scan
  checkpoint. So entries are valid across builds and across documents
  (a paragraph broken once is broken for every document with the same
  paragraph and parameters).
- Probed only for ops that opt in (`L::memo`): a probe costs a hash and
  a lookup, more than a small op. Node identity (7.4) is what reuses
  work inside a session; the memo is for expensive ops and for cold
  starts.
- Eviction is a trait: LRU by bytes by default.
- Persistence is a byte image through the client's `Codec`, with a
  format version; a decode error drops the image, never a build.
- Forces it: fonts, a TikZ picture, the line breaking of an unchanged
  paragraph, a cold start after a restart.

As built (3b, `crates/phi/src/memo.rs`):

- `Memo<V, E: Evict = Lru>` over the value type; the graph holds one
  behind a mutex, shared with the segments of a parallel cold build and
  read by dry steps (7.12), so a worker's miss fills it for the others.
  Off (budget 0) by default: `Graph::set_memo_budget`, `memo_stats`
  (entries, bytes, probes, hits), `save_memo` / `load_memo` (a bad
  image leaves the store empty, `MemoError`).
- Probed at every place a leaf is evaluated: emitted with ready
  operands, evaluated in its step, re-evaluated in a sweep or on its
  own. A leaf's key is `Ver::node(op_tag, operand versions)`, made
  without allocating for up to 8 operands.
- Scans: per element, two entries (the state after, the output), keyed
  by `(op_tag, state, element, arguments)`. The scan's own convergence
  (7.6) still decides where it stops; the memo only saves the element's
  op.
- `Lru`: a use stamps its entry; victims are sorted only when the store
  is over budget, and it then frees an eighth of the budget beyond the
  excess, so eviction is amortized.
- The check (7.13) evaluates raw, never through the store, so a wrong
  entry would be caught.
- Cost: a probe is about 90 ns (lock, key hash, two lookups; `prof`
  with every Add memoized: 123.7 -> 201.3 ns/node over 2M probes,
  99.95% hits). Worth it for ops above about 0.2 us, which is what
  `Lang::memo` is for.
- Tests: `a_memo_image_spares_a_fresh_build_its_memo_ops` (a fresh
  graph with another's image evaluates no hook use and no line breaker
  element, and equals a build without the store; an edit then steps
  only the edited paragraph's words; a truncated image errs and leaves
  the store empty; a 256-byte budget holds), and the parallel document
  of the random-edit test runs with a 4 KB store (hits, misses and
  evictions, compared with the sequential build after every edit).

### 7.9 Purity, by type

- An op's whole input is `Args<'_, L>` (shared references to operand
  values) and, for a step, `&mut StepCx`, which only buffers emissions
  and resolves reads through the core. No op signature mentions the
  graph, the store, the worklist or another node's state; there is no
  way to name them.
- `L::Op: Copy` excludes owned interior mutability (`Cell`, `RefCell`
  are not `Copy`). `L::Val: Send + Sync` excludes `Cell`/`RefCell` in
  values.
- `Local<'s>` is branded: emitted handles cannot escape their step (no
  edges to nodes out of scope).
- What the types do not exclude: a `static` with a `Mutex` or an
  atomic, or a value holding an `Arc<Mutex<_>>`. Those are reviewed
  away, and *check mode* (`Config::check`) evaluates every node that
  propagation would reuse and asserts its value is equal, naming the op
  that is not a function of its operands. The property tests run with
  it on.

### 7.10 Passes (primitive 9)

Each pass preserves the graph's results; each fold says why it adds no
dependency the unfolded graph lacks.

- **Dead values.** A node with no reader, no definition, no chain or
  slot and not its step's result keeps its structure (for matching) and
  drops its value. No result reads it, so none changes.
- **CSE.** Within one region, pure nodes with the same op and the same
  operands (node and `Sel`) are merged; the second's readers read the
  first. By purity their values are equal now and after any edit (they
  read the same operands), so every reader sees the same versions.
  Across regions the memo shares results instead, without merging
  identities.
- **Fold.** A *single-entry, single-exit* set of pure nodes in one
  region (no member read from outside but the exit, no definitions,
  effects or children) becomes one node holding its members' ops and
  local wiring (about 16 bytes a member, against 7.11's ~70). Its
  operands are the union of the members' outside operands.
  *Argument:* the folded node reads exactly what its members read, so
  every change that wakes it woke a member before; a member's value
  was read only inside the set, so no reader loses an edge; the fold's
  value is the composition of pure functions, equal to the exit's.
  Cost: an operand change re-runs the whole set where one member might
  have sufficed (interior cutoff is lost), never a wrong value.
- **Step fold.** A run of consecutive steps whose intermediate states
  are read only by the next step becomes one step: consumed counts
  added, reads unioned, children concatenated, and only the run's *net*
  definitions kept. *Argument:* a definition overwritten (or ended by a
  group's close) inside the run reaches no position after the run, so
  only members read it; the run's reads from outside are the union of
  the members'. Cost: a change re-runs the run from its start.
- A fold is undone (unfolded) when its node runs again more than
  `refold_after` times in a session: granularity follows edits.

As built (3b). Three of the four passes are not separate passes: the
structure that 3a built does each one where it costs nothing, with the
same argument. CSE is the one pass left, and it is built.

- **Dead values: 7.4's transient interiors.** An emission that no one
  outside its step reads (no definition, effect, slot, creator or
  outside reader) never becomes a node. Its value lives in the sweep's
  buffer and is dropped after the sweep. The emissions that remain nodes
  are the ones something outside reads, or may read: a definition's
  value is kept even when no reader reaches it yet, because a reader
  that appears later, and the cutoff, compare against its version.
  Nothing a result reads is dropped, so no result changes.
- **Fold: the step's interior.** A step's interior is single entry
  (its state and the reads it records) and single exit (its
  definitions and big emissions), and it is pure where it is
  transient. A woken step runs its interior again whole: that is the
  fold, and its cost (no interior cutoff) is the cost the design
  accepts. `keep_interior` is the unfolded form. Both build the same
  results; the property test checks both.
- **Constant folding: at emission.** A leaf whose operands are all
  known when it is emitted is evaluated then (`Spec::done`), so no node
  or sweep work is left for it.
- **Step fold: 7.11's sealed regions.** Net definitions only, reads
  unioned, consumed counts added. The refold rule is the unseal: a woken
  sealed run unseals, and is sealed again only once it has been quiet
  for `seal_quiet` runs.
- **CSE: built, per step, opt-in per op (`Lang::cse`).** When a step
  emits a pure leaf equal to one it emitted before (same op, aux and
  operands), the earlier emission is returned and no new one is made. An
  operand that reads a name counts as equal only if the step made no
  definition and no group event between the two reads, so both resolve
  to the same definition. *Argument:* the two leaves read the same
  operands, so by purity their values are equal now and after any edit;
  the readers of the second read the first, and no edge is lost. The
  merge is a function of the step's emissions only, so a fresh build
  and an edited one merge alike (the random-edit test compares them
  with the `\twice` command in its snippets). Hash collisions are
  checked against the operands, never trusted. `Report::merged` counts
  merges. Cost: about 20 ns a probe (`prof`, every Add probed, no
  merges: 116 -> 133 ns/node over 10M probes). That is why CSE is opt-in
  per op: a client turns it on for ops it emits twice in a step.
  Across steps and regions the memo store (7.8) shares results instead.
- The property test found a core bug here: in a segment (7.15), a leaf
  that read a name after its own step defined it, while that definition
  was still pending, was left for the sweep. The sweep resolved it at
  the step's position and recorded a read of the step's own definition
  as an outside operand. The values were right, but the operands were
  not the sequential build's. The sweep now resolves a name against the
  step's own definitions first, at the emission's place
  (`Emit::own_def_at`), as `eval_now` does.

### 7.11 Memory layout and cost

As built (checkpoint 2), the graph is one arena indexed by `NodeId(u32)`:
a header per node, its value in a column beside it, and two shared
arenas.

| | bytes | |
|---|---|---|
| header `Hdr` | 64 with an 8-byte op | `ord: u64` (label among siblings), `aux: u64` (table index, chain or slot), `op`, `parent`, `next`, `first`, `key` (hashed, for matching only), `a0` (first operand), `rd` (reader list head), `era`, `an: u16`, `depth: u16`, kind, class tag, flags |
| `val: L::Val` | client's (the bench's: 32) | |
| operand `(src, sel, name)` | 12 each | `name` set when it was resolved from a name |
| reader entry `(node, era, next)` | 12 per edge | singly linked from the source; stale entries (an older `era`) unlinked when met |

- The header was columns first: 16 vectors, about 20 cache lines
  touched per node made. One 64-byte header is one line, and a cold
  build of 1 M leaves went from 122 ms to 93 ms.
- **Every edge has a reverse entry**, which is not what 7.11 first
  said. The design had a region swept in order instead, but a client's
  root region can hold millions of nodes, and the sweep would make a
  one-leaf edit O(region). With reverse entries everywhere, a one-leaf
  edit of 1 M is 93–114 ns.
- Positions are Dewey paths (`parent`, `ord`), compared in O(depth).
  Steps' labels have gaps of 2³², and an insertion where there is no
  room relabels that unfold's steps.
- No allocation per node on the hot path: the header and value columns
  grow by doubling; emissions, operands and ids go through scratch
  buffers that are reused; the core's maps hash with a multiply-rotate
  hasher, not SipHash. A step that defines names allocates its
  definition list (a `Vec`), the remaining per-step allocation.
- Measured (7.16): 144 B per root leaf with two operands (64 header +
  32 value + 24 operands + 24 reverse entries) and 206 B per node of
  an unfold with ten leaves a step. That is above "tens of bytes".
  The retained tiers below are the answer.
- Targets: tens of ns per node evaluated (leaf, cold, sequential), tens
  of bytes per unfolded node, 10⁸ nodes at checkpoint 3.

**Retained memory** (designed before checkpoint 3). At 64 B a node, 10⁸
nodes are 6.4 GB: too much for a laptop or wasm. Once evaluated, most of
the graph is kept only for rebuilds, and a rebuild needs much less than
the evaluation made. Three tiers:
- *Live:* every column. The regions an edit touched lately, and
  anything with a cross-region reader still being propagated.
- *Version-only leaves.* A pure, cheap leaf (its op says so) drops its
  value and keeps its op, its operands and an 8-byte version. When it is
  read, it is recomputed from its operands, recursively through other
  version-only leaves, so a fold bounds the depth. When it is woken,
  the new value is compared with the kept version. 64 bits suffice for
  this comparison: a false "equal" needs a 2⁻⁶⁴ collision between a
  node's own successive values. The memo's keys stay 128-bit. About
  24 B a leaf.
- *Sealed regions.* A step whose region is done (its steps folded,
  7.10) keeps only:
  - its entry state (for re-entry);
  - its net definitions;
  - its outside reads (name, the definition reached, version) for
    waking;
  - its effects' payloads and its output state.

  The interior is dropped. When the region is woken it runs again from
  its entry state, keys matching nothing (there is nothing to match),
  and it can be sealed again. A paragraph of 1,000 command nodes keeps a
  few hundred bytes, under 1 B per original node.
- *Target:* at most 16 B per original node retained on average, live
  tier included, and below 2 GB for the course. This is measured
  against the TeX layer's node counts when they come; the core reports
  bytes by tier (`Graph::mem`).

**Sealed regions, as built (checkpoint 3a).** `Config::seal = F`
turns sealing on. A sealed region is the first step of a run of a root
unfold's steps, flagged and holding:
- its entry: the first step's key, cursor, group and the version of the
  state it ran from;
- its output state (as its value);
- its net definitions, with their sources kept as its members;
- its reads from outside (deduplicated, each with its field);
- its input length and its steps' and emissions' counts.

Everything else of the run is dropped, and compaction frees it.

- *Which runs.* A run holds only steps whose members are pure leaves
  and constants. An effect, a publisher, a creator or a cross read
  ends it, so a paragraph's end with its line breaker stays live.
- *Cuts.* A run is cut only where the group in force is the one it
  started in. Then no group the run opened is still open after it, and
  no group open before it closes inside it. Groups opened and closed
  inside are dropped with it. Among those balanced points a cut is made
  where the step's identity (key, cursor) hashes to one in F
  (content-defined, so a fresh build and an edited one cut alike).
- *The bound* is in emissions, not steps: `Config::seal_max = 4096`.
  Rationale: a first edit in a sealed region re-runs it whole. At the
  measured 60–80 ns per emission that is at most about 0.3 ms, under 2%
  of a 16.7 ms frame, leaving the frame to the edit itself and to
  layout. Measured on the bench client (F = 16, 100 k steps): runs of
  16 steps on average, p99 65, max 117; 178 emissions on average, max
  1,287.
- *A woken region unseals.* It runs again as its first step (`run_step`
  clears the flag), and the steps after it are made again until one
  meets the step that followed the region. Those steps are live and
  stay live: a step that ran after the first build is sealed again only
  after `Config::seal_quiet = 16` runs without running. A queue of such
  steps, by the run they become due, drives a local pass: back to the
  stretch's start (through each step's state operand) and forward to
  the next sealed region. So only the first edit in a region pays for
  it. The next edit there re-runs exactly the steps it would with
  sealing off (test `a_second_edit_in_a_sealed_region_is_step_precise`:
  equal step and evaluation counts).
- *Fields at the edge.* A region's net definitions keep their sources'
  values, so a reader of one field of them is woken only when that
  field changes (the equal-width `\pageref` test, sealed).
- *The name index.* A whole pass rebuilds each touched name's
  definition and reader lists in one merge. A local pass splices the
  run's range (from its first step to the node after it) in place.
  Readers that land out of order during a run wait in a per-name buffer
  and are merged once per run, not shifted in one by one.
- *Compaction* (`Graph::compact`, run after sealing when at least half
  the node table is dead): live nodes are moved into holes from the
  top, the tables truncated, and the operand, reverse-edge and
  definition arenas rebuilt from the live nodes. Root-level nodes never
  move (the client holds their ids). Group ids embed their opening
  step's id: they are remapped, and a dead step's slot that still names
  a referenced group is not reused.
- Equality: with sealing on (F = 1–4) and compaction after every run,
  40 random-edit seeds give the same observable results as fresh
  unsealed builds checked in full (`sealed_random_edits_equal_fresh_builds`).
  The text form can differ: where a fresh build cuts and where an
  edited one re-sealed are both valid.

### 7.12 The scheduler (primitive 7)

- **Sequential** (the default, and wasm's): a cold build evaluates in
  creation order, which is topological, with no queue. A rebuild pops
  the worklist (a binary heap by position): a region sweep from its
  first dirty child, a step to run, a scan to resume. In the acyclic
  part each node runs at most once per change.
- **Parallel** (`Executor` with width > 1, std threads, never on wasm):
  rounds, as Block-STM runs a block.
  1. Take the earliest dirty items up to a work budget.
  2. Workers evaluate them against the graph as it is (shared, read
     only during the round), each item locally in order, recording every
     outside version it read (operands, names with the definition they
     resolved to). Items further on are *speculative*: they use values
     an earlier item may change.
  3. The coordinator validates outcomes in position order as they
     arrive, against the changes accepted earlier in the round: an
     outcome whose read versions still hold is accepted, else its item is
     requeued. Accepted outcomes are applied in position order at the
     round's end (values, matched or new nodes, definitions, wakes).
  4. *Cancellation:* when an accepted outcome changes a value that an
     in-flight item reads statically, that item's flag is set; long ops
     (scans, unfolds) poll `cancelled()` between steps. A new edit bumps
     an epoch that cancels every item.
- **Determinism.** Every op is a function of its operands, every
  accepted outcome read the versions current at its position, and
  every later change re-wakes its readers; in the acyclic part the
  fixed point is unique, so any schedule ends in the sequential result.
  The order only changes the wasted work. Tests compare the two runs'
  values, chains and slots.
- **Cold builds** are wide through speculative entry (7.4): round one
  runs a segment per entry on the workers, and the commit walks them in
  order, accepting the segments whose arrival state matches.
- Forces it: a preamble `\def` wakes thousands of paragraphs at once;
  a cold build is wide only if later chapters run from predicted states
  (the last build's), validated as the chain reaches them; a keystroke
  cancels the background convergence.

As built (3b, `graph/par.rs`). The rounds are simpler than the plan
above: workers never apply anything. The coordinator stays the
sequential scheduler, and an outcome only replaces running an op.

1. When the heap's top is a dirty step, a round takes it and the dirty
   steps after it along its unfold's step list (a walk, not heap
   pops), up to the round size.
2. Workers (`std::thread::scope`) run each step's op *dry*: against the
   graph as it is, read only, into an emission buffer from a pool. Each
   records what it read from outside, with versions: its state, the
   unfold's operands, every name it read with that name's definition
   generation (`NameTab::gens`, bumped by any insert or remove of a
   definition of that name), every node operand, the group table's
   generation, the name count, the input's version and the step's
   position.
3. The sequential run goes on as before. When it reaches a step with an
   outcome, the outcome is used if all of these still hold (`holds`);
   otherwise the op runs again. Either way the step is then matched,
   swept and woken as before.

- *Correctness* is the sequential run's, by construction. A used
  outcome is the op's result on exactly the inputs the sequential run
  would give it, because an op is a function of what it reads, and
  every read is recorded and checked.
- *Round size* adapts. It starts at 64 steps a worker and doubles, up to
  4096 a worker, while at least 90% of a round's outcomes are used;
  otherwise it halves.
- *Gating:* one step in 32 is timed, and rounds run only when the mean
  op cost is at least `Config::round_min_ns` (default 10 us). A cheap
  op costs less to run in turn than its outcome costs to hand over:
  thread start, allocation, cold cache.
- *Measured* (`prof` with `PRE` and `HEAVY=20000`, about 75 us an op; a
  preamble edit re-runs 20k steps): 1512 ms sequential, 483 ms at W=4,
  263 ms at W=8.
- *Tests:* the random-edit test keeps a third document with
  `workers = 2`, `round_min_ns = 0` and a 4 KB memo store. It is
  compared with the sequential one after every edit, and asserts that
  outcomes were used.

Cancellation, as built: `Graph::cancel_token()` gives an
`Arc<AtomicBool>` that another thread may set. The run polls it between
work items, workers poll it between dry steps, and a long op may poll
it through `StepCx::cancelled()`. A cancelled run returns at once with
`Report::cancelled`. Its remaining work stays queued (dirty flags and
the heap are untouched), the flag is cleared, and the next `run`
finishes the job. The result equals an uncancelled run's
(`a_cancelled_run_leaves_its_work_for_the_next`).

### 7.13 The acceptance client and the tests

A toy TeX-like language in `crates/phi/tests/toy/` (not TeX):
- tokens are words and `\commands` in a `Seq`; the document is an
  unfold over it;
- `\def\x{body}` and macros that rewrite the input; `\csname`;
- `\if<a><b> … \else … \fi` with only the taken arm read, as a nested
  unfold, and a φ node at the join for each name the arm defined;
- `{`/`}` groups with local definitions and `\global`;
- a running counter (`\step`), and the same as a prefix sum;
- words become a paragraph `Seq`, broken by a scan (a minimum-raggedness
  DP), whose lines feed a page scan;
- `\write{out}`/`\message` effects on two chains, a barrier;
- `\label{k}` publishes, `\ref{k}` reads the slot.

Tests (seeded PRNG, as the repo's other tests; no `proptest`):
- after random edit sequences (words, macros, conditionals flipping,
  names appearing and disappearing, groups moving), every node value,
  chain and slot equals a fresh build's;
- parallel equals sequential, at several widths and seeds;
- a field nobody reads changes: zero downstream evaluations, asserted
  exactly;
- a scan resumes at the edit and stops at convergence: the elements
  stepped, asserted exactly;
- a correctly predicted slot costs zero extra iterations;
- `deps`: `crates/phi/Cargo.toml` has no path or workspace dependency
  (no TeX or PDF crate can creep in).

Benches (criterion, `crates/phi/benches/phi.rs`): build+evaluate N
leaves; rebuild after one leaf edit; scan resume; parallel scaling;
memory per node (counted, not timed).

### 7.14 The text form

Every IR has a canonical text form that round-trips (5.3), and so does
the graph. `Graph::to_text` prints one node per line in position order,
indented by region:

```text
%12 = step #a41f(%3, @foo:%7) ; st=…        key, operands, names read and where they resolved
  %13 = leaf add(%12.1, 4) = 7               a child: op, operands (.f a field), value
  def \foo = %13 [g2]                        a definition and its group
```

Ops and values print and parse through `Lang` hooks (`fmt_op`,
`parse_op`, `fmt_val`, `parse_val`). `Dump::parse(to_text(g))` gives
the same text and values again, which a test checks after random edits.
It is the debugging view, and it comes with checkpoint 2.

### 7.15 Speculative entry, as built

- A segment is a private `Graph` run on a worker. Its root holds the
  unfold's input, a guessed initial state and the arguments, and its
  unfold starts at the entry's element and key and stops at the next
  entry's element.
- Names the segment does not define are *imports*. Their values come
  from a snapshot of every name as the main graph has it where the
  segments start (`ext`, shared by all workers). An import is an input
  defined at the segment's very start, so a local definition shadows
  it.
- Grafting copies the segment's steps and their subtrees into the main
  unfold after its last step, remapping:
  - node ids, group ids (which embed the opening step's id), name ids
    (by spelling), and the operands that pointed at the segment's root
    inputs (to the unfold's own operands);
  - each operand that read an import or an undefined name, resolved
    again at its place here. If the version differs, its reader is
    woken: that is the validation of reads made during speculation.
- Segments are grafted in order as they finish, while workers run the
  rest. Each boundary is then checked by the ordinary successor check.
  A segment whose chain ended (`Done`) ends the unfold there.
- Segments keep the main graph's slot and family predictions and never
  run the cross-run loop themselves.
- *Bulk-append grafting* (checkpoint 3a). The worker that ran a segment
  also packs it (`Graph::pack`). The nodes under its unfold are
  renumbered from 0 in their order, every reference inside is made
  relative, and values and the kinds' tables are moved out. What
  crosses the segment's edge is listed apart:
  - its operands that are the unfold's;
  - its reads by name from before it, with the version each read;
  - its readers and definitions by name.

  The main thread appends: headers with ids shifted, values extended,
  each name's readers and definitions appended in one go when they come
  after the list's last. Then it fixes the listed edge operands, the
  first step's state operand and the top steps' labels. The first
  segment's sizes reserve the tables for the rest. The graft costs
  about one store per byte the main graph keeps; that, with the page
  faults on fresh table memory, is what still bounds W = 8 (7.18).
- The parked step's group is remapped as well. Before that fix, a
  mismatched arrival resynchronised under a group id of another step,
  and a local definition outlived its group (found by the random test
  with three workers).

### 7.16 Measured (checkpoint 2)

Local and on a loaded machine, so ratios are indicative; accl numbers
come at checkpoint 3. Criterion benches are in `crates/phi/benches/phi.rs`.
The bench client: `i64` values versioned by value, `Add` leaves, an
unfold whose steps emit ten leaves and define one name.

| bench | time | per node | target |
|---|---|---|---|
| build 1 M root leaves (two operands each), cold | 93 ms | 93 ns | tens of ns: missed |
| build an unfold of 100 k steps × 12 nodes, cold | 263 ms | 220 ns | missed |
| rebuild after one leaf of 1 M changed | 0.11 µs | — | met |
| rebuild after one step of 100 k changed (1.2 M nodes) | 8.5 µs | — | met |
| scan resume after one element of 1 M changed | 28 µs | — | met |
| memory, root leaves | 144 B/node | | tens of B: missed |
| memory, unfold k = 10 | 206 B/node | | missed |

- *Cold build.* Where the cost goes (perf, root leaves):
  - set_opds 21% (operand and reverse-entry writes);
  - alloc 11%;
  - eval and set_val 20%;
  - page faults on fresh memory 10%.

  It is memory traffic: 144 B per node written. A step adds its own
  emission, key matching and definitions.

  Reaching tens of ns means writing fewer bytes:
  - a 32-byte header (`first` only for parents, `key` only under steps);
  - 8-byte operands, with the name in a side table;
  - reverse entries only where a reader is outside its source's step.

  This goes with the retained tiers.
- *Parallel cold build.* The toy client, 800 paragraphs at 4 workers:
  0.25 s → 0.115 s (2.2×), every segment accepted. With the bench
  client, whose steps are as cheap as grafting a node, there is no
  gain at 2–8 workers (209 ms → 216–233 ms): grafting copies each
  node, and on one thread that costs about as much as making it.
  Grafting by bulk column append (segment ids are contiguous: an
  offset, not a map) is the checkpoint-3 fix.
- *Rebuilds* are where the design pays. A changed element of a 100 k
  step unfold runs two steps, and a changed element of a 1 M scan steps
  to convergence, independent of size.

### 7.18 Measured (checkpoint 3a)

On accl (EPYC 7763, one node, 16 CPUs). The bench client is as in
7.16: a node is a logical node (each step's ten or twenty leaves, its
constant and the step), whether or not it is kept.

| | k = 10 | k = 20 | target |
|---|---|---|---|
| cold build, 100 k steps, sequential | 84.9 ms, 70.8 ns/node | 117 ms, 53.3 ns/node | ≤ 50 ns: missed |
| W = 2 | 48.3 ms (1.76×) | 64.5 ms (1.82×) | |
| W = 4 | 25.1 ms (3.38×) | 35.6 ms (3.29×) | ≥ 2.4×: met |
| W = 8 | 23.7 ms (3.58×) | 24.9 ms (4.71×) | ≥ 4.8×: missed |
| live memory | 29.7 B/node | 16.2 B/node | ≤ 32: met |
| retained (sealed, F = 16, compacted) | 1.7 B/node | 0.9 B/node | ≤ 16: met |
| sealing pass | 12.9 ms (10.8 ns/node) | 12.4 ms | |

- The toy client, 800 paragraphs: 88.7 ms in turn, 27.7 ms at W = 4
  (3.20×) and 15.7 ms at W = 8 (5.67×), equal to the sequential build.
- At 1 M steps (k = 10): 74 ns/node cold, 1.7 B/node retained,
  sealing 13.2 ns/node.
- *Sealed runs* (k = 10, 100 k): 6,177 runs, 16.2 steps on average,
  p99 65, max 117; 178 emissions on average, max 1,287.
- *Edits*, each changing one input element:
  - with sealing off: median 3 µs (2 steps);
  - a first edit in a sealed run: median 102 µs, max 487 µs (28 steps
    on average);
  - a second edit beside it: median 8 µs.

  At 1 M steps the first edit's median is 839 µs and its max 45 ms. The
  name index's sorted lists shift on each insertion, which is O(list):
  that is the checkpoint-3b item "reader runs in O(log R)".
- *Trade-off of transient interiors* (criterion, accl):
  - one changed step of 100 k: 5.13 µs at checkpoint 2, 3.86 µs now;
  - a name read by one leaf of a ten-leaf step: 3.78 µs (7.6 µs
    locally before);
  - one leaf of 1 M root leaves: 62 → 59 ns.

  The cost is that a changed leaf re-runs its step: ten leaves, not one.
- *Interiors* (`interior_sizes`): the bench client's step has 10
  transient emissions (max and p99). The toy's are the same for 10,
  100 and 1,000 words a paragraph.
- *What bounds the misses.*
  - Cold build: about 6,700 instructions a step at k = 10 (accl IPC
    about 2.5). A leaf costs about 450 instructions (emission record,
    eager evaluation, sweep visit). A step's fixed part is about 2,000:
    three name lookups, a node and its step record, its definition,
    operands, reverse edges and a reader entry. Page faults on fresh
    table memory are 5–10%.
  - W = 8: the main thread's graft, about 22 ms per 100 k steps. It
    writes about 390 bytes a step: two 64-byte headers, a 64-byte step
    record, values, operands, reverse edges, and a definition kept
    twice (step record and name index, 40 bytes each). The next steps
    are the byte diet: one definition record indexed by the name
    lists, a 40-byte step record, and narrower headers. They cut the
    graft and the cold build alike.

### 7.19 Measured (checkpoint 3b, the name index and the diet)

On accl, the same benches as 7.18.

| | before (7.18) | now | target |
|---|---|---|---|
| first edit in a sealed run, 1 M steps, max | 45 ms | 1.09 ms | ≤ 2 ms: met |
| the same, median | 839 µs | 132 µs | |
| second edit beside it, median | 40 µs | 9 µs | |
| cold build k = 10, sequential | 70.8 ns/node | 71.9 ns/node | ≤ 50: missed |
| cold build k = 20 | 53.3 | 56.0 | |
| W = 4, k = 10 | 3.38× | 3.29× | |
| W = 8, k = 10 | 3.58× | 6.29× | ≥ 4.8×: met |
| W = 8, k = 20 | 4.71× | 6.96× | |
| retained, 1 M steps sealed | 1.7 B/node | 1.5 B/node | |

- *The 45 ms* was not the name lists alone. At 1 M steps `Seq::diff`
  fell back to element-by-element comparison whenever a splice had
  changed a subtree's shape (69 ms for one edit). It now walks both
  trees with a stack per side and skips subtrees shared by pointer at
  any depth. A splice copies only its path, so that is O(log n). The
  rest was the O(R) list shifts (now runs) and the first edit after a
  compaction rebuilding the step index (compaction now keeps it).
- *W = 8:* the graft lost its per-read re-resolution: a segment's reads
  from before it all resolve as at its start, so each name is resolved
  once. Definitions now cost one 32-byte record plus a 4-byte index
  instead of 72 bytes.
- *Cold build:* 8,600 instructions per k = 10 step, at IPC about 3 on
  accl. Where they go:
  - emissions: about 320 each (12 per step: records, eager
    evaluation 80, sweep visit 36);
  - the step's own work: about 2,000 (its node and record, the
    successor's, operands and reverse edges, a definition, a reader
    entry, three name lookups at 125 each).

  50 ns/node would mean about 6,000 instructions a step. The remaining
  cuts are each a few percent (smaller emission records, a sweep that
  skips leaves done at emission, a cheaper `Args::get`), so it is not
  pursued until the TeX layer's step shapes say which costs matter.

### 7.20 Measured (checkpoint 3b, the rest: 10^8 nodes)

On accl (acclnode01, 16 CPUs), `prof` at 64e1af5. K10 is 12 nodes a
step and K20 is 22, so 8.4M K10 steps and 4.6M K20 steps are each
about 10^8 nodes.

| | K10, 8.4M steps | K20, 4.6M steps |
|---|---|---|
| cold, W=1 | 74.9 ns/node, 7.55 s | 57.6 ns/node, 5.83 s |
| cold, W=8 | 30.2 ns/node, 3.04 s (2.48x) | 15.8 ns/node, 1.59 s (3.65x) |
| peak RSS, W=1 / W=8 | 2.74 / 4.16 GB | 1.50 / 2.06 GB |

- Sealing all 8.4M K10 steps takes 1.85 s (18.3 ns/node) and makes
  519,252 runs (mean 16.2 steps, max 117). Retained: 1.5 B/node.
- First edit in a sealed run at 10^8 nodes: median 74 us, max 250 us
  (30 steps). Second edit beside it: median 7 us, max 116 us. At 1M
  steps: median 54 us, max 263 us. The edit cost no longer grows with
  the document.
- W=8 at 10^8 is below the 100k-step figure (6.29x, 7.19). The segments
  run in parallel, but the coordinator grafts them one after another,
  and at this size the graft and the memory bandwidth dominate. A
  parallel graft is the next lever if the 10^8 cold build matters.
- Preamble edit with parallel rounds (20k steps re-run, about 29 us a
  step): 589 ms at W=1, 199 ms at W=4, 111 ms at W=8 (5.3x). Every
  outcome was used (19,999 of 19,999).
- Memo, every Add memoized (1M steps): 136.6 ns/node against 77.5, at
  10M probes and 99.99% hits. A probe costs about 60 ns on accl.
- CSE, every Add probed (1M steps): 90.8 ns/node against 77.5. A probe
  costs about 13 ns on accl.

What the 10^8 runs found, all fixed:

1. **Reader lists rewrote their block starts on every change.** That
   is O(R/256) per insert or remove, so a name read by every step made
   each edit linear. perf at 4M steps: 37% in it. The starts are now a
   Fenwick tree over the block lengths. First edit at 10^8: median
   1148 -> 74 us.
2. **Compaction cut every table's capacity to its length.** The first
   edit after a seal then moved a whole table (12 ms at 10^8 nodes).
   Compaction now leaves an eighth of room, as address space that is
   not resident until used, and touches the next 2 MB at the seal.
   `mem()` counts lengths, so the retained figure is unchanged.
3. **The cold "regression" was the bench client.** Measured at 72.3 ->
   84.8 ns/node, it came from the client: an environment lookup per
   step and non-constant hooks per leaf. With part 1's client, the core
   is 72.3 -> 75.8 ns/node (K10) and 54.1 -> 55.3 (K20) on accl,
   +1.6% instructions. That is the cost of the rounds, cancellation,
   appends, memo and CSE plumbing.

### 7.21 Profiles and tier 2 (`profile.rs`, `graph/tune.rs`)

A `Graph<L, true>` profiles; `Graph<L>` (`P = false`, the default)
compiles the profile out.

**What a profile records.**

- Per op, by the op value and with `Lang::op_tag` as its stable
  identity:
  - leaf and scan-element evaluations and their mean cost;
  - steps run, and of those the ones run on an edit (after the first
    build);
  - memo store probes and hits (counted per tag by the store);
  - CSE probes and merges;
  - for watched ops, the evaluations whose key was seen before (the
    reuse a memo would find);
  - whether the op was ever emitted with a class other than `Pure`.
- Per region (a step's key): re-runs on edits, and the last run.

**How it is cheap.** Evaluations are sampled at random intervals, one in
128 on average.

- An unsampled evaluation costs an inlined countdown and a branch.
- A sampled one is timed (the clock's own cost is measured once and
  taken off) and stands for its interval, so counts are estimates and
  means are unbiased.
- Steps are counted exactly, through a one-op accumulator.
- `Graph::profile()` reports the ops by estimated total time and the 64
  regions that re-ran most.
- `snapshot()` gives a `Snapshot` keyed by tag, with a byte image
  (`phiprof1`). `load_profile` adds it to a later session: its ops count
  once seen by tag, its regions at once, and its memo decisions are
  applied by the next `tune`.

**Tier 2: `Graph::tune()`.** The client calls it when the scheduler is
idle; `run` never does, and with work queued it returns `busy`. Every
decision keeps results:

- A memoized op returns what it returned before for the same key: the
  op's tag and its operands' versions, and the op is pure.
- Sealing is exact (7.11).
- Check mode evaluates raw, never through the store.

The decisions:

- **Memo opt-in.**
  - A `Pure` leaf or scan op that costs at least `memo_probe_ns` (default
    100) is watched: every evaluation's key is looked at, at about one
    key hash each, and only for ops that cost more than a probe.
  - It is opted in when cost × reuse > `memo_probe_ns`, with
    `tune_min_samples` and `tune_min_keyed` as the evidence needed.
  - It is opted out when cost × the hit rate since opt-in falls below the
    probe cost; it then stays out for 64 runs.
  - The store gets `auto_memo_bytes` if it had no budget.
  - The membership test is a 64-bit filter, then a set.
- **Sealing by region.**
  - A region with at least `hot_runs` re-runs in the last 4 ×
    `seal_quiet` runs waits four times longer before sealing.
  - A region that re-ran once is sealed after `seal_quiet` / 8.
  - The hot queue is re-ordered by these quiet times, and what is due is
    sealed then and there, with the run end's pruning (`tidy`).

**Measured.** On accl, job 7549, `prof`:

| | K10, 1M steps | K20, 1M steps | K10, 8.4M steps (10^8 nodes) |
|---|---|---|---|
| main (b40d4bf) | 80.0 ns/node | 57.0 | 78.1 (off) |
| off | 81.8 | 61.9 | |
| on | 82.5 | 63.2 | 83.1 |

- On vs off: +0.3 to 1.2% at 1M steps, and +6% at 10^8.
- The off path had a +8% regression at K20. Its cause: once the hook
  gave the crate more callers of `Args::get`, LLVM stopped inlining it
  into client ops. It is now `inline(always)`; locally at K20,
  instructions are +2% against main and cycles are level.

Auto opt-in, with a preamble edit that re-runs 20k steps whose leaves
cost n ns more (`LEAFHEAVY`):

| n | before | after one tune |
|---|---|---|
| 100 | 50 ms | 39 ms |
| 300 | 117 ms | 39 ms |
| 1000 | 330 ms | 38 ms |

The run in which the op is watched costs more (keys): 66 ms at n = 100.

In the toy's random-edit test (40 seeds × 60 edits), a profiled and
tuned document with check mode on makes 80 opt-ins and 7 opt-outs and
seals 4281 steps, and equals the plain one after every edit.

What the tests found:

1. The tuned document's check-mode failure was `tune`'s sealing
   compacting without the run end's pruning. The public `Graph::seal()`
   had the same pattern; both now go through `tidy`.
2. A pathological toy macro made one random-edit seed take minutes. The
   toy now has a bound on pending input.

### 7.22 Queries, named sources and continuation calls (for the TeX layer)

**Reads after the step's own close.** `cx.read` after a group the step
closed resolves past the close, as a leaf's operand does (the TeX
layer's patch, taken as is).

**Unrecorded queries** for the TeX layer's frontier (its engine arrays
at a run's start):

- `cx.defined_reaching()` lists every name a definition reaches, with
  its value as `read` gives it. Groups and `\global` count, the step's
  own definitions are included, and in a segment the parent's names are
  listed too.
- `cx.here()` and `cx.defined_since(p0)` list the names whose reaching
  definition may differ since an earlier step, each with its value here
  (`None`: nothing reaches). p0 and here may be in different unfolds,
  across calls and returns: the walk goes by the two steps' paths from
  the root unfold, from where they part down to here. That is:
  - names defined from p0 on, nested unfolds included;
  - names defined in a group closed since, so a local definition made
    before p0 whose group closed is in the list.

  It is `None` if p0's step is gone; then use `defined_reaching`. Cost:
  the steps walked (a sealed run counts as one) plus their definitions.
- Neither query records a read: the step depends only on what it then
  reads.

**Named sources.**

- `g.source(key, value)` makes an input that is defined to a reserved
  name at the root (before everything); later edits are `g.set` on its
  node.
- `g.remove_source(key)` removes the definition.
- `cx.source(key)` reads that name. It is `None` when the source is
  absent, and the read is recorded either way, so a file appearing,
  changing or going wakes its readers (`\IfFileExists`, `\openin`).
- A source's elements (`\read`) are a read of the same name.

**Continuation calls.**

- `cx.call(op, input, init, args)` emits a nested unfold, and the step
  returns `Step::Call { key }`. The unfold runs in the step's sweep, and
  its result is the step's value and the state its successor starts
  from.
- The step also takes the unfold as an operand. An edit inside the file
  (the source's input) resumes the nested unfold at the edit, as any
  unfold resumes. If its result changed, the call step runs again (one
  step) and the document goes on after it until it converges; if not,
  nothing after the call runs.
- `Step` has no `Arg` (that would put a lifetime on `Lang::step`'s
  return type), hence `cx.call` plus `Step::Call`.
- Speculative entry works across calls. A book's chapters that are
  `\input`s build in parallel from root-level entries, and the result
  equals the sequential build. There is no speculation within one called
  file yet.
- Sealing never takes a step with an unfold child, so a call step stays
  live.

**What the tests found.** A segment packed its definitions with a step's
own before its call's steps'. In position order the call's come first,
so the name index was left unsorted and removed steps' definitions
stayed in it. The pack now sorts each name's definitions by position.

**Tests.**

- `an_input_file_is_a_call_the_document_resumes_after`, with check mode
  on:
  - an edit inside the file re-runs at most 6 steps;
  - an open conditional at the file's end resumes the document;
  - a file appearing and going wakes `\iffileexists`;
  - each state equals a fresh build.
- `chapters_called_from_a_book_build_in_parallel_as_in_turn`.
- The random-edit test's documents can `\input` two files, one with an
  open conditional, and test `\iffileexists`.

**Known, not yet fixed (on main too).** A cross-run slot can stay stale
after an edit. `\ref{k}` reads Unit where a fresh build reads 1, with
one random snippet set: seed 5, edit 51. The repro is in HANDOFF.md.

### 7.17 Dependencies

- `criterion` (dev only, default features off): asked for, and the
  repo has no bench harness to reuse.
- Nothing else. Threads are `std::thread::scope` and channels, the
  PRNG is a xorshift, hashing is the copied `Stable`, serialization is
  the client's `Codec` over a small framed format.

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
