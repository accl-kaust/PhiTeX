//! `PHITEX_PAR_PROF=1`: the paragraph run's phases, summed over the
//! evaluation's exec runs and printed per paragraph (a `line_break`)
//! when the evaluation ends. A measurement aid: off, each probe is one
//! test of a flag read once.

use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

/// The phases.
pub(crate) const RUN: usize = 0;
pub(crate) const CHAR: usize = 1;
pub(crate) const CHAR_ENC: usize = 2;
pub(crate) const LB: usize = 3;
pub(crate) const LB_DEC: usize = 4;
pub(crate) const LB_PARAMS: usize = 5;
pub(crate) const LB_TS: usize = 6;
pub(crate) const LB_COMMIT: usize = 7;
pub(crate) const FOLD: usize = 8;
pub(crate) const FINISH: usize = 9;
const N: usize = 10;
const NAMES: [&str; N] = [
    "exec run (body+fold+finish)",
    "char_run",
    "  of which nodes to the list",
    "line_break",
    "  hlist to typeset nodes",
    "  params/patterns/lccode/fonts",
    "  typeset line_break",
    "  lines encoded + appended",
    "page_fold",
    "finish",
];

static NS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
static PARS: AtomicU64 = AtomicU64::new(0);
static RUNS: AtomicU64 = AtomicU64::new(0);

/// Whether the probes run.
#[inline]
pub(crate) fn on() -> bool {
    crate::exec::prof_flag!("PHITEX_PAR_PROF")
}

/// A probe's start (`None` when off).
#[inline]
pub(crate) fn start() -> Option<std::time::Instant> {
    on().then(std::time::Instant::now)
}

/// Phase `i` gains the time since `t0`.
#[inline]
pub(crate) fn add(i: usize, t0: Option<std::time::Instant>) {
    if let Some(t0) = t0 {
        let d = u64::try_from(t0.elapsed().as_nanos()).unwrap_or(0);
        NS[i].fetch_add(d, Relaxed);
    }
}

/// One paragraph broken.
pub(crate) fn paragraph() {
    if on() {
        PARS.fetch_add(1, Relaxed);
    }
}

/// One exec run done.
pub(crate) fn run() {
    if on() {
        RUNS.fetch_add(1, Relaxed);
    }
}

/// The summary, reset (nothing when off or no paragraph was broken).
pub(crate) fn print() {
    if !on() {
        return;
    }
    let pars = PARS.swap(0, Relaxed);
    let runs = RUNS.swap(0, Relaxed);
    let ns: Vec<u64> = NS.iter().map(|a| a.swap(0, Relaxed)).collect();
    if pars == 0 {
        return;
    }
    #[allow(clippy::cast_precision_loss)]
    let per = |x: u64| x as f64 / pars as f64 / 1000.0;
    eprintln!("parprof: {pars} paragraphs, {runs} exec runs; µs per paragraph:");
    for (k, name) in NAMES.iter().enumerate() {
        eprintln!("parprof:   {name:<32} {:8.1}", per(ns[k]));
    }
}

/// (`profile`, `PHITEX_CACHE_STATS=1`) The thread caches' lookups: per
/// cache its hits, misses and the time the misses spent making the
/// entry, counted per thread (no shared line written per lookup) and
/// summed when the evaluation's executor drops.
pub(crate) mod caches {
    use std::cell::Cell;
    use std::sync::Mutex;

    pub(crate) const DEC: usize = 0;
    pub(crate) const LEAF: usize = 1;
    pub(crate) const DEC_CHAR: usize = 2;
    pub(crate) const ENC: usize = 3;
    pub(crate) const ENC_TOK: usize = 4;
    pub(crate) const ENC_CHAR: usize = 5;
    pub(crate) const ENC_MACRO: usize = 6;
    pub(crate) const CS: usize = 7;
    pub(crate) const ENC_RESET: usize = 8;
    const N: usize = 9;
    const NAMES: [&str; N] = [
        "decoded", "leaf", "dec.char", "encoded", "enc.tok", "enc.char", "enc.macro", "cs.local",
        "enc.reset",
    ];

    type Counts = [[u64; 3]; N];
    static TOTAL: Mutex<Counts> = Mutex::new([[0; 3]; N]);

    /// This thread's counts, added to [`TOTAL`] when the thread ends or
    /// at [`flush`].
    struct Local(Cell<Counts>);
    impl Drop for Local {
        fn drop(&mut self) {
            add_total(self.0.get());
        }
    }
    thread_local! {
        static LOCAL: Local = const { Local(Cell::new([[0; 3]; N])) };
    }

    fn add_total(c: Counts) {
        let mut t = TOTAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        for (a, b) in t.iter_mut().zip(c) {
            for (x, y) in a.iter_mut().zip(b) {
                *x += y;
            }
        }
    }

    #[inline]
    pub(crate) fn on() -> bool {
        crate::exec::prof_flag!("PHITEX_CACHE_STATS")
    }

    #[inline]
    fn bump(i: usize, k: usize, n: u64) {
        let _ = LOCAL.try_with(|l| {
            let mut c = l.0.get();
            c[i][k] += n;
            l.0.set(c);
        });
    }

    /// A hit in cache `i`.
    #[inline]
    pub(crate) fn hit(i: usize) {
        if on() {
            bump(i, 0, 1);
        }
    }

    /// A miss in cache `i`: its time starts (`None` when off).
    #[inline]
    pub(crate) fn miss(i: usize) -> Option<(usize, std::time::Instant)> {
        on().then(|| {
            bump(i, 1, 1);
            (i, std::time::Instant::now())
        })
    }

    /// The miss's making ends.
    #[inline]
    pub(crate) fn made(m: Option<(usize, std::time::Instant)>) {
        if let Some((i, t)) = m {
            bump(i, 2, u64::try_from(t.elapsed().as_nanos()).unwrap_or(0));
        }
    }

    /// This thread's counts into the totals.
    pub(crate) fn flush() {
        if on() {
            let _ = LOCAL.try_with(|l| add_total(l.0.replace([[0; 3]; N])));
        }
    }

    /// The totals, printed and reset.
    pub(crate) fn print() {
        if !on() {
            return;
        }
        flush();
        let t = std::mem::replace(
            &mut *TOTAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner),
            [[0; 3]; N],
        );
        let parts: Vec<String> = NAMES
            .iter()
            .zip(t)
            .filter(|(_, c)| c[0] + c[1] > 0)
            .map(|(n, c)| {
                #[allow(clippy::cast_precision_loss)]
                let ms = c[2] as f64 / 1e6;
                format!("{n} {}h/{}m {ms:.3}ms", c[0], c[1])
            })
            .collect();
        eprintln!("acct: thread caches: {}", parts.join(", "));
    }
}
