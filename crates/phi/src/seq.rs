//! Persistent sequences (DESIGN 7.6): a 32-way tree of `Arc` nodes with
//! structural sharing, `partex-ssa::pvec`'s shape.
//!
//! - A sequence's version is the two-lane polynomial hash of its
//!   elements' versions ([`Poly`]), which combines associatively, so it is
//!   the content's whatever shape the edits gave the tree.
//! - Each element carries an [`ElemId`], its identity, which the client
//!   assigns. Steps and scans key by it. The version leaves ids out.
//! - Each node caches a [`Measure`] of its elements, a monoid: prefix
//!   sums are O(log n) and a splice updates O(log n) summaries.

use std::sync::Arc;

use crate::value::Value;
use crate::ver::{Ver, mulmod61};

/// Fan-out of leaves and inner nodes.
pub const B: usize = 32;

const P: u64 = (1 << 61) - 1;
const BASE: [u64; 2] = [0x0ab5_4c3a_79d1_2e6f, 0x1d6e_93c7_2f58_b40b];

#[inline]
fn addmod(a: u64, b: u64) -> u64 {
    let r = a + b;
    if r >= P { r - P } else { r }
}

/// The polynomial hash of a sequence: its value and `BASE^len`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Poly {
    h: [u64; 2],
    pw: [u64; 2],
}

impl Poly {
    pub const EMPTY: Poly = Poly {
        h: [0, 0],
        pw: [1, 1],
    };

    /// One element of version `v`.
    #[must_use]
    pub fn unit(v: Ver) -> Poly {
        // Truncation intended: the two halves of the version.
        #[allow(clippy::cast_possible_truncation)]
        let (lo, hi) = (v.0 as u64, (v.0 >> 64) as u64);
        Poly {
            h: [hi % (P - 1) + 1, lo % (P - 1) + 1],
            pw: BASE,
        }
    }

    /// The hash of `self` followed by `b`.
    #[must_use]
    pub fn then(self, b: Poly) -> Poly {
        Poly {
            h: [
                addmod(mulmod61(self.h[0], b.pw[0]), b.h[0]),
                addmod(mulmod61(self.h[1], b.pw[1]), b.h[1]),
            ],
            pw: [mulmod61(self.pw[0], b.pw[0]), mulmod61(self.pw[1], b.pw[1])],
        }
    }

    /// The version of a sequence of `len` elements with this hash.
    #[must_use]
    pub fn ver(self, len: usize) -> Ver {
        Ver::node(
            0x7365_7100 ^ len as u64,
            &[Ver(u128::from(self.h[0]) << 64 | u128::from(self.h[1]))],
        )
    }
}

/// An element's identity, assigned by the client (deterministically).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct ElemId(pub u64);

/// A monoid over elements, cached in every node.
pub trait Measure<T>: 'static {
    type S: Copy + Send + Sync + PartialEq + std::fmt::Debug;
    const ID: Self::S;
    fn of(x: &T) -> Self::S;
    fn op(a: Self::S, b: Self::S) -> Self::S;
}

/// No measure.
impl<T> Measure<T> for () {
    type S = ();
    const ID: () = ();
    fn of(_: &T) {}
    fn op((): (), (): ()) {}
}

enum Kind<T, M: Measure<T>> {
    Leaf(Vec<(ElemId, T)>),
    Inner(Vec<Arc<Node<T, M>>>),
}

impl<T: Clone, M: Measure<T>> Clone for Kind<T, M> {
    fn clone(&self) -> Self {
        match self {
            Kind::Leaf(xs) => Kind::Leaf(xs.clone()),
            Kind::Inner(ks) => Kind::Inner(ks.clone()),
        }
    }
}

struct Node<T, M: Measure<T>> {
    len: usize,
    /// The last element's identity (identities increase along an
    /// unfold's input, so [`Seq::index_of`] descends by it).
    hi: ElemId,
    poly: Poly,
    sum: M::S,
    kind: Kind<T, M>,
}

impl<T: Clone, M: Measure<T>> Clone for Node<T, M> {
    fn clone(&self) -> Self {
        Node {
            len: self.len,
            hi: self.hi,
            poly: self.poly,
            sum: self.sum,
            kind: self.kind.clone(),
        }
    }
}

impl<T: Value, M: Measure<T>> Node<T, M> {
    fn new(kind: Kind<T, M>) -> Self {
        let mut n = Node {
            len: 0,
            hi: ElemId(0),
            poly: Poly::EMPTY,
            sum: M::ID,
            kind,
        };
        n.fix();
        n
    }

    fn fix(&mut self) {
        let (mut len, mut poly, mut sum) = (0, Poly::EMPTY, M::ID);
        match &self.kind {
            Kind::Leaf(xs) => {
                self.hi = xs.last().map_or(ElemId(0), |e| e.0);
                for (_, x) in xs {
                    len += 1;
                    poly = poly.then(Poly::unit(x.ver()));
                    sum = M::op(sum, M::of(x));
                }
            }
            Kind::Inner(ks) => {
                self.hi = ks.last().map_or(ElemId(0), |k| k.hi);
                for k in ks {
                    len += k.len;
                    poly = poly.then(k.poly);
                    sum = M::op(sum, k.sum);
                }
            }
        }
        self.len = len;
        self.poly = poly;
        self.sum = sum;
    }

    fn get(&self, mut i: usize) -> &(ElemId, T) {
        let mut n = self;
        loop {
            match &n.kind {
                Kind::Leaf(xs) => return &xs[i],
                Kind::Inner(ks) => {
                    let mut k = 0;
                    while i >= ks[k].len {
                        i -= ks[k].len;
                        k += 1;
                    }
                    n = &ks[k];
                }
            }
        }
    }

    fn prefix(&self, mut i: usize) -> M::S {
        let mut n = self;
        let mut acc = M::ID;
        loop {
            match &n.kind {
                Kind::Leaf(xs) => {
                    for (_, x) in &xs[..i] {
                        acc = M::op(acc, M::of(x));
                    }
                    return acc;
                }
                Kind::Inner(ks) => {
                    let mut k = 0;
                    while k < ks.len() && i >= ks[k].len {
                        i -= ks[k].len;
                        acc = M::op(acc, ks[k].sum);
                        k += 1;
                    }
                    if k == ks.len() {
                        return acc;
                    }
                    n = &ks[k];
                }
            }
        }
    }

    /// Insert at `i`; a node grown past [`B`] splits and returns its
    /// right half.
    fn insert(&mut self, mut i: usize, v: (ElemId, T)) -> Option<Node<T, M>> {
        let split = match &mut self.kind {
            Kind::Leaf(xs) => {
                xs.insert(i, v);
                (xs.len() > B).then(|| Node::new(Kind::Leaf(xs.split_off(xs.len() / 2))))
            }
            Kind::Inner(ks) => {
                let mut k = 0;
                while k + 1 < ks.len() && i > ks[k].len {
                    i -= ks[k].len;
                    k += 1;
                }
                if let Some(right) = Arc::make_mut(&mut ks[k]).insert(i, v) {
                    ks.insert(k + 1, Arc::new(right));
                }
                (ks.len() > B).then(|| Node::new(Kind::Inner(ks.split_off(ks.len() / 2))))
            }
        };
        self.fix();
        split
    }

    /// Remove at `i`; empty children are dropped.
    fn remove(&mut self, mut i: usize) {
        match &mut self.kind {
            Kind::Leaf(xs) => {
                xs.remove(i);
            }
            Kind::Inner(ks) => {
                let mut k = 0;
                while i >= ks[k].len {
                    i -= ks[k].len;
                    k += 1;
                }
                Arc::make_mut(&mut ks[k]).remove(i);
                if ks[k].len == 0 {
                    ks.remove(k);
                }
            }
        }
        self.fix();
    }
}

/// A persistent sequence of values with identities.
pub struct Seq<T, M: Measure<T> = ()> {
    root: Option<Arc<Node<T, M>>>,
    ver: Ver,
}

impl<T, M: Measure<T>> Clone for Seq<T, M> {
    fn clone(&self) -> Self {
        Seq {
            root: self.root.clone(),
            ver: self.ver,
        }
    }
}

impl<T: Value, M: Measure<T>> Default for Seq<T, M> {
    fn default() -> Self {
        Self::new()
    }
}

/// Where two sequences differ: `prefix` elements equal at the start and
/// `suffix` at the end (by identity and version), not overlapping.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Hunk {
    pub prefix: usize,
    pub suffix: usize,
}

impl<T: Value, M: Measure<T>> Seq<T, M> {
    #[must_use]
    pub fn new() -> Self {
        Seq {
            root: None,
            ver: Poly::EMPTY.ver(0),
        }
    }

    /// A sequence of `items`, built bottom-up.
    #[must_use]
    pub fn from_vec(items: Vec<(ElemId, T)>) -> Self {
        if items.is_empty() {
            return Self::new();
        }
        let mut level: Vec<Arc<Node<T, M>>> = Vec::with_capacity(items.len() / B + 1);
        let mut it = items.into_iter().peekable();
        while it.peek().is_some() {
            let mut chunk = Vec::with_capacity(B + 1);
            chunk.extend(it.by_ref().take(B));
            level.push(Arc::new(Node::new(Kind::Leaf(chunk))));
        }
        Self::up(level)
    }

    /// A sequence of the nodes of one level, built up to a root.
    fn up(mut level: Vec<Arc<Node<T, M>>>) -> Self {
        while level.len() > 1 {
            let mut up = Vec::with_capacity(level.len() / B + 1);
            let mut it = level.into_iter().peekable();
            while it.peek().is_some() {
                let mut chunk = Vec::with_capacity(B + 1);
                chunk.extend(it.by_ref().take(B));
                up.push(Arc::new(Node::new(Kind::Inner(chunk))));
            }
            level = up;
        }
        let mut s = Seq {
            root: level.pop(),
            ver: Ver::ABSENT,
        };
        s.fix();
        s
    }

    fn fix(&mut self) {
        if self.root.as_ref().is_some_and(|r| r.len == 0) {
            self.root = None;
        }
        while let Some(r) = &self.root {
            match &r.kind {
                Kind::Inner(ks) if ks.len() == 1 => self.root = Some(ks[0].clone()),
                _ => break,
            }
        }
        self.ver = match &self.root {
            None => Poly::EMPTY.ver(0),
            Some(r) => r.poly.ver(r.len),
        };
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |r| r.len)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The version: the content's, whatever the tree's shape.
    #[must_use]
    pub fn ver(&self) -> Ver {
        self.ver
    }

    /// The polynomial hash, to combine sequences without hashing again.
    #[must_use]
    pub fn poly(&self) -> Poly {
        self.root.as_ref().map_or(Poly::EMPTY, |r| r.poly)
    }

    /// Element `i` with its identity.
    #[must_use]
    pub fn get(&self, i: usize) -> Option<(ElemId, &T)> {
        (i < self.len()).then(|| {
            let (id, x) = self.root.as_ref().expect("non-empty").get(i);
            (*id, x)
        })
    }

    /// The index of the element with identity `id`, in a sequence whose
    /// identities increase (an unfold's input), O(log n).
    #[must_use]
    pub fn index_of(&self, id: ElemId) -> Option<usize> {
        let mut n: &Node<T, M> = self.root.as_ref()?;
        let mut base = 0;
        loop {
            match &n.kind {
                Kind::Leaf(xs) => {
                    return xs.binary_search_by(|e| e.0.cmp(&id)).ok().map(|i| base + i);
                }
                Kind::Inner(ks) => {
                    let mut k = 0;
                    while k < ks.len() && ks[k].hi < id {
                        base += ks[k].len;
                        k += 1;
                    }
                    if k == ks.len() {
                        return None;
                    }
                    n = &ks[k];
                }
            }
        }
    }

    /// Whether the identities increase strictly (an unfold's input must).
    #[must_use]
    pub fn ids_increase(&self) -> bool {
        let mut last = None;
        self.iter().all(|(id, _)| {
            let ok = last.is_none_or(|l| l < id);
            last = Some(id);
            ok
        })
    }

    /// The measure of the whole sequence.
    #[must_use]
    pub fn measure(&self) -> M::S {
        self.root.as_ref().map_or(M::ID, |r| r.sum)
    }

    /// The measure of the first `i` elements, O(log n).
    #[must_use]
    pub fn prefix(&self, i: usize) -> M::S {
        self.root.as_ref().map_or(M::ID, |r| r.prefix(i.min(r.len)))
    }

    /// Insert `x` before element `i` (`i == len` appends).
    ///
    /// # Panics
    ///
    /// If `i` is past the end.
    pub fn insert(&mut self, i: usize, id: ElemId, x: T) {
        assert!(i <= self.len(), "Seq::insert out of range");
        match &mut self.root {
            None => {
                let mut xs = Vec::with_capacity(B + 1);
                xs.push((id, x));
                self.root = Some(Arc::new(Node::new(Kind::Leaf(xs))));
            }
            Some(r) => {
                if let Some(right) = Arc::make_mut(r).insert(i, (id, x)) {
                    let left = self.root.take().expect("root");
                    self.root = Some(Arc::new(Node::new(Kind::Inner(vec![left, Arc::new(right)]))));
                }
            }
        }
        self.fix();
    }

    /// Remove element `i`.
    ///
    /// # Panics
    ///
    /// If `i` is out of range.
    pub fn remove(&mut self, i: usize) {
        assert!(i < self.len(), "Seq::remove out of range");
        Arc::make_mut(self.root.as_mut().expect("non-empty")).remove(i);
        self.fix();
    }

    pub fn push(&mut self, id: ElemId, x: T) {
        self.insert(self.len(), id, x);
    }

    /// `del` elements at `at` replaced by `ins`: a new sequence sharing
    /// every node the splice does not touch.
    #[must_use]
    pub fn splice(&self, at: usize, del: usize, ins: impl IntoIterator<Item = (ElemId, T)>) -> Self {
        let mut s = self.clone();
        for _ in 0..del {
            s.remove(at);
        }
        for (k, (id, x)) in ins.into_iter().enumerate() {
            s.insert(at + k, id, x);
        }
        s
    }

    /// The elements in order, from `from`.
    #[must_use]
    pub fn iter_from(&self, from: usize) -> Iter<'_, T, M> {
        let mut it = Iter { stack: Vec::new() };
        let Some(r) = &self.root else { return it };
        if from >= r.len {
            return it;
        }
        let mut n: &Node<T, M> = r;
        let mut i = from;
        loop {
            match &n.kind {
                Kind::Leaf(_) => {
                    it.stack.push((n, i));
                    return it;
                }
                Kind::Inner(ks) => {
                    let mut k = 0;
                    while i >= ks[k].len {
                        i -= ks[k].len;
                        k += 1;
                    }
                    it.stack.push((n, k + 1));
                    n = &ks[k];
                }
            }
        }
    }

    #[must_use]
    pub fn iter(&self) -> Iter<'_, T, M> {
        self.iter_from(0)
    }

    /// Where `self` differs from `old`: the common prefix and suffix, by
    /// identity and version, skipping subtrees shared by pointer.
    #[must_use]
    pub fn diff(&self, old: &Self) -> Hunk {
        let (n, m) = (self.len(), old.len());
        let mut prefix = match (&self.root, &old.root) {
            (Some(a), Some(b)) => common(a, b, Side::Front).min(n).min(m),
            _ => 0,
        };
        // (where the trees' shapes part, element by element to the edit)
        for ((i, x), (j, y)) in self.iter_from(prefix).zip(old.iter_from(prefix)) {
            if i != j || x.ver() != y.ver() {
                break;
            }
            prefix += 1;
        }
        let room = n.min(m) - prefix;
        let mut suffix = match (&self.root, &old.root) {
            (Some(a), Some(b)) => common(a, b, Side::Back).min(room),
            _ => 0,
        };
        while suffix < room {
            let (x, y) = (self.get(n - 1 - suffix), old.get(m - 1 - suffix));
            match (x, y) {
                (Some((i, x)), Some((j, y))) if i == j && x.ver() == y.ver() => suffix += 1,
                _ => break,
            }
        }
        Hunk { prefix, suffix }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Front,
    Back,
}

/// The number of equal elements at one end of `a` and `b`.
fn common<T: Value, M: Measure<T>>(a: &Arc<Node<T, M>>, b: &Arc<Node<T, M>>, side: Side) -> usize {
    if Arc::ptr_eq(a, b) {
        return a.len;
    }
    if let (Kind::Inner(ka), Kind::Inner(kb)) = (&a.kind, &b.kind) {
        // (children aligned at this end, compared pairwise)
        let pairs: Box<dyn Iterator<Item = (&Arc<Node<T, M>>, &Arc<Node<T, M>>)>> = match side {
            Side::Front => Box::new(ka.iter().zip(kb.iter())),
            Side::Back => Box::new(ka.iter().rev().zip(kb.iter().rev())),
        };
        let mut total = 0;
        for (x, y) in pairs {
            let c = common(x, y, side);
            total += c;
            if c < x.len || x.len != y.len {
                return total;
            }
        }
        return total;
    }
    // (shapes differ: element by element)
    let (n, m) = (a.len, b.len);
    let mut k = 0;
    while k < n && k < m {
        let (i, j) = match side {
            Side::Front => (k, k),
            Side::Back => (n - 1 - k, m - 1 - k),
        };
        let (x, y) = (a.get(i), b.get(j));
        if x.0 != y.0 || x.1.ver() != y.1.ver() {
            break;
        }
        k += 1;
    }
    k
}

impl<T: Value, M: Measure<T>> PartialEq for Seq<T, M> {
    fn eq(&self, other: &Self) -> bool {
        self.ver == other.ver
    }
}
impl<T: Value, M: Measure<T>> Eq for Seq<T, M> {}

impl<T: Value + std::fmt::Debug, M: Measure<T>> std::fmt::Debug for Seq<T, M> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|(_, x)| x)).finish()
    }
}

/// An in-order iterator over a [`Seq`]: identities and elements.
pub struct Iter<'a, T, M: Measure<T>> {
    stack: Vec<(&'a Node<T, M>, usize)>,
}

impl<'a, T, M: Measure<T>> Iterator for Iter<'a, T, M> {
    type Item = (ElemId, &'a T);
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (n, i) = self.stack.last_mut()?;
            let node: &'a Node<T, M> = n;
            match &node.kind {
                Kind::Leaf(xs) => {
                    if *i < xs.len() {
                        *i += 1;
                        let (id, x) = &xs[*i - 1];
                        return Some((*id, x));
                    }
                    self.stack.pop();
                }
                Kind::Inner(ks) => {
                    if *i < ks.len() {
                        *i += 1;
                        let k: &'a Node<T, M> = &ks[*i - 1];
                        self.stack.push((k, 0));
                    } else {
                        self.stack.pop();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct N(u64);
    impl Value for N {
        fn ver(&self) -> Ver {
            Ver::of(&self.0)
        }
    }
    struct Sum;
    impl Measure<N> for Sum {
        type S = u64;
        const ID: u64 = 0;
        fn of(x: &N) -> u64 {
            x.0
        }
        fn op(a: u64, b: u64) -> u64 {
            a + b
        }
    }

    fn rng(x: &mut u64) -> u64 {
        *x ^= *x << 13;
        *x ^= *x >> 7;
        *x ^= *x << 17;
        *x
    }

    #[test]
    fn diff_sees_identity_changes() {
        let a: Seq<N> = Seq::from_vec((0..342).map(|i| (ElemId(i * 10), N(i % 5))).collect());
        let mut v: Vec<(ElemId, N)> = (0..342).map(|i| (ElemId(i * 10), N(i % 5))).collect();
        v[5].0 = ElemId(51);
        v[69].1 = N(77);
        let b: Seq<N> = Seq::from_vec(v);
        let h = b.diff(&a);
        assert_eq!(h.prefix, 5);
        assert_eq!(h.suffix, 342 - 70);
    }

    #[test]
    fn version_independent_of_shape_and_ids() {
        let a: Seq<N> = Seq::from_vec((0..1000).map(|i| (ElemId(i), N(i % 7))).collect());
        let mut b: Seq<N> = Seq::new();
        for i in (0..1000).rev() {
            b.insert(0, ElemId(5000 + i), N(i % 7));
        }
        assert_eq!(a.ver(), b.ver());
        assert_ne!(a.ver(), a.splice(10, 1, [(ElemId(9), N(99))]).ver());
    }

    #[test]
    fn random_splices_match_a_vec() {
        let mut x = 0x1234_5678_9abc_def1;
        let mut v: Vec<(ElemId, N)> = Vec::new();
        let mut s: Seq<N, Sum> = Seq::new();
        let mut next = 0;
        for _ in 0..3000 {
            let at = usize::try_from(rng(&mut x) % (v.len() as u64 + 1)).unwrap();
            let del = usize::try_from(rng(&mut x) % 4).unwrap().min(v.len() - at);
            let ins: Vec<(ElemId, N)> = (0..rng(&mut x) % 5)
                .map(|_| {
                    next += 1;
                    (ElemId(next), N(rng(&mut x) % 100))
                })
                .collect();
            let old = s.clone();
            s = s.splice(at, del, ins.clone());
            v.splice(at..at + del, ins.clone());
            assert_eq!(s.len(), v.len());
            let h = s.diff(&old);
            assert!(h.prefix >= at.min(h.prefix));
            assert!(h.prefix <= s.len() && h.prefix + h.suffix <= s.len().min(old.len()));
            // the hunk's bounds are exact: prefix and suffix equal, next differ
            let get = |q: &Seq<N, Sum>, i: usize| q.get(i).map(|(id, n)| (id, n.0));
            for i in 0..h.prefix {
                assert_eq!(get(&s, i), get(&old, i));
            }
            if h.prefix < s.len().min(old.len()) && h.prefix + h.suffix < s.len().min(old.len()) {
                assert_ne!(get(&s, h.prefix), get(&old, h.prefix));
            }
            for k in 0..h.suffix {
                assert_eq!(get(&s, s.len() - 1 - k), get(&old, old.len() - 1 - k));
            }
            let w: Vec<(ElemId, u64)> = s.iter().map(|(id, n)| (id, n.0)).collect();
            let want: Vec<(ElemId, u64)> = v.iter().map(|(id, n)| (*id, n.0)).collect();
            assert_eq!(w, want);
            let i = usize::try_from(rng(&mut x) % (v.len() as u64 + 1)).unwrap();
            assert_eq!(s.prefix(i), v[..i].iter().map(|e| e.1.0).sum::<u64>());
            let from: Vec<u64> = s.iter_from(i).map(|(_, n)| n.0).collect();
            assert_eq!(from, v[i..].iter().map(|e| e.1.0).collect::<Vec<_>>());
        }
        let fresh: Seq<N, Sum> = Seq::from_vec(v);
        let sorted: Seq<N> = Seq::from_vec((0..2000).map(|i| (ElemId(3 * i + 1), N(i))).collect());
        for i in 0..2000 {
            assert_eq!(sorted.index_of(ElemId(3 * i as u64 + 1)), Some(i));
            assert_eq!(sorted.index_of(ElemId(3 * i as u64 + 2)), None);
        }
        assert_eq!(fresh.ver(), s.ver());
    }
}
