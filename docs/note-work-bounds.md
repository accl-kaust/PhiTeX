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
