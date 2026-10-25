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
struct Rope {
    len: u64,
    pow: u64,
    left: ValueId,
    right: ValueId,
    unit: bool,
}

const SHARDS: usize = 64;

/// The value store.
pub struct Values {
    /// Interned strings, lists and big integers by payload: no lock to read.
    plain: Table<Arc<Value>>,
    /// Content hash of a plain value -> its id; a shard's lock is held for
    /// one insert.
    plain_ix: Box<[Lock<FxHashMap<u64, ValueId>>]>,
    /// Sequence nodes by their id's payload, sharded by it; a lookup holds a
    /// shard's read lock for one map get. Balanced on concat so a sequence
    /// of n elements is O(log n) deep.
    seqs: Box<[Lock<FxHashMap<u64, Rope>>]>,
    /// Distinct per store, for caches keyed by ids (`instance`).
    instance: u64,
}

impl Default for Values {
    fn default() -> Self {
        Values::new()
    }
}

impl Values {
    pub fn new() -> Values {
        todo!()
    }

    /// The id of `v`, made if new.
    pub fn intern(&self, v: Value) -> ValueId {
        todo!()
    }

    /// Plain values interned so far.
    pub fn len(&self) -> usize {
        todo!()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// A number distinct per store, for caches keyed by ids.
    pub fn instance(&self) -> u64 {
        self.instance
    }

    pub fn int(&self, i: i64) -> ValueId {
        todo!()
    }

    pub fn bool(&self, b: bool) -> ValueId {
        todo!()
    }

    pub fn str(&self, s: &str) -> ValueId {
        todo!()
    }

    pub fn list(&self, xs: Vec<ValueId>) -> ValueId {
        todo!()
    }

    pub fn list_from(&self, xs: &[ValueId]) -> ValueId {
        todo!()
    }

    pub fn field(&self, name: &str) -> Field {
        Field(self.str(name))
    }

    /// The value; a sequence node is built on demand.
    pub fn get(&self, id: ValueId) -> Arc<Value> {
        todo!()
    }

    /// `f` on the value without an `Arc` clone for inline values.
    pub fn with<R>(&self, id: ValueId, f: impl FnOnce(&Value) -> R) -> R {
        todo!()
    }

    pub fn as_int(&self, id: ValueId) -> Option<i64> {
        todo!()
    }

    pub fn as_str(&self, id: ValueId) -> Option<String> {
        todo!()
    }

    pub fn with_str<R>(&self, id: ValueId, f: impl FnOnce(&str) -> R) -> Option<R> {
        todo!()
    }

    pub fn with_list<R>(&self, id: ValueId, f: impl FnOnce(&[ValueId]) -> R) -> Option<R> {
        todo!()
    }

    pub fn as_bool(&self, id: ValueId) -> Option<bool> {
        todo!()
    }

    /// The one-element sequence.
    pub fn seq_unit(&self, x: ValueId) -> ValueId {
        todo!()
    }

    /// `a` then `b`; the id depends on the elements only.
    pub fn seq_concat(&self, a: ValueId, b: ValueId) -> ValueId {
        todo!()
    }

    pub fn seq_to_vec(&self, s: ValueId) -> Vec<ValueId> {
        todo!()
    }

    /// `Ok(x)` for a unit sequence, `Err((left, right))` for a longer one,
    /// None for NIL or a non-sequence.
    pub fn seq_split(&self, s: ValueId) -> Option<Result<ValueId, (ValueId, ValueId)>> {
        todo!()
    }

    /// Element `i`, in the rope's depth.
    pub fn seq_nth(&self, s: ValueId, i: u64) -> Option<ValueId> {
        todo!()
    }

    pub fn seq_len(&self, s: ValueId) -> u64 {
        todo!()
    }

    /// For diagnostics.
    pub fn render(&self, id: ValueId) -> String {
        todo!()
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
