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
#[allow(dead_code)]
pub(crate) const MARK: usize = 10;
#[allow(dead_code)]
pub(crate) const MARK_CK: usize = 11;
#[allow(dead_code)]
pub(crate) const MARK_TEXT: usize = 14;
#[allow(dead_code)]
pub(crate) const MARK_STATE: usize = 15;
#[allow(dead_code)]
pub(crate) const MARK_DIGEST: usize = 16;
const N: usize = 18;
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
    "marks: snapshot + digest",
    "marks: checkpoint",
    "marks: kept token",
    "marks: resume",
    "  snapshot: input",
    "  snapshot: table",
    "  digest",
    "marks: retext choice",
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
        #[allow(clippy::cast_precision_loss)]
        let ms = ns[k] as f64 / 1e6;
        eprintln!("parprof:   {name:<32} {:8.1}  ({ms:.1} ms)", per(ns[k]));
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
        "decoded",
        "leaf",
        "dec.char",
        "encoded",
        "enc.tok",
        "enc.char",
        "enc.macro",
        "cs.local",
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
        let mut t = TOTAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
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
            &mut *TOTAL
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
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

/// (`profile`, `PHITEX_PAGES_PROF=1`) The page builder's loop
/// (`crate::pages::builder`), its time per phase summed over its
/// attempts, with the units it folded, printed when the evaluation ends.
pub(crate) mod pages {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub(crate) const NEXT: usize = 0;
    pub(crate) const DECODE: usize = 1;
    pub(crate) const RESOLVE: usize = 2;
    pub(crate) const CLASSES: usize = 3;
    pub(crate) const DROP: usize = 4;
    pub(crate) const FOLD: usize = 6;
    pub(crate) const FIRE: usize = 7;
    pub(crate) const ATTEMPT: usize = 9;
    const N: usize = 10;
    const NAMES: [&str; N] = [
        "acc_next (the next contributor)",
        "decode + marks",
        "resolve_prof_of",
        "insertion classes",
        "drop_units",
        "(unused)",
        "fold_unit (fires included)",
        "  of which on_fire",
        "(unused)",
        "attempt (all of it)",
    ];
    static NS: [AtomicU64; N] = [const { AtomicU64::new(0) }; N];
    static UNITS: AtomicU64 = AtomicU64::new(0);
    static ATTEMPTS: AtomicU64 = AtomicU64::new(0);
    static TORNS: AtomicU64 = AtomicU64::new(0);

    #[inline]
    pub(crate) fn on() -> bool {
        crate::exec::prof_flag!("PHITEX_PAGES_PROF")
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
            NS[i].fetch_add(u64::try_from(t0.elapsed().as_nanos()).unwrap_or(0), Relaxed);
        }
    }

    /// One unit folded.
    #[inline]
    pub(crate) fn unit() {
        if on() {
            UNITS.fetch_add(1, Relaxed);
        }
    }

    /// One attempt of the loop.
    #[inline]
    pub(crate) fn attempt() {
        if on() {
            ATTEMPTS.fetch_add(1, Relaxed);
        }
    }

    /// The summary, reset.
    pub(crate) fn print() {
        if !on() {
            return;
        }
        let units = UNITS.swap(0, Relaxed);
        let attempts = ATTEMPTS.swap(0, Relaxed);
        if attempts == 0 {
            return;
        }
        let torn = TORNS.swap(0, Relaxed);
        eprintln!(
            "pagesprof: {attempts} attempts, {units} units, {torn} torn; ms total, µs per unit:"
        );
        for (k, name) in NAMES.iter().enumerate() {
            let ns = NS[k].swap(0, Relaxed);
            #[allow(clippy::cast_precision_loss)]
            let (ms, per) = (ns as f64 / 1e6, ns as f64 / 1e3 / units.max(1) as f64);
            eprintln!("pagesprof:   {name:<34} {ms:9.2} {per:8.2}");
        }
    }
}
