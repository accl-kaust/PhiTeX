//! An append-only table: fixed chunks of slots, each set once. Reads take no
//! lock and never wait on an append; an index is stable for ever. Nodes,
//! segments, operations and plain values live in tables, so an id is looked
//! up without touching any lock.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

const CHUNK_BITS: u32 = 16;
const CHUNK: usize = 1 << CHUNK_BITS;
const CHUNKS: usize = 1 << 14;

/// A chunk: `CHUNK` slots, each set once.
type Chunk<T> = Box<[OnceLock<T>]>;

pub(crate) struct Table<T> {
    /// A chunk is made on first use; a slot is set once.
    chunks: Box<[OnceLock<Chunk<T>>]>,
    next: AtomicUsize,
}

impl<T> Table<T> {
    pub(crate) fn new() -> Table<T> {
        Table {
            chunks: (0..CHUNKS).map(|_| OnceLock::new()).collect(),
            next: AtomicUsize::new(0),
        }
    }

    /// The next index, its slot still empty: an id can be made before its
    /// entry is built. `set` fills it, exactly once, into a chunk made
    /// here: a slot set under a lock (a placement under its segment's)
    /// allocates nothing.
    pub(crate) fn reserve(&self) -> usize {
        let i = self.next.fetch_add(1, Ordering::AcqRel);
        assert!(i < CHUNK * CHUNKS, "table full");
        self.chunk(i);
        i
    }

    /// `n` next indices in a row, the first returned.
    pub(crate) fn reserve_n(&self, n: usize) -> usize {
        let i = self.next.fetch_add(n, Ordering::AcqRel);
        assert!(i + n <= CHUNK * CHUNKS, "table full");
        for c in (i >> CHUNK_BITS)..=((i + n.max(1) - 1) >> CHUNK_BITS) {
            self.chunk(c << CHUNK_BITS);
        }
        i
    }

    fn chunk(&self, i: usize) -> &Chunk<T> {
        self.chunks[i >> CHUNK_BITS].get_or_init(|| (0..CHUNK).map(|_| OnceLock::new()).collect())
    }

    pub(crate) fn set(&self, i: usize, x: T) {
        assert!(
            self.chunk(i)[i & (CHUNK - 1)].set(x).is_ok(),
            "table slot {i} set twice"
        );
    }

    pub(crate) fn push(&self, x: T) -> usize {
        let i = self.reserve();
        self.set(i, x);
        i
    }

    /// None for an index not reserved or not yet set.
    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        self.chunks.get(i >> CHUNK_BITS)?.get()?[i & (CHUNK - 1)].get()
    }

    /// Indices handed out so far (set or not).
    pub(crate) fn len(&self) -> usize {
        self.next.load(Ordering::Acquire)
    }
}
