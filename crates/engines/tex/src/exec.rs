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

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use phitex_defuse::{Executor, RunResult};

/// Below this many jobs a batch runs on the calling thread.
const GRAIN: usize = 64;

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

/// The executor of an evaluation, with its counters: the time its jobs
/// ran (summed over jobs), and the most jobs running at once.
pub(crate) struct Threads {
    pub n: usize,
    busy: AtomicU64,
    running: AtomicUsize,
    peak: AtomicUsize,
}

impl Threads {
    pub(crate) fn new(n: usize) -> Threads {
        Threads {
            n,
            busy: AtomicU64::new(0),
            running: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    /// The jobs' summed run time (zero without a clock).
    pub(crate) fn busy(&self) -> std::time::Duration {
        std::time::Duration::from_nanos(self.busy.load(Ordering::Relaxed))
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

impl Executor for Threads {
    fn execute(
        &mut self,
        n: usize,
        run: &(dyn Fn(usize) -> RunResult + Sync),
        out: &mut Vec<RunResult>,
    ) {
        if self.n <= 1 || n < GRAIN {
            out.extend((0..n).map(|i| self.job(|| run(i))));
            return;
        }
        let slots: Vec<OnceLock<RunResult>> = (0..n).map(|_| OnceLock::new()).collect();
        self.parallel_each(n, &|i| {
            let _ = slots[i].set(self.job(|| run(i)));
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
            return;
        }
        let grain = (n / (self.n * 8)).clamp(4, GRAIN);
        trace(n, self.n);
        parallel(self.n, n, grain, f);
    }
}

fn trace(n: usize, threads: usize) {
    if std::env::var_os("PHITEX_EXEC_TRACE").is_some() {
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
    };
    std::thread::scope(|sc| {
        for _ in 1..threads.min(n.div_ceil(grain)) {
            sc.spawn(work);
        }
        work();
    });
}
