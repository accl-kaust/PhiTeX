//! A vector the running engine uses flat, and a checkpoint holds as
//! shared segments (DESIGN.md §5.3), like the token lists: the string
//! pool, `str_start`, the input buffer and the input and parameter
//! stacks, which change mostly at their tops.
//!
//! The running vector keeps a shadow: its contents at its last snapshot,
//! in fixed-size segments. A clone compares the vector with the shadow (a
//! memcmp, no write tracking to get wrong) and copies only the segments
//! that changed; the clone holds those segments and no flat vector until
//! it resumes ([`Flat::thaw`]). [`Flat::commit`] (the engine's
//! `snapshot`) then makes the clone's segments the new shadow, so a
//! checkpoint costs what changed since the last one, not the whole
//! vector. A clone never writes to what it clones, so the engine is
//! `Sync` (a snapshot can be read from several threads at once).
//! Code that reads a checkpoint without resuming it (the state hash,
//! saving) goes through [`Flat::prefix`] or [`Flat::to_vec`].
//!
//! The engine's snapshots commit by the vector's live prefix and floor
//! ([`Flat::commit_live`], DESIGN.md §7.16.3): only `[0, live)` is
//! captured (what lies past it is dead: the engine writes it before it
//! reads it), and the segments wholly below the floor, which the owner
//! declares unchanged since the last snapshot, are shared without a
//! compare. A checkpoint then holds the live prefix and the vector's
//! length; resumed, the dead part reads as zeros.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::hash::{Hash, Hasher};
use core::ops::{Deref, DerefMut};

/// Elements per segment: about 4 KiB (a checkpoint copies the segments
/// that changed, and the input stack's top changes at every command).
const fn chunk<T>() -> usize {
    let n = 4096 / core::mem::size_of::<T>();
    if n < 64 { 64 } else { n }
}

/// A segment of a [`Flat`], with the shared segment it equals, if any.
pub(crate) type Segment<'a, T> = (&'a [T], Option<&'a Arc<[T]>>);

pub(crate) struct Flat<T> {
    live: Vec<T>,
    /// In a checkpoint: the contents (and `live` is empty).
    frozen: Option<Segments<T>>,
    /// The running vector's contents at its last snapshot.
    shadow: Segments<T>,
    /// Not written to (no mutable borrow) since `shadow` was made: a
    /// clone or commit then takes the shadow as it is, with no compare
    /// (a snapshot commits, then clones).
    clean: bool,
    /// The elements below it are unchanged since `shadow` was made, by
    /// the owner's word ([`Flat::lower_floor`], [`Flat::commit_live`]).
    floor: usize,
}

/// Whether the engine's snapshots commit by live prefix and floor
/// ([`Flat::commit_live`]; on by default, `set_flat_live`).
static LIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Turn [`LIVE`] on or off for the process (`PARTEX_FLAT_LIVE=0`: a
/// snapshot compares every vector whole, as before).
pub fn set_flat_live(on: bool) {
    LIVE.store(on, core::sync::atomic::Ordering::Relaxed);
}

/// Whether [`LIVE`] is on.
pub(crate) fn flat_live() -> bool {
    LIVE.load(core::sync::atomic::Ordering::Relaxed)
}

/// A vector in segments: `parts` hold its first elements (all of them,
/// or its live prefix, [`Flat::commit_live`]), `len` is its length; the
/// elements past the parts are dead and read as zeros.
struct Segments<T> {
    parts: Vec<Arc<[T]>>,
    len: usize,
}

impl<T> Clone for Segments<T> {
    fn clone(&self) -> Self {
        Self {
            parts: self.parts.clone(),
            len: self.len,
        }
    }
}

impl<T> Default for Segments<T> {
    fn default() -> Self {
        Self {
            parts: Vec::new(),
            len: 0,
        }
    }
}

impl<T: Copy + PartialEq + Default> Segments<T> {
    /// `v` in segments, sharing those of `self` that hold the same
    /// elements: chunk by chunk, so only the chunks that changed are
    /// copied, wherever they are.
    fn refreshed(&self, v: &[T]) -> Self {
        self.refreshed_live(v, v.len(), 0)
    }

    /// [`Segments::refreshed`] of `v[..live]` (the length staying
    /// `v`'s), taking the segments wholly below `floor` from `self` with
    /// no compare: the caller vouches that they are unchanged.
    fn refreshed_live(&self, v: &[T], live: usize, floor: usize) -> Self {
        let size = chunk::<T>();
        let parts = v[..live.min(v.len())]
            .chunks(size)
            .enumerate()
            .map(|(i, c)| match self.parts.get(i) {
                Some(p) if (i + 1) * size <= floor && p.len() == size => {
                    debug_assert!(p[..] == *c, "a Flat changed below its floor");
                    p.clone()
                }
                Some(p) if p[..] == *c => p.clone(),
                _ => Arc::from(c),
            })
            .collect();
        Self {
            parts,
            len: v.len(),
        }
    }

    /// The elements the parts hold.
    fn covered(&self) -> usize {
        self.parts.iter().map(|p| p.len()).sum()
    }

    fn to_vec(&self) -> Vec<T> {
        let mut v = Vec::with_capacity(self.len);
        for p in &self.parts {
            v.extend_from_slice(p);
        }
        // (the dead part, past the live prefix, as zeros)
        v.resize(self.len.max(v.len()), T::default());
        v
    }
}

impl<T> Flat<T> {
    pub(crate) fn new(v: Vec<T>) -> Self {
        Self {
            live: v,
            frozen: None,
            shadow: Segments::default(),
            clean: false,
            floor: 0,
        }
    }
}

impl<T: Copy + PartialEq + Default> Flat<T> {
    /// A checkpoint goes on: its contents flat again.
    pub(crate) fn thaw(&mut self) {
        if let Some(f) = self.frozen.take() {
            self.live = f.to_vec();
            self.shadow = f;
            self.clean = true;
            self.floor = 0; // (the owner's word is from before the thaw)
        }
    }

    /// [`Flat::thaw`], rebasing `old`'s running vector (an engine this
    /// checkpoint replaces) instead of building a new one: a segment of
    /// `old`'s shadow that is this checkpoint's (the same shared part),
    /// and that `old` has not written since (clean, or wholly below its
    /// floor), holds this checkpoint's elements already; only the others
    /// are copied. Past the captured prefix the elements are dead: left as
    /// `old` had them. `old` is left empty.
    pub(crate) fn thaw_from(&mut self, old: &mut Self) {
        let Some(f) = self.frozen.take() else {
            return;
        };
        if !crate::journal::rebase() || old.frozen.is_some() {
            self.frozen = Some(f);
            return self.thaw();
        }
        let size = chunk::<T>();
        let mut live = core::mem::take(&mut old.live);
        live.resize(f.len, T::default());
        for (i, p) in f.parts.iter().enumerate() {
            let same = old.shadow.parts.get(i).is_some_and(|q| Arc::ptr_eq(p, q))
                && (old.clean || (i + 1) * size <= old.floor);
            if !same {
                live[i * size..i * size + p.len()].copy_from_slice(p);
            }
        }
        debug_assert!(
            f.parts
                .iter()
                .enumerate()
                .all(|(i, p)| live[i * size..i * size + p.len()] == p[..]),
            "a rebased Flat differs from its checkpoint"
        );
        self.live = live;
        self.shadow = f;
        self.clean = true;
        self.floor = 0;
    }

    /// The contents, if flat (not a checkpoint's).
    #[inline]
    pub(crate) fn flat(&self) -> Option<&[T]> {
        self.frozen.is_none().then_some(&self.live[..])
    }

    /// The first `n` elements (or all there are), in parts, flat or not.
    pub(crate) fn prefix(&self, n: usize) -> Prefix<'_, T> {
        let parts: &[Arc<[T]>] = self.frozen.as_ref().map_or(&[], |f| &f.parts);
        Prefix {
            flat: self
                .frozen
                .is_none()
                .then_some(&self.live[..n.min(self.live.len())]),
            parts,
            // (a checkpoint holds its live prefix: past it, nothing)
            n: n.min(
                self.frozen
                    .as_ref()
                    .map_or(self.live.len(), Segments::covered),
            ),
        }
    }

    /// The first `n` elements in segments of a fixed size (the last one
    /// possibly shorter), each with the shared segment holding the same
    /// elements if there is one (`hashmemo.rs` hashes those by address).
    pub(crate) fn segments(&self, n: usize) -> Vec<Segment<'_, T>> {
        let size = chunk::<T>();
        let mut out = Vec::new();
        let mut left = n.min(self.len_all());
        if let Some(f) = &self.frozen {
            for p in &f.parts {
                if left == 0 {
                    break;
                }
                let take = p.len().min(left);
                left -= take;
                // (parts are `size` long but for the last)
                out.push((&p[..take], (take == p.len() && take == size).then_some(p)));
            }
            return out;
        }
        for (i, c) in self.live[..left].chunks(size).enumerate() {
            // (unwritten since the shadow was made: the same, uncompared)
            let shared = self.shadow.parts.get(i).filter(|p| {
                c.len() == size && p.len() == size && ((self.clean && flat_live()) || p[..] == *c)
            });
            out.push((c, shared));
        }
        out
    }

    /// Element `i`, flat or not.
    pub(crate) fn get_all(&self, i: usize) -> T {
        match &self.frozen {
            None => self.live[i],
            Some(f) => f
                .parts
                .get(i / chunk::<T>())
                .and_then(|p| p.get(i % chunk::<T>()))
                .copied()
                .unwrap_or_default(),
        }
    }

    /// Elements `a..b`, flat or not (borrowed unless they span segments).
    pub(crate) fn range_all(&self, a: usize, b: usize) -> alloc::borrow::Cow<'_, [T]> {
        use alloc::borrow::Cow;
        let Some(f) = &self.frozen else {
            return Cow::Borrowed(&self.live[a..b]);
        };
        let size = chunk::<T>();
        if a == b {
            return Cow::Borrowed(&[]);
        }
        let (pa, pb) = (a / size, (b - 1) / size);
        if pa == pb && b <= f.covered() {
            return Cow::Borrowed(&f.parts[pa][a % size..=(b - 1) % size]);
        }
        Cow::Owned((a..b).map(|i| self.get_all(i)).collect())
    }

    /// The number of elements, flat or not.
    pub(crate) fn len_all(&self) -> usize {
        self.frozen.as_ref().map_or(self.live.len(), |f| f.len)
    }

    /// Make the running contents the shadow the next clones compare
    /// with (a snapshot: the clone taken right after shares everything).
    pub(crate) fn commit(&mut self) {
        if self.frozen.is_none() && !self.clean {
            self.shadow = self.shadow.refreshed(&self.live);
            self.clean = true;
        }
        self.floor = 0;
    }

    /// A snapshot's commit of the live prefix `[0, live)` only (the rest
    /// is dead), sharing without a compare the segments wholly below the
    /// floor; `next` is the owner's floor from now on (the elements below
    /// it stay as they are until [`Flat::lower_floor`] says otherwise).
    /// With `PARTEX_FLAT_LIVE=0`, [`Flat::commit`].
    pub(crate) fn commit_live(&mut self, live: usize, next: usize) {
        if !flat_live() {
            return self.commit();
        }
        if self.frozen.is_some() {
            return;
        }
        let live = live.min(self.live.len());
        if !(self.clean && self.shadow.covered() >= live) {
            self.shadow = self.shadow.refreshed_live(&self.live, live, self.floor);
            self.clean = true;
        }
        self.floor = next.min(live);
    }

    /// The elements from `k` on may change before the next snapshot
    /// (the owner's word, see [`Flat::commit_live`]).
    #[inline]
    pub(crate) fn lower_floor(&mut self, k: usize) {
        self.floor = self.floor.min(k);
    }

    /// All elements, flat or not.
    pub(crate) fn to_vec(&self) -> Vec<T> {
        self.frozen
            .as_ref()
            .map_or_else(|| self.live.clone(), Segments::to_vec)
    }
}

impl<T: Copy + PartialEq + Default> Clone for Flat<T> {
    fn clone(&self) -> Self {
        let frozen = self.frozen.clone().unwrap_or_else(|| {
            if self.clean {
                self.shadow.clone()
            } else {
                self.shadow.refreshed(&self.live)
            }
        });
        Self {
            live: Vec::new(),
            frozen: Some(frozen),
            shadow: Segments::default(),
            clean: false,
            floor: 0,
        }
    }
}

impl<T> Deref for Flat<T> {
    type Target = Vec<T>;
    #[inline]
    fn deref(&self) -> &Vec<T> {
        &self.live
    }
}

impl<T> DerefMut for Flat<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut Vec<T> {
        self.clean = false;
        &mut self.live
    }
}

/// A prefix of a [`Flat`], flat or in parts; hashes as the slice would.
pub(crate) struct Prefix<'a, T> {
    flat: Option<&'a [T]>,
    parts: &'a [Arc<[T]>],
    n: usize,
}

impl<'a, T> Prefix<'a, T> {
    /// The prefix as slices, in order.
    pub(crate) fn slices(&self) -> impl Iterator<Item = &'a [T]> + '_ {
        let mut left = self.n;
        self.flat
            .into_iter()
            .chain(self.parts.iter().map(|p| &p[..]))
            .map(move |s| {
                let take = s.len().min(left);
                left -= take;
                &s[..take]
            })
            .filter(|s| !s.is_empty())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &'a T> + '_ {
        self.slices().flatten()
    }
}

impl<T: Hash> Hash for Prefix<'_, T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // (as `<[T]>::hash`: the length, then the elements, which a
        // streaming hasher takes the same in parts)
        state.write_usize(self.n);
        for s in self.slices() {
            T::hash_slice(s, state);
        }
    }
}

/// Saved as its segments (a flat one as one), which saved checkpoints
/// share as they do in memory; loaded frozen, flat again when it resumes.
impl<T: Copy + PartialEq + Default + partex_engine::persist::Persist + Send + Sync + 'static>
    partex_engine::persist::Persist for Flat<T>
{
    fn save(&self, s: &mut partex_engine::persist::Saver) {
        if let Some(f) = &self.frozen {
            partex_engine::persist::save_seq(&f.parts, s);
            f.len.save(s);
        } else {
            partex_engine::persist::save_seq(&self.shadow.refreshed(&self.live).parts, s);
            self.live.len().save(s);
        }
    }
    fn load(l: &mut partex_engine::persist::Loader) -> Option<Self> {
        let parts: Vec<Arc<[T]>> = partex_engine::persist::load_seq(l)?;
        // (the length: a checkpoint's parts hold its live prefix only)
        let len = partex_engine::persist::Persist::load(l)?;
        Some(Self {
            live: Vec::new(),
            frozen: Some(Segments { parts, len }),
            shadow: Segments::default(),
            clean: false,
            floor: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Flat;

    /// A snapshot by live prefix and floor holds what a whole one holds
    /// below the live end, whatever was written where the floor allowed,
    /// and reads as zeros past it.
    #[test]
    fn a_live_commit_holds_the_live_prefix() {
        let n = 20_000;
        let mut f = Flat::new(alloc::vec![0u8; n]);
        for (i, x) in f.iter_mut().enumerate() {
            *x = u8::try_from(i % 251).unwrap();
        }
        f.commit_live(15_000, 9_000);
        let at = |f: &Flat<u8>, i: usize| f.clone().get_all(i);
        assert_eq!(at(&f, 14_999), u8::try_from(14_999 % 251).unwrap());
        assert_eq!(at(&f, 16_000), 0, "past the live prefix: dead");
        // (above the floor: found by the compare)
        f[12_000] = 7;
        f.commit_live(15_000, 9_000);
        assert_eq!(at(&f, 12_000), 7);
        // (below it, once the owner lowers it)
        f.lower_floor(100);
        f[200] = 9;
        f.commit_live(15_000, 15_000);
        assert_eq!(at(&f, 200), 9);
        // (a checkpoint resumed: the prefix, then zeros, at full length)
        let mut c = f.clone();
        c.thaw();
        assert_eq!(c.len(), n);
        assert_eq!(c[200], 9);
        assert_eq!(c[12_000], 7);
        assert!(c[15_000..].iter().all(|&x| x == 0));
        assert_eq!(c[..15_000], f[..15_000]);
    }
}
