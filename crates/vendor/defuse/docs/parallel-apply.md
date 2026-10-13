# The apply phase and what a parallel apply needs

Measured on the `typing` example at 1,000,000 lines, 16 threads, cold
evaluation (local machine, shared with other builds, so absolute numbers
are rough; the split is stable).

| phase | time | notes |
|---|---|---|
| run_batch | 5.5 s | the operations' runs; scales with threads |
| apply_batch | 11.5 s | one thread, every batch result in node order |
| children | 3.25 s | records 0.9 (treap insert 0.73), binding 2.06 |
| defs | 1.55 s | definition indexes and the re-bind search per new definition |
| claims | 1.36 s | claim recompute and flush |
| finish | 0.99 s | heights, waking waiters, settling |
| uses | 0.57 s | the users index |
| rest | ~3.7 s | apply's own bookkeeping: snapshots, taint, memo, flags |

`Report::applied_widths` on this workload: 5,645 steps, 1,185 of them
applying one run and 1,080 two or three; three steps apply 32k, 65k and
131k+ runs. The apply time sits in the wide steps; the step count in the
narrow ones (where the executor's hand-off per step is the cost).

The floor is about 10 us per applied node, spread over O(log n)
operations on five B-tree indexes (`defs_ix`, `local_ix`, `global_ix`,
`users`, `chain_users`), the order treap, two hash maps (`ids`,
`origins`) and the queue. No single structure dominates.

## What does not help

- **One order arena per unit.** All but a few thousand nodes of a
  document are in its one unit; a client's documents are one unit each.
- **Sharding the maps by field with a lock per shard.** The apply is one
  thread; locks give nothing. Staging a batch's inserts per shard and
  applying them in parallel at the end of the batch breaks
  read-your-writes inside the batch: a later sibling's walk must find an
  earlier sibling's definition, and `add_def`'s re-bind search reads the
  index it is inserting into.
- **Run insert of created children** (landed): strictly less treap work,
  no measurable change (3.4 children per run on average).

## What would

A partition of a wide step's results by label range. Every index key
carries the label (`(unit, field, label)`, `(unit, field, depth, label)`,
`(site, field, label)`), so partitions own disjoint key ranges; the
treap splits by label into one subtree per partition; node records and
ids are staged per partition and merged in one sequential pass.

The subtle part is a binding that crosses a partition boundary: a reader
in partition B whose reaching definition is a new definition in
partition A, applied concurrently. The walk in B must either see A's
definitions (partitions apply in label order and B reads A's finished
state, which serialises A before B) or the engine must re-bind B's
readers in a sequential fix-up after the parallel phase (the ordinary
`add_def` move of users after a definition, run once per boundary). The
second is the safe one: parallel phase for everything inside a
partition, then one sequential pass over the boundaries.

Unsafe would be confined to one module handing out disjoint ranges of
the node table and the treap arena; the wasm build keeps the sequential
path. Estimated at several days with the fuzz and gate suites as the
safety net.

## What landed (branch parallel-apply, interim)

A wide non-speculative batch (at least 256 results, nothing speculative
live) is applied in slices of consecutive results of one unit, in text
order:

- Writes a slice makes to nodes outside its range (dirty, raise, park,
  wake, output readers) are deferred to a tail, replayed after the merge.
- Results that read ranges or the environment, change effects or claims,
  publish, loop back or need an application are applied whole, after the
  tail.
- When `Executor::parallelism() > 1`, each slice runs in an overlay
  engine over the frozen batch-start engine. Nodes, auxes and tree nodes
  are copied on write, and new nodes go into reserved slots. A slice that
  meets an unsupported change aborts and its results are applied whole.
  The slices run through `Executor::for_each`.
- The merge goes in slice order. Each structure is merged as its own
  parallel task. Node and aux records are written with `par::scatter`.
  A unit's order tree is split at the slices' label ranges, each piece is
  replayed concurrently, and the pieces are joined.
- At parallelism 1 the slices apply in place, with no overlays.
- `unsafe` is confined to `src/par.rs`.
- Public API added: `Executor::parallelism` (default 1), plus the obs
  variants Tail, UseRecord, SliceAbort (counters) and Tail, Slices,
  Merge (spans).

Results are unchanged: fuzz passes on both pools under seq, spec, random
and par, also with `DEFUSE_PAR_ORDER=1`, and run counts move by under
0.002%.

### Measured on master dc331a5 against the branch

Typing example, cold build, release build, pinned to `taskset -c 0-11`.
Times are the cold phase. Loads are the 1-minute average at the start of
each run.

| lines | threads | master | branch | load average |
|---|---|---|---|---|
| 100k | 1 | 2.93 / 2.45 s | 1.95 / 1.92 s | 7.3, 16 |
| 100k | 12 | 1.72 / 1.43 s | 1.00 / 1.20 s | 7.3, 15 |
| 1M | 1 | 33.1 / 34.1 s | 25.0 / 26.0 s | 7.1, 13–16 |
| 1M | 12 | 16.5 / 16.9 / 18.6 s | 12.1 / 12.7 / 12.3 s | 8.4–10.9 |

Profiling build, 1M at 12 threads (load average 10):

- apply_batch: master 11.15 s, branch 5.98 s.
- evaluate: master 18.4 s, branch 12.2 s.

An earlier 1M pair at 12 threads, run while the machine was busy with
other work, read master 17.0 s against branch 19.0 s. Three quieter
repeats are the ones in the table.

### Open items

- **Overlay overhead.** At 12 threads the slices take about 1.8 s. The
  same work in place, at 1 thread, is about 5 s, against 8.4 s through
  overlays. The cost is the copy-on-write maps and the base fallbacks.
- **Scatter.** Writing the node and aux records takes about 0.6 s of
  the merge. The parts could be handed to the merge tasks instead of
  getting a pass of their own.
- **Sort key.** Sorting a batch's results takes about 0.4 s. The key
  could be stamped in `run_job`.
- **settle_deferred** has no parallel read phase.
- **Slice threshold.** T = 256 and S = clamp(n/32, 64, 256) are not yet
  measured break-evens, for example on Phi's width histograms.
- Speculative steps are never sliced. On a speculation-heavy schedule
  those batches keep the sequential apply.
