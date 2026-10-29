# The page loop's work bounds left red (note, 2026-10-28)

Status: a record for the next design pass on the page loop's wakes. Two
suite tests stay red by decision. Their cause is the loop's wake
scheduling, not a defect to patch. Numbers are from reds3 (ddd8f49d,
accl job 20712).

## pages2 `the_loop_parks_at_holes_and_resumes`

The bound is loop runs at most pages + 2, at eight threads, over 120
paragraphs. Measured: 31 loop runs for 5 pages. An earlier run of the
same tree gave 18; the count varies with the order the pieces commit.

At -j8 the pieces commit out of text order. Each commit of a piece
before the loop's cursor changes a contribution the loop already folded
(`Cause::Range` on `vprof`) and re-asks it. The loop does not stay
parked at the first hole and resume once that hole fills. Runs grow with
the number of out-of-order commits, not with the pages.

The `wakes_behind` attempt (branch `yield`, docs/note-loop-wake.md)
woke a parked loop only when the piece it waits at commits. It regressed
the -jN wall and is shelved.

## pages `vsize_document_pages`

The bound is runs at most 1.1 per node. It passed in job 20712 (106 runs
for 99 nodes, bound 108.9) and failed on the gates before (111 for 99).
It is the same scheduling dependence. The extra runs were:

- `pages` 51 five times: `Range vprof` three times, the cause above.
  `Output` twice: the routine gives `\vsize` back, and the first guess of
  the give-back lacks the marker. Guessing, then reading the finisher's
  output, is the design (spec-settle.md §2.1).
- `exec` 44 once: `\message{\the\count1}` at `\bye` reads the count the
  finishers step.
- `finish` 96 once.

Without the `Range` reruns the test sits at about 1.04 a node (103 for 99). A rule
for the loop's wakes that bounds them by the holes it meets, not by the
commits before its cursor, closes both tests.

## cuts `a_moved_page_fire_relocates_the_rest_and_keeps_the_finisher`

The bound is at most one finisher made anew when a paragraph inserted
before the group moves the page fire. Measured (job 20750): 13
finishers made for 8 pages, 12 finisher runs, 2 loop runs, 0
placements moved; the output is right. A page's key is kept by the
item it starts at (spec-continuations.md §2.4), and the insert moves
every page's start, so every page needs a new key. Even counted that
way, 13 is more than one a page: the loop's second run re-fires pages
with new starts again, and about five pages get a finisher made twice
in one rebuild. A bound that hid this (two a page) was not taken.

## Loop rests and text-side holds (branch docs-spec, 2026-10-28)

Where it stands, at wip12 (5112f74c on branch docs-spec):

- A page loop that has to wait for a routine's give-back stops there. It
  places a loop rest: its own node, `pages:rest:<parent>`, holding the
  state, the page key and per-contributor key floors. Each rest folds
  the pages after that page, so a loop run re-emits only its own pages.
- The speculation depth adapts. It is 0 on one thread and after a miss,
  and doubles after each give-back the guess got right.
- Text that reads a settling node's prediction at depth 0 waits for
  that node's commit (`Ctx::wait_for_settling`, `Shared::waiting`).
- wip12 fixes a rest's first finisher. It is now placed after the
  finisher of the page before it when both pages fire in the same
  piece (`LoopState::after`). Before the fix it was placed after the
  piece itself, which put it ahead of the earlier finishers. Its routine
  then read the column state from before them and gave back the same
  `\clearpage` for ever. floats2col cut at 24 paragraphs never ended;
  it now matches the base (md5 b69b81b2).
- floats2col (194 fires): -j1 finishes in 0.39 s. The finisher runs
  216 times (1.11x fires) and the page loop 347 times. The PDF equals
  pdfTeX's except for the date and ID. -j8 gives the same PDF in 1.4 s,
  but the finisher runs 2242 times (11.6x, over the 3x bound). The text
  runs 5238 times for 262 live nodes. The layout does not settle within
  the 12 passes, and held wakes are dropped.
- The -j8 cost is text speculation on the float machinery. The wake
  counts:

  | Wake | Count |
  |---|---:|
  | exec -> exec on `\@freelist` (a cascade through every later float) | 2061 |
  | exec -> exec on `\@currlist` | 724 |
  | exec <- finish on `\@freelist` (first causes) | 294 |
  | pages <- finish on `pages:out` | 899 |

  Text placed past the loop's frontier reads `\@freelist` before the
  finisher that will stand between it and its definition exists (the
  "no finisher yet" case). When that finisher is placed and run, the
  read changes, and every later float node re-runs.

Steps left, in order:

1. Frontier holds (started on branch docs-spec-frontier, wip13,
   incomplete and not built). Settling nodes' defined and predicted
   fields are marked in an engine bit set (`Engine::settle_marks`; mark
   in `commit` for `settles_at_quiet` ops and in `predict`). The page
   loop publishes its frontier through the product at commit: the hole
   node at `Fold::Hole`, none at the end, unchanged at `Fold::Wait`.
   - A non-settling reader of a marked field placed strictly after the
     frontier, at depth 0, stops (`Ctx::wait_frontier`). It parks on a
     frontier waiters list.
   - The commit that moves the frontier requeues the waiters at or
     before it. Quiescence clears the frontier and releases them all, so
     a stall cannot deadlock.
   - Count the reads in `Engine::frontier_reads`: held, past the
     frontier at depth over 0, a settling prediction, a settling
     definition. Print them in acct.
   - Open risk: before the first finisher commits, the mark set is
     empty and text runs ahead unheld. Measure how much of the -j8
     waste this leaves before seeding the marks any other way.
2. A/B on mc: floats2col at -j1 and -j8 (finisher runs at most 2x and
   3x the fires, the PDF right), amsldoc and para4000 at -j1, -j8 and
   -j16 (wall and Ir no worse), amsedit and editkinds.
3. Un-ignore `latex_twocol_floats_runs`. Fix
   `a_routines_vsize_reaches_the_builder_in_fold_order` (red at wip11:
   loop runs at -j1 at most 88).
4. Then the reds above (`the_loop_parks_at_holes_and_resumes`,
   `vsize_document_pages`, the cuts bound), Grail (-j1 finishes, -j8
   under 43 s), and curve2e -j1 (400 s on base too, its own cause).
5. The watch's page counter: report the settled frontier in pages, not
   the largest `p.normal`.
