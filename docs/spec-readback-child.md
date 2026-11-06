# A per-field read-back as a child node (design and exactness, 2026-11-04)

Status: built on branch xref9 (`crate::ops::ReadBack`, `Run::rb_read_back`,
`Run::rb_check`). It answers the tikz-cd label edit:
`\begin{document}`'s run (~130 ms) ran again on every new `\label`
because it read the `.aux` key set whole (`wkeys:<job>.aux`).

## 1. Before

A file whose lines only define (the `.aux` of most LaTeX documents) is
read back per field (`Run::read_back`): at its read-back point the run
reads the key set `wkeys:<file>`, and for each key its definition
`wdef:<file>\1<name>` (`[value, global]`), keeps the value it replaces
(`wpre:<name>`, for the lines written this round, `Run::pre_value`) and
defines the key in its own state. The run depends on the key set as a
whole, and publishes every key as its own definition: a new key changes
both, so the run goes again from its start. Besides, `Run::lb_known`,
`per_field`, `written_lines` and `file_inner` read the key set whole where
they use only its presence, whether it is the text mark, or whether it
has a key.

## 2. The shape

- The key set is read through paths where only that is used: presence
  (`lb_known` = `Step::Present`), the text mark (`Step::Eq`), non-empty
  (`[Index(0), Present]`).
- At the read-back point (a document's `exec`/`resume` run, not a line's
  interpretation nor a recorded load, the file per field, not seeded, not
  the file's first read-back in the job (`wread:` unset: a later one,
  `\\enddocument`'s, reads every key), `PHITEX_NO_RB_CHILD` unset), the run places
  a child `readback(key, run's node)` (`Ctx::child`).
- The child reads the key set and every key's definition, and outputs
  `[flags, [[name, value]…] global keys, [[name, value]…] local keys]`. Flags are 1 (and the
  child defines nothing) where any key is an integer register (read
  directly by a symbol, `crate::defer`) or a box (`ops::rb_answers`), any
  definition is not `[value, global]`, an error key (`rberr:`) is there,
  the key set is absent or the text mark. With flags 0 it defines each
  global key, except one the run itself defined after its own run
  (`read_at(f, AfterOwn(run))` by the run).
- The run reads the child's output one path at a time
  (`Ctx::placed_output_path_or`): flags (`[Index(0)]`) and each key it
  looks up (`[Index(1), Key(name)]`). Where the child has not run, the
  answers come from a guess: the same output computed from the loop-backs
  peeked (`Ctx::peek_final`, unrecorded); the recorded read is checked
  against the child's real output when it commits (`reask_parent`), so a
  wrong guess costs a run, never an output.
- Flags not 0: the run reads the file back inline, as before.
- Flags 0: the local keys (hyperref's `\providecommand` lines), their
  list read whole (`[Index(2)]`), are defined at the point as inline.
  Every location the run holds at the point (`held_locs`) is
  checked now: a key among them is defined exactly as the inline read-back
  defines it (`read_back_pre`, then the global definition). Any other
  location is checked where the run first uses it (`load_slow` and the
  eight meaning look-ups that read the engine through a projection):
  a key is defined then, the same way (`Run::rb_check`).

## 3. Exactness

TeX defines every key globally at the read-back point. The run with the
child differs only in *when* it makes the definitions of keys it had not
held, and in who defines the keys it never uses.

1. A key the run held at the point: defined at the point, as before.
2. A key the run first uses after the point, by a read: nothing of the
   run touched its location between the point and the use (it is checked
   at the first touch: a read through `load_slow` or a look-up, or a
   definition through `set_loc`). A global definition made at the first
   use is seen by that use and every later one exactly as one made at the
   point, at any group level. The value it replaces is the location's
   value at the point (untouched since), so `wpre:<name>` is the same.
3. A key the run first touches by a definition after the point
   (`Run::set_loc` calls `Run::rb_check` before defining): the
   read-back's definition is made first, globally, with the value it
   replaces kept (`wpre:`, the location's value at the point, untouched
   since), then the run's own definition, as inline. (Until 2026-11-06
   the pass was refused and the run made again inline: biblatex's
   `\begin{document}` then read the `.aux` key set whole in the second
   pass, and ran again on every new key of the file.)
4. A key the run never uses: the child defines it globally at its
   position, right after the run, before its continuation and next
   sibling, so every node after the run sees it, as it sees the run's
   global definition in TeX. Between the run's end and the child nothing
   runs. Not defined where the run defined it after its own run: the
   run's later definition is what TeX leaves.
5. Lines the run interprets (the `.aux` lines it writes, read back as the
   next round reads them): `rb_check` does nothing while a line is
   interpreted, so the line reads the location's value from before the
   read-back, which is what `Run::pre_value` gives it inline (its own
   `wpre:` holds that value); the line's state is rolled back after it.
6. Only keys the run reads through `load_slow` or a meaning look-up
   (meanings, dimensions, glue, token lists: hypdoc's `\\HD@SetMacroIndent`
   sets a dimension); integer registers, read directly by symbols, and
   boxes go inline. Local keys are made at the point by the run itself,
   as inline: only global ones are made late or by the child, and a
   global definition is the same at any group level.
7. No checkpoint is taken while the child is active (`resume.rs`), so a
   resumed run never starts after the point without it.

What the run reads: the flags, and each name it uses (present or not).
A new `\label` adds a key the run never looks up: neither read moves, and
the run stands; the child runs again (cheap) and defines the new key.

## 4. Tests

`auxloop` suite, a test that adds a `\label` and checks the
`\begin{document}` run does not run again, and the PDFs of every edit
equal a cold build's at -j1/-j8.
