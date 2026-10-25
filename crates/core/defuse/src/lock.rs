//! Every lock in the crate, and the time spent waiting for them.
//!
//! | lock | taken by | held for |
//! |---|---|---|
//! | a segment's `inner` (`segment::Segment`) | a read; a commit; a placement; a split | one map operation, or one commit's diff of one node; never two segments at once, never across a run |
//! | the order's list (`order::Order::segs`) | a position lookup; a split or a new unit | a binary search; one insert |
//! | a field-index shard (`index::FieldIndex`: definitions, contributions, readers) | a read's or a commit's skip | one set lookup or insert |
//! | a value shard (`values::Values`) | an intern; a sequence node lookup | one insert or one lookup; plain reads take none |
//! | paths, monoids, environment, shapes, memo, marks, pending (`engine::Engine`) | setup; rare inserts; a push or an edit outside an evaluation | one map operation |
//! | a node's `last`, `woke` and `text` (`node::Node`) | its commit; its re-ask; an edit; a read of its arguments | a swap; a clone |
//! | the pool's parking (`sched::Pool`) | an idle worker; a push that finds one idle | the rest itself; one notify |
//! | a block's steal flag (`sched::Shared::block_busy`, an atomic) | a thief | one steal of an even run of nodes |
//! | the spare workers and their handles (`sched::Pool::run`) | the caller's thread, starting the workers an evaluation turns out to need | one spawn |
//! | the probes (`obs`) | a probe, with `profile` | one counter |
//!
//! No lock is held while an operation runs. No thread holds two locks of the
//! same row at once. A lock not in this table is a bug: add the row first.
//!
//! A guard taken at once costs a `try_*`; one waited for adds its wait to the
//! thread's count, which the pool sums into [`crate::Times::lock_wait_ns`].

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard, TryLockError};
use std::time::Instant;

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
    pub(crate) fn read(&self) -> RwLockReadGuard<'_, T> {
        match self.0.try_read() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        let g = self.0.read().unwrap_or_else(PoisonError::into_inner);
        waited(ns(t));
        g
    }

    /// An exclusive guard, timed as `read`.
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, T> {
        match self.0.try_write() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        let g = self.0.write().unwrap_or_else(PoisonError::into_inner);
        waited(ns(t));
        g
    }
}

pub(crate) struct Mx<T>(Mutex<T>);

impl<T> Mx<T> {
    pub(crate) const fn new(x: T) -> Mx<T> {
        Mx(Mutex::new(x))
    }

    /// A guard, timed as `Lock::read`.
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        match self.0.try_lock() {
            Ok(g) => return g,
            Err(TryLockError::Poisoned(p)) => return p.into_inner(),
            Err(TryLockError::WouldBlock) => {}
        }
        let t = Instant::now();
        let g = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        waited(ns(t));
        g
    }
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
