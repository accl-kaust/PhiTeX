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

use phitex_defuse::FxHashMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use phitex_defuse::{Executor, RunResult};

/// Whether the environment variable `$name` is set, read once per
/// process (a `getenv` takes the environment's lock and scans it; the
/// debug switches are tested on every run). Unset, empty or "0" is off.
macro_rules! env_flag {
    ($name:literal) => {{
        static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        *F.get_or_init(|| std::env::var_os($name).is_some_and(|v| !v.is_empty() && v != "0"))
    }};
}
pub(crate) use env_flag;

/// A debug variable's text, read once per process (`None`: unset or
/// empty).
macro_rules! env_str {
    ($name:literal) => {{
        static F: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
        F.get_or_init(|| std::env::var($name).ok().filter(|v| !v.is_empty()))
            .as_deref()
    }};
}
pub(crate) use env_str;

/// A trace, dump or debug check's switch (`observe` builds): `$name` set
/// ([`env_flag`]); compiled out of other builds, its code with it.
macro_rules! obs_flag {
    ($name:literal) => {
        cfg!(feature = "observe") && $crate::exec::env_flag!($name)
    };
}
pub(crate) use obs_flag;

/// Lines the runs print with `PHITEX_TEX_TIME` (a node's time, a page's
/// phases), kept and printed after the evaluation ([`flush_later`]):
/// stderr written inside the round would be in its walls.
static LATER: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// Keeps `line` for [`flush_later`].
#[cold]
#[inline(never)]
pub(crate) fn later(line: String) {
    LATER
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .push(line);
}

/// Prints the lines kept by [`later`], in order.
#[cold]
#[inline(never)]
pub(crate) fn flush_later() {
    let lines = std::mem::take(
        &mut *LATER
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
    );
    for l in lines {
        eprintln!("{l}");
    }
}

/// A trace or dump's text switch (`observe` builds, [`env_str`]).
macro_rules! obs_str {
    ($name:literal) => {
        if cfg!(feature = "observe") {
            $crate::exec::env_str!($name)
        } else {
            None
        }
    };
}
pub(crate) use obs_str;

/// A timing or counter switch (`profile` builds): `$name` set
/// ([`env_flag`]); compiled out of other builds, its code with it.
macro_rules! prof_flag {
    ($name:literal) => {
        cfg!(feature = "profile") && $crate::exec::env_flag!($name)
    };
}
pub(crate) use prof_flag;

/// Below this many items an engine pass runs on the calling thread
/// (its items cost microseconds).
const GRAIN: usize = 64;

/// Below this many jobs a batch of runs goes inline. A run is a
/// paragraph region, a page step or a format (hundreds of microseconds
/// to seconds), so any batch of two or more is worth a hand-off to the
/// pool.
fn job_grain() -> usize {
    2
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
    pub offsets: Arc<FxHashMap<u64, u64>>,
}

impl Watch {
    pub(crate) fn new(workers: usize, given: Option<Arc<Progress>>) -> Watch {
        Watch {
            progress: given.unwrap_or_else(|| Arc::new(Progress::new(workers))),
            offsets: Arc::new(FxHashMap::default()),
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

/// What the operations of an evaluation did, counted where they run
/// ([`NodeTimer`]), not where the executor's tasks run: a task is a
/// worker's loop, which also waits while other workers hold work.
#[derive(Default)]
pub(crate) struct OpCounters {
    /// the operations' run time, summed
    busy: AtomicU64,
    /// the run time and the runs by node kind ([`KINDS`], spec-final §1)
    kind_ns: [AtomicU64; KINDS.len()],
    kind_runs: [AtomicU64; KINDS.len()],
    /// operations running now
    running: AtomicUsize,
    /// the most operations running at once
    peak: AtomicUsize,
}

/// The node kinds the cold accounting splits op time by (spec-final §1):
/// a package load is an `exec` run of a load's text.
pub(crate) const KINDS: [&str; 9] = [
    "exec", "load", "scan", "finish", "resume", "files", "link", "dvi", "put",
];

/// The executor of an evaluation, with its counters: the time its
/// operations ran (summed over runs), and the most runs at once.
pub(crate) struct Threads {
    pub n: usize,
    watch: Option<Watch>,
    ops: std::sync::Arc<OpCounters>,
    batches: AtomicU64,
    batches_inline: AtomicU64,
    jobs_max_batch: AtomicUsize,
}

impl Threads {
    pub(crate) fn new(n: usize) -> Threads {
        Threads {
            n,
            watch: None,
            ops: std::sync::Arc::default(),
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

    /// The operations' run time and runs by node kind ([`KINDS`]).
    pub(crate) fn by_kind(&self) -> Vec<(&'static str, std::time::Duration, u64)> {
        KINDS
            .iter()
            .enumerate()
            .map(|(i, &k)| {
                (
                    k,
                    std::time::Duration::from_nanos(self.ops.kind_ns[i].load(Ordering::Relaxed)),
                    self.ops.kind_runs[i].load(Ordering::Relaxed),
                )
            })
            .collect()
    }

    /// The operations' summed run time (zero without a clock).
    pub(crate) fn busy(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.ops.busy.load(Ordering::Relaxed))
    }

    /// The batches of runs, those run inline, and the largest batch.
    pub(crate) fn batches(&self) -> (u64, u64, usize) {
        (
            self.batches.load(Ordering::Relaxed),
            self.batches_inline.load(Ordering::Relaxed),
            self.jobs_max_batch.load(Ordering::Relaxed),
        )
    }

    /// The most operations that ran at once.
    pub(crate) fn peak(&self) -> usize {
        self.ops.peak.load(Ordering::Relaxed)
    }

    /// Runs one job (a worker's loop over runs): the operations it runs
    /// count on this executor's counters.
    fn job<R>(&self, f: impl FnOnce() -> R) -> R {
        let _run = widths::Run::begin();
        let prev = JOB_THREADS.with(|c| c.replace(self.n));
        let prev_ops = JOB_OPS.with(|c| c.replace(Some(self.ops.clone())));
        let r = f();
        JOB_OPS.with(|c| *c.borrow_mut() = prev_ops);
        JOB_THREADS.with(|c| c.set(prev));
        r
    }
}

thread_local! {
    /// The thread count of the executor whose job runs on this thread
    /// (1 outside a job).
    static JOB_THREADS: std::cell::Cell<usize> = const { std::cell::Cell::new(1) };
    /// The counters of the executor whose job runs on this thread.
    static JOB_OPS: RefCell<Option<std::sync::Arc<OpCounters>>> = const { RefCell::new(None) };
}

/// The thread count: `given`, else `PHITEX_THREADS`, else the available
/// parallelism; 1 where this target cannot run threads.
pub(crate) fn threads(given: Option<usize>) -> usize {
    // (`PHITEX_THREADS`, read once)
    static ENV: OnceLock<Option<usize>> = OnceLock::new();
    if !threads_possible() {
        return 1;
    }
    given.filter(|&n| n > 0).unwrap_or_else(|| {
        ENV.get_or_init(|| {
            std::env::var("PHITEX_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n > 0)
        })
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
        widths::print();
        crate::prof::print();
        if phitex_defuse::obs::enabled() && prof_flag!("PHITEX_OBS") {
            eprint!("probes (evaluation):\n{}", phitex_defuse::obs::snapshot());
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
        let _st = StepTrace::enter(n);
        if let Some(w) = &self.watch {
            w.progress
                .total
                .fetch_add(u64::try_from(n).unwrap_or(0), Ordering::Relaxed);
        }
        let _step = widths::Step::begin(n);
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

    /// The workers the engine's own passes (its apply) may split over:
    /// the pool's size where threads exist, else 1.
    fn parallelism(&self) -> usize {
        if threads_possible() { self.n.max(1) } else { 1 }
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

/// The executor of a client pass whose items each cost a run's worth
/// (the hint pass's chunks, spec-final §5.1): `Executor::for_each`
/// splits any batch of two or more over the pool, one item a task.
pub(crate) struct Coarse(pub Threads);

impl Executor for Coarse {
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> RunResult + Sync),
        out: &mut Vec<RunResult>,
    ) {
        self.0.execute(n, run, out);
    }

    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if self.0.n <= 1 || n < 2 || !threads_possible() {
            (0..n).for_each(f);
            return;
        }
        trace(n, self.0.n);
        parallel(self.0.n, n, 1, &|i| self.0.job(|| f(i)));
    }

    fn parallelism(&self) -> usize {
        self.0.parallelism()
    }
}

/// The executor of a fan-out inside an operation's run (the exact
/// PDF's page contents, the DVI file's pages): `for_each` splits a batch
/// over `self.0` threads of its own, never a pool's.
pub(crate) struct Inner(pub usize);

impl Executor for Inner {
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> RunResult + Sync),
        out: &mut Vec<RunResult>,
    ) {
        out.extend((0..n).map(run));
    }

    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        // (wasm32: `std::thread::spawn` is not there, whatever the host
        // gives the executor's pool; the fan-out runs in order)
        if self.0 <= 1 || n < 2 || !threads_possible() || cfg!(target_arch = "wasm32") {
            (0..n).for_each(f);
            return;
        }
        trace(n, self.0);
        // (threads of its own, not a pool's: a pool's `install` called
        // from an engine worker's thread runs that pool's other jobs
        // while it waits, an engine worker's loop among them, which
        // waits for this run: the stack itself deadlocked)
        scoped(self.0, n, 1, f);
    }

    fn parallelism(&self) -> usize {
        if threads_possible() { self.0.max(1) } else { 1 }
    }
}

/// `f(i)` for every `i < n` on executor `ex` (its `for_each`), the
/// results in order.
pub(crate) fn map_on<T: Send + Sync>(
    ex: &mut dyn Executor,
    n: usize,
    f: &(dyn Fn(usize) -> T + Sync),
) -> Vec<T> {
    let slots: Vec<OnceLock<T>> = (0..n).map(|_| OnceLock::new()).collect();
    ex.for_each(n, &|i| {
        let _ = slots[i].set(f(i));
    });
    slots
        .into_iter()
        .map(|s| s.into_inner().expect("every job ran"))
        .collect()
}

fn trace(n: usize, threads: usize) {
    if obs_flag!("PHITEX_EXEC_TRACE") {
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

/// The persistent pool of `threads` workers, the engine's workers' (an
/// operation's fan-out runs on threads of its own, [`Inner`]).
#[cfg(feature = "rayon")]
fn pool(threads: usize) -> std::sync::Arc<rayon::ThreadPool> {
    use std::sync::{Arc, Mutex, PoisonError};
    type Pools = Mutex<phitex_defuse::FxHashMap<usize, Arc<rayon::ThreadPool>>>;
    static POOLS: OnceLock<Pools> = OnceLock::new();
    let mut m = POOLS
        .get_or_init(|| Mutex::new(phitex_defuse::FxHashMap::default()))
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
    scoped(threads, n, grain, f);
}

/// `f(i)` for every `i < n` on `threads` threads of its own, made for
/// the call and joined at its end.
fn scoped(threads: usize, n: usize, grain: usize, f: &(dyn Fn(usize) + Sync)) {
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

/// `PHITEX_WIDTHS=1`: statistics only (no behaviour change) for the
/// engine's label-range partition decision. Per evaluation step (one
/// `execute` batch): the ready nodes, the distinct fields read and
/// defined; per run: the engine reads and definitions; the value-table
/// lookups per run and the share missing the per-worker read cache
/// (these two need the `profile` feature, else they print zero).
/// Histograms are printed when the executor is dropped.
pub(crate) mod widths {
    use phitex_defuse::FxHashSet as HashSet;
    use std::cell::Cell;
    use std::sync::Mutex;

    use phitex_defuse::Field;

    pub(crate) fn on() -> bool {
        super::prof_flag!("PHITEX_WIDTHS")
    }

    /// Power-of-two buckets: bucket `b` holds values in `[2^(b-1), 2^b)`,
    /// bucket 0 the zeros.
    #[derive(Default)]
    struct Hist {
        b: [u64; 32],
        n: u64,
        sum: u64,
        max: u64,
    }

    impl Hist {
        fn add(&mut self, x: u64) {
            let b = (64 - x.leading_zeros()) as usize;
            self.b[b.min(31)] += 1;
            self.n += 1;
            self.sum += x;
            self.max = self.max.max(x);
        }
        // PHITEX_WIDTHS statistics only: f64 precision is irrelevant here
        #[allow(clippy::cast_precision_loss)]
        fn print(&self, name: &str) {
            if self.n == 0 {
                return;
            }
            eprintln!(
                "widths: {name}: n={} mean={:.1} max={}",
                self.n,
                self.sum as f64 / self.n as f64,
                self.max
            );
            for (i, &c) in self.b.iter().enumerate() {
                if c == 0 {
                    continue;
                }
                let (lo, hi) = if i == 0 {
                    (0, 0)
                } else {
                    (1u64 << (i - 1), (1u64 << i) - 1)
                };
                let pct = 100.0 * c as f64 / self.n as f64;
                eprintln!("widths:   {lo:>8}..{hi:<8} {c:>9} {pct:5.1}%");
            }
        }
    }

    #[derive(Default)]
    struct Stats {
        ready: Hist,
        step_reads: Hist,
        step_defs: Hist,
        run_reads: Hist,
        run_defs: Hist,
        step_lookups_per_run: Hist,
        step_miss_pct: Hist,
        lookups: u64,
        misses: u64,
        reads: HashSet<Field>,
        defs: HashSet<Field>,
    }

    static STATS: Mutex<Option<Stats>> = Mutex::new(None);

    thread_local! {
        static RUN: Cell<(u64, u64)> = const { Cell::new((0, 0)) };
    }

    fn with<T>(f: impl FnOnce(&mut Stats) -> T) -> T {
        let mut g = STATS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(g.get_or_insert_with(Stats::default))
    }

    fn counters() -> (u64, u64) {
        let s = phitex_defuse::obs::snapshot();
        let get = |n: &str| s.counters.iter().find(|c| c.0 == n).map_or(0, |c| c.1);
        (get("value_get"), get("value_get_miss"))
    }

    /// An engine read of `f` by the current run.
    pub(crate) fn read(f: Field) {
        if on() {
            RUN.with(|c| c.set((c.get().0 + 1, c.get().1)));
            with(|s| s.reads.insert(f));
        }
    }

    /// A definition of `f` by the current run.
    pub(crate) fn def(f: Field) {
        if on() {
            RUN.with(|c| c.set((c.get().0, c.get().1 + 1)));
            with(|s| s.defs.insert(f));
        }
    }

    pub(crate) struct Step(Option<(u64, (u64, u64))>);

    impl Step {
        pub(crate) fn begin(n: usize) -> Step {
            if !on() {
                return Step(None);
            }
            with(|s| {
                s.reads.clear();
                s.defs.clear();
            });
            Step(Some((n as u64, counters())))
        }
    }

    impl Drop for Step {
        fn drop(&mut self) {
            let Some((n, (g0, m0))) = self.0 else { return };
            let (g1, m1) = counters();
            let (g, m) = (g1.saturating_sub(g0), m1.saturating_sub(m0));
            with(|s| {
                s.ready.add(n);
                s.step_reads.add(s.reads.len() as u64);
                s.step_defs.add(s.defs.len() as u64);
                s.step_lookups_per_run.add(g / n.max(1));
                if let Some(p) = (m * 100).checked_div(g) {
                    s.step_miss_pct.add(p);
                }
                s.lookups += g;
                s.misses += m;
            });
        }
    }

    pub(crate) struct Run(bool);

    impl Run {
        pub(crate) fn begin() -> Run {
            if on() {
                RUN.with(|c| c.set((0, 0)));
            }
            Run(on())
        }
    }

    impl Drop for Run {
        fn drop(&mut self) {
            if !self.0 {
                return;
            }
            // the worker's probe counts reach the global totals
            phitex_defuse::obs::flush();
            let (r, d) = RUN.with(Cell::get);
            with(|s| {
                s.run_reads.add(r);
                s.run_defs.add(d);
            });
        }
    }

    /// The histograms so far, then cleared.
    // PHITEX_WIDTHS statistics only: f64 precision is irrelevant here
    #[allow(clippy::cast_precision_loss)]
    pub(crate) fn print() {
        if !on() {
            return;
        }
        let Some(s) = STATS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        if s.ready.n == 0 {
            return;
        }
        s.ready.print("ready nodes per step");
        s.step_reads.print("distinct fields read per step");
        s.step_defs.print("distinct fields defined per step");
        s.run_reads.print("engine reads per run");
        s.run_defs.print("definitions per run");
        s.step_lookups_per_run
            .print("value lookups per run (step mean)");
        s.step_miss_pct.print("lookup cache miss % per step");
        let pct = if s.lookups == 0 {
            0.0
        } else {
            100.0 * s.misses as f64 / s.lookups as f64
        };
        eprintln!(
            "widths: lookups {} misses {} ({pct:.1}%)",
            s.lookups, s.misses
        );
    }
}

/// (measurement) `PHITEX_STEP_TRACE`: per batch, its width, its run time,
/// and the time since the previous batch ended (the engine's apply of it).
struct StepTrace(Option<(std::time::Instant, usize)>);

static STEP_LAST: std::sync::Mutex<Option<(std::time::Instant, usize)>> =
    std::sync::Mutex::new(None);

impl StepTrace {
    fn enter(n: usize) -> StepTrace {
        if !obs_flag!("PHITEX_STEP_TRACE") {
            return StepTrace(None);
        }
        let now = std::time::Instant::now();
        if let Some((t, pn)) = *STEP_LAST.lock().unwrap() {
            eprintln!(
                "stepgap: prev_n {pn} gap_us {}",
                now.duration_since(t).as_micros()
            );
        }
        StepTrace(Some((now, n)))
    }
}

impl Drop for StepTrace {
    fn drop(&mut self) {
        if let Some((t, n)) = self.0 {
            let now = std::time::Instant::now();
            eprintln!(
                "steprun: n {n} run_us {}",
                now.duration_since(t).as_micros()
            );
            *STEP_LAST.lock().unwrap() = Some((now, n));
        }
    }
}

/// One operation's run, counted on the running executor's
/// [`OpCounters`] (busy time, runs at once); with `PHITEX_TEX_TIME`
/// also its operation, node and wall time, printed when the run ends.
pub(crate) struct NodeTimer {
    trace: Option<(std::time::Instant, &'static str, u64)>,
    ops: Option<(std::sync::Arc<OpCounters>, Option<u64>, usize)>,
}

impl NodeTimer {
    pub(crate) fn new(op: &'static str, node: u64) -> NodeTimer {
        let kind = KINDS.iter().position(|&k| k == op).unwrap_or(0);
        let ops = JOB_OPS.with(|c| c.borrow().clone()).map(|o| {
            let c = o.running.fetch_add(1, Ordering::Relaxed) + 1;
            o.peak.fetch_max(c, Ordering::Relaxed);
            (o, clock().now(), kind)
        });
        let trace = prof_flag!("PHITEX_TEX_TIME").then(|| (std::time::Instant::now(), op, node));
        NodeTimer { trace, ops }
    }
}

impl Drop for NodeTimer {
    fn drop(&mut self) {
        if let Some((o, t0, kind)) = self.ops.take() {
            if let (Some(a), Some(b)) = (t0, clock().now()) {
                o.busy.fetch_add(b.saturating_sub(a), Ordering::Relaxed);
                o.kind_ns[kind].fetch_add(b.saturating_sub(a), Ordering::Relaxed);
            }
            o.kind_runs[kind].fetch_add(1, Ordering::Relaxed);
            o.running.fetch_sub(1, Ordering::Relaxed);
        }
        if let Some((t, op, node)) = self.trace {
            later(format!(
                "nodetime: {op} {node} {:.3}",
                t.elapsed().as_secs_f64() * 1e3
            ));
        }
    }
}
