# Settling: the page layer's feedback reaches the text once a pass

## 1. The root issue

TeX's text never sees a page value that is still converging: the aux file
is written at the end of a run and read at the start of the next, so a
paragraph only ever reads a page number a whole pass agreed on. This engine
made every intermediate value a finisher committed visible to the text at
once and re-asked the text against each. Every rerun chased on 2026-10-28
was a consequence: finishers remade or relocated publishing intermediates;
paragraphs reading a page number that changed three times while the
layout settled; the loop at eight threads firing pages on partial input
whose numbers the text ran on; two scheduling rules that tried to order
runs so the intermediates would be fewer (both measured worse, §5).

## 2. The rule

An operation may declare that its definitions settle at quiet points
(`Operation::settles_at_quiet`). The page builder and the finishers do.

- A changed or rebound definition by such a node re-asks a reader of
  another such operation at once: the page layer converges among itself
  inside a pass (the loop reads the finishers' give-backs and page
  parameters, a finisher reads what the finisher before left: counters,
  float queues).
- The same change re-asks any other reader (the text) only at the next
  quiet point (`sched::hold_wake`, `Engine::held`): the wake is listed,
  uncounted. At the quiet point, before the `final_only` nodes run, each
  listed reader's reads of that field are checked again
  (`read::holds`); one that moved is re-asked (`Report::held_fired`), one
  that holds is not: a reader that ran meanwhile for another reason read
  the live value and stands. Firing any makes a pass
  (`Report::held_passes`); the line runs quiet again and the next quiet
  point publishes again, until nothing moves.
- A run's own read that moved across the settling line (a text node whose
  read of a page value changed during its run) is held the same way, not
  re-run at once (`commit`: the causes loop).
- Readers of a settling node's output (`Ctx::placed_output`), of its
  contributions, of structure, are not held: those edges are the page
  layer's own.
- The passes are bounded as latexmk bounds its runs
  (`Limits::settle_passes`, default 5, latexmk's `max_repeat`): past it
  the wakes still held are dropped, the last state is written out, and
  the build reports it (`Report::held_unsettled`: "the page layout did
  not settle after 5 passes", a warning in the log so the PDF of the last
  pass is still written; latexmk's non-zero exit is not matched yet). No
  per-node budget: a first form spent the loop-back cap
  (`Limits::final_reruns`, 10 per node) on held reruns and left readers
  silently stale at eight threads (course3 -j8, 12 passes, 11 undefined
  references, md5 wrong).

Inside a pass the text runs once, on predictions or on the last pass's
values; the loop folds the boxes once (its parks at holes are resumes);
each finisher runs about once, re-asked only by the page layer itself. At
the quiet point the text readers whose page values moved run once, in
parallel. If a box changed height, a second pass. As latexmk: two to three
passes on a cold build, one on an edit with the previous build's values
as predictions.

## 3. Bars

course3 (md5 7705960b) and course1 (9bf25c07), measured on the final
tree, at -j1 and -j8:

- finisher runs a page at or under the number of held passes plus one;
- runs per node at -j8 within 10% of -j1;
- -j1 runs per node not above 2.15 (course3), the page loop's runs not
  above 39;
- `held_passes` at most 3 on a cold build.

## 4. Tests

`crates/core/defuse/tests/settle.rs`:

- `text_readers_of_a_page_value_run_once_at_the_quiet_point`: a page node
  edited; the page node reading it is re-asked at once, the two text
  readers at the quiet point, once each; two held, two fired, one pass.
- `a_held_reader_that_read_the_final_value_meanwhile_is_not_run_again`:
  held, then re-asked by a text change, it runs once on the final values;
  the held wake fires nothing.
- `a_page_node_reading_the_text_is_re_asked_at_once`.
- `held_wakes_are_never_lost_on_many_workers`: four workers, thirty edits.

## 5. The two scheduling rules rejected before this (record)

Both held a re-asked node back by its relation to the node that woke it,
and both were measured worse on course3 at one thread (md5 right):

| | base | wait for the waker (3db3086bc) | wait for the downstream (sched2) |
|---|---|---|---|
| -j1 runs per node | 2.15 (2.27 before first-item keys) | 3.25 | 3.38 |
| -j1 finisher runs a page | 9.86 | 12.3 | 17.9 |
| -j1 page loop runs | 39 | 170 | 41 |
| -j8 runs per node | 5.63 | 6.05 | 8.92 |

The first held finishers behind the loop, which then ran first after each
give-back; the second made a finisher wait for the text it woke, which
ran on the stale value and again on the fresh. Ordering runs cannot fix
what is a question of when values become visible; the engine does not
record an edge's direction, and both guesses were wrong on most edges.
Kept from those attempts: `Worker::seed` counts before pushing, and the
core tests' wall-clock bounds skip with `PHITEX_SKIP_TIMING`.
