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
    /// Those that may go to another thread, by address (what a
    /// [`Known`] hands on).
    pinned: BTreeMap<(usize, usize), Box<dyn Any + Send>>,
}

/// How [`Saver::share`] keeps a shared value alive.
pub enum Pin {
    /// A handle that may go to another thread (an `Arc`'s clone).
    Send(Box<dyn Any + Send>),
    /// One that may not (an `Rc`'s).
    Local(Box<dyn Any>),
    /// None needed: the value outlives the saver, and is not handed on.
    None,
}

/// Blobs by the address of the value each holds, with those values kept
/// alive (so the addresses stay theirs, and an `Arc` shared by the handle
/// is not changed in place): a [`Saver::merkle`] given them names such a
/// value by its blob without writing it again. From the last save
/// ([`Saver::into_known`]) or load ([`Loader::into_known`]).
#[derive(Default)]
pub struct Known {
    map: BTreeMap<(usize, usize), (u128, Box<dyn Any + Send>)>,
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
    pub have: BTreeSet<u128>,
    /// Shared values written so far, by address: their hashes.
    known: BTreeMap<(usize, usize), u128>,
    emitted: BTreeSet<u128>,
    /// Blobs of the store referred to whole ([`Saver::blob_ref`]).
    referenced: BTreeSet<u128>,
    /// The blobs each value being written refers to (the root's first).
    refs: Vec<Vec<u128>>,
    min: usize,
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
            pinned: BTreeMap::new(),
        }
    }

    /// [`Saver::merkle`] knowing `known`'s values' blobs (those `have`
    /// holds).
    #[must_use]
    pub fn merkle_known(min: usize, have: BTreeSet<u128>, known: Known) -> Self {
        let mut s = Self::merkle(min, have);
        if let Some(m) = &mut s.merkle {
            for (addr, (h, pin)) in known.map {
                if m.have.contains(&h) {
                    m.known.insert(addr, h);
                    s.pinned.insert(addr, pin);
                }
            }
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
                .filter_map(|(addr, pin)| m.known.get(&addr).map(|&h| (addr, (h, pin))))
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
                have,
                known: BTreeMap::new(),
                emitted: BTreeSet::new(),
                referenced: BTreeSet::new(),
                refs: alloc::vec![Vec::new()],
                min: min.max(17),
            }),
            ..Self::new()
        }
    }

    /// The blobs of a [`Saver::merkle`] (none otherwise) and the root's
    /// bytes.
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
        self.merkle.as_ref()?.known.get(&(addr, len)).copied()
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
            self.share_merkle(addr, keep, save);
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
    /// [`Saver::share`] in a [`Saver::merkle`]: tag 2 and the hash of a
    /// blob, tag 1 and the id of a small value this blob wrote before, or
    /// tag 3, a length and a small value's bytes (its own ids from 0).
    fn share_merkle(
        &mut self,
        addr: (usize, usize),
        keep: impl FnOnce() -> Pin,
        save: impl FnOnce(&mut Self),
    ) {
        let known = self
            .merkle
            .as_ref()
            .and_then(|m| m.known.get(&addr).copied());
        if let Some(h) = known {
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
        if inner.len() >= min {
            let h = blob_hash(&inner);
            if let Some(m) = &mut self.merkle {
                m.known.insert(addr, h);
                if !m.have.contains(&h) && m.emitted.insert(h) {
                    m.blobs.push((h, inner, refs));
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

/// Where a [`Loader::merkle`] gets a blob by its hash.
pub type Fetch<'f> = &'f dyn Fn(u128) -> Option<Vec<u8>>;

/// What a [`Loader::merkle`] notes of the blobs it loaded: each value's
/// address and blob, with the value.
type Noted = Vec<((usize, usize), u128, Box<dyn Any + Send>)>;

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
    pub fn note<P: Clone + Send + 'static>(&mut self, addr: usize, len: usize, v: &P) {
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
