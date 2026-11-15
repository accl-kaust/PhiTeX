//! Probes: counters and spans, compiled in with `profile`, no-ops without.

use crate::lock::Mx;

/// Whether probes are compiled in.
#[must_use]
pub fn enabled() -> bool {
    cfg!(feature = "profile")
}

/// A snapshot of the probes.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub counters: Vec<(&'static str, u64)>,
    /// (name, nanoseconds, count).
    pub spans: Vec<(&'static str, u64, u64)>,
    pub lock_wait_ns: u64,
    /// The lock waits by the engine's file that took the lock, ns.
    pub lock_sites: Vec<(&'static str, u64)>,
}

struct Probe {
    name: &'static str,
    count: u64,
    ns: u64,
    span: bool,
}

static PROBES: Mx<Vec<Probe>> = Mx::new(Vec::new());

fn with_probe(name: &'static str, span: bool, f: impl FnOnce(&mut Probe)) {
    if !enabled() {
        return;
    }
    let mut ps = PROBES.lock();
    match ps.iter_mut().find(|p| p.name == name && p.span == span) {
        Some(p) => f(p),
        None => {
            let mut p = Probe {
                name,
                count: 0,
                ns: 0,
                span,
            };
            f(&mut p);
            ps.push(p);
        }
    }
}

pub fn snapshot() -> Snapshot {
    if !enabled() {
        return Snapshot::default();
    }
    let ps = PROBES.lock();
    Snapshot {
        counters: ps.iter().filter(|p| !p.span).map(|p| (p.name, p.count)).collect(),
        spans: ps.iter().filter(|p| p.span).map(|p| (p.name, p.ns, p.count)).collect(),
        lock_wait_ns: ps.iter().filter(|p| p.name == "lock_wait").map(|p| p.ns).sum(),
        lock_sites: crate::lock::SITE_WAITS
            .iter()
            .enumerate()
            .map(|(i, w)| {
                (
                    crate::lock::SITE_FILES.get(i).copied().unwrap_or("other"),
                    w.load(std::sync::atomic::Ordering::Relaxed),
                )
            })
            .collect(),
    }
}

/// Prints the snapshot to stderr when `PHITEX_DEFUSE_PROF` is set.
pub fn flush() {
    if !enabled() || std::env::var_os("PHITEX_DEFUSE_PROF").is_none() {
        return;
    }
    let s = snapshot();
    for (name, n) in &s.counters {
        eprintln!("defuse-prof: {name} {n}");
    }
    for (name, ns, n) in &s.spans {
        eprintln!("defuse-prof: {name} {:.3} ms over {n}", *ns as f64 / 1e6);
    }
    for (f, w) in &s.lock_sites {
        eprintln!("defuse-prof: lock wait taken in {f} {:.3} ms", *w as f64 / 1e6);
    }
}

/// The refuted predictions by field and by why (`profile`, with
/// `PHITEX_PRED_MISSES` set): another value, or the same value at another
/// depth. Printed by [`flush`], the most first.
static PRED_MISSES: Mx<Vec<(crate::Field, bool, u64)>> = Mx::new(Vec::new());

fn pred_misses_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    enabled() && *ON.get_or_init(|| std::env::var_os("PHITEX_PRED_MISSES").is_some())
}

/// A prediction of field `f` refuted at commit: by an equal value at
/// another depth (`same_value`), else by another value.
pub(crate) fn pred_miss(f: crate::Field, same_value: bool) {
    if !pred_misses_on() {
        return;
    }
    let mut m = PRED_MISSES.lock();
    match m.iter_mut().find(|e| e.0 == f && e.1 == same_value) {
        Some(e) => e.2 += 1,
        None => m.push((f, same_value, 1)),
    }
}

/// Prints the refuted predictions by field ([`pred_miss`]) with the
/// fields' names from `v`, the most first, and clears them.
pub fn flush_pred_misses(v: &crate::Values) {
    if !pred_misses_on() {
        return;
    }
    let mut m = std::mem::take(&mut *PRED_MISSES.lock());
    m.sort_by_key(|e| std::cmp::Reverse(e.2));
    let total: u64 = m.iter().map(|e| e.2).sum();
    eprintln!("pred-misses: {total} refuted predictions by field");
    for (f, same, n) in m.iter().take(40) {
        eprintln!(
            "pred-miss: {n} {} {}",
            v.render(f.0),
            if *same { "depth" } else { "value" }
        );
    }
}

pub fn reset() {
    if enabled() {
        PROBES.lock().clear();
        for w in &crate::lock::SITE_WAITS {
            w.store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Adds `n` to counter `name`.
pub fn count(name: &'static str, n: u64) {
    with_probe(name, false, |p| p.count += n);
}

/// Adds `ns` to span `name`.
pub fn span(name: &'static str, ns: u64) {
    with_probe(name, true, |p| {
        p.ns += ns;
        p.count += 1;
    });
}

/// (`profile`) The lock waits by the file and line that took the lock,
/// the longest first.
#[must_use]
pub fn lock_lines() -> Vec<(String, u64)> {
    crate::lock::site_lines()
}

/// (`profile`) A commit's time under its segment's lock by part, reported
/// for one that held it past a millisecond.
pub(crate) struct Laps {
    #[cfg(feature = "profile")]
    t: std::time::Instant,
    /// (in place: a lap is taken under the segment's lock, where nothing
    /// is allocated)
    #[cfg(feature = "profile")]
    parts: [(&'static str, u64); LAPS],
    #[cfg(feature = "profile")]
    n: usize,
}

/// The laps a commit takes.
#[cfg(feature = "profile")]
const LAPS: usize = 8;

impl Laps {
    #[allow(unused_variables)]
    pub(crate) fn new(t: std::time::Instant) -> Laps {
        Laps {
            #[cfg(feature = "profile")]
            t,
            #[cfg(feature = "profile")]
            parts: [("", 0); LAPS],
            #[cfg(feature = "profile")]
            n: 0,
        }
    }

    #[allow(unused_variables, clippy::unused_self)]
    pub(crate) fn lap(&mut self, name: &'static str) {
        #[cfg(feature = "profile")]
        {
            let now = std::time::Instant::now();
            if let Some(p) = self.parts.get_mut(self.n) {
                *p = (
                    name,
                    u64::try_from(now.duration_since(self.t).as_nanos()).unwrap_or(u64::MAX),
                );
                self.n += 1;
            }
            self.t = now;
        }
    }

    #[allow(unused_variables, clippy::unused_self)]
    pub(crate) fn report(&self, node: u64, ns: u64, defs: usize, reads: usize) {
        #[cfg(feature = "profile")]
        if ns > 1_000_000 {
            let parts: Vec<String> = self.parts[..self.n]
                .iter()
                .map(|(n, t)| format!("{n} {:.3}", *t as f64 / 1e6))
                .collect();
            eprintln!(
                "commit-laps: node {node} {:.3} ms ({defs} defs, {reads} reads): {}",
                ns as f64 / 1e6,
                parts.join(", ")
            );
        }
    }
}

/// (`profile`) A hold of segment locks past a millisecond, reported:
/// what held them and for how long.
#[allow(unused_variables)]
pub(crate) fn long_hold(what: &str, n: usize, t: std::time::Instant) {
    #[cfg(feature = "profile")]
    {
        let ns = t.elapsed().as_nanos();
        if ns > 1_000_000 {
            eprintln!("long-hold: {what} ({n}) {:.3} ms", ns as f64 / 1e6);
        }
    }
}

/// (`PHITEX_LINE_DUMP=<n>`, profile, with `PHITEX_WAKE_LOG`) Every `n`
/// wakes, the line: each live node in position order with its position,
/// id, operation, state, arguments, and its definitions of the fields
/// named in `PHITEX_LINE_FIELDS` (comma-separated).
pub(crate) fn maybe_dump_line(eng: &crate::engine::Engine) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static EVERY: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let every = *EVERY.get_or_init(|| {
        std::env::var("PHITEX_LINE_DUMP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0)
    });
    if every == 0 {
        return;
    }
    let c = COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    if !c.is_multiple_of(every) {
        return;
    }
    let fields: Vec<String> = std::env::var("PHITEX_LINE_FIELDS")
        .map(|v| v.split(',').map(str::to_owned).collect())
        .unwrap_or_default();
    let v = eng.values();
    let fs: Vec<crate::values::Field> = fields.iter().map(|f| v.field(f)).collect();
    let mut out = format!("line-dump #{c}:\n");
    for sid in eng.order.all() {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        for (pos, nid) in &inner.nodes {
            let n = eng.node(*nid);
            let op = eng.ops.get(n.op().0 as usize).map_or("?", |o| o.name());
            let args: Vec<String> = n.args().iter().map(|&a| v.render(a)).collect();
            let mut args = args.join(" ");
            if args.len() > 90 {
                args.truncate(90);
                args.push('…');
            }
            let mut defs = String::new();
            for (name, f) in fields.iter().zip(&fs) {
                if let Some(m) = inner.defs.get(f)
                    && let Some(d) = m.get(pos)
                {
                    let mut val = v.render(d.value);
                    if val.len() > 60 {
                        val.truncate(60);
                        val.push('…');
                    }
                    defs.push_str(&format!(" {name}={val}{}", if d.provisional { "(prov)" } else { "" }));
                }
            }
            out.push_str(&format!(
                "  {:?} seg {:?} node {} {op} {:?} parent {:?} anchor {:?} slot {} [{args}]{defs}\n",
                pos.0,
                sid,
                nid.raw(),
                n.state(),
                n.parent.map(crate::api::NodeId::raw),
                n.anchor().map(crate::api::NodeId::raw),
                n.slot()
            ));
        }
    }
    eprint!("{out}");
}

/// (`profile`) A binary's global allocator that counts what a commit
/// allocates and frees under its segment's lock (`Section`): `A`'s
/// (the system's, `CountingAlloc(std::alloc::System)`, or the CLI's
/// jemalloc), counting on the threads in a section. A test binary
/// installs it (`#[global_allocator]`); [`crate::Times::commit_allocs`]
/// is 0 in one that does not ([`counting_allocs`]). With
/// `PHITEX_ALLOC_TRACE=n` set, one in every `n` counted (1 if not a
/// number) prints its backtrace to stderr.
#[cfg(feature = "profile")]
pub struct CountingAlloc<A>(pub A);

#[cfg(feature = "profile")]
mod counting {
    use std::alloc::{GlobalAlloc, Layout};
    use std::cell::Cell;
    use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

    thread_local! {
        /// In a section, and not in a trace of one.
        static IN: Cell<bool> = const { Cell::new(false) };
        static ALLOCS: Cell<u64> = const { Cell::new(0) };
        static FREES: Cell<u64> = const { Cell::new(0) };
        /// In a trace of any allocation (`PHITEX_ALLOC_ALL`).
        static TRACING: Cell<bool> = const { Cell::new(false) };
    }

    /// `PHITEX_ALLOC_ALL=n`: one in every `n` allocations anywhere prints
    /// its backtrace (`any-alloc:`); 0 off. Read at the first commit.
    static ALL: AtomicU64 = AtomicU64::new(0);
    /// Every allocation and free since, with `ALL` on.
    pub(super) static ALL_SEEN: AtomicU64 = AtomicU64::new(0);
    pub(super) static ALL_FREES: AtomicU64 = AtomicU64::new(0);

    /// Whether the binary's allocator is [`super::CountingAlloc`].
    pub(super) static INSTALLED: AtomicBool = AtomicBool::new(false);
    /// `PHITEX_ALLOC_TRACE`: 0 unread, 1 off, 2 on.
    static TRACE: AtomicU8 = AtomicU8::new(0);
    /// Its period, and those counted so far.
    static PERIOD: AtomicU64 = AtomicU64::new(1);
    static SEEN: AtomicU64 = AtomicU64::new(0);

    fn note(c: &'static std::thread::LocalKey<Cell<u64>>, what: &str) {
        if !INSTALLED.load(Ordering::Relaxed) {
            INSTALLED.store(true, Ordering::Relaxed);
        }
        let all = ALL.load(Ordering::Relaxed);
        if all > 0 && !TRACING.try_with(Cell::get).unwrap_or(true) {
            if what == "free" {
                ALL_FREES.fetch_add(1, Ordering::Relaxed);
            } else if ALL_SEEN.fetch_add(1, Ordering::Relaxed).is_multiple_of(all) {
                TRACING.with(|t| t.set(true));
                eprintln!("any-alloc: {what} at\n{}", std::backtrace::Backtrace::force_capture());
                TRACING.with(|t| t.set(false));
            }
        }
        if IN.try_with(Cell::get).unwrap_or(false) {
            let _ = c.try_with(|c| c.set(c.get() + 1));
            if TRACE.load(Ordering::Relaxed) == 2
                && SEEN
                    .fetch_add(1, Ordering::Relaxed)
                    .is_multiple_of(PERIOD.load(Ordering::Relaxed))
            {
                IN.with(|i| i.set(false));
                eprintln!(
                    "commit-alloc: {what} under a commit's lock at\n{}",
                    std::backtrace::Backtrace::force_capture()
                );
                IN.with(|i| i.set(true));
            }
        }
    }

    #[allow(unsafe_code)]
    // SAFETY: every call goes to `System` with the caller's arguments.
    unsafe impl<A: GlobalAlloc> GlobalAlloc for super::CountingAlloc<A> {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            note(&ALLOCS, "alloc");
            // SAFETY: the caller's contract, passed on.
            unsafe { self.0.alloc(l) }
        }

        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            note(&ALLOCS, "alloc");
            // SAFETY: the caller's contract, passed on.
            unsafe { self.0.alloc_zeroed(l) }
        }

        unsafe fn realloc(&self, p: *mut u8, l: Layout, new: usize) -> *mut u8 {
            note(&ALLOCS, "realloc");
            // SAFETY: the caller's contract, passed on.
            unsafe { self.0.realloc(p, l, new) }
        }

        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            note(&FREES, "free");
            // SAFETY: the caller's contract, passed on.
            unsafe { self.0.dealloc(p, l) }
        }
    }

    /// Enters a section; `PHITEX_ALLOC_TRACE` read once, before.
    pub(super) fn enter() {
        if TRACE.load(Ordering::Relaxed) == 0 {
            let on = std::env::var("PHITEX_ALLOC_TRACE").ok();
            if let Some(n) = on.as_deref().and_then(|x| x.parse::<u64>().ok()) {
                PERIOD.store(n.max(1), Ordering::Relaxed);
            }
            TRACE.store(if on.is_some() { 2 } else { 1 }, Ordering::Relaxed);
            let all = std::env::var("PHITEX_ALLOC_ALL")
                .ok()
                .and_then(|x| x.parse::<u64>().ok());
            ALL.store(all.unwrap_or(0), Ordering::Relaxed);
        }
        IN.with(|i| i.set(true));
    }

    /// Out of the section; what it counted stays for `take`.
    pub(super) fn out() {
        let _ = IN.try_with(|i| i.set(false));
    }

    /// What the sections allocated and freed since the last take.
    pub(super) fn take() -> (u64, u64) {
        (ALLOCS.with(|c| c.replace(0)), FREES.with(|c| c.replace(0)))
    }
}

/// (`profile`) A commit's section under its segment's lock, until it is
/// dropped: what it allocates and frees there is counted by
/// [`CountingAlloc`], if the binary installs it, and taken by
/// [`section_take`]. Nothing without `profile`.
pub(crate) struct Section;

impl Section {
    pub(crate) fn enter() -> Section {
        #[cfg(feature = "profile")]
        counting::enter();
        Section
    }
}

impl Drop for Section {
    fn drop(&mut self) {
        #[cfg(feature = "profile")]
        counting::out();
    }
}

/// (allocations, frees) of this thread's sections since the last take
/// (a commit's tries that let the lock go and took it again included);
/// (0, 0) without `profile`.
pub(crate) fn section_take() -> (u64, u64) {
    #[cfg(feature = "profile")]
    return counting::take();
    #[cfg(not(feature = "profile"))]
    (0, 0)
}

/// (`profile`, `PHITEX_ALLOC_ALL`) The allocations and frees anywhere
/// since the first commit: (0, 0) unless on.
#[must_use]
pub fn all_allocs() -> (u64, u64) {
    #[cfg(feature = "profile")]
    return (
        counting::ALL_SEEN.load(std::sync::atomic::Ordering::Relaxed),
        counting::ALL_FREES.load(std::sync::atomic::Ordering::Relaxed),
    );
    #[cfg(not(feature = "profile"))]
    (0, 0)
}

/// (`profile`) Whether the binary's allocator counts a commit's
/// allocations ([`CountingAlloc`]): known after a commit.
#[must_use]
pub fn counting_allocs() -> bool {
    #[cfg(feature = "profile")]
    return counting::INSTALLED.load(std::sync::atomic::Ordering::Relaxed);
    #[cfg(not(feature = "profile"))]
    false
}

/// (`PHITEX_INTERMED_LOG`, profile) Whether the intermediate-rerun probe
/// prints: every wake delivered (`imw`), every run with its wake and its
/// reads (`imr`), every commit's changed fields (`imc`), and the fields'
/// names once each (`imf`); docs/note-intermediate-reruns.md. Observe only.
pub(crate) fn im_on() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    // (`enabled` first: a constant, so a build without probes has none)
    enabled() && *F.get_or_init(|| std::env::var_os("PHITEX_INTERMED_LOG").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// (`PHITEX_INTERMED_LOG`) The key of the line's structure in
/// [`im_changed`]: no field's raw id.
pub(crate) const IM_STRUCTURE: u64 = u64::MAX;

/// (`PHITEX_INTERMED_LOG`) The node whose commit last moved each field's
/// definitions or contributions, or the line's structure
/// ([`IM_STRUCTURE`]): raw id plus one.
static IM_CHANGERS: Mx<Option<std::collections::HashMap<u64, u64>>> = Mx::new(None);

/// (`PHITEX_INTERMED_LOG`) The node committing on this thread moved `key`
/// (a field's raw id, or [`IM_STRUCTURE`]).
pub(crate) fn im_changed(key: u64) {
    if !im_on() {
        return;
    }
    let by = im_committing_now();
    if by == 0 {
        return;
    }
    IM_CHANGERS.lock().get_or_insert_with(Default::default).insert(key, by);
}

/// (`PHITEX_INTERMED_LOG`) The node whose commit last moved `key` (raw id
/// plus one, 0 none).
pub(crate) fn im_changer(key: u64) -> u64 {
    if !im_on() {
        return 0;
    }
    IM_CHANGERS
        .lock()
        .as_ref()
        .and_then(|m| m.get(&key).copied())
        .unwrap_or(0)
}

thread_local! {
    /// (`PHITEX_INTERMED_LOG`) The node whose commit moved what a wake made
    /// on this thread with no node of its own is for (raw id plus one, 0
    /// none): a commit's check of its own reads re-asks the run with no
    /// `by`, and the log names the definer that moved the read.
    static IM_HINT: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// (`PHITEX_INTERMED_LOG`) How this thread's worker took the node it
    /// runs next, and from which worker's queue.
    static IM_TOOK: std::cell::Cell<(&'static str, usize)> = const { std::cell::Cell::new(("-", 0)) };
}

thread_local! {
    /// (`PHITEX_INTERMED_LOG`) Where this thread's worker is, outside a run
    /// and a commit: a quiet point, an idle worker's release, an edit taken.
    static IM_PLACE: std::cell::Cell<&'static str> = const { std::cell::Cell::new("-") };
}

/// (`PHITEX_INTERMED_LOG`) Sets where this thread is ([`IM_PLACE`]); the
/// place before.
pub(crate) fn im_place(p: &'static str) -> &'static str {
    if !im_on() {
        return "-";
    }
    IM_PLACE.with(|c| c.replace(p))
}

/// (`PHITEX_INTERMED_LOG`) Where this thread is.
pub(crate) fn im_place_now() -> &'static str {
    IM_PLACE.with(std::cell::Cell::get)
}

/// (`PHITEX_INTERMED_LOG`) Sets the wake hint ([`IM_HINT`]); the one before.
pub(crate) fn im_hint(n: u64) -> u64 {
    if !im_on() {
        return 0;
    }
    IM_HINT.with(|c| c.replace(n))
}

/// (`PHITEX_INTERMED_LOG`) The wake hint.
pub(crate) fn im_hint_now() -> u64 {
    IM_HINT.with(std::cell::Cell::get)
}

/// (`PHITEX_INTERMED_LOG`) The node taken next on this thread was taken
/// `how`, from worker `from`'s queues.
pub(crate) fn im_took(how: &'static str, from: usize) {
    if im_on() {
        IM_TOOK.with(|c| c.set((how, from)));
    }
}

/// (`PHITEX_INTERMED_LOG`) How the node at work on this thread was taken.
pub(crate) fn im_took_now() -> (&'static str, usize) {
    IM_TOOK.with(std::cell::Cell::get)
}

/// (`PHITEX_INTERMED_LOG`) Field `f`'s raw id, its name printed once.
pub(crate) fn im_field(eng: &crate::engine::Engine, f: crate::values::Field) -> u64 {
    static SEEN: Mx<Option<std::collections::HashSet<u64>>> = Mx::new(None);
    let raw = f.0.raw();
    let new = SEEN.lock().get_or_insert_with(Default::default).insert(raw);
    if new {
        elog!(
            "imf\t{raw}\t{}",
            eng.values().as_str(f.0).unwrap_or_default().replace(['\t', '\n'], " ")
        );
    }
    raw
}

/// (`PHITEX_INTERMED_LOG`) A cause as `kind field by`.
pub(crate) fn im_cause(eng: &crate::engine::Engine, c: Option<&crate::api::Cause>) -> String {
    use crate::api::Cause;
    let Some(c) = c else { return "none 0 0".to_owned() };
    let (k, f, by) = match c {
        Cause::Rebind { field, to, .. } => ("Rebind", Some(*field), *to),
        Cause::Changed { field, site } => ("Changed", Some(*field), *site),
        Cause::Range { field, by } => ("Range", Some(*field), Some(*by)),
        Cause::Environment { field } => ("Environment", Some(*field), None),
        Cause::Output { of } => ("Output", None, Some(*of)),
        Cause::Structure => ("Structure", None, None),
        Cause::Created { by } => ("Created", None, *by),
        Cause::Edited => ("Edited", None, None),
    };
    format!(
        "{k} {} {}",
        f.map_or(0, |f| im_field(eng, f)),
        by.map_or(0, |b| b.raw() + 1)
    )
}

thread_local! {
    /// (`PHITEX_INTERMED_LOG`) The node whose commit this thread makes
    /// (raw id plus one, 0 none).
    static IM_COMMITTING: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// (`PHITEX_INTERMED_LOG`) Sets the node whose commit this thread makes;
/// the one before.
pub(crate) fn im_committing(n: u64) -> u64 {
    if !im_on() {
        return 0;
    }
    IM_COMMITTING.with(|c| c.replace(n))
}

/// (`PHITEX_INTERMED_LOG`) The node whose commit this thread makes.
pub(crate) fn im_committing_now() -> u64 {
    IM_COMMITTING.with(std::cell::Cell::get)
}

/// (`PHITEX_INTERMED_LOG`, profile, Linux) This thread's hardware counts
/// since its first call, `[cycles, instructions, cache references, cache
/// misses]`, user and kernel, from one perf group opened per thread
/// (`perf_event_open`, pid 0, any cpu); zeros where the kernel refuses it
/// (`perf_event_paranoid` above 2). Observe only: a run's cost split into
/// work (instructions), memory (misses, IPC) and clock (cycles per ns).
#[allow(unsafe_code)]
pub(crate) fn hw() -> [u64; 4] {
    #[cfg(all(feature = "profile", target_os = "linux"))]
    {
        thread_local! {
            static FD: std::cell::Cell<i32> = const { std::cell::Cell::new(-2) };
        }
        fn open(config: u64, group: i32) -> i32 {
            // perf_event_attr, VER5 (112 bytes), in a zeroed buffer
            let mut a = [0u64; 16];
            // type PERF_TYPE_HARDWARE (0), size 112
            a[0] = 112u64 << 32;
            a[1] = config;
            // read_format: PERF_FORMAT_GROUP (8)
            a[4] = 8;
            // flags: exclude_hv (bit 6)
            a[5] = 1 << 6;
            // SAFETY: a valid attr buffer for the syscall's lifetime.
            unsafe { libc::syscall(libc::SYS_perf_event_open, a.as_ptr(), 0i32, -1i32, group, 0u64) as i32 }
        }
        let fd = FD.with(|c| {
            if c.get() == -2 {
                let l = open(0, -1);
                if l >= 0 {
                    for k in 1..4 {
                        open(k, l);
                    }
                }
                c.set(l);
            }
            c.get()
        });
        if fd < 0 {
            return [0; 4];
        }
        let mut b = [0u64; 5];
        // SAFETY: reading the group's values into a buffer of 5 u64.
        let n = unsafe { libc::read(fd, b.as_mut_ptr().cast(), 40) };
        if n == 40 {
            return [b[1], b[2], b[3], b[4]];
        }
    }
    [0; 4]
}

/// (`PHITEX_INTERMED_LOG`, profile, Linux) The CPU this thread is on now
/// (`sched_getcpu`), -1 elsewhere.
#[allow(unsafe_code)]
pub(crate) fn cpu_now() -> i32 {
    #[cfg(all(feature = "profile", target_os = "linux"))]
    // SAFETY: no arguments, no memory.
    return unsafe { libc::sched_getcpu() };
    #[cfg(not(all(feature = "profile", target_os = "linux")))]
    -1
}

/// (`PHITEX_RUNCOST`, profile) Whether each node's last run with products
/// keeps its cost (`Node::cost`), summed by operation at the evaluation's
/// end: the kept runs' CPU, hardware counts and lock waits, with no line
/// printed per run (the per-run logs' own stderr lock and writes slowed a
/// -j8 build 4x).
pub(crate) fn rc_on() -> bool {
    static F: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    enabled() && *F.get_or_init(|| std::env::var_os("PHITEX_RUNCOST").is_some_and(|v| !v.is_empty() && v != "0"))
}

/// (`PHITEX_RUNCOST`) The kept runs' costs by operation, printed and
/// taken: `runcost\\top\\tnodes\\tcpu cycles instr cref cmiss lockwait reads wall runs`.
pub(crate) fn runcost_report(eng: &crate::engine::Engine) {
    #[cfg(not(feature = "profile"))]
    let _ = eng;
    #[cfg(feature = "profile")]
    {
        let mut by: std::collections::BTreeMap<&'static str, (u64, [u64; 9])> = Default::default();
        for i in 0..eng.nodes.len() {
            let Some(n) = eng.nodes.get(i) else { continue };
            if let Some((op, c)) = n.cost.lock().take() {
                let e = by.entry(op).or_default();
                e.0 += 1;
                for (a, b) in e.1.iter_mut().zip(c) {
                    *a += b;
                }
            }
        }
        for (op, (k, c)) in by {
            eprintln!(
                "runcost\t{op}\t{k}\t{}",
                c.iter().map(u64::to_string).collect::<Vec<_>>().join(" ")
            );
        }
    }
}

/// (profile) The per-run logs' lines, kept per thread and written at the
/// evaluation's end (or past 8 MB), not one `eprintln` each: the stderr
/// lock and a write per line made a logged -j8 build 4x slower and its
/// runs' clocks low (`log_line`).
static LOG_BUFS: Mx<Vec<std::sync::Arc<std::sync::Mutex<String>>>> = Mx::new(Vec::new());

thread_local! {
    static LOG_BUF: std::sync::Arc<std::sync::Mutex<String>> = {
        let b = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        LOG_BUFS.lock().push(b.clone());
        b
    };
}

/// (profile) One log line into this thread's buffer, while the per-run
/// logs are on (`PHITEX_RUN_LOG`, `PHITEX_INTERMED_LOG`); else to stderr
/// at once, as `eprintln` (a stuck build's dump, the wake log).
pub(crate) fn log_line(args: std::fmt::Arguments<'_>) {
    use std::fmt::Write as _;
    if !(im_on() || crate::sched::run_log()) {
        eprintln!("{args}");
        return;
    }
    LOG_BUF.with(|b| {
        let mut s = b.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = s.write_fmt(args);
        s.push('\n');
        if s.len() > 8 << 20 {
            use std::io::Write as _;
            let _ = std::io::stderr().lock().write_all(s.as_bytes());
            s.clear();
        }
    });
}

/// (profile) Every thread's buffered lines, written.
pub(crate) fn log_flush() {
    use std::io::Write as _;
    let bufs = LOG_BUFS.lock().clone();
    let mut e = std::io::stderr().lock();
    for b in bufs {
        let mut s = b.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = e.write_all(s.as_bytes());
        s.clear();
    }
}
