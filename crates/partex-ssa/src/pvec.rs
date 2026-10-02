//! A persistent vector: a chunked tree (up to [`B`] elements per leaf,
//! [`B`] children per node) with structural sharing and O(log n) get,
//! set, insert and remove.
//!
//! Each node carries a version of its elements in order. The version is
//! a polynomial hash over two lanes modulo 2⁶¹−1, which combines
//! associatively (`h(ab) = h(a)·Bˡᵉⁿ⁽ᵇ⁾ + h(b)`), so a node's version is
//! made from its children's in O(children), and two vectors with equal
//! content have equal versions whatever shape their edit history gave
//! the tree.

use alloc::sync::Arc;
use alloc::vec::Vec;
use core::fmt;

use crate::hash::Version;
use crate::value::Value;

/// Fan-out of leaves and inner nodes.
pub const B: usize = 32;

const P: u64 = (1 << 61) - 1;
const BASE: [u64; 2] = [0x0ab5_4c3a_79d1_2e6f, 0x1d6e_93c7_2f58_b40b];

#[inline]
fn mulmod(a: u64, b: u64) -> u64 {
    let x = u128::from(a) * u128::from(b);
    // Truncation intended: the reduction modulo 2^61 - 1.
    #[allow(clippy::cast_possible_truncation)]
    let (lo, hi) = ((x as u64) & P, (x >> 61) as u64);
    let r = lo + hi;
    if r >= P { r - P } else { r }
}

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
    pub fn unit(v: Version) -> Poly {
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
                addmod(mulmod(self.h[0], b.pw[0]), b.h[0]),
                addmod(mulmod(self.h[1], b.pw[1]), b.h[1]),
            ],
            pw: [mulmod(self.pw[0], b.pw[0]), mulmod(self.pw[1], b.pw[1])],
        }
    }

    /// The version of a sequence of `len` elements with this hash.
    #[must_use]
    pub fn version(self, len: usize) -> Version {
        Version::node(
            0x7365_7100 ^ len as u64,
            &[Version(u128::from(self.h[0]) << 64 | u128::from(self.h[1]))],
        )
    }
}

#[derive(Clone)]
enum Kind<T> {
    Leaf(Vec<T>),
    Inner(Vec<Arc<Node<T>>>),
}

#[derive(Clone)]
struct Node<T> {
    len: usize,
    poly: Poly,
    /// The hash of all elements (or children) but the last: an append or
    /// a change of the last one makes `poly` in O(1) (`head.then(last)`).
    head: Poly,
    kind: Kind<T>,
}

impl<T: Value> Node<T> {
    fn leaf(items: Vec<T>) -> Node<T> {
        let mut n = Node {
            len: 0,
            poly: Poly::EMPTY,
            head: Poly::EMPTY,
            kind: Kind::Leaf(items),
        };
        n.fix();
        n
    }

    fn inner(kids: Vec<Arc<Node<T>>>) -> Node<T> {
        let mut n = Node {
            len: 0,
            poly: Poly::EMPTY,
            head: Poly::EMPTY,
            kind: Kind::Inner(kids),
        };
        n.fix();
        n
    }

    /// Recompute the length and the version from the parts.
    fn fix(&mut self) {
        let (len, head, last) = match &self.kind {
            Kind::Leaf(xs) => {
                let n = xs.len();
                let head = xs[..n.saturating_sub(1)]
                    .iter()
                    .fold(Poly::EMPTY, |p, x| p.then(Poly::unit(x.version())));
                let last = xs.last().map_or(Poly::EMPTY, |x| Poly::unit(x.version()));
                (n, head, last)
            }
            Kind::Inner(ks) => {
                let n = ks.len();
                let (l, head) = ks[..n.saturating_sub(1)]
                    .iter()
                    .fold((0, Poly::EMPTY), |(l, p), k| (l + k.len, p.then(k.poly)));
                let (ll, last) = ks.last().map_or((0, Poly::EMPTY), |k| (k.len, k.poly));
                (l + ll, head, last)
            }
        };
        self.len = len;
        self.head = head;
        self.poly = head.then(last);
    }

    /// The version again after the last element (or the last child) changed,
    /// `grew` elements longer: O(1) from `head`. A leaf that grew by one
    /// had the element appended, so its old hash is the new `head`.
    fn fix_last(&mut self, grew: usize) {
        match &self.kind {
            Kind::Leaf(xs) => {
                if grew == 1 {
                    self.head = self.poly;
                }
                self.len = xs.len();
                let last = xs.last().map_or(Poly::EMPTY, |x| Poly::unit(x.version()));
                self.poly = self.head.then(last);
            }
            Kind::Inner(ks) => {
                let k = ks.last().expect("a child");
                self.len += grew;
                self.poly = self.head.then(k.poly);
            }
        }
    }

    fn get(&self, mut i: usize) -> &T {
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

    fn set(&mut self, mut i: usize, v: T) {
        let last = i + 1 == self.len;
        match &mut self.kind {
            Kind::Leaf(xs) => xs[i] = v,
            Kind::Inner(ks) => {
                let mut k = 0;
                while i >= ks[k].len {
                    i -= ks[k].len;
                    k += 1;
                }
                Arc::make_mut(&mut ks[k]).set(i, v);
            }
        }
        if last {
            self.fix_last(0);
        } else {
            self.fix();
        }
    }

    /// Insert at `i`; a node grown past [`B`] splits and returns its
    /// right half.
    fn insert(&mut self, mut i: usize, v: T) -> Option<Node<T>> {
        let append = i == self.len;
        let kids = match &self.kind {
            Kind::Inner(ks) => ks.len(),
            Kind::Leaf(_) => 0,
        };
        let split = match &mut self.kind {
            Kind::Leaf(xs) => {
                xs.insert(i, v);
                (xs.len() > B).then(|| Node::leaf(xs.split_off(xs.len() / 2)))
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
                (ks.len() > B).then(|| Node::inner(ks.split_off(ks.len() / 2)))
            }
        };
        // (an append that split nothing changes only the last part)
        let same_shape = match &self.kind {
            Kind::Inner(ks) => ks.len() == kids,
            Kind::Leaf(_) => true,
        };
        if append && split.is_none() && same_shape {
            self.fix_last(1);
        } else {
            self.fix();
        }
        split
    }

    /// Remove at `i`, returning the element; empty children are dropped.
    fn remove(&mut self, mut i: usize) -> T {
        let v = match &mut self.kind {
            Kind::Leaf(xs) => xs.remove(i),
            Kind::Inner(ks) => {
                let mut k = 0;
                while i >= ks[k].len {
                    i -= ks[k].len;
                    k += 1;
                }
                let v = Arc::make_mut(&mut ks[k]).remove(i);
                if ks[k].len == 0 {
                    ks.remove(k);
                }
                v
            }
        };
        self.fix();
        v
    }
}

/// A persistent vector of values.
#[derive(Clone)]
pub struct PVec<T> {
    root: Option<Arc<Node<T>>>,
    ver: Version,
}

impl<T: Value> Default for PVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Value> PVec<T> {
    #[must_use]
    pub fn new() -> Self {
        PVec {
            root: None,
            ver: Poly::EMPTY.version(0),
        }
    }

    /// A vector of `items`, built bottom-up.
    #[must_use]
    pub fn from_vec(items: Vec<T>) -> Self {
        if items.is_empty() {
            return Self::new();
        }
        let mut level: Vec<Arc<Node<T>>> = Vec::new();
        let mut it = items.into_iter().peekable();
        while it.peek().is_some() {
            let chunk: Vec<T> = it.by_ref().take(B).collect();
            level.push(Arc::new(Node::leaf(chunk)));
        }
        while level.len() > 1 {
            let mut up = Vec::new();
            let mut it = level.into_iter().peekable();
            while it.peek().is_some() {
                let chunk: Vec<_> = it.by_ref().take(B).collect();
                up.push(Arc::new(Node::inner(chunk)));
            }
            level = up;
        }
        let mut v = PVec {
            root: level.pop(),
            ver: Version::ABSENT,
        };
        v.fix();
        v
    }

    fn fix(&mut self) {
        if self.root.as_ref().is_some_and(|r| r.len == 0) {
            self.root = None;
        }
        // An inner root with one child is that child.
        while let Some(r) = &self.root {
            match &r.kind {
                Kind::Inner(ks) if ks.len() == 1 => self.root = Some(ks[0].clone()),
                _ => break,
            }
        }
        self.ver = match &self.root {
            None => Poly::EMPTY.version(0),
            Some(r) => r.poly.version(r.len),
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

    /// The version of the whole sequence.
    #[must_use]
    pub fn version(&self) -> Version {
        self.ver
    }

    /// The polynomial hash, to combine sequences without rehashing.
    #[must_use]
    pub fn poly(&self) -> Poly {
        self.root.as_ref().map_or(Poly::EMPTY, |r| r.poly)
    }

    #[must_use]
    pub fn get(&self, i: usize) -> Option<&T> {
        (i < self.len()).then(|| self.root.as_ref().expect("non-empty").get(i))
    }

    /// Replace element `i`. Panics if out of range.
    pub fn set(&mut self, i: usize, v: T) {
        assert!(i < self.len(), "PVec::set out of range");
        Arc::make_mut(self.root.as_mut().expect("non-empty")).set(i, v);
        self.fix();
    }

    /// Insert `v` before element `i` (`i == len` appends).
    pub fn insert(&mut self, i: usize, v: T) {
        assert!(i <= self.len(), "PVec::insert out of range");
        match &mut self.root {
            None => {
                // (a leaf's room for all it will hold, before it splits)
                let mut xs = Vec::with_capacity(B + 1);
                xs.push(v);
                self.root = Some(Arc::new(Node::leaf(xs)));
            }
            Some(r) => {
                if let Some(right) = Arc::make_mut(r).insert(i, v) {
                    let left = self.root.take().expect("root");
                    self.root = Some(Arc::new(Node::inner(alloc::vec![left, Arc::new(right)])));
                }
            }
        }
        self.fix();
    }

    pub fn push(&mut self, v: T) {
        self.insert(self.len(), v);
    }

    /// Remove element `i`. Panics if out of range.
    pub fn remove(&mut self, i: usize) -> T {
        assert!(i < self.len(), "PVec::remove out of range");
        let v = Arc::make_mut(self.root.as_mut().expect("non-empty")).remove(i);
        self.fix();
        v
    }

    /// The elements in order.
    #[must_use]
    pub fn iter(&self) -> Iter<'_, T> {
        let mut it = Iter { stack: Vec::new() };
        if let Some(r) = &self.root {
            it.stack.push((r, 0));
        }
        it
    }

    #[must_use]
    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    /// The elements, moved out where no other vector shares the nodes
    /// that hold them (copied where one does).
    #[must_use]
    pub fn into_vec(self) -> Vec<T> {
        fn take<T: Clone>(n: Arc<Node<T>>, out: &mut Vec<T>) {
            match Arc::try_unwrap(n) {
                Ok(node) => match node.kind {
                    Kind::Leaf(xs) => out.extend(xs),
                    Kind::Inner(ks) => {
                        for k in ks {
                            take(k, out);
                        }
                    }
                },
                Err(shared) => walk(&shared, out),
            }
        }
        fn walk<T: Clone>(n: &Node<T>, out: &mut Vec<T>) {
            match &n.kind {
                Kind::Leaf(xs) => out.extend(xs.iter().cloned()),
                Kind::Inner(ks) => {
                    for k in ks {
                        walk(k, out);
                    }
                }
            }
        }
        let mut out = Vec::with_capacity(self.len());
        if let Some(r) = self.root {
            take(r, &mut out);
        }
        out
    }

    /// The number of nodes shared with `other` (by pointer), for tests of
    /// structural sharing.
    #[must_use]
    pub fn shared_nodes(&self, other: &PVec<T>) -> usize {
        fn walk<T>(n: &Arc<Node<T>>, out: &mut Vec<*const Node<T>>) {
            out.push(Arc::as_ptr(n));
            if let Kind::Inner(ks) = &n.kind {
                for k in ks {
                    walk(k, out);
                }
            }
        }
        let (mut a, mut b) = (Vec::new(), Vec::new());
        if let Some(r) = &self.root {
            walk(r, &mut a);
        }
        if let Some(r) = &other.root {
            walk(r, &mut b);
        }
        b.sort_unstable();
        a.iter().filter(|p| b.binary_search(p).is_ok()).count()
    }
}

impl<T: Value> PartialEq for PVec<T> {
    fn eq(&self, other: &Self) -> bool {
        self.ver == other.ver
    }
}
impl<T: Value> Eq for PVec<T> {}

impl<T: Value + fmt::Debug> fmt::Debug for PVec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Value> Value for PVec<T> {
    fn version(&self) -> Version {
        self.ver
    }
    fn field(&self, field: u32) -> Option<Self> {
        let _ = field;
        None
    }
}

impl<'a, T: Value> IntoIterator for &'a PVec<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

/// An in-order iterator over a [`PVec`].
pub struct Iter<'a, T> {
    stack: Vec<(&'a Node<T>, usize)>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let (n, i) = self.stack.last_mut()?;
            let node: &'a Node<T> = n;
            match &node.kind {
                Kind::Leaf(xs) => {
                    if *i < xs.len() {
                        *i += 1;
                        return Some(&xs[*i - 1]);
                    }
                    self.stack.pop();
                }
                Kind::Inner(ks) => {
                    if *i < ks.len() {
                        *i += 1;
                        let k: &'a Node<T> = &ks[*i - 1];
                        self.stack.push((k, 0));
                    } else {
                        self.stack.pop();
                    }
                }
            }
        }
    }
}
