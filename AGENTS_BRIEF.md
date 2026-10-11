# AGENTS_BRIEF: the parallel pure-SSA design

This is the design. It is stated as if no code existed. When the code
disagrees with it, the code is wrong, however much of it there is. Read it
before every plan, and check every plan and report against **Banned** below
before you send it.

A document is a program. Building it means evaluating a dataflow graph of
commands over versioned values. A cold build and an edit are the same
evaluation, run by any number of cores.

## 1. Values

- Everything TeX calls "state" is a **value**: a macro's meaning, a catcode,
  a register, a parameter, a font, a box, the current list, the page so far,
  an open file, a write stream.
- A value has **fields**, and a node reads only the fields it uses. A box
  has *dimensions* and *contents*; a list has its *items' dimensions* and
  their *contents*.
- A field's **version** is a hash of its content, and only its content: no
  position, line number, cursor, group level, node id or pointer. Two
  definitions with equal content have the same version, and their readers
  cannot tell them apart.

## 2. Names

- A name (`\foo`, `\count3`, the catcode of `{`, `\baselineskip`, the current
  font) means its **reaching definition** at a point in the program.
- Groups are scopes. A local definition reaches up to its group's close; a
  global one reaches onward. Global versus local is decided by the slot's
  level at the end of the step.
- **A group's close emits nothing.** Later reads simply resolve to the older
  definition. There are no restores.

## 3. Nodes

- Every command that computes is a **node**: an op plus operands, which are
  the exact field versions it read, including the input tokens it consumed.
  It produces new definitions. It is a pure function: the same operand
  versions always give the same outputs.
- **A macro call is a step, not a node.** It rewrites the input and changes
  no state. Its expansion is memoised by the meaning's version and the
  argument tokens. Memoised expansion is required.
- **Conditionals.** The test is a node and only the taken arm is read.
  `\fi` defines nothing: **no φ** (dropped 2026-10-11); the untaken arm
  is text the graph holds.
- **`\input` is a call** into a named source. File contents are values like
  any other.
- **Line numbers** (`\inputlineno`, the error context) are a field that is
  read like any other. They are never part of the state a version hashes.

## 4. Layout is more nodes

- **The paragraph builder** reads a horizontal list and its parameters, and
  produces lines (dimensions and contents).
- **The page builder** is a scan of its own over the vertical stream. It
  reads dimensions only (heights, depths, glue, penalties, inserts' sizes)
  and produces break points. That is not a special case; it is what reading
  by field means. It is never part of a paragraph's entry state.
- **The output routine and shipout** read contents and produce pages.

## 5. Evaluation: a suspending dataflow graph

- **No thread is special.** N equal threads; each idle thread calls the
  **scheduler**, a function, to pick its next piece of work, runs it, and
  asks again.
- **A reader** goes over a span of source: it reads characters with the
  catcode version it read, expands macro calls as steps, and emits nodes.
  Its position is its own, never global state. Within a reader, tokens go
  in order because each one's meaning can depend on the previous command:
  that is a real dependency, not a machine.
- **Work is in one of three states:**
  - *ready*: every read it needs is available;
  - *parked*: stopped at a read of field F at point P, registered on F;
  - *speculable*: an entry not yet reached, or a parked read that could go
    on with a predicted version.
- **Mechanism:**
  - *run* a task until it finishes or reaches an unavailable read, and park
    it there;
  - *wake*: writing a field makes its parked readers ready;
  - *speculate*: continue a parked read on a predicted version, marking
    what follows provisional;
  - *validate*: when the real version is known, a match confirms; a
    mismatch re-runs from the first node that read the field.
- **Two kinds of unavailable read.** (1) The writer is known but not done:
  park. (2) It is not yet known whether anything earlier writes the name
  (any earlier macro call could define anything): wait until the earlier
  text has run past, or, as a scheduler policy, predict *no earlier write*
  and validate later.
- **Suspension first, speculation second.** The first scheduler policy is
  pure suspension. Speculation (predicting absence of an earlier write,
  from the last build's graph or the format) is a second policy, added when
  measurement says it pays.
- **The invariant: the scheduler affects speed, never results.** Any order
  of choices gives the same graph, PDF and `.aux`. Tested with a randomised
  scheduler.
- **Edit.** The changed text dirties the nodes that consumed it. Before a
  dirty node re-runs, check whether its operands actually changed. When a
  re-run produces equal versions, propagation stops there. Keys are stable:
  no line numbers, no cursors.
- **A cold build is an edit from nothing.** There is one entry point for
  both.

## 6. A cold build, step by step

1. **Known at t=0:** the format (a set of definitions with known versions)
   and the main file's text.
2. **Cut, without running TeX.** The CST splits each source at entries
   (paragraph boundaries at brace depth 0, `\input`, `\include`,
   `\usepackage`, `\documentclass`); files named there are opened and cut
   in parallel. The cuts are hints only.
3. **Readers start at entries.** Entry 0's reads resolve to the format's
   definitions. A later entry's reads park until they resolve, or, under a
   speculating policy, run on a prediction. Where the reader is (inside a
   group, an argument, a verbatim) is itself a read; a wrong cut is
   re-cut where the confirmed reader actually is.
4. **Downstream work overlaps without speculation:** line breaking of
   paragraph k while the reader goes on to k+1 (blocking only when
   something reads the lines' dimensions, such as `\prevdepth`), the page
   builder's scan, output routines, shipout, PDF and font work, each
   waiting only on its own inputs.
5. **The length of a cold build is the true dependency chain**, not the
   length of the file.

## 7. Outputs and the cross-run fixpoint

- Writes (`.aux`, PDF objects, `\write`) are values ordered by position and
  joined at the end. `\immediate` versus deferred is only which node owns the
  write.
- **The `.aux` round trip is a fixpoint.** Run k reads what run k−1 wrote, as
  a new version of that source. It stops when the versions are equal, with
  an iteration bound and a report if it oscillates.
- Per-label precision is not extra work: a node that reads `\r@foo` depends
  only on that macro's version.

## 8. Storage

- One graph, written concurrently. Each worker writes into node-id ranges
  reserved for it. Values are content-addressed and interned.
- Every array or table (eqtb, registers, a save stack, any index) is a
  derived cache of the graph. None is ever the source of truth.

## 9. What survives from Knuth

- **Kept, as pure functions over values:** his algorithms. Line breaking,
  hyphenation, box packing, font metrics, math layout, the page builder's
  cost function.
- **Dropped:** his machine. Global arrays as state, the save stack, one
  program counter walking the input, one thread.

## Banned

If a plan, brief, report or commit message contains one of these, it has
drifted. Stop and re-read this file.

| Banned | Instead |
|---|---|
| "Load" a state, frontier or snapshot | A segment starts with no state; its reads resolve. |
| A main, build, driver or ordering thread that runs TeX | No thread is special; the commit only confirms. |
| eqtb, registers or the save stack as the truth | They are caches of the graph. |
| Restores, unsave events, or a group close that emits | A group's close emits nothing. |
| Positions, line numbers, levels or ids in a version | A version is content only. |
| Record an imperative run, then parallelise it | Evaluation is parallel from the start. |
| Re-resolving every reader after a definition | Validate the exact reads. |
| Page builder as a special case, or inside entry state | It is a node that reads dimensions. |
| A cold-build path separate from the edit path | A cold build is an edit from nothing. |
| "The engine", an engine instance, a world struct (`Tex`) | Commands are functions; readers produce nodes. |
| Moving eqtb, nest or the input stack "into the graph" | They do not exist; names have reaching definitions. |
| Importing `phitex-*` into the new crate | Copy an algorithm out as a pure function. |

## Tests that hold the design

1. **A segment's start reads zero state:** fail if anything is copied in at
   a segment's start.
2. **No dominant thread:** a cold build at W workers fails if one thread does
   much more than 1/W of the work.
3. **Cold equals edit:** a cold build goes through the edit entry point,
   from empty.
4. **Content-only versions:** inserting a blank line above a definition
   leaves its version unchanged, and re-runs no reader of it.
5. **The same output at any W:** W=1 and W=8 give byte-identical PDF and
   `.aux`.
6. **The untaken arm:** an edit to a conditional's untaken arm re-runs
   nothing past it (no φ at `\fi`).
7. **Per-label `.aux`:** changing one label's page re-runs only that label's
   readers.

## Budgets

- A word edit: ≤50 ms and ≤300 steps.
- An inserted line: ≤50 ms.
- An edit that moves a page break: ≤100 ms.
- No edit costs more than 25% of a cold build.

## How we build it

- **The wall.** All new code lives in `crates/phitex-flow`. It never
  depends on `phitex-core` or any crate that holds engine state (eqtb,
  nest, input stack, a `Tex`/engine struct): Knuth's algorithms come
  across by copying them out of `phitex-core` as pure functions, and
  reaching for eqtb is a compile error. Pure libraries that hold no TeX
  state may be linked (decided by the user 2026-10-09): `phi`, and per
  explicit decision each of `phitex-otf` (OpenType shaping), the
  PNG/deflate/PDF-writing code, font parsers; `tests/wall.rs` is the
  allowlist.
- **The minimal system** (2026-10-09) is the base: `crates/phitex-flow`
  (pieces, reads that park or predict, the thin confirmation, joins, the
  page scan, DVI, N equal threads, edits). Features are added on top of it;
  a feature that seems to need its core changed is a design conversation
  first.
- **Milestones:**
  1. `{\def\a{x}}\a` and an `\if` on the graph at W=8, random
     scheduler, the graph printed.
  2. `-ini` documents with a TFM font shipped to DVI/PDF, byte-identical
     with TeX, at W=1 and W=8.
  3. plain TeX (the format built by the new engine), byte-identical.
  4. LaTeX, then edits against the budgets.
  5. Speculation as a second policy if measurement says so; then it
     replaces machine mode in the CLI and the old code is deleted.
- **Everyone re-reads this file every 30 minutes**, and before every plan
  or report.
