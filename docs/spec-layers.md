# Layers on a mergeless engine (the design, 2026-10-21)

Status: the design PhiTeX builds toward, decided by the user on
2026-10-21. The engine's side is IncEngine2's `docs/mergeless.md` in the
defuse repository (clean-core 12cc83b). This file is PhiTeX's side. It
supersedes the page chain's node layout (spec-page-chain.md §4) wherever
the two disagree; spec-page-chain.md's bars and invariants still hold.

## 1. The rule

Serial work is only what TeX itself makes serial: expansion's state
chain, the page-to-page state an output routine really carries, and the
rounds of files read back. Nothing serial may grow with the number of
nodes.

The engine's serial text-order merge broke this rule. Every result was
folded into one world on one thread, a document-order fold, so para4000
was capped near 0.63 s at any thread count (PLAY_DAG model, job 15731).
The page chain as engine nodes broke it too: 1,474 links at about 0.85 ms
each, against a 0.09 ms scan op.

## 2. The layers

Each layer is a def-use graph whose structure is fixed by the layer below.
Only expansion discovers structure by running.

| # | Layer | A node is | Placed by | Serial part |
|---|---|---|---|---|
| 0 | Environment | files, the format | | |
| 1 | Tokenize | a line of input, keyed by its bytes and catcode regime | the files | none |
| 2 | **Expand** | a paragraph's (display's, alignment's) commands: macros, registers, counters as folds, fonts, hbox packing | the input | TeX's state chain |
| 3 | Break paragraphs | one paragraph's line breaking, hyphenation, shaping, math layout | layer 2, one per paragraph | none: pure, memoised |
| 4 | Break pages | **one node**: TeX's page builder as a loop over the profile accumulator | layer 3's profiles | a loop, microseconds a paragraph |
| 5 | Finish pages | one per page: output routine, marks, inserts, floats | layer 4, as its children | page-to-page state, guessed and validated |
| 6 | Write and link | a page's PDF (DVI, XDV) bytes; object numbers by prefix sums | layer 5 | a scan |
| 7 | Rounds | the files the build writes and reads back | the build | the outer fixpoint |

- **hbox packing stays in expansion:** `\setbox`, `\wd`, `\settowidth`
  and their kin are read inline. Only the line breaking of a finished
  paragraph moves to layer 3.
- **A page that fires inside a paragraph** needs no split of that
  paragraph into nodes: the page loop works on lines.
- **SyncTeX records** are made where the boxes are made (layers 2-3) and
  collected per page by the finisher (layer 5), as spec-synctex.md says.

## 3. Feedback up the layers

A higher layer that reads a lower layer's result reads a predicted value.
The lower layer validates it when it settles, and only the nodes that
guessed wrong run again. Each case is named; a new one is added here.

| Read in a higher layer | Comes from | How |
|---|---|---|
| `\pagetotal`, `\pagegoal`, `\pagedepth`, page state in text | 4 | predicted, validated by the page loop |
| `\thepage` and page counters read in text | 5 | guessed folds, validated |
| object numbers read back (`\pdflastobj`, `\pdflastlink`, `\pdfximage` and kin) | 6 | the piece ends before the command (r8's fix, 2026-10-21), then predicted |
| `\hsize`, `\vsize` and kin changed by an output routine | 5 | predicted per page |
| `\prevgraf`, `\lastbox` after a paragraph | 3 | predicted, validated |
| catcode changes | 2 → 1 | the line's regime key |
| page references, contents, index, citations | 7 | the rounds, as latexmk |

## 4. A cold build

1. The preamble's loads run in sequence: TeX's real chain. Later it is a
   sequential region publishing one snapshot (note-sequential-region.md).
2. Every paragraph's expansion starts once its inputs are published.
   Counters are folds (acc_range_after_def). Each node publishes its own
   result on its worker; there is no barrier.
3. Each paragraph's lines are made as soon as its expansion ends, and its
   profile goes into the profile accumulator.
4. The page node reads the profiles (a range read over the accumulator)
   and runs the page builder as a loop. It places one finisher per page.
5. The finishers run in parallel on predicted page-to-page state.
6. Pages are written in parallel; the link numbers objects by prefix sums.
7. If a written file changed, only its changed entries' readers run again.

## 5. An edit

A word changes in paragraph 150. Its expansion runs again; if its state
outputs are unchanged, nothing else in layer 2 runs. Its lines run again
and its profile changes. The page node re-breaks from the first changed
position until the breaks agree again (the engine's "first changed
position of my range read", r6's E3), and only the changed pages are
finished and written again.

## 6. The engine (summary of mergeless.md)

- **Cells, not a world:** a field cell per (unit, field) holds the
  definers in text order, the contributions with prefix summaries, and
  the readers; a unit cell holds order, frames and windows; a node cell
  holds a node's state and result. Each is a persistent tree behind its
  own lock.
- **A node publishes its own result** on the worker that ran it: each
  read is filed and then re-checked under its cell's lock; each write
  notifies the readers it reaches.
- Text order lives only inside cells. Steps, batches, the barrier, the
  lane, flow and the merge are gone. Output stays byte-identical at every
  thread count, and every Ctx call keeps its meaning.
- Layers whose results the text reads as values (a paragraph's lines,
  the page loop's fold) are ranked units in one engine, with composite
  stable positions (the placer's position plus an ordinal).
- A finisher defines fields the text after its page reads, and
  definitions are scoped to a unit, so finishers stay in the text's unit,
  placed right after the piece their page fired in
  (`follow_after_any`). Its position is the composite (the anchor's
  position, the placer, an ordinal): appended within the anchor's slot,
  no renumbering (IncEngine2, 2026-10-21).
- A run's placements are committed at every stop, a park between
  attempts included. So the page builder's node places each finisher as
  its page fires, and the finishers run while it is still folding. A
  later attempt makes the same placement again; one no longer made is
  freed. (This comes on today's engine first, ahead of the cells.)

## 7. Order of work

1. **Pages as one node** (layers 4-5), on today's engine: removes the
   page chain. Model: para4000 `-j 8` 0.7-0.9 s.
2. **Line breaking out of expansion** (layers 2-3), and amsldoc's
   199-node data chain named node by node: each expansion link about 40%
   shorter.
3. **The cells engine**, migrated in steps that each keep the gate and
   fuzz green, then layer ranks (IncEngine2).
4. **Per-command speed** (the flat executor), the sequential preamble
   region and the first build.

Dropped on 2026-10-21: polishing the serial merge (lazy filing, the
per-drain builder, parallel filing, a tree merge), tuning CLEAN_FLOW,
the scan and continuation chain, and evaluating in rounds.

## 8. The code (the skeleton, 2026-10-21)

Steps 1 and 2 of §7, on today's engine. The scan, the continuation and
their fields (`scan:state`, `scan:from:j`, `scan:mat:j`, `rtn:j`) are
gone.

| Layer | File | What is there | Stub to fill |
|---|---|---|---|
| 2 | `crates/engines/tex/src/pages/piece.rs` | candidates, cuts, `Resume`, a piece's units and main-list tail, the finisher's check of a piece | |
| 3 | `crates/engines/tex/src/parbreak.rs` | `Break` (one paragraph's lines, a child of the piece), `BreakIn` | `Break::run`, `Run::place_break`, `Run::break_as_node` (false: lines still broken in the piece) |
| 4 | `crates/engines/tex/src/pages/builder.rs` | `Pages` (one node, pushed where the job begins), `PageLoop` (parked between attempts), `Src`/`Span` (where each item on a page came from), `Run::place_pages`, `Run::rtn_guess`, `PageState` (the builder on real nodes) | `Run::page_loop` |
| 5 | `crates/engines/tex/src/pages/finish.rs` | `Finish`, `FinishIn` (its arguments, `arg`), the routine, ship-out, the predictions | `Run::finisher_args`, `Run::define_page_start` |
| §3 | `crates/engines/tex/src/pages/feedback.rs` | `Feedback` (the named cases, counted), `PageStart`, `\pagetotal` | |

The interfaces between the layers are fields and accumulators:

- 2/3 → 4: `vprof` (profiles) and `pitems` (items), one unit per piece
  and per paragraph's lines.
- 4 → 5: a finisher's arguments (`FinishIn`): the spans of items its
  page is made of, the window of `pitems` they are in, the builder's
  state where the page began and its decisions there, the marks in
  force, the guess of what its routine gives back.
- 5 → 4: `pages:miss`, only where a routine gave back other than the
  guess.
- 5 → 2: the routine's definitions (fields, predicted) and
  `pages:start` for `\pagetotal`.
- 4 → 6: `pages:end`, read by the writers and the files.

Code that only exists until a stub is filled carries
`#[expect(dead_code, …)]`: the expectation fails once the code is used,
so it is removed with the stub.

Known before the skeleton (gate of the merged base b7ab9721, jobs
15768–15778): `latexmk_corpus_{image_twice,char_string,version_class}`
hit the loud check of a read of the pages' objects after a candidate no
pass can end at (`Feedback::ObjectNumbers`); `pages::vsize_document_pages`
runs 116 times for 105 nodes (bound 1.1).
