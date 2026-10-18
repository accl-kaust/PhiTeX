//! Content-addressed values.
//!
//! A [`ValueId`] is a function of the value's content alone, so equal
//! values have equal ids in every [`Values`] (the engine's and the
//! reference interpreter's results compare bytewise). Small integers,
//! booleans, unit and the empty sequence are encoded in the id; strings,
//! lists, large integers and sequences are hashed and interned.
//!
//! Sequences hash polynomially modulo 2^61 - 1, so concatenation is
//! associative on ids: `seq_concat` costs O(1) whatever the association,
//! and `seq_to_vec` walks the rope it built.

use std::fmt::Write as _;
use std::hash::{BuildHasherDefault, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::sync::Table;

/// A value's id: 3 tag bits and 61 payload bits.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ValueId(u64);

const TAG_SHIFT: u32 = 61;
const PAYLOAD: u64 = (1 << TAG_SHIFT) - 1;
const T_UNIT: u64 = 0;
const T_BOOL: u64 = 1;
const T_INT: u64 = 2;
const T_SEQ: u64 = 3;
const T_STR: u64 = 4;
const T_LIST: u64 = 5;
const T_BIG: u64 = 6;

/// The Mersenne prime 2^61 - 1.
const P: u64 = PAYLOAD;
/// The base of the sequence hash.
const B: u64 = 0x0b4b_82e9_7f3d_1a2b % P;

#[cfg(feature = "observe")]
impl serde::Serialize for ValueId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        crate::record::named(self.0);
        s.serialize_u64(self.0)
    }
}

#[cfg(feature = "observe")]
impl<'de> serde::Deserialize<'de> for ValueId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        u64::deserialize(d).map(ValueId)
    }
}

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
    pub(crate) const fn from_raw(raw: u64) -> ValueId {
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

/// One node of a sequence: `left`, then `elem`, then `right` (`len`
/// elements in all).
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

/// A sequence node of the rope.
struct Rope {
    len: u64,
    /// B^len mod P.
    pow: u64,
    left: ValueId,
    /// For a unit sequence: the element (and `left` is NIL).
    right: ValueId,
    unit: bool,
}

enum Entry {
    Plain(Arc<Value>),
    Seq(Rope),
}

/// The value store.
pub struct Values {
    table: Table<Entry>,
    count: AtomicUsize,
    instance: u64,
}

impl Default for Values {
    fn default() -> Values {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Values {
            table: Table::new(),
            count: AtomicUsize::new(0),
            instance: NEXT.fetch_add(1, Ordering::Relaxed),
        }
    }
}

const fn mulmod(a: u64, b: u64) -> u64 {
    let r = (a as u128) * (b as u128);
    #[allow(clippy::cast_possible_truncation)]
    let lo = (r as u64) & P;
    #[allow(clippy::cast_possible_truncation)]
    let hi = (r >> TAG_SHIFT) as u64;
    let s = lo + hi;
    if s >= P { s - P } else { s }
}

const fn addmod(a: u64, b: u64) -> u64 {
    let s = a + b;
    if s >= P { s - P } else { s }
}

/// A 64-bit mix (splitmix64's finaliser).
#[must_use]
pub(crate) const fn mix(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// A hash of `words` under `seed` (stable across runs and platforms).
#[must_use]
pub fn hash_words(seed: u64, words: &[u64]) -> u64 {
    hash_word_iter(seed, words.iter().copied())
}

/// [`hash_words`] over an iterator: the same hash, no slice built.
fn hash_word_iter(seed: u64, words: impl Iterator<Item = u64>) -> u64 {
    let mut h = mix(seed ^ 0x9e37_79b9_7f4a_7c15);
    let mut n = 0u64;
    for w in words {
        h = mix(h ^ w).wrapping_add(0x9e37_79b9_7f4a_7c15);
        n += 1;
    }
    mix(h ^ n)
}

/// A hash of `bytes` under `seed` (stable across runs and platforms).
#[must_use]
pub fn hash_bytes(seed: u64, b: &[u8]) -> u64 {
    let mut h = mix(seed ^ (b.len() as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    let (chunks, rest) = b.as_chunks::<8>();
    for c in chunks {
        h = mix(h ^ u64::from_le_bytes(*c)).wrapping_add(0x632b_e59b_d9b4_e019);
    }
    let mut last = [0u8; 8];
    last[..rest.len()].copy_from_slice(rest);
    mix(h ^ u64::from_le_bytes(last))
}

fn hashed(tag: u64, h: u64) -> ValueId {
    // Never payload 0 for hashed tags is not needed: the tag is non-zero.
    ValueId((tag << TAG_SHIFT) | (h & PAYLOAD))
}

const INT_MIN: i64 = -(1 << 60);
const INT_MAX: i64 = (1 << 60) - 1;

impl Values {
    /// Frees the value table's replaced generations (see `Table::reclaim`).
    pub(crate) fn reclaim(&mut self) {
        self.table.reclaim();
    }
}

impl Values {
    #[must_use]
    pub fn new() -> Values {
        Values::default()
    }

    fn put(&self, id: ValueId, make: impl FnOnce() -> Entry) -> ValueId {
        self.table.get_or_insert(id.0, || {
            self.count.fetch_add(1, Ordering::Relaxed);
            make()
        });
        id
    }

    /// Interns `v`. The cost is the size of `v` itself (its parts are ids),
    /// never the size of what it refers to.
    pub fn intern(&self, v: Value) -> ValueId {
        match v {
            Value::Unit => ValueId::UNIT,
            Value::Bool(b) => self.bool(b),
            Value::Int(i) => self.int(i),
            Value::Str(s) => self.str(&s),
            Value::List(xs) => self.list(xs.into_vec()),
            Value::Nil => ValueId::NIL,
            Value::Seq(n) => {
                let l = self.seq_concat(n.left, self.seq_unit(n.elem));
                self.seq_concat(l, n.right)
            }
        }
    }

    /// Number of values in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.count.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Distinguishes this table from others (a client cache of fields or
    /// ids can check it belongs to the same table).
    #[must_use]
    pub fn instance(&self) -> u64 {
        self.instance
    }

    /// An integer.
    pub fn int(&self, i: i64) -> ValueId {
        if (INT_MIN..=INT_MAX).contains(&i) {
            #[allow(clippy::cast_sign_loss)]
            return ValueId((T_INT << TAG_SHIFT) | ((i as u64) & PAYLOAD));
        }
        #[allow(clippy::cast_sign_loss)]
        let id = hashed(T_BIG, hash_words(T_BIG, &[i as u64]));
        self.put(id, || Entry::Plain(Arc::new(Value::Int(i))))
    }

    /// A boolean.
    #[must_use]
    pub fn bool(&self, b: bool) -> ValueId {
        ValueId((T_BOOL << TAG_SHIFT) | u64::from(b))
    }

    /// A string.
    pub fn str(&self, s: &str) -> ValueId {
        let id = hashed(T_STR, hash_bytes(T_STR, s.as_bytes()));
        self.put(id, || Entry::Plain(Arc::new(Value::Str(s.into()))))
    }

    /// A list.
    pub fn list(&self, xs: Vec<ValueId>) -> ValueId {
        let id = hashed(T_LIST, hash_word_iter(T_LIST, xs.iter().map(|x| x.0)));
        self.put(id, || Entry::Plain(Arc::new(Value::List(xs.into_boxed_slice()))))
    }

    /// A list from a borrowed slice: allocates only when the list is new
    /// to the table (the same id as [`Values::list`]).
    pub fn list_from(&self, xs: &[ValueId]) -> ValueId {
        let id = hashed(T_LIST, hash_word_iter(T_LIST, xs.iter().map(|x| x.0)));
        self.put(id, || Entry::Plain(Arc::new(Value::List(xs.into()))))
    }

    /// The field named `name`.
    pub fn field(&self, name: &str) -> Field {
        Field(self.str(name))
    }

    /// The value behind `id`.
    ///
    /// # Panics
    ///
    /// If `id` is unknown (never interned, or freed).
    #[must_use]
    pub fn get(&self, id: ValueId) -> Arc<Value> {
        if matches!(id.tag(), T_UNIT | T_BOOL | T_INT) {
            return Arc::new(self.with(id, Clone::clone));
        }
        match self.table.get(id.0) {
            Some(Entry::Plain(v)) => v.clone(),
            _ => Arc::new(self.with(id, Clone::clone)),
        }
    }

    /// Runs `f` on the value behind `id`, borrowed: no clone of a stored
    /// value and no allocation for an inline one (unlike [`Values::get`],
    /// which hands out an `Arc`: a shared count written per call, and a
    /// fresh allocation for an int, bool or unit).
    pub fn with<R>(&self, id: ValueId, f: impl FnOnce(&Value) -> R) -> R {
        match id.tag() {
            T_UNIT => f(&Value::Unit),
            T_BOOL => f(&Value::Bool(id.payload() != 0)),
            T_INT => {
                // Sign-extend the 61-bit payload.
                #[allow(clippy::cast_possible_wrap)]
                let i = ((id.payload() << 3) as i64) >> 3;
                f(&Value::Int(i))
            }
            T_SEQ if id == ValueId::NIL => f(&Value::Nil),
            _ => match self.table.get(id.0) {
                Some(Entry::Plain(v)) => f(v),
                Some(Entry::Seq(r)) => f(&Value::Seq(self.seq_node(id, r))),
                None => panic!("unknown or freed value {id:?}"),
            },
        }
    }

    /// A sequence as `left`, `elem`, `right`.
    fn seq_node(&self, id: ValueId, r: &Rope) -> SeqNode {
        if r.unit {
            return SeqNode {
                left: ValueId::NIL,
                elem: r.right,
                right: ValueId::NIL,
                len: 1,
                sealed: (),
            };
        }
        let (elem, rest) = self.split_first(r.right);
        let _ = id;
        SeqNode {
            left: r.left,
            elem,
            right: rest,
            len: r.len,
            sealed: (),
        }
    }

    /// The first element of a non-empty sequence and the rest.
    fn split_first(&self, s: ValueId) -> (ValueId, ValueId) {
        match self.table.get(s.0) {
            Some(Entry::Seq(r)) if r.unit => (r.right, ValueId::NIL),
            Some(Entry::Seq(r)) => {
                let (e, rest) = self.split_first(r.left);
                (e, self.seq_concat(rest, r.right))
            }
            _ => panic!("not a sequence: {s:?}"),
        }
    }

    /// Whether `id` is known to this store.
    #[must_use]
    pub(crate) fn contains(&self, id: ValueId) -> bool {
        matches!(id.tag(), T_UNIT | T_BOOL | T_INT) || id == ValueId::NIL || self.table.get(id.0).is_some()
    }

    /// The integer `id` holds, if it is one.
    #[must_use]
    pub fn as_int(&self, id: ValueId) -> Option<i64> {
        match id.tag() {
            // Inline: decoded from the id (sign-extending the payload).
            #[allow(clippy::cast_possible_wrap)]
            T_INT => Some(((id.payload() << 3) as i64) >> 3),
            T_BIG => self.with(id, |v| match v {
                Value::Int(i) => Some(*i),
                _ => None,
            }),
            _ => None,
        }
    }

    /// The string `id` holds, if it is one.
    #[must_use]
    pub fn as_str(&self, id: ValueId) -> Option<String> {
        if id.tag() != T_STR {
            return None;
        }
        self.with(id, |v| match v {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        })
    }

    /// Runs `f` on the string `id` holds, borrowed (`None`: not a string).
    pub fn with_str<R>(&self, id: ValueId, f: impl FnOnce(&str) -> R) -> Option<R> {
        if id.tag() != T_STR {
            return None;
        }
        self.with(id, |v| match v {
            Value::Str(s) => Some(f(s)),
            _ => None,
        })
    }

    /// Runs `f` on the elements of the list `id` holds, borrowed (`None`:
    /// not a list).
    pub fn with_list<R>(&self, id: ValueId, f: impl FnOnce(&[ValueId]) -> R) -> Option<R> {
        self.with(id, |v| match v {
            Value::List(xs) => Some(f(xs)),
            _ => None,
        })
    }

    /// The boolean `id` holds, if it is one.
    #[must_use]
    pub fn as_bool(&self, id: ValueId) -> Option<bool> {
        (id.tag() == T_BOOL).then(|| id.payload() != 0)
    }

    fn seq_meta(&self, s: ValueId) -> (u64, u64) {
        if s == ValueId::NIL {
            return (0, 1);
        }
        match self.table.get(s.0) {
            Some(Entry::Seq(n)) => (n.len, n.pow),
            _ => panic!("not a sequence: {s:?}"),
        }
    }

    /// The one-element sequence of `x`.
    pub fn seq_unit(&self, x: ValueId) -> ValueId {
        let h = mix(x.0 ^ 0x5851_f42d_4c95_7f2d) % P;
        let id = hashed(T_SEQ, h.max(1));
        self.put(id, || {
            Entry::Seq(Rope {
                len: 1,
                pow: B,
                left: ValueId::NIL,
                right: x,
                unit: true,
            })
        })
    }

    /// The concatenation of sequences `a` and `b` (associative on ids).
    pub fn seq_concat(&self, a: ValueId, b: ValueId) -> ValueId {
        if a == ValueId::NIL {
            return b;
        }
        if b == ValueId::NIL {
            return a;
        }
        let (la, pa) = self.seq_meta(a);
        let (lb, pb) = self.seq_meta(b);
        let h = addmod(mulmod(a.payload(), pb), b.payload());
        let id = hashed(T_SEQ, h.max(1));
        self.put(id, || {
            Entry::Seq(Rope {
                len: la + lb,
                pow: mulmod(pa, pb),
                left: a,
                right: b,
                unit: false,
            })
        })
    }

    /// The elements of sequence `s`, in order.
    pub fn seq_to_vec(&self, s: ValueId) -> Vec<ValueId> {
        let mut out = Vec::new();
        if s.tag() != T_SEQ {
            return out;
        }
        let mut stack = vec![s];
        while let Some(x) = stack.pop() {
            if x == ValueId::NIL {
                continue;
            }
            match self.table.get(x.0) {
                Some(Entry::Seq(n)) if n.unit => out.push(n.right),
                Some(Entry::Seq(n)) => {
                    stack.push(n.right);
                    stack.push(n.left);
                }
                _ => {}
            }
        }
        out
    }

    /// The length of sequence `s`.
    pub fn seq_len(&self, s: ValueId) -> u64 {
        if s.tag() == T_SEQ { self.seq_meta(s).0 } else { 0 }
    }

    /// `id` as text: integers, `"strings"`, `[lists]`, `<sequences>`,
    /// `()`, `true`/`false`.
    pub fn render(&self, id: ValueId) -> String {
        let mut s = String::new();
        self.render_into(id, &mut s, 0);
        s
    }

    fn render_into(&self, id: ValueId, out: &mut String, depth: usize) {
        if depth > 64 {
            out.push_str("..");
            return;
        }
        if !self.contains(id) {
            let _ = write!(out, "?{id:?}");
            return;
        }
        if id.tag() == T_SEQ {
            out.push('<');
            for (i, x) in self.seq_to_vec(id).into_iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                self.render_into(x, out, depth + 1);
            }
            out.push('>');
            return;
        }
        let v = self.get(id);
        match &*v {
            Value::Unit => out.push_str("()"),
            Value::Bool(b) => {
                let _ = write!(out, "{b}");
            }
            Value::Int(i) => {
                let _ = write!(out, "{i}");
            }
            Value::Str(s) => {
                let _ = write!(out, "{:?}", &**s);
            }
            Value::List(xs) => {
                out.push('[');
                for (i, &x) in xs.iter().enumerate() {
                    if i > 0 {
                        out.push(' ');
                    }
                    self.render_into(x, out, depth + 1);
                }
                out.push(']');
            }
            Value::Nil | Value::Seq(_) => {}
        }
    }
}

/// The Fx hash (rustc's): fast, for integer-like keys.
#[derive(Default, Clone, Copy)]
pub struct FxHasher(u64);

const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

impl Hasher for FxHasher {
    fn write(&mut self, bytes: &[u8]) {
        for c in bytes.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u8(&mut self, i: u8) {
        self.write_u64(u64::from(i));
    }
    fn write_u32(&mut self, i: u32) {
        self.write_u64(u64::from(i));
    }
    fn write_u64(&mut self, i: u64) {
        self.0 = (self.0.rotate_left(5) ^ i).wrapping_mul(SEED);
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

/// A small deterministic PRNG (xorshift64*).
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    #[must_use]
    pub fn new(seed: u64) -> Rng {
        Rng(mix(seed.wrapping_add(0x9e37_79b9_7f4a_7c15)) | 1)
    }

    pub(crate) fn state(&self) -> u64 {
        self.0
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..n` (`n > 0`).
    pub fn below(&mut self, n: usize) -> usize {
        #[allow(clippy::cast_possible_truncation)]
        let r = ((u128::from(self.next_u64()) * n as u128) >> 64) as usize;
        r
    }

    /// True with probability `a / b`.
    pub fn chance(&mut self, a: u64, b: u64) -> bool {
        self.next_u64() % b.max(1) < a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_content_addressed() {
        let a = Values::new();
        let b = Values::new();
        let la = a.list(vec![a.int(1), a.str("x"), a.int(1 << 62)]);
        let lb = b.list(vec![b.int(1), b.str("x"), b.int(1 << 62)]);
        assert_eq!(la, lb);
        assert_eq!(a.render(la), "[1 \"x\" 4611686018427387904]");
        assert_eq!(a.as_int(a.int(-5)), Some(-5));
        assert_eq!(a.as_int(a.int(i64::MIN)), Some(i64::MIN));
        assert_eq!(a.render(ValueId::UNIT), "()");
    }

    #[test]
    fn sequences_associate() {
        let v = Values::new();
        let xs: Vec<_> = (0..20).map(|i| v.seq_unit(v.int(i % 3))).collect();
        let left = xs.iter().fold(ValueId::NIL, |a, &x| v.seq_concat(a, x));
        let right = xs.iter().rev().fold(ValueId::NIL, |a, &x| v.seq_concat(x, a));
        assert_eq!(left, right);
        assert_eq!(v.seq_to_vec(left).len(), 20);
        assert_eq!(v.render(v.seq_concat(xs[0], xs[1])), "<0 1>");
    }
}
