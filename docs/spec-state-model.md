# The client's state model, rewritten (the design, 2026-10-28)

Status: decided by the user on 2026-10-28 ("what needs to be deleted and
rewritten"), after the storm attributions of 2026-10-27/28. This file is
the spec the rewrite is typed from. It replaces the state half of
`crates/engines/tex/src/run.rs`, all of `runstate.rs`, `fields.rs` and
`regfold.rs`, and the state parts of spec-final §2 and spec-runstate.md.
The engine (`crates/core/defuse`) is unchanged. The interpreter
(`control.rs`, `expand.rs`, `scan.rs`, `exec.rs`) stays and uses the API
of §7.

## 1. What is wrong with the model being replaced

Measured on course3 (three chapters of the user's book) at one thread,
81% of run time wasted, 3.3 runs a node, with these causes, each a
structural property of the old model:

- **Tables as one field.** `cattab`, `parpar`, `pagepar`, `sftab`,
  `lctab` are records: one value for a whole table, read whole, so a
  reader of one catcode is refuted by any catcode anyone before it
  changed (`cattab` is the top refuting field on course1 at eight
  threads, 557). The records were made for speed: a run's first read of
  each code cost an engine read.
- **Eager reads of page-layer values.** `\the\c@page` in a paragraph
  reads the page counter, which a finisher after it defines; 2159
  paragraph reruns on course3, 76% of all page-content changes one digit
  of cleveref's aux line (docs/spec-deferred-reads.md).
- **Writes that are not changes.** A global restore of what the run
  read is published as a definition (cleveref's probe: a relay through
  every labelled paragraph); the soft-read rule exists for local writes,
  continuations and loads, three implementations with different
  conditions.
- **Page-dependent macros predicted as stable.** A finisher's
  prediction of `\@currentHpage` and kin is "the value the page's last
  run left", which holds another page number: 25929 prediction misses
  against 1441 hits on course3 at one thread.
- **Publication by special cases.** What a run defines is computed by
  `published`, `base_publishes`, `abandon_to_scope`, `restored_locs`,
  `note_pre`, `reg_map_ok` and the continuation rules, each with its own
  conditions (rec_load, read_point_here, cand_marks, scope kinds).

## 2. The model

A run (one node's execution) sees TeX's state as **items**. Every item
is its own engine field. There are no table fields.

| item | field | definer makes | example |
|---|---|---|---|
| integer register | `count:<loc>` | a relation (`+c`) or a value | `\advance\c@page 1` |
| dimen, skip, muskip, toks register | `dimen:<n>` … `toks:<n>` | a value | |
| parameter | `param:<name>` | a value | `\parskip` |
| catcode, lccode, uccode, sfcode, mathcode, delcode | `cat:<c>`, `lc:<c>`, … | a value | `\catcode`\@=11` |
| control sequence meaning | `cs:<name>` | a value (the meaning) | `\def\foo{…}` |
| active character meaning | `act:<c>` | a value | |
| font | `font:<id>`, `fontinfo:<id>` | a value | |
| math family font | `fam:<size>:<n>` | a value | |
| box register | `box:<n>` | a value | |
| mark class | `mark:<class>` | a contribution | `\mark{…}` |
| insert class | `ins:<n>` | a contribution | |
| write stream | `write:<n>` | a contribution (the whatsit) | `\write1{…}` |
| the main list's tail | `pg:*`, `lines` | a contribution | a paragraph's lines |
| page-layer value in a piece | the same field, read as a **symbol** | | `\the\c@page` in text |

Every read and write of an item goes through §7's API. The engine
records what the run read (per item), files what it defined (per item),
and re-asks only the readers of an item whose definition reaching them
moved. A table is never a field: a run that reads 300 catcodes records
300 reads, and a definer of one catcode re-asks the readers of that one.

### 2.1 Records are caches, not reads

A run's first read of an item costs an engine read (~800 instructions).
To keep the old model's speed, a run **loads** its items from a
**snapshot**: a value the engine gives for a family at a position (the
catcode table reaching this node, the parameters reaching it), computed
by the engine as the fold of the family's definitions and cached by
content. The load is not a read. The run records a read of `cat:<c>`
only when it uses `cat:<c>`, with the value the snapshot held. A
definition of `cat:<c>` refutes exactly those readers.

The engine side already exists for this (`Ctx::read_path` with
`Step::Index` reads one entry of a value; `Ctx::fold` folds a family's
contributions); where a family snapshot is missing from the engine, the
rewrite adds `Ctx::snapshot(family)` as a pure, unrecorded load of the
reaching definitions of every item of the family, keyed by content, and
`Ctx::read_item(field, seen)` to record the per-item read after the
fact. Both are small and their tests are named in §9.

### 2.2 Reads

- A read is recorded when the value is **used**: scanned as a number,
  compared, typeset, expanded into a list that is printed or forced
  (`\immediate\write`, `\message`, `\special`), or written to another
  item. Loading is not using.
- A read of a page-layer item (a register the output routine may
  define: `count:0` and the routine hint's stepped counters; the page
  parameters it sets) in a piece yields a **symbol** (spec-deferred-reads
  §2): the value as of the piece, carried through `\edef`, macro bodies,
  deferred `\write` lists and marks, forced to a real read only where
  TeX needs the digits, resolved by the finisher at shipout, and a
  restore of the item to its own symbol is nothing.
- A read made at the run's start (its input frames, the format) and a
  read through a snapshot are recorded per item used, never per family.

### 2.3 Writes

In order of preference, the first that applies:

1. **Nothing**, when the write restores what the run read of the item
   (its symbol, or the value loaded before any map of this run), at any
   locality, in any mode. Neither read nor definition remains if the
   value flowed nowhere else; the read remains if it did (the symbol
   knows: forced or not).
2. **Nothing**, when the written value equals the value reaching the
   run's position for that item, at the same visibility.
3. **A relation**, when the write is `+c` on an integer register
   outside every group of the document and the run never read the
   register (`\advance`, `\stepcounter` where the counter is not read).
   The run composes its maps; the engine recomputes relations without a
   run and gives predicted ones to nodes ahead of their run.
4. **A value**, otherwise: filed at the run's position, Global if the
   assignment was `\global` or at the document's outer level, Local with
   the depth of the group it was made in (the engine's scope depth), as
   today.

What a run publishes is the set of items for which rule 3 or 4 fired at
its base level, computed from the run's write log, with no other list.
A local write inside a group the run closes is never published (it died
with the group); a local write inside a group the run leaves open is
published with that depth. Continuations (a resume) publish what they
wrote, by the same rules, against what reaches them.

### 2.4 Predictions

A node's predictions before its run (the engine's `predicted_defs` and
`predicted_rels`) come from one source each:

- A piece's: the hint pass (`hints.rs`, unchanged): `+1` relations for
  the counters its text steps, the catcodes and parameters its text sets
  to constants.
- A finisher's: the routine hint (unchanged for counters: `+1` on the
  page counter per shipped page, confirmed 100% on course3) and, new, a
  **template** for page-dependent macros: a finisher's last run's value
  of `cs:<name>` with every occurrence of the page number it ran with
  replaced by a hole; the prediction for the next run substitutes the
  predicted page number (`FinDefs::template`). A macro whose last value
  does not contain the page number predicts as today (stable). Bar:
  prediction misses on course3 at one thread from 25929 to under 2000.

### 2.5 Scope

One rule, the engine's: a definition carries the depth of the group it
was made in; it reaches a later position while no close between brings
the depth below it (`read::visible`). The client declares a run's
brackets (closes, opens) from its text as today (`Operation::effect`) and
files its writes with the depth the engine computes. The save stack of
the old table disappears: a local write inside a group the run closes
is a run-local value only; the group's end restores the loaded value
(the symbol, for a page-layer item).

### 2.6 The run-local table

A flat table by location (`Loc`), as today, holding for each item: the
loaded value (`Loaded(v)`, `Symbol(s)`, or `Absent`), the current value
(same kinds, plus `Written(v)`), the map for integer registers
(`Affine`), whether the item was used (read recorded), and the group
depth of the last local write. The meaning store for macros stays
(`Mean`, the decoded macro bodies) and its memos (runstate::Memos) stay
as they are: they are the interpreter's cache, not the state model.

## 3. What is deleted

- `runstate.rs`: the records (`B_CAT`, `B_PAR`, `B_PAGE`, `B_SF`,
  `B_LC`, `record_of`, `cells`), the save stack, `published`,
  `base_publishes`, `abandon_to_scope`, `restored_locs`, `read_locs`,
  `touched`, `cont_republished`, `PState`. Kept: `Entry`'s meaning
  encoding, the macro store, `Memos`.
- `fields.rs`: the `STATIC` table's record cells (`cattab`, `parpar`,
  `pagepar`, `sftab`, `lctab`, the penalty arrays as components); the
  location layout stays (parameters, codes, registers, boxes, fams,
  meanings, spill).
- `run.rs`: `load_slow`'s record path, `reg_map_ok` and its conditions,
  `reg_apply_map`'s loaded-value patching, `note_pre`, `read_point_here`,
  `drop_unchanged_republished`, `published_fields`, the definition loop
  in `finish` (replaced by the write log's publication), `fin_lazy`.
- `regfold.rs`: folded into the table's map kind.
- The deferred-read implementation on branch `defer` (src/defer.rs and
  its hooks) is the symbol part of this model: it is kept as code and
  moved under the new table's `Symbol` kind, its tests unchanged.

## 4. What stays

The interpreter's command, expansion and scanning code; `hints.rs`; the
page glue (`pages/`), which reads page items through the same API; the
engine; `val.rs`'s values and tokens; `build.rs`'s slicing (it reads the
new table's loaded values through the API).

## 5. The bars

Measured on the four-row table (course1 and course3 at -j1 and -j8,
fastdev, the latex agent's recipe), against the baseline at 657dd0933
(1.71/66%, 5.84/78%, 3.34/81%, 8.10/86%), all byte-identical:

- course3 -j1 under 1.6 runs a node and under 40% wasted; course1 -j1
  under 1.3.
- `cattab`, `parpar`, `pagepar`, `skind`, `count:0` absent from the top
  ten refuting fields at -j1.
- Prediction misses on course3 -j1 under 2000.
- The tex suites (plain, pdftex, pages, edits, latex, etex, hints,
  inputs, absorb, auxloop, tests/defer.rs, tests/state.rs) green on the
  accl gate; amsldoc 98c96e00 at -j1/8/16; course3 7705960b; course1
  9bf25c07.

## 6. Order of work (one pass, by one agent, from the skeleton)

1. The table (`state.rs`): kinds, load, use, write log, maps, depth.
2. Snapshots: `Ctx::snapshot` and `Ctx::read_item` in the engine with
   their tests; the client's families (codes, params, registers).
3. Symbols: move `defer.rs` under the table (the `defer` branch's code).
4. Publication from the write log; delete the old publication paths.
5. Templates for page-dependent macros in the finisher's predictions.
6. Delete the records and the save stack; the interpreter compiles
   against §7.
7. Suites, the four-row table, the gate.

## 7. The API the interpreter sees (`crates/engines/tex/src/state.rs`)

```rust
pub(crate) struct State { /* the table, §2.6 */ }
impl State {
    /// The item's value for use: loads through the family's snapshot on
    /// the first touch, records the read now (symbol: no read).
    pub(crate) fn get(&mut self, cx: &mut Ctx, loc: Loc) -> R<Value>;
    /// The item's value for a purpose that does not use it (a `\let`
    /// copying a meaning records a read of the source: `get`; a save
    /// stack copy does not: `peek`).
    pub(crate) fn peek(&mut self, cx: &mut Ctx, loc: Loc) -> R<Value>;
    /// A write, local at the current depth or global (§2.3).
    pub(crate) fn set(&mut self, loc: Loc, v: Value, global: bool);
    /// `+c` on an integer register (§2.3 rule 3 where it applies).
    pub(crate) fn add(&mut self, loc: Loc, c: i32, global: bool);
    /// A group opened or closed by the run (§2.5).
    pub(crate) fn open(&mut self); pub(crate) fn close(&mut self);
    /// The symbol of a page-layer item at this run (§2.2).
    pub(crate) fn symbol(&mut self, loc: Loc) -> Option<Sym>;
    /// At the run's end: the items to publish, each with its relation
    /// or value and locality (§2.3), computed from the write log.
    pub(crate) fn publish(&self, cx: &mut Ctx);
}
```

## 8. Tests (`crates/engines/tex/tests/state.rs`, written with the skeleton)

- `a_catcode_change_re_asks_only_the_readers_of_that_catcode`
- `a_parameter_set_by_the_output_routine_to_its_old_value_re_asks_nobody`
- `a_global_restore_of_a_read_value_publishes_nothing`
- `a_local_write_in_a_closed_group_publishes_nothing`
- `a_local_write_in_a_group_left_open_publishes_at_its_depth`
- `a_counter_step_without_a_read_is_a_relation`
- `a_page_dependent_macro_is_predicted_from_the_page_number`
- `a_run_records_a_read_only_of_the_items_it_used`
- `the_state_fixtures_are_byte_identical` (every fixture under
  oracle/fixtures/dvi and the latex suite's documents)

## 9. Engine additions (small, each with a test in `crates/core/defuse/tests/snapshot.rs`)

- `Ctx::snapshot(family: Field) -> ValueId`: the value folding the
  reaching definitions of every field with the family's prefix at the
  node's position, unrecorded, cached by content.
- `Ctx::read_item(f: Field, seen: Option<ValueId>)`: records a read of
  `f` with the value the run used, as if `read` had answered it.
