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
    the oracle, including rebuilds after edits.
- Each test runs under the real engine. Its raw outputs go to
  `refs/<engine>/`, which is gitignored and regenerated with
  `--oracle`, and ours are compared against them.
- The gate is `scripts/sandbox cargo xtask check`: fmt, clippy, the
  wasm build, trip, etrip and e2e.

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
- **The terminal** is drawn from events, not from the log: warnings,
  notes and pages shipped come as data from the host, and LaTeX's
  warnings are grouped and deduplicated.

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
  any grain.

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
  re-run out of order may have truncated.
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

### 3.8 Output: effects and the link

- Every output byte is an effect, handed to the innermost call. The
  outputs are the log, the terminal, each `\write` file, the DVI file,
  and the PDF bytes with their object marks.
- A call's effects are cut into **chunks** at the boundaries of the
  calls that apply (3.4): at such a call's start, and at its end before
  its writes are versioned. Each chunk carries a version made from its
  content, keyed by `step << 32 | k`.
- **The link** (`effects::link_cached`) lays every file out from the
  live chunks in program order. A chunk whose version and entry state
  (the object-stream counters, the file being filled, the numbers it
  writes) are those last linked is taken from the cache. Deflate is
  memoized by content. A file is written only when its bytes changed.
  `PARTEX_LINK_SPLICE=0` links everything in full, and a debug build
  checks each cached link against a full one.
- The writer's position in the file and the objects' offsets are not
  state: TeX never observes them, and the link places every object.
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
rule). A switch keeps one evaluation per trip, so a stage can still be
compared with a single plain run.

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
  wide.
- The limits are true dependencies only: chains of definitions of one
  address, kept short by the no-cursor rules (3.9), and the page fold.
- The SSA report prints the frontier's width, the re-runs validation
  caused, and the estimates hit, so the waste is measured.

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
| lists | the nest's lists and boxes, the alignment state, pack results, `last_badness`, `shown_mode` | a box carries its version, made when packed; a pack's result is its node's output |
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
| `PARTEX_SSA_CHECK=1` | check mode |
| `PARTEX_LINK_SPLICE=0` | link in full and write every file |
| `PARTEX_STAT_CACHE=0` | read every file again at a rebuild |
| `PARTEX_SSA_REBUILD` | commands run between rebuilds in one process (the harness) |

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
stamps in arrays, so the engine's hot path hashes nothing.

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
of *steps*: calls from one clean point to the next.
- A *clean point* is main control's top, in outer vertical mode, with
  an empty nest and contribution list, the input at a file level, and
  no output routine active.
- A page that completes defers its fire to the start of the next step.
- Each step has a fixed id and a key: cold keys are 2²⁰ apart, and a
  step spliced in takes a key between its neighbours.
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
   - the save stack below its pointer is placed whole;
   - the page list's length, its tail, and the nodes the step reads are
     placed;
   - the input is set to the previous step's result (3.5), mapped
     through the edits.
3. The step runs, applying the hits of the calls that apply (3.4). If
   it read a slot a later definition holds and that was not placed, its
   run is dropped and made again with that slot placed.
4. Its definitions replace its old ones. Each definition whose value
   changed, or that only one of the two runs made, marks its old readers
   dirty, up to the slot's next definition. Its stores changed make
   dirty the later loads that read the build's own store.
5. A step that ended where its old run did (its result, mapped, equals
   the old one) goes on to the next dirty step. One that ended elsewhere
   (Enter pressed, a paragraph break deleted) runs on, a step at a time,
   until one ends at an old step's start. The old steps passed over are
   removed with their definitions, and their readers now read the
   definitions before them.
6. After the steps, the arrays hold every slot's latest definition
   again, and the link writes the files (3.8).

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
| the save stack placed whole | TeX's undo log must be consistent where it is re-entered | scoped local definitions and group frames (3.2) |
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

1. **The window is the unit of re-execution.** A step (3.15's fold
   unit) may begin at any command boundary at main control's top
   (§1030) where no call is open and the output routine is not active:
   inside groups, inside boxes (any `nest_ptr`), with token lists on
   the input stack, inside alignments. A step ends at the first such
   boundary after: a paragraph's end (back in the enclosing vertical
   mode after `line_break`), a deferred fire, a file level opened or
   closed, or `PARTEX_SSA_WINDOW` commands (default 4,096) since it
   began. A text paragraph is then one window, about a thousand
   commands. Re-entry places what the step reads of the nest (each
   level's mode, fields and list as the appends made so far), the
   group frames, the conditionals, the alignment state, the page
   builder and the input, from the definitions reaching the step, as
   the code places the save stack and the page list today. Commands
   are not nodes: the trace is per window, and a window re-runs whole.
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
   plain partex run to its fixed point.
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
   window again with the full recorder.
8. **The gate.** `cargo xtask check` adds `ssa-edits`: every
   incremental e2e case run in SSA mode, one process, each stage
   byte-identical to plain partex on the same sources (logs masked).
   Machine mode stays until SSA passes everything; removing it is last.

The work, one agent each, on branches `np/<name>` in worktrees under
`~/code/tmp/`: `windows` (1), `rebuild-cost` (3), `records` (2),
`link` (4), `aux-loop` (5), `front` (6), `view` (7), `gate` (8).

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
  is `bench/edits.sh`.

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
  claim.
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
