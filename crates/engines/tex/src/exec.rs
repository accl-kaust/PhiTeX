//! The engine's executor: a batch of runs, and the engine's parallel
//! passes, on up to `threads` threads. The schedule changes how fast,
//! never what: every result lands in its index's slot and is read back
//! in index order, so any thread count, 1 included, gives the same
//! output.
//!
//! One impl of `Executor`, pluggable by target:
//! - native, feature `rayon` (default): a persistent rayon pool per
//!   thread count;
//! - native without it: workers scoped to each batch (`std::thread::scope`);
//! - wasm32 with `atomics` (shared memory): the same rayon pool, its
//!   workers spawned by the host through the callback given to
//!   [`set_spawn_worker`] (a web worker each, as wasm-bindgen-rayon
//!   does); the host adapts its own bindings, phitex-tex has none;
//! - wasm32 without `atomics`, or one thread, or no host callback on
//!   wasm32: in order on the calling thread.
//!
//! Which thread: the evaluation (`Build::with`, `Build::edit`) blocks in
//! `for_each` until a batch has run, so on wasm32 it must run in a
//! worker, never on the browser's main thread (which may not block).
//! Natively any thread may run it. The thread count is
//! `Options::threads`, else `PHITEX_THREADS`, else the available
//! parallelism; on wasm32 the host registers its worker callback before
//! the first build (without it the build runs in order). Workspace
//! rule: no `unsafe`; borrowed closures reach the workers through
//! rayon's scoped API or `std::thread::scope`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use phitex_defuse::{Executor, RunResult};

/// Whether the environment variable `$name` is set, read once per
/// process (a `getenv` takes the environment's lock and scans it; the
/// debug switches are tested on every run).
macro_rules! env_flag {
    ($name:literal) => {{
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *F.get_or_init(|| std::env::var_os($name).is_some())
    }};
}
pub(crate) use env_flag;

/// Below this many items an engine pass runs on the calling thread
/// (its items cost microseconds).
const GRAIN: usize = 64;

/// Below this many jobs a batch of runs goes inline. A run is a
/// paragraph region, a page step or a format (hundreds of microseconds
/// to seconds), so any batch of two or more is worth a hand-off to the
/// pool. `PHITEX_GRAIN` overrides it (for measurements; the schedule
/// never changes the output).
fn job_grain() -> usize {
    static G: OnceLock<usize> = OnceLock::new();
    *G.get_or_init(|| {
        std::env::var("PHITEX_GRAIN")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2)
    })
}

/// How the host starts a worker thread: it runs the closure on a new
/// worker (on wasm32: a web worker sharing the module's memory).
pub type SpawnWorker = Box<dyn Fn(Box<dyn FnOnce() + Send>) + Send + Sync>;

static SPAWN: OnceLock<SpawnWorker> = OnceLock::new();

/// Gives the executor the host's way to start workers (required on
/// wasm32 for threads; natively optional, std threads otherwise). Only
/// the first call counts.
pub fn set_spawn_worker(f: SpawnWorker) {
    let _ = SPAWN.set(f);
}

/// A clock for the executor's counters: nanoseconds from some fixed
/// point, `None` where the target has none (wasm32: the host's clock is
/// not reachable from here).
pub(crate) trait Clock: Sync {
    fn now(&self) -> Option<u64>;
}

/// `std::time::Instant` from the first reading.
struct StdClock;

impl Clock for StdClock {
    fn now(&self) -> Option<u64> {
        static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
        if cfg!(target_arch = "wasm32") {
            return None;
        }
        let e = *EPOCH.get_or_init(std::time::Instant::now);
        u64::try_from(e.elapsed().as_nanos()).ok()
    }
}

/// The executor's clock.
pub(crate) fn clock() -> &'static dyn Clock {
    &StdClock
}

/// What a build runs now, readable from another thread while it runs
/// (`Build::progress`): one slot per worker holding the job it runs
/// ([`Progress::pack`]: the operation and the node's byte offset in the
/// document; 0 when idle), the jobs finished in this evaluation and
/// the jobs known so far (growing as nodes unfold). Workers only store
/// into atomics (relaxed), once at a job's start and once at its end.
#[derive(Debug)]
pub struct Progress {
    slots: Box<[AtomicU64]>,
    done: AtomicU64,
    total: AtomicU64,
}

impl Progress {
    /// The offset of a node not in the document's own text (one a run
    /// unfolded).
    pub const INSIDE: u64 = (1 << 48) - 1;

    /// Slots for `workers` workers and the evaluating thread (a reader
    /// can make it before the build: [`crate::Options::progress`]).
    #[must_use]
    pub fn new(workers: usize) -> Progress {
        Progress {
            slots: (0..=workers).map(|_| AtomicU64::new(0)).collect(),
            done: AtomicU64::new(0),
            total: AtomicU64::new(0),
        }
    }

    /// A slot's value: `op` (1 a text node, 2 a box put, 3 an output
    /// page, 0 another) in the top 16 bits, the byte offset below.
    #[must_use]
    pub fn pack(op: u16, offset: u64) -> u64 {
        (u64::from(op) + 1) << 48 | offset.min(Self::INSIDE)
    }

    /// A slot's value as `(op, offset)`; `None` when idle.
    #[must_use]
    pub fn unpack(x: u64) -> Option<(u16, u64)> {
        let op = u16::try_from(x >> 48).ok()?.checked_sub(1)?;
        Some((op, x & Self::INSIDE))
    }

    /// What each worker runs now.
    #[must_use]
    pub fn running(&self) -> Vec<Option<(u16, u64)>> {
        self.slots
            .iter()
            .map(|s| Self::unpack(s.load(Ordering::Relaxed)))
            .collect()
    }

    /// The jobs finished in this evaluation.
    #[must_use]
    pub fn done(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }

    /// The jobs known so far in this evaluation.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.total.load(Ordering::Relaxed)
    }
}

/// The progress of a build and its text nodes' offsets, by node.
#[derive(Clone)]
pub(crate) struct Watch {
    pub progress: Arc<Progress>,
    pub offsets: Arc<HashMap<u64, u64>>,
}

impl Watch {
    pub(crate) fn new(workers: usize, given: Option<Arc<Progress>>) -> Watch {
        Watch {
            progress: given.unwrap_or_else(|| Arc::new(Progress::new(workers))),
            offsets: Arc::new(HashMap::new()),
        }
    }
}

thread_local! {
    /// The slot of the job this thread runs, while it runs one.
    static CURRENT: RefCell<Option<(Watch, usize)>> = const { RefCell::new(None) };
    /// This thread's worker number, for its slot.
    static WORKER: usize = {
        static NEXT: AtomicUsize = AtomicUsize::new(1);
        NEXT.fetch_add(1, Ordering::Relaxed)
    };
}

/// The slot of the calling thread: 0 for the evaluating thread, a
/// worker's own otherwise.
fn slot(n: usize) -> usize {
    #[cfg(feature = "rayon")]
    if let Some(i) = rayon::current_thread_index() {
        return (i + 1) % n;
    }
    if std::thread::current()
        .name()
        .is_some_and(|t| t.starts_with("phitex-"))
    {
        WORKER.with(|w| *w % n)
    } else {
        0
    }
}

/// A run begins: `op` on `node` (the job's slot says so).
pub(crate) fn note(op: u16, node: u64) {
    CURRENT.with(|c| {
        if let Some((w, i)) = &*c.borrow() {
            let off = w.offsets.get(&node).copied().unwrap_or(Progress::INSIDE);
            w.progress.slots[*i].store(Progress::pack(op, off), Ordering::Relaxed);
        }
    });
}

/// The executor of an evaluation, with its counters: the time its jobs
/// ran (summed over jobs), and the most jobs running at once.
pub(crate) struct Threads {
    pub n: usize,
    watch: Option<Watch>,
    busy: AtomicU64,
    running: AtomicUsize,
    peak: AtomicUsize,
    batches: AtomicU64,
    batches_inline: AtomicU64,
    jobs_max_batch: AtomicUsize,
}

impl Threads {
    pub(crate) fn new(n: usize) -> Threads {
        Threads {
            n,
            watch: None,
            busy: AtomicU64::new(0),
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            batches: AtomicU64::new(0),
            batches_inline: AtomicU64::new(0),
            jobs_max_batch: AtomicUsize::new(0),
        }
    }

    /// The executor reporting to `w`; its counters begin again.
    pub(crate) fn watched(mut self, w: &Watch) -> Threads {
        w.progress.done.store(0, Ordering::Relaxed);
        w.progress.total.store(0, Ordering::Relaxed);
        self.watch = Some(w.clone());
        self
    }

    /// One node's run, its slot set around it.
    fn node_job<R>(&self, f: impl FnOnce() -> R) -> R {
        let Some(w) = &self.watch else {
            return self.job(f);
        };
        let i = slot(w.progress.slots.len());
        let prev = CURRENT.with(|c| c.borrow_mut().replace((w.clone(), i)));
        let r = self.job(f);
        CURRENT.with(|c| *c.borrow_mut() = prev);
        // the slot is clear before the job counts as done
        w.progress.slots[i].store(0, Ordering::Relaxed);
        w.progress.done.fetch_add(1, Ordering::Relaxed);
        r
    }

    /// The jobs' summed run time (zero without a clock).
    pub(crate) fn busy(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.busy.load(Ordering::Relaxed))
    }

    /// The batches of runs, those run inline, and the largest batch.
    pub(crate) fn batches(&self) -> (u64, u64, usize) {
        (
            self.batches.load(Ordering::Relaxed),
            self.batches_inline.load(Ordering::Relaxed),
            self.jobs_max_batch.load(Ordering::Relaxed),
        )
    }

    /// The most jobs that ran at once.
    pub(crate) fn peak(&self) -> usize {
        self.peak.load(Ordering::Relaxed)
    }

    /// Runs one job, counted.
    fn job<R>(&self, f: impl FnOnce() -> R) -> R {
        let c = self.running.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak.fetch_max(c, Ordering::Relaxed);
        let t0 = clock().now();
        let r = f();
        if let (Some(a), Some(b)) = (t0, clock().now()) {
            self.busy.fetch_add(b.saturating_sub(a), Ordering::Relaxed);
        }
        self.running.fetch_sub(1, Ordering::Relaxed);
        r
    }
}

/// The thread count: `given`, else `PHITEX_THREADS`, else the available
/// parallelism; 1 where this target cannot run threads.
pub(crate) fn threads(given: Option<usize>) -> usize {
    if !threads_possible() {
        return 1;
    }
    given.filter(|&n| n > 0).unwrap_or_else(|| {
        std::env::var("PHITEX_THREADS")
            .ok()
            .and_then(|s| s.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
    })
}

/// Whether this target can run workers at all.
fn threads_possible() -> bool {
    if cfg!(target_arch = "wasm32") {
        cfg!(target_feature = "atomics") && cfg!(feature = "rayon") && SPAWN.get().is_some()
    } else {
        true
    }
}

/// After an evaluation, with `PHITEX_OBS` set and the `profile` feature
/// on: the engine's probes (spans and counters) printed, then reset.
impl Drop for Threads {
    fn drop(&mut self) {
        if phitex_defuse::obs::enabled() && env_flag!("PHITEX_OBS") {
            eprint!(
                "probes (evaluation):\n{}{}",
                phitex_defuse::obs::snapshot(),
                crate::prof::take()
            );
            phitex_defuse::obs::reset();
        }
    }
}

impl Executor for Threads {
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> RunResult + Sync),
        out: &mut Vec<RunResult>,
    ) {
        if let Some(w) = &self.watch {
            w.progress
                .total
                .fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
        }
        self.batches.fetch_add(1, Ordering::Relaxed);
        self.jobs_max_batch.fetch_max(n, Ordering::Relaxed);
        if self.n <= 1 || n < job_grain() || !threads_possible() {
            self.batches_inline.fetch_add(1, Ordering::Relaxed);
            out.extend((0..n).map(|i| self.node_job(|| run(i))));
            phitex_defuse::obs::flush();
            return;
        }
        let slots: Vec<OnceLock<RunResult>> = (0..n).map(|_| OnceLock::new()).collect();
        trace(n, self.n);
        // one job a task: each costs a run
        parallel(self.n, n, 1, &|i| {
            let _ = slots[i].set(self.node_job(|| run(i)));
        });
        out.extend(
            slots
                .into_iter()
                .map(|s| s.into_inner().expect("every job ran")),
        );
    }

    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        self.parallel_each(n, &|i| self.job(|| f(i)));
    }
}

impl Threads {
    /// `f(i)` for every `i < n`, on the pool when the batch is large
    /// enough.
    fn parallel_each(&self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if self.n <= 1 || n < GRAIN || !threads_possible() {
            (0..n).for_each(f);
            phitex_defuse::obs::flush();
            return;
        }
        let grain = (n / (self.n * 8)).clamp(4, GRAIN);
        trace(n, self.n);
        parallel(self.n, n, grain, f);
    }
}

fn trace(n: usize, threads: usize) {
    if env_flag!("PHITEX_EXEC_TRACE") {
        eprintln!("exec: {n} jobs on {threads} threads");
    }
}

#[cfg(feature = "rayon")]
fn parallel(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    use rayon::prelude::*;
    let pool = pool(threads);
    pool.install(|| {
        (0..n).into_par_iter().with_min_len(grain).for_each(f);
    });
    // every worker adds its probe counts and spans to the totals (a
    // no-op without the `profile` feature, so the broadcast is skipped)
    if cfg!(feature = "profile") {
        pool.broadcast(|_| phitex_defuse::obs::flush());
        phitex_defuse::obs::flush();
    }
}

/// The persistent pool of `threads` workers.
#[cfg(feature = "rayon")]
fn pool(threads: usize) -> std::sync::Arc<rayon::ThreadPool> {
    use std::sync::{Arc, Mutex, PoisonError};
    type Pools = Mutex<std::collections::HashMap<usize, Arc<rayon::ThreadPool>>>;
    static POOLS: OnceLock<Pools> = OnceLock::new();
    let mut m = POOLS
        .get_or_init(|| Mutex::new(std::collections::HashMap::new()))
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    m.entry(threads)
        .or_insert_with(|| {
            let b = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(|i| format!("phitex-{i}"));
            let pool = if let Some(spawn) = SPAWN.get() {
                b.spawn_handler(move |t| {
                    spawn(Box::new(move || t.run()));
                    Ok(())
                })
                .build()
            } else {
                b.build()
            };
            Arc::new(pool.expect("the worker pool"))
        })
        .clone()
}

#[cfg(not(feature = "rayon"))]
fn parallel(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let next = AtomicUsize::new(0);
    let work = || {
        loop {
            let s = next.fetch_add(grain, Ordering::Relaxed);
            if s >= n {
                break;
            }
            for i in s..(s + grain).min(n) {
                f(i);
            }
        }
        phitex_defuse::obs::flush();
    };
    std::thread::scope(|sc| {
        for _ in 1..threads.min(n.div_ceil(grain)) {
            std::thread::Builder::new()
                .name("phitex-worker".to_owned())
                .spawn_scoped(sc, work)
                .expect("a worker thread");
        }
        work();
    });
}
