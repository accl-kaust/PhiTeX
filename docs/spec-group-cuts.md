# Cuts at group closes (2026-11-26)

Status: implemented in `crates/engines/tex/src/cut.rs` (`Run::group_cut`),
tests in `crates/engines/tex/tests/cuts.rs` (`group_cut_*`).

## Why

A piece of the document's text was cut only where its state is easy to
name: a paragraph end at depth 0 (a slice end), a page-builder candidate,
its text's end (`Run::text_cut`), a closed engine scope, a `\write18`.
Inside one macro's expansion there was no cut: everything a command
expands to is one node. When one read inside that expansion moves, the
whole expansion runs again.

Beamer shows it at its worst. Each slide's `\beamer@setuplinks` runs
`\dohead`, a loop over every `.nav` entry; it reads the entry count and
every entry. Each frame's `\beamer@calculateheadfoot` typesets the
footline to measure it, and that reads `\c@framenumber` and the total.
One inserted frame changes those reads in every frame, so every frame
ran again whole (2.5 s and 0.3 s of the 7.2 s insert on the 112-page deck).
Both reads sit inside a group that closes before the frame's real work:
`{... \dohead}`, `\hbox{{... footline}}`.

## Rule

A group of the run's own that closes is a cut, its input going on (a
macro's tokens, the text), where:

1. the run is a document piece (`exec` or `resume`, the document's text);
2. no bar of commands (the text cut's `cut_min`): where a group closes
   is the text's structure, the same whatever the run read before it.
   A bar moved the cut with the work before it. The `.nav` loop runs
   empty in a clean build's first round and full in its second, so the
   pieces after it were made anew (pictures, finishers) between the
   rounds. On the deck, the clean build took 1.8 times as long with a
   bar of 200 commands in the closed group or 1000 since the run's
   start, and as long as before with none. `cut_min = u64::MAX` (never)
   still turns every cut off;
3. what stays open is only groups of its own that own no list (on the
   main vertical list, as at a forced candidate inside groups,
   `Run::cuttable_here`), so the continuation carries no list, only the
   groups' bindings;
4. the state is nameable: `Run::open_state_refusal` (no scan, read-back,
   alignment, math, finisher, second pass, scope effect, ...), no
   children but the ones a cut keeps, no insertion pending, nothing on
   the main list since the last candidate, and more than blank input left.

No environment, package or macro is named anywhere.

## State across the cut

The cut is the text cut's, at no candidate (`Run::text_cut` flag; the
fold takes the last candidate passed, `Run::fold_here`). The rest is a
`Resume` placed after the piece, its continuation the field
`cont:<piece>:k` (`Run::cont_encode`), which already carries everything
a candidate cut inside groups needs:

- the input: every token level's rest (the expansion in progress, the
  `\aftergroup` tokens the close just put back), line and file levels,
  the text's rest;
- the open groups: the nest's levels and the state table's saves and
  local bindings, stamps and group numbers by order only (a loop that
  ran more iterations inside the closed group makes the same
  continuation);
- the conditionals open, and the counters.

Engine scopes are unchanged: the piece closes only its own groups (a
scope's close is a `Close` effect and already ends the run,
`Run::end_at_close`). The rest runs in the same scope as the piece.

The closed group's local bindings are gone at the cut. A loop's counter
and `\iterate` are not in the continuation, so the piece that ran the
loop again hands over the same continuation, and the rest stands unless
a global it reads changed.

## Cost

Hot path: `Run::pop_level` sets a flag; the command loop tests it after
each command. A close with a list of the run's own open (inside a
paragraph or a box: most closes) is refused by the first check, a walk
of the nest. The other checks run cheapest first; the main-list look
and the blank-rest look come last.

Clean build: one more node per cut, and the continuation's encoding
(the input's token levels, the open groups' bindings). Rule 3 keeps cuts
out of paragraphs, boxes and pictures, so ordinary text makes none. The
deck's nodes went from 1391 to 6763, and its clean build took as long.
Measured on para4000, amsldoc and a TikZ document as well (the branch's
report).
