# Cuts at group closes (2026-11-26)

Status: implemented on branch `beamer-cuts` (`Run::group_cut` in
`crates/engines/tex/src/cut.rs`, tests `cuts::group_cut_*`). NOT MERGED:
at eight threads it exposes an engine placement race (see "Open defect").

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

A level of the run's own that closes is a cut, its input going on,
where:

1. the run is a document piece (`exec` or `resume`, the document's text);
2. the level is a brace group (`{`, `\bgroup`) or a box (`\hbox`,
   `\vbox`, `\vtop`), opened in a macro's expansion (a token list
   pushed by a macro call), and that list is still being read at the
   close: the group lies within one expansion (`Input::toks_open`,
   `Input::toks_going_on`; token lists are numbered as pushed). Not a
   `\begingroup` scope, not a group opened at a line's brace or across
   expansions (an environment's `\begin` and `\end`), never math, an
   alignment, an insert or the output routine;
3. no bar of commands (the text cut's `cut_min`): see "Why no bar";
   `cut_min = u64::MAX` (never) still turns every cut off;
4. what stays open is only groups of its own that own no list (on the
   main vertical list, as at a forced candidate inside groups,
   `Run::cuttable_here`), so the continuation carries no list, only the
   groups' bindings;
5. the state is nameable: `Run::open_state_refusal` (no scan, read-back,
   alignment, math, finisher, second pass, scope effect, ...), no
   children but the ones a cut keeps, no insertion pending, nothing on
   the main list since the last candidate, no `\csname` look-up put off,
   and more than blank input left.

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

## Why no bar, and why not every close

Measured on n11, `-j1` unless said (release build, rule toggled in the
same binary; seconds):

| rule | deck clean | deck insert | amsldoc clean j1 / j8 |
|---|---|---|---|
| none (dev) | 29.3 | 7.1 | 1.85 / 2.2 |
| every close, no bar | 28.0 | 4.4 | 2.9 / 5.8 |
| bar 3000 since the run's start | 29.8 | 6.7 | - |
| bar 1000 since the run's start | 51 | 5.3 | - |
| bar 200 in the closed group | 49 | 4.9 | - |
| one expansion, any group, no bar | 27.6 | 4.4 | 2.7 / 4.9 |
| one expansion, brace or box, no bar (this rule) | 27.1-29.5 | 4.4-5.0 | 1.9-2.2 / 2.4-2.6 |

- A bar of commands moves the cut with the work before it. The `.nav`
  loop runs empty in a clean build's first round and full in its second,
  so the cut appears in round 2 and every slide's pictures and finishers
  are made anew (deck clean build 1.5-1.8x).
- Every close cuts LaTeX's `\begingroup` scopes too: one at every
  paragraph's start (ltpara's `\group_begin: \tex_par:D \group_end:`),
  `\@ifundefined`, keyval: 3017 cuts in amsldoc, each a node that
  re-reads its working set (j1 +40%, j8 2x).
- Brace groups and boxes inside one expansion: 234 cuts in amsldoc; the
  deck keeps `\beamer@setuplinks`'s `{...\dohead}` and
  `\beamer@calculateheadfoot`'s `\hbox{...}`.

Deck insert at -j8: 7.9-9.6 s on dev, 3.7-4.1 s with this rule.

## Cost

Hot path: a level's close compares its group code and, for a brace
group or a box with a numbered opener, walks the input's levels; a
group's open records its opener the same way. The command loop tests a
flag once per command. para4000's edit and clean times are unchanged.

## Open defect (why this is not merged)

At eight threads, the frame insert on decks with overlay pictures gives
a wrong PDF in 25-40% of runs (deck2 fixture: 7 of 24; the lab3 deck: 4
of 12); with the rule turned off in the same binary, none (0 of 36). The pages of one frame's slides come
out rotated. In every failing run exactly one finisher stands at the end
of its frame's chain of rests instead of right after the piece its page
fired in (`PHITEX_PAGE_PLACE` names the right cursor; the node order has
it after the cursor's whole continuation chain). The likely mechanism
(not proven): the loop's commit keeps the finisher's old place (`commit.rs`, the kept check of a
`FollowAfter`): its `stranded` check sees the nodes between its floor and
it in its own segment under the lock and in other segments only through
`foreign_between`, read before the lock. The piece's continuation
(`Follow`, slot 2) placed concurrently by the piece's own commit is not
seen, and nothing re-asks the loop afterwards. Read checks
(`PHITEX_CHECK_READS`) find nothing stale; the order is wrong, not a
read. Cuts at group closes make many short pieces whose pages fire just
before their continuation is placed, which is that window.

Repro: `bench/group-cuts/` (`deck2.tex`, a 27-frame beamer fixture with
overlay rows of pictures; `fixins.py` inserts a frame; `parfx.sh` runs
six watch builds at once at -j8 and prints each edit's md5). The cold
build's md5 is `7930ee1ad4`, the edit's `ea27d8c13a`.
