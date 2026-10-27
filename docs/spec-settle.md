# Settle before rerun: a reader runs once its writer is done

## 1. The defect

A refuted node is queued at once and run as soon as a worker takes it,
whatever its writer is doing. A value that settles in steps (the page
builder re-placing after each paragraph that moved, a finisher's page
contents) refutes its readers once per step, and each reader runs once per
step. Measured on the book's three chapters (course3, 95 pages): a finisher
runs 10.4 times at one thread and 21.3 times at eight; 2.27 runs per node
at one thread, 5.80 at eight. The eight-thread excess is this alone: at one
thread the writer's steps mostly end before the reader's turn comes.

## 2. The rule

A node woken by another node's commit runs only once that node is not
going to run again on newer information.

- Taken from a deque, a dirty node looks at the node whose commit woke it
  last (`Wake::by`). If that node was re-asked after it woke the node
  (re-asked in its run, `RunningDirty`; or queued again by a wake newer
  than the node's own), the node is put aside: on the waker's list
  (`Node::waiters`), on the pool's (`Shared::waiting`), uncounted among the
  outstanding nodes, its `Node::waits_on` naming the waker.
- When the waker's run ends in any state but queued again (clean, taken,
  gone), its waiters are queued, on the ending worker's `later` deque.
- At a quiet point (nothing outstanding) the waiters still aside are
  released before the `final_only` nodes: their waker never ended so (a
  cycle of waiters, a waker taken by a claim), and nothing else can run.
  `Report::waits_broken` counts them; it is the number to watch, it should
  be near zero.
- A waker in the run whose commit woke the node, clean, parked, taken or
  gone is no reason to wait; nor is a wake with no waker (an edit, the
  environment, a client push). Nor is a waker queued by a wake older than
  the node's: one commit waking a chain of readers at once, each reader
  naming the one before it as its waker (a counter chain shifted by an
  insert, resolved in place), runs side by side as before. Only a waker
  re-asked later than the node holds it back.

One release per wait: the waker's end and the quiet point both take the
`waits_on` token by compare-and-swap. A node taken by a claim or removed
while aside is not queued by the release; given back, the claim's release
queues it as before.

## 3. What it is not

No barrier, no layer order. Nothing idles while any node is runnable, so
the paragraph-to-finisher cycle cannot serialise the document as the
reverted one-active-layer rule did (103 s against 30 on course3 at eight
threads). A node with a wrong guess still runs on it the first time; the
rule removes the runs between the first refutation and the writer's last
step.

## 4. Bars

Measured on the merged tip after this change, the same tree for both:

- course3 -j8 runs per node within 10% of -j1; finisher runs per page at
  -j8 no more than at -j1.
- course3 -j1 runs per node not above 2.27 (no loss at one thread).
- md5s unchanged: course3 7705960b, course1 9bf25c07, amsldoc 98c96e00,
  at -j1 and -j8.
- `waits_broken` under 1% of `runs_waited`.

## 5. Tests

`crates/core/defuse/tests/wait.rs`:

- `a_reader_waits_for_its_writer_to_settle_and_runs_once`: one thread, a
  writer whose own child changes what it read; its reader, re-asked while
  the writer is queued again, runs once, after it; `runs_waited` 1,
  `waits_broken` 0.
- `waiters_are_always_released_on_many_workers`: four workers, thirty
  edits; every evaluation ends with the right answer and the invariants
  hold.
