//! A persistent B-tree with subtree summaries (see `docs/clean-core.md`).
//!
//! A B+ tree: entries live in the leaves, inner nodes hold separator keys
//! and their children. Every node is behind an `Arc`; a mutation copies the
//! nodes on its path that are shared (`Arc::make_mut`) and changes the rest
//! in place, so a clone taken before it never sees it.
//!
//! A parent keeps, per child, the child's entry count and summary, so rank
//! and folds read one contiguous array per level. The root's count and
//! summary are kept in the map itself.
//!
//! The measure that makes the summaries is passed to every call that needs
//! it and is not stored: a tree is only meaningful with the one measure it
//! was built with.
//!
//! Costs: a mutation recomputes the summary of every node on its path, about
//! `B` `add`s per level (and `B` `of`s at the leaf). Copying a shared inner
//! node bumps the reference count of each of its children, which dominates
//! a path copy; later writes to the same copied path are in place, so a
//! batch of writes into one new version pays the copy once per node.

use std::fmt;
use std::mem;
use std::ops::Bound;
use std::sync::Arc;

/// Most entries in a leaf and most children of an inner node.
const B: usize = 32;
/// Fewest entries or children of a node other than the root.
const MIN: usize = B / 2;
/// Capacity of a node's vectors: one over `B` while a node splits.
const CAP: usize = B + 1;
/// Most inner levels a tree can have: below the root every inner node has
/// at least `MIN` children and every leaf `MIN` entries, so 16 levels hold
/// more than `usize::MAX` entries.
const MAX_DEPTH: usize = 16;

/// How a tree summarises a subtree (e.g. a count, or a client monoid's fold).
pub(crate) trait Measure<K, V> {
    type S: Clone + Send + Sync;
    fn zero(&self) -> Self::S;
    fn of(&self, k: &K, v: &V) -> Self::S;
    /// Associative, not necessarily commutative: `add(left, right)` in key order.
    fn add(&self, a: &Self::S, b: &Self::S) -> Self::S;
}

/// The trivial measure (no summary): `S = ()`.
pub(crate) struct NoMeasure;

impl<K, V> Measure<K, V> for NoMeasure {
    type S = ();
    fn zero(&self) {}
    fn of(&self, _: &K, _: &V) {}
    fn add(&self, (): &(), (): &()) {}
}

/// Counts entries (rank queries).
pub(crate) struct Count;

impl<K, V> Measure<K, V> for Count {
    type S = usize;
    fn zero(&self) -> usize {
        0
    }
    fn of(&self, _: &K, _: &V) -> usize {
        1
    }
    fn add(&self, a: &usize, b: &usize) -> usize {
        a + b
    }
}

/// A child as its parent sees it: the node, its entry count and summary.
/// The summary is made when a fold first needs it (a write only clears it,
/// so a client monoid combines on reads, not on every contribution).
#[derive(Clone)]
struct Child<K, V, S> {
    node: Arc<Node<K, V, S>>,
    len: usize,
    sum: std::sync::OnceLock<S>,
}

enum Node<K, V, S> {
    /// Entries in key order.
    Leaf(Vec<(K, V)>),
    /// `keys[i]` separates `kids[i]` (keys below it) from `kids[i + 1]`
    /// (keys at or above it); `keys.len() + 1 == kids.len()`.
    Inner { keys: Vec<K>, kids: Vec<Child<K, V, S>> },
}

/// A copy keeps room for one more entry, so the copied path does not
/// reallocate when the mutation that copied it inserts.
impl<K: Clone, V: Clone, S: Clone> Clone for Node<K, V, S> {
    fn clone(&self) -> Self {
        match self {
            Node::Leaf(es) => {
                let mut v = Vec::with_capacity(CAP);
                v.extend_from_slice(es);
                Node::Leaf(v)
            }
            Node::Inner { keys, kids } => {
                let mut k = Vec::with_capacity(CAP);
                k.extend_from_slice(keys);
                let mut c = Vec::with_capacity(CAP);
                c.extend_from_slice(kids);
                Node::Inner { keys: k, kids: c }
            }
        }
    }
}

/// Adds `s` on the right of a running fold.
fn push<K, V, M: Measure<K, V>>(acc: &mut Option<M::S>, m: &M, s: &M::S) {
    *acc = Some(match acc.take() {
        None => s.clone(),
        Some(a) => m.add(&a, s),
    });
}

/// Adds an owned `s` on the right of a running fold.
fn push_owned<K, V, M: Measure<K, V>>(acc: &mut Option<M::S>, m: &M, s: M::S) {
    *acc = Some(match acc.take() {
        None => s,
        Some(a) => m.add(&a, &s),
    });
}

/// The index of `k` in a leaf, or where it would go.
#[inline]
fn find<K: Ord, V>(es: &[(K, V)], k: &K) -> Result<usize, usize> {
    let i = prefix(es, |e| e.0 < *k);
    if es.get(i).is_some_and(|e| e.0 == *k) {
        Ok(i)
    } else {
        Err(i)
    }
}

/// The length of the prefix of `xs` on which `f` holds (it must hold on a
/// prefix only). A linear, branch-free count: on a node of 32 keys that is
/// not in cache it beats a binary search, whose loads wait on each other.
#[inline]
fn prefix<T>(xs: &[T], f: impl Fn(&T) -> bool) -> usize {
    xs.iter().map(|x| usize::from(f(x))).sum()
}

/// Moves `v[at..]` into a new vector with node capacity.
fn split_off<T>(v: &mut Vec<T>, at: usize) -> Vec<T> {
    let mut r = Vec::with_capacity(CAP);
    r.extend(v.drain(at..));
    r
}

/// The sizes of `ceil(n / B)` chunks of `n` items, as equal as can be.
fn chunk_sizes(n: usize) -> impl Iterator<Item = usize> {
    let parts = n.div_ceil(B);
    (0..parts).map(move |i| n / parts + usize::from(i < n % parts))
}

/// The index of the first leaf entry inside a lower bound.
fn lower_index<K: Ord, V>(es: &[(K, V)], lo: Bound<&K>) -> usize {
    match lo {
        Bound::Unbounded => 0,
        Bound::Included(k) => prefix(es, |e| e.0 < *k),
        Bound::Excluded(k) => prefix(es, |e| e.0 <= *k),
    }
}

/// The index past the last leaf entry inside an upper bound.
fn upper_index<K: Ord, V>(es: &[(K, V)], hi: Bound<&K>) -> usize {
    match hi {
        Bound::Unbounded => es.len(),
        Bound::Included(k) => prefix(es, |e| e.0 <= *k),
        Bound::Excluded(k) => prefix(es, |e| e.0 < *k),
    }
}

impl<K: Ord + Clone, V: Clone, S: Clone> Node<K, V, S> {
    /// [`PMap::get_sorted`] on this subtree.
    fn get_sorted<'a>(&'a self, ks: &[K], out: &mut Vec<Option<&'a V>>) {
        match self {
            Node::Leaf(es) => {
                let mut i = 0;
                for k in ks {
                    while i < es.len() && es[i].0 < *k {
                        i += 1;
                    }
                    out.push(es.get(i).filter(|e| e.0 == *k).map(|e| &e.1));
                }
            }
            Node::Inner { keys, kids } => {
                // (Kid j holds the keys below separator j.)
                let mut a = 0;
                for (j, kid) in kids.iter().enumerate() {
                    if a == ks.len() {
                        break;
                    }
                    let b = keys.get(j).map_or(ks.len(), |s| a + ks[a..].partition_point(|k| k < s));
                    if b > a {
                        kid.node.get_sorted(&ks[a..b], out);
                    }
                    a = b;
                }
            }
        }
    }

    /// Entries in a leaf, children of an inner node.
    fn size(&self) -> usize {
        match self {
            Node::Leaf(es) => es.len(),
            Node::Inner { kids, .. } => kids.len(),
        }
    }

    /// Entries in the subtree.
    fn count(&self) -> usize {
        match self {
            Node::Leaf(es) => es.len(),
            Node::Inner { kids, .. } => kids.iter().map(|c| c.len).sum(),
        }
    }

    /// The subtree's summary, from the entries or the children's summaries.
    fn measure<M: Measure<K, V, S = S>>(&self, m: &M) -> S {
        let mut acc = None;
        match self {
            Node::Leaf(es) => {
                for (k, v) in es {
                    push_owned(&mut acc, m, m.of(k, v));
                }
            }
            Node::Inner { kids, .. } => {
                for c in kids {
                    push(&mut acc, m, c.sum(m));
                }
            }
        }
        acc.unwrap_or_else(|| m.zero())
    }

    fn first(&self) -> &(K, V) {
        let mut node = self;
        loop {
            match node {
                Node::Leaf(es) => return &es[0],
                Node::Inner { kids, .. } => node = &kids[0].node,
            }
        }
    }

    fn last(&self) -> &(K, V) {
        let mut node = self;
        loop {
            match node {
                Node::Leaf(es) => return &es[es.len() - 1],
                Node::Inner { kids, .. } => node = &kids[kids.len() - 1].node,
            }
        }
    }

    /// The greatest entry with key `< k` (`<= k` when `incl`).
    fn pred(&self, k: &K, incl: bool) -> Option<&(K, V)> {
        match self {
            Node::Leaf(es) => {
                let i = prefix(es, |e| if incl { e.0 <= *k } else { e.0 < *k });
                i.checked_sub(1).map(|i| &es[i])
            }
            Node::Inner { keys, kids } => {
                // Every child before `i` holds only keys below `k`; every
                // child after it, none.
                let i = prefix(keys, |s| if incl { s <= k } else { s < k });
                kids[i]
                    .node
                    .pred(k, incl)
                    .or_else(|| i.checked_sub(1).map(|j| kids[j].node.last()))
            }
        }
    }

    /// The least entry with key `> k` (`>= k` when `incl`).
    fn succ(&self, k: &K, incl: bool) -> Option<&(K, V)> {
        match self {
            Node::Leaf(es) => {
                let i = prefix(es, |e| if incl { e.0 < *k } else { e.0 <= *k });
                es.get(i)
            }
            Node::Inner { keys, kids } => {
                let i = prefix(keys, |s| s <= k);
                kids[i]
                    .node
                    .succ(k, incl)
                    .or_else(|| kids.get(i + 1).map(|c| c.node.first()))
            }
        }
    }
}

impl<K: Ord + Clone, V: Clone, S: Clone> Child<K, V, S> {
    fn new<M: Measure<K, V, S = S>>(node: Node<K, V, S>, _m: &M) -> Self {
        Child {
            len: node.count(),
            sum: std::sync::OnceLock::new(),
            node: Arc::new(node),
        }
    }

    /// The subtree's summary, made on first use.
    fn sum<M: Measure<K, V, S = S>>(&self, m: &M) -> &S {
        self.sum.get_or_init(|| self.node.measure(m))
    }

    /// [`PMap::visit_where`] on this subtree.
    fn visit<M: Measure<K, V, S = S>>(&self, lo: Bound<&K>, m: &M, skip: &dyn Fn(&S) -> bool, f: &mut dyn FnMut(&K, &V)) {
        if skip(self.sum(m)) {
            return;
        }
        match &*self.node {
            Node::Leaf(es) => {
                for (k, v) in &es[lower_index(es, lo)..] {
                    f(k, v);
                }
            }
            Node::Inner { keys, kids } => {
                let a = match lo {
                    Bound::Unbounded => 0,
                    Bound::Included(k) | Bound::Excluded(k) => prefix(keys, |s| s <= k),
                };
                kids[a].visit(lo, m, skip, f);
                for c in &kids[a + 1..] {
                    c.visit(Bound::Unbounded, m, skip, f);
                }
            }
        }
    }

    /// Recomputes the count and summary after the node changed.
    fn refresh<M: Measure<K, V, S = S>>(&mut self, _m: &M) {
        self.len = self.node.count();
        self.sum = std::sync::OnceLock::new();
    }

    /// Inserts or replaces; on overflow, returns the new right sibling and
    /// the key that separates it.
    fn insert<M: Measure<K, V, S = S>>(&mut self, k: K, v: V, m: &M) -> (Option<V>, Option<(K, Self)>) {
        let node = Arc::make_mut(&mut self.node);
        let (old, split) = match node {
            Node::Leaf(es) => match find(es, &k) {
                Ok(i) => (Some(mem::replace(&mut es[i].1, v)), None),
                Err(i) => {
                    es.insert(i, (k, v));
                    let split = (es.len() > B).then(|| {
                        let right = split_off(es, es.len() / 2);
                        (right[0].0.clone(), Child::new(Node::Leaf(right), m))
                    });
                    (None, split)
                }
            },
            Node::Inner { keys, kids } => {
                let i = prefix(keys, |s| *s <= k);
                let (old, split) = kids[i].insert(k, v, m);
                if let Some((sep, right)) = split {
                    keys.insert(i, sep);
                    kids.insert(i + 1, right);
                }
                let split = (kids.len() > B).then(|| {
                    let mid = kids.len() / 2;
                    let rkeys = split_off(keys, mid);
                    let sep = keys.pop().expect("inner node has keys");
                    let rkids = split_off(kids, mid);
                    (
                        sep,
                        Child::new(
                            Node::Inner {
                                keys: rkeys,
                                kids: rkids,
                            },
                            m,
                        ),
                    )
                });
                (old, split)
            }
        };
        self.len = node.count();
        self.sum = std::sync::OnceLock::new();
        (old, split)
    }

    /// Removes `k`, which is in the subtree; the node may underflow.
    fn remove<M: Measure<K, V, S = S>>(&mut self, k: &K, m: &M) -> V {
        let node = Arc::make_mut(&mut self.node);
        let old = match node {
            Node::Leaf(es) => {
                let i = find(es, k).expect("removed key is present");
                es.remove(i).1
            }
            Node::Inner { keys, kids } => {
                let i = prefix(keys, |s| s <= k);
                let old = kids[i].remove(k, m);
                if kids[i].node.size() < MIN {
                    rebalance(keys, kids, i, m);
                }
                old
            }
        };
        self.len -= 1;
        self.sum = std::sync::OnceLock::new();
        old
    }

    /// Applies `f` to the value of `k`, which is in the subtree.
    fn update(&mut self, k: &K, f: impl FnOnce(&mut V)) {
        let node = Arc::make_mut(&mut self.node);
        match node {
            Node::Leaf(es) => {
                let i = find(es, k).expect("updated key is present");
                f(&mut es[i].1);
            }
            Node::Inner { keys, kids } => {
                let i = prefix(keys, |s| s <= k);
                kids[i].update(k, f);
            }
        }
        self.sum = std::sync::OnceLock::new();
    }

    /// Folds the entries of the subtree inside the bounds onto `acc`.
    fn fold<M: Measure<K, V, S = S>>(&self, lo: Bound<&K>, hi: Bound<&K>, m: &M, acc: &mut Option<S>) {
        if matches!((lo, hi), (Bound::Unbounded, Bound::Unbounded)) {
            push(acc, m, self.sum(m));
            return;
        }
        match &*self.node {
            Node::Leaf(es) => {
                let (a, b) = (lower_index(es, lo), upper_index(es, hi));
                for (k, v) in es.get(a..b).unwrap_or(&[]) {
                    push_owned(acc, m, m.of(k, v));
                }
            }
            Node::Inner { keys, kids } => {
                let a = match lo {
                    Bound::Unbounded => 0,
                    Bound::Included(k) | Bound::Excluded(k) => prefix(keys, |s| s <= k),
                };
                let b = match hi {
                    Bound::Unbounded => kids.len() - 1,
                    Bound::Included(k) => prefix(keys, |s| s <= k),
                    Bound::Excluded(k) => prefix(keys, |s| s < k),
                };
                match a.cmp(&b) {
                    std::cmp::Ordering::Greater => {}
                    std::cmp::Ordering::Equal => kids[a].fold(lo, hi, m, acc),
                    std::cmp::Ordering::Less => {
                        kids[a].fold(lo, Bound::Unbounded, m, acc);
                        for c in &kids[a + 1..b] {
                            push(acc, m, c.sum(m));
                        }
                        kids[b].fold(Bound::Unbounded, hi, m, acc);
                    }
                }
            }
        }
    }
}

/// Mends child `i` of an inner node, which has fewer than `MIN` entries or
/// children: borrows one from a sibling that can spare it, else merges with
/// a sibling.
fn rebalance<K: Ord + Clone, V: Clone, S: Clone, M: Measure<K, V, S = S>>(
    keys: &mut Vec<K>,
    kids: &mut Vec<Child<K, V, S>>,
    i: usize,
    m: &M,
) {
    if i > 0 && kids[i - 1].node.size() > MIN {
        let (a, b) = kids.split_at_mut(i);
        shift_right(&mut keys[i - 1], &mut a[i - 1], &mut b[0], m);
    } else if i + 1 < kids.len() && kids[i + 1].node.size() > MIN {
        let (a, b) = kids.split_at_mut(i + 1);
        shift_left(&mut keys[i], &mut a[i], &mut b[0], m);
    } else {
        merge(keys, kids, if i > 0 { i - 1 } else { i }, m);
    }
}

/// Moves the last entry or child of `left` to the front of `right`.
fn shift_right<K: Ord + Clone, V: Clone, S: Clone, M: Measure<K, V, S = S>>(
    sep: &mut K,
    left: &mut Child<K, V, S>,
    right: &mut Child<K, V, S>,
    m: &M,
) {
    match (Arc::make_mut(&mut left.node), Arc::make_mut(&mut right.node)) {
        (Node::Leaf(l), Node::Leaf(r)) => {
            let e = l.pop().expect("sibling can spare an entry");
            *sep = e.0.clone();
            r.insert(0, e);
        }
        (Node::Inner { keys: lk, kids: lc }, Node::Inner { keys: rk, kids: rc }) => {
            let kid = lc.pop().expect("sibling can spare a child");
            let key = lk.pop().expect("sibling has keys");
            rk.insert(0, mem::replace(sep, key));
            rc.insert(0, kid);
        }
        _ => unreachable!("siblings are at the same depth"),
    }
    left.refresh(m);
    right.refresh(m);
}

/// Moves the first entry or child of `right` to the end of `left`.
fn shift_left<K: Ord + Clone, V: Clone, S: Clone, M: Measure<K, V, S = S>>(
    sep: &mut K,
    left: &mut Child<K, V, S>,
    right: &mut Child<K, V, S>,
    m: &M,
) {
    match (Arc::make_mut(&mut left.node), Arc::make_mut(&mut right.node)) {
        (Node::Leaf(l), Node::Leaf(r)) => {
            l.push(r.remove(0));
            *sep = r[0].0.clone();
        }
        (Node::Inner { keys: lk, kids: lc }, Node::Inner { keys: rk, kids: rc }) => {
            lc.push(rc.remove(0));
            lk.push(mem::replace(sep, rk.remove(0)));
        }
        _ => unreachable!("siblings are at the same depth"),
    }
    left.refresh(m);
    right.refresh(m);
}

/// Merges child `l + 1` into child `l`.
fn merge<K: Ord + Clone, V: Clone, S: Clone, M: Measure<K, V, S = S>>(
    keys: &mut Vec<K>,
    kids: &mut Vec<Child<K, V, S>>,
    l: usize,
    m: &M,
) {
    let right = Arc::unwrap_or_clone(kids.remove(l + 1).node);
    let sep = keys.remove(l);
    match (Arc::make_mut(&mut kids[l].node), right) {
        (Node::Leaf(a), Node::Leaf(b)) => a.extend(b),
        (Node::Inner { keys: ak, kids: ac }, Node::Inner { keys: bk, kids: bc }) => {
            ak.push(sep);
            ak.extend(bk);
            ac.extend(bc);
        }
        _ => unreachable!("siblings are at the same depth"),
    }
    kids[l].refresh(m);
}

/// An ordered persistent map. Cloning is O(1): an `Arc` clone and a clone
/// of the root's summary. Mutation is copy-on-write: only the nodes on the
/// path that are shared (Arc strong count > 1) are copied, so a clone taken
/// before a mutation never sees it.
#[derive(Clone)]
pub(crate) struct PMap<K, V, S = ()> {
    root: Option<Child<K, V, S>>,
}

impl<K, V, S> Default for PMap<K, V, S> {
    fn default() -> Self {
        PMap { root: None }
    }
}

impl<K: fmt::Debug + Ord + Clone, V: fmt::Debug + Clone, S: Clone> fmt::Debug for PMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord + Clone, V: Clone, S: Clone> PMap<K, V, S> {
    /// The greatest entry with key <= k.
    #[cfg(test)]
    pub fn at_or_below(&self, k: &K) -> Option<(&K, &V)> {
        self.root.as_ref()?.node.pred(k, true).map(|(k, v)| (k, v))
    }

    #[cfg(test)]
    pub fn first(&self) -> Option<(&K, &V)> {
        let (k, v) = self.root.as_ref()?.node.first();
        Some((k, v))
    }

    /// The whole tree's summary (O(1) once made).
    #[cfg(test)]
    pub fn summary<M: Measure<K, V, S = S>>(&self, m: &M) -> S {
        self.root.as_ref().map_or_else(|| m.zero(), |c| c.sum(m).clone())
    }

    /// Whether two maps share the same root (O(1); for "unchanged" checks:
    /// while one is kept, a change to the other copies its root).
    pub fn ptr_eq(&self, other: &Self) -> bool {
        match (&self.root, &other.root) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(&a.node, &b.node),
            _ => false,
        }
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |c| c.len)
    }

    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    pub fn get(&self, k: &K) -> Option<&V> {
        let mut node = &*self.root.as_ref()?.node;
        loop {
            match node {
                Node::Inner { keys, kids } => node = &kids[prefix(keys, |s| s <= k)].node,
                Node::Leaf(es) => return find(es, k).ok().map(|i| &es[i].1),
            }
        }
    }

    /// The values of `ks` (in ascending order, repeats allowed), in their
    /// order, to `out`: one walk down the tree for all of them, each node
    /// on the way searched once for the keys under it.
    pub fn get_sorted<'a>(&'a self, ks: &[K], out: &mut Vec<Option<&'a V>>) {
        debug_assert!(ks.windows(2).all(|w| w[0] <= w[1]), "get_sorted: keys not in order");
        match &self.root {
            Some(r) => r.node.get_sorted(ks, out),
            None => out.extend(ks.iter().map(|_| None)),
        }
    }

    pub fn contains_key(&self, k: &K) -> bool {
        self.get(k).is_some()
    }

    /// Inserts or replaces the value of `k`; returns the old value.
    pub fn insert<M: Measure<K, V, S = S>>(&mut self, k: K, v: V, m: &M) -> Option<V> {
        let Some(root) = &mut self.root else {
            let mut es = Vec::with_capacity(CAP);
            es.push((k, v));
            self.root = Some(Child::new(Node::Leaf(es), m));
            return None;
        };
        let (old, split) = root.insert(k, v, m);
        if let Some((sep, right)) = split {
            let left = self.root.take().expect("root");
            let mut keys = Vec::with_capacity(CAP);
            keys.push(sep);
            let mut kids = Vec::with_capacity(CAP);
            kids.push(left);
            kids.push(right);
            self.root = Some(Child::new(Node::Inner { keys, kids }, m));
        }
        old
    }

    /// Removes `k`; returns its value. A missing key copies nothing.
    pub fn remove<M: Measure<K, V, S = S>>(&mut self, k: &K, m: &M) -> Option<V> {
        self.get(k)?;
        let root = self.root.as_mut()?;
        let old = root.remove(k, m);
        match Arc::make_mut(&mut root.node) {
            Node::Leaf(es) if es.is_empty() => self.root = None,
            Node::Inner { kids, .. } if kids.len() == 1 => *root = kids.pop().expect("one child"),
            _ => {}
        }
        Some(old)
    }

    /// Applies `f` to the value of `k`; the summaries on its path are made
    /// again when next asked (`_m`: their measure). False (and nothing
    /// copied) when `k` is missing.
    pub fn update<M: Measure<K, V, S = S>>(&mut self, k: &K, _m: &M, f: impl FnOnce(&mut V)) -> bool {
        if self.get(k).is_none() {
            return false;
        }
        self.root.as_mut().expect("present key").update(k, f);
        true
    }

    /// Applies `f` to the value of `k` in place, a default one inserted
    /// first when missing (a shared path is copied, an owned one not).
    pub fn upsert<M: Measure<K, V, S = S>>(&mut self, k: K, m: &M, f: impl FnOnce(&mut V))
    where
        V: Default,
    {
        let mut f = Some(f);
        if !self.update(&k, m, |v| (f.take().expect("once"))(v)) {
            let mut v = V::default();
            (f.take().expect("once"))(&mut v);
            self.insert(k, v, m);
        }
    }

    /// The greatest entry with key < k.
    pub fn below(&self, k: &K) -> Option<(&K, &V)> {
        self.root.as_ref()?.node.pred(k, false).map(|(k, v)| (k, v))
    }

    /// The least entry with key > k.
    pub fn above(&self, k: &K) -> Option<(&K, &V)> {
        self.root.as_ref()?.node.succ(k, false).map(|(k, v)| (k, v))
    }

    /// The least entry with key >= k.
    pub fn at_or_above(&self, k: &K) -> Option<(&K, &V)> {
        self.root.as_ref()?.node.succ(k, true).map(|(k, v)| (k, v))
    }

    pub fn last(&self) -> Option<(&K, &V)> {
        let (k, v) = self.root.as_ref()?.node.last();
        Some((k, v))
    }

    /// In-order iteration over a range, forward and reverse. A range whose
    /// start is after its end is empty (where `BTreeMap::range` panics).
    pub fn range(&self, lo: Bound<&K>, hi: Bound<&K>) -> Iter<'_, K, V, S> {
        let a = match lo {
            Bound::Unbounded => 0,
            Bound::Included(k) => self.rank_by(k, false),
            Bound::Excluded(k) => self.rank_by(k, true),
        };
        let b = match hi {
            Bound::Unbounded => self.len(),
            Bound::Included(k) => self.rank_by(k, true),
            Bound::Excluded(k) => self.rank_by(k, false),
        };
        self.iter_ranks(a, b)
    }

    pub fn iter(&self) -> Iter<'_, K, V, S> {
        self.iter_ranks(0, self.len())
    }

    /// Iterates over the entries of rank `a..b`.
    pub fn iter_ranks(&self, a: usize, b: usize) -> Iter<'_, K, V, S> {
        let b = b.min(self.len());
        match &self.root {
            Some(root) if a < b => Iter {
                front: Cursor::at(root, a),
                back: Cursor::at(root, b - 1),
                left: b - a,
            },
            _ => Iter {
                front: Cursor::empty(),
                back: Cursor::empty(),
                left: 0,
            },
        }
    }

    /// The summary of the entries in the range, in O(log n) `add`s (whole
    /// subtrees inside the range use their stored summary).
    pub fn fold<M: Measure<K, V, S = S>>(&self, lo: Bound<&K>, hi: Bound<&K>, m: &M) -> S {
        let mut acc = None;
        if let Some(root) = &self.root {
            root.fold(lo, hi, m, &mut acc);
        }
        acc.unwrap_or_else(|| m.zero())
    }

    /// The entries from `lo` on, in key order, to `f`, but for whole
    /// subtrees whose summary `skip` rejects (a subtree's summary rejected
    /// means none of its entries is wanted): O(log n) per entry visited.
    pub fn visit_where<M: Measure<K, V, S = S>>(&self, lo: Bound<&K>, m: &M, skip: &dyn Fn(&S) -> bool, f: &mut dyn FnMut(&K, &V)) {
        if let Some(root) = &self.root {
            root.visit(lo, m, skip, f);
        }
    }

    /// Number of entries with key < k (O(log n)).
    pub fn rank(&self, k: &K) -> usize {
        self.rank_by(k, false)
    }

    /// Number of entries with key < k (<= k when `incl`).
    fn rank_by(&self, k: &K, incl: bool) -> usize {
        let Some(root) = &self.root else { return 0 };
        let mut node = &*root.node;
        let mut r = 0;
        loop {
            match node {
                Node::Inner { keys, kids } => {
                    let i = prefix(keys, |s| if incl { s <= k } else { s < k });
                    r += kids[..i].iter().map(|c| c.len).sum::<usize>();
                    node = &kids[i].node;
                }
                Node::Leaf(es) => return r + prefix(es, |e| if incl { e.0 <= *k } else { e.0 < *k }),
            }
        }
    }

    /// The i-th entry in order (O(log n)).
    pub fn nth(&self, mut i: usize) -> Option<(&K, &V)> {
        let root = self.root.as_ref()?;
        if i >= root.len {
            return None;
        }
        let mut node = &*root.node;
        loop {
            match node {
                Node::Inner { kids, .. } => {
                    let mut j = 0;
                    while i >= kids[j].len {
                        i -= kids[j].len;
                        j += 1;
                    }
                    node = &kids[j].node;
                }
                Node::Leaf(es) => return Some((&es[i].0, &es[i].1)),
            }
        }
    }

    /// Bulk build from entries sorted by key without duplicates, O(n). The
    /// nodes come out full.
    pub fn from_sorted<M: Measure<K, V, S = S>>(items: Vec<(K, V)>, m: &M) -> Self {
        debug_assert!(
            items.windows(2).all(|w| w[0].0 < w[1].0),
            "from_sorted: keys not strictly increasing"
        );
        let n = items.len();
        let mut items = items.into_iter();
        // Each node of the level with its least key.
        let mut level: Vec<(K, Child<K, V, S>)> = chunk_sizes(n)
            .map(|size| {
                let mut es = Vec::with_capacity(CAP);
                es.extend(items.by_ref().take(size));
                (es[0].0.clone(), Child::new(Node::Leaf(es), m))
            })
            .collect();
        while level.len() > 1 {
            let n = level.len();
            let mut below = level.into_iter();
            level = chunk_sizes(n)
                .map(|size| {
                    let (first, kid) = below.next().expect("chunk is not empty");
                    let mut keys = Vec::with_capacity(CAP);
                    let mut kids = Vec::with_capacity(CAP);
                    kids.push(kid);
                    for (k, c) in below.by_ref().take(size - 1) {
                        keys.push(k);
                        kids.push(c);
                    }
                    (first, Child::new(Node::Inner { keys, kids }, m))
                })
                .collect();
        }
        PMap {
            root: level.pop().map(|(_, c)| c),
        }
    }
}

/// One level of a cursor's path: the children and the index taken.
type Step<'a, K, V, S> = (&'a [Child<K, V, S>], usize);

/// A position in a tree: the path from the root and an entry of a leaf.
/// The path is a fixed array, so cursors do not allocate.
struct Cursor<'a, K, V, S> {
    path: [Step<'a, K, V, S>; MAX_DEPTH],
    depth: usize,
    leaf: &'a [(K, V)],
    pos: usize,
}

impl<'a, K, V, S> Cursor<'a, K, V, S> {
    fn empty() -> Self {
        Cursor {
            path: [(&[], 0); MAX_DEPTH],
            depth: 0,
            leaf: &[],
            pos: 0,
        }
    }

    /// The cursor on the entry of rank `r < root.len`.
    fn at(root: &'a Child<K, V, S>, mut r: usize) -> Self {
        let mut c = Self::empty();
        let mut node = &*root.node;
        loop {
            match node {
                Node::Inner { kids, .. } => {
                    let mut j = 0;
                    while r >= kids[j].len {
                        r -= kids[j].len;
                        j += 1;
                    }
                    c.path[c.depth] = (kids, j);
                    c.depth += 1;
                    node = &kids[j].node;
                }
                Node::Leaf(es) => {
                    c.leaf = es;
                    c.pos = r;
                    return c;
                }
            }
        }
    }

    fn get(&self) -> (&'a K, &'a V) {
        let (k, v) = &self.leaf[self.pos];
        (k, v)
    }

    /// Descends from `node` to its first (or last) leaf.
    fn descend(&mut self, mut node: &'a Node<K, V, S>, last: bool) {
        loop {
            match node {
                Node::Inner { kids, .. } => {
                    let j = if last { kids.len() - 1 } else { 0 };
                    self.path[self.depth] = (kids, j);
                    self.depth += 1;
                    node = &kids[j].node;
                }
                Node::Leaf(es) => {
                    self.leaf = es;
                    self.pos = if last { es.len() - 1 } else { 0 };
                    return;
                }
            }
        }
    }

    /// Moves to the next entry; past the last one it is left anywhere.
    fn advance(&mut self) {
        self.pos += 1;
        if self.pos < self.leaf.len() {
            return;
        }
        while self.depth > 0 {
            let (kids, j) = self.path[self.depth - 1];
            if j + 1 < kids.len() {
                self.path[self.depth - 1].1 = j + 1;
                self.descend(&kids[j + 1].node, false);
                return;
            }
            self.depth -= 1;
        }
    }

    /// Moves to the previous entry; before the first one it is left anywhere.
    fn retreat(&mut self) {
        if self.pos > 0 {
            self.pos -= 1;
            return;
        }
        while self.depth > 0 {
            let (kids, j) = self.path[self.depth - 1];
            if j > 0 {
                self.path[self.depth - 1].1 = j - 1;
                self.descend(&kids[j - 1].node, true);
                return;
            }
            self.depth -= 1;
        }
    }
}

/// In-order iterator over a run of entries (see [`PMap::range`]).
pub(crate) struct Iter<'a, K, V, S> {
    front: Cursor<'a, K, V, S>,
    back: Cursor<'a, K, V, S>,
    /// Entries not yet yielded from either end.
    left: usize,
}

impl<'a, K, V, S> Iterator for Iter<'a, K, V, S> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        let item = self.front.get();
        self.left -= 1;
        if self.left > 0 {
            self.front.advance();
        }
        Some(item)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.left, Some(self.left))
    }
}

impl<K, V, S> DoubleEndedIterator for Iter<'_, K, V, S> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.left == 0 {
            return None;
        }
        let item = self.back.get();
        self.left -= 1;
        if self.left > 0 {
            self.back.retreat();
        }
        Some(item)
    }
}

impl<K, V, S> ExactSizeIterator for Iter<'_, K, V, S> {}

impl<'a, K: Ord + Clone, V: Clone, S: Clone> IntoIterator for &'a PMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V, S>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::fmt::Debug;
    use std::ops::Bound::{Excluded, Included, Unbounded};
    use std::time::Instant;

    /// xorshift64*: a small deterministic generator.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    /// An order-sensitive hash of the entries: `(h, p)` with
    /// `add((h1, p1), (h2, p2)) = (h1 * p2 + h2, p1 * p2)`.
    struct Poly;

    const BASE: u64 = 0x9e37_79b9_7f4a_7c15 | 1;

    impl Measure<u32, u32> for Poly {
        type S = (u64, u64);
        fn zero(&self) -> (u64, u64) {
            (0, 1)
        }
        fn of(&self, k: &u32, v: &u32) -> (u64, u64) {
            (u64::from(*k) * 1_000_003 + u64::from(*v) + 1, BASE)
        }
        fn add(&self, a: &(u64, u64), b: &(u64, u64)) -> (u64, u64) {
            (a.0.wrapping_mul(b.1).wrapping_add(b.0), a.1.wrapping_mul(b.1))
        }
    }

    /// The first and last keys, min and sum of values: not commutative.
    struct Span;

    impl Measure<u32, u32> for Span {
        type S = Option<(u32, u32, u32, u64)>;
        fn zero(&self) -> Self::S {
            None
        }
        fn of(&self, k: &u32, v: &u32) -> Self::S {
            Some((*k, *k, *v, u64::from(*v)))
        }
        fn add(&self, a: &Self::S, b: &Self::S) -> Self::S {
            match (a, b) {
                (None, x) | (x, None) => *x,
                (Some(a), Some(b)) => Some((a.0, b.1, a.2.min(b.2), a.3 + b.3)),
            }
        }
    }

    /// Key concatenation, the plainest non-commutative measure.
    struct Concat;

    impl Measure<u32, u32> for Concat {
        type S = String;
        fn zero(&self) -> String {
            String::new()
        }
        fn of(&self, k: &u32, _: &u32) -> String {
            format!("{k},")
        }
        fn add(&self, a: &String, b: &String) -> String {
            format!("{a}{b}")
        }
    }

    /// Checks the shape, the separators, the counts and the summaries.
    fn check<K: Ord + Clone + Debug, V: Clone, S: Clone + PartialEq + Debug, M: Measure<K, V, S = S>>(
        t: &PMap<K, V, S>,
        m: &M,
    ) {
        fn walk<K: Ord + Clone + Debug, V: Clone, S: Clone + PartialEq + Debug, M: Measure<K, V, S = S>>(
            c: &Child<K, V, S>,
            lo: Option<&K>,
            hi: Option<&K>,
            root: bool,
            m: &M,
        ) -> usize {
            assert!(c.node.size() <= B, "node over B");
            assert!(root || c.node.size() >= MIN, "node under MIN");
            assert_eq!(c.len, c.node.count(), "stale count");
            if let Some(s) = c.sum.get() {
                assert_eq!(*s, c.node.measure(m), "stale summary");
            }
            match &*c.node {
                Node::Leaf(es) => {
                    assert!(!es.is_empty());
                    assert!(es.windows(2).all(|w| w[0].0 < w[1].0), "leaf out of order");
                    assert!(lo.is_none_or(|lo| es[0].0 >= *lo), "leaf below its separator");
                    assert!(hi.is_none_or(|hi| es[es.len() - 1].0 < *hi), "leaf above its separator");
                    0
                }
                Node::Inner { keys, kids } => {
                    assert!(kids.len() >= 2);
                    assert_eq!(keys.len() + 1, kids.len());
                    assert!(keys.windows(2).all(|w| w[0] < w[1]), "separators out of order");
                    let mut depth = None;
                    for (j, kid) in kids.iter().enumerate() {
                        let klo = if j == 0 { lo } else { Some(&keys[j - 1]) };
                        let khi = if j == keys.len() { hi } else { Some(&keys[j]) };
                        let d = walk(kid, klo, khi, false, m);
                        assert!(depth.is_none_or(|x| x == d), "leaves at different depths");
                        depth = Some(d);
                    }
                    depth.unwrap() + 1
                }
            }
        }
        if let Some(root) = &t.root {
            walk(root, None, None, true, m);
        }
    }

    fn model_fold<M: Measure<u32, u32>>(m: &M, it: impl Iterator<Item = (u32, u32)>) -> M::S {
        it.fold(m.zero(), |a, (k, v)| m.add(&a, &m.of(&k, &v)))
    }

    fn in_range(k: u32, lo: Bound<&u32>, hi: Bound<&u32>) -> bool {
        (match lo {
            Unbounded => true,
            Included(a) => k >= *a,
            Excluded(a) => k > *a,
        }) && (match hi {
            Unbounded => true,
            Included(b) => k <= *b,
            Excluded(b) => k < *b,
        })
    }

    fn bound<'a>(r: &mut Rng, k: &'a u32) -> Bound<&'a u32> {
        match r.below(3) {
            0 => Unbounded,
            1 => Included(k),
            _ => Excluded(k),
        }
    }

    fn pairs<'a>(it: impl Iterator<Item = (&'a u32, &'a u32)>) -> Vec<(u32, u32)> {
        it.map(|(k, v)| (*k, *v)).collect()
    }

    /// Every query of `t` against the model, at a few random keys.
    fn compare<S: Clone + PartialEq + Debug, M: Measure<u32, u32, S = S>>(
        t: &PMap<u32, u32, S>,
        b: &BTreeMap<u32, u32>,
        m: &M,
        r: &mut Rng,
        space: u32,
    ) {
        assert_eq!(t.len(), b.len());
        assert_eq!(t.is_empty(), b.is_empty());
        let kv = |e: Option<(&u32, &u32)>| e.map(|(k, v)| (*k, *v));
        assert_eq!(kv(t.first()), kv(b.first_key_value()));
        assert_eq!(kv(t.last()), kv(b.last_key_value()));
        assert_eq!(t.summary(m), model_fold(m, b.iter().map(|(k, v)| (*k, *v))));
        for _ in 0..4 {
            let k = r.below(u64::from(space) + 2) as u32;
            assert_eq!(t.get(&k), b.get(&k));
            assert_eq!(kv(t.below(&k)), kv(b.range(..k).next_back()));
            assert_eq!(kv(t.at_or_below(&k)), kv(b.range(..=k).next_back()));
            assert_eq!(kv(t.above(&k)), kv(b.range((Excluded(k), Unbounded)).next()));
            let mut ks: Vec<u32> = (0..r.below(40)).map(|_| r.below(u64::from(space) + 2) as u32).collect();
            ks.sort_unstable();
            let mut got = Vec::new();
            t.get_sorted(&ks, &mut got);
            assert_eq!(got, ks.iter().map(|k| b.get(k)).collect::<Vec<_>>());
            assert_eq!(kv(t.at_or_above(&k)), kv(b.range(k..).next()));
            assert_eq!(t.rank(&k), b.range(..k).count());
            let i = r.below(b.len() as u64 + 2) as usize;
            assert_eq!(kv(t.nth(i)), kv(b.iter().nth(i)));
            let (x, y) = (
                r.below(u64::from(space) + 2) as u32,
                r.below(u64::from(space) + 2) as u32,
            );
            let (lo, hi) = (bound(r, &x), bound(r, &y));
            let want: Vec<(u32, u32)> = b
                .iter()
                .filter(|e| in_range(*e.0, lo, hi))
                .map(|(k, v)| (*k, *v))
                .collect();
            assert_eq!(pairs(t.range(lo, hi)), want, "range {lo:?} {hi:?}");
            assert_eq!(t.range(lo, hi).len(), want.len());
            let mut rev = pairs(t.range(lo, hi).rev());
            rev.reverse();
            assert_eq!(rev, want);
            // Both ends at once.
            let mut it = t.range(lo, hi);
            let (mut front, mut back) = (Vec::new(), Vec::new());
            loop {
                let e = if r.below(2) == 0 {
                    it.next().map(|e| front.push(e))
                } else {
                    it.next_back().map(|e| back.push(e))
                };
                if e.is_none() {
                    break;
                }
            }
            back.reverse();
            front.extend(back);
            assert_eq!(pairs(front.into_iter()), want);
            assert_eq!(t.fold(lo, hi, m), model_fold(m, want.into_iter()), "fold {lo:?} {hi:?}");
        }
    }

    /// Random inserts, removes and updates against `BTreeMap`.
    fn model<S: Clone + PartialEq + Debug, M: Measure<u32, u32, S = S>>(m: &M, seed: u64, space: u32, ops: usize) {
        let mut r = Rng(seed);
        let mut t: PMap<u32, u32, S> = PMap::new();
        let mut b = BTreeMap::new();
        for op in 0..ops {
            // Phases that grow and shrink the tree, so it empties and refills.
            let grow = (op / 2000) % 2 == 0;
            let k = r.below(u64::from(space)) as u32;
            let v = r.next() as u32;
            match r.below(10) {
                0..=5 if grow => assert_eq!(t.insert(k, v, m), b.insert(k, v)),
                0..=1 => assert_eq!(t.insert(k, v, m), b.insert(k, v)),
                6..=7 => {
                    let had = b.get_mut(&k).map(|x| *x = x.wrapping_add(v)).is_some();
                    assert_eq!(t.update(&k, m, |x| *x = x.wrapping_add(v)), had);
                }
                _ => assert_eq!(t.remove(&k, m), b.remove(&k)),
            }
            if op % 7 == 0 {
                compare(&t, &b, m, &mut r, space);
            }
            if op % 97 == 0 {
                check(&t, m);
                assert_eq!(pairs(t.iter()), pairs(b.iter()));
            }
        }
        check(&t, m);
        compare(&t, &b, m, &mut r, space);
    }

    #[test]
    fn model_no_measure() {
        for seed in 1..4 {
            model(&NoMeasure, seed, 3000, 20_000);
            model(&NoMeasure, seed + 100, 60, 5_000);
        }
    }

    #[test]
    fn model_count() {
        for seed in 1..4 {
            model(&Count, seed * 7, 3000, 20_000);
            model(&Count, seed * 7 + 100, 60, 5_000);
        }
    }

    #[test]
    fn model_noncommutative() {
        for seed in 1..4 {
            model(&Poly, seed * 13, 3000, 20_000);
            model(&Span, seed * 17, 3000, 20_000);
            model(&Poly, seed * 13 + 100, 60, 5_000);
        }
        model(&Concat, 5, 400, 6_000);
    }

    #[test]
    fn edges() {
        let t: PMap<u32, u32> = PMap::new();
        assert!(t.is_empty() && t.first().is_none() && t.nth(0).is_none());
        assert_eq!(t.iter().count(), 0);
        assert!(t.ptr_eq(&PMap::new()));
        let mut t: PMap<u32, u32, usize> = PMap::new();
        for k in 0..1000 {
            t.insert(k * 2, k, &Count);
        }
        // Inverted and empty ranges are empty, not a panic.
        assert_eq!(t.range(Included(&10), Included(&4)).count(), 0);
        assert_eq!(t.range(Excluded(&10), Excluded(&10)).count(), 0);
        assert_eq!(t.range(Excluded(&10), Excluded(&12)).count(), 0);
        assert_eq!(t.fold(Included(&10), Included(&4), &Count), 0);
        assert_eq!(t.fold(Included(&10), Included(&10), &Count), 1);
        assert_eq!(t.fold(Excluded(&9), Excluded(&13), &Count), 2);
        assert_eq!(t.iter_ranks(5, 3).count(), 0);
        assert_eq!(t.iter_ranks(998, 5000).count(), 2);
        for k in 0..1000 {
            assert_eq!(t.remove(&(k * 2), &Count), Some(k));
        }
        assert!(t.is_empty());
        assert_eq!(t.summary(&Count), 0);
    }

    /// A snapshot of every query, to compare a map before and after.
    type Snapshot<S> = (Vec<(u32, u32)>, S, Vec<S>, Vec<Option<(u32, u32)>>);

    fn snapshot<S: Clone + PartialEq + Debug, M: Measure<u32, u32, S = S>>(
        t: &PMap<u32, u32, S>,
        m: &M,
    ) -> Snapshot<S> {
        let folds = (0..50)
            .map(|i| t.fold(Included(&(i * 97)), Excluded(&(i * 211)), m))
            .collect();
        let below = (0..500).map(|i| t.below(&(i * 61)).map(|(k, v)| (*k, *v))).collect();
        (pairs(t.iter()), t.summary(m), folds, below)
    }

    #[test]
    fn persistence() {
        let m = &Poly;
        let mut r = Rng(42);
        let mut a: PMap<u32, u32, (u64, u64)> = PMap::new();
        for _ in 0..20_000 {
            a.insert(r.below(30_000) as u32, r.next() as u32, m);
        }
        let before = snapshot(&a, m);
        // Mutate the clone heavily: the original does not move.
        let mut b = a.clone();
        assert!(b.ptr_eq(&a));
        for _ in 0..40_000 {
            let k = r.below(30_000) as u32;
            match r.below(3) {
                0 => {
                    b.insert(k, r.next() as u32, m);
                }
                1 => {
                    b.remove(&k, m);
                }
                _ => {
                    b.update(&k, m, |v| *v ^= 1);
                }
            }
        }
        assert!(!b.ptr_eq(&a));
        assert_eq!(snapshot(&a, m), before);
        check(&a, m);
        check(&b, m);
        // And the reverse: the clone does not see the original's changes.
        let mid = snapshot(&b, m);
        let c = b.clone();
        for k in 0..30_000 {
            if k % 3 == 0 {
                b.remove(&k, m);
            } else {
                b.insert(k, k, m);
            }
        }
        assert_eq!(snapshot(&c, m), mid);
        assert_eq!(snapshot(&a, m), before);
        check(&b, m);
        check(&c, m);
        // Missing keys copy nothing.
        let d = c.clone();
        let mut e = c.clone();
        assert_eq!(e.remove(&40_000, m), None);
        assert!(!e.update(&40_000, m, |v| *v = 0));
        assert!(e.ptr_eq(&d));
    }

    fn snapshot_count(t: &PMap<u32, u32, usize>) -> (Vec<(u32, u32)>, usize, usize) {
        (
            pairs(t.iter()),
            t.summary(&Count),
            t.fold(Included(&1000), Excluded(&9000), &Count),
        )
    }

    #[test]
    fn concurrent_readers() {
        let m = &Count;
        let mut t: PMap<u32, u32, usize> = PMap::from_sorted((0..50_000).map(|k| (k * 2, k)).collect(), m);
        let snap = Arc::new(t.clone());
        let want = snapshot_count(&snap);
        std::thread::scope(|s| {
            for i in 0..4u64 {
                let snap = Arc::clone(&snap);
                let want = want.clone();
                s.spawn(move || {
                    let mut r = Rng(i + 1);
                    for _ in 0..20 {
                        assert_eq!(snapshot_count(&snap), want);
                        for _ in 0..1000 {
                            let k = r.below(100_000) as u32;
                            assert_eq!(snap.get(&k), k.is_multiple_of(2).then_some(&(k / 2)));
                            assert_eq!(snap.rank(&k), k.div_ceil(2) as usize);
                        }
                    }
                });
            }
            let mut r = Rng(99);
            for _ in 0..200_000 {
                let k = r.below(100_000) as u32;
                if r.below(2) == 0 {
                    t.insert(k, 7, m);
                } else {
                    t.remove(&k, m);
                }
            }
        });
        assert_eq!(snapshot_count(&snap), want);
        check(&t, m);
    }

    #[test]
    fn from_sorted_equals_inserts() {
        let m = &Poly;
        for n in [
            0, 1, 2, 31, 32, 33, 63, 64, 65, 512, 1023, 1024, 1025, 1057, 10_000, 33_000,
        ] {
            let items: Vec<(u32, u32)> = (0..n).map(|k| (k * 3 + 1, k ^ 0x55)).collect();
            let a = PMap::from_sorted(items.clone(), m);
            let mut b = PMap::new();
            for (k, v) in &items {
                b.insert(*k, *v, m);
            }
            check(&a, m);
            assert_eq!(snapshot(&a, m), snapshot(&b, m));
            assert_eq!(a.len(), n as usize);
            // A bulk-built tree mutates like any other.
            let mut a = a;
            let mut bm: BTreeMap<u32, u32> = items.into_iter().collect();
            let mut r = Rng(u64::from(n) + 1);
            for _ in 0..2000 {
                let k = r.below(u64::from(n) * 3 + 10) as u32;
                if r.below(2) == 0 {
                    assert_eq!(a.insert(k, 1, m), bm.insert(k, 1));
                } else {
                    assert_eq!(a.remove(&k, m), bm.remove(&k));
                }
            }
            check(&a, m);
            compare(&a, &bm, m, &mut r, n * 3 + 10);
        }
    }

    #[test]
    fn send_sync() {
        fn is<T: Send + Sync>() {}
        is::<PMap<u64, String, usize>>();
        is::<Iter<'static, u64, String, usize>>();
    }

    /// Runs `f` on a fresh `setup()` `reps` times and prints the best time
    /// (the machine is shared); returns the last result.
    fn best<A, T>(what: &str, n: usize, reps: usize, mut setup: impl FnMut() -> A, mut f: impl FnMut(A) -> T) -> T {
        let mut min = f64::MAX;
        let mut out = None;
        for _ in 0..reps {
            let a = setup();
            let t = Instant::now();
            let o = f(a);
            min = min.min(t.elapsed().as_secs_f64());
            out = Some(o);
        }
        println!("{what:<44} {:>8.1} ms {:>8.1} ns/op", min * 1e3, min * 1e9 / n as f64);
        out.expect("reps > 0")
    }

    /// `cargo test --release --lib clean::ptree::tests::bench -- --ignored --nocapture`
    #[test]
    #[ignore = "benchmark"]
    fn bench() {
        const N: usize = 1_000_000;
        const M: usize = 1000;
        let mut r = Rng(7);
        let keys: Vec<u64> = (0..N).map(|_| r.next()).collect();
        // Half hits, half misses.
        let probes: Vec<u64> = (0..N)
            .map(|i| {
                if i % 2 == 0 {
                    keys[r.below(N as u64) as usize]
                } else {
                    r.next()
                }
            })
            .collect();
        let ranges: Vec<(u64, u64)> = (0..N)
            .map(|_| {
                let (a, b) = (r.next(), r.next());
                (a.min(b), a.max(b))
            })
            .collect();
        let none = || ();

        let p = best("PMap<Count> insert 1M random", N, 3, none, |()| {
            let mut p: PMap<u64, u64, usize> = PMap::new();
            for (i, k) in keys.iter().enumerate() {
                p.insert(*k, i as u64, &Count);
            }
            p
        });
        best("PMap<()> insert 1M random", N, 3, none, |()| {
            let mut q: PMap<u64, u64> = PMap::new();
            for (i, k) in keys.iter().enumerate() {
                q.insert(*k, i as u64, &NoMeasure);
            }
            q
        });
        let b = best("BTreeMap insert 1M random", N, 3, none, |()| {
            let mut b = BTreeMap::new();
            for (i, k) in keys.iter().enumerate() {
                b.insert(*k, i as u64);
            }
            b
        });
        let sorted: Vec<(u64, u64)> = b.iter().map(|(k, v)| (*k, *v)).collect();
        best(
            "PMap<Count> from_sorted 1M",
            N,
            3,
            || sorted.clone(),
            |s| PMap::<u64, u64, usize>::from_sorted(s, &Count),
        );

        let hits = best("PMap get 1M", N, 5, none, |()| {
            probes.iter().filter(|k| p.contains_key(k)).count()
        });
        let hits2 = best("BTreeMap get 1M", N, 5, none, |()| {
            probes.iter().filter(|k| b.contains_key(k)).count()
        });
        assert_eq!(hits, hits2);

        let s1 = best("PMap below 1M", N, 5, none, |()| {
            probes.iter().filter_map(|k| p.below(k)).fold(0u64, |a, e| a ^ e.1)
        });
        let s2 = best("BTreeMap range(..k).next_back() 1M", N, 5, none, |()| {
            probes
                .iter()
                .filter_map(|k| b.range(..*k).next_back())
                .fold(0u64, |a, e| a ^ e.1)
        });
        assert_eq!(s1, s2);

        let f1 = best("PMap<Count> fold random range 1M", N, 5, none, |()| {
            ranges
                .iter()
                .map(|(a, z)| p.fold(Included(a), Excluded(z), &Count))
                .sum::<usize>()
        });
        let f2 = best("PMap rank(hi) - rank(lo) 1M", N, 5, none, |()| {
            ranges.iter().map(|(a, z)| p.rank(z) - p.rank(a)).sum::<usize>()
        });
        assert_eq!(f1, f2);
        let f3 = best("BTreeMap range(..).count() 1k, O(range)", M, 1, none, |()| {
            ranges[..M].iter().map(|(a, z)| b.range(*a..*z).count()).sum::<usize>()
        });
        assert_eq!(
            f3,
            ranges[..M]
                .iter()
                .map(|(a, z)| p.fold(Included(a), Excluded(z), &Count))
                .sum::<usize>()
        );

        best("PMap nth 1M", N, 5, none, |()| {
            (0..N).map(|i| *p.nth((i * 7919) % N).unwrap().1).fold(0, |a, v| a ^ v)
        });
        best("PMap iter 1M", N, 5, none, |()| p.iter().fold(0, |a, e| a ^ e.1));
        best("BTreeMap iter 1M", N, 5, none, |()| b.iter().fold(0, |a, e| a ^ e.1));

        // Path copy: insert into a clone of a shared tree, then drop it.
        best("PMap<Count> clone + insert + drop 1M", N, 3, none, |()| {
            for k in &probes {
                let mut c = p.clone();
                c.insert(*k, 0, &Count);
            }
        });
        // Path copy kept: each version shares all but its path with the last.
        let versions = best("PMap<Count> insert, keep every version 100k", N / 10, 1, none, |()| {
            let mut versions = Vec::with_capacity(N / 10);
            let mut c = p.clone();
            for k in &probes[..N / 10] {
                c.insert(*k ^ 1, 0, &Count);
                versions.push(c.clone());
            }
            versions
        });
        let mut versions = Some(versions);
        best("drop those 100k versions", N / 10, 1, none, |()| drop(versions.take()));
        // In place on an unshared tree, for comparison.
        let fresh = || PMap::<u64, u64, usize>::from_sorted(sorted.clone(), &Count);
        best("PMap<Count> insert in place, unshared 1M", N, 3, fresh, |mut c| {
            for k in &probes {
                c.insert(*k ^ 1, 0, &Count);
            }
            c
        });
        best("PMap<Count> remove 1M, unshared", N, 3, fresh, |mut c| {
            for k in &keys {
                c.remove(k, &Count);
            }
            assert!(c.is_empty());
        });
        best(
            "BTreeMap remove 1M",
            N,
            3,
            || b.clone(),
            |mut c| {
                for k in &keys {
                    c.remove(k);
                }
                assert!(c.is_empty());
            },
        );
    }
}
