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
//! Natively any thread may run it. Threads come from `PHITEX_THREADS`,
//! else the available parallelism (on wasm32: `navigator.hardwareConcurrency`
//! as the host passes it through `PHITEX_THREADS`, else 1). Workspace
//! rule: no `unsafe`; borrowed closures reach the workers through
//! rayon's scoped API or `std::thread::scope`.

use std::sync::OnceLock;

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

/// The executor of an evaluation.
pub(crate) struct Threads(pub usize);

impl Threads {
    /// `PHITEX_THREADS`, else the available parallelism; 1 where this
    /// target cannot run threads.
    pub(crate) fn from_env() -> Threads {
        static N: OnceLock<usize> = OnceLock::new();
        Threads(*N.get_or_init(|| {
            if !threads_possible() {
                return 1;
            }
            std::env::var("PHITEX_THREADS")
                .ok()
                .and_then(|s| s.parse().ok())
                .filter(|&n| n > 0)
                .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, usize::from))
        }))
    }
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
        if self.0 <= 1 || n < GRAIN {
            out.extend((0..n).map(run));
            return;
        }
        let slots: Vec<OnceLock<RunResult>> = (0..n).map(|_| OnceLock::new()).collect();
        self.for_each(n, &|i| {
            let _ = slots[i].set(run(i));
        });
        out.extend(
            slots
                .into_iter()
                .map(|s| s.into_inner().expect("every job ran")),
        );
    }

    fn for_each(&mut self, n: usize, f: &(dyn Fn(usize) + Sync)) {
        if self.0 <= 1 || n < GRAIN || !threads_possible() {
            (0..n).for_each(f);
            return;
        }
        let grain = (n / (self.0 * 8)).clamp(4, GRAIN);
        trace(n, self.0);
        parallel(self.0, n, grain, f);
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
