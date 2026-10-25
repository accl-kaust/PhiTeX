//! An append-only table: fixed chunks of slots, each set once. Reads take no
//! lock and never wait on an append; an index is stable for ever. Nodes,
//! segments, operations and plain values live in tables, so an id is looked
//! up without touching any lock.

use std::sync::OnceLock;
use std::sync::atomic::AtomicUsize;

const CHUNK_BITS: u32 = 16;
const CHUNK: usize = 1 << CHUNK_BITS;
const CHUNKS: usize = 1 << 14;

pub(crate) struct Table<T> {
    /// A chunk is made on first use; a slot is set once.
    chunks: Box<[OnceLock<Box<[OnceLock<T>]>>]>,
    next: AtomicUsize,
}

impl<T> Table<T> {
    pub(crate) fn new() -> Table<T> {
        todo!()
    }

    /// The next index, its slot still empty: an id can be made before its
    /// entry is built. `set` fills it, exactly once.
    pub(crate) fn reserve(&self) -> usize {
        todo!()
    }

    pub(crate) fn set(&self, i: usize, x: T) {
        todo!()
    }

    pub(crate) fn push(&self, x: T) -> usize {
        todo!()
    }

    /// None for an index not reserved or not yet set.
    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        todo!()
    }

    /// Indices handed out so far (set or not).
    pub(crate) fn len(&self) -> usize {
        todo!()
    }
}
