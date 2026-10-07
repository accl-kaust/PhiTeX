//! Engine state as bytes, for checkpoints that outlive the process
//! (`DESIGN.md` §5.3, "On-disk checkpoints").
//!
//! [`Persist`] writes a value to a [`Saver`] and reads it back from a
//! [`Loader`]. Values shared through `Rc`/`Arc` are written once per saver
//! and shared again when loaded, so several checkpoints in one file keep
//! the chunks they have in common (eqtb chunks, frozen token lists, file
//! contents, fonts) once, on disk and in memory. Nodes use the format
//! codec ([`crate::codec`]).
//!
//! Little-endian, fixed-width, no version of its own: the container
//! carries one. Loading returns `None` on malformed input.
//!
//! [`persist_struct!`](crate::persist_struct) implements the trait for a
//! struct by destructuring it exhaustively: a field added later does not
//! compile until it is persisted.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::any::{Any, TypeId};

use crate::codec::{Dec, Enc};

/// Where values are written.
pub struct Saver {
    pub enc: Enc,
    /// Shared allocations written so far, by address and length.
    shared: BTreeMap<(usize, usize), u32>,
    /// With [`Saver::with_sizes`]: where each named part began.
    sizes: Option<Vec<(&'static str, usize)>>,
    /// With [`Saver::pooled`]: shared values, written out of line.
    pool: Option<Pool>,
    /// With [`Saver::merkle`]: shared values as content-addressed blobs.
    merkle: Option<Merkle>,
    /// Every shared value written, kept alive while the saver is: shared
    /// values are known by address, and a temporary one freed while
    /// saving (a running vector's chunks copied out) could otherwise give
    /// its address to the next, which would then be written as a
    /// reference to the first.
    kept: Vec<Box<dyn Any>>,
    /// Values written by [`Saver::share_content`] so far (each counts as
    /// a shared value, as the loader counts it).
    anonymous: usize,
    /// Those that may go to another thread, by address (what a
    /// [`Known`] hands on).
    pinned: BTreeMap<(usize, usize), Box<dyn Held>>,
}

/// A value a [`Known`] keeps alive: a clone of an `Arc` that holds it.
pub trait Held: Send {
    /// Whether nothing but this handle holds the value (a value the build
    /// no longer has, which no save meets again).
    fn alone(&self) -> bool;
}

impl<T: ?Sized + Send + Sync + 'static> Held for Arc<T> {
    fn alone(&self) -> bool {
        // (a weak handle does not keep it: it fails to upgrade once this
        // one goes)
        Arc::strong_count(self) == 1
    }
}

/// How [`Saver::share`] keeps a shared value alive.
pub enum Pin {
    /// A handle that may go to another thread (an `Arc`'s clone).
    Send(Box<dyn Held>),
    /// One that may not (an `Rc`'s).
    Local(Box<dyn Any>),
    /// None needed: the value outlives the saver, and is not handed on.
    None,
}

/// Blobs by the address of the value each holds, with those values kept
/// alive (so the addresses stay theirs, and an `Arc` shared by the handle
/// is not changed in place): a [`Saver::merkle`] given them names such a
/// value by its blob without writing it again, and lets go of those
/// nothing else holds any more ([`Saver::merkle_known`]). From the last
/// save ([`Saver::take_known`]) or load ([`Loader::take_known`]).
#[derive(Default)]
pub struct Known {
    map: BTreeMap<(usize, usize), (u128, Box<dyn Held>)>,
}

impl Known {
    /// The blobs named.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Add `other`'s.
    pub fn extend(&mut self, other: Self) {
        self.map.extend(other.map);
    }
}

/// A [`Saver::merkle`]'s blobs: each shared value of at least `min`
/// bytes is written out of line as a blob named by the hash of its bytes
/// (in which the blobs it refers to appear by their hashes: a Merkle
/// DAG, so equal values have equal names from one save to the next);
/// smaller ones are written inline, each once per blob.
pub struct Merkle {
    /// The blobs met, in the order written (children before parents),
    /// each once, with its hash and the blobs it refers to.
    pub blobs: Vec<MerkleBlob>,
    /// Blobs the caller already has (not written again).
    pub have: Arc<BTreeSet<u128>>,
    /// Shared values written so far, by address: their hashes.
    known: BTreeMap<(usize, usize), u128>,
    /// Shared values known before this saver began (the last save's or
    /// load's, or the saver this one was forked from), by address.
    prior: Arc<BTreeMap<(usize, usize), u128>>,
    emitted: BTreeSet<u128>,
    /// Blobs of the store referred to whole ([`Saver::blob_ref`]).
    referenced: BTreeSet<u128>,
    /// The blobs each value being written refers to (the root's first).
    refs: Vec<Vec<u128>>,
    min: usize,
    /// Where each blob goes as it is made, instead of `blobs`
    /// ([`Saver::merkle_sink`]).
    sink: Option<Box<dyn FnMut(MerkleBlob)>>,
    /// With [`Saver::merkle_stats`]: what the blobs hold.
    stats: Option<Stats>,
}

/// What a [`Saver::merkle`]'s blobs hold, by the type of the shared value
/// and the part ([`Saver::mark`]) of it the bytes are in
/// ([`Saver::merkle_stats`]).
#[derive(Default)]
pub struct Stats {
    pub rows: BTreeMap<(&'static str, &'static str), StatRow>,
    /// The parts begun in each value being written (the root's first).
    marks: Vec<Vec<(&'static str, usize)>>,
}

/// A row of [`Stats`].
#[derive(Default, Clone, Copy)]
pub struct StatRow {
    /// New blobs, and their bytes (of this part).
    pub new: u64,
    pub new_bytes: u64,
    /// References to blobs in the new blobs' bytes.
    pub refs: u64,
    /// Blobs encoded again whose bytes were a blob already (another
    /// value, equal), and their bytes.
    pub same: u64,
    pub same_bytes: u64,
    /// Values known by address (not encoded).
    pub known: u64,
    /// Small values written inline, and their bytes.
    pub inline: u64,
    pub inline_bytes: u64,
}

impl Merkle {
    /// The blob of the shared value at `addr`, if this saver knows it.
    fn hash_of(&self, addr: (usize, usize)) -> Option<u128> {
        self.known
            .get(&addr)
            .or_else(|| self.prior.get(&addr))
            .copied()
    }
}

/// What a [`Saver::merkle`] hands a saver on another thread
/// ([`Saver::merkle_fork`], [`Saver::forked`]): the blobs the store has
/// and the values it knows, by address (the values themselves kept alive
/// by it, which outlives the fork).
#[derive(Clone)]
pub struct Fork {
    min: usize,
    have: Arc<BTreeSet<u128>>,
    prior: Arc<BTreeMap<(usize, usize), u128>>,
}

/// What a forked saver did, for the saver it was forked from
/// ([`Saver::into_joined`], [`Saver::merkle_join`]): the blobs it made
/// and the values it wrote, by address, with them.
pub struct Joined {
    emitted: BTreeSet<u128>,
    known: Known,
}

impl Saver {
    /// What a saver on another thread needs to do part of this one's
    /// work: its blobs written by it go to its own sink; its results come
    /// back with [`Saver::merkle_join`]. The blobs a forked saver makes
    /// are those this saver would make (a blob's bytes depend on its
    /// value only), so work split over forks writes the same store.
    #[must_use]
    pub fn merkle_fork(&self) -> Option<Fork> {
        let m = self.merkle.as_ref()?;
        let mut prior = (*m.prior).clone();
        prior.extend(m.known.iter().map(|(a, h)| (*a, *h)));
        Some(Fork {
            min: m.min,
            have: m.have.clone(),
            prior: Arc::new(prior),
        })
    }

    /// A saver doing part of the work of the saver `f` was forked from.
    #[must_use]
    pub fn forked(f: &Fork) -> Self {
        let mut s = Self::merkle(f.min, BTreeSet::new());
        if let Some(m) = &mut s.merkle {
            m.have = f.have.clone();
            m.prior = f.prior.clone();
        }
        s
    }

    /// This forked saver's results, for [`Saver::merkle_join`].
    #[must_use]
    pub fn into_joined(mut self) -> Joined {
        let known = self.take_known();
        let emitted = self.merkle.map(|m| m.emitted).unwrap_or_default();
        Joined { emitted, known }
    }

    /// Take a fork's results: its blobs count as made here (referred to,
    /// not made again), and the values it wrote as known.
    pub fn merkle_join(&mut self, j: Joined) {
        let Some(m) = &mut self.merkle else {
            return;
        };
        m.emitted.extend(j.emitted);
        for (addr, (h, pin)) in j.known.map {
            m.known.insert(addr, h);
            self.pinned.insert(addr, pin);
        }
    }
}

/// A blob a [`Saver::merkle`] wrote: its hash, bytes and the blobs it
/// refers to (what it keeps alive in a store).
pub type MerkleBlob = (u128, Vec<u8>, Vec<u128>);

/// The hash a [`Saver::merkle`] names a blob by.
#[must_use]
pub fn blob_hash(bytes: &[u8]) -> u128 {
    crate::stablehash::StableHasher::of(bytes)
}

/// Shared values written out of line, each once: its bytes, and where
/// each id's are (none for the preshared).
#[derive(Default)]
pub struct Pool {
    pub bytes: Vec<u8>,
    pub spans: Vec<(usize, usize)>,
    /// With [`Saver::pool_sizes`]: values and bytes by type.
    pub sizes: Option<BTreeMap<&'static str, (usize, usize)>>,
}

impl Default for Saver {
    fn default() -> Self {
        Self::new()
    }
}

impl Saver {
    #[must_use]
    pub fn new() -> Self {
        Self {
            enc: Enc::default(),
            shared: BTreeMap::new(),
            sizes: None,
            pool: None,
            merkle: None,
            kept: Vec::new(),
            anonymous: 0,
            pinned: BTreeMap::new(),
        }
    }

    /// [`Saver::merkle`] knowing `known`'s values' blobs (those `have`
    /// holds), but for the values nothing else holds any more: those are
    /// let go (a value goes with the last handle on it, its address with
    /// it; a value only another such value held goes too). Otherwise
    /// every value ever saved would stay alive as long as the store keeps
    /// its blob, each save adding the values the build replaced since.
    #[must_use]
    pub fn merkle_known(min: usize, have: BTreeSet<u128>, known: Known) -> Self {
        let mut s = Self::merkle(min, have);
        if let Some(m) = &mut s.merkle {
            let mut map = known.map;
            loop {
                let before = map.len();
                map.retain(|_, (h, pin)| m.have.contains(h) && !pin.alone());
                if map.len() == before {
                    break;
                }
            }
            let mut prior = BTreeMap::new();
            for (addr, (h, pin)) in map {
                prior.insert(addr, h);
                s.pinned.insert(addr, pin);
            }
            m.prior = Arc::new(prior);
        }
        s
    }

    /// The blobs of the values this saver wrote (or knew), with those
    /// values, for the next save.
    #[must_use]
    pub fn take_known(&mut self) -> Known {
        let Some(m) = &self.merkle else {
            return Known::default();
        };
        let pinned = core::mem::take(&mut self.pinned);
        Known {
            map: pinned
                .into_iter()
                .filter_map(|(addr, pin)| m.hash_of(addr).map(|h| (addr, (h, pin))))
                .collect(),
        }
    }

    /// A saver whose shared values of at least `min` bytes become
    /// content-addressed blobs ([`Merkle`]); `have`: the blobs the caller
    /// already keeps, which are named but not handed back.
    #[must_use]
    pub fn merkle(min: usize, have: BTreeSet<u128>) -> Self {
        Self {
            merkle: Some(Merkle {
                blobs: Vec::new(),
                have: Arc::new(have),
                known: BTreeMap::new(),
                prior: Arc::default(),
                emitted: BTreeSet::new(),
                referenced: BTreeSet::new(),
                refs: alloc::vec![Vec::new()],
                min: min.max(17),
                sink: None,
                stats: None,
            }),
            ..Self::new()
        }
    }

    /// Hand each blob of this [`Saver::merkle`] to `sink` as it is made
    /// (children before parents, each once), rather than keeping them all
    /// until [`Saver::into_merkle`]: a store writes them as they come.
    pub fn merkle_sink(&mut self, sink: Box<dyn FnMut(MerkleBlob)>) {
        if let Some(m) = &mut self.merkle {
            m.sink = Some(sink);
        }
    }

    /// The blobs of a [`Saver::merkle`] (none otherwise, nor those a
    /// [`Saver::merkle_sink`] took) and the root's bytes.
    #[must_use]
    pub fn into_merkle(self) -> (Vec<MerkleBlob>, Vec<u8>) {
        (self.merkle.map(|m| m.blobs).unwrap_or_default(), self.enc.0)
    }

    /// The blobs the root of a [`Saver::merkle`] refers to (the others
    /// are reached from these, through each blob's references).
    #[must_use]
    pub fn merkle_roots(&self) -> Vec<u128> {
        self.merkle
            .as_ref()
            .and_then(|m| m.refs.first().cloned())
            .unwrap_or_default()
    }

    /// The blob of the shared value at `addr` (of `len`) written so far.
    #[must_use]
    pub fn merkle_hash_of(&self, addr: usize, len: usize) -> Option<u128> {
        self.merkle.as_ref()?.hash_of((addr, len))
    }

    fn keep(&mut self, addr: (usize, usize), pin: Pin) {
        match pin {
            Pin::Send(p) => {
                self.pinned.insert(addr, p);
            }
            Pin::Local(p) => self.kept.push(p),
            Pin::None => {}
        }
    }

    /// A reference to blob `h` written here: noted as the current value's.
    fn blob_ref_here(&mut self, h: u128) {
        if let Some(m) = &mut self.merkle
            && let Some(top) = m.refs.last_mut()
        {
            top.push(h);
        }
        self.enc.u8(2);
        h.save(self);
    }

    /// Whether a [`Saver::blob_ref`] wrote a reference.
    #[must_use]
    pub fn merkle_referenced(&self) -> bool {
        self.merkle
            .as_ref()
            .is_some_and(|m| !m.referenced.is_empty())
    }

    /// In a [`Saver::merkle`] whose store has blob `h`: a reference to it
    /// (a value saved before, not saved again: the loader takes it with
    /// [`Loader::take_blob_ref`] or [`Loader::share`]). `false`, and
    /// nothing written, otherwise.
    pub fn blob_ref(&mut self, h: u128) -> bool {
        let Some(m) = &mut self.merkle else {
            return false;
        };
        if !m.have.contains(&h) && !m.emitted.contains(&h) {
            return false;
        }
        m.referenced.insert(h);
        self.blob_ref_here(h);
        true
    }

    /// A saver whose shared values go to a [`Pool`] rather than where they
    /// are first met: the parts written (see [`Saver::take_segment`]) can
    /// then be loaded separately, and in any order, sharing the values of
    /// one pool (a [`Loader::pooled`]).
    #[must_use]
    pub fn pooled() -> Self {
        Self {
            pool: Some(Pool::default()),
            ..Self::new()
        }
    }

    /// Count the pool's values and bytes by type (see [`Pool::sizes`]).
    pub fn pool_sizes(&mut self) {
        if let Some(p) = &mut self.pool {
            p.sizes = Some(BTreeMap::new());
        }
    }

    /// What was written since the last segment.
    pub fn take_segment(&mut self) -> Vec<u8> {
        core::mem::take(&mut self.enc.0)
    }

    /// The pool of a [`Saver::pooled`] (empty otherwise).
    #[must_use]
    pub fn into_pool(self) -> Pool {
        self.pool.unwrap_or_default()
    }

    /// A saver that records how many bytes each named part took (see
    /// [`Saver::mark`]).
    #[must_use]
    pub fn with_sizes() -> Self {
        Self {
            sizes: Some(Vec::new()),
            ..Self::new()
        }
    }

    /// A named part begins here (recorded only [`Saver::with_sizes`]).
    pub fn mark(&mut self, name: &'static str) {
        let at = self.enc.0.len();
        if let Some(v) = &mut self.sizes {
            v.push((name, at));
        }
        if let Some(st) = self.merkle.as_mut().and_then(|m| m.stats.as_mut())
            && let Some(top) = st.marks.last_mut()
        {
            top.push((name, at));
        }
    }

    /// Count what this [`Saver::merkle`]'s blobs hold ([`Stats`]).
    pub fn merkle_stats(&mut self) {
        if let Some(m) = &mut self.merkle {
            m.stats = Some(Stats {
                rows: BTreeMap::new(),
                marks: alloc::vec![Vec::new()],
            });
        }
    }

    /// What [`Saver::merkle_stats`] counted, the root's bytes included
    /// (as type `root`).
    #[must_use]
    pub fn take_merkle_stats(&mut self) -> Option<Stats> {
        let len = self.enc.0.len();
        let m = self.merkle.as_mut()?;
        let refs = m.refs.first().map_or(0, Vec::len);
        let mut st = m.stats.take()?;
        let marks = st.marks.pop().unwrap_or_default();
        if len > 0 {
            st.count_parts("root", &marks, len, refs);
        }
        Some(st)
    }

    /// Bytes written by each named part, summed over its occurrences,
    /// largest first.
    #[must_use]
    pub fn sizes(&self) -> Vec<(&'static str, usize)> {
        let Some(v) = &self.sizes else {
            return Vec::new();
        };
        let mut sum = BTreeMap::new();
        for (i, (name, at)) in v.iter().enumerate() {
            let end = v.get(i + 1).map_or(self.enc.0.len(), |n| n.1);
            *sum.entry(*name).or_insert(0) += end - at;
        }
        let mut out: Vec<_> = sum.into_iter().collect();
        out.sort_by_key(|x| core::cmp::Reverse(x.1));
        out
    }

    #[must_use]
    pub fn into_bytes(self) -> Vec<u8> {
        self.enc.0
    }

    /// Count `a` as already written, as the next shared value: the loader
    /// is given the same value by [`Loader::preshare`], so what refers to
    /// it is loaded sharing it (identity outside the saved values, like
    /// the contents of the files a session has read).
    /// `false` if it was already: then the loader is not given it again.
    pub fn preshare<T>(&mut self, a: &Arc<[T]>) -> bool {
        let id = u32::try_from(self.shared.len()).unwrap_or(u32::MAX);
        let key = (a.as_ptr().cast::<()>() as usize, a.len());
        if self.shared.contains_key(&key) {
            return false;
        }
        self.shared.insert(key, id);
        true
    }

    pub fn raw(&mut self, b: &[u8]) {
        self.enc.0.extend_from_slice(b);
    }

    /// A shared value at `addr` (of `len` elements): its contents
    /// (written by `save`) the first time, else a reference to them. In a
    /// pooled saver always a reference, the contents going to the pool.
    /// `keep`: a handle on the value (a clone of its `Arc`), kept while
    /// the saver is, so that its address is not reused meanwhile.
    pub fn share(
        &mut self,
        addr: usize,
        len: usize,
        what: &'static str,
        keep: impl FnOnce() -> Pin,
        save: impl FnOnce(&mut Self),
    ) {
        let addr = (addr, len);
        if self.merkle.is_some() {
            self.share_merkle(addr, what, keep, save);
            return;
        }
        if let Some(&id) = self.shared.get(&addr) {
            self.enc.u8(1);
            self.enc.i32(i32::try_from(id).unwrap_or(i32::MAX));
            return;
        }
        self.keep(addr, keep());
        let id = u32::try_from(self.shared.len()).unwrap_or(u32::MAX);
        self.shared.insert(addr, id);
        if self.pool.is_none() {
            self.enc.u8(0);
            save(self);
            return;
        }
        self.enc.u8(1);
        self.enc.i32(i32::try_from(id).unwrap_or(i32::MAX));
        let outer = core::mem::take(&mut self.enc.0);
        save(self);
        let inner = core::mem::replace(&mut self.enc.0, outer);
        if let Some(pool) = &mut self.pool {
            let id = id as usize;
            if pool.spans.len() <= id {
                pool.spans.resize(id + 1, (0, 0));
            }
            pool.spans[id] = (pool.bytes.len(), inner.len());
            pool.bytes.extend_from_slice(&inner);
            if let Some(sizes) = &mut pool.sizes {
                let e = sizes.entry(what).or_insert((0, 0));
                e.0 += 1;
                e.1 += inner.len();
            }
        }
    }
}

impl Saver {
    /// A value that is not shared in memory, written as a shared value
    /// named by its bytes alone: in a [`Saver::merkle`], a blob (equal
    /// copies, the same chunk of a table saved again unchanged, are one
    /// blob), else in place. Loaded with [`Loader::share`].
    pub fn share_content(&mut self, what: &'static str, save: impl FnOnce(&mut Self)) {
        // (no address: a key no value has, so later values' ids are those
        // the loader gives them)
        let key = (self.anonymous, usize::MAX);
        self.anonymous += 1;
        let outer_enc = core::mem::take(&mut self.enc.0);
        let outer_shared = core::mem::take(&mut self.shared);
        if let Some(m) = &mut self.merkle {
            m.refs.push(Vec::new());
            if let Some(st) = &mut m.stats {
                st.marks.push(Vec::new());
            }
        }
        save(self);
        let inner = core::mem::replace(&mut self.enc.0, outer_enc);
        self.shared = outer_shared;
        let min = self.merkle.as_ref().map_or(usize::MAX, |m| m.min);
        let refs = self
            .merkle
            .as_mut()
            .and_then(|m| m.refs.pop())
            .unwrap_or_default();
        if let Some(m) = &mut self.merkle
            && let Some(st) = &mut m.stats
        {
            let marks = st.marks.pop().unwrap_or_default();
            if inner.len() < min {
                let r = st.rows.entry((what, "")).or_default();
                r.inline += 1;
                r.inline_bytes += inner.len() as u64;
            } else if m.have.contains(&blob_hash(&inner)) || m.emitted.contains(&blob_hash(&inner))
            {
                let r = st.rows.entry((what, "")).or_default();
                r.same += 1;
                r.same_bytes += inner.len() as u64;
            } else {
                st.count_parts(what, &marks, inner.len(), refs.len());
            }
        }
        if inner.len() >= min {
            let h = blob_hash(&inner);
            if let Some(m) = &mut self.merkle
                && !m.have.contains(&h)
                && m.emitted.insert(h)
            {
                match &mut m.sink {
                    Some(sink) => sink((h, inner, refs)),
                    None => m.blobs.push((h, inner, refs)),
                }
            }
            self.blob_ref_here(h);
            return;
        }
        if let Some(top) = self.merkle.as_mut().and_then(|m| m.refs.last_mut()) {
            top.extend(refs);
        }
        let id = u32::try_from(self.shared.len()).unwrap_or(u32::MAX);
        self.shared.insert(key, id);
        self.enc.u8(3);
        save_len(inner.len(), self);
        self.raw(&inner);
    }

    /// [`Saver::share`] in a [`Saver::merkle`]: tag 2 and the hash of a
    /// blob, tag 1 and the id of a small value this blob wrote before, or
    /// tag 3, a length and a small value's bytes (its own ids from 0).
    fn share_merkle(
        &mut self,
        addr: (usize, usize),
        what: &'static str,
        keep: impl FnOnce() -> Pin,
        save: impl FnOnce(&mut Self),
    ) {
        let known = self.merkle.as_ref().and_then(|m| m.hash_of(addr));
        if let Some(h) = known {
            if let Some(st) = self.merkle.as_mut().and_then(|m| m.stats.as_mut()) {
                st.rows.entry((what, "")).or_default().known += 1;
            }
            self.blob_ref_here(h);
            return;
        }
        if let Some(&id) = self.shared.get(&addr) {
            self.enc.u8(1);
            self.enc.i32(i32::try_from(id).unwrap_or(i32::MAX));
            return;
        }
        self.keep(addr, keep());
        let outer_enc = core::mem::take(&mut self.enc.0);
        let outer_shared = core::mem::take(&mut self.shared);
        if let Some(m) = &mut self.merkle {
            m.refs.push(Vec::new());
            if let Some(st) = &mut m.stats {
                st.marks.push(Vec::new());
            }
        }
        save(self);
        let inner = core::mem::replace(&mut self.enc.0, outer_enc);
        self.shared = outer_shared;
        let refs = self
            .merkle
            .as_mut()
            .and_then(|m| m.refs.pop())
            .unwrap_or_default();
        let min = self.merkle.as_ref().map_or(usize::MAX, |m| m.min);
        if let Some(m) = &mut self.merkle
            && let Some(st) = &mut m.stats
        {
            let marks = st.marks.pop().unwrap_or_default();
            if inner.len() < min {
                let r = st.rows.entry((what, "")).or_default();
                r.inline += 1;
                r.inline_bytes += inner.len() as u64;
            } else {
                let h = blob_hash(&inner);
                if m.have.contains(&h) || m.emitted.contains(&h) {
                    let r = st.rows.entry((what, "")).or_default();
                    r.same += 1;
                    r.same_bytes += inner.len() as u64;
                } else {
                    st.count_parts(what, &marks, inner.len(), refs.len());
                }
            }
        }
        if inner.len() >= min {
            let h = blob_hash(&inner);
            if let Some(m) = &mut self.merkle {
                m.known.insert(addr, h);
                if !m.have.contains(&h) && m.emitted.insert(h) {
                    match &mut m.sink {
                        Some(sink) => sink((h, inner, refs)),
                        None => m.blobs.push((h, inner, refs)),
                    }
                }
            }
            self.blob_ref_here(h);
        } else {
            // (written in its parent: its references are its parent's)
            if let Some(top) = self.merkle.as_mut().and_then(|m| m.refs.last_mut()) {
                top.extend(refs);
            }
            let id = u32::try_from(self.shared.len()).unwrap_or(u32::MAX);
            self.shared.insert(addr, id);
            self.enc.u8(3);
            save_len(inner.len(), self);
            self.raw(&inner);
        }
    }
}

impl Stats {
    /// Add `o`'s counts (a forked saver's).
    pub fn merge(&mut self, o: Stats) {
        for (k, r) in o.rows {
            let e = self.rows.entry(k).or_default();
            e.new += r.new;
            e.new_bytes += r.new_bytes;
            e.refs += r.refs;
            e.same += r.same;
            e.same_bytes += r.same_bytes;
            e.known += r.known;
            e.inline += r.inline;
            e.inline_bytes += r.inline_bytes;
        }
    }

    /// Count a new blob of type `what`, of `len` bytes holding `refs`
    /// references, in its parts `marks`.
    fn count_parts(
        &mut self,
        what: &'static str,
        marks: &[(&'static str, usize)],
        len: usize,
        refs: usize,
    ) {
        let r = self.rows.entry((what, "")).or_default();
        r.new += 1;
        r.refs += refs as u64;
        let first = marks.first().map_or(len, |m| m.1);
        r.new_bytes += first as u64;
        for (i, (name, at)) in marks.iter().enumerate() {
            let end = marks.get(i + 1).map_or(len, |n| n.1);
            let r = self.rows.entry((what, name)).or_default();
            r.new += 1;
            r.new_bytes += (end - at) as u64;
        }
    }
}

/// Where a [`Loader::merkle`] gets a blob by its hash.
pub type Fetch<'f> = &'f dyn Fn(u128) -> Option<Vec<u8>>;

/// What a [`Loader::merkle`] notes of the blobs it loaded: each value's
/// address and blob, with the value.
type Noted = Vec<((usize, usize), u128, Box<dyn Held>)>;

/// The blobs a [`Loader::merkle`] has loaded, by hash and type (values
/// of two types can have the same bytes). `Send`, so that loading can go
/// on later on another thread (a snapshot loaded when first used).
pub type Loaded = BTreeMap<(u128, TypeId), Box<dyn Any + Send>>;

/// Where values are read from.
pub struct Loader<'a> {
    pub dec: Dec<'a>,
    shared: Shared,
    /// A pooled loader's pool.
    pool: Option<PoolRef<'a>>,
    /// A [`Loader::merkle`]'s blob source and the blobs loaded.
    fetch: Option<Fetch<'a>>,
    loaded: Loaded,
    /// The blob the last shared value loaded came from, if it did.
    last_blob: Option<u128>,
    noted: Noted,
}

/// A pool's bytes, and where each id's value is in them.
type PoolRef<'a> = (&'a [u8], &'a [(usize, usize)]);

/// The shared values a loader has loaded, by id: kept from one pooled
/// loader to the next over the same pool, so the parts loaded share them.
pub type Shared = Vec<Option<Box<dyn Any>>>;

impl<'a> Loader<'a> {
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self {
            dec: Dec::new(data),
            shared: Vec::new(),
            pool: None,
            fetch: None,
            loaded: Loaded::new(),
            last_blob: None,
            noted: Vec::new(),
        }
    }

    /// A loader of a [`Saver::merkle`]'s root `data`, taking blobs from
    /// `fetch` as they are needed (each once; `loaded`: those loaded by
    /// an earlier loader over the same store, see [`Loader::into_loaded`]).
    #[must_use]
    pub fn merkle(data: &'a [u8], fetch: Fetch<'a>, loaded: Loaded) -> Self {
        Self {
            fetch: Some(fetch),
            loaded,
            ..Self::new(data)
        }
    }

    /// The blobs loaded, for the next loader over the same store.
    #[must_use]
    pub fn into_loaded(self) -> Loaded {
        self.loaded
    }

    /// A loader of one segment of a [`Saver::pooled`], with the shared
    /// values loaded so far (see [`Loader::into_shared`]).
    #[must_use]
    pub fn pooled(
        data: &'a [u8],
        pool: &'a [u8],
        spans: &'a [(usize, usize)],
        shared: Shared,
    ) -> Self {
        Self {
            dec: Dec::new(data),
            shared,
            pool: Some((pool, spans)),
            fetch: None,
            loaded: Loaded::new(),
            last_blob: None,
            noted: Vec::new(),
        }
    }

    /// The shared values loaded, for the next segment's loader.
    #[must_use]
    pub fn into_shared(self) -> Shared {
        self.shared
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        self.dec.take(n)
    }

    /// The counterpart of [`Saver::preshare`], in the same order.
    pub fn preshare<P: Clone + 'static>(&mut self, v: P) {
        self.shared.push(Some(Box::new(v)));
    }

    #[must_use]
    pub fn at_end(&self) -> bool {
        self.dec.pos == self.dec.data.len()
    }

    /// The hash of the blob a [`Saver::blob_ref`] (or any blob
    /// reference) wrote here, if one is next: taken, not loaded.
    pub fn take_blob_ref(&mut self) -> Option<u128> {
        if self.fetch.is_none() || self.dec.data.get(self.dec.pos) != Some(&2) {
            return None;
        }
        let at = self.dec.pos;
        self.dec.pos += 1;
        let h = u128::load(self);
        if h.is_none() {
            self.dec.pos = at;
        }
        h
    }

    /// Blob `h` loaded by `load`.
    fn blob<P>(
        &mut self,
        h: u128,
        load: impl for<'b> FnOnce(&mut Loader<'b>) -> Option<P>,
    ) -> Option<P> {
        let bytes = (self.fetch?)(h)?;
        let mut inner = Loader {
            dec: Dec::new(&bytes),
            shared: Vec::new(),
            pool: None,
            fetch: self.fetch,
            loaded: core::mem::take(&mut self.loaded),
            last_blob: None,
            noted: core::mem::take(&mut self.noted),
        };
        let v = load(&mut inner);
        self.loaded = core::mem::take(&mut inner.loaded);
        self.noted = core::mem::take(&mut inner.noted);
        v
    }

    /// Note that `v`, at `addr` (of `len`), is the value the last shared
    /// value loaded was, if it came from a blob (see [`Known`]).
    pub fn note<P: Held + Clone + 'static>(&mut self, addr: usize, len: usize, v: &P) {
        if let Some(h) = self.last_blob.take() {
            self.noted.push(((addr, len), h, Box::new(v.clone())));
        }
    }

    /// What this loader noted, for a save to come.
    #[must_use]
    pub fn take_known(&mut self) -> Known {
        Known {
            map: core::mem::take(&mut self.noted)
                .into_iter()
                .map(|(addr, h, v)| (addr, (h, v)))
                .collect(),
        }
    }

    /// A shared value: loaded by `load` the first time, else the one
    /// loaded before.
    pub fn share<P: Clone + Send + 'static>(
        &mut self,
        load: impl for<'b> FnOnce(&mut Loader<'b>) -> Option<P>,
    ) -> Option<P> {
        if self.dec.data.get(self.dec.pos) == Some(&2) {
            self.dec.pos += 1;
            let h = u128::load(self)?;
            let key = (h, TypeId::of::<P>());
            if let Some(v) = self.loaded.get(&key) {
                self.last_blob = Some(h);
                return v.downcast_ref::<P>().cloned();
            }
            let v = self.blob(h, load)?;
            self.loaded.insert(key, Box::new(v.clone()));
            self.last_blob = Some(h);
            return Some(v);
        }
        self.last_blob = None;
        self.share_here(load)
    }

    /// [`Loader::share`] for a value that cannot go to another thread (an
    /// `Rc`): a blob is loaded each time it is met.
    pub fn share_local<P: Clone + 'static>(
        &mut self,
        load: impl for<'b> FnOnce(&mut Loader<'b>) -> Option<P>,
    ) -> Option<P> {
        if self.dec.data.get(self.dec.pos) == Some(&2) {
            self.dec.pos += 1;
            let h = u128::load(self)?;
            return self.blob(h, load);
        }
        self.share_here(load)
    }

    /// A shared value written in place, or referred to by id.
    fn share_here<P: Clone + 'static>(
        &mut self,
        load: impl for<'b> FnOnce(&mut Loader<'b>) -> Option<P>,
    ) -> Option<P> {
        match self.dec.u8()? {
            3 => {
                let n = usize::load(self)?;
                let end = self.dec.pos.checked_add(n)?;
                let slot = self.shared.len();
                let outer = core::mem::take(&mut self.shared);
                let v = load(self);
                self.shared = outer;
                let v = v?;
                if self.dec.pos != end {
                    return None;
                }
                self.shared.push(Some(Box::new(v.clone())));
                debug_assert_eq!(self.shared.len(), slot + 1);
                Some(v)
            }
            0 => {
                // (the slot is taken before loading: nested shared values
                // get the ids they were saved with)
                let slot = self.shared.len();
                self.shared.push(None);
                let v = load(self)?;
                self.shared[slot] = Some(Box::new(v.clone()));
                Some(v)
            }
            1 => {
                let id = usize::try_from(self.dec.i32()?).ok()?;
                if let Some(Some(v)) = self.shared.get(id) {
                    return v.downcast_ref::<P>().cloned();
                }
                // (pooled: loaded where it is in the pool, the first time)
                let (pool, spans) = self.pool?;
                let &(at, len) = spans.get(id)?;
                let bytes = pool.get(at..at.checked_add(len)?)?;
                let outer = core::mem::replace(&mut self.dec, Dec::new(bytes));
                let v = load(self);
                self.dec = outer;
                let v = v?;
                if self.shared.len() <= id {
                    self.shared.resize_with(id + 1, || None);
                }
                self.shared[id] = Some(Box::new(v.clone()));
                Some(v)
            }
            _ => None,
        }
    }
}

/// A value that can be saved and loaded.
pub trait Persist: Sized {
    fn save(&self, s: &mut Saver);
    fn load(l: &mut Loader) -> Option<Self>;

    /// The elements of a slice (its length written before): one by one,
    /// unless a type has a faster way.
    fn save_slice(v: &[Self], s: &mut Saver) {
        for x in v {
            x.save(s);
        }
    }

    /// `n` elements saved by [`Persist::save_slice`].
    fn load_vec(n: usize, l: &mut Loader) -> Option<Vec<Self>> {
        let mut v = Vec::with_capacity(n.min(1 << 20));
        for _ in 0..n {
            v.push(Self::load(l)?);
        }
        Some(v)
    }
}

macro_rules! ints {
    ($($t:ty),*) => {$(
        impl Persist for $t {
            fn save(&self, s: &mut Saver) {
                s.raw(&self.to_le_bytes());
            }
            fn load(l: &mut Loader) -> Option<Self> {
                Some(<$t>::from_le_bytes(l.take(core::mem::size_of::<$t>())?.try_into().ok()?))
            }
            fn save_slice(v: &[Self], s: &mut Saver) {
                let b = &mut s.enc.0;
                b.reserve(v.len() * core::mem::size_of::<$t>());
                for x in v {
                    b.extend_from_slice(&x.to_le_bytes());
                }
            }
            fn load_vec(n: usize, l: &mut Loader) -> Option<Vec<Self>> {
                const N: usize = core::mem::size_of::<$t>();
                let bytes = l.take(n.checked_mul(N)?)?;
                Some(
                    bytes
                        .chunks_exact(N)
                        .map(|c| <$t>::from_le_bytes(c.try_into().unwrap_or_default()))
                        .collect(),
                )
            }
        }
    )*};
}
ints!(i8, u16, i16, u32, i32, u64, i64, u128, i128);

/// Bytes are written as they are (file contents: the bulk of a session).
impl Persist for u8 {
    fn save(&self, s: &mut Saver) {
        s.enc.u8(*self);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.dec.u8()
    }
    fn save_slice(v: &[Self], s: &mut Saver) {
        s.raw(v);
    }
    fn load_vec(n: usize, l: &mut Loader) -> Option<Vec<Self>> {
        Some(l.take(n)?.to_vec())
    }
}

impl Persist for usize {
    fn save(&self, s: &mut Saver) {
        (*self as u64).save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        usize::try_from(u64::load(l)?).ok()
    }
}

impl Persist for isize {
    fn save(&self, s: &mut Saver) {
        (*self as i64).save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        isize::try_from(i64::load(l)?).ok()
    }
}

impl Persist for bool {
    fn save(&self, s: &mut Saver) {
        s.enc.u8(u8::from(*self));
    }
    fn load(l: &mut Loader) -> Option<Self> {
        match l.dec.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl Persist for f64 {
    fn save(&self, s: &mut Saver) {
        self.to_bits().save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        Some(f64::from_bits(u64::load(l)?))
    }
}

impl Persist for f32 {
    fn save(&self, s: &mut Saver) {
        self.to_bits().save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        Some(f32::from_bits(u32::load(l)?))
    }
}

impl Persist for char {
    fn save(&self, s: &mut Saver) {
        u32::from(*self).save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        char::from_u32(u32::load(l)?)
    }
}

impl Persist for () {
    fn save(&self, _: &mut Saver) {}
    fn load(_: &mut Loader) -> Option<Self> {
        Some(())
    }
}

/// A shared value's address: what names it while it is alive (see
/// [`save_seq`]).
pub trait Addressed {
    fn address(&self) -> usize;
}

impl<T: ?Sized> Addressed for Arc<T> {
    fn address(&self) -> usize {
        Arc::as_ptr(self).cast::<()>() as usize
    }
}

/// Children per node of a [`save_seq`] tree.
const SEQ_FANOUT: usize = 32;

/// The span of the root's children in a [`save_seq`] tree of `n`
/// elements: the least power of [`SEQ_FANOUT`] with at most that many
/// children.
fn seq_span(n: usize) -> usize {
    let mut span: usize = 1;
    while span.saturating_mul(SEQ_FANOUT) < n {
        span *= SEQ_FANOUT;
    }
    span
}

/// Save a sequence of shared values (the chunks of a vector kept in
/// shared chunks) as a tree: nodes of [`SEQ_FANOUT`] children, each node
/// a shared value named by the addresses of the values under it. A
/// sequence saved again with most of its values the same (the next
/// snapshot of a table that changed a few chunks) refers to the nodes it
/// has in common (in a [`Saver::merkle`], blobs written once) and writes
/// only the nodes on the paths to what changed: a save costs what
/// changed, as the snapshot does in memory, not a reference per value.
/// The values are kept alive while the saver is (as every shared value
/// it writes), so an address names one value for the whole save.
pub fn save_seq<A: Persist + Addressed>(items: &[A], s: &mut Saver) {
    save_len(items.len(), s);
    save_seq_level(items, seq_span(items.len()), s);
}

fn save_seq_level<A: Persist + Addressed>(items: &[A], span: usize, s: &mut Saver) {
    if span == 1 {
        for x in items {
            x.save(s);
        }
        return;
    }
    for c in items.chunks(span) {
        use core::hash::Hasher as _;
        let mut h = crate::stablehash::StableHasher::new();
        h.write_usize(span);
        h.write_usize(c.len());
        for x in c {
            h.write_usize(x.address());
        }
        let k = h.finish128();
        // (a key no allocation has: its length has the top bit set)
        #[allow(clippy::cast_possible_truncation)] // (the hash's halves)
        let (addr, len) = (k as usize, ((k >> 64) as usize) | !(usize::MAX >> 1));
        s.share(
            addr,
            len,
            "sequence",
            || Pin::None,
            |s| save_seq_level(c, span / SEQ_FANOUT, s),
        );
    }
}

/// A sequence [`save_seq`] saved (its nodes shared between the sequences
/// a loader loads, as they were saved).
pub fn load_seq<A: Persist + Clone + Send + Sync + 'static>(l: &mut Loader) -> Option<Vec<A>> {
    let n = usize::load(l)?;
    let mut out = Vec::with_capacity(n.min(1 << 20));
    load_seq_level(l, n, seq_span(n), &mut out)?;
    Some(out)
}

fn load_seq_level<A: Persist + Clone + Send + Sync + 'static>(
    l: &mut Loader,
    n: usize,
    span: usize,
    out: &mut Vec<A>,
) -> Option<()> {
    if span == 1 {
        for _ in 0..n {
            out.push(A::load(l)?);
        }
        return Some(());
    }
    let mut left = n;
    while left > 0 {
        let k = left.min(span);
        let node: Arc<[A]> = l.share(|l| {
            let mut v = Vec::with_capacity(k.min(1 << 20));
            load_seq_level(l, k, span / SEQ_FANOUT, &mut v)?;
            Some(Arc::from(v))
        })?;
        if node.len() != k {
            return None;
        }
        out.extend(node.iter().cloned());
        left -= k;
    }
    Some(())
}

/// Elements per chunk of a [`save_chunked`] table.
const TABLE_CHUNK: usize = 64;

/// Save a table that is not shared in memory but changes little from
/// one save to the next (a font table copied whole when one font
/// changes) in chunks of [`TABLE_CHUNK`], each named by its bytes
/// ([`Saver::share_content`]): a copy that changed a few elements costs
/// their chunks.
pub fn save_chunked<T: Persist>(items: &[T], s: &mut Saver) {
    save_len(items.len(), s);
    for c in items.chunks(TABLE_CHUNK) {
        s.share_content("table chunk", |s| T::save_slice(c, s));
    }
}

/// A table [`save_chunked`] saved (its chunks shared between the tables
/// a loader loads, as they were saved).
pub fn load_chunked<T: Persist + Clone + Send + Sync + 'static>(l: &mut Loader) -> Option<Vec<T>> {
    let n = usize::load(l)?;
    let mut out = Vec::with_capacity(n.min(1 << 20));
    while out.len() < n {
        let k = (n - out.len()).min(TABLE_CHUNK);
        let c: Arc<[T]> = l.share(|l| Some(Arc::from(T::load_vec(k, l)?)))?;
        if c.len() != k {
            return None;
        }
        out.extend(c.iter().cloned());
    }
    Some(out)
}

/// A length.
fn save_len(n: usize, s: &mut Saver) {
    n.save(s);
}

impl<T: Persist> Persist for Vec<T> {
    fn save(&self, s: &mut Saver) {
        save_len(self.len(), s);
        T::save_slice(self, s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let n = usize::load(l)?;
        T::load_vec(n, l)
    }
}

impl<T: Persist> Persist for VecDeque<T> {
    fn save(&self, s: &mut Saver) {
        save_len(self.len(), s);
        for x in self {
            x.save(s);
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        Some(Vec::<T>::load(l)?.into())
    }
}

impl Persist for String {
    fn save(&self, s: &mut Saver) {
        s.enc.bytes(self.as_bytes());
    }
    fn load(l: &mut Loader) -> Option<Self> {
        String::from_utf8(l.dec.bytes()?.to_vec()).ok()
    }
}

impl<T: Persist, const N: usize> Persist for [T; N] {
    fn save(&self, s: &mut Saver) {
        for x in self {
            x.save(s);
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let v: Vec<T> = (0..N).map(|_| T::load(l)).collect::<Option<_>>()?;
        v.try_into().ok()
    }
}

impl<T: Persist> Persist for Option<T> {
    fn save(&self, s: &mut Saver) {
        match self {
            None => s.enc.u8(0),
            Some(x) => {
                s.enc.u8(1);
                x.save(s);
            }
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        match l.dec.u8()? {
            0 => Some(None),
            1 => Some(Some(T::load(l)?)),
            _ => None,
        }
    }
}

impl<T: Persist> Persist for Box<T> {
    fn save(&self, s: &mut Saver) {
        (**self).save(s);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        Some(Box::new(T::load(l)?))
    }
}

impl<K: Persist + Ord, V: Persist> Persist for BTreeMap<K, V> {
    fn save(&self, s: &mut Saver) {
        save_len(self.len(), s);
        for (k, v) in self {
            k.save(s);
            v.save(s);
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let n = usize::load(l)?;
        let mut m = BTreeMap::new();
        for _ in 0..n {
            let k = K::load(l)?;
            m.insert(k, V::load(l)?);
        }
        Some(m)
    }
}

impl<K: Persist + Ord> Persist for BTreeSet<K> {
    fn save(&self, s: &mut Saver) {
        save_len(self.len(), s);
        for k in self {
            k.save(s);
        }
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let n = usize::load(l)?;
        (0..n).map(|_| K::load(l)).collect()
    }
}

macro_rules! tuples {
    ($(($($n:ident),+))*) => {$(
        impl<$($n: Persist),+> Persist for ($($n,)+) {
            #[allow(non_snake_case)]
            fn save(&self, s: &mut Saver) {
                let ($($n,)+) = self;
                $($n.save(s);)+
            }
            fn load(l: &mut Loader) -> Option<Self> {
                Some(($($n::load(l)?,)+))
            }
        }
    )*};
}
tuples!((A)(A, B)(A, B, C)(A, B, C, D)(A, B, C, D, E)(
    A, B, C, D, E, F
));

impl<T: Persist + 'static> Persist for Rc<T> {
    fn save(&self, s: &mut Saver) {
        s.share(
            Rc::as_ptr(self).cast::<()>() as usize,
            1,
            core::any::type_name::<T>(),
            || Pin::Local(Box::new(self.clone())),
            |s| (**self).save(s),
        );
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.share_local(|l| Some(Rc::new(T::load(l)?)))
    }
}

impl<T: Persist + Send + Sync + 'static> Persist for Arc<T> {
    fn save(&self, s: &mut Saver) {
        s.share(
            Arc::as_ptr(self).cast::<()>() as usize,
            1,
            core::any::type_name::<T>(),
            || Pin::Send(Box::new(self.clone())),
            |s| (**self).save(s),
        );
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let v = l.share(|l| Some(Arc::new(T::load(l)?)))?;
        l.note(Arc::as_ptr(&v).cast::<()>() as usize, 1, &v);
        Some(v)
    }
}

impl<T: Persist + Send + Sync + 'static> Persist for Arc<[T]> {
    fn save(&self, s: &mut Saver) {
        s.share(
            self.as_ptr().cast::<()>() as usize,
            self.len(),
            core::any::type_name::<[T]>(),
            || Pin::Send(Box::new(self.clone())),
            |s| {
                save_len(self.len(), s);
                T::save_slice(self, s);
            },
        );
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let v: Self = l.share(|l| Some(Arc::from(Vec::<T>::load(l)?)))?;
        l.note(v.as_ptr().cast::<()>() as usize, v.len(), &v);
        Some(v)
    }
}

impl<T: Persist + 'static> Persist for Rc<[T]> {
    fn save(&self, s: &mut Saver) {
        s.share(
            self.as_ptr().cast::<()>() as usize,
            self.len(),
            core::any::type_name::<[T]>(),
            || Pin::Local(Box::new(self.clone())),
            |s| {
                save_len(self.len(), s);
                T::save_slice(self, s);
            },
        );
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.share_local(|l| Some(Rc::from(Vec::<T>::load(l)?)))
    }
}

impl Persist for &'static [u8] {
    fn save(&self, s: &mut Saver) {
        s.enc.bytes(self);
    }
    /// (Leaked: help texts and the like, a few per load.)
    fn load(l: &mut Loader) -> Option<Self> {
        let b = l.dec.bytes()?;
        Some(Box::leak(b.to_vec().into_boxed_slice()))
    }
}

impl Persist for &'static str {
    fn save(&self, s: &mut Saver) {
        s.enc.bytes(self.as_bytes());
    }
    fn load(l: &mut Loader) -> Option<Self> {
        let b = core::str::from_utf8(l.dec.bytes()?).ok()?;
        Some(Box::leak(String::from(b).into_boxed_str()))
    }
}

/// Implement [`Persist`] for a struct, field by field, destructuring it
/// exhaustively (a new field does not compile until listed):
/// `persist_struct!(Type { a, b, c });`, or with generic parameters
/// `persist_struct!(impl[T] Type<T> { a, b });`. Tuple structs:
/// `persist_struct!(Type; 0, 1);`.
#[macro_export]
macro_rules! persist_struct {
    (impl [$($g:ident),*] $t:ty { $($f:ident),* $(,)? }) => {
        impl<$($g: $crate::persist::Persist + Send + Sync + 'static),*> $crate::persist::Persist for $t {
            $crate::persist_struct!(@body { $($f),* });
        }
    };
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl $crate::persist::Persist for $t {
            $crate::persist_struct!(@body { $($f),* });
        }
    };
    (impl [$($g:ident),*] $t:ty; $($i:tt),* $(,)?) => {
        impl<$($g: $crate::persist::Persist + Send + Sync + 'static),*> $crate::persist::Persist for $t {
            $crate::persist_struct!(@tuple $($i),*);
        }
    };
    ($t:ty; $($i:tt),* $(,)?) => {
        impl $crate::persist::Persist for $t {
            $crate::persist_struct!(@tuple $($i),*);
        }
    };
    (@body { $($f:ident),* }) => {
        fn save(&self, s: &mut $crate::persist::Saver) {
            let Self { $($f),* } = self;
            $( $crate::persist::Persist::save($f, s); )*
        }
        fn load(l: &mut $crate::persist::Loader) -> Option<Self> {
            Some(Self { $($f: $crate::persist::Persist::load(l)?),* })
        }
    };
    (@tuple $($i:tt),*) => {
        fn save(&self, s: &mut $crate::persist::Saver) {
            $( $crate::persist::Persist::save(&self.$i, s); )*
        }
        fn load(l: &mut $crate::persist::Loader) -> Option<Self> {
            Some(Self( $( { let _ = $i; $crate::persist::Persist::load(l)? } ),* ))
        }
    };
}

/// Implement [`Persist`] for an enum: each variant with its tag, as
/// `Unit`, `Tuple(a, b)` (binding names) or `Struct { a, b }`:
/// `persist_enum!(Type { A, B(x), C { y, z } });`. The match is
/// exhaustive: a new variant does not compile until listed.
#[macro_export]
macro_rules! persist_enum {
    ($(impl [$($g:ident),*])? $t:ty { $($v:ident $( ( $($tf:ident),* ) )? $( { $($sf:ident),* } )?),* $(,)? }) => {
        impl$(<$($g: $crate::persist::Persist + 'static),*>)? $crate::persist::Persist for $t {
            #[allow(non_snake_case)]
            fn save(&self, s: &mut $crate::persist::Saver) {
                #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
                enum Tag { $($v),* }
                match self {
                    $(
                        Self::$v $( ( $($tf),* ) )? $( { $($sf),* } )? => {
                            s.enc.u8(Tag::$v as u8);
                            $( $( $crate::persist::Persist::save($tf, s); )* )?
                            $( $( $crate::persist::Persist::save($sf, s); )* )?
                        }
                    )*
                }
            }
            #[allow(non_snake_case)]
            fn load(l: &mut $crate::persist::Loader) -> Option<Self> {
                #[allow(non_camel_case_types, clippy::upper_case_acronyms)]
                enum Tag { $($v),* }
                let want = l.dec.u8()?;
                $(
                    if want == Tag::$v as u8 {
                        return Some(Self::$v
                            $( ( $( { let $tf = (); let () = $tf; $crate::persist::Persist::load(l)? } ),* ) )?
                            $( { $( $sf: $crate::persist::Persist::load(l)? ),* } )?
                        );
                    }
                )*
                None
            }
        }
    };
}

impl Persist for crate::node::Node {
    fn save(&self, s: &mut Saver) {
        s.enc.node(self);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.dec.node()
    }
}

impl Persist for crate::node::GlueSpec {
    fn save(&self, s: &mut Saver) {
        s.enc.glue(self);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.dec.glue()
    }
}

impl Persist for crate::node::BoxNode {
    fn save(&self, s: &mut Saver) {
        s.enc.box_node(self);
    }
    fn load(l: &mut Loader) -> Option<Self> {
        l.dec.box_node()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two values saved, the build keeping one: the next save knows the
    /// one kept (not written again) and lets the other go, with a value
    /// that only the other held.
    #[test]
    fn known_lets_go_of_what_the_build_dropped() {
        let kept: Arc<Vec<u8>> = Arc::new(alloc::vec![7; 100]);
        let inner: Arc<Vec<u8>> = Arc::new(alloc::vec![9; 100]);
        let dropped: Arc<Vec<Arc<Vec<u8>>>> = Arc::new(alloc::vec![inner.clone()]);
        let inner_weak = Arc::downgrade(&inner);
        drop(inner);
        let mut s = Saver::merkle(16, BTreeSet::new());
        kept.save(&mut s);
        dropped.save(&mut s);
        let known = s.take_known();
        let (blobs, _) = s.into_merkle();
        assert_eq!(known.len(), 3);
        let have: BTreeSet<u128> = blobs.iter().map(|b| b.0).collect();
        drop(dropped);
        // (the inner value is still held, by its pin and the outer pin's
        // value: both go)
        assert!(inner_weak.upgrade().is_some());
        let mut s = Saver::merkle_known(16, have, known);
        assert!(inner_weak.upgrade().is_none());
        kept.save(&mut s);
        let known = s.take_known();
        let (blobs, _) = s.into_merkle();
        assert!(blobs.is_empty(), "a value known is not written again");
        assert_eq!(known.len(), 1);
    }

    /// A sink takes the blobs a saver keeps without one, in the same
    /// order, and the root is the same bytes.
    #[test]
    fn a_sink_takes_the_blobs_as_they_are_made() {
        let leaf: Arc<Vec<u8>> = Arc::new(alloc::vec![7; 100]);
        let value: Arc<Vec<Arc<Vec<u8>>>> = Arc::new(alloc::vec![leaf.clone(), leaf]);
        let mut s = Saver::merkle(16, BTreeSet::new());
        value.save(&mut s);
        let (kept, root) = s.into_merkle();
        let taken = Rc::new(core::cell::RefCell::new(Vec::new()));
        let mut s = Saver::merkle(16, BTreeSet::new());
        let sink = taken.clone();
        s.merkle_sink(Box::new(move |b| sink.borrow_mut().push(b)));
        value.save(&mut s);
        let (left, sunk_root) = s.into_merkle();
        assert!(left.is_empty(), "the sink took every blob");
        assert_eq!(sunk_root, root);
        assert_eq!(*taken.borrow(), kept);
        assert_eq!(kept.len(), 2);
    }
}
