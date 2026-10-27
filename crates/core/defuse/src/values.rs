//! Values: immutable, interned, equal when their ids are.
//!
//! Small values live in the id itself: unit, booleans, integers of 61 bits.
//! Strings and lists are interned in an append-only table (`plain`, reads
//! without a lock) found through a hash index sharded by hash, whose shard
//! lock is held for one insert. A collection ([`Values::collect`], with
//! nothing running) frees the lists and sequence nodes no root reaches; a
//! list's slot stays, empty, its index never given again, so an id held
//! past its collection never names another value. A sequence (the monoid of folds) is a rope
//! whose id is a function of its elements alone, a polynomial hash modulo
//! 2^61 - 1 as a 61-bit payload under the sequence tag: two folds of the
//! same elements made in different shapes are one value, so a reader's
//! `seen` compares by id.

use std::hash::{BuildHasherDefault, Hasher};
use std::cell::Cell;
use std::sync::Arc;
use std::sync::atomic::{AtomicPtr, AtomicU64, AtomicUsize, Ordering};


use crate::lock::Lock;
use crate::table::Table;

/// A value's id: 3 tag bits and 61 payload bits.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct ValueId(u64);

const TAG_SHIFT: u32 = 61;
const PAYLOAD: u64 = (1 << TAG_SHIFT) - 1;
const T_UNIT: u64 = 0;
const T_BOOL: u64 = 1;
const T_INT: u64 = 2;
const T_SEQ: u64 = 3;
const T_STR: u64 = 4;
const T_LIST: u64 = 5;
/// An integer that does not fit 61 bits, interned as a plain value.
const T_BIG: u64 = 6;

/// The Mersenne prime 2^61 - 1, the sequence hash's modulus.
const P: u64 = PAYLOAD;
/// The base of the sequence hash.
const B: u64 = 0x0b4b_82e9_7f3d_1a2b % P;

/// Integers kept in the id: 61 bits, two's complement.
const INT_MIN: i64 = -(1 << 60);
const INT_MAX: i64 = (1 << 60) - 1;

impl ValueId {
    /// The unit value `()`.
    pub const UNIT: ValueId = ValueId(T_UNIT << TAG_SHIFT);
    /// The empty sequence.
    pub const NIL: ValueId = ValueId(T_SEQ << TAG_SHIFT);

    /// The id's 64 bits.
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// The id with these 64 bits.
    #[must_use]
    pub const fn from_raw(raw: u64) -> ValueId {
        ValueId(raw)
    }

    const fn tag(self) -> u64 {
        self.0 >> TAG_SHIFT
    }

    const fn payload(self) -> u64 {
        self.0 & PAYLOAD
    }

    const fn make(tag: u64, payload: u64) -> ValueId {
        ValueId((tag << TAG_SHIFT) | (payload & PAYLOAD))
    }
}

/// A field name: any value can name a field.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
#[cfg_attr(feature = "observe", derive(serde::Serialize, serde::Deserialize))]
pub struct Field(pub ValueId);

/// One node of a sequence as a client sees it: `left`, then `elem`, then
/// `right` (`len` elements in all).
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
#[allow(clippy::manual_non_exhaustive)]
pub struct SeqNode {
    pub left: ValueId,
    pub elem: ValueId,
    pub right: ValueId,
    pub len: u64,
    pub(crate) sealed: (),
}

/// An immutable value.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub enum Value {
    Unit,
    Bool(bool),
    Int(i64),
    Str(Box<str>),
    List(Box<[ValueId]>),
    /// The empty sequence.
    Nil,
    /// A non-empty sequence (see [`Values::seq_concat`]).
    Seq(SeqNode),
}

/// A rope node: `left` then `right`, `len` elements; a unit node holds its
/// one element in `right` with `left` NIL. `pow` is B^len mod P.
#[derive(Clone, Copy)]
struct Rope {
    len: u64,
    pow: u64,
    left: ValueId,
    right: ValueId,
    unit: bool,
}

/// A shard's lock on a cache line of its own.
#[repr(align(64))]
struct Padded<T>(T);

type Shard<V> = Padded<Lock<FxHashMap<u64, V>>>;

/// Many, each a cache line apart: an intern meets another's lock only when
/// their hashes share a shard ([`shard_of`]).
const SHARDS: usize = 1024;

/// A hash's shard: its bits mixed first (a list's hash is weak in its top
/// bits).
fn shard_of(h: u64) -> usize {
    usize::try_from(h.wrapping_mul(0x9e37_79b9_7f4a_7c15) >> 54).unwrap_or(0) & (SHARDS - 1)
}

static INSTANCES: AtomicU64 = AtomicU64::new(1);

/// The payload counter's shards ([`Values::payload_bytes`]): an intern
/// adds to its hash's, not to one count every thread writes.
const BYTE_SHARDS: usize = 64;

/// What a collection freed ([`Values::collect`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Collected {
    pub lists: usize,
    pub ropes: usize,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

/// A plain value's slot: its `Arc` as a raw pointer, null once collected
/// (a list's only), and whether it holds a list, set before the slot is
/// published.
struct Slot {
    ptr: AtomicPtr<Value>,
    list: bool,
}

impl Slot {
    fn new(v: Value) -> Slot {
        let list = matches!(v, Value::List(_));
        Slot { ptr: AtomicPtr::new(Arc::into_raw(Arc::new(v)).cast_mut()), list }
    }
}

impl Drop for Slot {
    #[allow(unsafe_code)]
    fn drop(&mut self) {
        let p = *self.ptr.get_mut();
        if !p.is_null() {
            // SAFETY: a non-null slot owns one count of the `Arc` it was
            // made from (`Slot::new`); a collection nulls it as it takes it.
            drop(unsafe { Arc::from_raw(p) });
        }
    }
}

/// The reader pins' shards ([`Values::pins`]).
const PIN_SHARDS: usize = 16;

thread_local! {
    /// The store whose evaluation's task this thread runs ([`TaskGuard`]),
    /// 0 for none.
    static TASK_OF: Cell<u64> = const { Cell::new(0) };
    /// This thread's pin shard.
    static PIN_SHARD: usize = {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        NEXT.fetch_add(1, Ordering::Relaxed) % PIN_SHARDS
    };
}

/// A worker's task in an evaluation of the store's engine (a run, its
/// commit, the re-asks it makes: `sched::work`): its reads take no pin.
/// Sound as no collection runs while a task does: `Values::collect` is
/// called by the collector of the evaluation's barrier, which every
/// active worker reaches between its tasks, or by `Engine::collect`,
/// which refuses during an evaluation (`Engine::busy`).
pub(crate) struct TaskGuard(u64);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        TASK_OF.with(|t| t.set(self.0));
    }
}

struct Unpin<'a>(&'a AtomicUsize);

impl Drop for Unpin<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The value store.
pub struct Values {
    /// Interned strings, lists and big integers by payload: no lock to read
    /// (a list's slot emptied by a collection frees its value).
    plain: Table<Slot>,
    /// The readers of a list outside a task of this store's evaluation, by
    /// thread ([`Values::at`]): a collection frees what it took only when
    /// none is pinned.
    pins: Box<[Padded<AtomicUsize>]>,
    /// The lists a collection took while a reader was pinned: freed by the
    /// first collection that finds none pinned.
    graveyard: Lock<Vec<Arc<Value>>>,
    /// The payload bytes made since the store began, less those collected
    /// (plain values and sequence nodes): what a collection is set off by.
    bytes: Box<[Padded<AtomicU64>]>,
    /// Whether `bytes` is kept ([`Values::count_payload`]): only for a
    /// store that collects.
    counting: std::sync::atomic::AtomicBool,
    /// Collections made ([`Values::collections`]).
    collections: AtomicU64,
    /// (observe) The sequence nodes collected: a use of one not made again
    /// since is a root the collection missed.
    #[cfg(feature = "observe")]
    gone_ropes: Lock<FxHashSet<u64>>,
    /// Content hash of a plain value -> its id; a shard's lock is held for
    /// one insert.
    plain_ix: Box<[Shard<ValueId>]>,
    /// Sequence nodes by their id's payload, sharded by it; a lookup holds a
    /// shard's read lock for one map get. Balanced on concat so a sequence
    /// of n elements is O(log n) deep.
    seqs: Box<[Shard<Rope>]>,
    /// Distinct per store, for caches keyed by ids (`instance`).
    instance: u64,
    /// Its plain values' first payload (`PLAIN_SPAN` times its instance):
    /// the stores of one process give their plain values distinct ids, so
    /// a cache of the process keyed by ids (a font's file, a map's text)
    /// never takes one store's value for another's.
    base: u64,
}

/// The payloads of one store's plain values.
const PLAIN_SPAN: u64 = 1 << 40;

impl Default for Values {
    fn default() -> Self {
        Values::new()
    }
}

fn mulmod(a: u64, b: u64) -> u64 {
    u64::try_from((u128::from(a) * u128::from(b)) % u128::from(P)).unwrap_or(0)
}

fn addmod(a: u64, b: u64) -> u64 {
    let s = a + b;
    if s >= P { s - P } else { s }
}

/// An element's hash, never zero (zero is NIL's payload).
fn elem_hash(x: ValueId) -> u64 {
    let h = hash_words(0x9e37_79b9_7f4a_7c15, &[x.0]) % P;
    if h == 0 { 1 } else { h }
}

/// Weight-balanced with weights len + 1 and ratio 3.
fn balanced(x: u64, y: u64) -> bool {
    3 * (x + 1) > y && 3 * (y + 1) > x
}

fn plain_hash(v: &Value) -> u64 {
    match v {
        Value::Str(s) => hash_bytes(T_STR, s.as_bytes()),
        Value::List(xs) => {
            let mut h = FxHasher(T_LIST);
            for x in xs {
                h.write_u64(x.0);
            }
            h.finish()
        }
        Value::Int(i) => hash_words(T_BIG, &[i.cast_unsigned()]),
        Value::Unit | Value::Bool(_) | Value::Nil | Value::Seq(_) => 0,
    }
}

impl Values {
    pub fn new() -> Values {
        let instance = INSTANCES.fetch_add(1, Ordering::Relaxed);
        Values {
            plain: Table::new(),
            plain_ix: (0..SHARDS).map(|_| Padded(Lock::new(FxHashMap::default()))).collect(),
            seqs: (0..SHARDS).map(|_| Padded(Lock::new(FxHashMap::default()))).collect(),
            bytes: (0..BYTE_SHARDS).map(|_| Padded(AtomicU64::new(0))).collect(),
            counting: std::sync::atomic::AtomicBool::new(false),
            pins: (0..PIN_SHARDS).map(|_| Padded(AtomicUsize::new(0))).collect(),
            graveyard: Lock::new(Vec::new()),
            collections: AtomicU64::new(0),
            #[cfg(feature = "observe")]
            gone_ropes: Lock::new(FxHashSet::default()),
            instance,
            base: (instance % (PAYLOAD / PLAIN_SPAN)) * PLAIN_SPAN,
        }
    }

    /// The index in `plain` of plain value `id`'s payload.
    fn plain_index(&self, id: ValueId) -> usize {
        id.payload().checked_sub(self.base).filter(|&i| i < PLAIN_SPAN).and_then(|i| usize::try_from(i).ok()).unwrap_or(usize::MAX)
    }

    /// The id of `v`, made if new.
    pub fn intern(&self, v: Value) -> ValueId {
        match v {
            Value::Unit => ValueId::UNIT,
            Value::Bool(b) => self.bool(b),
            Value::Int(i) => self.int(i),
            Value::Nil => ValueId::NIL,
            Value::Seq(n) => {
                let e = self.seq_unit(n.elem);
                self.seq_concat(n.left, self.seq_concat(e, n.right))
            }
            Value::Str(_) => self.intern_plain(v, T_STR),
            Value::List(_) => self.intern_plain(v, T_LIST),
        }
    }

    fn plain_is(&self, id: ValueId, v: &Value) -> bool {
        self.at(self.plain_index(id), |x| x == Some(v))
    }

    /// The current thread runs a task of this store's evaluation until the
    /// guard drops ([`TaskGuard`]).
    pub(crate) fn task(&self) -> TaskGuard {
        TaskGuard(TASK_OF.with(|t| t.replace(self.instance)))
    }

    /// `f` on the value in slot `i` (None: no such slot, or collected).
    /// A string or big integer is never freed, and no list is freed while
    /// a task of this store runs ([`TaskGuard`]): those read the pointer
    /// as it is. Any other read of a list pins the store first, and a
    /// collection that took the list since keeps it until none is pinned.
    #[inline]
    #[allow(unsafe_code)]
    fn at<R>(&self, i: usize, f: impl FnOnce(Option<&Value>) -> R) -> R {
        let Some(slot) = self.plain.get(i) else { return f(None) };
        if !slot.list || TASK_OF.with(Cell::get) == self.instance {
            // SAFETY: a string's or big integer's slot is never emptied
            // (`Values::collect` empties lists only), and a list's is not
            // emptied while a task of this store runs (`TaskGuard`): the
            // value outlives this borrow.
            return f(unsafe { slot.ptr.load(Ordering::Acquire).as_ref() });
        }
        let pin = &self.pins[PIN_SHARD.with(|s| *s)].0;
        pin.fetch_add(1, Ordering::SeqCst);
        let _unpin = Unpin(pin);
        // SAFETY: pinned before the load (both SeqCst): a collection that
        // empties the slot either finds the pin after its swap and keeps
        // the value (`Values::graveyard`), or swapped before this load,
        // which then reads null.
        f(unsafe { slot.ptr.load(Ordering::SeqCst).as_ref() })
    }

    /// The `Arc` in slot `i`, shared ([`Values::at`]).
    #[allow(unsafe_code)]
    fn arc_at(&self, i: usize) -> Option<Arc<Value>> {
        self.at(i, |x| {
            x.map(|v| {
                let p: *const Value = v;
                // SAFETY: `p` is the slot's `Arc` pointer, alive for this
                // borrow ([`Values::at`]): one more count, then owned.
                unsafe {
                    Arc::increment_strong_count(p);
                    Arc::from_raw(p)
                }
            })
        })
    }

    /// `f` on plain value `id` (None: not this store's). In observe
    /// builds a value collected is a root the collection missed: a panic
    /// naming the id and the caller.
    #[cfg_attr(feature = "observe", track_caller)]
    fn with_plain<R>(&self, id: ValueId, f: impl FnOnce(Option<&Value>) -> R) -> R {
        let i = self.plain_index(id);
        #[cfg(feature = "observe")]
        assert!(self.plain.get(i).is_none_or(|s| !s.ptr.load(Ordering::Relaxed).is_null()), "value {id:?} used after its collection");
        self.at(i, f)
    }

    /// The payload bytes a value holds, as [`Values::payload_bytes`]
    /// counts them.
    fn weight(v: &Value) -> u64 {
        32 + match v {
            Value::Str(s) => s.len() as u64,
            Value::List(xs) => xs.len() as u64 * 8,
            _ => 8,
        }
    }

    /// A sequence node's bytes, as [`Values::payload_bytes`] counts them.
    const ROPE_BYTES: u64 = 64;

    /// Keeps [`Values::payload_bytes`] from now (`on`), counted afresh
    /// from what the store holds, or stops keeping it (then 0): a store
    /// that does not collect pays no count per value made.
    pub(crate) fn count_payload(&self, on: bool) {
        if self.counting.swap(on, Ordering::SeqCst) == on {
            return;
        }
        let (_, plain, seqs) = if on { self.footprint() } else { (0, 0, 0) };
        let total = plain as u64 + seqs as u64 * Self::ROPE_BYTES;
        for (i, b) in self.bytes.iter().enumerate() {
            b.0.store(if i == 0 { total } else { 0 }, Ordering::SeqCst);
        }
    }

    /// The payload bytes the store holds (plain values and sequence nodes),
    /// kept while it collects ([`Values::count_payload`]), else 0.
    pub fn payload_bytes(&self) -> u64 {
        self.bytes.iter().map(|b| b.0.load(Ordering::Relaxed)).sum()
    }

    /// A string, list or big integer by content hash; a hash held by
    /// another value probes on with a salted hash.
    fn intern_plain(&self, v: Value, tag: u64) -> ValueId {
        let mut h = plain_hash(&v);
        loop {
            let shard = &self.plain_ix[shard_of(h)].0;
            let hit = shard.read().get(&h).copied();
            if let Some(id) = hit {
                if self.plain_is(id, &v) {
                    return id;
                }
                h = hash_words(h, &[1]);
                continue;
            }
            let mut w = shard.write();
            if let Some(&id) = w.get(&h) {
                if self.plain_is(id, &v) {
                    return id;
                }
                drop(w);
                h = hash_words(h, &[1]);
                continue;
            }
            if self.counting.load(Ordering::Relaxed) {
                self.bytes[shard_of(h) % BYTE_SHARDS].0.fetch_add(Self::weight(&v), Ordering::Relaxed);
            }
            let i = self.plain.push(Slot::new(v));
            let id = ValueId::make(tag, self.base + i as u64);
            w.insert(h, id);
            return id;
        }
    }

    /// (measure) The plain values' count and payload bytes (a list's ids,
    /// a string's bytes, an allocation's header each), and the sequence
    /// nodes' count: what the store holds now.
    pub fn footprint(&self) -> (usize, usize, usize) {
        let n = self.plain.len();
        let (mut count, mut bytes) = (0usize, 0usize);
        for i in 0..n {
            if let Some(w) = self.at(i, |x| x.map(Self::weight)) {
                count += 1;
                bytes += usize::try_from(w).unwrap_or(0);
            }
        }
        let seqs = self.seqs.iter().map(|s| s.0.read().len()).sum();
        (count, bytes, seqs)
    }

    /// Frees the lists and sequence nodes no root reaches (through a
    /// list's elements, a sequence's nodes and elements): with nothing
    /// running, every id a run or a table holds is among `roots` or
    /// reached from them. Strings and big integers stay (names, a small
    /// share of the payload); a freed list's slot stays, empty, its id
    /// never given again; a sequence node is made again by the same
    /// elements under the same id. The hash index is made anew from what
    /// stays, its probe chains whole.
    #[allow(unsafe_code)]
    pub(crate) fn collect(&self, roots: impl IntoIterator<Item = ValueId>) -> Collected {
        let counting = self.counting.load(Ordering::SeqCst);
        let before = if counting {
            self.payload_bytes()
        } else {
            let (_, plain, seqs) = self.footprint();
            plain as u64 + seqs as u64 * Self::ROPE_BYTES
        };
        let n = self.plain.len();
        let mut plain_mark = vec![0u64; n.div_ceil(64)];
        let mut rope_mark: FxHashSet<u64> = FxHashSet::default();
        let mut stack: Vec<ValueId> = roots.into_iter().collect();
        while let Some(x) = stack.pop() {
            match x.tag() {
                T_SEQ if x != ValueId::NIL => {
                    let p = x.payload();
                    if !rope_mark.insert(p) {
                        continue;
                    }
                    if let Some(r) = self.seqs[shard_of(p)].0.read().get(&p).copied() {
                        if !r.unit {
                            stack.push(r.left);
                        }
                        stack.push(r.right);
                    }
                }
                T_STR | T_LIST | T_BIG => {
                    let i = self.plain_index(x);
                    if i >= n || plain_mark[i / 64] & (1 << (i % 64)) != 0 {
                        continue;
                    }
                    plain_mark[i / 64] |= 1 << (i % 64);
                    if x.tag() == T_LIST {
                        self.at(i, |v| {
                            if let Some(Value::List(xs)) = v {
                                stack.extend(xs.iter().copied());
                            }
                        });
                    }
                }
                _ => {}
            }
        }
        let mut out = Collected {
            bytes_before: before,
            ..Collected::default()
        };
        let mut kept = 0u64;
        let mut ix: Vec<(u64, ValueId)> = Vec::new();
        let mut taken: Vec<Arc<Value>> = Vec::new();
        for i in 0..n {
            let Some(slot) = self.plain.get(i) else { continue };
            if slot.list && plain_mark[i / 64] & (1 << (i % 64)) == 0 {
                let p = slot.ptr.swap(std::ptr::null_mut(), Ordering::SeqCst);
                if !p.is_null() {
                    // SAFETY: the slot's count of its `Arc`, taken with
                    // the pointer (`Slot`).
                    taken.push(unsafe { Arc::from_raw(p) });
                    out.lists += 1;
                }
                continue;
            }
            let Some((tag, w, h)) = self.at(i, |v| {
                v.map(|v| {
                    let tag = match v {
                        Value::Str(_) => T_STR,
                        Value::List(_) => T_LIST,
                        _ => T_BIG,
                    };
                    (tag, Self::weight(v), plain_hash(v))
                })
            }) else {
                continue;
            };
            kept += w;
            ix.push((h, ValueId::make(tag, self.base + i as u64)));
        }
        // (a reader outside a task pinned before its load: seen here, it
        // may hold what was taken, which waits for a collection that sees
        // none; unseen, its load came after the swaps and read null)
        if self.pins.iter().all(|p| p.0.load(Ordering::SeqCst) == 0) {
            drop(std::mem::take(&mut *self.graveyard.write()));
            drop(taken);
        } else {
            self.graveyard.write().extend(taken);
        }
        for sh in &*self.seqs {
            let mut m = sh.0.write();
            let k = m.len();
            #[cfg(feature = "observe")]
            {
                let mut gone = self.gone_ropes.write();
                gone.extend(m.keys().filter(|p| !rope_mark.contains(p)).copied());
            }
            m.retain(|p, _| rope_mark.contains(p));
            m.shrink_to_fit();
            out.ropes += k - m.len();
            kept += m.len() as u64 * Self::ROPE_BYTES;
        }
        for sh in &*self.plain_ix {
            sh.0.write().clear();
        }
        for (h0, id) in ix {
            let mut h = h0;
            loop {
                let mut w = self.plain_ix[shard_of(h)].0.write();
                if w.contains_key(&h) {
                    drop(w);
                    h = hash_words(h, &[1]);
                    continue;
                }
                w.insert(h, id);
                break;
            }
        }
        for sh in &*self.plain_ix {
            sh.0.write().shrink_to_fit();
        }
        if counting {
            for (i, b) in self.bytes.iter().enumerate() {
                b.0.store(if i == 0 { kept } else { 0 }, Ordering::Relaxed);
            }
        }
        self.collections.fetch_add(1, Ordering::SeqCst);
        out.bytes_after = kept;
        out
    }

    /// The collections made so far.
    pub fn collections(&self) -> u64 {
        self.collections.load(Ordering::SeqCst)
    }

    /// Plain values interned so far.
    pub fn len(&self) -> usize {
        self.plain.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A hash of `id`'s content, the same in every store (an id is its
    /// store's own: two builds' equal values have other ids): the
    /// identity of a value compared across builds, a page's.
    pub fn digest(&self, id: ValueId) -> u64 {
        match id.tag() {
            T_UNIT | T_BOOL | T_INT => hash_words(0x6469_6765_7374, &[id.0]),
            T_SEQ => {
                let xs = self.seq_to_vec(id);
                let ds: Vec<u64> = xs.iter().map(|&x| self.digest(x)).collect();
                hash_words(T_SEQ, &ds)
            }
            _ => {
                let x = self.plain_full(id);
                match x.as_deref() {
                    Some(Value::Str(s)) => hash_bytes(T_STR, s.as_bytes()),
                    Some(Value::List(xs)) => {
                        let ds: Vec<u64> = xs.iter().map(|&x| self.digest(x)).collect();
                        hash_words(T_LIST, &ds)
                    }
                    Some(Value::Int(i)) => hash_words(T_BIG, &[i.cast_unsigned()]),
                    _ => 0,
                }
            }
        }
    }

    /// A number distinct per store, for caches keyed by ids.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// The store's instance and the collections it has made: a cache of
    /// ids made at one epoch holds while it lasts (a collected id is never
    /// handed out again, but a cache's id may have been collected).
    pub fn epoch(&self) -> u64 {
        self.instance | (self.collections() << 40)
    }

    pub fn int(&self, i: i64) -> ValueId {
        if (INT_MIN..=INT_MAX).contains(&i) {
            ValueId::make(T_INT, i.cast_unsigned())
        } else {
            self.intern_plain(Value::Int(i), T_BIG)
        }
    }

    pub fn bool(&self, b: bool) -> ValueId {
        ValueId::make(T_BOOL, u64::from(b))
    }

    pub fn str(&self, s: &str) -> ValueId {
        self.intern_plain(Value::Str(s.into()), T_STR)
    }

    pub fn list(&self, xs: Vec<ValueId>) -> ValueId {
        self.intern_plain(Value::List(xs.into_boxed_slice()), T_LIST)
    }

    pub fn list_from(&self, xs: &[ValueId]) -> ValueId {
        self.list(xs.to_vec())
    }

    pub fn field(&self, name: &str) -> Field {
        Field(self.str(name))
    }

    const fn small_int(id: ValueId) -> i64 {
        (id.payload() << 3).cast_signed() >> 3
    }

    /// Plain value `id`, shared (see [`Values::with_plain`]).
    #[cfg_attr(feature = "observe", track_caller)]
    fn plain_full(&self, id: ValueId) -> Option<Arc<Value>> {
        let i = self.plain_index(id);
        self.plain.get(i)?;
        let x = self.arc_at(i);
        #[cfg(feature = "observe")]
        assert!(x.is_some(), "value {id:?} used after its collection");
        x
    }

    /// The value; a sequence node is built on demand.
    #[cfg_attr(feature = "observe", track_caller)]
    pub fn get(&self, id: ValueId) -> Arc<Value> {
        match id.tag() {
            T_UNIT => Arc::new(Value::Unit),
            T_BOOL => Arc::new(Value::Bool(id.payload() != 0)),
            T_INT => Arc::new(Value::Int(Self::small_int(id))),
            T_SEQ => Arc::new(self.seq_value(id)),
            _ => self.plain_full(id).unwrap_or_else(|| Arc::new(Value::Unit)),
        }
    }

    fn seq_value(&self, id: ValueId) -> Value {
        match self.rope(id) {
            None => Value::Nil,
            Some(r) if r.unit => Value::Seq(SeqNode {
                left: ValueId::NIL,
                elem: r.right,
                right: ValueId::NIL,
                len: 1,
                sealed: (),
            }),
            Some(r) => Value::Seq(SeqNode {
                left: r.left,
                elem: self.seq_first(r.right),
                right: self.seq_drop_first(r.right),
                len: r.len,
                sealed: (),
            }),
        }
    }

    /// `f` on the value without an `Arc` clone for inline values.
    #[cfg_attr(feature = "observe", track_caller)]
    pub fn with<R>(&self, id: ValueId, f: impl FnOnce(&Value) -> R) -> R {
        match id.tag() {
            T_UNIT => f(&Value::Unit),
            T_BOOL => f(&Value::Bool(id.payload() != 0)),
            T_INT => f(&Value::Int(Self::small_int(id))),
            T_SEQ => f(&self.seq_value(id)),
            _ => self.with_plain(id, |x| f(x.unwrap_or(&Value::Unit))),
        }
    }

    /// (a small integer answered inline, a big one out of line: the
    /// slot's load kept out of every caller's frame)
    #[inline]
    #[cfg_attr(feature = "observe", track_caller)]
    pub fn as_int(&self, id: ValueId) -> Option<i64> {
        match id.tag() {
            T_INT => Some(Self::small_int(id)),
            T_BIG => self.big_int(id),
            _ => None,
        }
    }

    #[inline(never)]
    #[cfg_attr(feature = "observe", track_caller)]
    fn big_int(&self, id: ValueId) -> Option<i64> {
        self.with_plain(id, |x| match x {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        })
    }

    #[cfg_attr(feature = "observe", track_caller)]
    pub fn as_str(&self, id: ValueId) -> Option<String> {
        self.with_str(id, str::to_owned)
    }

    #[cfg_attr(feature = "observe", track_caller)]
    pub fn with_str<R>(&self, id: ValueId, f: impl FnOnce(&str) -> R) -> Option<R> {
        if id.tag() != T_STR {
            return None;
        }
        self.with_plain(id, |x| match x {
            Some(Value::Str(s)) => Some(f(s)),
            _ => None,
        })
    }

    #[cfg_attr(feature = "observe", track_caller)]
    pub fn with_list<R>(&self, id: ValueId, f: impl FnOnce(&[ValueId]) -> R) -> Option<R> {
        if id.tag() != T_LIST {
            return None;
        }
        self.with_plain(id, |x| match x {
            Some(Value::List(xs)) => Some(f(xs)),
            _ => None,
        })
    }

    pub fn as_bool(&self, id: ValueId) -> Option<bool> {
        (id.tag() == T_BOOL).then(|| id.payload() != 0)
    }

    #[cfg_attr(feature = "observe", track_caller)]
    fn rope(&self, id: ValueId) -> Option<Rope> {
        if id.tag() != T_SEQ || id == ValueId::NIL {
            return None;
        }
        let p = id.payload();
        let r = self.seqs[shard_of(p)].0.read().get(&p).copied();
        #[cfg(feature = "observe")]
        assert!(
            r.is_some() || !self.gone_ropes.read().contains(&p),
            "sequence {id:?} used after its collection"
        );
        r
    }

    fn insert_rope(&self, payload: u64, r: Rope) {
        if let std::collections::hash_map::Entry::Vacant(e) = self.seqs[shard_of(payload)].0.write().entry(payload) {
            e.insert(r);
            if self.counting.load(Ordering::Relaxed) {
                self.bytes[shard_of(payload) % BYTE_SHARDS].0.fetch_add(Self::ROPE_BYTES, Ordering::Relaxed);
            }
        }
    }

    /// The one-element sequence.
    pub fn seq_unit(&self, x: ValueId) -> ValueId {
        let h = elem_hash(x);
        self.insert_rope(
            h,
            Rope {
                len: 1,
                pow: B,
                left: ValueId::NIL,
                right: x,
                unit: true,
            },
        );
        ValueId::make(T_SEQ, h)
    }

    /// A sequence: a non-sequence is the unit sequence of itself.
    fn as_seq(&self, x: ValueId) -> ValueId {
        if x.tag() == T_SEQ { x } else { self.seq_unit(x) }
    }

    /// `a` then `b`; the id depends on the elements only.
    pub fn seq_concat(&self, a: ValueId, b: ValueId) -> ValueId {
        let (a, b) = (self.as_seq(a), self.as_seq(b));
        if a == ValueId::NIL {
            return b;
        }
        if b == ValueId::NIL {
            return a;
        }
        self.join(a, b)
    }

    /// A rope not in this store (another store's id): a unit of itself.
    fn rope_of(&self, s: ValueId) -> Rope {
        self.rope(s).unwrap_or(Rope {
            len: 1,
            pow: B,
            left: ValueId::NIL,
            right: s,
            unit: true,
        })
    }

    /// The node `l` then `r`.
    fn node(&self, l: ValueId, lr: &Rope, r: ValueId, rr: &Rope) -> ValueId {
        let h = addmod(mulmod(l.payload(), rr.pow), r.payload());
        self.insert_rope(
            h,
            Rope {
                len: lr.len + rr.len,
                pow: mulmod(lr.pow, rr.pow),
                left: l,
                right: r,
                unit: false,
            },
        );
        ValueId::make(T_SEQ, h)
    }

    fn node2(&self, l: ValueId, r: ValueId) -> ValueId {
        self.node(l, &self.rope_of(l), r, &self.rope_of(r))
    }

    /// The weight-balanced join of two non-empty sequences.
    fn join(&self, l: ValueId, r: ValueId) -> ValueId {
        let (lr, rr) = (self.rope_of(l), self.rope_of(r));
        if balanced(lr.len, rr.len) || lr.unit && rr.unit {
            return self.node(l, &lr, r, &rr);
        }
        if lr.len > rr.len && !lr.unit {
            let (a, b) = (lr.left, lr.right);
            let t = self.join(b, r);
            let (ar, tr) = (self.rope_of(a), self.rope_of(t));
            if balanced(ar.len, tr.len) || tr.unit {
                return self.node(a, &ar, t, &tr);
            }
            let (t1, t2) = (tr.left, tr.right);
            let (t1r, t2r) = (self.rope_of(t1), self.rope_of(t2));
            if t1r.unit || balanced(ar.len, t1r.len) && balanced(ar.len + t1r.len, t2r.len) {
                let n = self.node(a, &ar, t1, &t1r);
                return self.node2(n, t2);
            }
            let (t11, t12) = (t1r.left, t1r.right);
            let (x, y) = (self.node2(a, t11), self.node2(t12, t2));
            self.node2(x, y)
        } else if !rr.unit {
            let (c, d) = (rr.left, rr.right);
            let t = self.join(l, c);
            let (tr, dr) = (self.rope_of(t), self.rope_of(d));
            if balanced(tr.len, dr.len) || tr.unit {
                return self.node(t, &tr, d, &dr);
            }
            let (t1, t2) = (tr.left, tr.right);
            let (t1r, t2r) = (self.rope_of(t1), self.rope_of(t2));
            if t2r.unit || balanced(t2r.len, dr.len) && balanced(t1r.len, t2r.len + dr.len) {
                let n = self.node(t2, &t2r, d, &dr);
                return self.node2(t1, n);
            }
            let (t21, t22) = (t2r.left, t2r.right);
            let (x, y) = (self.node2(t1, t21), self.node2(t22, d));
            self.node2(x, y)
        } else {
            self.node(l, &lr, r, &rr)
        }
    }

    fn seq_first(&self, s: ValueId) -> ValueId {
        let mut s = s;
        loop {
            let r = self.rope_of(s);
            if r.unit {
                return r.right;
            }
            s = r.left;
        }
    }

    fn seq_drop_first(&self, s: ValueId) -> ValueId {
        let r = self.rope_of(s);
        if r.unit {
            return ValueId::NIL;
        }
        let l = self.seq_drop_first(r.left);
        self.seq_concat(l, r.right)
    }

    pub fn seq_to_vec(&self, s: ValueId) -> Vec<ValueId> {
        let mut out = Vec::with_capacity(usize::try_from(self.seq_len(s)).unwrap_or(0));
        let mut stack = vec![s];
        while let Some(x) = stack.pop() {
            match self.rope(x) {
                None => {
                    if x.tag() != T_SEQ {
                        out.push(x);
                    }
                }
                Some(r) if r.unit => out.push(r.right),
                Some(r) => {
                    stack.push(r.right);
                    stack.push(r.left);
                }
            }
        }
        out
    }

    /// `Ok(x)` for a unit sequence, `Err((left, right))` for a longer one,
    /// None for NIL or a non-sequence.
    pub fn seq_split(&self, s: ValueId) -> Option<Result<ValueId, (ValueId, ValueId)>> {
        let r = self.rope(s)?;
        Some(if r.unit { Ok(r.right) } else { Err((r.left, r.right)) })
    }

    /// Element `i`, in the rope's depth.
    pub fn seq_nth(&self, s: ValueId, i: u64) -> Option<ValueId> {
        let (mut s, mut i) = (s, i);
        loop {
            let r = self.rope(s)?;
            if r.unit {
                return (i == 0).then_some(r.right);
            }
            let ll = self.rope(r.left).map_or(0, |x| x.len);
            if i < ll {
                s = r.left;
            } else {
                i -= ll;
                s = r.right;
            }
        }
    }

    pub fn seq_len(&self, s: ValueId) -> u64 {
        self.rope(s).map_or(0, |r| r.len)
    }

    /// For diagnostics.
    pub fn render(&self, id: ValueId) -> String {
        match id.tag() {
            T_UNIT => "()".to_owned(),
            T_BOOL => format!("{}", id.payload() != 0),
            T_INT => format!("{}", Self::small_int(id)),
            T_SEQ => {
                let xs = self.seq_to_vec(id);
                let shown: Vec<String> = xs.iter().take(8).map(|&x| self.render(x)).collect();
                let more = if xs.len() > 8 { ", .." } else { "" };
                format!("<{}{more}>", shown.join(", "))
            }
            _ => {
                let x = self.arc_at(self.plain_index(id));
                match x.as_deref() {
                    Some(Value::Str(s)) => format!("{s:?}"),
                    Some(Value::Int(i)) => format!("{i}"),
                    Some(Value::List(xs)) => {
                        let shown: Vec<String> = xs.iter().take(16).map(|&x| self.render(x)).collect();
                        let more = if xs.len() > 16 { ", .." } else { "" };
                        format!("[{}{more}]", shown.join(", "))
                    }
                    _ => format!("#{}", id.0),
                }
            }
        }
    }
}

/// A fast non-cryptographic hasher (FxHash).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for FxHasher {
    /// A word at a time: the bytes in little-endian 8-byte words, the
    /// last one padded with zeros.
    #[inline]
    fn write(&mut self, bytes: &[u8]) {
        let (words, r) = bytes.as_chunks::<8>();
        for w in words {
            self.write_u64(u64::from_le_bytes(*w));
        }
        if !r.is_empty() {
            let mut b = [0u8; 8];
            b[..r.len()].copy_from_slice(r);
            self.write_u64(u64::from_le_bytes(b));
        }
    }

    #[inline]
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(SEED);
    }

    #[inline]
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }

    #[inline]
    fn write_u16(&mut self, i: u16) {
        self.write_u64(u64::from(i));
    }

    #[inline]
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }

    #[inline]
    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxHashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxHashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

/// A hash of words, seeded.
#[inline]
pub fn hash_words(seed: u64, words: &[u64]) -> u64 {
    let mut h = FxHasher(seed);
    for &w in words {
        h.write_u64(w);
    }
    h.finish()
}

/// A hash of bytes, seeded.
#[inline]
pub fn hash_bytes(seed: u64, b: &[u8]) -> u64 {
    let mut h = FxHasher(seed);
    h.write(b);
    h.finish()
}
