# A linearly scaling core

Design for a rewrite of the evaluator's core around one rule. The semantics
stay those of the current engine: order is used only for binding, nothing
waits except on what its binding crosses, the schedule changes speed and
never results, and evaluation is eager. What changes is how those rules are
enforced: by one mechanism instead of a flag per case, and without a global
step.

## Why the current core cannot scale

Measured at 1M lines on 12 physical cores (profiling build, parallel-apply
branch head, pinned):

| phase | 1 thread | 12 threads |
|---|---|---|
| run_batch | 16.0 s | 4.2 s |
| apply_batch | 5.8 s | 6.4 s |
| gate, select, settle, sweeps | 1.6 s | 1.7 s |
| evaluate | 23.4 s | 12.3 s |

Three structural reasons, in order of size.

1. **The step.** The engine runs in lock-step batches: every ready node at a
   level runs, then the main thread applies all results, then the next level.
   Workers idle through the apply and through every narrow step. On Phi's
   graphs 54% of steps have one ready node; our 1M build has a 359-step
   critical path. Amdahl caps this regardless of the thread count.
2. **The sequential apply on a cold cache.** The main thread applies results
   the workers produced, and both Phi and the apply agent measured it growing
   with the thread count although it runs alone: its cache was evicted by the
   run phase.
3. **The height heuristic.** Levels, raises, floors, lifts and gates exist to
   avoid running a node before its definers. They cost a select phase per
   step and a web of special cases, and they still leave 0.85 stops per node
   on Phi's graphs.

Everything else (the value table's locality, the index B-trees, the treap)
is a constant factor and carries over or improves independently.

## The rule

A run is a transaction.

- It reads facts from a versioned store and records every read as a
  predicate: the question asked and the answer it got.
- It commits only if every recorded predicate still holds against the store
  at commit time.
- A committed result stays valid while its predicates hold. A fact change
  that breaks a predicate invalidates the result, and nothing else does.

Every current mechanism is one of four uses of this rule.

| today | under the rule |
|---|---|
| a wait (End::Wait, Gated, WaitRange, WaitEnv, park/resume, reads_hold) | a predicate whose subject is in work: the run parks on the subject and resumes with its read log intact |
| speculation (assumptions, confirmations, mispredictions, spec_blocked, mispredicted, F_NOSPEC) | a predicate answered with a guess; commit validates it; a wrong guess re-runs the node |
| edits (touch_log, relabels, binding_epoch, F_REBIND, revalidate, stale-site checks, claim_seen) | fact changes that break predicates |
| ordering (height, level, raise, floor, lift, gate, select, steps) | gone: a dirty node runs at once and parks at its first unavailable read |

Today three partial validators exist for one job: reads_hold for a resume,
stale_assumptions_hold for a speculation, and the epoch re-walk in apply.
There will be one read log and one validator, used for commit, resume,
speculation and edits.

## Predicates

The store answers a closed set of questions. Each answer is recorded with
the version of the shard it came from, so validation is a version compare in
the common case and a re-ask only when the shard moved.

| predicate | answer | today's counterpart |
|---|---|---|
| Definer(unit, label, scope path, field) | the nearest earlier definition in scope: a site and its value, a pending node, or the unit's start value | walk, K_FIELD uses, pending_for, may_define |
| Output(node) | the node's output once done | K_OUTPUT uses, out_readers |
| Range(unit, lo, hi, field) | the fold of contributions over the range, valid only when no node in the range is in work | acc_range, watches, stab_*, ord_fold |
| Siblings(node, n) | the n following siblings and their text | Ctx::following, claim_seen, following_seen |
| Structure(unit, lo, hi) | the scope boundaries and claims inside a range | binding_epoch, effect_changed, stream claims, relabels |
| Env(field) | the environment value, or absent, or unknown | env_value, EnvProvider |
| Loop(field) | the loop-back's current round value | loop_vals, LoopStart |

A run that writes facts (definitions, publications, contributions, children,
claims, scope effects, declarations) records them as the transaction's write
set. There is no separate "shape change" path: a claim is a fact about
Structure, a child is a fact about the order, and both break the predicates
of whoever read them.

## The store

### Shards

The order of a unit is cut into shards by label range. A shard owns:

- its piece of the order sequence (the B+tree piece the apply branch already
  has, now permanently owned rather than borrowed per slice);
- the definer index for every field over its labels;
- the subscription index: for each fact key, the nodes whose predicates
  depend on it (today's users, chain_users, out_readers and watches);
- a version counter, bumped by every commit that touches the shard;
- a summary for cross-shard queries: per field, whether the shard holds a
  definition or a pending definer; the count of nodes in work; the scope
  depth delta across the shard.

Shards split when they exceed a size bound and merge when they shrink, the
same way the current tree pieces would. A unit of one page is one shard; the
1M build is a few thousand.

### Cross-shard binding

Definer(label) is answered by the shard holding the label when it has an
earlier definition in scope, and otherwise by walking the shard summaries
backward. The summaries form a segment tree over the shards of a unit, so
the walk is logarithmic in the number of shards. The answer records the
versions of every shard consulted, including the empty ones, because an
insertion into an empty shard is exactly what changes the answer.

Scope boundaries cross shards the same way: the depth delta summary tells
whether a scope opened in an earlier shard is still open at the label.

### Commits

A commit takes the write locks of the shards it writes: the node's own
shard, plus the shard of each definer it shadows. Reads need no lock; they
read the version first, the fact, and the version again, and retry if it
moved. Writes are rare relative to reads (Phi's graphs: 455 reads per run,
0.9 definitions), so the lock is uncontended in the common case and the
sharding bounds the rest.

Under the lock, the commit validates the read log (version compare per
shard, re-ask on mismatch), applies the write set, bumps the version, and
marks every subscriber of every changed key dirty. The worker that ran the
node does this, so the data it touches is in its cache.

A commit whose validation fails discards the result and re-queues the node.
To rule out livelock between two nodes that keep invalidating each other,
validation failures are resolved by order: the earlier node's commit wins and
the later one retries. Progress is guaranteed because the earliest dirty node
in a unit always commits.

### Values

Each worker owns a value arena; a value id carries the worker's tag in its
high bits. Interning is per worker, with a global merge at sweep time rather
than a shared table on the read path. The sweep is epoch based: a value is
free once no committed result and no parked continuation of an older epoch
references it. This removes the value table's share of last-level cache
misses (measured at 30%) from the shared path.

## The scheduler

There are no steps. Each worker has a deque of dirty nodes and steals from
others when empty. The loop is:

1. Pop a node. Run it against the current store.
2. At a read whose subject is in work, park: register the continuation on the
   subject (a node, or a shard range for Range predicates) and go to 1.
3. At the end, commit. On success, push every node the commit woke onto this
   worker's deque (they will run on this worker next, with the data hot, or
   be stolen). On failure, re-queue the node.

A node is woken by the commit of the subject it parked on, and by nothing
else. The current engine already has the continuation (Ctx::park, resume);
it is no longer an optimisation but the only way to wait.

Speculation is a scheduling choice, not a mechanism: when a worker would
otherwise park, it may answer the predicate with the previous value and
continue. The read log records the guess; commit validates it like any other
predicate. The budget per node is the only policy.

The main thread is a worker. In the browser it is the thread that must not
block, so it runs the loop in slices and yields; the executor seam stays
where it is.

## What the rule guarantees

- **Schedule independence.** A committed result depends only on the facts it
  read, and every fact is a function of the final structure: definitions by
  order, scope by boundaries, ranges by their final contents. Two schedules
  reach the same fixpoint because every run that read a fact the fixpoint
  later changed is invalidated and re-run. Conflicts (two definitions of one
  field, an undeclared definition) are reported at the end from the final
  store, in order, not from arrival.
- **Nothing waits except on what its binding crosses.** A run parks only on
  the subject of a predicate it asked. There is no level to wait for.
- **Eagerness.** Every dirty node is in a deque; nothing is held back.
- **Incrementality.** An edit is a commit with a write set. The nodes it
  invalidates are exactly the subscribers of the changed keys, which is what
  the users index gives today, minus the epoch re-walks.

## Cost model

- Work: one run per node plus one partial run per park, plus re-runs on
  invalidation. Parks are bounded by the number of unresolved inputs a node
  reads, which is what the current engine already pays as stops (0.85 per
  node on Phi's graphs, under the height heuristic). Continuations make a
  park cost its prefix once, not per attempt.
- Time: the critical path of the dependency graph, with no barrier added.
  Cycles should scale linearly with workers until the critical path binds;
  wall time still bends under the all-core clock and SMT, measured at about
  55% per run on 12 cores.
- Apply: gone as a phase. Commit cost is per node on the producing worker.
- Memory: the shard indexes replace the global composite B-trees; a shard's
  indexes fit a worker's L2 for the sizes we run.

## Risks

- **Wasted prefixes.** Starting every node immediately runs prefixes that
  the height heuristic avoided. Mitigation: the continuation makes the prefix
  pay once, and a node's previous run gives a free hint of its first blocked
  read, which the scheduler may use to delay it. This is a policy knob, not a
  mechanism, and it is measured on Phi's graphs first.
- **Hot shards.** A cold build inserts at the end of one unit. Mitigation:
  shards split under write pressure, and the commit lock is per shard.
- **Cross-shard queries.** The summary walk is the one genuinely new
  algorithm. It is logarithmic and read-only, and it is the piece to
  prototype and benchmark first.
- **Rewrite size.** The engine is 8.5k lines with many tested behaviours.
  The test harness, 79 engine tests, 34 client toys and the differential
  fuzzer against the reference interpreter under three schedules, defines
  the semantics well enough to build the new core beside the old one and
  gate every milestone on it.

## Plan

Build `src/core/` beside the current engine, behind the same public API.

1. **Store and binding.** Shards, definer index, summaries, the Definer and
   Output predicates, commits with validation. Single worker. Gate: the
   engine tests that need only definitions and scopes.
2. **No-barrier scheduler.** Work-stealing deques, parks, wakes, commit on
   the producing worker. Gate: typing cold build at 1 and 12 threads, cycles
   per run and idle fraction, against the current numbers.
3. **Structure.** Children, claims, scope effects, ranges, continuations and
   declarations as predicates. Gate: the client toys and the fuzzer.
4. **Loop-backs, environment, speculation, edits.** Gate: the full suite and
   the fuzzer under all three schedules, plus Phi's three reference graphs.
5. **Browser.** The wasm threads build, the sliced main-thread worker, the
   4 GB memory bound.

Each step lands behind the API; the old core is removed when step 4 passes
the gate.
