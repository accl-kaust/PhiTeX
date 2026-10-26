//! Values: immutable, interned, equal when their ids are.
//!
//! Small values live in the id itself: unit, booleans, integers of 61 bits.
//! Strings and lists are interned in an append-only table (`plain`, reads
//! without a lock) found through a hash index sharded by hash, whose shard
//! lock is held for one insert. A sequence (the monoid of folds) is a rope
//! whose id is a function of its elements alone, a polynomial hash modulo
//! 2^61 - 1 as a 61-bit payload under the sequence tag: two folds of the
//! same elements made in different shapes are one value, so a reader's
//! `seen` compares by id.

use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

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

/// The value store.
pub struct Values {
    /// Interned strings, lists and big integers by payload: no lock to read.
    plain: Table<Arc<Value>>,
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
        self.plain.get(self.plain_index(id)).is_some_and(|a| **a == *v)
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
            let i = self.plain.push(Arc::new(v));
            let id = ValueId::make(tag, self.base + i as u64);
            w.insert(h, id);
            return id;
        }
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
            _ => match self.plain_value(id).map(|a| &**a) {
                Some(Value::Str(s)) => hash_bytes(T_STR, s.as_bytes()),
                Some(Value::List(xs)) => {
                    let ds: Vec<u64> = xs.iter().map(|&x| self.digest(x)).collect();
                    hash_words(T_LIST, &ds)
                }
                Some(Value::Int(i)) => hash_words(T_BIG, &[i.cast_unsigned()]),
                _ => 0,
            },
        }
    }

    /// A number distinct per store, for caches keyed by ids.
    pub fn instance(&self) -> u64 {
        self.instance
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

    fn plain_value(&self, id: ValueId) -> Option<&Arc<Value>> {
        self.plain.get(self.plain_index(id))
    }

    /// The value; a sequence node is built on demand.
    pub fn get(&self, id: ValueId) -> Arc<Value> {
        match id.tag() {
            T_UNIT => Arc::new(Value::Unit),
            T_BOOL => Arc::new(Value::Bool(id.payload() != 0)),
            T_INT => Arc::new(Value::Int(Self::small_int(id))),
            T_SEQ => Arc::new(self.seq_value(id)),
            _ => self.plain_value(id).cloned().unwrap_or_else(|| Arc::new(Value::Unit)),
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
    pub fn with<R>(&self, id: ValueId, f: impl FnOnce(&Value) -> R) -> R {
        match id.tag() {
            T_UNIT => f(&Value::Unit),
            T_BOOL => f(&Value::Bool(id.payload() != 0)),
            T_INT => f(&Value::Int(Self::small_int(id))),
            T_SEQ => f(&self.seq_value(id)),
            _ => match self.plain_value(id) {
                Some(a) => f(a),
                None => f(&Value::Unit),
            },
        }
    }

    pub fn as_int(&self, id: ValueId) -> Option<i64> {
        match id.tag() {
            T_INT => Some(Self::small_int(id)),
            T_BIG => match self.plain_value(id).map(|a| &**a) {
                Some(Value::Int(i)) => Some(*i),
                _ => None,
            },
            _ => None,
        }
    }

    pub fn as_str(&self, id: ValueId) -> Option<String> {
        self.with_str(id, str::to_owned)
    }

    pub fn with_str<R>(&self, id: ValueId, f: impl FnOnce(&str) -> R) -> Option<R> {
        if id.tag() != T_STR {
            return None;
        }
        match self.plain_value(id).map(|a| &**a) {
            Some(Value::Str(s)) => Some(f(s)),
            _ => None,
        }
    }

    pub fn with_list<R>(&self, id: ValueId, f: impl FnOnce(&[ValueId]) -> R) -> Option<R> {
        if id.tag() != T_LIST {
            return None;
        }
        match self.plain_value(id).map(|a| &**a) {
            Some(Value::List(xs)) => Some(f(xs)),
            _ => None,
        }
    }

    pub fn as_bool(&self, id: ValueId) -> Option<bool> {
        (id.tag() == T_BOOL).then(|| id.payload() != 0)
    }

    fn rope(&self, id: ValueId) -> Option<Rope> {
        if id.tag() != T_SEQ || id == ValueId::NIL {
            return None;
        }
        let p = id.payload();
        self.seqs[shard_of(p)].0.read().get(&p).copied()
    }

    fn insert_rope(&self, payload: u64, r: Rope) {
        self.seqs[shard_of(payload)].0
            .write()
            .entry(payload)
            .or_insert(r);
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
            _ => match self.plain_value(id).map(|a| &**a) {
                Some(Value::Str(s)) => format!("{s:?}"),
                Some(Value::Int(i)) => format!("{i}"),
                Some(Value::List(xs)) => {
                    let shown: Vec<String> = xs.iter().take(16).map(|&x| self.render(x)).collect();
                    let more = if xs.len() > 16 { ", .." } else { "" };
                    format!("[{}{more}]", shown.join(", "))
                }
                _ => format!("#{}", id.0),
            },
        }
    }
}

/// A fast non-cryptographic hasher (FxHash).
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0.rotate_left(5) ^ u64::from(b)).wrapping_mul(SEED);
        }
    }

    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(SEED);
    }

    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }

    fn write_usize(&mut self, i: usize) {
        self.write_u64(i as u64);
    }

    fn finish(&self) -> u64 {
        self.0
    }
}

pub type FxHashMap<K, V> = std::collections::HashMap<K, V, BuildHasherDefault<FxHasher>>;
pub type FxHashSet<K> = std::collections::HashSet<K, BuildHasherDefault<FxHasher>>;

/// A hash of words, seeded.
pub fn hash_words(seed: u64, words: &[u64]) -> u64 {
    let mut h = FxHasher(seed);
    for &w in words {
        h.write_u64(w);
    }
    h.finish()
}

/// A hash of bytes, seeded.
pub fn hash_bytes(seed: u64, b: &[u8]) -> u64 {
    let mut h = FxHasher(seed);
    h.write(b);
    h.finish()
}
