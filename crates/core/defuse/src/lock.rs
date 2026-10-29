//! Every lock in the crate, and the time spent waiting for them.
//!
//! | lock | taken by | held for |
//! |---|---|---|
//! | a segment's `inner` (`segment::Segment`) | a read; a commit; a placement; a split; a `next` filed at its stop; brackets filed | one map operation, or one commit's diff of one node; never two segments at once (but a rebalance's window), never across a run |
//! | a segment's `chain` (`segment::Segment`) | a scope chain found or kept (`read::chain_start`) | an `Arc` clone or swap |
//! | the order's list (`order::Order::segs`) | a position lookup; a split or a new unit | a binary search; one insert |
//! | a field-index shard (`index::FieldIndex`: definitions, contributions, readers) | a read's or a commit's skip | one set lookup or insert |
//! | a range-index shard (`index::RangeIndex`: by field, the range readers by interval, the `At::Final` readers, the environment's readers) | a commit filing its ranges, loop-back and environment reads; a changed contribution asking which hold its label; a loop-back value read, marked, or published at a quiet point; an environment change; a rebalance widening intervals | one commit's ranges of one field; a few map ranges; one list; one value; one shard's intervals |
//! | a value shard (`values::Values`) | an intern; a sequence node lookup | one insert or one lookup; plain reads take none |
//! | paths and their extensions, monoids, environment values, shapes, memo, marks, pending (`engine::Engine`) | setup; rare inserts; a view's step; a push or an edit outside an evaluation | one map operation |
//! | the settling layer's fields (`engine::Engine::settle_exact`) | a settling node's commit or prediction marking a field it has not marked; a read whose field's bit is set | one insert; one lookup |
//! | a node's `last`, `woke`, `text`, `stopped`, `bounded` and `anchored` (`node::Node`) | its commit; its re-ask; an edit; a read of its arguments; a walk stopping at it; a range reader bounded by it filed or taken; a reader anchored at it filed (under that reader's segment's lock) or its removal | a swap; a clone; a push |
//! | a node's claim walk (`node::Node::claim_mx`) | a claim taking or giving back its siblings (`commit::apply_claim`) | one walk from its `claim_end` to the one it stores, each segment's `inner` taken in turn under it; never two nodes' at once, nor under a segment's lock |
//! | an edit source's roots (`sched::Shared::extra_roots`) | the caller's worker coming to a collection's barrier; the collector | one extend; one take |
//! | the lists a collection took while a reader was pinned (`values::Values::graveyard`) | a collection | one take or one extend |
//! | the panicked runs' reads (`sched::Shared::panicked_roots`) | a run that panicked on inputs that held; the collector | one extend; one copy |
//! | the pool's parking (`sched::Pool`) | an idle worker that found no work (looked for outside it); a push that finds one idle | the rest itself; one notify |
//! | a block's steal flag (`sched::Shared::block_busy`, an atomic) | a thief | one steal of an even run of nodes |
//! | the segments of a window of the line in ascending order (`order::Order::rebalance`) | a placement that finds a segment's labels used up | one even relabel of the window, its nodes moved between neighbours: the one time several segments are held, taken while no other lock is held (a field-index shard under them, as a split takes it; the range-index shards after them) |
//! | the spare workers and their handles (`sched::Pool::run`) | the caller's thread, starting the workers an evaluation turns out to need | one spawn |
//! | the probes (`obs`) | a probe, with `profile` | one counter |
//!
//! No lock is held while an operation runs. No thread holds two locks of the
//! same row at once. A lock not in this table is a bug: add the row first.
//!
//! A guard taken at once costs a `try_*`; a refused one tries again a few
//! hundred times before it blocks, and adds its wait to the thread's count, which the pool sums into [`crate::Times::lock_wait_ns`].

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::Instant;

/// A held write guard of a [`Lock`].
pub(crate) type WriteGuard<'a, T> = RwLockWriteGuard<'a, T>;

thread_local! {
    static WAIT_NS: Cell<u64> = const { Cell::new(0) };
    static WAIT_MAX_NS: Cell<u64> = const { Cell::new(0) };
}

pub(crate) struct Lock<T>(RwLock<T>);

impl<T> Lock<T> {
    pub(crate) const fn new(x: T) -> Lock<T> {
        Lock(RwLock::new(x))
    }

    /// A shared guard: `try_read` first, and only a refused try is timed.
    #[cfg_attr(feature = "profile", track_caller)]
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, T> {
        match self.0.try_read() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        for _ in 0..SPIN_TRIES {
            spin();
            match self.0.try_read() {
                Ok(g) => return timed(g, t),
                Err(TryLockError::Poisoned(p)) => return timed(p.into_inner(), t),
                Err(TryLockError::WouldBlock) => {}
            }
        }
        let g = self.0.read().unwrap_or_else(PoisonError::into_inner);
        timed(g, t)
    }

    /// An exclusive guard, timed as `read`.
    #[cfg_attr(feature = "profile", track_caller)]
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, T> {
        match self.0.try_write() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        for _ in 0..SPIN_TRIES {
            spin();
            match self.0.try_write() {
                Ok(g) => return timed(g, t),
                Err(TryLockError::Poisoned(p)) => return timed(p.into_inner(), t),
                Err(TryLockError::WouldBlock) => {}
            }
        }
        let g = self.0.write().unwrap_or_else(PoisonError::into_inner);
        timed(g, t)
    }
}

pub(crate) struct Mx<T>(Mutex<T>);

impl<T> Mx<T> {
    pub(crate) const fn new(x: T) -> Mx<T> {
        Mx(Mutex::new(x))
    }

    /// A guard, timed as `Lock::read`.
    #[cfg_attr(feature = "profile", track_caller)]
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        match self.0.try_lock() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        for _ in 0..SPIN_TRIES {
            spin();
            match self.0.try_lock() {
                Ok(g) => return timed(g, t),
                Err(TryLockError::Poisoned(p)) => return timed(p.into_inner(), t),
                Err(TryLockError::WouldBlock) => {}
            }
        }
        let g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        timed(g, t)
    }

    /// A guard if it is free now, never waited for.
    pub(crate) fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        match self.0.try_lock() {
            Ok(g) => Some(g),
            Err(TryLockError::Poisoned(p)) => Some(p.into_inner()),
            Err(TryLockError::WouldBlock) => None,
        }
    }
}

/// Tries before a refused guard blocks: every hold in the table is short,
/// and a spin longer than a few dozen is a wait by another name (counted as
/// one all the same).
const SPIN_TRIES: u32 = 32;

fn spin() {
    for _ in 0..16 {
        std::hint::spin_loop();
    }
}

#[cfg_attr(feature = "profile", track_caller)]
fn timed<G>(g: G, t: Instant) -> G {
    let w = ns(t);
    waited(w);
    // (`profile`: by the file that took the lock, a probe each at the
    // thread's next take)
    if cfg!(feature = "profile") {
        let f = std::panic::Location::caller().file();
        let i = SITE_FILES
            .iter()
            .position(|s| f.ends_with(s))
            .unwrap_or(SITE_FILES.len());
        SITE_WAITS[i].fetch_add(w, std::sync::atomic::Ordering::Relaxed);
        let line = std::panic::Location::caller().line();
        site_line_add((i as u64) << 32 | u64::from(line), w);
    }
    g
}

/// (`profile`) The lock waits by the file and line that took the lock:
/// open addressing over atomics, the key `file index << 32 | line`.
static SITE_LINES: [(std::sync::atomic::AtomicU64, std::sync::atomic::AtomicU64); 512] = [const {
    (
        std::sync::atomic::AtomicU64::new(0),
        std::sync::atomic::AtomicU64::new(0),
    )
}; 512];

fn site_line_add(key: u64, w: u64) {
    use std::sync::atomic::Ordering::Relaxed;
    let k = key + 1;
    let mut i = (k.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 55) as usize;
    for _ in 0..512 {
        let (kk, ww) = &SITE_LINES[i % 512];
        match kk.compare_exchange(0, k, Relaxed, Relaxed) {
            Ok(_) => {
                ww.fetch_add(w, Relaxed);
                return;
            }
            Err(x) if x == k => {
                ww.fetch_add(w, Relaxed);
                return;
            }
            Err(_) => i += 1,
        }
    }
}

/// (`profile`) The lock waits by file and line, the longest first:
/// `("read.rs:123", ns)`.
#[must_use]
pub fn site_lines() -> Vec<(String, u64)> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut out: Vec<(String, u64)> = SITE_LINES
        .iter()
        .filter_map(|(k, w)| {
            let k = k.load(Relaxed);
            (k != 0).then(|| {
                let k = k - 1;
                let f = SITE_FILES.get((k >> 32) as usize).copied().unwrap_or("other");
                (format!("{f}:{}", k & 0xffff_ffff), w.load(Relaxed))
            })
        })
        .collect();
    out.sort_by_key(|x| std::cmp::Reverse(x.1));
    out
}

/// (`profile`) This thread's CPU time, nanoseconds
/// (`CLOCK_THREAD_CPUTIME_ID`): what a commit spent on the CPU, apart
/// from the time the thread was off it. 0 without `profile` or off unix.
#[allow(unsafe_code)]
pub(crate) fn thread_cpu_ns() -> u64 {
    #[cfg(all(feature = "profile", unix))]
    {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: `t` is a valid, writable timespec for the call.
        if unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &raw mut t) } == 0 {
            return u64::try_from(t.tv_sec)
                .unwrap_or(0)
                .saturating_mul(1_000_000_000)
                .saturating_add(u64::try_from(t.tv_nsec).unwrap_or(0));
        }
    }
    0
}

/// Nanoseconds since `t`.
pub(crate) fn ns(t: Instant) -> u64 {
    u64::try_from(t.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

/// Adds one wait to the thread's count.
pub(crate) fn waited(ns: u64) {
    WAIT_NS.with(|c| c.set(c.get().saturating_add(ns)));
    WAIT_MAX_NS.with(|c| c.set(c.get().max(ns)));
}

/// The thread's lock waits since the last take, (summed, longest), reset.
pub(crate) fn take_wait() -> (u64, u64) {
    let sum = WAIT_NS.with(|c| c.replace(0));
    let max = WAIT_MAX_NS.with(|c| c.replace(0));
    (sum, max)
}

/// (`profile`) The files whose lock waits are summed apart, by the file
/// that took the lock; the rest in one more.
pub(crate) const SITE_FILES: [&str; 10] = [
    "segment.rs",
    "read.rs",
    "commit.rs",
    "index.rs",
    "values.rs",
    "engine.rs",
    "sched.rs",
    "node.rs",
    "ctx.rs",
    "table.rs",
];

/// (`profile`) The lock waits by [`SITE_FILES`], nanoseconds.
pub(crate) static SITE_WAITS: [std::sync::atomic::AtomicU64; 11] = [const { std::sync::atomic::AtomicU64::new(0) }; 11];
