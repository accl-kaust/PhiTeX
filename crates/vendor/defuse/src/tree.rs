//! An arena of augmented treaps keyed by `u64`, with cached subtree
//! summaries: O(log n) insert, remove, update, range fold and monotone
//! searches.

pub(crate) const NIL: u32 = u32::MAX;

/// How items are summarised.
pub(crate) trait Aug<I, S: Copy> {
    fn empty(&self) -> S;
    fn leaf(&self, item: &I) -> S;
    fn combine(&self, a: S, b: S) -> S;
}

struct TNode<I, S> {
    key: u64,
    prio: u64,
    left: u32,
    right: u32,
    item: I,
    sum: S,
}

pub(crate) struct Arena<I, S> {
    nodes: Vec<Option<TNode<I, S>>>,
    free: Vec<u32>,
}

impl<I, S: Copy> Default for Arena<I, S> {
    fn default() -> Self {
        Arena {
            nodes: Vec::new(),
            free: Vec::new(),
        }
    }
}

fn prio_of(key: u64) -> u64 {
    let mut x = key.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

impl<I, S: Copy> Arena<I, S> {
    fn n(&self, i: u32) -> &TNode<I, S> {
        self.nodes[i as usize].as_ref().expect("live treap node")
    }

    fn nm(&mut self, i: u32) -> &mut TNode<I, S> {
        self.nodes[i as usize].as_mut().expect("live treap node")
    }

    pub(crate) fn sum<A: Aug<I, S>>(&self, a: &A, t: u32) -> S {
        if t == NIL { a.empty() } else { self.n(t).sum }
    }

    fn pull<A: Aug<I, S>>(&mut self, a: &A, t: u32) {
        let n = self.n(t);
        let (l, r) = (n.left, n.right);
        let s = a.combine(a.combine(self.sum(a, l), a.leaf(&n.item)), self.sum(a, r));
        self.nm(t).sum = s;
    }

    /// Splits into keys `< key` and `>= key`.
    fn split<A: Aug<I, S>>(&mut self, a: &A, t: u32, key: u64) -> (u32, u32) {
        if t == NIL {
            return (NIL, NIL);
        }
        if self.n(t).key < key {
            let r = self.n(t).right;
            let (x, y) = self.split(a, r, key);
            self.nm(t).right = x;
            self.pull(a, t);
            (t, y)
        } else {
            let l = self.n(t).left;
            let (x, y) = self.split(a, l, key);
            self.nm(t).left = y;
            self.pull(a, t);
            (x, t)
        }
    }

    fn merge<A: Aug<I, S>>(&mut self, a: &A, x: u32, y: u32) -> u32 {
        if x == NIL {
            return y;
        }
        if y == NIL {
            return x;
        }
        if self.n(x).prio >= self.n(y).prio {
            let r = self.n(x).right;
            let m = self.merge(a, r, y);
            self.nm(x).right = m;
            self.pull(a, x);
            x
        } else {
            let l = self.n(y).left;
            let m = self.merge(a, x, l);
            self.nm(y).left = m;
            self.pull(a, y);
            y
        }
    }

    pub(crate) fn insert<A: Aug<I, S>>(&mut self, a: &A, root: u32, key: u64, item: I) -> u32 {
        let sum = a.leaf(&item);
        let node = TNode {
            key,
            prio: prio_of(key),
            left: NIL,
            right: NIL,
            item,
            sum,
        };
        let i = if let Some(i) = self.free.pop() {
            self.nodes[i as usize] = Some(node);
            i
        } else {
            self.nodes.push(Some(node));
            u32::try_from(self.nodes.len() - 1).expect("treap arena overflow")
        };
        let (l, r) = self.split(a, root, key);
        let l = self.merge(a, l, i);
        self.merge(a, l, r)
    }

    pub(crate) fn remove<A: Aug<I, S>>(&mut self, a: &A, root: u32, key: u64) -> (u32, Option<I>) {
        let (l, r) = self.split(a, root, key);
        let (m, r) = self.split(a, r, key.saturating_add(1));
        let mut item = None;
        if m != NIL {
            item = self.nodes[m as usize].take().map(|n| n.item);
            self.free.push(m);
        }
        (self.merge(a, l, r), item)
    }

    /// Applies `f` to the item at `key` and re-summarises the path.
    pub(crate) fn update<A: Aug<I, S>>(&mut self, a: &A, t: u32, key: u64, f: &mut dyn FnMut(&mut I)) -> bool {
        if t == NIL {
            return false;
        }
        let found = match key.cmp(&self.n(t).key) {
            std::cmp::Ordering::Equal => {
                f(&mut self.nm(t).item);
                true
            }
            std::cmp::Ordering::Less => {
                let l = self.n(t).left;
                self.update(a, l, key, f)
            }
            std::cmp::Ordering::Greater => {
                let r = self.n(t).right;
                self.update(a, r, key, f)
            }
        };
        if found {
            self.pull(a, t);
        }
        found
    }

    /// Sets the items of existing keys (`items` sorted by key) and
    /// re-summarises the union of their paths once.
    pub(crate) fn set_many<A: Aug<I, S>>(&mut self, a: &A, t: u32, items: &[(u64, I)])
    where
        I: Copy,
    {
        if t == NIL || items.is_empty() {
            return;
        }
        let k = self.n(t).key;
        let split = items.partition_point(|x| x.0 < k);
        let (lo, rest) = items.split_at(split);
        let (here, hi) = match rest.first() {
            Some(x) if x.0 == k => (Some(x.1), &rest[1..]),
            _ => (None, rest),
        };
        let (l, r) = (self.n(t).left, self.n(t).right);
        self.set_many(a, l, lo);
        self.set_many(a, r, hi);
        if let Some(i) = here {
            self.nm(t).item = i;
        }
        self.pull(a, t);
    }

    pub(crate) fn get(&self, mut t: u32, key: u64) -> Option<&I> {
        while t != NIL {
            let n = self.n(t);
            if key == n.key {
                return Some(&n.item);
            }
            t = if key < n.key { n.left } else { n.right };
        }
        None
    }

    /// Summary of the keys in `[lo, hi)`.
    pub(crate) fn fold<A: Aug<I, S>>(&self, a: &A, t: u32, lo: u64, hi: u64) -> S {
        self.fold_in(a, t, lo, hi, false, false)
    }

    fn fold_in<A: Aug<I, S>>(&self, a: &A, t: u32, lo: u64, hi: u64, all_lo: bool, all_hi: bool) -> S {
        if t == NIL || lo >= hi {
            return a.empty();
        }
        if all_lo && all_hi {
            return self.n(t).sum;
        }
        let n = self.n(t);
        if !all_lo && n.key < lo {
            return self.fold_in(a, n.right, lo, hi, false, all_hi);
        }
        if !all_hi && n.key >= hi {
            return self.fold_in(a, n.left, lo, hi, all_lo, false);
        }
        let l = self.fold_in(a, n.left, lo, hi, all_lo, true);
        let r = self.fold_in(a, n.right, lo, hi, true, all_hi);
        a.combine(a.combine(l, a.leaf(&n.item)), r)
    }

    /// The nearest key `k < before` such that `pred(fold[k, before))` holds,
    /// for a predicate monotone in the range growing to the left.
    pub(crate) fn search_back<A: Aug<I, S>>(
        &self,
        a: &A,
        t: u32,
        before: u64,
        pred: &dyn Fn(S) -> bool,
    ) -> Option<u64> {
        let mut acc = a.empty();
        self.back(a, t, before, &mut acc, pred)
    }

    fn back<A: Aug<I, S>>(&self, a: &A, t: u32, before: u64, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let n = self.n(t);
        if n.key >= before {
            return self.back(a, n.left, before, acc, pred);
        }
        if let Some(k) = self.back(a, n.right, before, acc, pred) {
            return Some(k);
        }
        *acc = a.combine(a.leaf(&n.item), *acc);
        if pred(*acc) {
            return Some(n.key);
        }
        self.back_all(a, n.left, acc, pred)
    }

    fn back_all<A: Aug<I, S>>(&self, a: &A, t: u32, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let n = self.n(t);
        let whole = a.combine(n.sum, *acc);
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if let Some(k) = self.back_all(a, n.right, acc, pred) {
            return Some(k);
        }
        *acc = a.combine(a.leaf(&n.item), *acc);
        if pred(*acc) {
            return Some(n.key);
        }
        self.back_all(a, n.left, acc, pred)
    }

    /// The nearest key `k > after` such that `pred(fold(after, k]))` holds,
    /// for a predicate monotone in the range growing to the right.
    pub(crate) fn search_fwd<A: Aug<I, S>>(&self, a: &A, t: u32, after: u64, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        let mut acc = a.empty();
        self.fwd(a, t, after, &mut acc, pred)
    }

    fn fwd<A: Aug<I, S>>(&self, a: &A, t: u32, after: u64, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let n = self.n(t);
        if n.key <= after {
            return self.fwd(a, n.right, after, acc, pred);
        }
        if let Some(k) = self.fwd(a, n.left, after, acc, pred) {
            return Some(k);
        }
        *acc = a.combine(*acc, a.leaf(&n.item));
        if pred(*acc) {
            return Some(n.key);
        }
        self.fwd_all(a, n.right, acc, pred)
    }

    fn fwd_all<A: Aug<I, S>>(&self, a: &A, t: u32, acc: &mut S, pred: &dyn Fn(S) -> bool) -> Option<u64> {
        if t == NIL {
            return None;
        }
        let n = self.n(t);
        let whole = a.combine(*acc, n.sum);
        if !pred(whole) {
            *acc = whole;
            return None;
        }
        if let Some(k) = self.fwd_all(a, n.left, acc, pred) {
            return Some(k);
        }
        *acc = a.combine(*acc, a.leaf(&n.item));
        if pred(*acc) {
            return Some(n.key);
        }
        self.fwd_all(a, n.right, acc, pred)
    }

    /// Visits the items with keys in `[lo, hi)` in key order, each with the
    /// summary of everything before it (from the start of the tree), leaving
    /// out the subtrees `skip(before, sum)` rejects (their summaries still
    /// count as passed).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn visit<A: Aug<I, S>>(
        &self,
        a: &A,
        t: u32,
        lo: u64,
        hi: u64,
        before: &mut S,
        skip: &dyn Fn(S, S) -> bool,
        f: &mut dyn FnMut(u64, &I, S),
    ) {
        if t == NIL {
            return;
        }
        let n = self.n(t);
        if n.key < lo {
            *before = a.combine(a.combine(*before, self.sum(a, n.left)), a.leaf(&n.item));
            self.visit(a, n.right, lo, hi, before, skip, f);
            return;
        }
        if n.key >= hi {
            self.visit(a, n.left, lo, hi, before, skip, f);
            return;
        }
        let ls = self.sum(a, n.left);
        if skip(*before, ls) {
            *before = a.combine(*before, ls);
        } else {
            self.visit(a, n.left, lo, hi, before, skip, f);
        }
        f(n.key, &n.item, *before);
        *before = a.combine(*before, a.leaf(&n.item));
        let rs = self.sum(a, n.right);
        if skip(*before, rs) {
            *before = a.combine(*before, rs);
        } else {
            self.visit(a, n.right, lo, hi, before, skip, f);
        }
    }

    /// The first key `> key`.
    pub(crate) fn next(&self, mut t: u32, key: u64) -> Option<(u64, &I)> {
        let mut best = None;
        while t != NIL {
            let n = self.n(t);
            if n.key > key {
                best = Some((n.key, &n.item));
                t = n.left;
            } else {
                t = n.right;
            }
        }
        best
    }

    /// The last key `< key`.
    pub(crate) fn prev(&self, mut t: u32, key: u64) -> Option<(u64, &I)> {
        let mut best = None;
        while t != NIL {
            let n = self.n(t);
            if n.key < key {
                best = Some((n.key, &n.item));
                t = n.right;
            } else {
                t = n.left;
            }
        }
        best
    }

    /// The last key overall.
    pub(crate) fn last(&self, t: u32) -> Option<(u64, &I)> {
        self.prev(t, u64::MAX)
    }

    /// Up to `limit` `(key, item)` pairs with keys in `[lo, hi]`, in order.
    pub(crate) fn range<O>(&self, t: u32, lo: u64, hi: u64, out: &mut Vec<(u64, O)>, limit: usize)
    where
        I: Copy + Into<O>,
    {
        if t == NIL || out.len() >= limit {
            return;
        }
        let n = self.n(t);
        if n.key > lo {
            self.range(n.left, lo, hi, out, limit);
        }
        if n.key >= lo && n.key <= hi && out.len() < limit {
            out.push((n.key, n.item.into()));
        }
        if n.key < hi {
            self.range(n.right, lo, hi, out, limit);
        }
    }
}

/// A second fold over a treap, into values that are expensive to build:
/// whole subtrees go through `whole` (e.g. a cache keyed by their summary).
pub(crate) trait LazyFold<I, S, R: Copy> {
    fn empty(&self) -> R;
    fn leaf(&self, item: &I) -> R;
    fn combine(&self, a: R, b: R) -> R;
    /// The value of a whole subtree with summary `sum`, computed by `build`
    /// if not known.
    fn whole(&self, sum: &S, build: &mut dyn FnMut() -> R) -> R;
}

impl<I, S: Copy> Arena<I, S> {
    /// The lazy fold of the keys in `[lo, hi)`.
    pub(crate) fn lazy_fold<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32, lo: u64, hi: u64) -> R {
        if t == NIL || lo >= hi {
            return f.empty();
        }
        let n = self.n(t);
        if n.key < lo {
            return self.lazy_fold(f, n.right, lo, hi);
        }
        if n.key >= hi {
            return self.lazy_fold(f, n.left, lo, hi);
        }
        let l = self.lazy_from(f, n.left, lo);
        let r = self.lazy_to(f, n.right, hi);
        f.combine(f.combine(l, f.leaf(&n.item)), r)
    }

    /// Keys `>= lo` of subtree `t`.
    fn lazy_from<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32, lo: u64) -> R {
        if t == NIL {
            return f.empty();
        }
        let n = self.n(t);
        if n.key < lo {
            return self.lazy_from(f, n.right, lo);
        }
        let l = self.lazy_from(f, n.left, lo);
        let r = self.lazy_whole(f, n.right);
        f.combine(f.combine(l, f.leaf(&n.item)), r)
    }

    /// Keys `< hi` of subtree `t`.
    fn lazy_to<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32, hi: u64) -> R {
        if t == NIL {
            return f.empty();
        }
        let n = self.n(t);
        if n.key >= hi {
            return self.lazy_to(f, n.left, hi);
        }
        let l = self.lazy_whole(f, n.left);
        let r = self.lazy_to(f, n.right, hi);
        f.combine(f.combine(l, f.leaf(&n.item)), r)
    }

    pub(crate) fn lazy_whole<R: Copy>(&self, f: &dyn LazyFold<I, S, R>, t: u32) -> R {
        if t == NIL {
            return f.empty();
        }
        let n = self.n(t);
        f.whole(&n.sum, &mut || {
            let l = self.lazy_whole(f, n.left);
            let r = self.lazy_whole(f, n.right);
            f.combine(f.combine(l, f.leaf(&n.item)), r)
        })
    }
}

impl<I, S: Copy> Arena<I, S> {
    /// Visits every live item and cached sum.
    pub(crate) fn for_each(&self, f: &mut dyn FnMut(&I, &S)) {
        for n in self.nodes.iter().flatten() {
            f(&n.item, &n.sum);
        }
    }
}
