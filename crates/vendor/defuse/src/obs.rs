//! Observability for profiling builds: counters and timed spans, with an
//! optional event trace exportable to Chrome / Perfetto.
//!
//! Probes are compiled in only with the `profile` feature
//! (`cargo build --profile profiling --features profile`). Without it every
//! probe expands to nothing, and the functions here return empty results,
//! so release builds pay nothing.
//!
//! Counts and span totals are kept per thread and merged into the global
//! totals when a thread exits or calls [`snapshot`], so probes do not add
//! cross-thread contention to what they measure. On `wasm32` spans are
//! counted but not timed (there is no clock).

/// What a counter counts.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Counter {
    /// Binding walks (reaching-definition lookups).
    Walk,
    /// Scope levels visited by binding walks.
    WalkLevel,
    /// Scheduler steps (one batch each).
    Step,
    /// Jobs started (all kinds).
    Job,
    /// Jobs started speculatively.
    SpecJob,
    /// Nodes held back by their read hints before starting.
    Gated,
    /// Nodes parked waiting on another node.
    Park,
    /// Parked nodes woken.
    Wake,
    /// Nodes dirtied.
    Dirty,
    /// Uses moved to another reaching definition.
    MoveUser,
    /// Nodes re-bound because a scope boundary changed.
    RegionNode,
    /// Runs discarded because a definition they read went away meanwhile.
    StaleSite,
    /// Results dropped because the node went away or was re-bound (its
    /// epoch moved) while it ran.
    DeadResult,
    /// Results dropped because the node's state no longer takes one.
    StateResult,
    /// Re-bound nodes revalidated without running.
    RevalidateOk,
    /// Re-bound nodes that had to run after all.
    RevalidateMiss,
    ProjKept,
    /// Claims recomputed.
    ClaimRecompute,
    /// Nodes created.
    Create,
    /// Nodes freed.
    Free,
    /// Value table lookups.
    ValueGet,
    /// Value lookups missing the per-thread read cache.
    ValueGetMiss,
    /// Values interned.
    ValueIntern,
    /// Value table sweeps.
    Sweep,
    /// Writes to another slice's nodes deferred to the end of a batch.
    Tail,
    /// Use records a run left (registered by `set_uses`).
    UseRecord,
    /// Slices of a wide batch that aborted (applied whole instead).
    SliceAbort,
    /// Results applied in slices of a wide batch.
    SlicedResult,
    /// Results of a wide batch applied whole, after its slices.
    WholeResult,
    /// Results of batches too narrow to slice.
    NarrowResult,
}

impl Counter {
    pub const ALL: [Counter; 30] = [
        Counter::Walk,
        Counter::WalkLevel,
        Counter::Step,
        Counter::Job,
        Counter::SpecJob,
        Counter::Gated,
        Counter::Park,
        Counter::Wake,
        Counter::Dirty,
        Counter::MoveUser,
        Counter::RegionNode,
        Counter::StaleSite,
        Counter::DeadResult,
        Counter::StateResult,
        Counter::RevalidateOk,
        Counter::RevalidateMiss,
        Counter::ProjKept,
        Counter::ClaimRecompute,
        Counter::Create,
        Counter::Free,
        Counter::ValueGet,
        Counter::ValueGetMiss,
        Counter::ValueIntern,
        Counter::Sweep,
        Counter::Tail,
        Counter::UseRecord,
        Counter::SliceAbort,
        Counter::SlicedResult,
        Counter::WholeResult,
        Counter::NarrowResult,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Counter::Walk => "walk",
            Counter::WalkLevel => "walk_level",
            Counter::Step => "step",
            Counter::Job => "job",
            Counter::SpecJob => "spec_job",
            Counter::Gated => "gated",
            Counter::Park => "park",
            Counter::Wake => "wake",
            Counter::Dirty => "dirty",
            Counter::MoveUser => "move_user",
            Counter::RegionNode => "region_node",
            Counter::StaleSite => "stale_site",
            Counter::DeadResult => "dead_result",
            Counter::StateResult => "state_result",
            Counter::RevalidateOk => "revalidate_ok",
            Counter::RevalidateMiss => "revalidate_miss",
            Counter::ProjKept => "proj_kept",
            Counter::ClaimRecompute => "claim_recompute",
            Counter::Create => "create",
            Counter::Free => "free",
            Counter::ValueGet => "value_get",
            Counter::ValueGetMiss => "value_get_miss",
            Counter::ValueIntern => "value_intern",
            Counter::Sweep => "sweep",
            Counter::Tail => "tail",
            Counter::UseRecord => "use_record",
            Counter::SliceAbort => "slice_abort",
            Counter::SlicedResult => "sliced_result",
            Counter::WholeResult => "whole_result",
            Counter::NarrowResult => "narrow_result",
        }
    }
}

/// A timed region.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Span {
    /// A whole `evaluate` call.
    Evaluate,
    /// Running one batch of jobs (the parallel part).
    RunBatch,
    /// Applying one batch's results (the sequential part).
    ApplyBatch,
    /// One job (an operation run).
    Job,
    /// Settling claims after structure edits.
    FlushClaims,
    /// Freeing unreferenced nodes.
    FlushFrees,
    /// Collecting cycles of applications.
    Cycles,
    /// A value table sweep.
    Sweep,
    /// A client edit (insert, remove, replace).
    Edit,
    /// Recomputing one claim.
    Claim,
    /// Re-binding the region of a changed scope boundary.
    Region,
    /// Checking ready nodes' read hints before starting them.
    Gate,
    /// Choosing the batch (candidates, scheduler, sanitising).
    Select,
    /// Registering a run's uses (the users index).
    Uses,
    /// Demand bookkeeping for changed dependencies.
    Deps,
    /// Settling a finished node (heights, waking waiters).
    Finish,
    /// Applying a run's unfolded children (creating and binding them).
    Children,
    /// Applying a run's definitions (re-binding readers of changed ones).
    Defs,
    /// Creating the children's records (inside `Children`).
    Create,
    /// Inserting a record into the unit's order (inside `Create`).
    OrderInsert,
    /// Binding the created children (inside `Children`, after `Create`).
    Bind,
    /// Replaying a sliced batch's deferred writes (inside `ApplyBatch`).
    Tail,
    /// Applying a wide batch's slices through overlays (inside `ApplyBatch`).
    Slices,
    /// Merging the slices' overlays into the engine (inside `ApplyBatch`).
    Merge,
}

impl Span {
    pub const ALL: [Span; 24] = [
        Span::Evaluate,
        Span::RunBatch,
        Span::ApplyBatch,
        Span::Job,
        Span::FlushClaims,
        Span::FlushFrees,
        Span::Cycles,
        Span::Sweep,
        Span::Edit,
        Span::Claim,
        Span::Region,
        Span::Gate,
        Span::Select,
        Span::Uses,
        Span::Deps,
        Span::Finish,
        Span::Children,
        Span::Defs,
        Span::Create,
        Span::OrderInsert,
        Span::Bind,
        Span::Tail,
        Span::Slices,
        Span::Merge,
    ];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Span::Evaluate => "evaluate",
            Span::RunBatch => "run_batch",
            Span::ApplyBatch => "apply_batch",
            Span::Job => "job",
            Span::FlushClaims => "flush_claims",
            Span::FlushFrees => "flush_frees",
            Span::Cycles => "cycles",
            Span::Sweep => "sweep",
            Span::Edit => "edit",
            Span::Claim => "claim",
            Span::Region => "region",
            Span::Gate => "gate",
            Span::Select => "select",
            Span::Uses => "uses",
            Span::Deps => "deps",
            Span::Finish => "finish",
            Span::Children => "children",
            Span::Defs => "defs",
            Span::Create => "create",
            Span::OrderInsert => "order_insert",
            Span::Bind => "bind",
            Span::Tail => "tail",
            Span::Slices => "slices",
            Span::Merge => "merge",
        }
    }
}

/// Totals since the last [`reset`].
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// `(name, count)` for every counter.
    pub counters: Vec<(&'static str, u64)>,
    /// `(name, entries, total nanoseconds)` for every span.
    pub spans: Vec<(&'static str, u64, u64)>,
}

impl std::fmt::Display for Snapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (name, n) in &self.counters {
            if *n > 0 {
                writeln!(f, "  {name:<18} {n:>12}")?;
            }
        }
        for (name, n, ns) in &self.spans {
            if *n > 0 {
                #[allow(clippy::cast_precision_loss)]
                let ms = *ns as f64 / 1e6;
                writeln!(f, "  {name:<18} {n:>12}  {ms:>10.3} ms")?;
            }
        }
        Ok(())
    }
}

/// Whether probes are compiled in.
#[must_use]
pub const fn enabled() -> bool {
    cfg!(feature = "profile")
}

#[cfg(feature = "profile")]
mod imp {
    use super::{Counter, Snapshot, Span};
    use std::cell::RefCell;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    const NC: usize = Counter::ALL.len();
    const NS: usize = Span::ALL.len();
    /// Trace events kept at most (later ones are dropped and counted).
    const MAX_EVENTS: usize = 4_000_000;

    #[derive(Clone, Copy)]
    struct Event {
        span: Span,
        tid: u32,
        start_ns: u64,
        dur_ns: u64,
    }

    struct Global {
        counters: [AtomicU64; NC],
        span_n: [AtomicU64; NS],
        span_ns: [AtomicU64; NS],
        events: Mutex<Vec<Event>>,
        dropped: AtomicU64,
    }

    static GLOBAL: Global = Global {
        counters: [const { AtomicU64::new(0) }; NC],
        span_n: [const { AtomicU64::new(0) }; NS],
        span_ns: [const { AtomicU64::new(0) }; NS],
        events: Mutex::new(Vec::new()),
        dropped: AtomicU64::new(0),
    };
    static TRACING: AtomicBool = AtomicBool::new(false);
    static NEXT_TID: AtomicU64 = AtomicU64::new(0);

    struct Local {
        tid: u32,
        counters: [u64; NC],
        span_n: [u64; NS],
        span_ns: [u64; NS],
        events: Vec<Event>,
    }

    impl Local {
        fn flush(&mut self) {
            for (g, l) in GLOBAL.counters.iter().zip(&mut self.counters) {
                g.fetch_add(std::mem::take(l), Ordering::Relaxed);
            }
            for (g, l) in GLOBAL.span_n.iter().zip(&mut self.span_n) {
                g.fetch_add(std::mem::take(l), Ordering::Relaxed);
            }
            for (g, l) in GLOBAL.span_ns.iter().zip(&mut self.span_ns) {
                g.fetch_add(std::mem::take(l), Ordering::Relaxed);
            }
            if !self.events.is_empty() {
                let mut ev = GLOBAL.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let room = MAX_EVENTS.saturating_sub(ev.len());
                let n = self.events.len();
                ev.extend(self.events.drain(..n.min(room)));
                GLOBAL.dropped.fetch_add(self.events.len() as u64, Ordering::Relaxed);
                self.events.clear();
            }
        }
    }

    impl Drop for Local {
        fn drop(&mut self) {
            self.flush();
        }
    }

    thread_local! {
        static LOCAL: RefCell<Local> = RefCell::new(Local {
            tid: u32::try_from(NEXT_TID.fetch_add(1, Ordering::Relaxed)).unwrap_or(u32::MAX),
            counters: [0; NC],
            span_n: [0; NS],
            span_ns: [0; NS],
            events: Vec::new(),
        });
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn now_ns() -> u64 {
        use std::sync::OnceLock;
        use std::time::Instant;
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_nanos()).unwrap_or(u64::MAX)
    }

    #[cfg(target_arch = "wasm32")]
    fn now_ns() -> u64 {
        0
    }

    #[inline]
    pub fn count(c: Counter, n: u64) {
        LOCAL.with(|l| l.borrow_mut().counters[c as usize] += n);
    }

    /// Records a span when dropped.
    pub struct Guard {
        span: Span,
        start: u64,
    }

    #[inline]
    #[must_use]
    pub fn enter(span: Span) -> Guard {
        Guard { span, start: now_ns() }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            let dur = now_ns().saturating_sub(self.start);
            let trace = TRACING.load(Ordering::Relaxed);
            LOCAL.with(|l| {
                let mut l = l.borrow_mut();
                l.span_n[self.span as usize] += 1;
                l.span_ns[self.span as usize] += dur;
                if trace {
                    let tid = l.tid;
                    l.events.push(Event {
                        span: self.span,
                        tid,
                        start_ns: self.start,
                        dur_ns: dur,
                    });
                }
            });
        }
    }

    pub fn set_tracing(on: bool) {
        TRACING.store(on, Ordering::Relaxed);
    }

    pub fn flush_here() {
        LOCAL.with(|l| l.borrow_mut().flush());
    }

    pub fn snapshot() -> Snapshot {
        flush_here();
        Snapshot {
            counters: Counter::ALL
                .iter()
                .map(|&c| (c.name(), GLOBAL.counters[c as usize].load(Ordering::Relaxed)))
                .collect(),
            spans: Span::ALL
                .iter()
                .map(|&s| {
                    (
                        s.name(),
                        GLOBAL.span_n[s as usize].load(Ordering::Relaxed),
                        GLOBAL.span_ns[s as usize].load(Ordering::Relaxed),
                    )
                })
                .collect(),
        }
    }

    pub fn reset() {
        flush_here();
        for a in GLOBAL.counters.iter().chain(&GLOBAL.span_n).chain(&GLOBAL.span_ns) {
            a.store(0, Ordering::Relaxed);
        }
    }

    pub fn clear_trace() {
        flush_here();
        GLOBAL
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
        GLOBAL.dropped.store(0, Ordering::Relaxed);
    }

    pub fn chrome_trace() -> String {
        use std::fmt::Write;
        flush_here();
        let ev = GLOBAL.events.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut s = String::from("{\"traceEvents\":[");
        for (i, e) in ev.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            // Chrome trace times are in microseconds.
            let _ = write!(
                s,
                "{{\"name\":\"{}\",\"ph\":\"X\",\"pid\":1,\"tid\":{},\"ts\":{}.{:03},\"dur\":{}.{:03}}}",
                e.span.name(),
                e.tid,
                e.start_ns / 1000,
                e.start_ns % 1000,
                e.dur_ns / 1000,
                e.dur_ns % 1000
            );
        }
        let _ = write!(
            s,
            "],\"displayTimeUnit\":\"ms\",\"otherData\":{{\"dropped_events\":{}}}}}",
            GLOBAL.dropped.load(Ordering::Relaxed)
        );
        s
    }
}

/// Totals since the last [`reset`] (empty without the `profile` feature).
#[must_use]
pub fn snapshot() -> Snapshot {
    #[cfg(feature = "profile")]
    {
        imp::snapshot()
    }
    #[cfg(not(feature = "profile"))]
    {
        Snapshot::default()
    }
}

/// Adds this thread's counts and span totals to the totals. A thread's
/// counts are buffered locally and added when it exits or calls this, so
/// an executor's worker threads call it at the end of each batch for
/// [`snapshot`] to cover their jobs.
pub fn flush() {
    #[cfg(feature = "profile")]
    imp::flush_here();
}

/// Zeroes all counters and span totals (recorded trace events are kept).
pub fn reset() {
    #[cfg(feature = "profile")]
    imp::reset();
}

/// Drops recorded trace events.
pub fn clear_trace() {
    #[cfg(feature = "profile")]
    imp::clear_trace();
}

/// Starts or stops recording one trace event per span.
pub fn set_tracing(on: bool) {
    #[cfg(feature = "profile")]
    imp::set_tracing(on);
    #[cfg(not(feature = "profile"))]
    let _ = on;
}

/// The recorded events in Chrome trace format (`chrome://tracing`,
/// <https://ui.perfetto.dev>); an empty trace without the `profile` feature.
#[must_use]
pub fn chrome_trace() -> String {
    #[cfg(feature = "profile")]
    {
        imp::chrome_trace()
    }
    #[cfg(not(feature = "profile"))]
    {
        String::from("{\"traceEvents\":[]}")
    }
}

#[cfg(feature = "profile")]
#[doc(hidden)]
pub use imp::{Guard, count as count_impl, enter as enter_impl};

/// Adds to a counter (profile builds only).
macro_rules! count {
    ($c:ident) => {
        $crate::obs::count!($c, 1)
    };
    ($c:ident, $n:expr) => {{
        #[cfg(feature = "profile")]
        $crate::obs::count_impl($crate::obs::Counter::$c, $n as u64);
    }};
}

/// Times the rest of the enclosing block (profile builds only):
/// `span!(Apply);`
macro_rules! span {
    ($s:ident) => {
        #[cfg(feature = "profile")]
        let _obs_guard = $crate::obs::enter_impl($crate::obs::Span::$s);
    };
}

pub(crate) use count;
pub(crate) use span;
