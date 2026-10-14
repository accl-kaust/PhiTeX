//! An arena of augmented B+trees keyed by `u64`, with cached summaries per
//! child: O(log n) insert, remove, update, range fold and monotone
//! searches, touching few cache lines per level.
//!
//! A tree's summary is a fold of its items in key order under an
//! associative `combine`, so the shape of a tree never shows in a result:
//! folds, searches with monotone predicates and lazy folds give what a
//! flat sequence of the same items would.
//!
//! A tree is named by its root (`NIL`: empty). Leaves hold up to `LCAP`
//! sorted keys with their items; inner nodes up to `ICAP` children, with
//! each child's summary and a lower bound of its keys (`keys[i]`, `i > 0`:
//! every key of child `i - 1` is below it, every key of child `i` at or
//! above it; `keys[0]` is unused).

#![allow(clippy::needless_range_loop)] // child i and its bounds go together

use crate::hash::FxHashMap;
use crate::par::{Base, Disjoint};

pub(crate) const NIL: u32 = u32::MAX;

/// Node ids with this bit are leaves.
const LEAF: u32 = 1 << 31;
/// In an overlay, node ids with this bit are the overlay's own.
const LOCAL: u32 = 1 << 30;
const LCAP: usize = 32;
const ICAP: usize = 16;

/// How items are summarised. `P` is a narrow projection of a summary,
/// kept apart (per child and per item) so that the queries needing only
/// it (`pfold`, `psearch_back`, `psearch_fwd`) read a few bytes per child.
pub(crate) trait Aug<I, S: Copy, P: Copy = ()> {
    fn empty(&self) -> S;
    fn leaf(&self, item: &I) -> S;
    fn combine(&self, a: S, b: S) -> S;
    /// `proj` of `combine` is `pcombine` of `proj`s.
    fn proj(&self, s: &S) -> P;
    fn pempty(&self) -> P;
    fn pcombine(&self, a: P, b: P) -> P;
}

#[derive(Clone, Copy)]
struct Leaf<I, S, P> {
    /// 0: on the free list.
    len: u32,
    sum: S,
    keys: [u64; LCAP],
    /// The projection of each item's summary.
    ps: [P; LCAP],
    items: [I; LCAP],
}

#[derive(Clone, Copy)]
struct Inner<S, P> {
    len: u32,
    sum: S,
    keys: [u64; ICAP],
    kids: [u32; ICAP],
    /// The projection of each child's summary.
    ps: [P; ICAP],
    sums: [S; ICAP],
}

/// An arena, or an overlay of one (see `par`): an overlay reads the
/// base's nodes until it writes one, which it then copies (`cow`: base id
/// to its own copy's id); the nodes it makes carry `LOCAL`.
pub(crate) struct Arena<I, S, P = ()> {
    leaves: Vec<Leaf<I, S, P>>,
    inners: Vec<Inner<S, P>>,
    free_leaves: Vec<u32>,
    free_inners: Vec<u32>,
    base: Base<Arena<I, S, P>>,
    cow: FxHashMap<u32, u32>,
    /// A piece (see `pieces`): the shared arena's nodes, of which it owns
    /// its subtree and its free lists.
    piece: Option<Views<I, S, P>>,
    /// An overlay or a piece: nodes are not in `leaves`/`inners` alone.
    special: bool,
}

impl<I, S, P> Default for Arena<I, S, P> {
    fn default() -> Self {
        Arena {
            leaves: Vec::new(),
            inners: Vec::new(),
            free_leaves: Vec::new(),
            free_inners: Vec::new(),
            base: Base::none(),
            cow: FxHashMap::default(),
            piece: None,
            special: false,
        }
    }
}

fn is_leaf(t: u32) -> bool {
    t & LEAF != 0
}

fn len32(n: usize) -> u32 {
    u32::try_from(n).expect("node length")
}

/// The bounds `[lo, hi)` a node's keys lie in (`hi`: `None` for no bound).
#[derive(Copy, Clone)]
struct Bounds {
    lo: u64,
    hi: Option<u64>,
}

const ALL: Bounds = Bounds { lo: 0, hi: None };

impl Bounds {
    /// Whether every key in the bounds lies in `[lo, hi)`.
    fn inside(self, lo: u64, hi: u64) -> bool {
        self.lo >= lo && self.hi.is_some_and(|h| h <= hi)
    }

    /// Whether every key in the bounds lies below `x`.
    fn below(self, x: u64) -> bool {
        self.hi.is_some_and(|h| h <= x)
    }
}

impl<I: Copy, S: Copy, P: Copy> Arena<I, S, P> {
    /// An empty overlay of `base` (itself a plain arena): the trees it
    /// names are the base's until written.
    pub(crate) fn over(base: &Arena<I, S, P>) -> Self {
        debug_assert!(base.base.is_none());
        Arena {
            base: Base::new(base),
            special: true,
            ..Arena::default()
        }
    }

    /// Where an overlay keeps node `t` (its own id), if it does.
    #[inline]
    fn local_id(&self, t: u32) -> Option<u32> {
        if t & LOCAL != 0 {
            Some(t)
        } else {
            self.cow.get(&t).copied()
        }
    }

    #[inline]
    fn leaf(&self, t: u32) -> &Leaf<I, S, P> {
        if !self.special {
            return &self.leaves[(t & !LEAF) as usize];
        }
        if let Some((l, _)) = &self.piece {
            return l.get((t & !LEAF) as usize);
        }
        if let Some(b) = self.base.get() {
            return match self.local_id(t) {
                Some(l) => &self.leaves[(l & !(LEAF | LOCAL)) as usize],
                None => b.leaf(t),
            };
        }
        &self.leaves[(t & !LEAF) as usize]
    }

    #[inline]
    fn leaf_mut(&mut self, t: u32) -> &mut Leaf<I, S, P> {
        if !self.special {
            return &mut self.leaves[(t & !LEAF) as usize];
        }
        if let Some((l, _)) = &self.piece {
            return l.get_mut((t & !LEAF) as usize);
        }
        if let Some(b) = self.base.get() {
            let l = if let Some(l) = self.local_id(t) {
                l
            } else {
                let copy = *b.leaf(t);
                self.leaves.push(copy);
                let l = u32::try_from(self.leaves.len() - 1).expect("tree overlay overflow") | LEAF | LOCAL;
                self.cow.insert(t, l);
                l
            };
            return &mut self.leaves[(l & !(LEAF | LOCAL)) as usize];
        }
        &mut self.leaves[(t & !LEAF) as usize]
    }

    #[inline]
    fn inner(&self, t: u32) -> &Inner<S, P> {
        if !self.special {
            return &self.inners[t as usize];
        }
        if let Some((_, n)) = &self.piece {
            return n.get(t as usize);
        }
        if let Some(b) = self.base.get() {
            return match self.local_id(t) {
                Some(l) => &self.inners[(l & !LOCAL) as usize],
                None => b.inner(t),
            };
        }
        &self.inners[t as usize]
    }

    #[inline]
    fn inner_mut(&mut self, t: u32) -> &mut Inner<S, P> {
        if !self.special {
            return &mut self.inners[t as usize];
        }
        if let Some((_, n)) = &self.piece {
            return n.get_mut(t as usize);
        }
        if let Some(b) = self.base.get() {
            let l = if let Some(l) = self.local_id(t) {
                l
            } else {
                let copy = *b.inner(t);
                self.inners.push(copy);
                let l = u32::try_from(self.inners.len() - 1).expect("tree overlay overflow") | LOCAL;
                self.cow.insert(t, l);
                l
            };
            return &mut self.inners[(l & !LOCAL) as usize];
        }
        &mut self.inners[t as usize]
    }

    fn new_leaf(&mut self, a: &impl Aug<I, S, P>, fill: I) -> u32 {
        let l = Leaf {
            len: 0,
            sum: a.empty(),
            keys: [0; LCAP],
            ps: [a.pempty(); LCAP],
            items: [fill; LCAP],
        };
        if let Some((v, _)) = &self.piece {
            let i = self.free_leaves.pop().expect("a tree piece's reserved leaves ran out");
            *v.get_mut(i as usize) = l;
            return i | LEAF;
        }
        let i = if let Some(i) = self.free_leaves.pop() {
            self.leaves[i as usize] = l;
            i
        } else {
            self.leaves.push(l);
            u32::try_from(self.leaves.len() - 1).expect("tree arena overflow")
        };
        assert!(i < LOCAL, "tree arena overflow");
        if self.base.is_none() {
            i | LEAF
        } else {
            i | LEAF | LOCAL
        }
    }

    fn new_inner(&mut self, a: &impl Aug<I, S, P>) -> u32 {
        let n = Inner {
            len: 0,
            sum: a.empty(),
            keys: [0; ICAP],
            kids: [NIL; ICAP],
            ps: [a.pempty(); ICAP],
            sums: [a.empty(); ICAP],
        };
        if let Some((_, v)) = &self.piece {
            let i = self
                .free_inners
                .pop()
                .expect("a tree piece's reserved inner nodes ran out");
            *v.get_mut(i as usize) = n;
            return i;
        }
        let i = if let Some(i) = self.free_inners.pop() {
            self.inners[i as usize] = n;
            i
        } else {
            self.inners.push(n);
            u32::try_from(self.inners.len() - 1).expect("tree arena overflow")
        };
        assert!(i < LOCAL, "tree arena overflow");
        if self.base.is_none() { i } else { i | LOCAL }
    }

    fn free(&mut self, t: u32) {
        if !self.base.is_none() {
            // An overlay reuses only its own nodes (a base node stays
            // the base's; the copy is dropped with the overlay).
            if t & LOCAL == 0 {
                self.cow.remove(&t);
                return;
            }
            if is_leaf(t) {
                self.leaf_mut(t).len = 0;
                self.free_leaves.push(t & !(LEAF | LOCAL));
            } else {
                self.inner_mut(t).len = 0;
                self.free_inners.push(t & !LOCAL);
            }
            return;
        }
        if is_leaf(t) {
            self.leaf_mut(t).len = 0;
            self.free_leaves.push(t & !LEAF);
        } else {
            self.inner_mut(t).len = 0;
            self.free_inners.push(t);
        }
    }

    fn len(&self, t: u32) -> usize {
        if is_leaf(t) {
            self.leaf(t).len as usize
        } else {
            self.inner(t).len as usize
        }
    }

    fn node_sum(&self, t: u32) -> S {
        if is_leaf(t) {
            self.leaf(t).sum
        } else {
            self.inner(t).sum
        }
    }

    /// Recomputes a node's own summary from its items or children, and
    /// the projections of each.
    fn resum(&mut self, a: &impl Aug<I, S, P>, t: u32) {
        if is_leaf(t) {
            let l = self.leaf_mut(t);
            let mut s = a.empty();
            for j in 0..l.len as usize {
                let x = a.leaf(&l.items[j]);
                l.ps[j] = a.proj(&x);
                s = a.combine(s, x);
            }
            l.sum = s;
        } else {
            let n = self.inner_mut(t);
            let mut s = a.empty();
            for i in 0..n.len as usize {
                n.ps[i] = a.proj(&n.sums[i]);
                s = a.combine(s, n.sums[i]);
            }
            n.sum = s;
        }
    }

    /// The child of inner `t` whose bounds hold `key`.
    fn route(&self, t: u32, key: u64) -> usize {
        let n = self.inner(t);
        n.keys[1..n.len as usize].partition_point(|&k| k <= key)
    }

    /// The bounds of child `i` of inner `t`, which has bounds `b`.
    fn child_bounds(&self, t: u32, i: usize, b: Bounds) -> Bounds {
        let n = self.inner(t);
        Bounds {
            lo: if i == 0 { b.lo } else { n.keys[i] },
            hi: if i + 1 < n.len as usize {
                Some(n.keys[i + 1])
            } else {
                b.hi
            },
        }
    }

    /// The position of `key` in leaf `t`, if there.
    fn find(&self, t: u32, key: u64) -> Option<usize> {
        let l = self.leaf(t);
        let len = l.len as usize;
        let pos = l.keys[..len].partition_point(|&k| k < key);
        (pos < len && l.keys[pos] == key).then_some(pos)
    }

    pub(crate) fn sum<A: Aug<I, S, P>>(&self, a: &A, t: u32) -> S {
        if t == NIL { a.empty() } else { self.node_sum(t) }
    }

    // ------------------------------------------------------------ insert

    pub(crate) fn insert<A: Aug<I, S, P>>(&mut self, a: &A, root: u32, key: u64, item: I) -> u32 {
        if root == NIL {
            let l = self.new_leaf(a, item);
            let n = self.leaf_mut(l);
            n.len = 1;
            n.keys[0] = key;
            n.sum = a.leaf(&item);
            n.ps[0] = a.proj(&n.sum);
            return l;
        }
        match self.ins(a, root, key, item) {
            None => root,
            Some((sep, right)) => {
                let r = self.new_inner(a);
                let (ls, rs) = (self.node_sum(root), self.node_sum(right));
                let n = self.inner_mut(r);
                n.len = 2;
                n.kids[0] = root;
                n.kids[1] = right;
                n.keys[1] = sep;
                n.sums[0] = ls;
                n.sums[1] = rs;
                n.ps[0] = a.proj(&ls);
                n.ps[1] = a.proj(&rs);
                n.sum = a.combine(ls, rs);
                r
            }
        }
    }

    /// Inserts into subtree `t`; returns the node split off its right end
    /// (with the lower bound of its keys), if it overflowed.
    #[allow(clippy::too_many_lines)]
    fn ins(&mut self, a: &impl Aug<I, S, P>, t: u32, key: u64, item: I) -> Option<(u64, u32)> {
        if is_leaf(t) {
            let len = self.leaf(t).len as usize;
            let pos = self.leaf(t).keys[..len].partition_point(|&k| k < key);
            if len < LCAP {
                let l = self.leaf_mut(t);
                l.keys.copy_within(pos..len, pos + 1);
                l.items.copy_within(pos..len, pos + 1);
                l.keys[pos] = key;
                l.items[pos] = item;
                l.len += 1;
                if pos == len {
                    let x = a.leaf(&item);
                    l.ps[pos] = a.proj(&x);
                    l.sum = a.combine(l.sum, x);
                } else {
                    self.resum(a, t);
                }
                return None;
            }
            // Full: the upper half moves to a new leaf.
            let r = self.new_leaf(a, item);
            let half = LCAP / 2;
            let (lk, li) = {
                let l = self.leaf(t);
                (l.keys, l.items)
            };
            {
                let rn = self.leaf_mut(r);
                rn.keys[..LCAP - half].copy_from_slice(&lk[half..]);
                rn.items[..LCAP - half].copy_from_slice(&li[half..]);
                rn.len = len32(LCAP - half);
            }
            self.leaf_mut(t).len = len32(half);
            let (target, tpos) = if pos <= half { (t, pos) } else { (r, pos - half) };
            {
                let l = self.leaf_mut(target);
                let len = l.len as usize;
                l.keys.copy_within(tpos..len, tpos + 1);
                l.items.copy_within(tpos..len, tpos + 1);
                l.keys[tpos] = key;
                l.items[tpos] = item;
                l.len += 1;
            }
            self.resum(a, t);
            self.resum(a, r);
            return Some((self.leaf(r).keys[0], r));
        }
        let i = self.route(t, key);
        let kid = self.inner(t).kids[i];
        let split = self.ins(a, kid, key, item);
        let ks = self.node_sum(kid);
        self.inner_mut(t).sums[i] = ks;
        let Some((sep, nk)) = split else {
            self.resum(a, t);
            return None;
        };
        let nsum = self.node_sum(nk);
        let len = self.inner(t).len as usize;
        if len < ICAP {
            let n = self.inner_mut(t);
            n.keys.copy_within(i + 1..len, i + 2);
            n.kids.copy_within(i + 1..len, i + 2);
            n.sums.copy_within(i + 1..len, i + 2);
            n.keys[i + 1] = sep;
            n.kids[i + 1] = nk;
            n.sums[i + 1] = nsum;
            n.len += 1;
            self.resum(a, t);
            return None;
        }
        // Full: insert into a scratch copy, then split it in two.
        let mut keys = [0u64; ICAP + 1];
        let mut kids = [NIL; ICAP + 1];
        let mut sums = [a.empty(); ICAP + 1];
        {
            let n = self.inner(t);
            keys[..=i].copy_from_slice(&n.keys[..=i]);
            kids[..=i].copy_from_slice(&n.kids[..=i]);
            sums[..=i].copy_from_slice(&n.sums[..=i]);
            keys[i + 2..].copy_from_slice(&n.keys[i + 1..]);
            kids[i + 2..].copy_from_slice(&n.kids[i + 1..]);
            sums[i + 2..].copy_from_slice(&n.sums[i + 1..]);
        }
        keys[i + 1] = sep;
        kids[i + 1] = nk;
        sums[i + 1] = nsum;
        let half = ICAP.div_ceil(2);
        let r = self.new_inner(a);
        {
            let n = self.inner_mut(t);
            n.keys[..half].copy_from_slice(&keys[..half]);
            n.kids[..half].copy_from_slice(&kids[..half]);
            n.sums[..half].copy_from_slice(&sums[..half]);
            n.len = len32(half);
        }
        {
            let rn = self.inner_mut(r);
            let m = ICAP + 1 - half;
            rn.keys[..m].copy_from_slice(&keys[half..]);
            rn.kids[..m].copy_from_slice(&kids[half..]);
            rn.sums[..m].copy_from_slice(&sums[half..]);
            rn.len = len32(m);
        }
        self.resum(a, t);
        self.resum(a, r);
        Some((keys[half], r))
    }

    /// Inserts a run of new keys (`items` sorted by key, none of them
    /// present).
    pub(crate) fn insert_run<A: Aug<I, S, P>>(&mut self, a: &A, root: u32, items: &[(u64, I)]) -> u32 {
        let (Some(&(first, _)), Some(&(last, _))) = (items.first(), items.last()) else {
            return root;
        };
        if items.len() >= 2 * LCAP && !self.special && self.next(root, first).is_none_or(|(k, _)| k > last) {
            // One gap of the tree holds the whole run (a merge replays runs
            // whose keys may straddle others'): built as a tree of its own
            // and joined in, with no path re-summed per item.
            let (l, r) = self.split(a, root, first);
            let m = self.build(a, items);
            let lm = self.join(a, l, m);
            return self.join(a, lm, r);
        }
        let mut root = root;
        for &(k, i) in items {
            root = self.insert(a, root, k, i);
        }
        root
    }

    // ------------------------------------------------------------ remove

    pub(crate) fn remove<A: Aug<I, S, P>>(&mut self, a: &A, root: u32, key: u64) -> (u32, Option<I>) {
        if root == NIL {
            return (NIL, None);
        }
        let item = self.rem(a, root, key);
        if item.is_none() {
            return (root, None);
        }
        let mut root = root;
        loop {
            if self.len(root) == 0 {
                self.free(root);
                return (NIL, item);
            }
            if !is_leaf(root) && self.len(root) == 1 {
                let k = self.inner(root).kids[0];
                self.free(root);
                root = k;
                continue;
            }
            return (root, item);
        }
    }

    fn rem(&mut self, a: &impl Aug<I, S, P>, t: u32, key: u64) -> Option<I> {
        if is_leaf(t) {
            let pos = self.find(t, key)?;
            let l = self.leaf_mut(t);
            let len = l.len as usize;
            let item = l.items[pos];
            l.keys.copy_within(pos + 1..len, pos);
            l.items.copy_within(pos + 1..len, pos);
            l.len -= 1;
            self.resum(a, t);
            return Some(item);
        }
        let i = self.route(t, key);
        let kid = self.inner(t).kids[i];
        let item = self.rem(a, kid, key)?;
        let klen = self.len(kid);
        if klen == 0 {
            self.free(kid);
            self.drop_child(t, i);
        } else {
            let ks = self.node_sum(kid);
            self.inner_mut(t).sums[i] = ks;
            let cap = if is_leaf(kid) { LCAP } else { ICAP };
            if klen < cap / 4 {
                self.merge_near(a, t, i);
            }
        }
        self.resum(a, t);
        Some(item)
    }

    /// Removes child `i` of inner `t` (already freed or merged away).
    fn drop_child(&mut self, t: u32, i: usize) {
        let n = self.inner_mut(t);
        let len = n.len as usize;
        n.keys.copy_within(i + 1..len, i);
        n.kids.copy_within(i + 1..len, i);
        n.sums.copy_within(i + 1..len, i);
        n.len -= 1;
    }

    /// Merges underfull child `i` of `t` into a neighbour if both fit in
    /// one node.
    fn merge_near(&mut self, a: &impl Aug<I, S, P>, t: u32, i: usize) {
        let len = self.inner(t).len as usize;
        let j = if i + 1 < len {
            i
        } else if i > 0 {
            i - 1
        } else {
            return;
        };
        let (l, r) = (self.inner(t).kids[j], self.inner(t).kids[j + 1]);
        let (ll, rl) = (self.len(l), self.len(r));
        if is_leaf(l) {
            if ll + rl > LCAP {
                return;
            }
            let (rk, ri) = {
                let rn = self.leaf(r);
                (rn.keys, rn.items)
            };
            let ln = self.leaf_mut(l);
            ln.keys[ll..ll + rl].copy_from_slice(&rk[..rl]);
            ln.items[ll..ll + rl].copy_from_slice(&ri[..rl]);
            ln.len += len32(rl);
        } else {
            if ll + rl > ICAP {
                return;
            }
            let sep = self.inner(t).keys[j + 1];
            let (rk, rkids, rs) = {
                let rn = self.inner(r);
                (rn.keys, rn.kids, rn.sums)
            };
            let ln = self.inner_mut(l);
            ln.keys[ll..ll + rl].copy_from_slice(&rk[..rl]);
            ln.keys[ll] = sep;
            ln.kids[ll..ll + rl].copy_from_slice(&rkids[..rl]);
            ln.sums[ll..ll + rl].copy_from_slice(&rs[..rl]);
            ln.len += len32(rl);
        }
        self.resum(a, l);
        let ls = self.node_sum(l);
        self.free(r);
        self.drop_child(t, j + 1);
        self.inner_mut(t).sums[j] = ls;
    }

    // ------------------------------------------------------------ update

    /// Applies `f` to the item at `key` and re-summarises the path.
    pub(crate) fn update<A: Aug<I, S, P>>(&mut self, a: &A, t: u32, key: u64, f: &mut dyn FnMut(&mut I)) -> bool {
        if t == NIL {
            return false;
        }
        if is_leaf(t) {
            let Some(pos) = self.find(t, key) else {
                return false;
            };
            f(&mut self.leaf_mut(t).items[pos]);
            self.resum(a, t);
            return true;
        }
        let i = self.route(t, key);
        let kid = self.inner(t).kids[i];
        if !self.update(a, kid, key, f) {
            return false;
        }
        let ks = self.node_sum(kid);
        self.inner_mut(t).sums[i] = ks;
        self.resum(a, t);
        true
    }

    /// Sets the items of existing keys (`items` sorted by key) and
    /// re-summarises each touched node once.
    pub(crate) fn set_many<A: Aug<I, S, P>>(&mut self, a: &A, t: u32, items: &[(u64, I)]) {
        if t == NIL || items.is_empty() {
            return;
        }
        if is_leaf(t) {
            let mut any = false;
            for &(k, it) in items {
                if let Some(pos) = self.find(t, k) {
                    self.leaf_mut(t).items[pos] = it;
                    any = true;
                }
            }
            if any {
                self.resum(a, t);
            }
            return;
        }
        let len = self.inner(t).len as usize;
        let mut rest = items;
        for i in 0..len {
            if rest.is_empty() {
                break;
            }
            let take = if i + 1 < len {
                let hi = self.inner(t).keys[i + 1];
                rest.partition_point(|x| x.0 < hi)
            } else {
                rest.len()
            };
            if take > 0 {
                let kid = self.inner(t).kids[i];
                self.set_many(a, kid, &rest[..take]);
                let ks = self.node_sum(kid);
                self.inner_mut(t).sums[i] = ks;
                rest = &rest[take..];
            }
        }
        self.resum(a, t);
    }

    // ------------------------------------------------------------ queries

    pub(crate) fn get(&self, t: u32, key: u64) -> Option<&I> {
        if t == NIL {
            return None;
        }
        let mut t = t;
        while !is_leaf(t) {
            t = self.inner(t).kids[self.route(t, key)];
        }
        self.find(t, key).map(|pos| &self.leaf(t).items[pos])
    }

    /// Summary of the keys in `[lo, hi)`.
    pub(crate) fn fold<A: Aug<I, S, P>>(&self, a: &A, t: u32, lo: u64, hi: u64) -> S {
        if t == NIL || lo >= hi {
            return a.empty();
        }
        self.fold_in(a, t, lo, hi, ALL)
    }

    fn fold_in(&self, a: &impl Aug<I, S, P>, t: u32, lo: u64, hi: u64, b: Bounds) -> S {
        if b.inside(lo, hi) {
            return self.node_sum(t);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k < lo);
            let to = l.keys[..len].partition_point(|&k| k < hi);
            let mut s = a.empty();
            for it in &l.items[from..to.max(from)] {
                s = a.combine(s, a.leaf(it));
            }
            return s;
        }
        let n = self.inner(t);
        let mut s = a.empty();
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= hi {
                break;
            }
            if cb.below(lo) {
                continue;
            }
            let cs = if cb.inside(lo, hi) {
                n.sums[i]
            } else {
                self.fold_in(a, n.kids[i], lo, hi, cb)
            };
            s = a.combine(s, cs);
        }
        s
    }

    /// The projection of a node's summary.
    fn node_p(&self, a: &impl Aug<I, S, P>, t: u32) -> P {
        a.proj(&self.node_sum(t))
    }

    /// `proj(fold(lo, hi))`, reading only projections.
    pub(crate) fn pfold<A: Aug<I, S, P>>(&self, a: &A, t: u32, lo: u64, hi: u64) -> P {
        if t == NIL || lo >= hi {
            return a.pempty();
        }
        self.pfold_in(a, t, lo, hi, ALL)
    }

    fn pfold_in(&self, a: &impl Aug<I, S, P>, t: u32, lo: u64, hi: u64, b: Bounds) -> P {
        if b.inside(lo, hi) {
            return self.node_p(a, t);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k < lo);
            let to = l.keys[..len].partition_point(|&k| k < hi);
            let mut s = a.pempty();
            for &p in &l.ps[from..to.max(from)] {
                s = a.pcombine(s, p);
            }
            return s;
        }
        let n = self.inner(t);
        let mut s = a.pempty();
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= hi {
                break;
            }
            if cb.below(lo) {
                continue;
            }
            let cs = if cb.inside(lo, hi) {
                n.ps[i]
            } else {
                self.pfold_in(a, n.kids[i], lo, hi, cb)
            };
            s = a.pcombine(s, cs);
        }
        s
    }

    /// [`Self::search_back`] with a predicate on projections.
    pub(crate) fn psearch_back<A: Aug<I, S, P>>(
        &self,
        a: &A,
        t: u32,
        before: u64,
        pred: &dyn Fn(P) -> bool,
    ) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let mut acc = a.pempty();
        self.pback(a, t, before, ALL, &mut acc, pred)
    }

    fn pback(
        &self,
        a: &impl Aug<I, S, P>,
        t: u32,
        before: u64,
        b: Bounds,
        acc: &mut P,
        pred: &dyn Fn(P) -> bool,
    ) -> Option<u64> {
        if b.below(before) {
            return self.pback_all(a, t, acc, pred);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let to = l.keys[..l.len as usize].partition_point(|&k| k < before);
            for j in (0..to).rev() {
                *acc = a.pcombine(l.ps[j], *acc);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in (0..n.len as usize).rev() {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= before {
                continue;
            }
            if let Some(k) = self.pback(a, n.kids[i], before, cb, acc, pred) {
                return Some(k);
            }
        }
        None
    }

    fn pback_all(&self, a: &impl Aug<I, S, P>, t: u32, acc: &mut P, pred: &dyn Fn(P) -> bool) -> Option<u64> {
        let whole = a.pcombine(self.node_p(a, t), *acc);
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            for j in (0..l.len as usize).rev() {
                *acc = a.pcombine(l.ps[j], *acc);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in (0..n.len as usize).rev() {
            let whole = a.pcombine(n.ps[i], *acc);
            if !pred(whole) {
                *acc = whole;
                continue;
            }
            if let Some(k) = self.pback_all(a, n.kids[i], acc, pred) {
                return Some(k);
            }
        }
        None
    }

    /// [`Self::search_fwd`] with a predicate on projections.
    pub(crate) fn psearch_fwd<A: Aug<I, S, P>>(
        &self,
        a: &A,
        t: u32,
        after: u64,
        pred: &dyn Fn(P) -> bool,
    ) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let mut acc = a.pempty();
        self.pfwd(a, t, after, ALL, &mut acc, pred)
    }

    fn pfwd(
        &self,
        a: &impl Aug<I, S, P>,
        t: u32,
        after: u64,
        b: Bounds,
        acc: &mut P,
        pred: &dyn Fn(P) -> bool,
    ) -> Option<u64> {
        if b.lo > after {
            return self.pfwd_all(a, t, acc, pred);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k <= after);
            for j in from..len {
                *acc = a.pcombine(*acc, l.ps[j]);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            if cb.below(after.saturating_add(1)) {
                continue;
            }
            if let Some(k) = self.pfwd(a, n.kids[i], after, cb, acc, pred) {
                return Some(k);
            }
        }
        None
    }

    fn pfwd_all(&self, a: &impl Aug<I, S, P>, t: u32, acc: &mut P, pred: &dyn Fn(P) -> bool) -> Option<u64> {
        let whole = a.pcombine(*acc, self.node_p(a, t));
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            for j in 0..l.len as usize {
                *acc = a.pcombine(*acc, l.ps[j]);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in 0..n.len as usize {
            let whole = a.pcombine(*acc, n.ps[i]);
            if !pred(whole) {
                *acc = whole;
                continue;
            }
            if let Some(k) = self.pfwd_all(a, n.kids[i], acc, pred) {
                return Some(k);
            }
        }
        None
    }

    /// The nearest key `k < before` such that `pred(fold[k, before))` holds,
    /// for a predicate monotone in the range growing to the left.
    pub(crate) fn search_back<A: Aug<I, S, P>>(
        &self,
        a: &A,
        t: u32,
        before: u64,
        pred: &dyn Fn(S) -> bool,
    ) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let mut acc = a.empty();
        self.back(a, t, before, ALL, &mut acc, pred)
    }

    fn back(
        &self,
        a: &impl Aug<I, S, P>,
        t: u32,
        before: u64,
        b: Bounds,
        acc: &mut S,
        pred: &dyn Fn(S) -> bool,
    ) -> Option<u64> {
        if b.below(before) {
            return self.back_all(a, t, acc, pred);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let to = l.keys[..l.len as usize].partition_point(|&k| k < before);
            for j in (0..to).rev() {
                *acc = a.combine(a.leaf(&l.items[j]), *acc);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in (0..n.len as usize).rev() {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= before {
                continue;
            }
            if let Some(k) = self.back(a, n.kids[i], before, cb, acc, pred) {
                return Some(k);
            }
        }
        None
    }

    fn back_all(&self, a: &impl Aug<I, S, P>, t: u32, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        let whole = a.combine(self.node_sum(t), *acc);
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            for j in (0..l.len as usize).rev() {
                *acc = a.combine(a.leaf(&l.items[j]), *acc);
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in (0..n.len as usize).rev() {
            let whole = a.combine(n.sums[i], *acc);
            if !pred(whole) {
                *acc = whole;
                continue;
            }
            if let Some(k) = self.back_all(a, n.kids[i], acc, pred) {
                return Some(k);
            }
        }
        None
    }

    /// The nearest key `k > after` such that `pred(fold(after, k]))` holds,
    /// for a predicate monotone in the range growing to the right.
    #[allow(dead_code)] // the full-summary twin of `psearch_fwd`, checked against it
    pub(crate) fn search_fwd<A: Aug<I, S, P>>(
        &self,
        a: &A,
        t: u32,
        after: u64,
        pred: &dyn Fn(S) -> bool,
    ) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let mut acc = a.empty();
        self.fwd(a, t, after, ALL, &mut acc, pred)
    }

    #[allow(dead_code)] // the full-summary twin of `psearch_fwd`, checked against it
    fn fwd(
        &self,
        a: &impl Aug<I, S, P>,
        t: u32,
        after: u64,
        b: Bounds,
        acc: &mut S,
        pred: &dyn Fn(S) -> bool,
    ) -> Option<u64> {
        if b.lo > after {
            return self.fwd_all(a, t, acc, pred);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k <= after);
            for j in from..len {
                *acc = a.combine(*acc, a.leaf(&l.items[j]));
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            // Every key of the child at or below `after`.
            if cb.below(after.saturating_add(1)) {
                continue;
            }
            if let Some(k) = self.fwd(a, n.kids[i], after, cb, acc, pred) {
                return Some(k);
            }
        }
        None
    }

    #[allow(dead_code)] // the full-summary twin of `psearch_fwd`, checked against it
    fn fwd_all(&self, a: &impl Aug<I, S, P>, t: u32, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        let whole = a.combine(*acc, self.node_sum(t));
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            for j in 0..l.len as usize {
                *acc = a.combine(*acc, a.leaf(&l.items[j]));
                if pred(*acc) {
                    return Some(l.keys[j]);
                }
            }
            return None;
        }
        let n = self.inner(t);
        for i in 0..n.len as usize {
            let whole = a.combine(*acc, n.sums[i]);
            if !pred(whole) {
                *acc = whole;
                continue;
            }
            if let Some(k) = self.fwd_all(a, n.kids[i], acc, pred) {
                return Some(k);
            }
        }
        None
    }

    /// Visits the items with keys in `[lo, hi)` in key order, each with the
    /// summary of everything before it (from the start of the tree), leaving
    /// out the subtrees `skip(before, sum)` rejects (their summaries still
    /// count as passed).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn visit<A: Aug<I, S, P>>(
        &self,
        a: &A,
        t: u32,
        lo: u64,
        hi: u64,
        before: &mut S,
        skip: &dyn Fn(S, S) -> bool,
        f: &mut dyn FnMut(u64, &I, S),
    ) {
        if t == NIL || lo >= hi {
            return;
        }
        self.visit_in(a, t, lo, hi, ALL, before, skip, f);
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_in(
        &self,
        a: &impl Aug<I, S, P>,
        t: u32,
        lo: u64,
        hi: u64,
        b: Bounds,
        before: &mut S,
        skip: &dyn Fn(S, S) -> bool,
        f: &mut dyn FnMut(u64, &I, S),
    ) {
        if is_leaf(t) {
            let l = self.leaf(t);
            for j in 0..l.len as usize {
                let k = l.keys[j];
                if k >= hi {
                    return;
                }
                if k >= lo {
                    f(k, &l.items[j], *before);
                }
                *before = a.combine(*before, a.leaf(&l.items[j]));
            }
            return;
        }
        let n = self.inner(t);
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= hi {
                return;
            }
            if cb.below(lo) || skip(*before, n.sums[i]) {
                *before = a.combine(*before, n.sums[i]);
                continue;
            }
            self.visit_in(a, n.kids[i], lo, hi, cb, before, skip, f);
        }
    }

    /// The first key `> key`.
    pub(crate) fn next(&self, t: u32, key: u64) -> Option<(u64, &I)> {
        if t == NIL {
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let pos = l.keys[..len].partition_point(|&k| k <= key);
            return (pos < len).then(|| (l.keys[pos], &l.items[pos]));
        }
        let n = self.inner(t);
        (self.route(t, key)..n.len as usize).find_map(|i| self.next(n.kids[i], key))
    }

    /// The last key `< key`.
    pub(crate) fn prev(&self, t: u32, key: u64) -> Option<(u64, &I)> {
        if t == NIL {
            return None;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let pos = l.keys[..l.len as usize].partition_point(|&k| k < key);
            return (pos > 0).then(|| (l.keys[pos - 1], &l.items[pos - 1]));
        }
        let n = self.inner(t);
        (0..=self.route(t, key)).rev().find_map(|i| self.prev(n.kids[i], key))
    }

    /// The last key overall.
    pub(crate) fn last(&self, t: u32) -> Option<(u64, &I)> {
        self.prev(t, u64::MAX)
    }

    /// Up to `limit` `(key, item)` pairs with keys in `[lo, hi]`, in order.
    pub(crate) fn range<O>(&self, t: u32, lo: u64, hi: u64, out: &mut Vec<(u64, O)>, limit: usize)
    where
        I: Into<O>,
    {
        if t == NIL || out.len() >= limit {
            return;
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k < lo);
            for j in from..len {
                if l.keys[j] > hi || out.len() >= limit {
                    return;
                }
                out.push((l.keys[j], l.items[j].into()));
            }
            return;
        }
        let n = self.inner(t);
        for i in self.route(t, lo)..n.len as usize {
            if i > 0 && n.keys[i] > hi {
                return;
            }
            self.range(n.kids[i], lo, hi, out, limit);
            if out.len() >= limit {
                return;
            }
        }
    }
}

/// A second fold over a tree, into values that are expensive to build:
/// whole subtrees go through `whole` (e.g. a cache keyed by their summary).
pub(crate) trait LazyFold<I, S, R: Copy> {
    fn empty(&self) -> R;
    fn leaf(&self, item: &I) -> R;
    fn combine(&self, a: R, b: R) -> R;
    /// The value of a whole subtree with summary `sum`, computed by `build`
    /// if not known.
    fn whole(&self, sum: &S, build: &mut dyn FnMut() -> R) -> R;
}

impl<I: Copy, S: Copy, P: Copy> Arena<I, S, P> {
    /// The lazy fold of the keys in `[lo, hi)`.
    pub(crate) fn lazy_fold<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32, lo: u64, hi: u64) -> R {
        if t == NIL || lo >= hi {
            return f.empty();
        }
        self.lazy_in(f, t, lo, hi, ALL)
    }

    fn lazy_in<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32, lo: u64, hi: u64, b: Bounds) -> R {
        if b.inside(lo, hi) {
            return self.lazy_whole(f, t);
        }
        if is_leaf(t) {
            let l = self.leaf(t);
            let len = l.len as usize;
            let from = l.keys[..len].partition_point(|&k| k < lo);
            let to = l.keys[..len].partition_point(|&k| k < hi);
            let mut r = f.empty();
            for it in &l.items[from..to.max(from)] {
                r = f.combine(r, f.leaf(it));
            }
            return r;
        }
        let n = self.inner(t);
        let mut r = f.empty();
        for i in 0..n.len as usize {
            let cb = self.child_bounds(t, i, b);
            if cb.lo >= hi {
                break;
            }
            if cb.below(lo) {
                continue;
            }
            r = f.combine(r, self.lazy_in(f, n.kids[i], lo, hi, cb));
        }
        r
    }

    pub(crate) fn lazy_whole<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32) -> R {
        if t == NIL {
            return f.empty();
        }
        f.whole(&self.node_sum(t), &mut || {
            let mut r = f.empty();
            if is_leaf(t) {
                let l = self.leaf(t);
                for it in &l.items[..l.len as usize] {
                    r = f.combine(r, f.leaf(it));
                }
            } else {
                let n = self.inner(t);
                for &k in &n.kids[..n.len as usize] {
                    r = f.combine(r, self.lazy_whole(f, k));
                }
            }
            r
        })
    }

    /// Visits every item held (also in trees no longer named) with the
    /// summary of its leaf.
    pub(crate) fn for_each(&self, f: &mut dyn FnMut(&I, &S)) {
        debug_assert!(self.base.is_none());
        for l in &self.leaves {
            for it in &l.items[..l.len as usize] {
                f(it, &l.sum);
            }
        }
    }
}

// ------------------------------------------------------------ split / join
//
// The tree is relaxed (no minimum occupancy beyond one entry), so a split
// is one descent that cuts every node on the path in two, and a join is
// one descent down the taller tree's spine. Every node on the path is
// re-summarised: O(height * fanout) combines.

impl<I: Copy, S: Copy, P: Copy> Arena<I, S, P> {
    /// Inner levels above the leaves.
    pub(crate) fn height(&self, t: u32) -> u32 {
        let mut h = 0;
        let mut t = t;
        while !is_leaf(t) {
            t = self.inner(t).kids[0];
            h += 1;
        }
        h
    }

    fn min_key(&self, t: u32) -> u64 {
        let mut t = t;
        while !is_leaf(t) {
            t = self.inner(t).kids[0];
        }
        self.leaf(t).keys[0]
    }

    /// Splits `root` into the keys below `key` and those at or above it.
    pub(crate) fn split<A: Aug<I, S, P>>(&mut self, a: &A, root: u32, key: u64) -> (u32, u32) {
        if root == NIL {
            return (NIL, NIL);
        }
        let (l, r) = self.split_in(a, root, key);
        (self.trim(l), self.trim(r))
    }

    /// Drops single-child roots.
    fn trim(&mut self, t: u32) -> u32 {
        let mut t = t;
        while t != NIL && !is_leaf(t) && self.inner(t).len == 1 {
            let k = self.inner(t).kids[0];
            self.free(t);
            t = k;
        }
        t
    }

    /// Both halves keep `t`'s height (NIL when empty).
    fn split_in(&mut self, a: &impl Aug<I, S, P>, t: u32, key: u64) -> (u32, u32) {
        if is_leaf(t) {
            let len = self.leaf(t).len as usize;
            let pos = self.leaf(t).keys[..len].partition_point(|&k| k < key);
            if pos == len {
                return (t, NIL);
            }
            if pos == 0 {
                return (NIL, t);
            }
            let fill = self.leaf(t).items[0];
            let r = self.new_leaf(a, fill);
            let (lk, li) = {
                let l = self.leaf(t);
                (l.keys, l.items)
            };
            let rn = self.leaf_mut(r);
            rn.keys[..len - pos].copy_from_slice(&lk[pos..len]);
            rn.items[..len - pos].copy_from_slice(&li[pos..len]);
            rn.len = len32(len - pos);
            self.leaf_mut(t).len = len32(pos);
            self.resum(a, t);
            self.resum(a, r);
            return (t, r);
        }
        let i = self.route(t, key);
        let len = self.inner(t).len as usize;
        let kid = self.inner(t).kids[i];
        let (cl, cr) = self.split_in(a, kid, key);
        // Right: cr then kids[i+1..].
        let n = self.inner(t);
        let (keys, kids, sums) = (n.keys, n.kids, n.sums);
        let mut rk = [0u64; ICAP];
        let mut rkids = [NIL; ICAP];
        let mut rs = [a.empty(); ICAP];
        let mut m = 0;
        if cr != NIL {
            rkids[0] = cr;
            rs[0] = self.node_sum(cr);
            m = 1;
        }
        for j in i + 1..len {
            rk[m] = keys[j];
            rkids[m] = kids[j];
            rs[m] = sums[j];
            m += 1;
        }
        let right = if m == 0 {
            NIL
        } else {
            let r = self.new_inner(a);
            let rn = self.inner_mut(r);
            rn.keys = rk;
            rn.kids = rkids;
            rn.sums = rs;
            rn.len = len32(m);
            self.resum(a, r);
            r
        };
        // Left: kids[..i] then cl, in place.
        let mut ll = i;
        if cl != NIL {
            let s = self.node_sum(cl);
            let n = self.inner_mut(t);
            n.kids[i] = cl;
            n.sums[i] = s;
            ll += 1;
        }
        let left = if ll == 0 {
            self.free(t);
            NIL
        } else {
            self.inner_mut(t).len = len32(ll);
            self.resum(a, t);
            t
        };
        (left, right)
    }

    /// Joins two trees, every key of `l` below every key of `r`. The seam
    /// (the right spine of `l` against the left spine of `r`, down to the
    /// lower tree's height) is zipped level by level and the entries on it
    /// redistributed, so the thin nodes a split leaves on its path are
    /// absorbed by the join that undoes it.
    pub(crate) fn join<A: Aug<I, S, P>>(&mut self, a: &A, l: u32, r: u32) -> u32 {
        if l == NIL {
            return r;
        }
        if r == NIL {
            return l;
        }
        let (hl, hr) = (self.height(l), self.height(r));
        let mut list = self.join_at(a, l, hl, r, hr);
        // Grow roots until one node holds the list.
        while list.len() > 1 {
            let (lo, kids) = (list[0].0, list);
            list = self.pack(a, lo, &kids);
        }
        list[0].1
    }

    /// Joins `l` (height `hl`) and `r` (height `hr`): the nodes, all of
    /// height `max(hl, hr)`, with their least keys.
    fn join_at(&mut self, a: &impl Aug<I, S, P>, l: u32, hl: u32, r: u32, hr: u32) -> Vec<(u64, u32)> {
        if hl == hr {
            return self.join_eq(a, l, r, hl);
        }
        if hl > hr {
            let len = self.inner(l).len as usize;
            let last = self.inner(l).kids[len - 1];
            let sub = self.join_at(a, last, hl - 1, r, hr);
            let mut entries = self.entries(l);
            entries.pop();
            entries.extend(sub);
            let lo = entries[0].0;
            self.free(l);
            self.pack(a, lo, &entries)
        } else {
            let first = self.inner(r).kids[0];
            let sub = self.join_at(a, l, hl, first, hr - 1);
            let mut entries = sub;
            let rest = self.entries(r);
            entries.extend_from_slice(&rest[1..]);
            let lo = entries[0].0;
            self.free(r);
            self.pack(a, lo, &entries)
        }
    }

    /// The children of inner `t` with their least keys (`keys[0]` is
    /// replaced by the true least key).
    fn entries(&self, t: u32) -> Vec<(u64, u32)> {
        let n = self.inner(t);
        let mut v: Vec<(u64, u32)> = (0..n.len as usize).map(|i| (n.keys[i], n.kids[i])).collect();
        v[0].0 = self.min_key(n.kids[0]);
        v
    }

    fn join_eq(&mut self, a: &impl Aug<I, S, P>, l: u32, r: u32, h: u32) -> Vec<(u64, u32)> {
        if h == 0 {
            let (ll, rl) = (self.len(l), self.len(r));
            let mut keys = Vec::with_capacity(ll + rl);
            let mut items = Vec::with_capacity(ll + rl);
            for t in [l, r] {
                let n = self.leaf(t);
                keys.extend_from_slice(&n.keys[..n.len as usize]);
                items.extend_from_slice(&n.items[..n.len as usize]);
            }
            let total = keys.len();
            let parts = total.div_ceil(LCAP);
            let mut out = Vec::with_capacity(parts);
            let mut at = 0;
            for p in 0..parts {
                let t = match p {
                    0 => l,
                    1 => r,
                    _ => self.new_leaf(a, items[0]),
                };
                let take = (total - at) / (parts - p);
                let n = self.leaf_mut(t);
                n.keys[..take].copy_from_slice(&keys[at..at + take]);
                n.items[..take].copy_from_slice(&items[at..at + take]);
                n.len = len32(take);
                self.resum(a, t);
                out.push((keys[at], t));
                at += take;
            }
            if parts == 1 {
                self.free(r);
            }
            return out;
        }
        let ll = self.inner(l).len as usize;
        let last = self.inner(l).kids[ll - 1];
        let first = self.inner(r).kids[0];
        let mid = self.join_eq(a, last, first, h - 1);
        let mut entries = self.entries(l);
        entries.pop();
        entries.extend(mid);
        let rest = self.entries(r);
        entries.extend_from_slice(&rest[1..]);
        let lo = entries[0].0;
        self.free(l);
        self.free(r);
        self.pack(a, lo, &entries)
    }

    /// A tree of `items` (sorted by key, at least one), its leaves and
    /// inner nodes filled evenly.
    fn build(&mut self, a: &impl Aug<I, S, P>, items: &[(u64, I)]) -> u32 {
        let total = items.len();
        let parts = total.div_ceil(LCAP);
        let mut list = Vec::with_capacity(parts);
        let mut at = 0;
        for p in 0..parts {
            let take = (total - at) / (parts - p);
            let t = self.new_leaf(a, items[at].1);
            let n = self.leaf_mut(t);
            for (j, &(k, i)) in items[at..at + take].iter().enumerate() {
                n.keys[j] = k;
                n.items[j] = i;
            }
            n.len = len32(take);
            self.resum(a, t);
            list.push((items[at].0, t));
            at += take;
        }
        while list.len() > 1 {
            list = self.pack(a, list[0].0, &list);
        }
        list[0].1
    }

    /// Packs children (with least keys) evenly into new inner nodes.
    fn pack(&mut self, a: &impl Aug<I, S, P>, _lo: u64, entries: &[(u64, u32)]) -> Vec<(u64, u32)> {
        let total = entries.len();
        let parts = total.div_ceil(ICAP);
        let mut out = Vec::with_capacity(parts);
        let mut at = 0;
        for p in 0..parts {
            let take = (total - at) / (parts - p);
            let t = self.new_inner(a);
            for (j, &(k, c)) in entries[at..at + take].iter().enumerate() {
                let s = self.node_sum(c);
                let n = self.inner_mut(t);
                n.keys[j] = k;
                n.kids[j] = c;
                n.sums[j] = s;
            }
            self.inner_mut(t).len = len32(take);
            self.resum(a, t);
            out.push((entries[at].0, t));
            at += take;
        }
        out
    }
}

/// The pieces of a split arena (see `Arena::pieces`), written in place
/// in parallel: each owns the subtree under its root and its reserved
/// nodes.
/// The shared node buffers a piece writes through.
type Views<I, S, P> = (Disjoint<Leaf<I, S, P>>, Disjoint<Inner<S, P>>);

pub(crate) struct Pieces<'a, I, S, P> {
    arena: &'a mut Arena<I, S, P>,
    pub(crate) parts: Vec<Arena<I, S, P>>,
}

impl<I: Copy, S: Copy, P: Copy> Arena<I, S, P> {
    /// Arenas for the disjoint trees under `roots` (as splits leave them),
    /// the `i`th with `reserve[i]` (leaves, inner nodes) of its own to
    /// make nodes from. They write this arena's nodes in place; `finish`
    /// takes their spare and freed nodes back.
    pub(crate) fn pieces(&mut self, a: &impl Aug<I, S, P>, fill: I, reserve: &[(usize, usize)]) -> Pieces<'_, I, S, P> {
        debug_assert!(!self.special);
        let mut parts = Vec::with_capacity(reserve.len());
        let mut lists = Vec::with_capacity(reserve.len());
        for &(nl, ni) in reserve {
            let mut fl = Vec::with_capacity(nl);
            for _ in 0..nl {
                let t = self.new_leaf(a, fill);
                self.leaf_mut(t).len = 0;
                fl.push(t & !LEAF);
            }
            let mut fi = Vec::with_capacity(ni);
            for _ in 0..ni {
                let t = self.new_inner(a);
                fi.push(t);
            }
            lists.push((fl, fi));
        }
        let views = (Disjoint::new(&mut self.leaves), Disjoint::new(&mut self.inners));
        for (fl, fi) in lists {
            parts.push(Arena {
                free_leaves: fl,
                free_inners: fi,
                piece: Some(views),
                special: true,
                ..Arena::default()
            });
        }
        Pieces { arena: self, parts }
    }
}

impl<I: Copy, S: Copy, P: Copy> Pieces<'_, I, S, P> {
    /// Ends the pieces: their unused and freed nodes return to the arena's
    /// free lists.
    pub(crate) fn finish(self, parts: Vec<Arena<I, S, P>>) {
        for p in parts {
            debug_assert!(p.piece.is_some());
            self.arena.free_leaves.extend(p.free_leaves);
            self.arena.free_inners.extend(p.free_inners);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Arena, Aug, LazyFold, NIL};
    use std::cell::RefCell;
    use std::collections::{BTreeMap, HashMap};

    const P: u64 = 1_000_000_007;

    /// Brackets, a count, a max and a non-commutative content hash.
    #[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default)]
    struct S {
        c: u32,
        o: u32,
        n: u32,
        mx: u64,
        h: u64,
        p: u64,
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    struct It {
        c: u32,
        o: u32,
        v: u64,
    }

    struct A;

    impl Aug<It, S, (u32, u32)> for A {
        fn empty(&self) -> S {
            S { p: 1, ..S::default() }
        }
        fn leaf(&self, i: &It) -> S {
            S {
                c: i.c,
                o: i.o,
                n: 1,
                mx: i.v,
                h: i.v % P,
                p: 31,
            }
        }
        fn combine(&self, a: S, b: S) -> S {
            S {
                c: a.c + b.c.saturating_sub(a.o),
                o: b.o + a.o.saturating_sub(b.c),
                n: a.n + b.n,
                mx: a.mx.max(b.mx),
                h: (a.h * b.p + b.h) % P,
                p: (a.p * b.p) % P,
            }
        }
        fn proj(&self, s: &S) -> (u32, u32) {
            (s.c, s.o)
        }
        fn pempty(&self) -> (u32, u32) {
            (0, 0)
        }
        fn pcombine(&self, a: (u32, u32), b: (u32, u32)) -> (u32, u32) {
            (a.0 + b.0.saturating_sub(a.1), b.1 + a.1.saturating_sub(b.0))
        }
    }

    /// A sequence hash built lazily, with whole subtrees cached by summary.
    struct L<'a> {
        cache: &'a RefCell<HashMap<S, (u64, u64)>>,
    }

    impl LazyFold<It, S, (u64, u64)> for L<'_> {
        fn empty(&self) -> (u64, u64) {
            (0, 1)
        }
        fn leaf(&self, i: &It) -> (u64, u64) {
            ((i.v * 7 + 3) % P, 1_000_003)
        }
        fn combine(&self, a: (u64, u64), b: (u64, u64)) -> (u64, u64) {
            ((a.0 * b.1 + b.0) % P, (a.1 * b.1) % P)
        }
        fn whole(&self, sum: &S, build: &mut dyn FnMut() -> (u64, u64)) -> (u64, u64) {
            if let Some(&v) = self.cache.borrow().get(sum) {
                return v;
            }
            let v = build();
            self.cache.borrow_mut().insert(*sum, v);
            v
        }
    }

    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    fn model_fold(m: &BTreeMap<u64, It>, lo: u64, hi: u64) -> S {
        let mut s = A.empty();
        if lo < hi {
            for it in m.range(lo..hi).map(|x| x.1) {
                s = A.combine(s, A.leaf(it));
            }
        }
        s
    }

    /// The grown arena an overlay was made of, its model and root.
    type Grown = (Box<Arena<It, S, (u32, u32)>>, BTreeMap<u64, It>, u32);

    #[allow(clippy::too_many_lines)]
    #[test]
    fn matches_a_sorted_model() {
        let seeds: u64 = if cfg!(debug_assertions) { 10 } else { 48 };
        for seed in 1..seeds {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut ar: Arena<It, S, (u32, u32)> = Arena::default();
            let mut root = NIL;
            let mut other = NIL;
            let mut m: BTreeMap<u64, It> = BTreeMap::new();
            let space = [50u64, 400, 5000, 1 << 40][usize::try_from(seed % 4).unwrap()];
            // Grow first, then churn (for odd seeds, in an overlay of the
            // grown arena, which must stay as it was).
            let steps = 2500 + rng.below(2500);
            let mut base: Option<Grown> = None;
            for step in 0..steps {
                if seed % 2 == 1 && step == steps / 2 {
                    let b = Box::new(std::mem::take(&mut ar));
                    ar = Arena::over(&b);
                    base = Some((b, m.clone(), root));
                }
                let op = rng.below(100);
                let k = rng.below(space);
                let it = It {
                    c: u32::try_from(rng.below(3)).unwrap(),
                    o: u32::try_from(rng.below(3)).unwrap(),
                    v: rng.below(1000),
                };
                let grow = step < steps / 2;
                if op < if grow { 50 } else { 30 } {
                    if let std::collections::btree_map::Entry::Vacant(e) = m.entry(k) {
                        e.insert(it);
                        root = ar.insert(&A, root, k, it);
                    }
                } else if op < 40 && !grow {
                    // Fresh keys spread over existing ones (as a merge
                    // replays a run whose span holds another's keys).
                    let mut run: Vec<(u64, It)> = (0..rng.below(200))
                        .map(|_| (rng.below(space), it))
                        .filter(|(x, _)| !m.contains_key(x))
                        .collect();
                    run.sort_by_key(|x| x.0);
                    run.dedup_by_key(|x| x.0);
                    for &(x, i) in &run {
                        m.insert(x, i);
                    }
                    if !run.is_empty() {
                        root = ar.insert_run(&A, root, &run);
                    }
                } else if op < 55 {
                    // A run of fresh keys from `k` up to the next key.
                    let next = m.range(k..).next().map_or(u64::MAX, |x| *x.0);
                    let mut run = Vec::new();
                    let mut x = k;
                    while x < next && run.len() < 200 {
                        run.push((x, it));
                        x += 1 + rng.below(3);
                    }
                    for &(x, i) in &run {
                        m.insert(x, i);
                    }
                    root = ar.insert_run(&A, root, &run);
                } else if op < if grow { 65 } else { 85 } {
                    let (r, got) = ar.remove(&A, root, k);
                    root = r;
                    assert_eq!(got, m.remove(&k));
                    if let Some((&k2, _)) = m.range(k..).next()
                        && rng.below(2) == 0
                    {
                        let (r, got) = ar.remove(&A, root, k2);
                        root = r;
                        assert_eq!(got, m.remove(&k2));
                    }
                } else if op < 91 {
                    let found = ar.update(&A, root, k, &mut |i| i.v = it.v);
                    assert_eq!(found, m.contains_key(&k));
                    if let Some(x) = m.get_mut(&k) {
                        x.v = it.v;
                    }
                } else if op < 96 {
                    let mut ks: Vec<(u64, It)> = (0..rng.below(40))
                        .map(|_| {
                            let k = rng.below(space);
                            (m.range(k..).next().map_or(k, |x| *x.0), it)
                        })
                        .collect();
                    ks.sort_by_key(|x| x.0);
                    ks.dedup_by_key(|x| x.0);
                    ar.set_many(&A, root, &ks);
                    for (k, i) in ks {
                        if let Some(x) = m.get_mut(&k) {
                            *x = i;
                        }
                    }
                } else {
                    // A second tree in the same arena.
                    other = ar.insert(&A, other, k, it);
                }
                let lo = rng.below(space + 2);
                let hi = rng.below(space + 2);
                assert_eq!(ar.fold(&A, root, lo, hi), model_fold(&m, lo, hi));
                assert_eq!(ar.sum(&A, root), model_fold(&m, 0, u64::MAX));
                assert_eq!(ar.get(root, k).copied(), m.get(&k).copied());
                let want_next = if k < u64::MAX {
                    m.range(k + 1..).next().map(|x| (*x.0, *x.1))
                } else {
                    None
                };
                assert_eq!(ar.next(root, k).map(|x| (x.0, *x.1)), want_next);
                assert_eq!(
                    ar.prev(root, k).map(|x| (x.0, *x.1)),
                    m.range(..k).next_back().map(|x| (*x.0, *x.1))
                );
                assert_eq!(
                    ar.last(root).map(|x| x.0),
                    m.range(..u64::MAX).next_back().map(|x| *x.0)
                );
                let lim = usize::try_from(rng.below(12)).unwrap();
                let mut out: Vec<(u64, It)> = Vec::new();
                ar.range(root, lo, hi, &mut out, lim);
                let want: Vec<(u64, It)> = if lo <= hi {
                    m.range(lo..=hi).take(lim).map(|x| (*x.0, *x.1)).collect()
                } else {
                    Vec::new()
                };
                assert_eq!(out, want);
                let need = u32::try_from(rng.below(4)).unwrap();
                let mx = rng.below(1000);
                let back_model = |pred: &dyn Fn(S) -> bool| {
                    let mut acc = A.empty();
                    for (&x, it) in m.range(..hi).rev() {
                        acc = A.combine(A.leaf(it), acc);
                        if pred(acc) {
                            return Some(x);
                        }
                    }
                    None
                };
                let fwd_model = |pred: &dyn Fn(S) -> bool| {
                    let mut acc = A.empty();
                    for (&x, it) in m.range(lo + 1..) {
                        acc = A.combine(acc, A.leaf(it));
                        if pred(acc) {
                            return Some(x);
                        }
                    }
                    None
                };
                let p1 = |s: S| s.o > need;
                let p2 = |s: S| s.mx >= mx;
                let p3 = |s: S| s.c > need;
                assert_eq!(ar.search_back(&A, root, hi, &p1), back_model(&p1));
                assert_eq!(ar.search_back(&A, root, hi, &p2), back_model(&p2));
                assert_eq!(ar.search_fwd(&A, root, lo, &p3), fwd_model(&p3));
                assert_eq!(ar.search_fwd(&A, root, lo, &p2), fwd_model(&p2));
                let f = ar.fold(&A, root, lo, hi);
                assert_eq!(ar.pfold(&A, root, lo, hi), (f.c, f.o));
                assert_eq!(
                    ar.psearch_back(&A, root, hi, &|p: (u32, u32)| p.1 > need),
                    back_model(&p1)
                );
                assert_eq!(
                    ar.psearch_fwd(&A, root, lo, &|p: (u32, u32)| p.0 > need),
                    fwd_model(&p3)
                );
                // A visit skipping subtrees whose values are all small.
                let small = rng.below(1000);
                let mut seen = Vec::new();
                let mut before = A.empty();
                ar.visit(
                    &A,
                    root,
                    lo,
                    hi,
                    &mut before,
                    &|_, s: S| s.mx < small,
                    &mut |k, i, b| {
                        if i.v >= small {
                            seen.push((k, *i, b));
                        }
                    },
                );
                let mut want: Vec<(u64, It, S)> = Vec::new();
                let mut acc = A.empty();
                for (&x, it) in &m {
                    if x >= hi {
                        break;
                    }
                    if x >= lo && it.v >= small {
                        want.push((x, *it, acc));
                    }
                    acc = A.combine(acc, A.leaf(it));
                }
                assert_eq!(seen, want);
                let cache = RefCell::new(HashMap::new());
                let l = L { cache: &cache };
                let mut want = l.empty();
                if lo < hi {
                    for it in m.range(lo..hi).map(|x| x.1) {
                        want = l.combine(want, l.leaf(it));
                    }
                }
                assert_eq!(ar.lazy_fold(&l, root, lo, hi), want);
                assert_eq!(ar.lazy_fold(&l, root, lo, hi), want);
            }
            if let Some((b, bm, broot)) = &base {
                assert_eq!(b.fold(&A, *broot, 0, u64::MAX), model_fold(bm, 0, u64::MAX));
                let mut out: Vec<(u64, It)> = Vec::new();
                b.range(*broot, 0, u64::MAX, &mut out, usize::MAX);
                assert_eq!(out.len(), bm.len());
            } else {
                let mut n = 0;
                ar.for_each(&mut |_, _| n += 1);
                assert!(n >= m.len());
            }
            let _ = other;
        }
    }
}
