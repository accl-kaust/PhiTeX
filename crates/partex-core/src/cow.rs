//! A growable vector in shared chunks, copied on write (the token list
//! records of a checkpoint, `tok.rs`). The flat vectors that journal their
//! writes are `journal.rs`.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::{Index, IndexMut};

/// Elements per chunk (a checkpoint copies each chunk with a list that
/// changed; lists are made and freed everywhere in the id space).
const BITS: usize = 6;
pub(crate) const CHUNK: usize = 1 << BITS;
const MASK: usize = CHUNK - 1;

/// A growable vector entirely in shared chunks, copied on write, for
/// elements that are not `Copy` (the token list records): cloning it copies
/// one reference count per chunk.
#[derive(Clone, Debug)]
pub(crate) struct Chunked<T> {
    chunks: Vec<Arc<[T; CHUNK]>>,
    len: usize,
}

partex_engine::persist_struct!(impl[T] Chunked<T> { chunks, len });

impl<T> Default for Chunked<T> {
    fn default() -> Self {
        Self {
            chunks: Vec::new(),
            len: 0,
        }
    }
}

impl<T: Clone + Default> Chunked<T> {
    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn push(&mut self, v: T) {
        if self.len & MASK == 0 {
            self.chunks
                .push(Arc::new(core::array::from_fn(|_| T::default())));
        }
        let i = self.len;
        self.len += 1;
        self[i] = v;
    }

    /// `n` default elements, in shared chunks.
    pub(crate) fn with_len(n: usize) -> Self {
        let blank: Arc<[T; CHUNK]> = Arc::new(core::array::from_fn(|_| T::default()));
        Self {
            chunks: (0..n.div_ceil(CHUNK)).map(|_| blank.clone()).collect(),
            len: n,
        }
    }

    #[inline]
    pub(crate) fn get(&self, i: usize) -> Option<&T> {
        (i < self.len).then(|| &self.chunks[i >> BITS][i & MASK])
    }

    pub(crate) fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        (i < self.len).then(|| &mut self[i])
    }

    /// Whether chunk `c` (elements `c * 64..`) is the same shared chunk
    /// in `other`.
    pub(crate) fn shares_chunk(&self, other: &Self, c: usize) -> bool {
        match (self.chunks.get(c), other.chunks.get(c)) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &T> {
        self.chunks.iter().flat_map(|c| c.iter()).take(self.len)
    }
}

impl<T> Index<usize> for Chunked<T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        debug_assert!(
            i < self.len,
            "index {i} out of range for length {}",
            self.len
        );
        &self.chunks[i >> BITS][i & MASK]
    }
}

impl<T: Clone> IndexMut<usize> for Chunked<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        debug_assert!(
            i < self.len,
            "index {i} out of range for length {}",
            self.len
        );
        &mut Arc::make_mut(&mut self.chunks[i >> BITS])[i & MASK]
    }
}

impl<T: PartialEq> PartialEq for Chunked<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len
            && self
                .chunks
                .iter()
                .zip(&other.chunks)
                .enumerate()
                .all(|(c, (a, b))| {
                    let n = (self.len - c * CHUNK).min(CHUNK);
                    Arc::ptr_eq(a, b) || a[..n] == b[..n]
                })
    }
}

impl<T: Eq> Eq for Chunked<T> {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_shares_until_written() {
        let mut a: Chunked<i32> = Chunked::default();
        for i in 0..CHUNK + 3 {
            a.push(i32::try_from(i).unwrap());
        }
        let mut b = a.clone();
        b[CHUNK + 1] = -1;
        assert_eq!(
            (a[CHUNK + 1], b[CHUNK + 1]),
            (i32::try_from(CHUNK).unwrap() + 1, -1)
        );
        assert_ne!(a, b);
        assert_eq!(a.iter().count(), CHUNK + 3);
        assert!(Arc::ptr_eq(&a.chunks[0], &b.chunks[0]));
    }
}

/// Shards of a [`ShardMap`].
const SHARDS: usize = 64;

/// A map from `i32` keys in shared shards, copied on write: cloning it
/// copies a reference count per shard, and a change copies one shard (a
/// snapshot of the PDF object table, keyed by virtual ids that are spread
/// over the whole range, copies what changed since the last one, not the
/// table).
#[derive(Clone, PartialEq)]
pub(crate) struct ShardMap<V> {
    shards: Vec<Arc<alloc::collections::BTreeMap<i32, V>>>,
}

impl<V> Default for ShardMap<V> {
    fn default() -> Self {
        Self {
            shards: (0..SHARDS).map(|_| Arc::default()).collect(),
        }
    }
}

impl<V: core::fmt::Debug> core::fmt::Debug for ShardMap<V> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_map()
            .entries(self.shards.iter().flat_map(|s| s.iter()))
            .finish()
    }
}

/// The shard of key `k` (its bits mixed: virtual ids are hashes, other
/// keys may be dense).
fn shard(k: i32) -> usize {
    #[allow(clippy::cast_possible_truncation)] // (the top six bits)
    let s = (k.cast_unsigned().wrapping_mul(0x9e37_79b9) >> 26) as usize;
    s
}

#[allow(clippy::trivially_copy_pass_by_ref)] // (`BTreeMap`'s signatures, which it replaces)
impl<V: Clone> ShardMap<V> {
    pub(crate) fn get(&self, k: &i32) -> Option<&V> {
        self.shards[shard(*k)].get(k)
    }

    pub(crate) fn get_mut(&mut self, k: &i32) -> Option<&mut V> {
        let s = &mut self.shards[shard(*k)];
        if !s.contains_key(k) {
            return None;
        }
        Arc::make_mut(s).get_mut(k)
    }

    pub(crate) fn contains_key(&self, k: &i32) -> bool {
        self.shards[shard(*k)].contains_key(k)
    }

    pub(crate) fn insert(&mut self, k: i32, v: V) -> Option<V> {
        Arc::make_mut(&mut self.shards[shard(k)]).insert(k, v)
    }

    pub(crate) fn remove(&mut self, k: &i32) -> Option<V> {
        let s = &mut self.shards[shard(*k)];
        if !s.contains_key(k) {
            return None;
        }
        Arc::make_mut(s).remove(k)
    }

    pub(crate) fn len(&self) -> usize {
        self.shards.iter().map(|s| s.len()).sum()
    }

    /// The value of `k`, `f()` put there first if there is none.
    pub(crate) fn get_or_insert_with(&mut self, k: i32, f: impl FnOnce() -> V) -> &mut V {
        Arc::make_mut(&mut self.shards[shard(k)])
            .entry(k)
            .or_insert_with(f)
    }

    /// The entries, by key.
    pub(crate) fn sorted(&self) -> Vec<(i32, &V)> {
        let mut all: Vec<(i32, &V)> = self
            .shards
            .iter()
            .flat_map(|s| s.iter().map(|(&k, v)| (k, v)))
            .collect();
        all.sort_unstable_by_key(|e| e.0);
        all
    }

    /// The keys, in increasing order.
    pub(crate) fn keys(&self) -> Vec<i32> {
        let mut k: Vec<i32> = self.shards.iter().flat_map(|s| s.keys().copied()).collect();
        k.sort_unstable();
        k
    }
}

/// Saved as its shards, shared as they are in memory (a snapshot that
/// changed a shard costs that shard; `persist::save_seq`).
impl<V: Clone + partex_engine::persist::Persist + Send + Sync + 'static>
    partex_engine::persist::Persist for ShardMap<V>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        partex_engine::persist::save_seq(&self.shards, s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let shards: Vec<Arc<alloc::collections::BTreeMap<i32, V>>> =
            partex_engine::persist::load_seq(l)?;
        // (each key in its shard: what the map's lookups assume)
        let ok = shards.len() == SHARDS
            && shards
                .iter()
                .enumerate()
                .all(|(i, m)| m.keys().all(|&k| shard(k) == i));
        ok.then_some(Self { shards })
    }
}

/// A value shared between the running engine and its checkpoints, copied
/// on its first write after a clone (`Arc::make_mut`, through
/// `DerefMut`): cloning it is one reference count. For tables written
/// rarely (DESIGN.md §7.16.3), which a mutable borrow copies whole the
/// first time after each snapshot.
#[derive(Debug, Default)]
pub(crate) struct Shared<T>(Arc<T>);

impl<T> Shared<T> {
    pub(crate) fn new(v: T) -> Self {
        Self(Arc::new(v))
    }
}

impl<T> Clone for Shared<T> {
    fn clone(&self) -> Self {
        Self(self.0.clone())
    }
}

impl<T: PartialEq> PartialEq for Shared<T> {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0) || *self.0 == *other.0
    }
}

impl<T: core::hash::Hash> core::hash::Hash for Shared<T> {
    fn hash<H: core::hash::Hasher>(&self, h: &mut H) {
        (*self.0).hash(h);
    }
}

impl<'a, T> IntoIterator for &'a Shared<T>
where
    &'a T: IntoIterator,
{
    type Item = <&'a T as IntoIterator>::Item;
    type IntoIter = <&'a T as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        (&*self.0).into_iter()
    }
}

impl<T> core::ops::Deref for Shared<T> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        &self.0
    }
}

impl<T: Clone> core::ops::DerefMut for Shared<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        Arc::make_mut(&mut self.0)
    }
}

impl<T: partex_engine::persist::Persist + Send + Sync + 'static> partex_engine::persist::Persist
    for Shared<T>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.0.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        Some(Self(partex_engine::persist::Persist::load(l)?))
    }
}
