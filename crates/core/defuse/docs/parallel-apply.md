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
