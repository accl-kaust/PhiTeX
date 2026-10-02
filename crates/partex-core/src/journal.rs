//! A flat vector that journals which chunks it wrote, so a checkpoint
//! costs what changed (DESIGN.md §5.3): eqtb, the hash and the save stack.
//!
//! The running engine reads and writes `live`, a plain vector: a read is
//! a bounds check and a load, whether or not the run keeps checkpoints. A
//! write also sets its block's bit in `dirty` (a block is an eighth of a
//! chunk, 64 elements: a snapshot compares only the blocks written, to
//! find the chunks written back as they were). `base` is the contents at
//! the last snapshot, in shared chunks. A clone (a checkpoint) is `base`
//! with the dirty chunks copied, and holds no flat vector; a snapshot
//! ([`JVec::commit`]) makes that the new `base`, so the next checkpoint
//! copies only the chunks written in between. A checkpoint that resumes
//! is flattened again ([`JVec::thaw`]); until then it answers reads from
//! its chunks. Nothing here writes through `&self`, so a checkpoint can be
//! read from several threads.
//!
//! This replaces a vector whose tail lived in copy-on-write chunks while
//! the run kept checkpoints (its head, most of eqtb and the hash, was
//! copied by every checkpoint, and every meaning in the tail cost an
//! indirection: 2–3% of a run).

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::ops::{Index, IndexMut};

/// Elements per chunk.
const BITS: usize = 9;
pub(crate) const CHUNK: usize = 1 << BITS;
const MASK: usize = CHUNK - 1;
/// Elements per block (the unit of `dirty`), and blocks per chunk (a
/// chunk's bits are one byte of a `dirty` word).
const BLOCK_BITS: usize = 6;
const BLOCK: usize = 1 << BLOCK_BITS;
const BLOCKS: usize = CHUNK / BLOCK;
const _: () = assert!(BLOCKS == 8);

/// Whether a snapshot compares only the blocks written of a chunk
/// written (on by default; `set_written_blocks`). Off, it compares the
/// whole chunk, as before.
static WRITTEN_BLOCKS: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`WRITTEN_BLOCKS`] on or off for the process
/// (`PARTEX_JVEC_BLOCKS=0`: off).
pub fn set_written_blocks(on: bool) {
    WRITTEN_BLOCKS.store(on, core::sync::atomic::Ordering::Relaxed);
}

fn written_blocks() -> bool {
    WRITTEN_BLOCKS.load(core::sync::atomic::Ordering::Relaxed)
}

/// Whether a restore rebases the running engine's vectors onto the
/// checkpoint's instead of thawing new ones ([`JVec::thaw_from`],
/// `flat.rs`, `tok.rs`; on by default; `set_rebase`).
static REBASE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`REBASE`] on or off for the process (`PARTEX_RESTORE_REBASE=0`:
/// a restore thaws every vector anew, as before).
pub fn set_rebase(on: bool) {
    REBASE.store(on, core::sync::atomic::Ordering::Relaxed);
}

pub(crate) fn rebase() -> bool {
    REBASE.load(core::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug)]
pub(crate) struct JVec<T> {
    /// The running contents (empty in a checkpoint).
    live: Vec<T>,
    /// The contents at the last snapshot (a checkpoint's contents). A
    /// running vector may have fewer chunks than it needs before its
    /// first snapshot: those count as dirty.
    base: Vec<Arc<[T; CHUNK]>>,
    /// Blocks written since the last snapshot, one bit each, a chunk's in
    /// one byte (running only).
    dirty: Vec<u64>,
    len: usize,
    frozen: bool,
}

impl<T: Copy + Default + PartialEq> JVec<T> {
    /// `n` copies of `v`, running.
    pub(crate) fn from_elem(v: T, n: usize) -> Self {
        Self {
            live: alloc::vec![v; n],
            base: Vec::new(),
            dirty: alloc::vec![0; n.div_ceil(CHUNK).div_ceil(8)],
            len: n,
            frozen: false,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.len
    }

    pub(crate) fn chunks(&self) -> usize {
        self.len.div_ceil(CHUNK)
    }

    /// The blocks of chunk `c` written since the last snapshot, a bit each.
    #[inline]
    fn blocks(&self, c: usize) -> u8 {
        (self.dirty[c >> 3] >> ((c & 7) << 3)).to_le_bytes()[0]
    }

    /// Whether running chunk `c` may differ from `base`'s.
    #[inline]
    fn is_dirty(&self, c: usize) -> bool {
        c >= self.base.len() || self.blocks(c) != 0
    }

    /// Note a write of element `i` (one OR, as for a chunk's bit: the
    /// block's index is a shift of `i` too).
    #[inline]
    fn mark(&mut self, i: usize) {
        let b = i >> BLOCK_BITS;
        self.dirty[b >> 6] |= 1 << (b & 63);
    }

    /// The element ranges within chunk `c` to compare with `base`'s: the
    /// blocks written, or (`PARTEX_JVEC_BLOCKS=0`) the whole chunk.
    fn written(&self, c: usize) -> impl Iterator<Item = (usize, usize)> {
        let n = ((c + 1) * CHUNK).min(self.len) - c * CHUNK;
        let mut bits = if written_blocks() {
            self.blocks(c)
        } else {
            0xff
        };
        core::iter::from_fn(move || {
            while bits != 0 {
                let k = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let (x, y) = (k * BLOCK, ((k + 1) * BLOCK).min(n));
                if x < y {
                    return Some((x, y));
                }
            }
            None
        })
    }

    /// Whether running chunk `c`, written, is `b` (`base`'s) still: the
    /// blocks written compared (a group's local assignments are written
    /// back as they were when it ends).
    fn same_as_base(&self, c: usize, b: &[T; CHUNK]) -> bool {
        let s = &self.live[c * CHUNK..];
        self.written(c).all(|(x, y)| b[x..y] == s[x..y])
    }

    /// Running chunk `c`, copied into a shared chunk.
    fn copy_chunk(&self, c: usize) -> Arc<[T; CHUNK]> {
        let s = &self.live[c * CHUNK..((c + 1) * CHUNK).min(self.len)];
        let mut a = [T::default(); CHUNK];
        a[..s.len()].copy_from_slice(s);
        Arc::new(a)
    }

    /// The chunks of the current contents: `base`'s where clean.
    fn frozen_chunks(&self) -> Vec<Arc<[T; CHUNK]>> {
        if self.frozen {
            return self.base.clone();
        }
        (0..self.chunks())
            .map(|c| {
                if self.is_dirty(c) {
                    self.copy_chunk(c)
                } else {
                    self.base[c].clone()
                }
            })
            .collect()
    }

    /// A snapshot: the running contents become the base the next clones
    /// share (only the chunks written since the last one are copied).
    pub(crate) fn commit(&mut self) {
        if self.frozen {
            return;
        }
        let n = self.chunks();
        for c in 0..n {
            if self.is_dirty(c) {
                if let Some(b) = self.base.get(c) {
                    // (written back as it was, as a group's local
                    // assignments are when it ends: still shared)
                    if self.same_as_base(c, b) {
                        continue;
                    }
                }
                let a = self.copy_chunk(c);
                if c < self.base.len() {
                    self.base[c] = a;
                } else {
                    self.base.push(a);
                }
            }
        }
        self.dirty.fill(0);
    }

    /// [`JVec::commit`], timed by `now` (`PARTEX_CUT_TIMING`): the same
    /// result, in three passes, walking the dirty bits, comparing the
    /// dirty chunks with the base and copying those that differ. Returns
    /// the nanoseconds of each, then the chunks, the dirty ones and the
    /// copied ones.
    pub(crate) fn commit_timed(&mut self, now: &dyn Fn() -> u64) -> [u64; 6] {
        if self.frozen {
            return [0; 6];
        }
        let n = self.chunks();
        let t0 = now();
        let dirty: Vec<usize> = (0..n).filter(|&c| self.is_dirty(c)).collect();
        let t1 = now();
        let differ: Vec<usize> = dirty
            .iter()
            .copied()
            .filter(|&c| self.base.get(c).is_none_or(|b| !self.same_as_base(c, b)))
            .collect();
        let t2 = now();
        for &c in &differ {
            let a = self.copy_chunk(c);
            if c < self.base.len() {
                self.base[c] = a;
            } else {
                self.base.push(a);
            }
        }
        self.dirty.fill(0);
        let t3 = now();
        [
            t1 - t0,
            t2 - t1,
            t3 - t2,
            n as u64,
            dirty.len() as u64,
            differ.len() as u64,
        ]
    }

    /// A checkpoint goes on: its contents flat again.
    pub(crate) fn thaw(&mut self) {
        if !self.frozen {
            return;
        }
        let mut live = Vec::with_capacity(self.len);
        for (c, a) in self.base.iter().enumerate() {
            let n = (self.len - c * CHUNK).min(CHUNK);
            live.extend_from_slice(&a[..n]);
        }
        self.live = live;
        self.dirty = alloc::vec![0; self.chunks().div_ceil(8)];
        self.frozen = false;
    }

    /// [`JVec::thaw`], rebasing `old`'s running vector (an engine this
    /// checkpoint replaces) instead of building a new one: a chunk `old`
    /// has unwritten since its base, and whose base is this checkpoint's
    /// (the same shared chunk), holds this checkpoint's elements already;
    /// only the others are copied (DESIGN.md §7.16.3, a restore rebases).
    /// `old` is left empty.
    pub(crate) fn thaw_from(&mut self, old: &mut Self) {
        if !self.frozen {
            return;
        }
        if !rebase() || old.frozen || old.len != self.len {
            return self.thaw();
        }
        let mut live = core::mem::take(&mut old.live);
        for (c, a) in self.base.iter().enumerate() {
            let same = c < old.base.len() && Arc::ptr_eq(&old.base[c], a) && !old.is_dirty(c);
            if !same {
                let (x, n) = (c * CHUNK, (self.len - c * CHUNK).min(CHUNK));
                live[x..x + n].copy_from_slice(&a[..n]);
            }
        }
        debug_assert!(self.slices_equal(&live));
        self.live = live;
        let mut dirty = core::mem::take(&mut old.dirty);
        dirty.clear();
        dirty.resize(self.chunks().div_ceil(8), 0);
        self.dirty = dirty;
        self.frozen = false;
    }

    /// (Debug builds:) whether `live` holds this checkpoint's elements.
    fn slices_equal(&self, live: &[T]) -> bool {
        self.base.iter().enumerate().all(|(c, a)| {
            let (x, n) = (c * CHUNK, (self.len - c * CHUNK).min(CHUNK));
            live[x..x + n] == a[..n]
        })
    }

    /// All the elements, as consecutive slices.
    pub(crate) fn slices(&self) -> impl Iterator<Item = &[T]> {
        let running = (!self.frozen).then_some(self.live.as_slice());
        let parts = if self.frozen { &self.base[..] } else { &[] };
        running.into_iter().chain(
            parts
                .iter()
                .enumerate()
                .map(|(c, a)| &a[..(self.len - c * CHUNK).min(CHUNK)]),
        )
    }

    /// Chunk `c` of the contents, and the shared chunk it equals, if known.
    pub(crate) fn view(&self, c: usize) -> (&[T], Option<&Arc<[T; CHUNK]>>) {
        let n = (self.len - c * CHUNK).min(CHUNK);
        if self.frozen {
            let a = &self.base[c];
            (&a[..n], Some(a))
        } else {
            let s = &self.live[c * CHUNK..c * CHUNK + n];
            (s, (!self.is_dirty(c)).then(|| &self.base[c]))
        }
    }

    /// The indices where `self` and `other` (of the same length) differ
    /// by `same`. Chunks the two share are skipped unread, so comparing a
    /// checkpoint with a later state costs what changed in between.
    pub(crate) fn differences(&self, other: &Self, same: impl Fn(&T, &T) -> bool) -> Vec<usize> {
        let mut out = Vec::new();
        if self.len != other.len {
            return (0..self.len.max(other.len)).collect();
        }
        for c in 0..self.chunks() {
            let ((a, sa), (b, sb)) = (self.view(c), other.view(c));
            if let (Some(x), Some(y)) = (sa, sb)
                && Arc::ptr_eq(x, y)
            {
                continue;
            }
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                if !same(x, y) {
                    out.push(c * CHUNK + i);
                }
            }
        }
        out
    }

    /// The elements `range`, copied out.
    pub(crate) fn to_vec(&self, range: core::ops::Range<usize>) -> Vec<T> {
        range.map(|i| self[i]).collect()
    }

    /// Set `self[at..at + src.len()]`.
    pub(crate) fn copy_from(&mut self, at: usize, src: &[T]) {
        assert!(at + src.len() <= self.len, "copy out of range");
        self.thaw();
        self.live[at..at + src.len()].copy_from_slice(src);
        if !src.is_empty() {
            for b in (at >> BLOCK_BITS)..=((at + src.len() - 1) >> BLOCK_BITS) {
                self.mark(b << BLOCK_BITS);
            }
        }
    }

    /// Element `i` of a checkpoint (the cold path of indexing).
    #[cold]
    #[inline(never)]
    fn frozen_at(&self, i: usize) -> &T {
        &self.base[i >> BITS][i & MASK]
    }
}

impl<T: Copy + Default + PartialEq> Clone for JVec<T> {
    fn clone(&self) -> Self {
        Self {
            live: Vec::new(),
            base: self.frozen_chunks(),
            dirty: Vec::new(),
            len: self.len,
            frozen: true,
        }
    }
}

impl<T: Copy + Default + PartialEq> Index<usize> for JVec<T> {
    type Output = T;
    #[inline]
    fn index(&self, i: usize) -> &T {
        match self.live.get(i) {
            Some(v) => v,
            None => self.frozen_at(i),
        }
    }
}

impl<T: Copy + Default + PartialEq> IndexMut<usize> for JVec<T> {
    #[inline]
    fn index_mut(&mut self, i: usize) -> &mut T {
        if self.frozen {
            self.thaw();
        }
        self.mark(i);
        &mut self.live[i]
    }
}

/// Saved as its chunks (which saved checkpoints share, as in memory);
/// loaded as a checkpoint, flat again when it resumes.
impl<T: Copy + Default + PartialEq + partex_engine::persist::Persist + Send + Sync + 'static>
    partex_engine::persist::Persist for JVec<T>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        self.frozen_chunks().save(s);
        self.len.save(s);
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        use partex_engine::persist::Persist;
        let base: Vec<Arc<[T; CHUNK]>> = Persist::load(l)?;
        let len = usize::load(l)?;
        (base.len() == len.div_ceil(CHUNK)).then_some(Self {
            live: Vec::new(),
            base,
            dirty: Vec::new(),
            len,
            frozen: true,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoints_share_what_did_not_change() {
        let mut v = JVec::from_elem(0u64, 3 * CHUNK + 7);
        v[5] = 1;
        let a = v.clone();
        v.commit();
        let b = v.clone();
        assert_eq!(a.differences(&b, |x, y| x == y), Vec::<usize>::new());
        v[2 * CHUNK + 3] = 9;
        let c = v.clone();
        assert!(Arc::ptr_eq(&b.base[0], &c.base[0]));
        assert!(!Arc::ptr_eq(&b.base[2], &c.base[2]));
        assert_eq!(b.differences(&c, |x, y| x == y), [2 * CHUNK + 3]);
        assert_eq!(v.differences(&b, |x, y| x == y), [2 * CHUNK + 3]);
        // a checkpoint reads from its chunks, and runs on flat again
        assert_eq!((c[5], c[2 * CHUNK + 3], c[3 * CHUNK + 6]), (1, 9, 0));
        let mut d = c.clone();
        d[3 * CHUNK + 6] = 4;
        assert_eq!(d.slices().flatten().copied().sum::<u64>(), 14);
        assert_eq!(c.slices().flatten().copied().sum::<u64>(), 10);
        d.copy_from(CHUNK - 1, &[7, 7]);
        assert_eq!(d.to_vec(CHUNK - 2..CHUNK + 2), [0, 7, 7, 0]);
    }
}
