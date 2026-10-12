//! The engine's executor: a batch of runs, and the engine's parallel
//! passes, on up to `threads` threads (this one included). The schedule
//! changes how fast, never what: every result lands in its index's
//! slot, read back in index order.
//!
//! Threads come from `PHITEX_THREADS`, else the machine's available
//! parallelism; one thread, or wasm32, is the sequential path. The
//! workers are scoped to each batch (`std::thread::scope`): borrowed
//! closures cannot go to a persistent pool without `unsafe`, which the
//! workspace denies. Small batches run inline.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

use phitex_defuse::{Executor, RunResult};

/// Below this many jobs a batch runs on the calling thread.
const GRAIN: usize = 64;

/// The executor of an evaluation.
pub(crate) struct Threads(pub usize);

impl Threads {
    /// `PHITEX_THREADS`, else the available parallelism (1 on wasm32).
    pub(crate) fn from_env() -> Threads {
        static N: OnceLock<usize> = OnceLock::new();
        Threads(*N.get_or_init(|| {
            if cfg!(target_arch = "wasm32") {
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
        if self.0 <= 1 || n < GRAIN || cfg!(target_arch = "wasm32") {
            (0..n).for_each(f);
            return;
        }
        let next = AtomicUsize::new(0);
        let grain = (n / (self.0 * 8)).clamp(4, GRAIN);
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
        let threads = self.0.min(n.div_ceil(grain));
        if std::env::var_os("PHITEX_EXEC_TRACE").is_some() {
            eprintln!("exec: {n} jobs on {threads} threads");
        }
        std::thread::scope(|sc| {
            for _ in 1..threads {
                sc.spawn(work);
            }
            work();
        });
    }
}
