# Deferred reads of page-layer values (the design, 2026-10-28)

Status: decided by the user on 2026-10-27 ("a wrong page number is
unlikely to change a paragraph's width or height"; "every value, not
only labels"). This file is the client's side; the engine is unchanged.
Skeleton `crates/engines/tex/src/defer.rs`, tests
`crates/engines/tex/tests/defer.rs`.

## 1. The defect it removes

Measured on course3 at one thread (branch p2-layer, 2026-10-27): of
2159 paragraph reruns refuted by `count:0`, 1686 changed the page's
items, and in 941 the changed item is the aux `\write` whatsit of a
cleveref `\label`, whose `[\arabic{page}]` was expanded when the
paragraph was typeset. tcolorbox reads `\c@page` into a local macro at
every box (1013 reads, 89 of them later expanded). cleveref's `\label`
saves and restores `\c@page` globally (632 reads, each a pass-through
definition). None of these needs the page number while the paragraph
runs: the digits go into a token list that a finisher expands at
shipout, or into a macro nothing expands, or back into the register.

TeX expands `\the\c@page` at typesetting time, so the value is the page
counter *as of the paragraph's position*: `\c@page` is stepped by
`\@outputpage` at each shipout, and a paragraph is contributed whole, so
the value a paragraph sees is 1 + the pages fired before it. That is
exactly the definition of `count:0` reaching the paragraph's position on
the line, where every finisher defines it at the position of its fire.

## 2. The rule

A read of a page-layer register inside a piece (ops `exec`, `resume`)
made by `\the` or `\number` on the register alone yields a **symbol**,
not digits: a token standing for "the value of register L as of piece
P". The symbol flows through `\edef`, `\xdef`, macro bodies, `\toks`, a
deferred `\write`'s token list and a `\mark` untouched, exactly as a
`\protected` macro does in a list built with expansion (e-TeX §478,
`etex::protect`). It becomes a real read only where TeX needs the digits
now (**forcing**):

- a number scanned: `\ifnum`, `\ifodd`, `\ifcase`, `\numexpr`, an
  `\advance`/`\multiply`/`\divide` operand, an assignment of another
  register, `\romannumeral`, `\number` applied to a list holding it;
- typesetting: the symbol reaching the main loop as text;
- a name built from it (`\csname …\endcsname`), `\string`, `\meaning`,
  `\ifx` against a non-symbol;
- `\immediate\write`, `\message`, `\special`, `\errmessage`: the list
  is printed now (`show_toks_into`, `expand_late` in a piece).

Forcing in a piece is the read the piece makes today (`Run::int_loc`
through `load_slow`, `Ctx::read` at the piece's own position). The
forced value is memoised for the run: later symbols of the same register
are concrete.

**Restoring** the register to its own symbol (`\c@page=<symbol of
\c@page by this piece>`, the shape of cleveref's probe and of every
save/restore) is neither a read nor a definition: the run's map of the
register ([`crate::regfold`]) becomes the identity again
(`Run::reg_restore`), whatever maps it composed since. A restore of
another register's symbol, or of a symbol another piece made, forces.

**Resolution** happens in a finisher (op `finish`), where the page is
known: a symbol met while the finisher expands a deferred `\write` at
shipout (`routine::ship_out` → `write_out` → `expand_late`), a `\mark`
(`\topmark` and kin in the output routine), or any other list, is
answered by `Ctx::read_at(field of L, At::Before(P))`: the definition of
L reaching P's position, which is 1 + the finishers' steps before P. The
finisher reads it; the piece never did. A symbol must never reach any
other operation (the link, the files, a tool): resolution at shipout
leaves none, and meeting one there is a bug (`defer::leaked`, a panic
in every build).

Which registers: `count:0`, and every integer register the output
routine may define, as the finisher's `may_define` lists them from the
routine hint (`hints::routine_hint`): the registers a finisher steps or
sets. A register the piece itself wrote before the read (`\setcounter
{page}{1}` in `\mainmatter`) is concrete from then on: its run-local
value is known.

## 3. What it costs and what it must keep

- The symbol is an internal control sequence
  (`\u{10FFFE}sym\u{2}<loc>\u{2}<node>`, `defer::sym_tok`): one word in a
  token list, interned once per (register, piece), encoded and decoded
  as any control sequence is, so records, whatsits and marks carry it
  unchanged. Its meaning is a new command code `cmd::SYM`
  (`defer::is_sym`), expandable: `expand` forces or resolves it
  (`defer::expand_sym`); in a list built with expansion it is kept as
  `protect` keeps a protected macro.
- Byte-identity: a resolved symbol prints exactly the digits `\number`
  would have printed at typesetting time (the sign and no leading
  zeros, `print::print_int`). The DVI/PDF fixtures and the course
  oracles stay byte-identical at every thread count.
- The log is not a bar, but a symbol printed to the log prints its
  digits where the piece could force it, never its internal name.

## 4. Bars

- `tests/defer.rs` green at -j1 and -j8.
- course3 -j1 (profile fastdev): `count:0` refutations of exec pieces
  under 100 (from 2159), md5 7705960b; the pitems-changed-under-count:0
  count under 100 (from 1686).
- Suites plain, pdftex, pages, edits, latex green on the accl gate; a
  pdfTeX oracle for `oracle/fixtures/dvi/defer.tex` made on accl.
