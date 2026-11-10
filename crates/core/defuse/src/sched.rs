//! Workers: one deque each, stealing, no centre.
//!
//! A node marked dirty by a commit goes on the committing worker's own
//! deque, so the reader of a value runs on the thread that just made it,
//! at once. The nodes dirty before the evaluation (the client's pushes and
//! edits) are dealt out as contiguous blocks, one per worker, run in order
//! from a second deque: nodes pushed in a row often read what the one
//! before defined. An idle worker steals single nodes from the others'
//! deques, then an even run of nodes from the far end of a block with plenty
//! left (a block's tail is its owner's, so no steal cuts a definer from
//! the reader right after it), then parks. The evaluation ends when
//! `outstanding` (queued plus running nodes) is zero: no step, no round,
//! no barrier. Nothing waits for a node.
//!
//! One kind of wake is held (`hold_wake`, docs/spec-settle.md): a reader
//! re-asked by a changed definition of a node whose operation settles at
//! quiet points (`Operation::settles_at_quiet`), when the reader's own
//! operation does not. It is listed on the engine, uncounted, and at the
//! next quiet point each listed reader's reads of that field are checked
//! again: those that moved are re-asked then, the rest not at all. The
//! page layer converges among itself inside a pass; the text sees one
//! state of it per pass.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering, fence};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use crossbeam_deque::{Injector, Steal, Stealer, Worker as Deque};

use crate::api::{Cause, EditSource, EvalError, Limits, NodeId, OpId, Settled};
use crate::commit::{CommitStats, commit};
use crate::ctx::Ctx;
use crate::engine::Engine;
use crate::lock::{Mx, ns, take_wait};
use crate::node::{Node, State, Wake};
use crate::values::{Field, FxHashMap};

/// What every worker and the pool share.
pub(crate) struct Shared {
    injector: Injector<NodeId>,
    /// The quiet points at which a published loop-back value moved and
    /// its readers ran again (`commit::publish_finals`): the loop-back's
    /// passes beyond the first, `Report::final_reruns`.
    passes: AtomicU64,
    /// Nodes run so far in the evaluation, and at each pass's start
    /// (`Report::pass_runs`).
    runs_now: AtomicU64,
    /// Each worker's runnable nodes by place ([`Ready`]), indexed by the
    /// worker's `ix`: its own taken first, a thief takes another's.
    ready: Vec<Ready>,
    block_stealers: Vec<Stealer<(NodeId, u32)>>,
    /// One thief at a time per block: a steal takes an even run of nodes
    /// from its far end, whole.
    block_busy: Vec<AtomicBool>,
    /// Queue entries plus running nodes; zero ends the evaluation.
    outstanding: AtomicUsize,
    idle: AtomicUsize,
    /// Counts the wakes: a worker that looked for work outside the park's
    /// lock rests only if none came since it looked.
    wakes: AtomicU64,
    park: Mutex<()>,
    /// Each worker's place to rest ([`Rest`]), indexed by its `ix`.
    rest: Vec<Rest>,
    stop: AtomicBool,
    /// Workers running so far; the rest start one at a time, each when work
    /// has waited ([`Worker::offer`]) and no worker is idle, so a small
    /// evaluation starts no thread.
    spawned: AtomicUsize,
    /// Starts one more worker while the evaluation's scope stands
    /// ([`Pool::run`]).
    spawner: Mx<Option<Spawner>>,
    pub(crate) n: usize,
    /// `Limits::final_reruns`.
    final_limit: u32,
    /// When the evaluation began (`Ctx::elapsed`, a run's own check of
    /// `Limits::time`).
    pub(crate) t0: Instant,
    /// A collection is wanted (`Limits::collect_at_bytes`): every worker
    /// ending a run, or idle, comes to the barrier (`Pool::barrier`); the
    /// last to come collects, with no run in flight. `active` counts the
    /// workers in the evaluation (counted when spawned, so one starting
    /// late comes too), `arrived` those at the barrier, `collect_gen` the
    /// collections made; the payload of the next is `collect_next`.
    collect_wanted: AtomicBool,
    /// Dirty nodes of `Operation::final_only` operations, uncounted, run
    /// when the line is next quiet.
    deferred: Mx<Vec<NodeId>>,
    /// Runs stopped waiting for a node's commit (`Ctx::read` on a
    /// settling node's prediction at depth 0), by that node: queued when
    /// it commits, or at the quiet point.
    pub(crate) waiting: Mx<crate::values::FxHashMap<NodeId, Vec<NodeId>>>,
    /// Runs stopped at a read past the settling layer's frontier
    /// (`Ctx::set_frontier`): queued as it passes them, or at the quiet
    /// point.
    pub(crate) frontier_waiting: Mx<Vec<NodeId>>,
    /// Each worker's node at work on (its raw id plus one, 0: none), for
    /// [`Pool::release_past_only`].
    pub(crate) at_work: Box<[AtomicU64]>,
    /// The roots the caller's edit source holds (`EditSource::roots`),
    /// given by the caller's worker as it comes to the barrier.
    extra_roots: Mx<Vec<crate::values::ValueId>>,
    /// What the runs that panicked on inputs that held saw, judged at the
    /// evaluation's end (`Engine::evaluate`): roots of every collection
    /// before it, so a freed answer does not pass a genuine failure for a
    /// moved input.
    panicked_roots: Mx<Vec<crate::values::ValueId>>,
    collecting: AtomicBool,
    active: AtomicUsize,
    arrived: AtomicUsize,
    collect_gen: AtomicU64,
    collect_next: AtomicU64,
}

/// A worker's place to rest: its own condition, so that a wake chooses
/// the worker.
#[derive(Default)]
struct Rest {
    cv: Condvar,
    parked: AtomicBool,
}

/// [`Shared::spawner`]: the closure of [`Pool::run`], which clears this
/// under its lock before the closure ends ([`ClearSpawner`]).
struct Spawner(*const (dyn Fn() + Sync));

// SAFETY: the closure is `Sync`, and is called only under `Shared::spawner`'s
// lock while `Pool::run` keeps it alive.
#[allow(unsafe_code)]
unsafe impl Send for Spawner {}

/// Clears [`Shared::spawner`] as the closure it points to is about to go.
struct ClearSpawner<'a>(&'a Shared);

impl Drop for ClearSpawner<'_> {
    fn drop(&mut self) {
        *self.0.spawner.lock() = None;
    }
}

impl Shared {
    /// One more worker at work: the lowest-numbered at rest, the one whose
    /// thread ran most lately and whose caches are the warmest (a condition
    /// shared by all woke the one that had waited longest, so an edit's 200
    /// runs went through 27 of 32 threads, each cold), else one not started.
    fn wake_one(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        if self.idle.load(Ordering::SeqCst) > 0 {
            let _g = self.park.lock().unwrap_or_else(PoisonError::into_inner);
            for r in &self.rest {
                if r.parked.swap(false, Ordering::SeqCst) {
                    r.cv.notify_one();
                    break;
                }
            }
            return;
        }
        if self.spawned.load(Ordering::Relaxed) < self.n
            && let Some(g) = self.spawner.try_lock()
            && let Some(sp) = &*g
        {
            // SAFETY: `Spawner`: set, so the closure is alive, and we hold
            // the lock that clears it.
            #[allow(unsafe_code)]
            unsafe {
                (*sp.0)();
            }
        }
    }

    fn wake_all(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        let _g = self.park.lock().unwrap_or_else(PoisonError::into_inner);
        for r in &self.rest {
            r.parked.store(false, Ordering::SeqCst);
            r.cv.notify_one();
        }
    }

    /// One queue entry or running node less. The last one wakes the worker
    /// that decides what the quiet point does, the caller's; waking them
    /// all had each thread of the pool off its core at every quiet point
    /// for nothing.
    fn done_one(&self) {
        if self.outstanding.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.wakes.fetch_add(1, Ordering::SeqCst);
            let _g = self.park.lock().unwrap_or_else(PoisonError::into_inner);
            self.rest[0].parked.store(false, Ordering::SeqCst);
            self.rest[0].cv.notify_one();
        }
    }
}

pub(crate) struct Pool {
    pub(crate) n: usize,
    shared: Arc<Shared>,
    failed: Mx<Option<EvalError>>,
    total_runs: AtomicU64,
    pub(crate) epoch: u64,
    pub(crate) limits: Limits,
    pub(crate) t0: Instant,
}

/// What a worker logs beyond its counters.
#[derive(Default)]
pub(crate) struct WorkerLog {
    pub(crate) runs_by_op: FxHashMap<OpId, u64>,
    /// The nodes whose run stood (`Ctx::last_run_stands`): counted as
    /// runs, nothing of their operation ran.
    pub(crate) stood: Vec<NodeId>,
    /// Nodes the commits' placements made, by operation.
    pub(crate) placed_by_op: FxHashMap<OpId, u64>,
    /// Placements relocated instead of remade (`commit::relocate`), and
    /// by the relocated node's operation.
    pub(crate) placements_moved: u64,
    pub(crate) moved_by_op: FxHashMap<OpId, u64>,
    /// Runs whose commit found every output of the run before them
    /// (`commit`: no definition, contribution, output, bracket or
    /// placement changed), by operation ([`crate::Report::runs_wasted`]).
    pub(crate) wasted_by_op: FxHashMap<OpId, u64>,
    /// The commits' whole time (under the lock and the re-asks after)
    /// and the reads their records file, by operation.
    pub(crate) commit_by_op: FxHashMap<OpId, (u64, u64)>,
    pub(crate) causes: Vec<(NodeId, Cause)>,
    pub(crate) refuted_by_field: FxHashMap<Field, u64>,
    pub(crate) run_times: Vec<(NodeId, Duration)>,
    /// Loop-back readers past `Limits::final_reruns` (`allow_final_rerun`).
    pub(crate) capped: Vec<NodeId>,
    /// Runs that panicked on inputs that held when they did: the node,
    /// what it read, the panic; failures only if those reads still hold
    /// once the line is quiet (`Engine::evaluate`).
    pub(crate) panicked: Vec<(NodeId, Vec<crate::node::ReadRec>, &'static str, String)>,
    /// The runs made before each pass after the first began (the caller's
    /// worker only).
    pub(crate) pass_starts: Vec<u64>,
    /// The edits taken during the evaluation: their version and the runs
    /// made before (the caller's worker only).
    pub(crate) absorbed: Vec<(u64, u64)>,
    /// The reader records of earlier runs dropped at the passes' quiet
    /// points (`Engine::sweep_readers`; the caller's worker only).
    pub(crate) swept: u64,
    /// The collections this worker made, with the time each held the
    /// workers (ns).
    pub(crate) collections: Vec<(crate::Collected, u64)>,
}

/// One worker's deques and counters (nanoseconds).
pub(crate) struct Worker {
    pub(crate) ix: usize,
    /// Its block of the nodes dirty before the evaluation, in order, each
    /// with the run it was put in by (a seed, a steal).
    block: Deque<(NodeId, u32)>,
    /// The runs it has put in its block.
    block_runs: AtomicU32,
    /// When it may next wake a worker for its queue ([`Worker::offer`]),
    /// ns on the pool's clock.
    next_offer: AtomicU64,
    pub(crate) shared: Arc<Shared>,
    pub(crate) runs: AtomicU64,
    pub(crate) nodes_run: AtomicU64,
    pub(crate) refuted_nodes: AtomicU64,
    pub(crate) resumed: AtomicU64,
    pub(crate) parks: AtomicU64,
    /// Runs that waited for a node before them before doing anything
    /// (`Ctx::wait_for_definer`): not runs ([`Report::waits`]).
    pub(crate) waits: AtomicU64,
    /// Runs that panicked on inputs that moved during the run.
    pub(crate) stale_panics: AtomicU64,
    /// Runs stopped at a check because a read moved while they ran
    /// (`Ctx::tick`).
    pub(crate) stopped_stale: AtomicU64,
    pub(crate) op_ns: AtomicU64,
    pub(crate) wasted_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) commit_max_ns: AtomicU64,
    pub(crate) commit_max_cpu_ns: AtomicU64,
    pub(crate) commit_allocs: AtomicU64,
    pub(crate) commit_frees: AtomicU64,
    /// The node of the longest commit (raw id).
    pub(crate) commit_max_node: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) idle_ns: AtomicU64,
    pub(crate) lock_wait_ns: AtomicU64,
    pub(crate) lock_wait_max_ns: AtomicU64,
    pub(crate) steals: AtomicU64,
    /// Wakes it held to a quiet point, those it fired there, and the quiet
    /// points at which it fired any (`hold_wake`).
    pub(crate) held_deferred: AtomicU64,
    pub(crate) held_fired: AtomicU64,
    pub(crate) held_passes: AtomicU64,
    /// Held wakes dropped past `Limits::settle_passes` (1: yes).
    pub(crate) held_unsettled: AtomicU64,
    pub(crate) woken: AtomicU64,
    pub(crate) walked: AtomicU64,
    pub(crate) scope_asked: AtomicU64,
    pub(crate) relative_resolved: AtomicU64,
    pub(crate) predictions_confirmed: AtomicU64,
    pub(crate) shaped: AtomicU64,
    pub(crate) commits: AtomicU64,
    /// The most `At::Final` reruns of one node it allowed.
    pub(crate) final_max: AtomicU64,
    /// Its last commit and when (`Engine::critical_chain`).
    pub(crate) last_commit: AtomicU64,
    pub(crate) last_done: AtomicU64,
    pub(crate) log: Mx<WorkerLog>,
}

impl Worker {
    /// Its runnable nodes ([`Ready`]).
    pub(crate) fn ready(&self) -> &Ready {
        &self.shared.ready[self.ix]
    }

    pub(crate) fn add(c: &AtomicU64, x: u64) {
        c.fetch_add(x, Ordering::Relaxed);
    }

    fn max(c: &AtomicU64, x: u64) {
        c.fetch_max(x, Ordering::Relaxed);
    }

    /// Adds a walk's counts.
    pub(crate) fn note(&self, st: &CommitStats) {
        Worker::add(&self.woken, st.woken);
        Worker::add(&self.walked, st.walked);
        Worker::add(&self.scope_asked, st.scope_asked);
        Worker::add(&self.relative_resolved, st.relative_resolved);
        Worker::add(&self.predictions_confirmed, st.predictions_confirmed);
        Worker::add(&self.shaped, st.shaped);
    }

    /// Queues `nodes` on its ready heap ([`Ready`]): each taken by place,
    /// the earliest first, by it or by any idle worker.
    pub(crate) fn ready_all(&self, eng: &Engine, nodes: &[NodeId]) {
        // (counted before pushed, as `seed`)
        self.shared.outstanding.fetch_add(nodes.len(), Ordering::SeqCst);
        for &n in nodes {
            self.ready().push(eng, eng.node(n));
        }
    }

    /// Wakes another worker if its queue has stood non-empty for
    /// [`offer_ns`] (or its block can be stolen from): looked at as it
    /// queues a node, takes one, and at a run's checks (`Ctx::tick`), so
    /// nothing waits much past that behind a long run.
    ///
    /// A queue that empties at each take is a chain, a page's finisher to
    /// the page's fold to the next finisher, each step reading what the
    /// last just wrote: on the same thread and core that is 1 to 1.5 times
    /// faster than handed over, and no one is woken for it (Grail, delete a
    /// paragraph: 1 run at a time for 90% of the edit, 1050 ms at -j1, 1400
    /// ms at -j8, 1650 ms at -j32; node 133's one run 34, 60, 61 ms, and 35
    /// ms at -j8 pinned to one cpu). Waking a thread and moving a node costs
    /// more than a short run saves, so short runs stay where they are.
    /// [`Worker::offer`] as it takes `node`: what it leaves queued goes to
    /// another worker at once if `node`'s run will be long, as its last
    /// was, or it has none (a first evaluation's nodes, a new or edited
    /// one), whether or not the run checks in.
    fn offer_for(&self, node: &Node) {
        let sh = &self.shared;
        if sh.n == 1 {
            return;
        }
        // (an edited node's last run says nothing of this one)
        let unknown = !node.committed()
            || node
                .woke
                .lock()
                .as_ref()
                .is_some_and(|w| matches!(w.cause, Cause::Edited | Cause::Created { .. }));
        if self.ready().len() > 0 && (unknown || node.run_ns.load(Ordering::Relaxed) >= offer_ns()) {
            self.ready().since.store(0, Ordering::Relaxed);
            sh.wake_one();
            return;
        }
        self.offer();
    }

    pub(crate) fn offer(&self) {
        let sh = &self.shared;
        if sh.n == 1 {
            return;
        }
        let r = self.ready();
        let stands = self.block.len() >= BLOCK_MIN;
        if !stands && r.len() == 0 {
            return;
        }
        let now = ns(sh.t0);
        if (stands || r.waited(now)) && now >= self.next_offer.load(Ordering::Relaxed) {
            self.next_offer.store(now + offer_ns(), Ordering::Relaxed);
            sh.wake_one();
        }
    }

    /// Seeds the block with nodes to run in this order; a node whose work
    /// is at the settling layer's frontier on the ready heap, ordered by the
    /// frontier ([`Queues`]): in a block it ran by its label, after the
    /// document (at one thread every edit's nodes are a block).
    pub(crate) fn seed(&self, eng: &Engine, nodes: &[NodeId]) {
        let run = self.block_runs.fetch_add(1, Ordering::Relaxed);
        // (counted before pushed: a count that reaches zero ends the
        // evaluation, so no entry may be seen before its count)
        self.shared.outstanding.fetch_add(nodes.len(), Ordering::SeqCst);
        for &n in nodes.iter().rev() {
            let node = eng.node(n);
            if eng.op(node.op()).works_at_frontier() {
                self.ready().push(eng, node);
            } else {
                self.block.push((n, run));
            }
        }
    }
}

/// A worker's runnable nodes, the earliest by place first,
/// the first queued first among equals: on one thread TeX's own order (a
/// predecessor before its reader, the page built after each piece and a
/// routine run before the text after its page), on more the earliest work
/// first, the critical path's.
/// A queued node's order: its place, text after the settling
/// layer at one place, the order it was queued in, its raw id.
type ReadyKey = (u64, bool, u64, u64);

/// A queued node whose work is at the settling layer's frontier
/// (`Operation::works_at_frontier`): its label, the order it was queued
/// in, its raw id.
type FrontierKey = (u64, u64, u64);

/// [`Ready`]'s two queues.
#[derive(Default)]
struct Queues {
    /// Those keyed by their label, for good.
    placed: std::collections::BinaryHeap<std::cmp::Reverse<ReadyKey>>,
    /// Those keyed where the frontier stands when they are taken
    /// ([`frontier_place`]), not where it stood when they were queued.
    /// Keyed once, a loop node dirty since the edit kept the key of the
    /// frontier then, the document's end, while the runs before it moved
    /// the frontier back to the page they changed: it ran only after every
    /// finisher in the queue, each of those on a page it had not fired yet
    /// and on the float list of a page it would fire differently, one such
    /// wave of the document's pages per loop node (amsldoc, a figure
    /// deleted: 582 finisher runs at -j1).
    at_frontier: std::collections::BinaryHeap<std::cmp::Reverse<FrontierKey>>,
}

impl Queues {
    /// The next node's key and whether it is at the frontier.
    fn first(&self, eng: &Engine) -> Option<(ReadyKey, bool)> {
        let placed = self.placed.peek().map(|x| x.0);
        let front = self.at_frontier.peek().map(|x| {
            let (p, s, id) = x.0;
            ((frontier_place(eng).map_or(p, |f| p.min(f)), false, s, id), true)
        });
        match (placed, front) {
            (Some(a), Some((b, f))) if b < a => Some((b, f)),
            (Some(a), _) => Some((a, false)),
            (None, b) => b,
        }
    }

    fn pop(&mut self, eng: &Engine) -> Option<NodeId> {
        let (k, front) = self.first(eng)?;
        if front {
            self.at_frontier.pop();
        } else {
            self.placed.pop();
        }
        Some(NodeId::from_raw(k.3))
    }
}

pub(crate) struct Ready {
    heap: Mx<Queues>,
    seq: AtomicU64,
    len: AtomicUsize,
    /// When it last went from empty to not, ns on the pool's clock.
    since: AtomicU64,
    t0: Instant,
}

impl Ready {
    fn new(t0: Instant) -> Ready {
        Ready {
            heap: Mx::new(Queues::default()),
            seq: AtomicU64::new(0),
            len: AtomicUsize::new(0),
            since: AtomicU64::new(0),
            t0,
        }
    }

    /// Whether it has stood non-empty for [`offer_ns`]: a thief's to take
    /// from. Before that its owner, about to take the node, runs it on the
    /// caches that hold what it reads.
    fn waited(&self, now: u64) -> bool {
        now >= self.since.load(Ordering::Relaxed).saturating_add(offer_ns())
    }

    pub(crate) fn push(&self, eng: &Engine, n: &Node) {
        let p = n.pos_hint().0;
        let s = self.seq.fetch_add(1, Ordering::Relaxed);
        let op = eng.op(n.op());
        if op.works_at_frontier() {
            self.heap.lock().at_frontier.push(std::cmp::Reverse((p, s, n.id.raw())));
        } else {
            // (at one place the settling layer first: TeX builds the page
            // after the paragraph that ends there, before the text after it)
            let text = !op.settles_at_quiet();
            self.heap
                .lock()
                .placed
                .push(std::cmp::Reverse((p, text, s, n.id.raw())));
        }
        if self.len.fetch_add(1, Ordering::Relaxed) == 0 {
            self.since.store(ns(self.t0), Ordering::Relaxed);
        }
    }

    pub(crate) fn pop(&self, eng: &Engine) -> Option<NodeId> {
        if self.len.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let x = self.heap.lock().pop(eng)?;
        self.len.fetch_sub(1, Ordering::Relaxed);
        Some(x)
    }

    /// A thief's `pop`: none where the heap is empty or its owner (or
    /// another thief) holds it, never waited for (every idle worker
    /// locking every other's heap in turn made the heaps' locks the
    /// engine's: 5.3% of worker time at 32 threads,
    /// `commits_do_not_wait_on_each_other`). None before it has waited
    /// ([`Ready::waited`]).
    fn steal(&self, eng: &Engine) -> Option<NodeId> {
        if self.len.load(Ordering::Relaxed) == 0 || !self.waited(ns(self.t0)) {
            return None;
        }
        let x = self.heap.try_lock()?.pop(eng)?;
        self.len.fetch_sub(1, Ordering::Relaxed);
        Some(x)
    }

    /// The node a `pop` takes next, if any: a peek.
    fn first(&self, eng: &Engine) -> Option<NodeId> {
        if self.len.load(Ordering::Relaxed) == 0 {
            return None;
        }
        self.heap.lock().first(eng).map(|x| NodeId::from_raw(x.0.3))
    }

    /// The earliest node's key, if any.
    fn first_key(&self, eng: &Engine) -> Option<u64> {
        if self.len.load(Ordering::Relaxed) == 0 {
            return None;
        }
        self.heap.lock().first(eng).map(|x| x.0.0)
    }

    /// A thief's `pop`, waiting for the lock of a heap that has waited.
    fn steal_waiting(&self, eng: &Engine) -> Option<NodeId> {
        if self.len.load(Ordering::Relaxed) == 0 || !self.waited(ns(self.t0)) {
            return None;
        }
        self.pop(eng)
    }

    fn len(&self) -> usize {
        self.len.load(Ordering::Relaxed)
    }
}

/// Where the settling layer's frontier stands (`Ctx::set_frontier`), for
/// the order of a node whose work is at it (`Operation::works_at_frontier`):
/// the page loop stands after the whole text and folds the piece at its
/// hole, a rest folds from the page its loop node waited at. A finisher
/// keeps its label: keyed at the frontier, a finisher of a page past it,
/// re-asked by the finisher before (a counter, a mark), ran ahead of the
/// loop still remaking the pages before it, and re-asked the finishers after
/// it for every intermediate state of each one before: lineno's numbered
/// lines, a routine fired at every line, ran the finishers of the
/// pages after the loop's 100 to 150 times each a pass (-j1, 60 000
/// finisher runs for 2 900 pages; more than 600 s where pdflatex takes
/// 4.4 s; accl 22446).
///
/// A frontier at a node an edit removed stands where that node stood, for
/// this order: the layer folds again from there (`frontier_back`).
fn frontier_place(eng: &Engine) -> Option<u64> {
    let fr = eng.frontier.load(Ordering::Acquire);
    fr.checked_sub(1)
        .and_then(|f| eng.placed_node(NodeId::from_raw(f)))
        .map(|f| f.pos_hint().0)
}

/// Starts `ws` on threads of scope `s`: the first's thread starts two
/// halves of the rest before it works; each gives its worker to `back`
/// when it is done.
fn spawn_tree<'s, 'e: 's>(
    s: &'s crate::crew::Scope<'s, 'e>,
    eng: &'e Engine,
    pool: &'e Pool,
    back: &'e Mutex<Vec<Worker>>,
    mut ws: Vec<Worker>,
) {
    if ws.is_empty() {
        return;
    }
    let w = ws.remove(0);
    pool.shared.spawned.fetch_add(1, Ordering::SeqCst);
    pool.shared.active.fetch_add(1, Ordering::SeqCst);
    s.spawn(move || {
        let right = ws.split_off(ws.len() / 2);
        spawn_tree(s, eng, pool, back, ws);
        spawn_tree(s, eng, pool, back, right);
        run_worker(eng, pool, &w, None);
        back.lock().unwrap_or_else(PoisonError::into_inner).push(w);
    });
}

/// `PHITEX_HELD_TRACE` (`profile` builds): the quiet points' re-asked
/// readers printed, with the reads that moved.
fn held_trace() -> bool {
    #[cfg(feature = "profile")]
    {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *ON.get_or_init(|| std::env::var_os("PHITEX_HELD_TRACE").is_some())
    }
    #[cfg(not(feature = "profile"))]
    {
        false
    }
}

/// How long a queue stands non-empty before another worker is woken for
/// it and may take from it ([`Worker::offer`]): about what moving a run
/// costs, a thread's wake and a run's worth of cold caches.
const fn offer_ns() -> u64 {
    200_000
}

/// A run this long or longer (its last) is kept on the worker whose
/// queue holds it rather than run after a much shorter one there
/// (`Pool::keep_longest`).
const LONG_NS: u64 = 1_000_000;

/// Spins before parking.
const SPINS: u32 = 64;
/// A block is stolen from only with this much left, half of it at a time
/// and at least `BLOCK_STEAL` nodes (an even run from its far end, run in
/// order): the tail is its owner's, no steal cuts a definer from the reader
/// right after it, and two thieves of a block seldom share a segment. A
/// reader at the head of a stolen run is re-asked once its owner's chain
/// reaches it (`commit::first_hole`), not once per definer before it.
pub(crate) const BLOCK_MIN: usize = 96;
const BLOCK_STEAL: usize = 32;
/// An idle worker with no wake looks for work again after this (the
/// caller's, which also asks for edits and starts the spare workers, after
/// [`ABSORB_EVERY`]).
const IDLE_LOOK: Duration = Duration::from_millis(20);
/// Edits are asked for this often from the caller's thread.
const ABSORB_EVERY: Duration = Duration::from_millis(2);

impl Pool {
    /// A pool of `n` workers and their deques.
    /// `collect_mark`: the store's mark as the last collection left it.
    pub(crate) fn new(n: usize, epoch: u64, limits: Limits, collect_mark: u64) -> (Pool, Vec<Worker>) {
        let n = n.max(1);
        let blocks: Vec<Deque<(NodeId, u32)>> = (0..n).map(|_| Deque::new_lifo()).collect();
        let block_stealers = blocks.iter().map(Deque::stealer).collect();
        let t0 = Instant::now();
        let shared = Arc::new(Shared {
            injector: Injector::new(),
            passes: AtomicU64::new(0),
            runs_now: AtomicU64::new(0),
            ready: (0..n).map(|_| Ready::new(t0)).collect(),
            block_stealers,
            block_busy: (0..n).map(|_| AtomicBool::new(false)).collect(),
            outstanding: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            wakes: AtomicU64::new(0),
            park: Mutex::new(()),
            rest: (0..n).map(|_| Rest::default()).collect(),
            stop: AtomicBool::new(false),
            spawned: AtomicUsize::new(1),
            spawner: Mx::new(None),
            n,
            final_limit: limits.final_reruns,
            t0,
            collect_wanted: AtomicBool::new(false),
            deferred: Mx::new(Vec::new()),
            waiting: Mx::new(crate::values::FxHashMap::default()),
            frontier_waiting: Mx::new(Vec::new()),
            at_work: (0..n.max(1)).map(|_| AtomicU64::new(0)).collect(),
            extra_roots: Mx::new(Vec::new()),
            panicked_roots: Mx::new(Vec::new()),
            collecting: AtomicBool::new(false),
            active: AtomicUsize::new(1),
            arrived: AtomicUsize::new(0),
            collect_gen: AtomicU64::new(0),
            collect_next: AtomicU64::new(
                limits
                    .collect_at_bytes
                    .map_or(u64::MAX, |floor| floor.max(collect_mark)),
            ),
        });
        let workers = blocks
            .into_iter()
            .enumerate()
            .map(|(ix, block)| Worker {
                ix,
                block,
                block_runs: AtomicU32::new(0),
                next_offer: AtomicU64::new(0),
                shared: shared.clone(),
                runs: AtomicU64::new(0),
                nodes_run: AtomicU64::new(0),
                refuted_nodes: AtomicU64::new(0),
                resumed: AtomicU64::new(0),
                parks: AtomicU64::new(0),
                waits: AtomicU64::new(0),
                stopped_stale: AtomicU64::new(0),
                stale_panics: AtomicU64::new(0),
                op_ns: AtomicU64::new(0),
                wasted_ns: AtomicU64::new(0),
                commit_ns: AtomicU64::new(0),
                commit_max_ns: AtomicU64::new(0),
                commit_max_cpu_ns: AtomicU64::new(0),
                commit_allocs: AtomicU64::new(0),
                commit_frees: AtomicU64::new(0),
                commit_max_node: AtomicU64::new(0),
                reask_ns: AtomicU64::new(0),
                idle_ns: AtomicU64::new(0),
                lock_wait_ns: AtomicU64::new(0),
                lock_wait_max_ns: AtomicU64::new(0),
                steals: AtomicU64::new(0),
                held_deferred: AtomicU64::new(0),
                held_fired: AtomicU64::new(0),
                held_passes: AtomicU64::new(0),
                held_unsettled: AtomicU64::new(0),
                woken: AtomicU64::new(0),
                walked: AtomicU64::new(0),
                scope_asked: AtomicU64::new(0),
                relative_resolved: AtomicU64::new(0),
                predictions_confirmed: AtomicU64::new(0),
                shaped: AtomicU64::new(0),
                commits: AtomicU64::new(0),
                final_max: AtomicU64::new(0),
                last_commit: AtomicU64::new(u64::MAX),
                last_done: AtomicU64::new(0),
                log: Mx::new(WorkerLog::default()),
            })
            .collect();
        let pool = Pool {
            n,
            shared,
            failed: Mx::new(None),
            total_runs: AtomicU64::new(0),
            epoch,
            limits,
            t0: Instant::now(),
        };
        (pool, workers)
    }

    /// Runs until no node is dirty or running, on `n` threads, the
    /// caller's one of them and the others the crew's (`crate::crew`:
    /// kept for the next evaluation, their clients' caches warm); the caller's thread also asks `src`
    /// for edits. The workers come back with their counts.
    pub(crate) fn run(
        &self,
        eng: &Engine,
        workers: Vec<Worker>,
        src: Option<&mut dyn EditSource>,
    ) -> Result<Vec<Worker>, EvalError> {
        // the caller's thread takes the first node before the others start:
        // a claimer at the head of a unit takes its siblings before anyone
        // runs them
        if self.n > 1
            && let Some((n, _)) = workers[0].block.pop()
            && let Err(e) = work_caught(eng, self, &workers[0], n)
        {
            self.fail(e);
        }
        let mut workers = workers;
        let mut rest: Vec<Worker> = workers.drain(1..).collect();
        let first = workers.pop().expect("a worker");
        // workers with a block start now; the rest when asked for
        let active = rest.iter().filter(|w| !w.block.is_empty()).count();
        let later: Vec<Worker> = rest.drain(active..).collect();
        let back: Mutex<Vec<Worker>> = Mutex::new(Vec::with_capacity(self.n));
        let spare = Mutex::new(later);
        crate::crew::scope(|s| {
            // (spawned as a tree, each thread starting two before it
            // works: no thread waits for the others' starts, the caller's
            // least of all)
            spawn_tree(s, eng, self, &back, std::mem::take(&mut rest));
            let spawn_one = || {
                let mut more = spare.lock().unwrap_or_else(PoisonError::into_inner);
                if more.is_empty() {
                    return;
                }
                let w = more.remove(0);
                drop(more);
                spawn_tree(s, eng, self, &back, vec![w]);
            };
            let spawn_one: &(dyn Fn() + Sync) = &spawn_one;
            // SAFETY: the lifetime only: `ClearSpawner` clears it before
            // `spawn_one` goes, under the lock each call holds.
            #[allow(unsafe_code)]
            let spawn_one: &'static (dyn Fn() + Sync) =
                unsafe { std::mem::transmute::<&(dyn Fn() + Sync), &'static (dyn Fn() + Sync)>(spawn_one) };
            *self.shared.spawner.lock() = Some(Spawner(spawn_one));
            let _clear = ClearSpawner(&self.shared);
            run_worker(eng, self, &first, src);
        });
        let mut rest = back.into_inner().unwrap_or_else(PoisonError::into_inner);
        rest.extend(spare.into_inner().unwrap_or_else(PoisonError::into_inner));
        rest.sort_by_key(|w| w.ix);
        let mut workers = vec![first];
        workers.extend(rest);
        match self.failed.lock().take() {
            Some(e) => {
                self.leave_queued(eng, &workers);
                Err(e)
            }
            None => Ok(workers),
        }
    }

    /// The nodes still queued when a failure stopped the evaluation, to the
    /// engine's pending list: the next evaluation runs them.
    fn leave_queued(&self, eng: &Engine, workers: &[Worker]) {
        let mut left = Vec::new();
        for w in workers {
            while let Some(n) = w.ready().pop(eng) {
                left.push(n);
            }
            while let Some((n, _)) = w.block.pop() {
                left.push(n);
            }
        }
        loop {
            match self.shared.injector.steal() {
                Steal::Success(n) => left.push(n),
                Steal::Empty => break,
                Steal::Retry => {}
            }
        }
        for n in left {
            if eng.node(n).state() == State::Dirty {
                eng.pend(n);
            }
        }
    }

    fn fail(&self, e: EvalError) {
        let mut f = self.failed.lock();
        if f.is_none() {
            *f = Some(e);
        }
        self.shared.stop.store(true, Ordering::SeqCst);
        self.shared.wake_all();
    }

    pub(crate) fn outstanding(&self) -> usize {
        self.shared.outstanding.load(Ordering::SeqCst)
    }

    /// The caller's thread drains the engine's pending edits onto its deque.
    /// Whether dirty node `n` waits for the quiet point
    /// (`Operation::final_only`), kept for it.
    pub(crate) fn defer(&self, eng: &Engine, n: NodeId) -> bool {
        defer(eng, &self.shared, n)
    }

    fn drain_pending(&self, eng: &Engine, w: &Worker) -> usize {
        let mut nodes = eng.take_pending(w);
        nodes.retain(|&n| !defer(eng, &self.shared, n));
        w.seed(eng, &nodes);
        nodes.len()
    }

    /// The next node for `w`, and another worker woken if what is left
    /// behind it has waited.
    fn find_work(&self, eng: &Engine, w: &Worker) -> Option<NodeId> {
        let n = self.take_work(eng, w);
        if let Some(n) = n {
            w.offer_for(eng.node(n));
        }
        n
    }

    /// `n`, just taken from `w`'s own queue, or the node queued after it
    /// where that one's last run was much longer (`LONGER` times, and at
    /// least [`LONG_NS`]) and a worker is idle: `n` goes back for that
    /// worker (or one started for it), and `w` runs the long one on the
    /// caches its last runs left (what they read is what it reads). Moved to another worker, a long
    /// run costs up to twice its time there (tikz-cd's heading edit at
    /// -j8: the `\tableofcontents` run 28-30 ms where `\begin{document}`
    /// had just run, 47-55 ms on another worker, the same 59 719 commands,
    /// reads and decodings, mc 80254), a short one little: before, the
    /// short read-back child placed between the two took the warm worker
    /// and the long run went at once to a cold one (`offer_for`). Order
    /// within one worker only: with no worker idle, `n` runs first.
    fn keep_longest(&self, eng: &Engine, w: &Worker, n: NodeId) -> NodeId {
        const LONGER: u64 = 4;
        let sh = &self.shared;
        if self.n == 1 || (sh.idle.load(Ordering::SeqCst) == 0 && sh.spawned.load(Ordering::Relaxed) >= sh.n) {
            return n;
        }
        let me = w.ready();
        let Some(y) = me.first(eng) else {
            return n;
        };
        let known = |x: &Node| {
            x.committed()
                && !x
                    .woke
                    .lock()
                    .as_ref()
                    .is_some_and(|w| matches!(w.cause, Cause::Edited | Cause::Created { .. }))
        };
        let (nx, ny) = (eng.node(n), eng.node(y));
        let (tx, ty) = (nx.run_ns.load(Ordering::Relaxed), ny.run_ns.load(Ordering::Relaxed));
        // (its last run an earlier evaluation's: in a first build a run
        // measured on unsettled input says little of the next, and there
        // the order is the one to keep, -j4 cold builds up to 11% slower
        // with it on every node, mc 80270)
        if !known(nx)
            || !known(ny)
            || ny.run_epoch.load(Ordering::Relaxed) == self.epoch
            || ty < LONG_NS
            || ty < LONGER.saturating_mul(tx.max(offer_ns()))
        {
            return n;
        }
        match me.pop(eng) {
            Some(m) if m == y => {
                me.push(eng, nx);
                y
            }
            Some(m) => {
                // (another node came first meanwhile: as taken)
                me.push(eng, eng.node(m));
                n
            }
            None => n,
        }
    }

    fn take_work(&self, eng: &Engine, w: &Worker) -> Option<NodeId> {
        // (its earliest runnable node by place ([`Ready`]), or the head of
        // its block where that stands first: a node run before one ahead
        // of it on the line reads what that one will change, and runs again
        // once it has. Taken by its deques' order, a piece placed by the one
        // before it ran ahead of the page loop the commit before re-asked,
        // and a reader re-run behind a definer still to run: at one thread
        // the page loop ran once the whole text had, each piece past a
        // page's break on its routine's state before that page and again
        // as each finisher came)
        let me = w.ready();
        match (me.first_key(eng), w.block.pop()) {
            (Some(k), Some((b, run))) => {
                if eng.node(b).pos_hint().0 < k {
                    return Some(b);
                }
                w.block.push((b, run));
                if let Some(n) = me.pop(eng) {
                    return Some(self.keep_longest(eng, w, n));
                }
            }
            (Some(_), None) => {
                if let Some(n) = me.pop(eng) {
                    return Some(self.keep_longest(eng, w, n));
                }
            }
            (None, Some((b, _))) => return Some(b),
            (None, None) => {}
        }
        loop {
            match self.shared.injector.steal() {
                Steal::Success(n) => return Some(n),
                Steal::Empty => break,
                Steal::Retry => {}
            }
        }
        let n = self.n;
        for k in 1..n {
            let v = (w.ix + k) % n;
            if let Some(x) = self.shared.ready[v].steal(eng) {
                Worker::add(&w.steals, 1);
                // (one woken at a time, each for what is left)
                if self.shared.ready[v].len() > 0 {
                    self.shared.wake_one();
                }
                return Some(x);
            }
        }
        // (one held meanwhile, not empty: waited for, no work left behind)
        for k in 1..n {
            let v = (w.ix + k) % n;
            if let Some(x) = self.shared.ready[v].steal_waiting(eng) {
                Worker::add(&w.steals, 1);
                if self.shared.ready[v].len() > 0 {
                    self.shared.wake_one();
                }
                return Some(x);
            }
        }
        for k in 1..n {
            let v = (w.ix + k) % n;
            let st = &self.shared.block_stealers[v];
            if st.len() < BLOCK_MIN {
                continue;
            }
            let busy = &self.shared.block_busy[v];
            if busy
                .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            // half of what is left: thieves of one block work far apart
            let want = ((st.len() / 2) & !1).max(BLOCK_STEAL);
            let mut taken: Vec<(NodeId, u32)> = Vec::with_capacity(want);
            // (where the owner emptied the block and put a run of its own
            // in meanwhile, the steal goes on into that run: from its far
            // end, by an even run again)
            let mut second: Option<usize> = None;
            let mut stray: Option<NodeId> = None;
            loop {
                match second {
                    Some(s) if (taken.len() - s).is_multiple_of(2) => break,
                    None if taken.len() >= want => break,
                    // (`want` from a length read before the owner went on:
                    // a thief off its CPU meanwhile came to the owner's end
                    // and took the reader whose definer the owner was
                    // running, which then ran on what was not there yet,
                    // defuse's commits_do_not_wait_on_each_other 64001 runs
                    // for 64000, 4 in 600 on a loaded node, accl 23793)
                    None if taken.len().is_multiple_of(2) && st.len() < BLOCK_STEAL => break,
                    _ => {}
                }
                match st.steal() {
                    Steal::Success(x) => {
                        if let Some(s) = second
                            && taken[s].1 != x.1
                        {
                            // (a third run's far end: it runs last)
                            stray = Some(x.0);
                            break;
                        }
                        if second.is_none() && taken.first().is_some_and(|f| f.1 != x.1) {
                            second = Some(taken.len());
                        }
                        taken.push(x);
                    }
                    Steal::Empty => break,
                    Steal::Retry => {}
                }
            }
            busy.store(false, Ordering::Release);
            if taken.is_empty() {
                continue;
            }
            Worker::add(&w.steals, 1);
            if st.len() >= BLOCK_MIN {
                self.shared.wake_one();
            }
            if let Some(x) = stray {
                let run = w.block_runs.fetch_add(1, Ordering::Relaxed);
                w.block.push((x, run));
            }
            // the far end first into the deque, so a run comes out in
            // order; the owner's second run after the first
            if let Some(s) = second {
                let run = w.block_runs.fetch_add(1, Ordering::Relaxed);
                for x in taken.split_off(s) {
                    w.block.push((x.0, run));
                }
            } else if taken.len() % 2 == 1 && taken.len() > 1 {
                // (a run the owner's end cut short of even may start at a
                // reader whose definer the owner took: that one runs last)
                let run = w.block_runs.fetch_add(1, Ordering::Relaxed);
                w.block.push((taken.pop().expect("one taken").0, run));
            }
            let run = w.block_runs.fetch_add(1, Ordering::Relaxed);
            let last = taken.pop().expect("one taken").0;
            for x in taken {
                w.block.push((x.0, run));
            }
            return Some(last);
        }
        None
    }

    /// Rests until there is work, the evaluation stops, or nothing is
    /// outstanding (then the caller's thread decides).
    /// The collection's barrier (`Limits::collect_at_bytes`): `w`, with
    /// no run in flight, waits for every other worker of the evaluation
    /// to end its run or leave its idle wait; the last to come collects
    /// (`Engine::collect`) and lets them go. A worker leaving the
    /// evaluation meanwhile is one fewer to wait for. Nothing runs while
    /// it collects: no run's stack holds a value.
    fn barrier(&self, eng: &Engine, w: &Worker) {
        let sh = &self.shared;
        let seen = sh.collect_gen.load(Ordering::SeqCst);
        sh.arrived.fetch_add(1, Ordering::SeqCst);
        let m = &sh.park;
        let cv = &sh.rest[w.ix].cv;
        let mut g = m.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if sh.collect_gen.load(Ordering::SeqCst) != seen || sh.stop.load(Ordering::SeqCst) {
                break;
            }
            if sh.arrived.load(Ordering::SeqCst) >= sh.active.load(Ordering::SeqCst)
                && !sh.collecting.swap(true, Ordering::SeqCst)
            {
                drop(g);
                let t = Instant::now();
                let mut extra = std::mem::take(&mut *sh.extra_roots.lock());
                extra.extend(sh.panicked_roots.lock().iter().copied());
                let floor = self.limits.collect_at_bytes.unwrap_or(u64::MAX);
                let collected = eng.collect_at_barrier(extra);
                let stood = collected
                    .as_ref()
                    .map_or_else(|| eng.values().payload_bytes(), |c| c.bytes_after);
                // (the next once dead values match half of what stood (a
                // wasm core, its memory fixed at 4 GiB) or all of it)
                let grown = if cfg!(target_family = "wasm") {
                    stood.saturating_add(stood / 2)
                } else {
                    stood.saturating_mul(2)
                };
                let next = floor.max(grown);
                sh.collect_next.store(next, Ordering::SeqCst);
                eng.collect_mark.store(next, Ordering::SeqCst);
                if let Some(c) = collected {
                    w.log.lock().collections.push((c, ns(t)));
                }
                sh.arrived.store(0, Ordering::SeqCst);
                sh.collect_wanted.store(false, Ordering::SeqCst);
                sh.collecting.store(false, Ordering::SeqCst);
                sh.collect_gen.fetch_add(1, Ordering::SeqCst);
                sh.wake_all();
                return;
            }
            g = cv
                .wait_timeout(g, Duration::from_millis(2))
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
        drop(g);
        // (stopped before the collection: no longer waited for)
        if sh.collect_gen.load(Ordering::SeqCst) == seen {
            sh.arrived.fetch_sub(1, Ordering::SeqCst);
        }
    }

    /// Waits for work while some is outstanding: a node to run, or none
    /// when nothing is outstanding, the pool stops or collects, or (the
    /// worker that asks the edit source, `edits`) [`ABSORB_EVERY`] has
    /// passed, for it to ask.
    /// Nothing to take, nodes held behind the frontier, and every run going
    /// stands past the frontier too: nothing before it is at work, so it
    /// moves only when those runs end. One of them may be reading what a
    /// held node would change: a run on old values in an endless loop of its
    /// own (numerica at -j8, a thread at 100% and 14 GB, no read of it
    /// moving, for no node before it ran) never ended, and the line never
    /// went quiet. The frontier moves to the earliest held node, which goes
    /// on; the others stay held, each let go in its turn as the one before
    /// it ends. (Let go all at once, every held run went on on old values
    /// whenever the runs at work were past it for a moment: at -j32 a letter
    /// typed into a beamer frame ran 285 runs, 2.5 s.) Nor past a node
    /// before the earliest held one that is to run again
    /// (`read::dirty_between`): re-asked and queued again, or at work and
    /// re-asked; it commits again, on the new values, and what it defines
    /// then the held one may read. The order of the runs only: what they
    /// make is what they read.
    fn release_past_only(&self, eng: &Engine, w: &Worker) -> bool {
        let fr = eng.frontier.load(Ordering::Acquire);
        if fr == 0 || self.shared.frontier_waiting.lock().is_empty() {
            return false;
        }
        let Some(earliest) = self
            .shared
            .frontier_waiting
            .lock()
            .iter()
            .filter_map(|&n| eng.placed_node(n))
            .filter(|n| n.state() == State::Dirty && !n.held_window.load(Ordering::Acquire))
            .min_by_key(|n| n.pos_hint())
        else {
            return false;
        };
        let first = earliest.id.raw() + 1;
        let mut any = false;
        for s in &self.shared.at_work {
            let x = s.load(Ordering::Relaxed);
            if x == 0 {
                continue;
            }
            any = true;
            if !eng
                .placed_node(NodeId::from_raw(x - 1))
                .is_some_and(|n| past_frontier(eng, n, fr))
            {
                return false;
            }
        }
        if !any {
            return false;
        }
        // (and no node between the frontier and the earliest held one is to
        // run again: re-asked and queued again, not yet at its hold, or at
        // work and re-asked. The earliest held node is not the earliest to
        // run; the frontier moved past such a one to the held one, which
        // read what it had yet to define)
        let from = fr
            .checked_sub(1)
            .and_then(|r| crate::read::pos_of(eng, NodeId::from_raw(r)))
            .unwrap_or_else(|| crate::read::unit_start(eng, earliest));
        if crate::read::on_one_layout(eng, || crate::read::dirty_between(eng, from, earliest)).is_some() {
            return false;
        }
        if eng
            .frontier
            .compare_exchange(fr, first, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        eng.frontier_gen.fetch_add(1, Ordering::AcqRel);
        log_frontier(eng, fr, first, None, "past only");
        release_frontier(eng, Some(w));
        true
    }

    /// No work to find and runs held by the speculation window alone
    /// (docs/spec-window.md): it widens to the nearest of them and lets
    /// them go. The window orders runs; it never keeps the only work
    /// there is from running (a fold waiting on a node the window held:
    /// floats2col at -j16, no run ahead left to widen it, every worker
    /// idle).
    fn release_window_only(&self, eng: &Engine, w: &Worker) -> bool {
        if !eng.window_on.load(Ordering::Acquire) {
            return false;
        }
        // (only where nothing else runs: every worker idle, none at a
        // task. While one runs, what it commits widens the window or wakes
        // what it held; a worker idle beside it that let them go would let
        // go every run the window holds as fast as it found no other work)
        if self.shared.idle.load(Ordering::SeqCst) < self.shared.spawned.load(Ordering::SeqCst)
            || self.shared.at_work.iter().any(|s| s.load(Ordering::SeqCst) != 0)
        {
            return false;
        }
        let fr = eng.frontier.load(Ordering::Acquire);
        let nearest = {
            let m = self.shared.frontier_waiting.lock();
            m.iter()
                .filter_map(|&n| eng.placed_node(n))
                .filter(|x| x.held_window.load(Ordering::Acquire))
                .filter_map(|x| window_distance(eng, x, fr))
                .min()
        };
        let Some(d) = nearest else { return false };
        let cur = eng.window.load(Ordering::Acquire);
        if d > cur {
            eng.window.fetch_max(d, Ordering::AcqRel);
        }
        release_frontier(eng, Some(w)) > 0
    }

    fn idle_wait(&self, eng: &Engine, w: &Worker, edits: bool) -> Option<NodeId> {
        let m = &self.shared.park;
        let me = &self.shared.rest[w.ix];
        // (at a quiet point the caller's worker decides; the others rest
        // until it has work for them or stops, no thread spinning beside it)
        let quiet_ends = w.ix == 0;
        self.shared.idle.fetch_add(1, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        let t_idle = Instant::now();
        let runs0 = self.shared.runs_now.load(Ordering::Relaxed);
        let r = loop {
            stall_dump(eng, &self.shared, t_idle);
            // (every worker idle, work outstanding and no run ended for a
            // sixth of `Limits::stall`: a wake was lost and nothing will
            // run it; the evaluation ends stuck instead of waiting)
            if let Some(s) = self.limits.stall
                && t_idle.elapsed() >= s / 6
                && self.shared.idle.load(Ordering::SeqCst) >= self.shared.spawned.load(Ordering::SeqCst)
                && self.shared.outstanding.load(Ordering::SeqCst) > 0
                && self.shared.runs_now.load(Ordering::Relaxed) == runs0
            {
                let n = self.shared.outstanding.load(Ordering::SeqCst);
                self.fail(EvalError::Stuck {
                    node: None,
                    op: "",
                    why: format!(
                        "every worker idle for {} s with {n} nodes outstanding: a wake was lost",
                        (s / 6).as_secs()
                    ),
                });
                break None;
            }
            if self.shared.stop.load(Ordering::SeqCst) || self.shared.collect_wanted.load(Ordering::SeqCst) {
                break None;
            }
            // (looked for outside the park's lock: a steal is no reason to
            // keep another idle worker or a waker out)
            let seen = self.shared.wakes.load(Ordering::SeqCst);
            if let Some(n) = self.find_work(eng, w) {
                break Some(n);
            }
            if quiet_ends && self.shared.outstanding.load(Ordering::SeqCst) == 0 {
                break None;
            }
            if self.release_past_only(eng, w) || self.release_window_only(eng, w) {
                continue;
            }
            if edits && t_idle.elapsed() >= ABSORB_EVERY {
                break None;
            }
            let g = m.lock().unwrap_or_else(PoisonError::into_inner);
            if self.shared.stop.load(Ordering::SeqCst)
                || (quiet_ends && self.shared.outstanding.load(Ordering::SeqCst) == 0)
                || self.shared.collect_wanted.load(Ordering::SeqCst)
            {
                break None;
            }
            // (a push between the search and the lock notified no one
            // yet waiting: its work is looked for again before resting)
            if self.shared.wakes.load(Ordering::SeqCst) != seen {
                continue;
            }
            // (the timeout only guards a lost wakeup; a short one has idle
            // workers preempting the busy ones, and with 31 of them on 2 ms
            // one looked every 65 microseconds and took the node a worker
            // had just queued for itself)
            me.parked.store(true, Ordering::SeqCst);
            let (g, _) = me
                .cv
                .wait_timeout(g, if w.ix == 0 { ABSORB_EVERY } else { IDLE_LOOK })
                .unwrap_or_else(PoisonError::into_inner);
            me.parked.store(false, Ordering::SeqCst);
            drop(g);
        };
        self.shared.idle.fetch_sub(1, Ordering::SeqCst);
        r
    }
}

fn run_worker(eng: &Engine, pool: &Pool, w: &Worker, mut src: Option<&mut dyn EditSource>) {
    let mut last_absorb = Instant::now();
    loop {
        if pool.shared.stop.load(Ordering::SeqCst) {
            break;
        }
        if pool.shared.collect_wanted.load(Ordering::SeqCst) {
            // (what the edit source holds for edits not taken yet, given
            // before this worker comes: the last to come collects)
            if let Some(s) = src.as_deref() {
                let mut x = pool.shared.extra_roots.lock();
                s.roots(&mut |v| x.push(v));
            }
            pool.barrier(eng, w);
            continue;
        }
        if let Some(n) = pool.find_work(eng, w) {
            if let Err(e) = work_caught(eng, pool, w, n) {
                pool.fail(e);
                break;
            }
            if let Some(s) = src.as_deref_mut()
                && last_absorb.elapsed() >= ABSORB_EVERY
            {
                last_absorb = Instant::now();
                if absorb(eng, pool, w, s) {
                    pool.drain_pending(eng, w);
                }
            }
            continue;
        }
        let t = Instant::now();
        let mut found = None;
        for _ in 0..SPINS {
            if let Some(n) = pool.find_work(eng, w) {
                found = Some(n);
                break;
            }
            std::hint::spin_loop();
        }
        if found.is_none() && pool.outstanding() > 0 {
            found = pool.idle_wait(eng, w, src.is_some());
        }
        Worker::add(&w.idle_ns, ns(t));
        if let Some(n) = found {
            if let Err(e) = work_caught(eng, pool, w, n) {
                pool.fail(e);
                break;
            }
            continue;
        }
        if pool.outstanding() > 0 {
            // (idle while others run: an edit is taken now as after a
            // run of this worker's, not at the quiet point alone, which
            // a run that goes on for long, or for ever, keeps away; what
            // it changes under a running node stops that run at its next
            // check)
            if let Some(s) = src.as_deref_mut()
                && last_absorb.elapsed() >= ABSORB_EVERY
            {
                last_absorb = Instant::now();
                if absorb(eng, pool, w, s) {
                    pool.drain_pending(eng, w);
                }
            }
            continue;
        }
        // quiescent: the nodes that run only then (`Operation::final_only`)
        // released, and once they are done, the loop-back values
        // published, their readers whose answer moved run again (a pass)
        if w.ix == 0 {
            // (the wakes held to this quiet point: each reader's reads of
            // the field checked again now, re-asked where one moved; a
            // pass of the page layer's feedback into the text)
            let held: Vec<(NodeId, Wake)> = std::mem::take(&mut *eng.held.lock());
            if !held.is_empty() && w.held_passes.load(Ordering::Relaxed) >= u64::from(pool.limits.settle_passes) {
                // (as latexmk past its maximum runs: the wakes still held
                // are dropped, the last state is written out, and the
                // client reports it)
                w.held_unsettled.store(1, Ordering::Relaxed);
            } else if !held.is_empty() {
                let mut seen: crate::values::FxHashSet<NodeId> = crate::values::FxHashSet::default();
                let mut memo = crate::read::FoldMemo::new();
                let mut k = 0u64;
                for (n, wake) in held {
                    if !seen.insert(n) {
                        continue;
                    }
                    let node = eng.node(n);
                    if node.state() != State::Clean {
                        continue;
                    }
                    // (every read of the node checked, not the woken field's
                    // alone: a node held for two fields is listed twice and
                    // seen once, and a first form that checked the first
                    // wake's field let a stale read of the second stand, an
                    // amsldoc -j16 build wrong by one /PageLabels entry)
                    let moved = node
                        .record()
                        .is_some_and(|r| r.reads.iter().any(|rd| !crate::read::holds(eng, node, rd, &mut memo)));
                    if !moved {
                        continue;
                    }
                    // (`PHITEX_HELD_TRACE`: each pass's re-asked readers and
                    // the reads that moved)
                    if held_trace() {
                        let v = eng.values();
                        let what: Vec<String> = node.record().map_or_else(Vec::new, |r| {
                            r.reads
                                .iter()
                                .filter(|rd| !crate::read::holds(eng, node, rd, &mut memo))
                                .map(|rd| {
                                    let f = rd
                                        .kind(&eng.read_keys)
                                        .field()
                                        .map_or_else(|| "-".to_owned(), |f| v.render(f.0));
                                    let seen = rd
                                        .seen()
                                        .map_or_else(String::new, |x| v.render(x).chars().take(160).collect());
                                    format!("{f} (seen {seen})")
                                })
                                .collect()
                        });
                        eprintln!(
                            "held: pass {} node {} ({}) at {}: {}",
                            w.held_passes.load(Ordering::Relaxed) + 1,
                            n.raw(),
                            eng.op(node.op()).name(),
                            node.pos_hint().0,
                            what.join("; ")
                        );
                    }
                    if mark_stale_now(eng, Some(w), node, wake) {
                        k += 1;
                    }
                }
                if k > 0 {
                    Worker::add(&w.held_fired, k);
                    Worker::add(&w.held_passes, 1);
                    continue;
                }
            }
            // (the writers whose definitions are read back first; the
            // others, the output's (a page's PDF side, the link, the end's
            // file), feed no pass: kept until a quiet point that starts no
            // pass, below, so that a pass's output is not written only to
            // be written again by the next pass)
            let (mut rel, outs): (Vec<NodeId>, Vec<NodeId>) = std::mem::take(&mut *pool.shared.deferred.lock())
                .into_iter()
                .filter(|&n| eng.placed_node(n).is_some_and(|x| x.state() == State::Dirty))
                .partition(|&n| eng.op(eng.node(n).op()).read_back());
            pool.shared.deferred.lock().extend(outs);
            // (and those at the frontier: the layer is quiet, nothing
            // places a node before them now. Let go together: one that
            // reads what an earlier one defines waits for it, below)
            eng.frontier_gen.fetch_add(1, Ordering::AcqRel);
            let old = eng.frontier.swap(0, Ordering::AcqRel);
            log_frontier(eng, old, 0, None, "quiet");
            rel.extend(
                std::mem::take(&mut *pool.shared.frontier_waiting.lock())
                    .into_iter()
                    .filter(|&n| eng.placed_node(n).is_some_and(|x| x.state() == State::Dirty)),
            );
            // (and the runs that waited for a node that has not committed:
            // they go on, on its prediction. Not those waiting for one this
            // quiet point lets go itself, held at the frontier or deferred:
            // its commit is coming and lets them go (`release_waiting`); let
            // go with it, one read what it had yet to define and ran again
            // once it had: `settle::runs_held_at_the_frontier_are_let_go_
            // the_earliest_first`, red on every try with T2 waiting for T1)
            {
                let going: crate::values::FxHashSet<NodeId> = rel.iter().copied().collect();
                let mut m = pool.shared.waiting.lock();
                let (keep, go): (Vec<_>, Vec<_>) = std::mem::take(&mut *m)
                    .into_iter()
                    .partition(|(on, _)| going.contains(on));
                m.extend(keep);
                drop(m);
                rel.extend(
                    go.into_iter()
                        .flat_map(|(_, ws)| ws)
                        .filter(|&n| eng.placed_node(n).is_some_and(|x| x.state() == State::Dirty)),
                );
            }
            if !rel.is_empty() {
                // (on the ready heap, by place, where every idle worker
                // takes the earliest: as a block they were one worker's,
                // stolen from only past `BLOCK_MIN`, and the others idled
                // while it ran them one after another; Grail -j8 at a
                // quiet point inside a pass: worker 0 busy 7.5 s of 10.3,
                // the other seven 0.9-2 s each, accl 23051)
                w.ready_all(eng, &rel);
                continue;
            }
            let (k, pass) = crate::commit::publish_finals(eng, w);
            if run_log() {
                eprintln!("quiet: {} re-asked {k} pass {pass}", log_clock(Instant::now()));
            }
            if pass {
                let k = pool.shared.passes.fetch_add(1, Ordering::SeqCst) + 1;
                Worker::max(&w.final_max, k);
                // (the line quiet between passes: the records of the runs
                // the pass before threw away dropped)
                let swept = eng.sweep_readers() as u64;
                let mut log = w.log.lock();
                log.pass_starts.push(pool.shared.runs_now.load(Ordering::SeqCst));
                log.swept += swept;
            }
            if k > 0 {
                continue;
            }
            // (a wake held since the drain above, by anything run at this
            // quiet point: the next quiet point drains it, never the end)
            if !eng.held.lock().is_empty() {
                continue;
            }
            // (no pass follows: the output's writers kept above run now)
            let outs: Vec<NodeId> = std::mem::take(&mut *pool.shared.deferred.lock())
                .into_iter()
                .filter(|&n| eng.placed_node(n).is_some_and(|x| x.state() == State::Dirty))
                .collect();
            if !outs.is_empty() {
                w.seed(eng, &outs);
                continue;
            }
            if let Some(s) = src.as_deref_mut() {
                let items: Vec<Settled> = eng.take_settled_quiet();
                if !items.is_empty() && s.settled(eng, &items) {
                    pool.shared.stop.store(true, Ordering::SeqCst);
                    pool.shared.wake_all();
                    break;
                }
                last_absorb = Instant::now();
                // (an edit that queues nothing may still move a loop-back
                // value, a label removed: the line is quiet again, and its
                // values published, before the evaluation ends)
                if absorb(eng, pool, w, s) {
                    pool.drain_pending(eng, w);
                    continue;
                }
            }
            pool.shared.stop.store(true, Ordering::SeqCst);
            pool.shared.wake_all();
            break;
        }
        let t = Instant::now();
        let r = pool.idle_wait(eng, w, false);
        Worker::add(&w.idle_ns, ns(t));
        if let Some(n) = r
            && let Err(e) = work_caught(eng, pool, w, n)
        {
            pool.fail(e);
            break;
        }
    }
    // (one fewer for a collection's barrier to wait for)
    pool.shared.active.fetch_sub(1, Ordering::SeqCst);
    pool.shared.wake_all();
    let (sum, max) = take_wait();
    Worker::add(&w.lock_wait_ns, sum);
    Worker::max(&w.lock_wait_max_ns, max);
}

/// Edits taken from `s` during the evaluation: the engine's version is
/// theirs (`Report::version`, `Settled::version`), and the log has them
/// with the runs made before (`Report::absorbed`); whether there were any.
fn absorb(eng: &Engine, pool: &Pool, w: &Worker, s: &mut dyn EditSource) -> bool {
    let Some(v) = s.absorb(eng) else {
        return false;
    };
    eng.set_version(v);
    // (a new document: its loop-back takes passes of its own)
    eng.final_gen.fetch_add(1, Ordering::AcqRel);
    w.log
        .lock()
        .absorbed
        .push((v, pool.shared.runs_now.load(Ordering::SeqCst)));
    true
}

/// Whether dirty node `n` waits for the line's quiet point
/// (`Operation::final_only`): then it is kept, uncounted, for it.
fn defer(eng: &Engine, sh: &Shared, n: NodeId) -> bool {
    let Some(node) = eng.placed_node(n) else { return false };
    if !eng.op(node.op()).final_only() {
        return false;
    }
    sh.deferred.lock().push(n);
    true
}

/// Queues a node made dirty (a new node, a released sibling): on `w`'s
/// deque, or the engine's pending list when no evaluation runs.
pub(crate) fn enqueue(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) {
    // (a node of an edit that has never run: what it defines is not known,
    // so those after it wait for it (`hold_for`))
    if matches!(wake.cause, Cause::Created { .. })
        && (eng.holds_on.load(Ordering::Acquire) || (w.is_none() && eng.epoch.load(Ordering::Acquire) > 0))
        && n.record().is_none()
    {
        let op = eng.op(n.op());
        if !op.chains() && !op.works_at_frontier() && !op.final_only() && !n.moves.swap(true, Ordering::AcqRel) {
            eng.movers.lock().insert(n.id);
            eng.movers_n.fetch_add(1, Ordering::Release);
        }
    }
    *n.woke.lock() = Some(wake);
    match w {
        Some(w) if defer(eng, &w.shared, n.id) => {}
        Some(w) => {
            w.shared.outstanding.fetch_add(1, Ordering::SeqCst);
            w.ready().push(eng, n);
            w.offer();
        }
        None => eng.pend(n.id),
    }
}

/// Queues a dirty node again, keyed by its label now: one whose label
/// moved while it was queued (`commit::relocate`) stood in the ready heap
/// at its old label's key. The old entry, popped later, finds the node
/// run (`work` takes only a dirty node) and counts itself done. Outside
/// an evaluation, or for a node run only at the end, its place in the
/// queue does not matter.
pub(crate) fn requeue(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>) {
    let Some(w) = w else { return };
    if eng.op(n.op()).final_only() {
        return;
    }
    w.shared.outstanding.fetch_add(1, Ordering::SeqCst);
    w.ready().push(eng, n);
    w.offer();
}

/// A walk's re-ask: marks a clean node dirty and queues it. A running node
/// is left alone until its commit: what it reads is checked again there,
/// after the definition the walk is for was filed, so it needs no mark; one
/// here would run it again on a read it may well have made right. Once its
/// commit has checked (`Node::committing`), it is marked as `mark_dirty`
/// marks it.
pub(crate) fn mark_stale(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    if asked_before(eng, n, &wake) {
        return false;
    }
    if let Cause::Changed { field, .. } = wake.cause
        && crate::commit::ftrace(eng, field)
    {
        eprintln!(
            "ftrace: mark-stale {} node {} state {:?} held {} woke {:?} t {}",
            eng.values().as_str(field.0).unwrap_or_default(),
            n.id.raw(),
            n.state(),
            held_edge(eng, &wake, n),
            n.woke.lock().as_ref().map(|w| w.cause.clone()),
            eng.now_ns()
        );
    }
    if held_edge(eng, &wake, n) {
        // (a running reader is told too: a run on what moved may never
        // end, a TeX loop on a stale float state, and the quiet point the
        // wake is held for never comes while it runs)
        if n.state() == State::Running {
            n.asked.store(true, Ordering::SeqCst);
            note_stale(
                n,
                Wake {
                    by: wake.by,
                    cause: wake.cause.clone(),
                    at_ns: wake.at_ns,
                },
            );
        }
        hold_wake(eng, w, n.id, wake);
        return false;
    }
    mark_stale_now(eng, w, n, wake)
}

/// A re-ask of running node `n`, kept for its run's next check
/// (`Ctx::tick`): the run is stopped there if one of its reads moved,
/// and goes on if all still hold.
fn note_stale(n: &Node, wake: Wake) {
    let mut s = n.stale_wake.lock();
    match &*s {
        // (the cause told is the first: the run this one is for checks
        // what it kept, `Node::woke_merged`)
        Some(_) => n.woke_merged.store(true, Ordering::SeqCst),
        None => *s = Some(wake),
    }
    n.stale.store(true, Ordering::Release);
}

/// `mark_stale` with no wake held.
pub(crate) fn mark_stale_now(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    note_move(eng, wake.by, n, &wake.cause);
    frontier_back(eng, n, &wake);
    if n.committing.load(Ordering::Acquire) {
        return mark_dirty_now(eng, w, n, wake);
    }
    if n.state() == State::Clean && n.cas_state(State::Clean, State::Dirty) {
        log_wake(eng, w, n, &wake);
        enqueue(eng, w, n, wake);
        return true;
    }
    match n.state() {
        State::Dirty => merge_woke(eng, n, wake),
        State::Running => {
            // (told before looking again: either its commit, which sets
            // `committing` and then reads `asked`, sees this, or this
            // sees `committing`. Its filed reads rest on it)
            n.asked.store(true, Ordering::SeqCst);
            if n.committing.load(Ordering::SeqCst) {
                return mark_dirty_now(eng, w, n, wake);
            }
            note_stale(n, wake);
        }
        _ => {}
    }
    false
}

/// Whether node `n` was re-asked already for `wake`'s change: the same
/// field, by the same run of the same node (its commit's walks can reach
/// one reader twice, through the readers its lock found and a walk on),
/// while that commit goes on.
/// The first re-ask stands for both: it came after the commit had
/// published what it defines, so a run of `n` begun since reads it, one
/// at work then was told, and one committing then runs again. Asked
/// again, a reader taken at once by another worker after the first was
/// committing by the second and ran a third time, on reads that all held
/// (`moving_breaks_rescan_touched_pages_and_one` at -j4, 1 build in 60:
/// the page loop's rest four times, mc 80629). Records the re-ask.
/// But not where the node put right in place since what it published
/// (`Node::restated`): a reader that read the older value and committed
/// between the two re-asks was asked only for it, and the second re-ask
/// is the one that carries the change (a relation counted again from an
/// input that moved under it, `commit::settle_rel`: a name built from a
/// stepped counter kept the counter's older value at -j8, 1 build in 15).
fn asked_before(eng: &Engine, n: &Node, wake: &Wake) -> bool {
    let (Some(by), Cause::Changed { field, .. } | Cause::Range { field, .. }) = (wake.by, &wake.cause) else {
        return false;
    };
    // (within its commit only: a reader filed while it committed, at a
    // stop its commit had taken, is asked again after it,
    // `commit::reask_late_stops`, for a read the first re-ask did not see)
    let Some(b) = eng.placed_node(by).filter(|b| b.committing.load(Ordering::Acquire)) else {
        return false;
    };
    let key = (
        by,
        b.serial.load(Ordering::Acquire),
        b.restated.load(Ordering::Acquire),
        *field,
    );
    let mut asks = n.asks.lock();
    if asks.contains(&key) {
        return true;
    }
    // (kept while their commits go on: another node's re-ask between two
    // of one commit's, the routine's of the page after, left only its own
    // and the commit's second re-ask went through, 1 build in 160)
    asks.retain(|&(m, ser, _, _)| {
        eng.placed_node(m)
            .is_some_and(|x| x.committing.load(Ordering::Acquire) && x.serial.load(Ordering::Acquire) == ser)
    });
    asks.push(key);
    false
}

/// Marks a node dirty for `wake`'s reason and queues it, unless it is
/// already dirty, inert or gone. A running node becomes `RunningDirty`: its
/// reads return `Stop` from now, its commit goes through, and it is queued
/// after. Returns whether it was newly marked.
pub(crate) fn mark_dirty(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    if asked_before(eng, n, &wake) {
        return false;
    }
    if held_edge(eng, &wake, n) {
        hold_wake(eng, w, n.id, wake);
        return false;
    }
    mark_dirty_now(eng, w, n, wake)
}

/// Whether `wake` of `n` is held to the next quiet point: a changed or
/// rebound definition by a node of an operation that settles at quiet
/// points, read by a node of one that does not.
fn held_edge(eng: &Engine, wake: &Wake, n: &Node) -> bool {
    let Some(by) = wake.by else { return false };
    if !matches!(wake.cause, Cause::Changed { .. } | Cause::Rebind { .. }) {
        return false;
    }
    if let Cause::Changed { field, .. } = wake.cause
        && eng
            .placed_node(by)
            .is_some_and(|b| eng.op(b.op()).settles_now(field, eng.values()))
    {
        // (at once, but not a definition withdrawn with none left on the
        // line: one the layer has yet to make again, the same as a rule,
        // by another of its nodes. A page that fires in a piece, numbered
        // anew by an edit before it, is fired by another finisher: the old
        // one's run stopped defining the piece's fire conflict and the new
        // one's defined it again as it was, and in between the piece ran
        // whole on no conflict, and once more on the same one, twice per
        // such piece, ~0.5 s each in circuitikz's examples, accl 25667.
        // Held, the quiet point reads it again: run once if it is gone,
        // not at all if it came back)
        return !eng.op(n.op()).settles_at_quiet()
            && eng
                .defs
                .between(field, crate::order::Pos(0), crate::order::Pos(u64::MAX))
                .is_empty();
    }
    settles_edge(eng, by, n)
}

/// Whether a definition by `by` reaches `reader` across the settling
/// line: `by`'s operation settles at quiet points and its last run said
/// nothing of `Ctx::settle_now`, `reader`'s does not settle, and the
/// reader stands before the layer's frontier. One past it read the
/// field while no frontier held it (before the layer first defined the
/// field, `Engine::mark_settled`, or with the layer's fold ended; a read
/// of a field the layer defines waits at the frontier,
/// `Ctx::hold_at_frontier`): asked again now, it waits for the
/// frontier to pass it and runs once on what the pages before it left,
/// as at one thread. Held, it ran again only at the next quiet point,
/// and the next page's routine read what that run left there: a chain
/// through every page, text to routine to text, one page a pass
/// (beautybook-en at -j4: pgf's `\pgf@picture@serial@count`, which each
/// page's TikZ header and the TikZ in its text step, re-asked one page
/// a pass, "the page layout did not settle after 12 passes" and no
/// output file in 11 of 30 builds, accl 22502, 22609).
pub(crate) fn settles_edge(eng: &Engine, by: NodeId, reader: &Node) -> bool {
    eng.placed_node(by)
        .is_some_and(|b| eng.op(b.op()).settles_at_quiet() && !b.settle_now.load(Ordering::Acquire))
        && !eng.op(reader.op()).settles_at_quiet()
        && !past_frontier(eng, reader, eng.frontier.load(Ordering::Acquire))
}

/// Lists a wake for the next quiet point (`Report::held_deferred`).
pub(crate) fn hold_wake(eng: &Engine, w: Option<&Worker>, n: NodeId, wake: Wake) {
    eng.held.lock().push((n, wake));
    if let Some(w) = w {
        Worker::add(&w.held_deferred, 1);
    }
}

/// `mark_dirty` with no wake held.
pub(crate) fn mark_dirty_now(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    note_move(eng, wake.by, n, &wake.cause);
    frontier_back(eng, n, &wake);
    loop {
        match n.state() {
            State::Clean => {
                if n.cas_state(State::Clean, State::Dirty) {
                    log_wake(eng, w, n, &wake);
                    enqueue(eng, w, n, wake);
                    return true;
                }
            }
            State::Running => {
                if n.cas_state(State::Running, State::RunningDirty) {
                    log_wake(eng, w, n, &wake);
                    *n.woke.lock() = Some(wake);
                    return true;
                }
            }
            State::Dirty | State::RunningDirty => {
                merge_woke(eng, n, wake);
                return false;
            }
            State::Inert | State::RunningInert | State::Gone => return false,
        }
    }
}

/// A node asked again before the run its last cause asks for: the cause
/// the run is told (`Ctx::woken_by`) is the one that asks most of it, an
/// edit of its text first, then the change nearest its start (no site:
/// the start itself), never one dropped for the cause already there: a
/// loop resuming from its state past a change behind it would keep a sum
/// that change made stale.
fn merge_woke(eng: &Engine, n: &Node, wake: Wake) {
    let mut w = n.woke.lock();
    // (two causes, one told: the run checks the reads it keeps, which the
    // one dropped may have moved (`Node::woke_merged`). A loop rest woken
    // by the hole it parked at filling, its start page's give-back changed
    // meanwhile, went on from its tail on the give-back it had and kept
    // that read unchecked, as one woken by the answer it parked on alone:
    // argumentation-doc at -j4, a page's float and its links from the
    // routine's earlier run, 2 to 7 builds in 60, accl 24790, 24989)
    if w.is_some() {
        n.woke_merged.store(true, Ordering::SeqCst);
    }
    if !keeps_woke(eng, w.as_ref(), &wake.cause) {
        *w = Some(wake);
    }
}

/// The site a wake's cause names, where its run goes back before.
fn wake_site(c: &Cause) -> Option<NodeId> {
    match c {
        Cause::Changed { site, .. } => *site,
        Cause::Range { by, .. } => Some(*by),
        Cause::Output { of } => Some(*of),
        Cause::Rebind { from, to, .. } => from.or(*to),
        Cause::Environment { .. } | Cause::Structure | Cause::Created { .. } | Cause::Edited => None,
    }
}

/// Whether a dirty node's wake `old` stands over a new one for `c`
/// ([`merge_woke`]): an edit's or a creation's, or one whose site is no
/// later.
fn keeps_woke(eng: &Engine, old: Option<&Wake>, c: &Cause) -> bool {
    match (old, c) {
        (_, Cause::Edited) | (None, _) => false,
        (Some(o), _) if matches!(o.cause, Cause::Edited | Cause::Created { .. }) => true,
        (Some(o), c) => {
            let at = |s: Option<NodeId>| {
                s.and_then(|s| eng.placed_node(s))
                    .map_or(crate::order::Pos(0), |x| x.pos_hint())
            };
            at(wake_site(&o.cause)) <= at(wake_site(c))
        }
    }
}

/// Whether asking `n` again for `c` would change nothing: it is queued
/// to run (not running), its wake stands over this one ([`keeps_woke`]),
/// and it is no settling node (whose re-ask moves the frontier back,
/// [`frontier_back`]). A change's re-ask need not then compute whether
/// `n`'s read moved (`commit::reask_range`): its run comes after the
/// change, and goes back at least as far.
pub(crate) fn wake_redundant(eng: &Engine, n: &Node, c: &Cause) -> bool {
    n.state() == State::Dirty && !eng.op(n.op()).settles_at_quiet() && keeps_woke(eng, n.woke.lock().as_ref(), c)
}

/// (`PHITEX_STALL_SECS=n`, profile) No run has ended for `n` seconds
/// while this worker was idle with nodes outstanding: the evaluation is
/// stuck (every worker idle: a count not released, a wake lost; some
/// still running: a run that never ends, a spin); the outstanding count,
/// the idle workers and every node not clean, inert or gone are printed
/// once, with their states (a `Running` one is the run that never ends).
fn stall_dump(eng: &Engine, shared: &Shared, t_idle: Instant) {
    static SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static DONE: AtomicBool = AtomicBool::new(false);
    static LAST_RUNS: AtomicU64 = AtomicU64::new(u64::MAX);
    static LAST_CHANGE_MS: AtomicU64 = AtomicU64::new(0);
    let secs = *SECS.get_or_init(|| {
        std::env::var("PHITEX_STALL_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    if secs == 0 || !crate::obs::enabled() || t_idle.elapsed().as_secs() < secs || DONE.load(Ordering::Relaxed) {
        return;
    }
    // (progress: runs ending anywhere; none for `secs` is a stall even
    // with workers busy)
    let runs = shared.runs_now.load(Ordering::Relaxed);
    let now_ms = u64::try_from(t_idle.elapsed().as_millis()).unwrap_or(u64::MAX);
    if LAST_RUNS.swap(runs, Ordering::Relaxed) != runs {
        LAST_CHANGE_MS.store(now_ms, Ordering::Relaxed);
        return;
    }
    if now_ms.saturating_sub(LAST_CHANGE_MS.load(Ordering::Relaxed)) < secs * 1000 {
        return;
    }
    let idle = shared.idle.load(Ordering::SeqCst);
    let spawned = shared.spawned.load(Ordering::SeqCst);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut out = format!(
        "stall: {} outstanding, {idle} of {spawned} workers idle, no run ended for {secs} s; nodes not clean:\n",
        shared.outstanding.load(Ordering::SeqCst)
    );
    {
        let fr = eng.frontier.load(Ordering::Acquire);
        let waiting: Vec<String> = shared
            .frontier_waiting
            .lock()
            .iter()
            .map(|&n| {
                let x = eng.placed_node(n);
                format!(
                    "{}{}{}",
                    n.raw(),
                    if x.as_ref().is_some_and(|x| x.held_window.load(Ordering::Acquire)) {
                        "w"
                    } else {
                        ""
                    },
                    x.and_then(|x| window_distance(eng, x, fr))
                        .map_or(String::new(), |d| format!(":{d}"))
                )
            })
            .collect();
        out.push_str(&format!(
            "  frontier {} window {} on {} waiting at the frontier {waiting:?}\n",
            fr.saturating_sub(1),
            eng.window.load(Ordering::Acquire),
            eng.window_on.load(Ordering::Acquire)
        ));
    }
    for sid in eng.order.all() {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        for (pos, nid) in &inner.nodes {
            let n = eng.node(*nid);
            let st = n.state();
            if matches!(st, State::Clean | State::Inert | State::Gone) {
                continue;
            }
            let op = eng.ops.get(n.op().0 as usize).map_or("?", |o| o.name());
            out.push_str(&format!(
                "  node {} {op} {st:?} pos {:?} committing {}\n",
                nid.raw(),
                pos.0,
                n.committing.load(Ordering::Acquire)
            ));
        }
    }
    eprint!("{out}");
}

/// (`PHITEX_WAKE_LOG`, profile) Whether every wake of a committed node is
/// printed: the node, the field and the cause, as the re-ask happens.
/// (`PHITEX_RUN_LOG`, profile) Whether every run is printed as it ends:
/// the node, its operation and place, and how it ended.
pub(crate) fn run_log() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| {
        crate::obs::enabled() && std::env::var_os("PHITEX_RUN_LOG").is_some_and(|v| !v.is_empty() && v != "0")
    })
}

/// (`PHITEX_RUN_LOG`) `t` on the run log's clock: ns since the first
/// time asked, one clock for the process (every evaluation's runs on it).
pub(crate) fn log_clock(t: Instant) -> u128 {
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    t.saturating_duration_since(*T0.get_or_init(Instant::now)).as_nanos()
}

/// (`PHITEX_RUN_LOG`) The settling layer's frontier moved: from `old` to
/// `new` (raw ids plus one, 0 none), by `by`, `why`; with the place of
/// the node it names.
pub(crate) fn log_frontier(eng: &Engine, old: u64, new: u64, by: Option<NodeId>, why: &str) {
    if !run_log() || old == new {
        return;
    }
    let pos = |f: u64| {
        f.checked_sub(1)
            .and_then(|r| eng.placed_node(NodeId::from_raw(r)))
            .map_or(0, |x| x.pos_hint().0)
    };
    eprintln!(
        "frontier: {} -> {} (at {}) by {} {why} t {}",
        old.saturating_sub(1),
        new.saturating_sub(1),
        pos(new),
        by.map_or(0, NodeId::raw),
        log_clock(Instant::now())
    );
}

pub(crate) fn wake_log() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("PHITEX_WAKE_LOG").is_some_and(|v| !v.is_empty() && v != "0"))
}

fn log_wake(eng: &Engine, w: Option<&Worker>, n: &Node, wake: &Wake) {
    if !n.committed() {
        return;
    }
    let field = match &wake.cause {
        Cause::Rebind { field, .. }
        | Cause::Changed { field, .. }
        | Cause::Range { field, .. }
        | Cause::Environment { field } => Some(*field),
        Cause::Output { .. } | Cause::Structure | Cause::Created { .. } | Cause::Edited => None,
    };
    if crate::obs::enabled() && wake_log() {
        let name = field.and_then(|f| eng.values().as_str(f.0)).unwrap_or_default();
        let site = match &wake.cause {
            Cause::Changed { site: Some(s), .. } | Cause::Range { by: s, .. } | Cause::Output { of: s } => Some(*s),
            Cause::Rebind { to, .. } => *to,
            _ => None,
        };
        let site_pos = site.map_or(String::new(), |s| format!(" site-pos {:?}", eng.node(s).pos_hint()));
        let site_op = site.map_or("-", |s| eng.op(eng.node(s).op()).name());
        let kind = match &wake.cause {
            Cause::Rebind { .. } => "Rebind",
            Cause::Changed { .. } => "Changed",
            Cause::Range { .. } => "Range",
            Cause::Output { .. } => "Output",
            Cause::Environment { .. } => "Environment",
            Cause::Structure => "Structure",
            Cause::Created { .. } => "Created",
            Cause::Edited => "Edited",
        };
        crate::obs::maybe_dump_line(eng);
        // (one line a wake, tab-separated for the attribution table: the
        // woken run's op, the field, the cause's kind, the site's op, the
        // time of the run it refutes)
        let fr = eng.frontier.load(Ordering::Acquire);
        let fr_pos = fr
            .checked_sub(1)
            .and_then(|f| eng.placed_node(NodeId::from_raw(f)))
            .map(|f| f.pos_hint().0);
        let marked = field.is_some_and(|f| eng.settle_marked(f));
        // (and the node whose commit woke it, with its op)
        // (its effect and what it may define, as its operation says:
        // `Pending` brackets, `Any`)
        let by = wake.by.map_or("-".to_owned(), |b| {
            let x = eng.node(b);
            let op = eng.op(x.op());
            let args = x.args();
            let eff = match op.effect(&args, eng.values()) {
                crate::api::Effect::Pending => "pending",
                crate::api::Effect::None => "none",
                _ => "scope",
            };
            let may = match op.may_define(&args, eng.values()) {
                crate::api::MayDefine::Any => "any",
                crate::api::MayDefine::Nothing => "nothing",
                crate::api::MayDefine::Names(_) => "names",
                crate::api::MayDefine::Prefixes(_) => "prefixes",
            };
            format!("{} {} {eff} {may}", b.raw(), op.name())
        });
        eprintln!(
            "wake: node {} op {:?} pos {:?} field {name} {:?}{site_pos} frontier-pos {fr_pos:?} marked {marked} by {by}\twake\t{}\t{}\t{name}\t{kind}\t{site_op}\t{}",
            n.id.raw(),
            n.op(),
            n.pos_hint(),
            wake.cause,
            eng.op(n.op()).name(),
            n.id.raw(),
            n.run_ns.load(Ordering::Relaxed)
        );
    }
    match w {
        Some(w) => {
            let mut log = w.log.lock();
            log.causes.push((n.id, wake.cause.clone()));
            if let Some(f) = field {
                *log.refuted_by_field.entry(f).or_default() += 1;
            }
        }
        None => eng.log_outside(n.id, wake.cause.clone(), field),
    }
}

/// One node's run and commit on worker `w`: state to `Running`, a `Ctx`,
/// the operation (panics caught into `EvalError::OpPanicked`), the commit,
/// then `Clean` or, if re-asked meanwhile, `Dirty` and queued again. A
/// `Stop` from the run is a refuted run: queued again without a commit.
/// `work`, a panic outside the run (a monoid's or a hook's, at a commit or
/// a re-ask) ending the evaluation with the error: the worker would die
/// with `n` counted outstanding, and the others would wait for it for ever;
/// `n` is pending again, for the next evaluation.
/// A worker's [`Shared::at_work`] slot, emptied when its task ends.
struct AtWork<'a>(Option<&'a AtomicU64>);

impl Drop for AtWork<'_> {
    fn drop(&mut self) {
        if let Some(s) = self.0 {
            s.store(0, Ordering::Relaxed);
        }
    }
}

fn work_caught(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
    // (the whole task, run, commit and re-asks: no collection meanwhile)
    let _task = eng.values().task();
    let slot = pool.shared.at_work.get(w.ix);
    if let Some(s) = slot {
        s.store(n.raw() + 1, Ordering::Relaxed);
    }
    let _done = AtWork(slot);
    catch_unwind(AssertUnwindSafe(|| work(eng, pool, w, n))).unwrap_or_else(|p| {
        let message = p
            .downcast_ref::<&str>()
            .map(|s| (*s).to_owned())
            .or_else(|| p.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "panic".to_owned());
        // (a commit cut short: the next evaluation checks the line first)
        eng.repair.store(true, Ordering::Release);
        give_back(eng, eng.node(n));
        Err(EvalError::OpPanicked {
            node: n,
            op: eng.op(eng.node(n).op()).name(),
            message,
        })
    })
}

/// A run this long, as its node's longest one that committed took (its
/// last may be a short one on a value on the way, where the routine's
/// long part did not run: then it committed that, and the text after it
/// ran the same 4 s at once, Grail's lastpage hook, accl 23633), starts
/// only once every node before it in its unit has settled: run sooner, on
/// what those nodes have not said yet, it is seconds thrown away (Grail's
/// \cite at -j8: the last pages' finishers, 4.1 s of hyperxmp each, on
/// values among finishers that were still running, where -j1 ran neither,
/// accl 23609). One asked for a change that holds then runs once; one
/// whose reads have come back runs nothing (`Ctx::last_run_stands`). The
/// order of the runs only: what they make is what they read.
const LONG_RUN_NS: u64 = 100_000_000;

/// `by`'s definition moved what `reader` read: a node to wait for from now
/// on (`hold_for`), until a commit of it moves nothing (`clear_move`). Only
/// what has moved tells: most links of the graph carry no change, and nodes
/// joined by those run side by side (para4000's deleted paragraph, its pages'
/// routines; amsldoc's heading edit, 13 runs, 55 ms at -j8 against 64 at -j1).
pub(crate) fn note_move(eng: &Engine, by: Option<NodeId>, reader: &Node, cause: &Cause) {
    let Some(by) = by.filter(|&b| b != reader.id) else {
        return;
    };
    let Some(b) = eng.placed_node(by) else { return };
    // (a routine moving the page loop, by its give-back: the loop goes on on
    // a guess past it anyway, and no node after it read what moved)
    if b.moves.load(Ordering::Relaxed) || eng.op(reader.op()).works_at_frontier() || eng.op(reader.op()).final_only() {
        return;
    }
    // (a relation's value, a counter's step: recomputed in place as what it
    // steps from moves, not by a run of its node: 500 routines shifted by an
    // inserted one have their answers at once, side by side)
    if let Cause::Changed { field, .. } = cause
        && b.record()
            .is_some_and(|r| r.defs.iter().any(|d| d.f == *field && d.rel.is_some()))
    {
        return;
    }
    if !b.moves.swap(true, Ordering::AcqRel) && eng.op(b.op()).chains() {
        eng.movers.lock().insert(by);
        eng.movers_n.fetch_add(1, Ordering::Release);
    }
}

/// A commit publishing what it published moved nothing yet: the walks
/// after it say whether it does (`note_move`).
pub(crate) fn clear_move(eng: &Engine, node: &Node) {
    if node.moves.load(Ordering::Relaxed)
        && node.moves.swap(false, Ordering::AcqRel)
        && eng.movers_n.load(Ordering::Acquire) != 0
        && eng.movers.lock().remove(&node.id)
    {
        eng.movers_n.fetch_sub(1, Ordering::Release);
    }
}

/// A node an edit removed at `at` in unit `u` (`Engine::remove`) leaves
/// the state after it as no run has seen it: the first node after it that
/// is to run again is the front of that change, the first to run on it at
/// one thread, and what it defines now is not known (the font a deleted
/// paragraph first loaded is loaded by the next paragraph that uses it).
/// Those after it wait for it (`hold_for`), as for a node an edit made
/// (`enqueue`), until it commits, changed or not (`clear_front`). Run
/// sooner, each ran on the removed node's definitions gone and none in
/// their place, a state no order of the runs has, and again once the front
/// had committed: microtype's font list, every paragraph after a deleted
/// one that first used a font at -j8, then those that first use a font
/// again on the list, about one edit in 13
/// (`a_font_list_append_does_not_run_again_when_the_list_moves`).
pub(crate) fn mark_front(eng: &Engine, at: crate::order::Pos, u: crate::api::UnitId) {
    let Some(x) = crate::read::on_one_layout(eng, || crate::read::to_run_after(eng, at, u)) else {
        return;
    };
    let Some(n) = eng.placed_node(x) else { return };
    if !n.front.swap(true, Ordering::AcqRel) && eng.movers.lock().insert(x) {
        eng.movers_n.fetch_add(1, Ordering::Release);
    }
}

/// A front's commit, or its removal (`mark_front`): those after it wait
/// for it no more. (A front is never one of a chain, whose moves keep it
/// listed, `note_move`.)
pub(crate) fn clear_front(eng: &Engine, node: &Node) {
    if node.front.load(Ordering::Relaxed)
        && node.front.swap(false, Ordering::AcqRel)
        && eng.movers.lock().remove(&node.id)
    {
        eng.movers_n.fetch_sub(1, Ordering::Release);
    }
}

/// The unsettled node `node` should wait for, by the last evaluation's
/// graph: one before it that has never run or moved a read of its chain
/// (`read::mover_before`), or that its last run read from and that moved a
/// reader at its last commit (`read::open_definer_before`). Without this,
/// on an edit that re-asks many nodes at once each worker ran its node on a
/// value about to move: a generated 82-page book's added paragraph took
/// 1.2 s at -j8 and 2 s at -j32 where -j1 takes 125 ms, Grail's deleted
/// paragraph's finishers 1790 ms of work at -j8 for -j1's 123 (accl 23926,
/// 23972). The rule comes from the old graph as an edit's whole schedule
/// (`PHITEX_ORACLE`, branch moonshot, 127 ms at -j8); every link of the graph
/// counted, unmoving ones too, makes pages that run side by side one chain
/// (para4000's deleted paragraph, -j8, 108 ms to 228 ms): hence `Node::moves`.
/// The page loop's rests are not held: they go on on a guess by themselves.
fn hold_for(eng: &Engine, node: &Node) -> Option<NodeId> {
    let op = eng.op(node.op());
    if op.works_at_frontier() || op.final_only() {
        return None;
    }
    // (a clean build's (the speculation window's, docs/spec-window.md):
    // a run again waits for a source of its last run that is to run again
    // and moved a reader; not for the nodes an edit made, there are none)
    if eng.holds_on.load(Ordering::Acquire)
        && eng.movers_n.load(Ordering::Acquire) != 0
        && let Some(x) = crate::read::mover_before(eng, node)
    {
        return Some(x);
    }
    let r = node.run_record()?;
    crate::read::open_definer_before(eng, node, &r).or_else(|| crate::read::derived_target_open(eng, node, &r))
}

fn work(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
    let node = eng.node(n);
    if !node.cas_state(State::Dirty, State::Running) {
        pool.shared.done_one();
        return Ok(());
    }
    // (a quiet point let it go: nothing it waited for will come first)
    let quiet = node.held.swap(false, Ordering::AcqRel);
    // (a long run waits for the nodes before it: `LONG_RUN_NS`)
    if node.full_ns.load(Ordering::Relaxed) >= LONG_RUN_NS
        && node.run_record().is_some_and(|r| r.state.is_none())
        && let Some(x) = crate::read::on_one_layout(eng, || crate::read::unsettled_before(eng, node))
    {
        if run_log() {
            eprintln!("long: node {} waits for {}", n.raw(), x.raw());
        }
        node.held.store(true, Ordering::Release);
        park_waiting(eng, pool, w, node, Some(x));
        return Ok(());
    }
    // (one of a chain waits for the node before it that moved a read of
    // the chain's: run sooner, it runs on the value before that one's,
    // and again. One thread takes them in order anyway)
    if !quiet
        && pool.shared.n > 1
        && (eng.holds_on.load(Ordering::Acquire)
            || (eng.window_on.load(Ordering::Acquire) && eng.window_chains.load(Ordering::Acquire)))
        && let Some(x) = hold_for(eng, node)
    {
        if run_log() {
            eprintln!("chain: node {} waits for {}", n.raw(), x.raw());
        }
        node.held.store(true, Ordering::Release);
        park_waiting(eng, pool, w, node, Some(x));
        return Ok(());
    }
    // (one that stopped at the frontier before: not begun past it, where
    // it would stop again and its run be thrown away; the end of a Grail
    // edit's document, 65-100 ms of commands an edit)
    // (one the window held: not begun past it, nor parked once let go,
    // `frontier_holds`; a run parked here and let go at once was queued
    // and taken again for ever, every run ending, nothing moving: the
    // livelock at -j16, docs/spec-window.md, Progress)
    if (node.held_frontier.load(Ordering::Relaxed) || node.held_window.load(Ordering::Acquire))
        && frontier_holds(eng, node, eng.frontier.load(Ordering::Acquire))
    {
        park_frontier(eng, pool, w, node);
        return Ok(());
    }
    let first = node.run_epoch.swap(pool.epoch, Ordering::AcqRel) != pool.epoch;
    if first {
        node.runs.store(0, Ordering::Relaxed);
        node.reruns.store(0, Ordering::Relaxed);
        Worker::add(&w.nodes_run, 1);
    }
    let last_ns = node.run_ns.load(Ordering::Relaxed);
    let runs = node.runs.fetch_add(1, Ordering::Relaxed) + 1;
    let op = eng.op(node.op()).clone();
    if runs > pool.limits.runs_per_node {
        give_back(eng, node);
        return Err(EvalError::RunawayNode {
            node: n,
            op: op.name(),
            runs,
        });
    }
    if let Some(t) = pool.limits.total_runs {
        let total = pool.total_runs.fetch_add(1, Ordering::Relaxed) + 1;
        if total > t {
            give_back(eng, node);
            return Err(EvalError::TotalRuns { runs: total });
        }
    }
    if pool.limits.time.is_some_and(|t| pool.t0.elapsed() > t) {
        give_back(eng, node);
        return Err(EvalError::Deadline);
    }
    let t = Instant::now();
    let version = eng.version.load(Ordering::Acquire);
    node.held_panic.store(false, Ordering::Release);
    let mut cx = Ctx::new(eng, Some(w), node);
    let retries0 = crate::read::retries();
    let stands = cx.last_run_stands();
    let r = match stands {
        Some(out) => Ok(Ok(out)),
        None => catch_unwind(AssertUnwindSafe(|| op.run(&mut cx))),
    };
    if run_log() {
        let how = match &r {
            Ok(Ok(_)) if stands.is_some() => "stands".to_owned(),
            Ok(Ok(_)) => "ok".to_owned(),
            Ok(Err(_)) if cx.wait_frontier => "held at the frontier".to_owned(),
            Ok(Err(_)) => match cx.wait_on {
                Some(on) => format!("waits for {}", on.raw()),
                None => "stopped".to_owned(),
            },
            Err(_) => "panicked".to_owned(),
        };
        // (and how it began: going on from a park, the cause that woke
        // it and the field it parked on)
        let parked = node
            .parked_on
            .lock()
            .and_then(|f| eng.values().as_str(f.0))
            .unwrap_or_default();
        // (and when: its start and end on the process's clock, ns, and
        // its worker, for a timeline of the runs)
        eprintln!(
            "run: node {} ({}) at {}: {how}; goes on {}, woken by {:?}, parked on {parked:?}\tt {} {} w {} e {}",
            n.raw(),
            op.name(),
            node.pos_hint().0,
            cx.goes_on,
            cx.woke,
            log_clock(t),
            log_clock(Instant::now()),
            w.ix,
            pool.epoch
        );
        let wf = match &cx.woke {
            Some(
                Cause::Range { field, .. }
                | Cause::Changed { field, .. }
                | Cause::Rebind { field, .. }
                | Cause::Environment { field },
            ) => eng.values().as_str(field.0),
            _ => None,
        };
        if let Some(wf) = wf {
            eprintln!("runf: node {} field {wf}", n.raw());
        }
        // (the reads it computed again on a moved layout, and the reads it
        // filed)
        eprintln!(
            "runr: node {} retries {} reads {}",
            n.raw(),
            crate::read::retries() - retries0,
            cx.product.reads.len()
        );
    }
    // (a resumption: the run woken by the answer its node's last run
    // parked on, going on from there; else the last run's work, if any,
    // was thrown away)
    let resumption = matches!(r, Ok(Ok(_))) && cx.goes_on && {
        let f = match &cx.woke {
            Some(Cause::Range { field, .. } | Cause::Changed { field, .. } | Cause::Rebind { field, .. }) => {
                Some(*field)
            }
            _ => None,
        };
        f.is_some() && *node.parked_on.lock() == f
    };
    // (a run that waited for a node before it that may define what it
    // read, before it did anything, `Ctx::wait_for_definer`: a wait, not
    // a run; its node runs when that node commits)
    // (and one the window held at its first read, before it did
    // anything: a wait too)
    let waited = ((cx.waits_only && cx.wait_on.is_some()) || cx.wait_window) && matches!(r, Ok(Err(_)));
    if waited {
        Worker::add(&w.waits, 1);
    } else if resumption {
        Worker::add(&w.resumed, 1);
    } else {
        if !first {
            Worker::add(&w.wasted_ns, last_ns);
        }
        if node.reruns.fetch_add(1, Ordering::Relaxed) + 1 == 2 {
            Worker::add(&w.refuted_nodes, 1);
        }
    }
    // (a re-ask kept for this run's checks is its commit's now)
    if node.stale.swap(false, Ordering::AcqRel) {
        node.stale_wake.lock().take();
    }
    let run_ns = ns(t);
    node.run_ns.store(run_ns, Ordering::Relaxed);
    Worker::add(&w.op_ns, run_ns);
    if !waited {
        Worker::add(&w.runs, 1);
    }
    w.shared.runs_now.fetch_add(1, Ordering::Relaxed);
    {
        let mut log = w.log.lock();
        if !waited {
            *log.runs_by_op.entry(node.op()).or_default() += 1;
        }
        if stands.is_some() {
            log.stood.push(node.id);
        }
        if crate::obs::enabled() {
            log.run_times.push((n, Duration::from_nanos(run_ns)));
        }
    }
    match r {
        // (a run that read what has moved since, or one re-asked or taken
        // meanwhile, panicked on inputs no longer current: run again)
        Err(_)
            if node.state() != State::Running
                || cx
                    .product
                    .reads
                    .iter()
                    .any(|rd| !crate::read::holds(eng, node, rd, &mut crate::read::FoldMemo::new())) =>
        {
            drop(cx);
            Worker::add(&w.stale_panics, 1);
            end_run(eng, pool, w, node, State::Dirty);
            Ok(())
        }
        Err(payload) => {
            // (held, not failed: a run still going may change what it
            // read; `Engine::evaluate` decides once the line is quiet)
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned());
            let reads = std::mem::take(&mut cx.product.reads);
            drop(cx);
            node.held_panic.store(true, Ordering::Release);
            give_back(eng, node);
            pool.shared
                .panicked_roots
                .lock()
                .extend(reads.iter().filter_map(|r| r.seen()));
            w.log.lock().panicked.push((n, reads, op.name(), message));
            pool.shared.done_one();
            Ok(())
        }
        Ok(Err(_stop)) if cx.stuck.is_some() => {
            let why = cx.stuck.take().unwrap_or_default();
            drop(cx);
            // (stuck on what the nodes before it have not settled: a guess
            // of theirs, which TeX never reads, may loop where the true
            // input does not (amsldoc -j16: an endless loop at line 1701
            // in 1 build of 800, then 100 errors and no PDF); it runs again
            // once that node commits, and is stuck in earnest only on a
            // prefix every node of which has run)
            if let Some(x) = crate::read::open_before(eng, node) {
                Worker::add(&w.stale_panics, 1);
                park_waiting(eng, pool, w, node, Some(x));
                return Ok(());
            }
            give_back(eng, node);
            Err(EvalError::Stuck {
                node: Some(n),
                op: op.name(),
                why,
            })
        }
        Ok(Err(_stop)) if cx.wait_frontier => {
            // (a layer's read held: the next run waits for the frontier
            // before it begins; the window's hold is its own,
            // `Node::held_window`)
            if !cx.wait_window {
                node.held_frontier.store(true, Ordering::Relaxed);
            }
            drop(cx);
            park_frontier(eng, pool, w, node);
            Ok(())
        }
        Ok(Err(_stop)) if cx.wait_on.is_some() => {
            let on = cx.wait_on.take();
            drop(cx);
            park_waiting(eng, pool, w, node, on);
            Ok(())
        }
        Ok(Err(_stop)) => {
            drop(cx);
            end_run(eng, pool, w, node, State::Dirty);
            Ok(())
        }
        Ok(Ok(out)) => {
            if stands.is_none() {
                node.full_ns.fetch_max(run_ns, Ordering::Relaxed);
            }
            debug_assert!(
                !cx.wait_frontier,
                "node {} ended its run past a stop at the frontier that it did not return",
                node.id.raw()
            );
            let product = cx.into_product(out);
            if product.parked_on.is_some() {
                Worker::add(&w.parks, 1);
            }
            *node.parked_on.lock() = product.parked_on;
            // (commits under way at once, `Report::commits_at_once`)
            let at_once = eng.committing.fetch_add(1, Ordering::Relaxed) + 1;
            eng.commit_peak.fetch_max(at_once, Ordering::Relaxed);
            let st = match product.stood.clone() {
                Some(r) => crate::commit::commit_stood(eng, w, node, &r, product.scope_gen),
                None => None,
            }
            .unwrap_or_else(|| commit(eng, w, node, product));
            eng.committing.fetch_sub(1, Ordering::Relaxed);
            // (the store past the collection's mark: the workers called to
            // its barrier as their runs end)
            let sh = &pool.shared;
            let next = sh.collect_next.load(Ordering::Relaxed);
            if next != u64::MAX
                && eng.values().payload_bytes() > next
                && !sh.collect_wanted.swap(true, Ordering::SeqCst)
            {
                sh.wake_all();
            }
            Worker::add(&w.commits, 1);
            Worker::add(&w.commit_ns, st.commit_ns);
            w.commit_max_cpu_ns.fetch_max(st.commit_cpu_ns, Ordering::Relaxed);
            Worker::add(&w.commit_allocs, st.commit_allocs);
            Worker::add(&w.commit_frees, st.commit_frees);
            if st.commit_ns > w.commit_max_ns.load(Ordering::Relaxed) {
                w.commit_max_ns.store(st.commit_ns, Ordering::Relaxed);
                w.commit_max_node.store(n.raw(), Ordering::Relaxed);
            }
            Worker::add(&w.reask_ns, st.reask_ns);
            w.note(&st);
            node.commit_ns.store(st.commit_ns, Ordering::Relaxed);
            node.reask_ns.store(st.reask_ns, Ordering::Relaxed);
            {
                let reads = node.record().map_or(0, |r| r.reads.len() as u64);
                let mut log = w.log.lock();
                let e = log.commit_by_op.entry(node.op()).or_default();
                e.0 += st.commit_ns + st.reask_ns;
                e.1 += reads;
            }
            let done = ns(pool.t0);
            node.done_ns.store(done, Ordering::Relaxed);
            if run_log() {
                eprintln!(
                    "commit: node {} done {} commit {} reask {} walked {} woken {}",
                    n.raw(),
                    log_clock(Instant::now()),
                    st.commit_ns,
                    st.reask_ns,
                    st.walked,
                    st.woken
                );
            }
            w.last_commit.store(n.raw(), Ordering::Relaxed);
            w.last_done.store(done, Ordering::Relaxed);
            if end_run(eng, pool, w, node, State::Clean) {
                eng.note_settled(node, out, version);
            }
            Ok(())
        }
    }
}

/// A node whose run a failure stopped (a limit, a panic), `Dirty` again on
/// the engine's pending list for the next evaluation.
fn give_back(eng: &Engine, node: &Node) {
    loop {
        let (from, to) = match node.state() {
            s @ (State::Running | State::RunningDirty) => (s, State::Dirty),
            State::RunningInert => (State::RunningInert, State::Inert),
            State::Clean | State::Dirty | State::Inert | State::Gone => return,
        };
        if node.cas_state(from, to) {
            node.committing.store(false, Ordering::Release);
            if to == State::Dirty {
                eng.pend(node.id);
            }
            return;
        }
    }
}

/// The end of a run on `w`: `Running` becomes `done` (`Clean` after a
/// commit, `Dirty` and queued again after a `Stop`), `RunningDirty` is
/// queued again, `RunningInert` (taken meanwhile) becomes `Inert`. The run
/// held one count of the outstanding nodes: it goes with the node queued
/// again, or is given up here, whatever state the node is found in.
/// Whether the node ended `Clean`. Its commit's `committing` is cleared
/// after the state is: a walk meanwhile marks it as `mark_dirty` does.
fn end_run(eng: &Engine, pool: &Pool, w: &Worker, node: &Node, done: State) -> bool {
    loop {
        let (from, to) = match node.state() {
            State::Running => (State::Running, done),
            State::RunningDirty => (State::RunningDirty, State::Dirty),
            State::RunningInert => (State::RunningInert, State::Inert),
            // (removed meanwhile; nothing else leaves a run's states)
            State::Gone | State::Inert | State::Clean | State::Dirty => {
                node.committing.store(false, Ordering::Release);
                pool.shared.done_one();
                return false;
            }
        };
        if !node.cas_state(from, to) {
            continue;
        }
        node.committing.store(false, Ordering::Release);
        // (not open any more: the readers filed at it after its commit
        // took its stops, re-asked)
        if to != State::Dirty {
            crate::commit::reask_late_stops(eng, Some(w), node);
            // (and the runs that parked on it after its commit released
            // them (`park_waiting`): it was running then, not settled, and
            // they were listed after that release; settled now, they go on)
            release_waiting(eng, Some(w), node.id);
        }
        // (a move back of the frontier its wake made, undone when the
        // layer is not to run again)
        if to == State::Clean {
            frontier_restore(eng, Some(w), node);
        }
        if to == State::Dirty && !defer(eng, &pool.shared, node.id) {
            // (stopped, to run again: another worker may take it at once.
            // A run stopped at a node its own worker has yet to run, taken
            // again by that worker before it, stopped again for ever, as
            // review.rs's `next` readers did)
            w.ready().push(eng, node);
            w.ready().since.store(0, Ordering::Relaxed);
            pool.shared.wake_one();
        } else {
            pool.shared.done_one();
        }
        return to == State::Clean;
    }
}

/// Whether node `n` has nothing more to commit that a run waiting for it
/// (`park_waiting`) could meet: gone or taken by a claim, or clean on a
/// run of its own. One dirty or running commits again (a node relocated
/// runs again where it stands, its record a run's, and a reader that
/// took that record for its commit was queued again at once, ahead of
/// it on one thread, and ran again for ever).
pub(crate) fn settled_node(eng: &Engine, n: NodeId) -> bool {
    eng.placed_node(n).is_none_or(|x| match x.state() {
        State::Gone | State::Inert => true,
        State::Clean => x.run_record().is_some_and(|r| !r.provisional),
        // (a run that panicked on inputs that hold, held for the
        // evaluation's decision: no commit of it comes in this pool. Waited
        // for, its readers waited again each time the quiet point let them
        // go, and the line never ended: latex_minutes_Protokol's finisher
        // 872 panicked once, its readers 197 and 227 ran 80 000 times each
        // in 150 s at -j1, accl 22791)
        _ => x.held_panic.load(Ordering::Acquire),
    })
}

/// A run that stopped to wait for node `on`'s commit (`Ctx::read`):
/// dirty, queued nowhere, listed under `on` (`Shared::waiting`) unless
/// `on` has committed meanwhile or the run was re-asked (then queued).
/// The node a run of `me` gives its worker to, if one: every worker is
/// at work and none is still to start, no run of `me`'s unit at work is
/// placed after `me` (the run furthest ahead gives way, one at a time),
/// and a node of `me`'s unit placed before `me` waits in a ready queue for
/// a worker: the earliest such. A run past a node still to run rests on
/// a guess of that node's answer, and a guess TeX never reads may loop
/// where the true input does not; held by such runs, every worker could
/// stay away from the text that would answer their guesses for ever
/// (biblatex at -j4, 1 build in 4 on four CPUs: three runs in bookmark's
/// parent walk on a guessed chain for minutes, 3500 nodes before them
/// queued). With this the earliest work waiting always gets a worker at
/// the next step of the run furthest ahead, at any speed and load. No
/// time is measured.
pub(crate) fn starved_by(eng: &Engine, w: &Worker, me: &Node) -> Option<NodeId> {
    let sh = &w.shared;
    if sh.n == 1 || sh.idle.load(Ordering::SeqCst) > 0 || sh.spawned.load(Ordering::Relaxed) < sh.n {
        return None;
    }
    let at = me.pos_hint().0;
    for slot in &sh.at_work {
        let raw = slot.load(Ordering::Acquire);
        if raw == 0 || raw - 1 == me.id.raw() {
            continue;
        }
        if eng
            .placed_node(NodeId::from_raw(raw - 1))
            .is_some_and(|o| o.unit == me.unit && o.pos_hint().0 > at)
        {
            return None;
        }
    }
    let mut best: Option<(u64, NodeId)> = None;
    for r in &sh.ready {
        if r.len.load(Ordering::Relaxed) == 0 {
            continue;
        }
        let q = r.heap.lock();
        let keys = q
            .placed
            .iter()
            .map(|x| ((x.0).0, (x.0).3))
            .chain(q.at_frontier.iter().map(|x| ((x.0).0, (x.0).2)));
        for (p, raw) in keys {
            if p >= at || best.is_some_and(|b| b.0 <= p) {
                continue;
            }
            let id = NodeId::from_raw(raw);
            if eng
                .placed_node(id)
                .is_some_and(|n| n.unit == me.unit && n.state() == State::Dirty && n.pos_hint().0 < at)
            {
                best = Some((p, id));
            }
        }
    }
    best.map(|b| b.1)
}

fn park_waiting(eng: &Engine, pool: &Pool, w: &Worker, node: &Node, on: Option<NodeId>) {
    let committed = |n: NodeId| settled_node(eng, n);
    if node.state() != State::Running || on.is_none_or(committed) || !node.cas_state(State::Running, State::Dirty) {
        end_run(eng, pool, w, node, State::Dirty);
        return;
    }
    node.committing.store(false, Ordering::Release);
    let Some(on) = on else { return };
    let mut m = pool.shared.waiting.lock();
    // (marked awaited before `on`'s commit is checked again, under the
    // lock its commit takes once it finds the mark: either the check
    // sees the commit or the commit sees the mark, no wake lost,
    // `release_waiting`)
    if let Some(x) = eng.placed_node(on) {
        x.awaited.store(true, Ordering::SeqCst);
    }
    std::sync::atomic::fence(Ordering::SeqCst);
    if committed(on) {
        drop(m);
        w.ready().push(eng, node);
        w.offer();
        return;
    }
    m.entry(on).or_default().push(node.id);
    drop(m);
    pool.shared.done_one();
}

/// A node of the settling layer re-asked by a change of a range it folds
/// at `by`, a node outside the layer (the text's contribution to the
/// page list): the layer may place nodes again anywhere past `by` (a page
/// fired elsewhere, its finisher), so its frontier goes back to `by`
/// until the layer's next run sets it (`Ctx::set_frontier`). Left where
/// the layer's last run put it (past `by`, or none once the fold reached
/// the text's end), a reader past `by` of what a routine defines read
/// the give-back of a routine about to run again on another page, and
/// ran again with every give-back after it.
fn frontier_back(eng: &Engine, n: &Node, wake: &Wake) {
    let (Cause::Range { .. }, Some(by)) = (&wake.cause, wake.by) else {
        return;
    };
    if !eng.op(n.op()).settles_at_quiet() {
        return;
    }
    let Some(b) = eng.placed_node(by) else { return };
    if eng.op(b.op()).settles_at_quiet() {
        return;
    }
    let mut fr = eng.frontier.load(Ordering::Acquire);
    loop {
        // (at `by` already, or before it; none is the fold's end)
        if fr == by.raw() + 1 || (fr != 0 && past_frontier(eng, b, fr)) {
            return;
        }
        match eng
            .frontier
            .compare_exchange(fr, by.raw() + 1, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                log_frontier(eng, fr, by.raw() + 1, Some(n.id), "back");
                let g = eng.frontier_gen.load(Ordering::Acquire);
                let mut b = n.back_frontier.lock();
                // (moved back again before it committed: the frontier it
                // replaced first, if nothing set one since)
                let from = match *b {
                    Some((to, from, bg)) if to == fr && bg == g => from,
                    _ => fr,
                };
                *b = Some((by.raw() + 1, from, g));
                return;
            }
            Err(x) => fr = x,
        }
    }
}

/// Undoes, once node `n`'s run has committed, the move back of the frontier its wake
/// made (`frontier_back`), when nothing has moved the frontier since and
/// neither `n` nor the layer node that last set it is to run again. The
/// layer's next run was to set it again; a commit of `n` that re-asks no
/// node of the layer brings none, and the frontier stayed where `n` put it,
/// before the page the loop had reached: every reader past it held, the
/// loop's next step among them, the only work left the later pages'
/// finishers on their old pages, one wave of them a commit that changed a
/// counter they pass on (circuitikz, a section added at the start: 4591
/// finisher runs of 373 finishers at -j1, each run until a quiet point let
/// the held ones go).
pub(crate) fn frontier_restore(eng: &Engine, w: Option<&Worker>, n: &Node) {
    let Some((to, from, g)) = n.back_frontier.lock().take() else {
        return;
    };
    let again = |x: &Node| matches!(x.state(), State::Dirty | State::Running);
    if again(n) {
        // (re-asked meanwhile: its next commit decides)
        *n.back_frontier.lock() = Some((to, from, g));
        return;
    }
    if eng.frontier_gen.load(Ordering::Acquire) != g {
        return;
    }
    let by = eng.frontier_by.load(Ordering::Acquire);
    if by
        .checked_sub(1)
        .and_then(|b| eng.placed_node(NodeId::from_raw(b)))
        .is_some_and(|x| again(x))
    {
        return;
    }
    if eng
        .frontier
        .compare_exchange(to, from, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
    {
        log_frontier(eng, to, from, Some(n.id), "restore");
        release_frontier(eng, w);
    }
}

/// Its widest: the whole text.
pub(crate) const WINDOW_MAX: u64 = 1 << 20;
/// The speculation window's first width, in places (docs/spec-window.md):
/// whole. It narrows only where the page layer refutes a run ahead of it:
/// a text that exchanges nothing with the layer is never held.
pub(crate) const WINDOW_START: u64 = WINDOW_MAX;
/// The places a frontier's move counts where its positions are not known.
pub(crate) const WINDOW_STEP: u64 = 8;

/// `node`'s distance past the frontier `fr`, in places (slices as pushed,
/// `order::GAP` labels apart), where the speculation window is on and
/// `node` stands past it in its unit (docs/spec-window.md); none
/// otherwise.
pub(crate) fn window_distance(eng: &Engine, node: &Node, fr: u64) -> Option<u64> {
    if fr == 0 || !eng.window_on.load(Ordering::Acquire) || eng.op(node.op()).settles_at_quiet() {
        return None;
    }
    let f = eng.placed_node(NodeId::from_raw(fr - 1))?;
    let (a, b) = (f.pos_hint().0, node.pos_hint().0);
    let span = crate::order::UNIT_SPAN;
    if b <= a || a / span != b / span {
        return None;
    }
    Some((b - a) / crate::order::GAP)
}

/// Whether the speculation window holds `node` back at frontier `fr`:
/// past it by more than the window.
pub(crate) fn window_holds(eng: &Engine, node: &Node, fr: u64) -> bool {
    window_distance(eng, node, fr).is_some_and(|d| beyond_window(eng, d))
}

/// Whether `d` places past the frontier is past the window: never where
/// it is whole (a unit holds more places than `WINDOW_MAX`).
pub(crate) fn beyond_window(eng: &Engine, d: u64) -> bool {
    let w = eng.window.load(Ordering::Acquire);
    w < WINDOW_MAX && d > w
}

/// A run ahead of the frontier by `ahead` places refuted: the window
/// halves, to at most half that distance and at least one place.
pub(crate) fn window_refuted(eng: &Engine, ahead: u64) {
    eng.window_missed.store(true, Ordering::Release);
    let w = eng.window.load(Ordering::Acquire);
    let n = (w.min(ahead) / 2).max(eng.window_floor.load(Ordering::Acquire));
    if n < w
        && eng
            .window
            .compare_exchange(w, n, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        // (the held runs looked at at this width: let go once it has grown
        // by half again, `window_acked`)
        eng.window_released.store(n, Ordering::Release);
        eng.window_stats[1].fetch_add(1, Ordering::Relaxed);
    }
}

/// A run ahead of the frontier committed: the window grows by a place (a
/// window's worth of runs in, it has doubled: as fast as the text runs,
/// not only as the fold moves). True where it has grown by half since the
/// runs it held were last looked at: let them go where now within it.
pub(crate) fn window_acked(eng: &Engine) -> bool {
    if !eng.window_on.load(Ordering::Acquire) {
        return false;
    }
    // (once the layer has refuted a run ahead, a place a window's worth
    // of runs in, as a congestion window avoids the loss it met: grown a
    // place a run, floats2col's window was whole again within a page)
    if exchanged(eng) && eng.window_pin.load(Ordering::Acquire) {
        return false;
    }
    if exchanged(eng) {
        let w = eng.window.load(Ordering::Acquire);
        let k = eng.window_acks.fetch_add(1, Ordering::AcqRel) + 1;
        if k < w.max(1) {
            return false;
        }
        eng.window_acks.store(0, Ordering::Release);
    }
    let w = eng.window.fetch_add(1, Ordering::AcqRel).saturating_add(1);
    if w > WINDOW_MAX {
        eng.window.store(WINDOW_MAX, Ordering::Release);
        return false;
    }
    let seen = eng.window_released.load(Ordering::Acquire);
    w >= seen.saturating_add(seen / 2).max(seen + 1)
        && eng
            .window_released
            .compare_exchange(seen, w, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
}

/// The layer moved its frontier on by `by` places: with no run ahead
/// refuted since its last move, the window grows by twice that and at
/// least doubles (a fold that moves in bursts, many pages a run, widens
/// it as fast as it moves).
pub(crate) fn window_moved_on(eng: &Engine, by: u64) {
    if !eng.window_on.load(Ordering::Acquire) {
        return;
    }
    if eng.window_missed.swap(false, Ordering::AcqRel) {
        return;
    }
    let w = eng.window.load(Ordering::Acquire);
    // (after the layer refuted a run ahead, a place a move: it doubled
    // before, and was whole again within a few pages)
    let n = if exchanged(eng) && eng.window_pin.load(Ordering::Acquire) {
        w
    } else if exchanged(eng) {
        w.saturating_add(1).min(WINDOW_MAX)
    } else {
        (w * 2).max(w.saturating_add(by.saturating_mul(2))).min(WINDOW_MAX)
    };
    if n > w
        && eng
            .window
            .compare_exchange(w, n, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    {
        eng.window_stats[2].fetch_add(1, Ordering::Relaxed);
    }
}

/// The page layer has refuted a run ahead of it in this evaluation: its
/// routines and the text exchange state (docs/spec-window.md).
pub(crate) fn exchanged(eng: &Engine) -> bool {
    eng.window_aimd.load(Ordering::Acquire) && eng.window_stats[1].load(Ordering::Acquire) > 0
}

/// Whether `node` stands past the settling layer's frontier `fr` (its
/// raw id plus one, `Engine::frontier`): after it, which stands.
pub(crate) fn past_frontier(eng: &Engine, node: &Node, fr: u64) -> bool {
    fr != 0
        && eng
            .placed_node(NodeId::from_raw(fr - 1))
            .is_some_and(|x| x.state() != State::Gone && node.pos_hint() > x.pos_hint())
}

/// Whether `node`'s run waits at frontier `fr`: held by the window alone
/// (`Node::held_window`), while past it and not let go by it
/// (`Node::window_pass`); else while past the frontier. The one test of
/// a held run, before it begins (`work`), as it parks (`park_frontier`)
/// and as it is let go (`release_frontier`): a run let go is never parked
/// again by a test that disagrees (docs/spec-window.md, Progress).
pub(crate) fn frontier_holds(eng: &Engine, node: &Node, fr: u64) -> bool {
    if node.held_window.load(Ordering::Acquire) {
        !node.window_pass.load(Ordering::Acquire) && window_holds(eng, node, fr)
    } else {
        past_frontier(eng, node, fr)
    }
}

/// A run that stopped at a read past the frontier (`Ctx::read`): dirty,
/// queued nowhere, listed in `Shared::frontier_waiting` unless the
/// frontier passed it meanwhile or the run was re-asked (then queued).
fn park_frontier(eng: &Engine, pool: &Pool, w: &Worker, node: &Node) {
    let held = || frontier_holds(eng, node, eng.frontier.load(Ordering::Acquire));
    if node.state() != State::Running || !held() || !node.cas_state(State::Running, State::Dirty) {
        end_run(eng, pool, w, node, State::Dirty);
        return;
    }
    node.committing.store(false, Ordering::Release);
    let mut m = pool.shared.frontier_waiting.lock();
    // (checked again under the lock the frontier's move takes: no wake
    // lost)
    if !held() {
        drop(m);
        w.ready().push(eng, node);
        w.offer();
        return;
    }
    m.push(node.id);
    drop(m);
    if run_log() {
        eprintln!(
            "frontier: hold {} at {} t {}",
            node.id.raw(),
            eng.frontier.load(Ordering::Acquire).saturating_sub(1),
            log_clock(Instant::now())
        );
    }
    pool.shared.done_one();
}

/// The runs waiting at the frontier that it has passed queued again
/// (`park_frontier`); how many.
pub(crate) fn release_frontier(eng: &Engine, w: Option<&Worker>) -> usize {
    let Some(w) = w else { return 0 };
    let fr = eng.frontier.load(Ordering::Acquire);
    let go: Vec<NodeId> = {
        let mut m = w.shared.frontier_waiting.lock();
        if m.is_empty() {
            return 0;
        }
        // (held by the window alone: let go once within it, with its pass:
        // its next run is not held by the window again)
        let (go, keep): (Vec<NodeId>, Vec<NodeId>) = m
            .iter()
            .partition(|&&n| eng.placed_node(n).is_none_or(|x| !frontier_holds(eng, x, fr)));
        for &n in &go {
            if let Some(x) = eng.placed_node(n)
                && x.held_window.load(Ordering::Acquire)
            {
                x.window_pass.store(true, Ordering::Release);
            }
        }
        *m = keep;
        go
    };
    if run_log() && !go.is_empty() {
        eprintln!(
            "frontier: released {} at {} t {}",
            go.len(),
            fr.saturating_sub(1),
            log_clock(Instant::now())
        );
    }
    let mut released = 0;
    for n in go {
        if let Some(x) = eng.placed_node(n).filter(|x| x.state() == State::Dirty) {
            // (the run goes on for the cause that woke it, which the hold
            // only paused: its resumption point and the wake's attribution
            // are that cause's, not a structural one's)
            let wake = x.woke.lock().as_ref().map_or(
                Wake {
                    by: None,
                    cause: Cause::Structure,
                    at_ns: eng.now_ns(),
                },
                |o| Wake {
                    by: o.by,
                    cause: o.cause.clone(),
                    at_ns: eng.now_ns(),
                },
            );
            enqueue(eng, Some(w), x, wake);
            released += 1;
        }
    }
    released
}

/// The runs waiting for `n`'s commit queued again (`park_waiting`).
pub(crate) fn release_waiting(eng: &Engine, w: Option<&Worker>, n: NodeId) {
    let Some(w) = w else { return };
    // (no run marked it awaited: no lock, which every commit of every
    // worker took (at 32 threads half their time waiting on it, defuse's
    // `commits_do_not_wait_on_each_other`). The commit is visible before
    // the mark is read, and a run that parks marks before it checks the
    // commit, both past a fence: one of the two sees the other)
    std::sync::atomic::fence(Ordering::SeqCst);
    let Some(node) = eng.placed_node(n) else { return };
    if !node.awaited.load(Ordering::SeqCst) {
        return;
    }
    let ws = {
        let mut m = w.shared.waiting.lock();
        node.awaited.store(false, Ordering::SeqCst);
        m.remove(&n)
    };
    let alone = ws.as_ref().is_some_and(|l| l.len() == 1);
    for m in ws.into_iter().flatten() {
        if let Some(x) = eng.placed_node(m).filter(|x| x.state() == State::Dirty) {
            // (stopped before its run or in it: what asked for that run
            // still asks, an edit or a creation above all, which
            // `Ctx::last_run_stands` must not take for a read's)
            let chained = x.held.swap(false, Ordering::AcqRel);
            let kept = x.woke.lock().take();
            // (the one a chain goes on with, this worker's next: another
            // woken for it takes it from here with nothing in its caches.
            // book5's deleted paragraph at -j8, a chain of text nodes held
            // and let go: 204 ms to 273 ms, the same runs, accl 23994)
            if chained && kept.is_some() && alone && !defer(eng, &w.shared, x.id) {
                *x.woke.lock() = kept;
                w.shared.outstanding.fetch_add(1, Ordering::SeqCst);
                w.ready().push(eng, x);
                continue;
            }
            enqueue(
                eng,
                Some(w),
                x,
                kept.unwrap_or(Wake {
                    by: Some(n),
                    cause: Cause::Output { of: n },
                    at_ns: eng.now_ns(),
                }),
            );
        }
    }
}

/// A loop-back reader re-asked at a quiet point (`commit::publish_finals`):
/// counts the rerun; past `Limits::final_reruns` in one evaluation the
/// node is left with its last answer, listed in `Report::capped`, and not
/// queued. True when it may run.
pub(crate) fn allow_final_rerun(eng: &Engine, w: &Worker, node: &Node) -> bool {
    let ep = eng.final_gen.load(Ordering::Acquire);
    if node.final_epoch.swap(ep, Ordering::AcqRel) != ep {
        node.final_reruns.store(0, Ordering::Release);
    }
    let k = node.final_reruns.fetch_add(1, Ordering::AcqRel);
    if k < w.shared.final_limit {
        return true;
    }
    w.log.lock().capped.push(node.id);
    false
}

#[cfg(test)]
mod frontier_tests {
    //! The settling layer's frontier goes back to a change of the range
    //! it folds (`frontier_back`).

    use std::sync::Arc;
    use std::sync::atomic::Ordering;

    use crate::node::Wake;
    use crate::{Cause, Ctx, Engine, Field, NodeId, Operation, Stop, SumMonoid, ValueId, Values};

    /// `[x]`: text, contributes `vp` := x.
    struct Piece;

    impl Operation for Piece {
        fn name(&self) -> &'static str {
            "piece"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let vp = cx.values().field("vp");
            cx.contribute(vp, cx.args()[0]);
            Ok(ValueId::UNIT)
        }
    }

    /// The page layer: folds `vp` before it and ends its fold (no
    /// frontier).
    struct Layer;

    impl Operation for Layer {
        fn name(&self) -> &'static str {
            "layer"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let vp = cx.values().field("vp");
            let x = cx.fold(vp, None, None)?;
            cx.set_frontier(None);
            Ok(x)
        }

        fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
            false
        }

        fn settles_at_quiet(&self) -> bool {
            true
        }

        fn works_at_frontier(&self) -> bool {
            true
        }
    }

    /// Line: piece A, piece B, the layer L, whose fold ended (no
    /// frontier). L re-asked by B's contribution: the frontier is B. Then
    /// by A's: A, before it. By B's again: it stays at A. A wake of L
    /// for another cause, or of a piece, moves nothing; the wake itself
    /// moves it, and the line runs quiet again.
    #[test]
    fn a_change_of_the_range_the_layer_folds_moves_its_frontier_back_to_it() {
        let e = Engine::new();
        let (piece, layer) = (e.register(Arc::new(Piece)), e.register(Arc::new(Layer)));
        let v = e.values();
        let vp = v.field("vp");
        e.declare_accumulator(vp, Arc::new(SumMonoid));
        let u = e.new_unit();
        let a = e.push(u, piece, vec![v.int(1)]);
        let b = e.push(u, piece, vec![v.int(2)]);
        let l = e.push(u, layer, vec![]);
        e.evaluate(1).unwrap();
        assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3));
        assert_eq!(e.frontier.load(Ordering::Acquire), 0, "the fold ended");
        let (ln, an) = (e.placed_node(l).unwrap(), e.placed_node(a).unwrap());
        let range = |by: NodeId| Wake {
            by: Some(by),
            cause: Cause::Range { field: vp, by },
            at_ns: 0,
        };
        let at = |n: NodeId| n.raw() + 1;
        super::frontier_back(&e, ln, &range(b));
        assert_eq!(e.frontier.load(Ordering::Acquire), at(b));
        super::frontier_back(&e, ln, &range(a));
        assert_eq!(e.frontier.load(Ordering::Acquire), at(a));
        super::frontier_back(&e, ln, &range(b));
        assert_eq!(e.frontier.load(Ordering::Acquire), at(a), "past the frontier: stays");
        e.frontier.store(0, Ordering::Release);
        let changed = Wake {
            by: Some(b),
            cause: Cause::Changed {
                field: vp,
                site: Some(b),
            },
            at_ns: 0,
        };
        super::frontier_back(&e, ln, &changed);
        super::frontier_back(&e, an, &range(b));
        assert_eq!(e.frontier.load(Ordering::Acquire), 0, "no range of the layer's");
        super::mark_stale_now(&e, None, ln, range(b));
        assert_eq!(e.frontier.load(Ordering::Acquire), at(b));
        e.evaluate(1).unwrap();
        assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(3));
    }

    /// A wake's move back of the frontier is undone once the node woken
    /// has committed, nothing having set the frontier since and the layer
    /// not to run again: back at the fold's end. Two moves back before the
    /// commit: the first frontier comes back. A frontier set meanwhile, or
    /// the layer to run again: the move stays, for the layer's run to set.
    #[test]
    fn a_move_back_of_the_frontier_is_undone_when_its_node_commits_and_nothing_set_it() {
        let e = Engine::new();
        let (piece, layer) = (e.register(Arc::new(Piece)), e.register(Arc::new(Layer)));
        let v = e.values();
        let vp = v.field("vp");
        e.declare_accumulator(vp, Arc::new(SumMonoid));
        let u = e.new_unit();
        let a = e.push(u, piece, vec![v.int(1)]);
        let b = e.push(u, piece, vec![v.int(2)]);
        let l = e.push(u, layer, vec![]);
        e.evaluate(1).unwrap();
        assert_eq!(e.frontier_by.load(Ordering::Acquire), l.raw() + 1, "the layer set it");
        let ln = e.placed_node(l).unwrap();
        let range = |by: NodeId| Wake {
            by: Some(by),
            cause: Cause::Range { field: vp, by },
            at_ns: 0,
        };
        let at = |n: NodeId| n.raw() + 1;
        let fr = || e.frontier.load(Ordering::Acquire);
        super::frontier_back(&e, ln, &range(b));
        assert_eq!(fr(), at(b));
        super::frontier_restore(&e, None, ln);
        assert_eq!(fr(), 0, "undone: the fold's end");
        super::frontier_back(&e, ln, &range(b));
        super::frontier_back(&e, ln, &range(a));
        assert_eq!(fr(), at(a));
        super::frontier_restore(&e, None, ln);
        assert_eq!(fr(), 0, "two moves back: the first frontier");
        super::frontier_back(&e, ln, &range(b));
        e.frontier_gen.fetch_add(1, Ordering::AcqRel);
        super::frontier_restore(&e, None, ln);
        assert_eq!(fr(), at(b), "set meanwhile: stays");
        e.frontier.store(0, Ordering::Release);
        super::frontier_back(&e, ln, &range(b));
        assert!(ln.cas_state(super::State::Clean, super::State::Dirty));
        super::frontier_restore(&e, None, ln);
        assert_eq!(fr(), at(b), "the layer to run again: stays");
        assert!(ln.cas_state(super::State::Dirty, super::State::Clean));
        super::frontier_restore(&e, None, ln);
        assert_eq!(fr(), 0, "kept for its next commit");
    }

    /// A layer's node queued while the frontier stood at the line's end is
    /// taken by where the frontier stands when it is taken: moved back to
    /// A, the layer goes before B's piece, queued first and standing before
    /// the layer; moved on past B, after it.
    #[test]
    fn a_node_at_the_frontier_is_ordered_by_where_the_frontier_stands_when_taken() {
        let e = Engine::new();
        let (piece, layer) = (e.register(Arc::new(Piece)), e.register(Arc::new(Layer)));
        let v = e.values();
        e.declare_accumulator(v.field("vp"), Arc::new(SumMonoid));
        let u = e.new_unit();
        let a = e.push(u, piece, vec![v.int(1)]);
        let b = e.push(u, piece, vec![v.int(2)]);
        let l = e.push(u, layer, vec![]);
        e.evaluate(1).unwrap();
        let (ln, bn) = (e.placed_node(l).unwrap(), e.placed_node(b).unwrap());
        let ready = super::Ready::new(std::time::Instant::now());
        let queue = || {
            ready.push(&e, bn);
            ready.push(&e, ln);
        };
        e.frontier.store(l.raw() + 1, Ordering::Release);
        queue();
        e.frontier.store(a.raw() + 1, Ordering::Release);
        assert_eq!(ready.first_key(&e), Some(e.placed_node(a).unwrap().pos_hint().0));
        assert_eq!(ready.pop(&e), Some(l), "the frontier moved back after it was queued");
        assert_eq!(ready.pop(&e), Some(b));
        queue();
        e.frontier.store(l.raw() + 1, Ordering::Release);
        assert_eq!(ready.pop(&e), Some(b), "the frontier past the piece");
        assert_eq!(ready.pop(&e), Some(l));
        queue();
        e.frontier.store(0, Ordering::Release);
        assert_eq!(ready.pop(&e), Some(b), "no frontier: by their places");
        assert_eq!(ready.pop(&e), Some(l));
        assert_eq!(ready.pop(&e), None);
    }

    /// An edit's nodes at one thread are a block, run by their labels; the
    /// layer's, edited with them, is not one of it: with the frontier back
    /// at A it runs before B's piece and sees B's new value on a second
    /// fold, not after the whole line has run once on what it had.
    #[test]
    fn an_edit_at_one_thread_runs_the_layer_at_its_frontier_not_after_the_text() {
        static ORDER: std::sync::Mutex<Vec<&'static str>> = std::sync::Mutex::new(Vec::new());
        struct Logged(&'static str);
        impl Operation for Logged {
            fn name(&self) -> &'static str {
                self.0
            }

            fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
                ORDER.lock().unwrap().push(self.0);
                let f = cx.values().field("vp");
                let x = cx.args()[0];
                cx.contribute(f, x);
                Ok(ValueId::UNIT)
            }
        }
        struct LoggedLayer;
        impl Operation for LoggedLayer {
            fn name(&self) -> &'static str {
                "layer"
            }

            fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
                ORDER.lock().unwrap().push("layer");
                let vp = cx.values().field("vp");
                let x = cx.fold(vp, None, None)?;
                cx.set_frontier(None);
                Ok(x)
            }

            fn contributes(&self, _: &[ValueId], _: &Values, _: Field) -> bool {
                false
            }

            fn settles_at_quiet(&self) -> bool {
                true
            }

            fn works_at_frontier(&self) -> bool {
                true
            }
        }
        let e = Engine::new();
        let (pa, pb) = (e.register(Arc::new(Logged("a"))), e.register(Arc::new(Logged("b"))));
        let layer = e.register(Arc::new(LoggedLayer));
        let v = e.values();
        let vp = v.field("vp");
        e.declare_accumulator(vp, Arc::new(SumMonoid));
        let u = e.new_unit();
        let a = e.push(u, pa, vec![v.int(1)]);
        let b = e.push(u, pb, vec![v.int(2)]);
        let l = e.push(u, layer, vec![]);
        e.evaluate(1).unwrap();
        ORDER.lock().unwrap().clear();
        // (B and the layer edited, both seeds, the frontier back at A: a
        // re-ask that answers what it answered stands without a run, so
        // the two are edited, and the layer runs)
        e.replace(b, pb, vec![v.int(5)]);
        e.replace(l, layer, vec![v.int(0)]);
        e.frontier.store(a.raw() + 1, Ordering::Release);
        e.evaluate(1).unwrap();
        // (the layer first, on B's old value; again once B has run)
        let order = ORDER.lock().unwrap().clone();
        assert_eq!(order, ["layer", "b", "layer"]);
        assert_eq!(e.output(l).and_then(|x| v.as_int(x)), Some(6));
    }
}
