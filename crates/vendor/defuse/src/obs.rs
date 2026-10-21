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

use std::fmt;

/// What a counter counts.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
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
}

impl Counter {
    pub const ALL: [Counter; 26] = [
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
        }
    }
}

/// A timed region.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
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

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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

/// Adds the time since `.1` to the span `.0` when dropped (profiling).
#[cfg(feature = "profile")]
pub struct Ticked(pub &'static str, pub std::time::Instant);

#[cfg(feature = "profile")]
impl Drop for Ticked {
    fn drop(&mut self) {
        #[allow(clippy::cast_possible_truncation)]
        add_impl(self.0, self.1.elapsed().as_nanos() as u64);
    }
}

/// Whether probes are compiled in.
#[must_use]
pub const fn enabled() -> bool {
    cfg!(feature = "profile")
}
#[cfg(feature = "profile")]
mod imp {
    use std::cell::RefCell;
    use std::collections::BTreeMap;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    #[derive(Default)]
    struct Local {
        counters: BTreeMap<&'static str, u64>,
        spans: BTreeMap<&'static str, (u64, u64)>,
        events: Vec<(&'static str, u64, u64)>,
    }

    struct Global {
        counters: BTreeMap<&'static str, u64>,
        spans: BTreeMap<&'static str, (u64, u64)>,
        events: Vec<(&'static str, u64, u64, u64)>,
    }

    static GLOBAL: Mutex<Global> = Mutex::new(Global {
        counters: BTreeMap::new(),
        spans: BTreeMap::new(),
        events: Vec::new(),
    });
    static TRACING: AtomicBool = AtomicBool::new(false);

    thread_local! {
        static LAST_FLUSH: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
        static LOCAL: RefCell<Local> = RefCell::new(Local::default());
    }

    fn epoch() -> &'static Instant {
        static E: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
        E.get_or_init(Instant::now)
    }

    pub fn count(c: super::Counter, n: u64) {
        LOCAL.with(|l| *l.borrow_mut().counters.entry(c.name()).or_default() += n);
    }

    /// Records a span when dropped.
    pub struct Guard {
        name: &'static str,
        t: Instant,
    }

    #[must_use]
    pub fn enter(span: super::Span) -> Guard {
        Guard {
            name: span.name(),
            t: Instant::now(),
        }
    }

    impl Drop for Guard {
        fn drop(&mut self) {
            #[allow(clippy::cast_possible_truncation)]
            let ns = self.t.elapsed().as_nanos() as u64;
            #[allow(clippy::cast_possible_truncation)]
            let start = self.t.duration_since(*epoch()).as_nanos() as u64;
            LOCAL.with(|l| {
                let mut l = l.borrow_mut();
                let e = l.spans.entry(self.name).or_default();
                e.0 += ns;
                e.1 += 1;
                if TRACING.load(Ordering::Relaxed) {
                    l.events.push((self.name, start, ns));
                }
            });
            // Merged every millisecond or so, not per span: a global lock
            // per span would add the contention it measures (workers
            // flush when they leave, `snapshot` flushes its own thread).
            let now = start + ns;
            if LAST_FLUSH.with(|c| now.saturating_sub(c.get()) >= 1_000_000) {
                LAST_FLUSH.with(|c| c.set(now));
                flush();
            }
        }
    }

    /// Adds `ns` to the span named `name` (flushed with the next span).
    pub fn add(name: &'static str, ns: u64) {
        LOCAL.with(|l| {
            let mut l = l.borrow_mut();
            let e = l.spans.entry(name).or_default();
            e.0 += ns;
            e.1 += 1;
        });
    }

    /// Records a blocked lock acquisition at `loc` as the span
    /// `lock <file>:<line>`.
    pub fn lock_wait(loc: &'static std::panic::Location<'static>, ns: u64) {
        static NAMES: Mutex<BTreeMap<(&'static str, u32), &'static str>> = Mutex::new(BTreeMap::new());
        let name = *NAMES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .entry((loc.file(), loc.line()))
            .or_insert_with(|| Box::leak(format!("lock {}:{}", loc.file(), loc.line()).into_boxed_str()));
        add(name, ns);
    }

    pub fn flush() {
        let tid = thread_id();
        LOCAL.with(|l| {
            let mut l = l.borrow_mut();
            if l.counters.is_empty() && l.spans.is_empty() && l.events.is_empty() {
                return;
            }
            let mut g = GLOBAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            for (k, v) in std::mem::take(&mut l.counters) {
                *g.counters.entry(k).or_default() += v;
            }
            for (k, (a, b)) in std::mem::take(&mut l.spans) {
                let e = g.spans.entry(k).or_default();
                e.0 += a;
                e.1 += b;
            }
            for (n, s, d) in std::mem::take(&mut l.events) {
                g.events.push((n, s, d, tid));
            }
        });
    }

    fn thread_id() -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        std::thread::current().id().hash(&mut h);
        h.finish() % 1000
    }

    pub fn reset_totals() {
        flush();
        let mut g = GLOBAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        g.counters.clear();
        g.spans.clear();
    }

    pub fn snapshot() -> super::Snapshot {
        flush();
        let g = GLOBAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Spans outside `Span::ALL` (lock waits by call site, worker
        // idling), largest first.
        let mut extra: Vec<(&'static str, u64, u64)> = g
            .spans
            .iter()
            .filter(|(k, _)| !super::Span::ALL.iter().any(|sp| sp.name() == **k))
            .map(|(k, (ns, n))| (*k, *n, *ns))
            .collect();
        extra.sort_by_key(|e| std::cmp::Reverse(e.2));
        let mut snap = super::Snapshot {
            counters: super::Counter::ALL
                .iter()
                .map(|c| (c.name(), g.counters.get(c.name()).copied().unwrap_or(0)))
                .collect(),
            spans: super::Span::ALL
                .iter()
                .map(|sp| {
                    let (ns, n) = g.spans.get(sp.name()).copied().unwrap_or((0, 0));
                    (sp.name(), n, ns)
                })
                .collect(),
        };
        snap.spans.extend(extra);
        snap
    }

    pub fn clear_trace() {
        flush();
        GLOBAL
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .events
            .clear();
    }

    pub fn set_tracing(on: bool) {
        TRACING.store(on, Ordering::Relaxed);
    }

    pub fn chrome_trace() -> String {
        use std::fmt::Write as _;
        flush();
        let g = GLOBAL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut s = String::from("{\"traceEvents\":[");
        for (i, (n, st, d, tid)) in g.events.iter().enumerate() {
            if i > 0 {
                s.push(',');
            }
            let _ = write!(
                s,
                "{{\"name\":\"{n}\",\"ph\":\"X\",\"ts\":{},\"dur\":{},\"pid\":1,\"tid\":{tid}}}",
                st / 1000,
                d / 1000
            );
        }
        s.push_str("]}");
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
    imp::flush();
}

/// Zeroes all counters and span totals (recorded trace events are kept).
pub fn reset() {
    #[cfg(feature = "profile")]
    imp::reset_totals();
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
        "{\"traceEvents\":[]}".into()
    }
}

#[cfg(feature = "profile")]
#[doc(hidden)]
pub use imp::{Guard, add as add_impl, count as count_impl, enter as enter_impl, lock_wait};
