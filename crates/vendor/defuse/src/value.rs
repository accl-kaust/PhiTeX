//! Content-addressed immutable values.
//!
//! A [`ValueId`] is a 64-bit hash of the value's content (composites hash the
//! ids of their parts), so ids are the same under every schedule and in every
//! process. Equal values have equal ids; a hash collision between different
//! values is detected and panics.
//!
//! [`Value::Seq`] is a canonical, structurally shared sequence (a treap whose
//! shape is a function of its elements), so concatenation shares both sides
//! and equal sequences have equal ids however they were built.

use std::fmt::Write as _;
use std::sync::{Arc, PoisonError, RwLock};

use crate::hash::{FxHashMap, FxHashSet, hash_bytes, hash_words};

/// Content hash of a value.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct ValueId(pub(crate) u64);

impl ValueId {
    /// The unit value.
    pub const UNIT: ValueId = ValueId(0x0a11_ce00_0000_0001);
    /// The empty sequence.
    pub const NIL: ValueId = ValueId(0x0a11_ce00_0000_0002);
    /// "No value" (never interned).
    pub(crate) const NONE: ValueId = ValueId(0);

    /// The raw 64-bit content hash.
    #[must_use]
    pub fn raw(self) -> u64 {
        self.0
    }
}

/// A field name: any value can name a field.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Debug)]
pub struct Field(pub ValueId);

/// One node of a canonical sequence treap.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct SeqNode {
    pub left: ValueId,
    pub elem: ValueId,
    pub right: ValueId,
    pub len: u64,
    pub(crate) prio: u64,
    pub(crate) first: ValueId,
    pub(crate) lead: u64,
    pub(crate) last: ValueId,
    pub(crate) trail: u64,
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

fn content_id(v: &Value) -> ValueId {
    let h = match v {
        Value::Unit => return ValueId::UNIT,
        Value::Nil => return ValueId::NIL,
        Value::Bool(b) => hash_words(1, &[u64::from(*b)]),
        #[allow(clippy::cast_sign_loss)]
        Value::Int(i) => hash_words(2, &[*i as u64]),
        Value::Str(s) => hash_bytes(3, s.as_bytes()),
        Value::List(xs) => {
            let ws: Vec<u64> = xs.iter().map(|x| x.0).collect();
            hash_words(4, &ws)
        }
        Value::Seq(n) => hash_words(5, &[n.left.0, n.elem.0, n.right.0, n.prio]),
    };
    // Keep the reserved ids free.
    if h == ValueId::NONE.0 || h == ValueId::UNIT.0 || h == ValueId::NIL.0 {
        ValueId(h ^ 0x8000_0000_0000_0000)
    } else {
        ValueId(h)
    }
}

/// The value table, shared by all workers.
pub struct Values {
    /// Sharded by id (ids are content hashes, so shards are even), so
    /// concurrent readers on different values do not contend.
    shards: Box<[Shard]>,
    /// Distinguishes tables in the per-thread read cache.
    instance: u64,
}

static INSTANCES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

type Shard = RwLock<FxHashMap<ValueId, Arc<Value>>>;

const SHARDS: usize = 64;

/// A read-cache slot: table instance, id, and the private copy once made.
type ReadSlot = (u64, ValueId, Option<Arc<Value>>);

/// Slots in a thread's read cache (direct-mapped by id).
const READ_SLOTS: usize = 1 << 12;

thread_local! {
    /// Each thread's own copies of the values it reads repeatedly: hot
    /// values (a client's syntax tags, small constants) are read by every
    /// worker, and a shared lock or reference count on them would bounce
    /// between cores. A value read once is shared, not copied; on its second
    /// read in a while it gets a private copy. Ids are content hashes, so an
    /// entry is never stale.
    static READS: std::cell::RefCell<Vec<ReadSlot>> =
        std::cell::RefCell::new(vec![(0, ValueId::NONE, None); READ_SLOTS]);
}

impl Values {
    fn shard(&self, id: ValueId) -> &Shard {
        &self.shards[(id.0 >> 58) as usize % SHARDS]
    }
}

impl Default for Values {
    fn default() -> Self {
        let v = Values {
            shards: (0..SHARDS).map(|_| RwLock::new(FxHashMap::default())).collect(),
            instance: INSTANCES.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        };
        for (id, x) in [(ValueId::UNIT, Value::Unit), (ValueId::NIL, Value::Nil)] {
            v.shard(id)
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(id, Arc::new(x));
        }
        v
    }
}

impl Values {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Interns `v`. The cost is the size of `v` itself (its parts are ids),
    /// never the size of what it refers to.
    ///
    /// # Panics
    ///
    /// On a 64-bit content hash collision between different values.
    pub fn intern(&self, v: Value) -> ValueId {
        crate::obs::count!(ValueIntern);
        let id = content_id(&v);
        {
            let t = self.shard(id).read().unwrap_or_else(PoisonError::into_inner);
            if let Some(old) = t.get(&id) {
                assert!(**old == v, "value hash collision");
                return id;
            }
        }
        let mut t = self.shard(id).write().unwrap_or_else(PoisonError::into_inner);
        if let Some(old) = t.get(&id) {
            assert!(**old == v, "value hash collision");
            return id;
        }
        t.insert(id, Arc::new(v));
        id
    }

    /// The value behind `id`.
    ///
    /// # Panics
    ///
    /// If `id` is unknown (never interned, or freed).
    #[must_use]
    pub fn get(&self, id: ValueId) -> Arc<Value> {
        crate::obs::count!(ValueGet);
        #[allow(clippy::cast_possible_truncation)]
        let slot = (id.0 as usize ^ self.instance as usize) % READ_SLOTS;
        let seen = READS.with(|c| {
            let c = c.borrow();
            let e = &c[slot];
            if e.0 == self.instance && e.1 == id {
                Some(e.2.clone())
            } else {
                None
            }
        });
        if let Some(Some(v)) = seen {
            return v;
        }
        crate::obs::count!(ValueGetMiss);
        let shared = self
            .shard(id)
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .cloned()
            .expect("unknown or freed value");
        // Second read: a private copy, whose count is this thread's alone.
        let private = seen.is_some().then(|| Arc::new(Value::clone(&shared)));
        READS.with(|c| c.borrow_mut()[slot] = (self.instance, id, private));
        shared
    }

    /// Number of values in the table.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shards
            .iter()
            .map(|s| s.read().unwrap_or_else(PoisonError::into_inner).len())
            .sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        false
    }

    #[must_use]
    pub fn int(&self, i: i64) -> ValueId {
        self.intern(Value::Int(i))
    }

    #[must_use]
    pub fn bool(&self, b: bool) -> ValueId {
        self.intern(Value::Bool(b))
    }

    #[must_use]
    pub fn str(&self, s: &str) -> ValueId {
        self.intern(Value::Str(s.into()))
    }

    #[must_use]
    pub fn list(&self, items: Vec<ValueId>) -> ValueId {
        self.intern(Value::List(items.into_boxed_slice()))
    }

    /// The field named by the string `name`.
    #[must_use]
    pub fn field(&self, name: &str) -> Field {
        Field(self.str(name))
    }

    #[must_use]
    pub fn as_int(&self, id: ValueId) -> Option<i64> {
        match *self.get(id) {
            Value::Int(i) => Some(i),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_bool(&self, id: ValueId) -> Option<bool> {
        match *self.get(id) {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    #[must_use]
    pub fn as_str(&self, id: ValueId) -> Option<String> {
        match &*self.get(id) {
            Value::Str(s) => Some(s.to_string()),
            _ => None,
        }
    }

    /// Frees every value not reachable from `live`. Returns how many.
    pub(crate) fn sweep(&self, live: &FxHashSet<ValueId>) -> usize {
        let mut marked: FxHashSet<ValueId> = FxHashSet::default();
        let mut stack: Vec<ValueId> = live.iter().copied().collect();
        let mut t: Vec<_> = self
            .shards
            .iter()
            .map(|s| s.write().unwrap_or_else(PoisonError::into_inner))
            .collect();
        while let Some(v) = stack.pop() {
            if v == ValueId::NONE || !marked.insert(v) {
                continue;
            }
            match t[(v.0 >> 58) as usize % SHARDS].get(&v).map(|a| &**a) {
                Some(Value::List(xs)) => stack.extend(xs.iter().copied()),
                Some(Value::Seq(n)) => stack.extend([n.left, n.elem, n.right, n.first, n.last]),
                _ => {}
            }
        }
        let mut freed = 0;
        for t in &mut t {
            let before = t.len();
            t.retain(|k, _| marked.contains(k) || *k == ValueId::UNIT || *k == ValueId::NIL);
            freed += before - t.len();
        }
        freed
    }

    /// Renders a value; equal values render equally.
    #[must_use]
    pub fn render(&self, id: ValueId) -> String {
        let mut s = String::new();
        self.render_into(id, &mut s);
        s
    }

    fn render_into(&self, id: ValueId, s: &mut String) {
        match &*self.get(id) {
            Value::Unit => s.push_str("()"),
            Value::Bool(b) => {
                let _ = write!(s, "{b}");
            }
            Value::Int(i) => {
                let _ = write!(s, "{i}");
            }
            Value::Str(x) => {
                let _ = write!(s, "{x:?}");
            }
            Value::List(xs) => {
                s.push('[');
                for (i, x) in xs.iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    self.render_into(*x, s);
                }
                s.push(']');
            }
            Value::Nil | Value::Seq(_) => {
                s.push('<');
                for (i, x) in self.seq_to_vec(id).into_iter().enumerate() {
                    if i > 0 {
                        s.push(' ');
                    }
                    self.render_into(x, s);
                }
                s.push('>');
            }
        }
    }

    // ------------------------------------------------------------ sequences
    //
    // A canonical treap: element i's priority is hash(element, r) where r is
    // the number of equal elements directly before it. The shape is then a
    // function of the sequence alone (on equal priorities the left one is the
    // root), and runs of equal elements still get distinct priorities, so the
    // expected depth stays logarithmic.

    fn seq(&self, id: ValueId) -> Option<SeqNode> {
        match &*self.get(id) {
            Value::Seq(n) => Some(n.clone()),
            _ => None,
        }
    }

    /// Length of a sequence (0 for anything that is not one).
    #[must_use]
    pub fn seq_len(&self, id: ValueId) -> u64 {
        self.seq(id).map_or(0, |n| n.len)
    }

    fn prio(elem: ValueId, rank: u64) -> u64 {
        hash_words(6, &[elem.0, rank])
    }

    fn make(&self, left: ValueId, elem: ValueId, prio: u64, right: ValueId) -> ValueId {
        type Seg = (ValueId, u64, ValueId, u64, u64); // first, lead, last, trail, len
        fn comb(a: Seg, b: Seg) -> Seg {
            let lead = if a.1 == a.4 && a.2 == b.0 { a.4 + b.1 } else { a.1 };
            let trail = if b.3 == b.4 && a.2 == b.0 { b.4 + a.3 } else { b.3 };
            (a.0, lead, b.2, trail, a.4 + b.4)
        }
        let seg = |n: Option<SeqNode>| n.map(|n| (n.first, n.lead, n.last, n.trail, n.len));
        let mut s: Seg = (elem, 1, elem, 1, 1);
        if let Some(a) = seg(self.seq(left)) {
            s = comb(a, s);
        }
        if let Some(b) = seg(self.seq(right)) {
            s = comb(s, b);
        }
        self.intern(Value::Seq(SeqNode {
            left,
            elem,
            right,
            len: s.4,
            prio,
            first: s.0,
            lead: s.1,
            last: s.2,
            trail: s.3,
        }))
    }

    fn merge(&self, a: ValueId, b: ValueId) -> ValueId {
        let (Some(x), Some(y)) = (self.seq(a), self.seq(b)) else {
            return if a == ValueId::NIL { b } else { a };
        };
        if x.prio >= y.prio {
            let r = self.merge(x.right, b);
            self.make(x.left, x.elem, x.prio, r)
        } else {
            let l = self.merge(a, y.left);
            self.make(l, y.elem, y.prio, y.right)
        }
    }

    fn split_at(&self, t: ValueId, k: u64) -> (ValueId, ValueId) {
        let Some(n) = self.seq(t) else {
            return (ValueId::NIL, ValueId::NIL);
        };
        let ll = self.seq_len(n.left);
        if k <= ll {
            let (a, b) = self.split_at(n.left, k);
            (a, self.make(b, n.elem, n.prio, n.right))
        } else {
            let (a, b) = self.split_at(n.right, k - ll - 1);
            (self.make(n.left, n.elem, n.prio, a), b)
        }
    }

    fn run(&self, x: ValueId, start: u64, count: u64) -> ValueId {
        let mut t = ValueId::NIL;
        for i in 0..count {
            let one = self.make(ValueId::NIL, x, Self::prio(x, start + i), ValueId::NIL);
            t = self.merge(t, one);
        }
        t
    }

    /// The one-element sequence.
    #[must_use]
    pub fn seq_unit(&self, x: ValueId) -> ValueId {
        self.make(ValueId::NIL, x, Self::prio(x, 0), ValueId::NIL)
    }

    /// Concatenates two sequences in O(log n) (plus the length of a run of
    /// equal elements spanning the join), sharing both sides.
    #[must_use]
    pub fn seq_concat(&self, a: ValueId, b: ValueId) -> ValueId {
        let (Some(x), Some(y)) = (self.seq(a), self.seq(b)) else {
            return if a == ValueId::NIL { b } else { a };
        };
        if x.last == y.first {
            // The leading run of `b` changes rank: rebuild just that run.
            let (_, rest) = self.split_at(b, y.lead);
            let fixed = self.run(y.first, x.trail, y.lead);
            let ab = self.merge(a, fixed);
            self.merge(ab, rest)
        } else {
            self.merge(a, b)
        }
    }

    /// The elements of a sequence.
    #[must_use]
    pub fn seq_to_vec(&self, id: ValueId) -> Vec<ValueId> {
        let mut out = Vec::new();
        self.seq_collect(id, &mut out);
        out
    }

    fn seq_collect(&self, id: ValueId, out: &mut Vec<ValueId>) {
        if let Some(n) = self.seq(id) {
            self.seq_collect(n.left, out);
            out.push(n.elem);
            self.seq_collect(n.right, out);
        }
    }
}
