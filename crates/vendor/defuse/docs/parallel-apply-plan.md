# Brief: a cheap, parallel apply phase

You are redesigning the apply phase of `defuse`, an incremental, parallel,
field-level def-use evaluator in Rust (this worktree, branch
`parallel-apply`, base commit 3c631fb of master). Read
`docs/parallel-apply.md` first: it has the measured cost split and what
has already been tried. This file is your working plan: keep it updated
with decisions and measurements as you go, and commit it with the code.

## The goal

The cold evaluation of the 1,000,000-line `typing` example on 16 threads
spends about 1 s running operations and about 11.5 s in `apply_batch`,
on one thread. The three wide steps (32k, 65k and 131k applied runs)
hold that time. The goal is an apply whose cost on a wide step divides
by the thread count, and whose cost per applied node on narrow steps does
not rise:

- 1M, 16 threads, cold: `apply_batch` total at or under 4 s (from 11.5),
  measured with the profiling build's span table in the same session as
  a baseline run of the base commit, back to back (the machine is shared;
  only back-to-back comparisons mean anything).
- 1M, 1 thread: not more than 5% slower than the base commit.
- 100k, 16 threads, and the edit timings (`--edits`): not slower.
- Results identical to the base commit on every program: the fuzz pools,
  the generated-program gates, the three schedulers, 1/4/16 threads.
- The wasm32 +atomics build in `scripts/checks.sh` keeps building; the
  browser with threads is the user's real target, so the parallel apply
  must go through the `Executor` seam (`src/sched.rs`) like `run_batch`
  does, with a sequential path when the executor has one thread.

## Your freedom

The user's words: you have freedom to change anything you wish. The
design below is one that would work; it is not a mandate. You may
restructure the engine, replace the indexes and the order treap, change
how a batch is chosen and applied, change the `RunResult` contract with
`Ctx`, move work between the run and apply phases, or drop the notion of
a separate apply phase altogether, if that gets a cheaper result. The
only things that stay are in the next section. When you change a project
constraint (a dependency, an edition, a toolchain version, the wasm
build, the public API that `examples/` and `tests/` use), say so in the
report with the reason; prefer not to, but do it when it is the honest
way to the goal.

## Rules that stay

- Results are identical to the base commit on every program under every
  schedule and thread count: this is the engine's contract.
- `unsafe` is allowed (the user approved it on 2026-10-13); keep it in
  one module with a safety comment per block, and keep a sequential path
  that the wasm build can use. Prefer safe code where it costs nothing:
  contiguous per-partition ranges of a `Vec` split with
  `split_at_mut`/`chunks_mut` need no unsafe.
- `scripts/checks.sh` must print `CHECKS_OK` before every commit you
  consider done (fmt, pedantic clippy on default, `--features profile`
  and wasm32, debug tests, wasm32 +atomics build-std build). If you
  change what the project builds for, update the script in the same
  commit and say so.
- Every cargo invocation and every engine binary goes through
  `scripts/nc.sh` (cargo +nightly) or `scripts/run.sh` (any command):
  they run in a capped systemd slice and a sandbox where only the repo is
  writable. Outputs go under `target/`. A 1M run takes about 3 GB; do not
  run two of them at once.
- Commit as you go on this branch, one coherent step per commit, tests
  green at each. Describe the change in the message; no session links,
  no attribution lines. Do not push, do not touch master, do not message
  other sessions.
- Schedule changes speed, never results. The order is used only for
  binding. Nothing waits except on what its binding crosses. Evaluation
  is eager. These are the engine's semantics; keep them.

## Where the time goes (1M, 16 threads, base commit)

| part of apply | s | what |
|---|---|---|
| children | 3.25 | `set_children`: records 0.9 (treap insert 0.73), binding 2.06 |
| defs | 1.55 | `add_def`: the definition indexes and the re-bind search per new definition |
| claims | 1.36 | claim recompute and `flush_claims` |
| finish | 0.99 | `finish`: heights, waking waiters, settle heap |
| uses | 0.57 | `set_uses`: the users index |
| rest | ~3.7 | apply's own bookkeeping: touch snapshots, taint, memo, flags |

About 10 us per applied node over five B-tree indexes (`defs_ix`,
`local_ix`, `global_ix`, `users`, `chain_users`, all keyed with the
label), the order treap (`src/tree.rs`, `Arena`, augmented summaries
`OAug`/`OSum`, a lazy `order_dirty` flush), the `ids` and `origins` hash
maps and the height queue. No single structure dominates, so the win is
in doing the writes on many threads, not in one faster structure.

Already landed and measured as not helping: per-unit order arenas (one
unit holds nearly every node), sharded maps with staged inserts applied
at the end of a batch (breaks read-your-writes: a later sibling's walk
must find an earlier sibling's definition, and `add_def`'s re-bind search
reads the index it inserts into), and the run insert of created children
(`Arena::insert_run`, strictly less treap work, no measurable change).

## Code map

- The batch loop: `src/engine.rs` around the `RunBatch` and `ApplyBatch`
  spans (near line 8080). `run_batch` dispatches jobs through the
  `Executor`; `run_job(&self, ..)` runs on a frozen `&Engine`. Then every
  `RunResult` goes through `apply(&mut self, r)` (line ~6593) in node
  order.
- Inside `apply`: `apply_partial`, `set_uses` (~5987), `set_children`
  (~6077) which calls `create_raw` (~3006) and `bind_created` (~3207),
  `add_def` (~4170), `finish` (~7060), `flush_claims` (~4878).
- The precedent for moving work to the run threads: `RunResult` already
  carries `child_ids`, `child_heights` and `old_sites`, computed in
  `Ctx::finish` on the run thread from the frozen state. Everything that
  only reads the pre-batch state can move there. The apply should be
  writes only.
- Profiling: build with
  `CARGO_TARGET_DIR=target/fp RUSTFLAGS="-C force-frame-pointers=yes" scripts/nc.sh build --profile profiling --features profile --example typing`,
  run `scripts/run.sh target/fp/profiling/examples/typing 1000000 --threads 16 --edits 0 --no-check`
  and read the span table (the block after `probes (cold evaluation)`
  up to the `load` line). Spans are declared in `src/obs.rs`; add spans
  for the phases you create. `perf record -F 499 --call-graph fp` works
  on that binary through `systemd-run --user --slice=defuse.slice --scope`.
- Release timing: `scripts/nc.sh build --release --example typing --example fuzz`,
  then `scripts/run.sh ./target/release/examples/typing 1000000 --threads 16 --edits 0 --no-check`
  (prints load, cold, runs, critical path, `widths`, rss); 100k the same
  with 100000; edits with `--edits 50`.
- Tests: `scripts/nc.sh test -q --release --test engine --test client`.
  Fuzz: `SCHED={seq,spec,random} scripts/run.sh ./target/release/examples/fuzz examples/fuzz_pool.txt 120 1`
  must print `all ok` for each scheduler. The engine tests include
  generated-program gates against the interpreter in `examples/common`
  (the reference semantics) with 4 threads and the random scheduler.
- `Engine::check_invariants()` returns a list of broken invariants; the
  tests call it. Extend it with every new invariant of the partition
  (slot ranges, treap subtrees, index key ranges).

## One design that would work (yours to take, change or replace)

A label-range partition of a wide step. All index keys carry the label,
so partitions own disjoint key ranges; the treap splits by label into
one subtree per partition; node records and ids are staged per
partition and merged in one sequential pass.

1. **Measure first.** Reproduce the span table at the base commit and
   keep it in this file. Then decide the partition threshold from
   `Report::applied_widths`: steps below it stay on the sequential path.
2. **Move reads to the run threads.** Whatever `apply` reads from the
   pre-batch state (walk sites for `add_def`'s re-bind search, local
   ends, old uses, statics) is computed in `Ctx::finish` and carried in
   `RunResult`. This is pure speedup, safe, and needed anyway: the
   parallel phase must not read shared mutable state.
3. **Reserve, then write in parallel.** Before the parallel phase, one
   sequential pass reserves contiguous slot ranges in `nodes` (and the
   treap arena) per partition and assigns each result to a partition by
   label. Each partition then builds its node records, its treap subtree
   (`insert_run` style, a Cartesian build), its sorted index entries and
   its queue pushes on its own thread, writing only into its own ranges.
   Reads of the frozen pre-batch state are fine; reads of another
   partition's new state are not.
4. **Merge sequentially.** Join the subtrees into the order, bulk-insert
   the sorted index entries, extend `ids`, concatenate the queues. Sorted
   bulk inserts into the B-trees are the part to measure: if they are
   still the floor, replace the five B-trees with per-unit sorted
   vectors (the keys are labels; a wide step inserts a contiguous run).
5. **Fix the boundaries.** A reader in partition B whose reaching
   definition is a new definition in partition A: after the merge, run
   the ordinary `add_def` user move once per boundary, sequentially.
   Inside a partition the sequential order gives read-your-writes as
   today. Write a test that constructs exactly this case (a definition
   near the end of one partition, a reader at the start of the next, a
   third sibling further on) with a forced small partition size.
6. **Claims and finish.** `flush_claims` and `finish` may stay
   sequential at first if they are not the floor after 1 through 5;
   measure and say so. Waking waiters and the settle heap are cheap per
   node; the claim recompute is the one to look at second.

Keep the sequential path as the reference: a debug-only check that runs
a wide step both ways on a cloned engine and compares the state would be
the strongest test, if cloning is feasible; otherwise rely on
`check_invariants`, the gates and the fuzz with the partition threshold
forced to 1 so every step partitions.

## What to report back

The list of commits with one line each, the span table before and after
(same session, back to back), the 1M and 100k release lines for 1 and 16
threads before and after, the fuzz and checks results, and what is left:
which phase is now the floor, and what you would do next. If a goal is
not met, say which and by how much, with the measurement.

## Log (decisions and measurements)

Machine: 12 physical cores, 2 threads each. Local comparisons are pinned
with `taskset -c 0-11`, base and head back to back; 12 threads stands in
for 16 on distinct cores. Reported numbers come from the accl job (see the
end of this file).

### Baseline at 3c631fb (local, 2026-10-13)

Profiling build, 1M, 16 threads, cold: evaluate 19.27 s, run_batch 6.95 s,
apply_batch 10.33 s; inside apply: children 2.79 (create 0.49, order
insert 0.33, bind 2.05), defs 1.48, finish 1.06, uses 0.55; flush_claims
0.74 (outside apply_batch).

Release, `--edits 0`: 1M 1 thread cold 35.5 s, 16 threads 18.2 s; 100k
1 thread 3.04 s, 16 threads 1.57 s.

**Apply by step width** (release, 1M, 16 threads, timed per step): steps
of 4096+ results 5.7 s (94 steps, 884k results, about 6.4 us each), 256 to
4095 results 3.4 s, below 256 0.9 s. This corrects the brief: the wide
steps alone are not enough for 4 s, so a parallel path must start near 256
results and the per-result cost on medium steps matters as much.

**What a wide step does** (counters over the 4096+ steps): 884k results,
547k finish, 82k wait, 239k gated; 203k children created by 55k results;
193k definitions added, 85k of them through the full re-bind search,
**0 users moved**, **0 region re-binds**, **0 unbalanced spans**,
**0 claim recomputes**, **0 replaced children**; 355k definition values
set, 18k nodes dirtied, 140k waiters woken, 311k parks, 68k walks.

**Where the time is** (perf, frame pointers, release code): the order
treap's pointer chasing (about 28 levels at 1.3M items: `fold_in`,
`back`, `set_many`, `pull`) is about a third of apply and most of the
walks inside runs (walks are 45% of all evaluation CPU).

**Order of results.** `apply` takes results in `NodeId` order, a hash
unrelated to text order, so the engine is already correct under any
order of a batch's results: any serializable parallel apply is valid.

### Step 1: the order and accumulator trees become B+trees

`src/tree.rs`: same API, augmented B+tree (leaves of 32 keys, inner nodes
of 16 children with each child's summary). Every summary is an
associative monoid and every search predicate monotone, so the shape
never shows in a result. Tested against a sorted-map model (all
operations, monotone searches, skipping visits, cached lazy folds).

Release, 1M, pinned to 12 cores, back to back with base: 12 threads cold
18.25 -> 16.80 s; 1 thread 36.9 -> 33.0 s. Runs, stops and widths
identical. Fuzz seq/spec/random all ok; CHECKS_OK.

### Step 2: a site-only walk for the re-bind search

Apply time per result class over steps of 256+ results (release, after
step 1, 12 threads): gated 464k results 0.47 s (1.0 us each); waits 102k
0.48 s; finishing without children 892k 2.84 s (3.2 us); finishing with
fresh children 77k 2.29 s (30 us, about 8 us per child).

Every walk inside apply comes from `add_def`'s re-bind search, which
needs only the reaching site, but `walk` also scanned for pending
structure (`walk_effects`, prefixes, claim swallow): 662 of its 1242
samples. `walk_site` computes the site alone; `walk` is now `walk_site`
plus `walk_pending`.

Profiling build, 1M, 12 threads pinned, two rounds back to back with
step 1: apply_batch 8.93 / 8.72 -> 7.85 / 7.79 s; bind 1.80 -> 1.17 s;
defs 1.34 -> 1.04 s. Fuzz all ok, run counts identical, CHECKS_OK.

### Tried and dropped (measured, not kept)

- **Raising not-yet-run hint readers with their hinted input** (to avoid
  gated runs): gated 468k -> 194k but cold 15.0 -> 22.0 s, critical path
  359 -> 400, many more narrow steps. Heights decide batching too.
- **Software prefetch** of the next results' records in the apply loop:
  no change within noise.
- **Parallel warm-up** (all threads read what a chunk of 2048 results
  will touch, into the shared L3, before the chunk is applied): warm-up
  0.34 s, apply_batch unchanged (8.11 -> 8.23 s).

### Step 3: a batch is applied in text order

`results` sorted by (unit, label) instead of node id: consecutive
results touch neighbouring index entries and order leaves. Profiling,
1M, 12 threads pinned, back to back: apply_batch 8.10 -> 7.52 s (bind
1.20 -> 1.07, uses 0.54 -> 0.48). Release: 1M cold 15.36 -> 14.33 s,
100k 1.32 -> 1.12 s. Run counts shift slightly (1085551 -> 1085530 runs,
gated 467927 -> 458630): a schedule change; live nodes identical, fuzz
all ok.

One client test asserted a schedule detail (that the kind reader
*parked* on the opener's run); in text order the opener is applied first
in the same batch and the reader is lifted above it instead. The
assertion is dropped (its outputs and the "no wait on a scope effect"
check stay). `scripts/checks.sh` now fails when a test fails (it used to
grep the test output and print CHECKS_OK anyway).

### Step 4: a definition after every use of its field moves nothing

`user_max[(unit, field)]`, a bound on the labels of every use ever
indexed (it only grows, so it stays a bound under removal and
renumbering): when it is at or below a new definition's label, the
re-bind search (walk to the previous site, scope end, users range) is
skipped, as its range would be empty. Profiling, 1M, 12 threads pinned,
three alternating rounds (machine loaded, load average about 8):
defs 1.09 / 1.16 / 1.07 -> 0.81 / 0.70 / 0.74 s, bind 1.09 / 1.16 / 1.07
-> 1.00 / 0.92 / 0.95 s, uses +0.07 s (the bound's update). Fuzz all ok.

### Step 5: the definition and use indexes are split by key

`defs_ix`, `local_ix`, `global_ix`, `users` and `chain_users` were global
`BTreeMap`s keyed by tuples ending in the label. They are now
`ix::LabelIx<K>`: a hash map from the key prefix ((unit, field), (unit,
field, depth), (site, field)) to that key's labels, a sorted vector up to
24 entries and a `BTreeMap<u64, Slot>` past that. A query hashes once and
searches only its key's labels. Same ranges, same order; tested against
the composite B-tree.

Profiling, 1M, 12 threads pinned, min of 3 alternating rounds (from here
on small deltas are compared this way, `target/tools/cmp.sh`): evaluate
14.98 -> 13.89 s, run_batch 6.41 -> 5.87 s, apply_batch 6.89 -> 6.39 s
(defs 0.65 -> 0.47, uses 0.55 -> 0.46, bind 0.87 -> 0.72). Fuzz all ok,
CHECKS_OK.

### Step 6: bracket queries read a narrow projection of the order summary

Walk time (perf, all threads) was 48% order tree, 25% std B-trees, 15%
values. Most order queries in walks need only the bracket counts (depth
at a label, the nearest opener or close), but each tree level read 16
full 56-byte summaries. `Aug` now has a projection `P` (for the order,
`Br { c, o }`, 8 bytes) kept per child and per item; `pfold`,
`psearch_back`, `psearch_fwd` read only those. Ten folds and ten searches
switched. Min of 3 alternating rounds: evaluate 14.22 -> 13.56 s,
run_batch 6.04 -> 5.68 s, apply_batch 6.51 -> 6.28 s.

### Merge of master (bbc5e4e)

master moved (c82b848 binding epoch, e8d567f may_define refutation,
a751a3b environment provider, a2bd556 invariants); merged without
conflicts; fuzz all ok, CHECKS_OK on the merged tree. Measurements from
here on compare against the merged head.

### Step 7: node origins and the parked set leave the hash map and the B-tree

perf on the merged head: hash probes 12.5% of apply, 7% of it `origins`
(touched for nearly every node), and the `parked` `BTreeSet<(NodeId,
Slot)>` (an insert and a remove per park) about 12%. `origins` is now
`ix::SlotMap`, dense by slot index with the slot's generation (a freed
slot finds nothing, as before). `parked` is a vector in parking order
with lazy deletion (an entry stands while its node holds `F_PARKSET`;
compacted when stale entries dominate); the waiter list sorts as before,
speculation candidates are scanned in parking order instead of id order
(a schedule choice). Min of 3 alternating rounds against the merged
head: apply_batch 5.98 -> 5.60 s, finish 0.85 -> 0.59 s; evaluate 12.97
-> 12.63 s. Fuzz all ok, CHECKS_OK.

### Step 8: waiting range readers are settled once per batch

Outside apply proper, `settle_deferred` was 18% of the main thread's
non-run time: 1.08M `stab_settle` calls, each finding about one watch,
and for each waiting reader a full-summary fold of its range, repeated
for every finished node of the batch inside that range. Now the batch's
waiting readers are collected, deduplicated and checked once each; the
in-work count joins the narrow projection (`Br { c, o, work }`) and the
work-only folds use it. Min of 3: apply_batch 5.79 -> 5.49 s, evaluate
13.06 -> 12.62 s. Fuzz all ok, CHECKS_OK.
- **`maxh` in the narrow projection** (so the range readers' fold reads
  16 bytes per child): two sets of 3 rounds disagree (+0.22 s, -0.14 s
  apply); not kept.
- **Skipping unchanged order items in `set_many`, merge sort of the
  dirty list**: apply_batch 5.88 -> 6.02 s (min of 3); not kept (most
  dirty items do change).

### Step 9: a created node with no scope and no marker skips the kind sync

Per class over steps of 256+ results (release, after step 8): gated 455k
0.33 s, waits 102k 0.16 s, finishing without children 897k 2.10 s
(2.3 us each), unfolding 78k 1.39 s (17.8 us each, about 5 us per child).
In the unfolding path `sync_kind` was 18%: for every created child it
built a full order item (a hash probe of the claim-dirty set, aux and
stream lookups) and a marker list, to find that a node opening no scope
and holding no marker has nothing to do. It now returns first in that
case (`eff_now` gives the effect alone). Min of 3: apply_batch 6.10 ->
5.76 s, evaluate 13.95 -> 13.11 s. Fuzz all ok, CHECKS_OK.

### Step 10: effect reads through `eff_now`, per-run counts by slot

Every `oitem(x).eff` (a full order item built to read one field) reads
`eff_now(x)`; `runs_in_eval` is a `SlotCounts` (dense by slot, cleared by
an epoch) instead of a hash map; the per-run `spec_done.remove` is
skipped when the set is empty. Min of 3: apply_batch 5.75 -> 5.59 s,
evaluate 13.14 -> 12.83 s. Fuzz all ok, CHECKS_OK.

### Step 11: a wide batch is applied in slices with a tail (merge design, step 1)

Following docs/apply-merge.md (branch apply-merge) section 8 step 1: in
a batch of 256+ results with nothing speculative (spec_live 0), results
are cut into slices of `clamp(n/32, 64, 256)` consecutive results of one
unit (never dependent on the thread count). While a slice is applied,
the writes to a node outside its unit and label range (dirty, raise,
park-on, wake, output-reader add/drop) go to a tail list, replayed after
the last slice; results that read ranges, the environment, change effect
or claims, carry assumptions or errors, or need an application are
applied whole after the tail. Assumer pushes need no deferral: they only
come from speculative results, and those steps are not sliced. Reads,
indexes and the tree are unchanged. `DEFUSE_SLICE` / `DEFUSE_SLICE_MIN`
force S and the threshold (tests). New profile counter and span `tail`
(public `obs::Counter`, `obs::Span` gain a variant each).

Measured (1M, 12 threads pinned, profiling build): tail 208k items at
S=256 (225k adaptive, 237k at S=64), tail span 26 ms over 540 sliced
batches; runs 1,085,570 against 1,085,530 (0.004%); min of 3: apply
5.62 -> 5.82 s (the per-result whole predicate and ownership checks; to
be won back by the parallel slices). Fuzz all ok under seq/spec/random
with S in {1, 2, 64} and T=1, and at the defaults; the debug test suite
passes with S=1, T=1; CHECKS_OK. The design's riskiest assumption holds:
the tail is small and cheap.

### Rebase onto master 4c9f7b0, and waits keep the id-order outcome

Rebased (the earlier merge folded in). Master's new tests
`writers_of_one_field_...` and `a_woken_link_names_...` failed from step
3 on: where a reader x and the node t it waited on both ran in a batch,
id order parked x and woke it (finishing above t, the floor) when x came
first, and lifted it without a floor when t came first; text order
mostly applies t first. `stop_on` now gives the id-order outcome whatever
the apply order (x before t by id: parked and woken, or parked then
woken at once if t is already done; t first: lifted, also when x parked
and t finishes later in the batch, through `batch_lift`). A first
version that always woke raised the 1M critical path from 359 to 467
and apply by 0.63 s; this one keeps 359 (runs 1,085,569) at apply 6.25 ->
6.32 s (noise). Note the master merge itself moved 1M (profiling, 12
threads): evaluate 13.6 -> 11.7 s, run_batch 5.8 -> 3.8 s, apply 6.15 ->
6.25 s, bind 0.62 -> 1.26 s.

### Step 12: slices apply through overlays, in parallel

Each slice of a wide batch is applied by an overlay engine (`src/engine/slice.rs`):
an `Engine` whose structures hold only what the slice wrote and read the rest
from the engine as it was before the batch. The containers learned to be
overlays of a frozen base (`ix::LabelIx`, `SlotMap`, `SlotCounts`, `ov::OvMap`,
`ov::OvSet`, `tree::Arena`, the last copying a node on its first write); node
records and auxes are copied on write (the base's `stage` table names the slice
holding a copy). New nodes take slots reserved before the slices run. The
slices run through `Executor::for_each`; the overlays are then merged in slice
order (order and accumulator logs compressed to the final items first), the
tails drained, and the whole results applied. A slice that meets what an
overlay does not support (freeing a node, a new unit or application,
renumbering, a scope boundary change, the environment, a unit change) aborts
and its results apply whole (counter `slice_abort`; none on the 1M build).
The pending sets became `LabelIx`es (one key per unit, field or prefix).
`par` holds the one `unsafe`: the base pointer, valid while the engine is
borrowed shared for the slices' run (lib.rs: `deny(unsafe_code)`, allowed in
`par`). New spans `slices`, `merge`; the examples' `Threads::for_each` hands out
one index at a time; fuzz gained `SCHED=par` (greedy, 4 threads).

Correctness: fuzz all ok for seq/spec/random/par with S in {1, 2, 64}, T=1 and
the defaults; the debug test suite passes with S=1 and S=3, T=1; 1M runs
1,085,569, critical path 359 (as without overlays). First 1M numbers
(profiling, 12 threads, machine heavily loaded by another build, so only
indicative): one thread through the slices: slices 9.1 s, merge 2.2 s
(order replay ~30%, index merges ~20%), apply 13.6 s against 6.3 s in place.

## Step 13: parallel order replay (split, pieces, join)

The merge's order task replays each unit's tree as disjoint pieces: the
tree is split at each slice's label range, each slice's compressed ops go
into its own piece concurrently (`tree::Pieces` over `par::Disjoint`, with
reserved node pools), and the pieces are joined. A unit whose slices'
ops overlap falls back to the sequential replay. `DEFUSE_PAR_ORDER` sets
the minimum op count (default 4096).

Fuzz (S in {1, 2, 64}, T=1, `DEFUSE_PAR_ORDER=1`, seq/spec/random/par): all ok.

Measured (1M, 12 threads, pinned 0-11, profiling build, load average 8),
o8 (this step) against o7: merge 1.86 s against 1.97 s, apply_batch 6.35 s
against 6.19 s (noise), slices 1.78 s against 1.77 s. The order replay was
not the merge's critical path once the merge structures ran as parallel
tasks; what remains is the scatter (about 0.6 s) and the sequential
pre-merge work. Next: measure the merge's tasks separately, at 1 thread
switch to the in-place path, and cut the overlay overhead.
