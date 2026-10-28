# Scheduling rules measured and rejected

Two rules for holding a re-asked node back were written, tested and
measured on the book's chapters. Both made the counts worse. The record
is here so neither is tried again. The scheduler stays as it is: a node
re-asked by a commit is queued on the committing worker's `later` deque
(FIFO), a node placed on its `local` deque, idle workers steal, nothing
waits for a node.

## 1. Wait for the waker (3db3086bc, reverted in 39c17d37a)

A node taken from a deque while the node whose commit woke it was
re-asked after that (in its run, or queued by a newer wake) was put
aside until that node's run ended.

course3 (md5 7705960b), one thread, deterministic: 3.25 runs per node
from 2.27; the page loop 170 runs from 45; finisher placements 375 from
202; exec 2734 runs from 1732. Diagnosis from the wake log (4558 waits):
91% were finishers parked behind the page loop; with them held back the
loop ran first after each finisher's give-back, and each of its runs
re-asked the finishers through the page counters and the text through
the page number. The loop needed to wait for the finishers, not the
finishers for the loop.

## 2. Wait for the downstream (branch sched2, never merged)

A node re-asked while a node its last commit woke was still to run, and
had not committed since that wake, let that one go first. A chain shifted
by one edit ran side by side (the serial guard); a fold woken by many
readers ran once after them.

course3, same recipe, md5 right at both thread counts:

| | base fdbdf7fab | downstream rule |
|---|---|---|
| -j1 runs per node | 2.15 | 3.38 |
| -j1 finisher runs a page | 9.86 | 17.9 |
| -j1 page loop runs | 39 | 41 |
| -j8 runs per node | 5.63 | 8.92 |
| -j8 finisher runs a page | 23.0 | 42.0 |

Without the serial guard the first form serialised every chain woken by
one event (tests/core.rs an_insert_shifts_a_counter_chain_side_by_side
568 ms for a 63 ms bar; course3 -j1 past the engine's time limit at
229 s). With it, the ledger shows why it is still worse: a finisher
re-asked waits for the text pieces it woke, which run on its stale
values and then again on its fresh ones. Waiting for readers pays only
where the readers feed back into the writer (the page loop and its
finishers); on a feed-forward edge, most edges, the writer must go first.

## 3. What was learned

- At one thread the FIFO order already runs a fold's readers before its
  rerun; the loop's 39 to 45 runs are not a scheduling loss.
- The eight-thread excess (the loop 152 runs, finishers 23 a page) is
  the loop starting on partial input: it parks at each hole and resumes
  as profiles arrive, firing pages that move. A rule for that is about
  readiness of the loop by position (no piece pending before its place),
  not a generic wait; it is not written.
- A generic "hold a node back" rule needs the direction of the edge
  (does the reader feed back into the writer?), which the engine does
  not record. Two forms that guessed it were both wrong on the majority
  of edges.

## 4. Kept from the attempt

- `Worker::seed` counts the nodes outstanding before pushing them: a
  count that reaches zero ends the evaluation, so no entry may be seen
  before its count. The rule's own release paths had this hazard the
  other way round and lost runs; the base's seed had it only on the one
  thread that seeds.
- tests/core.rs: the wall-clock bounds are skipped with
  `PHITEX_SKIP_TIMING` set (a loaded machine); the run counts are
  checked always; the gate on accl leaves timing on.
