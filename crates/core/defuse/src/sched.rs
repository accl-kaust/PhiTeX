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
    stealers: Vec<Stealer<NodeId>>,
    later_stealers: Vec<Stealer<NodeId>>,
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
    park: (Mutex<()>, Condvar),
    stop: AtomicBool,
    /// Workers running so far; the rest start when a deque holds more than
    /// its owner will run next (`want_spawn`), so a small evaluation spawns
    /// no thread.
    spawned: AtomicUsize,
    want_spawn: AtomicBool,
    n: usize,
    /// `Limits::final_reruns`.
    final_limit: u32,
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

impl Shared {
    fn wake_one(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        if self.idle.load(Ordering::SeqCst) > 0 {
            let _g = self.park.0.lock().unwrap_or_else(PoisonError::into_inner);
            self.park.1.notify_one();
        }
    }

    fn wake_all(&self) {
        self.wakes.fetch_add(1, Ordering::SeqCst);
        let _g = self.park.0.lock().unwrap_or_else(PoisonError::into_inner);
        self.park.1.notify_all();
    }

    /// One queue entry or running node less; the last one wakes everyone.
    fn done_one(&self) {
        if self.outstanding.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.wake_all();
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
    /// The commits' refuted predictions by field ([`crate::Report::predictions_refuted_by_field`]).
    pub(crate) pred_refuted_by_field: FxHashMap<Field, u64>,
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
    pub(crate) swept: u64,    /// The collections this worker made, with the time each held the
    /// workers (ns).
    pub(crate) collections: Vec<(crate::Collected, u64)>,
}

/// One worker's deques and counters (nanoseconds).
pub(crate) struct Worker {
    pub(crate) ix: usize,
    /// Nodes placed by its commits, and its runs to run again: next.
    local: Deque<NodeId>,
    /// Nodes its commits re-asked, in the order asked: after its placed
    /// ones, so a reader of many (a fold over the whole unit) runs behind
    /// the definers queued before it, not once per definer.
    later: Deque<NodeId>,
    /// Its block of the nodes dirty before the evaluation, in order, each
    /// with the run it was put in by (a seed, a steal).
    block: Deque<(NodeId, u32)>,
    /// The runs it has put in its block.
    block_runs: AtomicU32,
    shared: Arc<Shared>,
    pub(crate) runs: AtomicU64,
    pub(crate) nodes_run: AtomicU64,
    pub(crate) refuted_nodes: AtomicU64,
    /// Runs that panicked on inputs that moved during the run.
    pub(crate) stale_panics: AtomicU64,
    pub(crate) op_ns: AtomicU64,
    pub(crate) wasted_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) commit_max_ns: AtomicU64,
    /// The node of the longest commit (raw id).
    pub(crate) commit_max_node: AtomicU64,
    pub(crate) reask_ns: AtomicU64,
    pub(crate) idle_ns: AtomicU64,
    pub(crate) lock_wait_ns: AtomicU64,
    pub(crate) lock_wait_max_ns: AtomicU64,
    pub(crate) steals: AtomicU64,
    pub(crate) woken: AtomicU64,
    pub(crate) relative_resolved: AtomicU64,
    pub(crate) predictions_confirmed: AtomicU64,
    pub(crate) predictions_refuted: AtomicU64,
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
    fn add(c: &AtomicU64, x: u64) {
        c.fetch_add(x, Ordering::Relaxed);
    }

    fn max(c: &AtomicU64, x: u64) {
        c.fetch_max(x, Ordering::Relaxed);
    }

    /// Adds a walk's counts.
    pub(crate) fn note(&self, st: &CommitStats) {
        Worker::add(&self.woken, st.woken);
        Worker::add(&self.relative_resolved, st.relative_resolved);
        Worker::add(&self.predictions_confirmed, st.predictions_confirmed);
        Worker::add(&self.predictions_refuted, st.predictions_refuted);
        Worker::add(&self.shaped, st.shaped);
    }

    /// Seeds the block with nodes to run in this order.
    pub(crate) fn seed(&self, nodes: &[NodeId]) {
        let run = self.block_runs.fetch_add(1, Ordering::Relaxed);
        for &n in nodes.iter().rev() {
            self.block.push((n, run));
        }
        self.shared.outstanding.fetch_add(nodes.len(), Ordering::SeqCst);
    }
}

/// Starts `ws` on threads of scope `s`: the first's thread starts two
/// halves of the rest before it works; each gives its worker to `back`
/// when it is done.
fn spawn_tree<'s, 'e: 's>(
    s: &'s std::thread::Scope<'s, 'e>,
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
        run_worker(eng, pool, &w, None, None);
        back.lock().unwrap_or_else(PoisonError::into_inner).push(w);
    });
}

/// Spins before parking.
const SPINS: u32 = 64;
/// A block is stolen from only with this much left, half of it at a time
/// and at least `BLOCK_STEAL` nodes (an even run from its far end, run in
/// order): the tail is its owner's, no steal cuts a definer from the reader
/// right after it, and two thieves of a block seldom share a segment. A
/// reader at the head of a stolen run is re-asked once its owner's chain
/// reaches it (`commit::first_hole`), not once per definer before it.
const BLOCK_MIN: usize = 96;
const BLOCK_STEAL: usize = 32;
/// Edits are asked for this often from the caller's thread.
const ABSORB_EVERY: Duration = Duration::from_millis(2);

impl Pool {
    /// A pool of `n` workers and their deques.
    pub(crate) fn new(n: usize, epoch: u64, limits: Limits) -> (Pool, Vec<Worker>) {
        let n = n.max(1);
        let deques: Vec<Deque<NodeId>> = (0..n).map(|_| Deque::new_lifo()).collect();
        let blocks: Vec<Deque<(NodeId, u32)>> = (0..n).map(|_| Deque::new_lifo()).collect();
        let laters: Vec<Deque<NodeId>> = (0..n).map(|_| Deque::new_fifo()).collect();
        let stealers = deques.iter().map(Deque::stealer).collect();
        let later_stealers = laters.iter().map(Deque::stealer).collect();
        let block_stealers = blocks.iter().map(Deque::stealer).collect();
        let shared = Arc::new(Shared {
            injector: Injector::new(),
            passes: AtomicU64::new(0),
            runs_now: AtomicU64::new(0),
            stealers,
            later_stealers,
            block_stealers,
            block_busy: (0..n).map(|_| AtomicBool::new(false)).collect(),
            outstanding: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            wakes: AtomicU64::new(0),
            park: (Mutex::new(()), Condvar::new()),
            stop: AtomicBool::new(false),
            spawned: AtomicUsize::new(1),
            want_spawn: AtomicBool::new(false),
            n,
            final_limit: limits.final_reruns,
            collect_wanted: AtomicBool::new(false),
            deferred: Mx::new(Vec::new()),
            extra_roots: Mx::new(Vec::new()),
            panicked_roots: Mx::new(Vec::new()),
            collecting: AtomicBool::new(false),
            active: AtomicUsize::new(1),
            arrived: AtomicUsize::new(0),
            collect_gen: AtomicU64::new(0),
            collect_next: AtomicU64::new(limits.collect_at_bytes.unwrap_or(u64::MAX)),
        });
        let workers = deques
            .into_iter()
            .zip(blocks)
            .zip(laters)
            .enumerate()
            .map(|(ix, ((local, block), later))| Worker {
                ix,
                local,
                later,
                block,
                block_runs: AtomicU32::new(0),
                shared: shared.clone(),
                runs: AtomicU64::new(0),
                nodes_run: AtomicU64::new(0),
                refuted_nodes: AtomicU64::new(0),
                stale_panics: AtomicU64::new(0),
                op_ns: AtomicU64::new(0),
                wasted_ns: AtomicU64::new(0),
                commit_ns: AtomicU64::new(0),
                commit_max_ns: AtomicU64::new(0),
                commit_max_node: AtomicU64::new(0),
                reask_ns: AtomicU64::new(0),
                idle_ns: AtomicU64::new(0),
                lock_wait_ns: AtomicU64::new(0),
                lock_wait_max_ns: AtomicU64::new(0),
                steals: AtomicU64::new(0),
                woken: AtomicU64::new(0),
                relative_resolved: AtomicU64::new(0),
                predictions_confirmed: AtomicU64::new(0),
                predictions_refuted: AtomicU64::new(0),
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

    /// Runs until no node is dirty or running, on `n` scoped threads, the
    /// caller's thread one of them; the caller's thread also asks `src`
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
        std::thread::scope(|s| {
            // (spawned as a tree, each thread starting two before it
            // works: no thread waits for the others' starts, the caller's
            // least of all)
            spawn_tree(s, eng, self, &back, std::mem::take(&mut rest));
            let spawn_more = || {
                self.shared.want_spawn.store(false, Ordering::SeqCst);
                let more: Vec<Worker> = spare.lock().unwrap_or_else(PoisonError::into_inner).drain(..).collect();
                spawn_tree(s, eng, self, &back, more);
            };
            run_worker(eng, self, &first, src, Some(&spawn_more));
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
            while let Some(n) = w.local.pop() {
                left.push(n);
            }
            while let Some(n) = w.later.pop() {
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
        w.seed(&nodes);
        nodes.len()
    }

    fn find_work(&self, w: &Worker) -> Option<NodeId> {
        if let Some(n) = w.local.pop() {
            return Some(n);
        }
        if let Some(n) = w.later.pop() {
            return Some(n);
        }
        if let Some((n, _)) = w.block.pop() {
            return Some(n);
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
            loop {
                match self.shared.stealers[v].steal() {
                    Steal::Success(x) => {
                        Worker::add(&w.steals, 1);
                        return Some(x);
                    }
                    Steal::Empty => break,
                    Steal::Retry => {}
                }
            }
        }
        for k in 1..n {
            let v = (w.ix + k) % n;
            loop {
                match self.shared.later_stealers[v].steal() {
                    Steal::Success(x) => {
                        Worker::add(&w.steals, 1);
                        return Some(x);
                    }
                    Steal::Empty => break,
                    Steal::Retry => {}
                }
            }
        }
        for k in 1..n {
            let v = (w.ix + k) % n;
            let st = &self.shared.block_stealers[v];
            if st.len() < BLOCK_MIN {
                continue;
            }
            let busy = &self.shared.block_busy[v];
            if busy.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() {
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
        let (m, cv) = &sh.park;
        let mut g = m.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if sh.collect_gen.load(Ordering::SeqCst) != seen || sh.stop.load(Ordering::SeqCst) {
                break;
            }
            if sh.arrived.load(Ordering::SeqCst) >= sh.active.load(Ordering::SeqCst) && !sh.collecting.swap(true, Ordering::SeqCst) {
                drop(g);
                let t = Instant::now();
                let mut extra = std::mem::take(&mut *sh.extra_roots.lock());
                extra.extend(sh.panicked_roots.lock().iter().copied());
                let floor = self.limits.collect_at_bytes.unwrap_or(u64::MAX);
                if let Some(c) = eng.collect_at_barrier(extra) {
                    let held = ns(t);
                    sh.collect_next.store(floor.max(c.bytes_after.saturating_mul(2)), Ordering::SeqCst);
                    w.log.lock().collections.push((c, held));
                } else {
                    sh.collect_next.store(floor.max(eng.values().payload_bytes().saturating_mul(2)), Ordering::SeqCst);
                }
                sh.arrived.store(0, Ordering::SeqCst);
                sh.collect_wanted.store(false, Ordering::SeqCst);
                sh.collecting.store(false, Ordering::SeqCst);
                sh.collect_gen.fetch_add(1, Ordering::SeqCst);
                sh.wake_all();
                return;
            }
            g = cv.wait_timeout(g, Duration::from_millis(2)).unwrap_or_else(PoisonError::into_inner).0;
        }
        drop(g);
        // (stopped before the collection: no longer waited for)
        if sh.collect_gen.load(Ordering::SeqCst) == seen {
            sh.arrived.fetch_sub(1, Ordering::SeqCst);
        }
    }

    fn idle_wait(&self, eng: &Engine, w: &Worker) -> Option<NodeId> {
        let (m, cv) = &self.shared.park;
        self.shared.idle.fetch_add(1, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        let t_idle = Instant::now();
        let r = loop {
            stall_dump(eng, &self.shared, t_idle);
            if self.shared.stop.load(Ordering::SeqCst) || self.shared.collect_wanted.load(Ordering::SeqCst) {
                break None;
            }
            // (looked for outside the park's lock: a steal is no reason to
            // keep another idle worker or a waker out)
            let seen = self.shared.wakes.load(Ordering::SeqCst);
            if let Some(n) = self.find_work(w) {
                break Some(n);
            }
            if self.shared.outstanding.load(Ordering::SeqCst) == 0 {
                break None;
            }
            if w.ix == 0 && self.shared.want_spawn.load(Ordering::SeqCst) {
                break None;
            }
            let g = m.lock().unwrap_or_else(PoisonError::into_inner);
            if self.shared.stop.load(Ordering::SeqCst)
                || self.shared.outstanding.load(Ordering::SeqCst) == 0
                || self.shared.collect_wanted.load(Ordering::SeqCst)
            {
                break None;
            }
            // (a push between the search and the lock notified no one
            // yet waiting: its work is looked for again before resting)
            if self.shared.wakes.load(Ordering::SeqCst) != seen {
                continue;
            }
            // (the timeout only guards a lost wakeup; a short one would
            // have idle workers preempting the busy ones every few
            // microseconds)
            drop(cv.wait_timeout(g, Duration::from_millis(2)).unwrap_or_else(PoisonError::into_inner));
        };
        self.shared.idle.fetch_sub(1, Ordering::SeqCst);
        r
    }
}

fn run_worker(
    eng: &Engine,
    pool: &Pool,
    w: &Worker,
    mut src: Option<&mut dyn EditSource>,
    spawn_more: Option<&dyn Fn()>,
) {
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
        if let Some(sp) = spawn_more
            && pool.shared.want_spawn.load(Ordering::SeqCst)
        {
            sp();
        }
        if let Some(n) = pool.find_work(w) {
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
            if let Some(n) = pool.find_work(w) {
                found = Some(n);
                break;
            }
            std::hint::spin_loop();
        }
        if found.is_none() && pool.outstanding() > 0 {
            found = pool.idle_wait(eng, w);
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
            continue;
        }
        // quiescent: the nodes that run only then (`Operation::final_only`)
        // released, and once they are done, the loop-back values
        // published, their readers whose answer moved run again (a pass)
        if w.ix == 0 {
            let rel: Vec<NodeId> = std::mem::take(&mut *pool.shared.deferred.lock())
                .into_iter()
                .filter(|&n| eng.placed_node(n).is_some_and(|x| x.state() == State::Dirty))
                .collect();
            if !rel.is_empty() {
                w.seed(&rel);
                pool.shared.wake_all();
                continue;
            }
            let (k, pass) = crate::commit::publish_finals(eng, w);
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
        let r = pool.idle_wait(eng, w);
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
    w.log.lock().absorbed.push((v, pool.shared.runs_now.load(Ordering::SeqCst)));
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
    let created = matches!(wake.cause, Cause::Created { .. });
    *n.woke.lock() = Some(wake);
    match w {
        Some(w) if defer(eng, &w.shared, n.id) => {}
        Some(w) => {
            w.shared.outstanding.fetch_add(1, Ordering::SeqCst);
            if created {
                w.local.push(n.id);
            } else {
                w.later.push(n.id);
            }
            let sh = &w.shared;
            if sh.spawned.load(Ordering::Relaxed) < sh.n && w.local.len() >= 2 {
                sh.want_spawn.store(true, Ordering::SeqCst);
            }
            sh.wake_one();
        }
        None => eng.pend(n.id),
    }
}

/// A walk's re-ask: marks a clean node dirty and queues it. A running node
/// is left alone until its commit: what it reads is checked again there,
/// after the definition the walk is for was filed, so it needs no mark; one
/// here would run it again on a read it may well have made right. Once its
/// commit has checked (`Node::committing`), it is marked as `mark_dirty`
/// marks it.
pub(crate) fn mark_stale(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
    if n.committing.load(Ordering::Acquire) {
        return mark_dirty(eng, w, n, wake);
    }
    if n.state() == State::Clean && n.cas_state(State::Clean, State::Dirty) {
        log_wake(eng, w, n, &wake);
        enqueue(eng, w, n, wake);
        return true;
    }
    if n.state() == State::Dirty {
        merge_woke(eng, n, wake);
    }
    false
}

/// Marks a node dirty for `wake`'s reason and queues it, unless it is
/// already dirty, inert or gone. A running node becomes `RunningDirty`: its
/// reads return `Stop` from now, its commit goes through, and it is queued
/// after. Returns whether it was newly marked.
pub(crate) fn mark_dirty(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) -> bool {
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
    fn site(c: &Cause) -> Option<NodeId> {
        match c {
            Cause::Changed { site, .. } => *site,
            Cause::Range { by, .. } => Some(*by),
            Cause::Output { of } => Some(*of),
            Cause::Rebind { from, to, .. } => from.or(*to),
            Cause::Environment { .. } | Cause::Structure | Cause::Created { .. } | Cause::Edited => None,
        }
    }
    let mut w = n.woke.lock();
    let keep = match (&*w, &wake.cause) {
        (_, Cause::Edited) | (None, _) => false,
        (Some(o), _) if matches!(o.cause, Cause::Edited | Cause::Created { .. }) => true,
        (Some(o), c) => {
            let at = |s: Option<NodeId>| s.and_then(|s| eng.placed_node(s)).map_or(crate::order::Pos(0), |x| x.pos_hint());
            at(site(&o.cause)) <= at(site(c))
        }
    };
    if !keep {
        *w = Some(wake);
    }
}

/// (`PHITEX_STALL_SECS=n`, profile) Every worker idle for `n` seconds
/// with nodes outstanding: the evaluation is stuck (a count not released,
/// a wake lost); the outstanding count and every node not clean, inert
/// or gone are printed once, with their states.
fn stall_dump(eng: &Engine, shared: &Shared, t_idle: Instant) {
    static SECS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static DONE: AtomicBool = AtomicBool::new(false);
    let secs = *SECS.get_or_init(|| std::env::var("PHITEX_STALL_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(0));
    if secs == 0 || !crate::obs::enabled() || t_idle.elapsed().as_secs() < secs || DONE.load(Ordering::Relaxed) {
        return;
    }
    let idle = shared.idle.load(Ordering::SeqCst);
    let spawned = shared.spawned.load(Ordering::SeqCst);
    if idle < spawned.max(1) {
        return;
    }
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    let mut out = format!(
        "stall: {} outstanding, {idle} of {spawned} workers idle for {secs} s; nodes not clean:\n",
        shared.outstanding.load(Ordering::SeqCst)
    );
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
            out.push_str(&format!("  node {} {op} {st:?} pos {:?} committing {}\n", nid.raw(), pos.0, n.committing.load(Ordering::Acquire)));
        }
    }
    eprint!("{out}");
}

/// (`PHITEX_WAKE_LOG`, profile) Whether every wake of a committed node is
/// printed: the node, the field and the cause, as the re-ask happens.
fn wake_log() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *F.get_or_init(|| std::env::var_os("PHITEX_WAKE_LOG").is_some_and(|v| !v.is_empty() && v != "0"))
}

fn log_wake(eng: &Engine, w: Option<&Worker>, n: &Node, wake: &Wake) {
    if !n.committed() {
        return;
    }
    let field = match &wake.cause {
        Cause::Rebind { field, .. } | Cause::Changed { field, .. } | Cause::Range { field, .. } | Cause::Environment { field } => Some(*field),
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
        eprintln!(
            "wake: node {} op {:?} pos {:?} field {name} {:?}{site_pos}\twake\t{}\t{}\t{name}\t{kind}\t{site_op}\t{}",
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
fn work_caught(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
    // (the whole task, run, commit and re-asks: no collection meanwhile)
    let _task = eng.values().task();
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

fn work(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
    let node = eng.node(n);
    if !node.cas_state(State::Dirty, State::Running) {
        pool.shared.done_one();
        return Ok(());
    }
    let first = node.run_epoch.swap(pool.epoch, Ordering::AcqRel) != pool.epoch;
    if first {
        node.runs.store(0, Ordering::Relaxed);
        Worker::add(&w.nodes_run, 1);
    } else {
        Worker::add(&w.wasted_ns, node.run_ns.load(Ordering::Relaxed));
    }
    let runs = node.runs.fetch_add(1, Ordering::Relaxed) + 1;
    if runs == 2 {
        Worker::add(&w.refuted_nodes, 1);
    }
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
    let mut cx = Ctx::new(eng, Some(w), node);
    let r = catch_unwind(AssertUnwindSafe(|| op.run(&mut cx)));
    let run_ns = ns(t);
    node.run_ns.store(run_ns, Ordering::Relaxed);
    Worker::add(&w.op_ns, run_ns);
    Worker::add(&w.runs, 1);
    w.shared.runs_now.fetch_add(1, Ordering::Relaxed);
    {
        let mut log = w.log.lock();
        *log.runs_by_op.entry(node.op()).or_default() += 1;
        if crate::obs::enabled() {
            log.run_times.push((n, Duration::from_nanos(run_ns)));
        }
    }
    match r {
        // (a run that read what has moved since, or one re-asked or taken
        // meanwhile, panicked on inputs no longer current: run again)
        Err(_)
            if node.state() != State::Running
                || cx.product.reads.iter().any(|rd| !crate::read::holds(eng, node, rd, &mut crate::read::FoldMemo::new())) =>
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
            give_back(eng, node);
            pool.shared.panicked_roots.lock().extend(reads.iter().filter_map(|r| r.seen));
            w.log.lock().panicked.push((n, reads, op.name(), message));
            pool.shared.done_one();
            Ok(())
        }
        Ok(Err(_stop)) => {
            drop(cx);
            end_run(eng, pool, w, node, State::Dirty);
            Ok(())
        }
        Ok(Ok(out)) => {
            let product = cx.into_product(out);
            let st = commit(eng, w, node, product);
            // (the store past the collection's mark: the workers called to
            // its barrier as their runs end)
            let sh = &pool.shared;
            let next = sh.collect_next.load(Ordering::Relaxed);
            if next != u64::MAX && eng.values().payload_bytes() > next && !sh.collect_wanted.swap(true, Ordering::SeqCst) {
                sh.wake_all();
            }
            Worker::add(&w.commits, 1);
            Worker::add(&w.commit_ns, st.commit_ns);
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
        if to == State::Dirty && !defer(eng, &pool.shared, node.id) {
            w.local.push(node.id);
            w.shared.wake_one();
        } else {
            pool.shared.done_one();
        }
        return to == State::Clean;
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
