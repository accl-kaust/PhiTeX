//! Every lock in the crate, and the time spent waiting for them.
//!
//! | lock | taken by | held for |
//! |---|---|---|
//! | a segment's `inner` (`segment::Segment`) | a read; a commit; a placement; a split | one map operation, or one commit's diff of one node; never two segments at once, never across a run |
//! | the order's list (`order::Order::segs`) | a position lookup; a split or a new unit | a binary search; one insert |
//! | a field-index shard (`index::FieldIndex`) | a read's or a commit's skip | one set lookup or insert |
//! | a value shard (`values::Values`) | an intern; a sequence node lookup | one insert or one lookup; plain reads take none |
//! | paths, monoids, environment, shapes, memo, marks (`engine::Engine`) | setup; rare inserts | one map operation |
//! | a node's `last` and `woke` (`node::Node`) | its commit; its re-ask | a swap |
//! | the pool's parking (`sched::Pool`) | an idle worker | the park itself |
//!
//! No lock is held while an operation runs. No thread holds two locks of the
//! same row at once. A lock not in this table is a bug: add the row first.
//!
//! A guard taken at once costs a `try_*`; one waited for adds its wait to the
//! thread's count, which the pool sums into [`crate::Times::lock_wait_ns`].

use std::cell::Cell;
use std::sync::{Mutex, MutexGuard, RwLock, RwLockReadGuard, RwLockWriteGuard};

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
        todo!()
    }

    /// An exclusive guard, timed as `read`.
    pub(crate) fn write(&self) -> RwLockWriteGuard<'_, T> {
        todo!()
    }
}

pub(crate) struct Mx<T>(Mutex<T>);

impl<T> Mx<T> {
    pub(crate) const fn new(x: T) -> Mx<T> {
        Mx(Mutex::new(x))
    }

    /// A guard, timed as `Lock::read`.
    pub(crate) fn lock(&self) -> MutexGuard<'_, T> {
        todo!()
    }
}

/// Adds one wait to the thread's count.
pub(crate) fn waited(ns: u64) {
    todo!()
}

/// The thread's lock waits since the last take, (summed, longest), reset.
pub(crate) fn take_wait() -> (u64, u64) {
    todo!()
}
