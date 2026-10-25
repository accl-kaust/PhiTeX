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
use crate::values::{Field, FxHashMap, FxHashSet};

/// What every worker and the pool share.
pub(crate) struct Shared {
    injector: Injector<NodeId>,
    stealers: Vec<Stealer<NodeId>>,
    later_stealers: Vec<Stealer<NodeId>>,
    block_stealers: Vec<Stealer<(NodeId, u32)>>,
    /// One thief at a time per block: a steal takes an even run of nodes
    /// from its far end, whole.
    block_busy: Vec<AtomicBool>,
    /// Queue entries plus running nodes; zero ends the evaluation.
    outstanding: AtomicUsize,
    idle: AtomicUsize,
    park: (Mutex<()>, Condvar),
    stop: AtomicBool,
    /// Workers running so far; the rest start when a deque holds more than
    /// its owner will run next (`want_spawn`), so a small evaluation spawns
    /// no thread.
    spawned: AtomicUsize,
    want_spawn: AtomicBool,
    n: usize,
}

impl Shared {
    fn wake_one(&self) {
        fence(Ordering::SeqCst);
        if self.idle.load(Ordering::SeqCst) > 0 {
            let _g = self.park.0.lock().unwrap_or_else(PoisonError::into_inner);
            self.park.1.notify_one();
        }
    }

    fn wake_all(&self) {
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
    pub(crate) causes: Vec<(NodeId, Cause)>,
    pub(crate) refuted_by_field: FxHashMap<Field, u64>,
    pub(crate) run_times: Vec<(NodeId, Duration)>,
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
    pub(crate) op_ns: AtomicU64,
    pub(crate) wasted_ns: AtomicU64,
    pub(crate) commit_ns: AtomicU64,
    pub(crate) commit_max_ns: AtomicU64,
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
    /// Its last commit and when (`Engine::critical_chain`).
    pub(crate) last_commit: AtomicU64,
    pub(crate) last_done: AtomicU64,
    /// The engine's stable fields, taken once for the evaluation.
    pub(crate) stable: Arc<FxHashSet<Field>>,
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
    pub(crate) fn new(n: usize, epoch: u64, limits: Limits, stable: Arc<FxHashSet<Field>>) -> (Pool, Vec<Worker>) {
        let n = n.max(1);
        let deques: Vec<Deque<NodeId>> = (0..n).map(|_| Deque::new_lifo()).collect();
        let blocks: Vec<Deque<(NodeId, u32)>> = (0..n).map(|_| Deque::new_lifo()).collect();
        let laters: Vec<Deque<NodeId>> = (0..n).map(|_| Deque::new_fifo()).collect();
        let stealers = deques.iter().map(Deque::stealer).collect();
        let later_stealers = laters.iter().map(Deque::stealer).collect();
        let block_stealers = blocks.iter().map(Deque::stealer).collect();
        let shared = Arc::new(Shared {
            injector: Injector::new(),
            stealers,
            later_stealers,
            block_stealers,
            block_busy: (0..n).map(|_| AtomicBool::new(false)).collect(),
            outstanding: AtomicUsize::new(0),
            idle: AtomicUsize::new(0),
            park: (Mutex::new(()), Condvar::new()),
            stop: AtomicBool::new(false),
            spawned: AtomicUsize::new(1),
            want_spawn: AtomicBool::new(false),
            n,
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
                op_ns: AtomicU64::new(0),
                wasted_ns: AtomicU64::new(0),
                commit_ns: AtomicU64::new(0),
                commit_max_ns: AtomicU64::new(0),
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
                last_commit: AtomicU64::new(u64::MAX),
                last_done: AtomicU64::new(0),
                stable: stable.clone(),
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
            && let Err(e) = work(eng, self, &workers[0], n)
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
            Some(e) => Err(e),
            None => Ok(workers),
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
    fn drain_pending(&self, eng: &Engine, w: &Worker) -> usize {
        let nodes = eng.take_pending(w);
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
    fn idle_wait(&self, w: &Worker) -> Option<NodeId> {
        let (m, cv) = &self.shared.park;
        let mut g = m.lock().unwrap_or_else(PoisonError::into_inner);
        self.shared.idle.fetch_add(1, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        let r = loop {
            if self.shared.stop.load(Ordering::SeqCst) {
                break None;
            }
            if let Some(n) = self.find_work(w) {
                break Some(n);
            }
            if self.shared.outstanding.load(Ordering::SeqCst) == 0 {
                break None;
            }
            if w.ix == 0 && self.shared.want_spawn.load(Ordering::SeqCst) {
                break None;
            }
            // (the timeout only guards a lost wakeup; a short one would
            // have idle workers preempting the busy ones every few
            // microseconds)
            let (gg, _) = cv
                .wait_timeout(g, Duration::from_millis(2))
                .unwrap_or_else(PoisonError::into_inner);
            g = gg;
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
        if let Some(sp) = spawn_more
            && pool.shared.want_spawn.load(Ordering::SeqCst)
        {
            sp();
        }
        if let Some(n) = pool.find_work(w) {
            if let Err(e) = work(eng, pool, w, n) {
                pool.fail(e);
                break;
            }
            if let Some(s) = src.as_deref_mut()
                && last_absorb.elapsed() >= ABSORB_EVERY
            {
                last_absorb = Instant::now();
                if s.absorb(eng).is_some() {
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
            found = pool.idle_wait(w);
        }
        Worker::add(&w.idle_ns, ns(t));
        if let Some(n) = found {
            if let Err(e) = work(eng, pool, w, n) {
                pool.fail(e);
                break;
            }
            continue;
        }
        if pool.outstanding() > 0 {
            continue;
        }
        // quiescent
        if w.ix == 0 {
            if let Some(s) = src.as_deref_mut() {
                let items: Vec<Settled> = eng.take_settled();
                if !items.is_empty() && s.settled(eng, &items) {
                    pool.shared.stop.store(true, Ordering::SeqCst);
                    pool.shared.wake_all();
                    break;
                }
                last_absorb = Instant::now();
                if s.absorb(eng).is_some() && pool.drain_pending(eng, w) > 0 {
                    continue;
                }
            }
            pool.shared.stop.store(true, Ordering::SeqCst);
            pool.shared.wake_all();
            break;
        }
        let t = Instant::now();
        let r = pool.idle_wait(w);
        Worker::add(&w.idle_ns, ns(t));
        if let Some(n) = r
            && let Err(e) = work(eng, pool, w, n)
        {
            pool.fail(e);
            break;
        }
    }
    let (sum, max) = take_wait();
    Worker::add(&w.lock_wait_ns, sum);
    Worker::max(&w.lock_wait_max_ns, max);
}

/// Queues a node made dirty (a new node, a released sibling): on `w`'s
/// deque, or the engine's pending list when no evaluation runs.
pub(crate) fn enqueue(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, wake: Wake) {
    let created = matches!(wake.cause, Cause::Created { .. });
    *n.woke.lock() = Some(wake);
    match w {
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
            State::Dirty | State::RunningDirty | State::Inert | State::RunningInert | State::Gone => return false,
        }
    }
}

fn log_wake(eng: &Engine, w: Option<&Worker>, n: &Node, wake: &Wake) {
    if !n.committed() {
        return;
    }
    let field = match &wake.cause {
        Cause::Rebind { field, .. } | Cause::Changed { field, .. } | Cause::Range { field, .. } | Cause::Environment { field } => Some(*field),
        Cause::Output { .. } | Cause::Structure | Cause::Created { .. } | Cause::Edited => None,
    };
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
pub(crate) fn work(eng: &Engine, pool: &Pool, w: &Worker, n: NodeId) -> Result<(), EvalError> {
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
        return Err(EvalError::RunawayNode {
            node: n,
            op: op.name(),
            runs,
        });
    }
    if let Some(t) = pool.limits.total_runs {
        let total = pool.total_runs.fetch_add(1, Ordering::Relaxed) + 1;
        if total > t {
            return Err(EvalError::TotalRuns { runs: total });
        }
    }
    if pool.limits.time.is_some_and(|t| pool.t0.elapsed() > t) {
        return Err(EvalError::Deadline);
    }
    let t = Instant::now();
    let mut cx = Ctx::new(eng, Some(w), node);
    let r = catch_unwind(AssertUnwindSafe(|| op.run(&mut cx)));
    let run_ns = ns(t);
    node.run_ns.store(run_ns, Ordering::Relaxed);
    Worker::add(&w.op_ns, run_ns);
    Worker::add(&w.runs, 1);
    {
        let mut log = w.log.lock();
        *log.runs_by_op.entry(node.op()).or_default() += 1;
        if crate::obs::enabled() {
            log.run_times.push((n, Duration::from_nanos(run_ns)));
        }
    }
    match r {
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_owned())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic".to_owned());
            Err(EvalError::OpPanicked {
                node: n,
                op: op.name(),
                message,
            })
        }
        Ok(Err(_stop)) => {
            drop(cx);
            end_run(pool, w, node, State::Dirty);
            Ok(())
        }
        Ok(Ok(out)) => {
            let product = cx.into_product(out);
            let st = commit(eng, w, node, product);
            Worker::add(&w.commits, 1);
            Worker::add(&w.commit_ns, st.commit_ns);
            Worker::max(&w.commit_max_ns, st.commit_ns);
            Worker::add(&w.reask_ns, st.reask_ns);
            w.note(&st);
            node.commit_ns.store(st.commit_ns, Ordering::Relaxed);
            node.reask_ns.store(st.reask_ns, Ordering::Relaxed);
            let done = ns(pool.t0);
            node.done_ns.store(done, Ordering::Relaxed);
            w.last_commit.store(n.raw(), Ordering::Relaxed);
            w.last_done.store(done, Ordering::Relaxed);
            if end_run(pool, w, node, State::Clean) {
                eng.note_settled(node, out);
            }
            Ok(())
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
fn end_run(pool: &Pool, w: &Worker, node: &Node, done: State) -> bool {
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
        if to == State::Dirty {
            w.local.push(node.id);
            w.shared.wake_one();
        } else {
            pool.shared.done_one();
        }
        return to == State::Clean;
    }
}
