# Registers as folds of affine writes: a design (2026-10-21)

Status: design only, not built ([spec-page-chain.md](spec-page-chain.md) §5.1,
§9 Q3). It follows [note-page-chain-p0.md](note-page-chain-p0.md).

## Why

On dev e7aa893f amsldoc's longest piece→piece chain is 98 links once the
cut fields are left out:

- registers: 55 links;
- the list tail: 16;
- the list-label box: 11;
- macros: 11.

The longest register run is `count:187`, a counter the marks code steps.
It gets 36 links, and every write adds 2. Each link is a piece that reads
the value the piece before wrote:

- `\advance` reads the old value (`do_register_command`, control.rs:
  `let old = self.read_f(field)?`);
- the next piece reads it to use it (`scan_something_internal` → `get_loc`).

Cold, a reader waits for the writer before it: a chain through every writer.

## Where a write is decided (run.rs, control.rs)

- `\advance`, `\multiply`, `\divide` and an assignment through a register
  primitive (`\count n =`) go to `Run::do_register_command` (§1236–§1238).
  - It reads the old value first for every kind, an assignment included.
    The assignment's read is useless: §1238 reads `eqtb[l]` only for
    `advance`/`multiply`/`divide`. That is a needless read like rules 1-3,
    fixable on its own.
  - It then defines through `set_loc` → `State::set`. `note_kind` records
    whether the write was an `\advance`.
- A `\countdef` token's assignment is `cmd::ASSIGN_INT` in
  `prefixed_command` (control.rs `ASSIGN_INT`): `set_loc`, no read.
- A write outlives the piece when it is global (`\global`) or made at the
  run's base level (`grp == None`). A local write inside a group the run
  opens and closes is undone (§282) and never leaves the run.

## The contribution

- For each integer register `count:n`, an accumulator `acc:count:n` with
  the affine-map monoid:
  - a unit is `(a, b)`, read as `x ↦ a·x + b`;
  - `set c` is `(0, c)` and `add c` is `(1, c)`;
  - the fold composes in position order:
    `(a2, b2) ∘ (a1, b1) = (a2·a1, a2·b1 + b2)`, wrapping at 32 bits as
    web2c's `\advance` does.
- A write that outlives the run contributes:
  - `\advance` by a constant: `add c`;
  - an assignment (by `\countdef` token or `\count n =`): `set c`;
  - `\multiply`/`\divide`: the value the run computed, `set v`. This one
    needs the old value, a real read; it is rare.
- The run no longer defines `count:n` for such a write. Within the run, the
  slot holds the map since the run's start, with no base: a pending `(a, b)`.
- **The write does not read the old value.** `\advance` composes `add c`
  onto the slot's pending map. Only a use of the value needs the base.

## The reader

- A use of the value (`\the`, `\number`, `\ifnum`, a register in an
  expression, `\multiply`) on a slot with no base reads the fold of
  `acc:count:n` before the run: `acc_range(acc:count:n, None)`.
  - That fold is applied to the register's start value (the format's, an
    environment field), then the run's own pending map.
- `count:n` stays a field only for the start value. Every outliving write
  is a contribution, so the fold alone decides the value; definition and
  fold never compete.
- **Cut checks** (`check_cut`): an affine contribution is a definition of
  `count:n` for the routine's view, which reads its own fold at its
  position.

## What the engine needs (§9 Q3)

- **Speculative fold reads.**
  - `acc_range` today waits while a possible contributor before the reader
    is in work (`World::range_blocker`). Pieces declare their
    contributions only after running, so a cold reader waits for every
    earlier piece. That is not a chain of values, but a reader of the
    fold cannot finish before every earlier piece has run once.
  - Wanted: the fold as it stands, with an in-work contributor's last
    unit, or none on a cold build. The read is recorded as speculative and
    asked again when the contributor commits.
  - A contribution never depends on the value read: `add 2` is `add 2`
    whatever the base. So a cold build converges in two waves:
    1. every piece runs on a guessed fold and contributes the right maps;
    2. each reader whose fold changed runs once more.
  - Edits guess the previous build's fold, which is right before the
    edited piece.
- **Accumulators by family.**
  - `declare_accumulator(f, m)` takes one field. A register's
    accumulator appears with the first write, from inside a run, where the
    engine cannot be borrowed.
  - Wanted: one monoid for every field whose name has a prefix
    (`acc:count:`), or declaration from a run.
  - The alternative, declaring all 32768 count accumulators up front,
    works but is wasteful.
- No change to placement or to the lane.

## The list tail and the label box

**The list tail** (`vlist:last`, `vlist:pd`, `vlist:since`, 16 links) is a
real dependency:

- A piece's first box needs `\prevdepth` from the piece before for its
  interline glue (§679, `append_to_vlist`).
- Text that reads `\lastskip`/`\lastpenalty` (vertical-space macros that
  compare the last skip, `\unskip`) reads the item
  the piece before left last.
- `define_main_tail` already reads the field only where the piece's own
  items leave it standing.
- What remains can go:
  - the interline glue as a symbolic item (spec §5.2): only a run that
    reads `\prevdepth` as a value reads the field;
  - `\lastskip`-type reads of a piece's predecessor are true reads of
    TeX's state.

**The label box** (`box:35`, 11 links) is real:

- A list item's command sets the label box globally.
- The paragraph after it reads and empties the box where it starts (the
  `\everypar` it set, §1091).
- The box's contents are typeset, so this is no projection.
- The links pair an item with its paragraph. The chain continues through
  other fields.
