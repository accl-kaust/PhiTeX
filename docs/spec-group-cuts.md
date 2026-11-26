# Cuts at group closes (2026-11-26)

Status: implemented (`Run::group_cut` in
`crates/engines/tex/src/cut.rs`, tests `cuts::group_cut_*`). The
placement race it exposed at eight threads is fixed in defuse (see
"The relocation race").

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
   groups' bindings. A box appended to a vertical list at its close is no
   cut (`Run::box_end_in`): it is an item for the page, and the candidate
   after it ends the run under that rule's spacing. Cut there, every
   `\hbox` line a macro set on the main list was a run of its own (403
   rests for 8 pages in `pages2::a_page_read_after_pages_fired_in_a_group_ends_the_run_at_the_last`).
   A box set by `\setbox` is still a cut;
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

## Results (n11, after the race fix, against dev at the same base)

The 112-page lab3 deck, a frame inserted (seconds, three interleaved runs):

| | dev | group cuts |
|---|---|---|
| insert -j1 | 7.32, 7.32, 7.75 | 4.60, 4.65, 4.79 |
| insert -j8 | 7.41, 8.07, 7.85 | 4.18, 3.57, 3.47 |
| clean -j1 | 38.9, 34.6, 30.7 | 37.2, 29.0, 29.4 |
| clean -j8 | 22.2, 23.3, 23.0 | 22.2, 21.6, 21.1 |

Shares, on dev with each read taken out of the deck by hand: without the
`.nav` loop (A) the insert is 4.82-6.12 s, without the footline measure
(D) 4.85-5.09 s. They are not additive: a frame ran again whole while
either read moved. With group cuts the deck without the loop takes no
less (4.87-5.22 s): A's cost is gone, and so is D's.

para4000, amsldoc, the TikZ doc (watch, six word edits each, `--shelf`):
edit medians the same within noise at -j1 and -j8 (para4000 12-14 ms,
amsldoc 16-21 ms, TikZ 6-7 ms). Clean builds the same but amsldoc at -j8,
2.61-2.66 s against 2.20-2.35 s: its group-cut rests run 40-60% more runs
(7.7-7.9 k to 10.7-12.6 k) and the node count varies from build to build
at -j8 (2180 and 2723 in two), every PDF byte-identical. Open.

amsldoc's clean build, six interleaved runs each (n11, seconds; nodes;
runs): -j1 1.69-1.80 on dev against 1.76-1.85 (1351 against 1554 nodes;
2725 against 3071 runs); -j8 1.87-2.14 against 2.36-2.54 (about 1880
against 2190-2370; 8.1-9.2 k against 11.0-13.2 k); -j16 2.30-2.42 against
2.71-2.94 (1860-2870 against 2350-3050; 10-14.7 k against 14.6-21.7 k).
At -j1 amsldoc takes 241 group cuts; at -j8 1400-1870 cut events, most
at closes a run reaches only on a guessed state (inside `\verbatim@font`,
`\@donoparitem`, `\endalign`, none at -j1): the rests they make are
remade or run again as the guesses are corrected. Tried, none within
noise of dev while keeping the deck's gain: reads inside the closed
group (non-selective: 233 of 241 closes have them), a cut only where the
placer has settled (no change), a hold of a placement while its placer
is to run again (no change), a minimum of commands inside the group
(amsldoc -j8 runs back to dev's at 400, but the deck's cut moves between
rounds: its clean build at -j8 3.7 s to 6.1-6.5 s), and
`Ctx::settled_here` at the close (the deck's clean build at -j1 hung).

Clean builds at many threads, the merged binary, six at once on 16 cores:
deck2 360 (180 at -j8, 180 at -j16), the lab3 deck 312 (156 and 156), no
wrong PDF. `DEFUSE_PLACE_LOG=1` prints every placement, kept check and
relocation (`place-log:` lines) in any build.

## The relocation race

At eight threads the frame insert gave a wrong PDF in about one edit in
four (deck2: 12 of 48; the lab3 deck: 4 of 12): the slides of one frame
out of order. In every failing run one page finisher stood after its
frame's later pieces instead of right after the piece its page ends at.

Cause (defuse `commit::relocate`), shown by a placement log of every
placement, keep and move: the frame's piece `P` places its continuation
`C`; the page loop places the finisher `F` after a piece of `C`'s chain
(`follow_after`). In the edit, `C`'s placer moved `C`; `relocate` lists
`C`'s subtree, the nodes standing under it by their anchor chain, `F`
among them, then lifts and lands them one by one. Between the listing
and `F`'s lift, the page loop's commit stored `F`'s new anchor (the
piece its page now ends at) and moved `F` there. `C`'s move then lifted
`F` from its new place and landed it under `C` again, with its anchor
elsewhere; nothing asked the loop again. The log of a failing run:
`F` kept under the old piece, then not kept (anchor the new piece),
moved after the new piece by the loop, then moved after the chain's
last node by the continuation's relocation, which had listed 17 nodes.

Fix: a node others placed under the moved node goes with it only while
its anchor chain still reaches the moved node when it is lifted, read
under its segment's lock (`lift`'s `under`). Every order of the two
moves ends where the placer put it: the placer stores the anchor before
it lifts, so a later lift by the other move sees it and leaves the node;
a lift by the other move first is followed by the placer's own lift,
which waits for the landing (`own_write`) and moves it on. No lock is
added and no commit waits for another.

Test: `relocate::a_node_its_placer_moved_while_another_move_listed_it_stays_where_its_placer_put_it`
holds the first move at the node's lift with `Engine::set_move_hook`
until the other placer's commit has moved it: red 3 of 3 before the
fix, green after. On the deck, the code before the rebase that showed
12 of 48 wrong showed 0 of 60 with the fix alone.

Repro: `bench/group-cuts/` (`deck2.tex`, a 27-frame beamer fixture with
overlay rows of pictures; `fixins.py` inserts a frame; `parfx.sh` runs
six watch builds at once, `J` threads each, and prints each build's
md5s). The clean build's md5 is `7930ee1ad4`, the edit's `ea27d8c13a`.
