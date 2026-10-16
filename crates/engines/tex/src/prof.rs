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
    crate::exec::env_flag!("PHITEX_PAR_PROF")
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
