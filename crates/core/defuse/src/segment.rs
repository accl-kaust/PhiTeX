//! A segment: a bounded range of the line and everything about it.
//!
//! The segment is the unit of locking. A read of field `f` at `p` locks
//! `p`'s segment for one map lookup (and, through the field index, the one
//! earlier segment that last defines `f`). A commit locks the committing
//! node's segment for one diff. Nothing ever holds two segments.

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::api::{NodeId, OpId, PathId, UnitId};
use crate::engine::Engine;
use crate::lock::Lock;
use crate::node::State;
use crate::order::{Pos, SEG_CAP, SegId};
use crate::values::{Field, FxHashMap, ValueId};

pub(crate) struct Segment {
    pub(crate) id: SegId,
    pub(crate) unit: UnitId,
    /// Its first label, fixed for its life (a split keeps the lower half here).
    pub(crate) lo: Pos,
    pub(crate) inner: Lock<Inner>,
}

pub(crate) struct Inner {
    /// One past its last label.
    pub(crate) hi: Pos,
    pub(crate) nodes: BTreeMap<Pos, NodeId>,
    /// Definitions by field, in position order. A relation's value is kept
    /// materialised and recomputed in place when its input changes.
    pub(crate) defs: FxHashMap<Field, BTreeMap<Pos, Def>>,
    /// Readers of each field anchored in this segment. Stale records (an
    /// older serial than the reader's) are dropped when met.
    pub(crate) readers: FxHashMap<Field, Vec<Reader>>,
    /// Contributions by field, in position order, and their fold.
    pub(crate) contribs: FxHashMap<Field, BTreeMap<Pos, ValueId>>,
    pub(crate) sums: FxHashMap<Field, ValueId>,
    /// Readers of folds whose range covers this segment.
    pub(crate) range_readers: FxHashMap<Field, Vec<RangeReader>>,
    /// Its nodes by operation, for `commit::learn_shape`.
    pub(crate) by_op: FxHashMap<OpId, Vec<NodeId>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Def {
    pub(crate) value: ValueId,
    pub(crate) by: NodeId,
    /// `Some(c)`: value = the definition reaching `by` + c.
    pub(crate) rel: Option<i64>,
    /// Filed before its node ran (a prediction, a learned shape): the run
    /// replaces it.
    pub(crate) provisional: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct Reader {
    pub(crate) node: NodeId,
    pub(crate) serial: u32,
    /// Whose position the read is at (the reader itself, or the node an
    /// `At` names).
    pub(crate) anchor: NodeId,
    /// `At::AfterOwn`: the anchor's own definition counts.
    pub(crate) inclusive: bool,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
}

#[derive(Clone, Debug)]
pub(crate) struct RangeReader {
    pub(crate) node: NodeId,
    pub(crate) serial: u32,
    pub(crate) from: Option<NodeId>,
    /// A fold's end (None: the reader); a `next`'s stop: the contribution
    /// found or the uncommitted node the search stopped at (None: the end).
    pub(crate) to: Option<NodeId>,
    /// `Ctx::next_contribution` rather than a fold.
    pub(crate) next: bool,
    pub(crate) path: PathId,
    pub(crate) seen: Option<ValueId>,
}

/// Whether a reader record still stands for its node.
pub(crate) fn live(eng: &Engine, node: NodeId, serial: u32) -> bool {
    let n = eng.node(node);
    n.serial.load(Ordering::Acquire) == serial && n.state() != State::Gone
}

/// Pushes a record, pruning the stale ones when the list is about to grow.
pub(crate) fn push_pruned<T>(v: &mut Vec<T>, x: T, live: impl Fn(&T) -> bool) {
    if v.len() >= 64 && v.len() == v.capacity() {
        v.retain(live);
    }
    v.push(x);
}

/// A balanced reduction: the monoid is associative, and a balanced fold
/// keeps a sequence's rope shallow.
pub(crate) fn reduce(eng: &Engine, mono: &dyn crate::api::Monoid, xs: Vec<ValueId>) -> ValueId {
    let v = eng.values();
    if xs.is_empty() {
        return mono.empty(v);
    }
    let mut xs = xs;
    while xs.len() > 1 {
        let mut next = Vec::with_capacity(xs.len().div_ceil(2));
        let mut it = xs.chunks(2);
        for pair in &mut it {
            next.push(if pair.len() == 2 { mono.combine(v, pair[0], pair[1]) } else { pair[0] });
        }
        xs = next;
    }
    xs[0]
}

impl Inner {
    /// Its sum of `f` is stale: dropped, computed again at the next fold
    /// (`sum`); an empty field is dropped.
    pub(crate) fn resum(&mut self, _eng: &Engine, f: Field) {
        self.sums.remove(&f);
        if self.contribs.get(&f).is_some_and(BTreeMap::is_empty) {
            self.contribs.remove(&f);
        }
    }

    /// The sum of its contributions to `f`, cached.
    pub(crate) fn sum(&mut self, eng: &Engine, f: Field) -> ValueId {
        if let Some(&x) = self.sums.get(&f) {
            return x;
        }
        let mono = eng.monoid(f);
        let xs: Vec<ValueId> = self.contribs.get(&f).map(|m| m.values().copied().collect()).unwrap_or_default();
        let x = reduce(eng, &*mono, xs);
        self.sums.insert(f, x);
        x
    }
}

impl Segment {
    pub(crate) fn new(id: SegId, unit: UnitId, lo: Pos, hi: Pos) -> Segment {
        Segment {
            id,
            unit,
            lo,
            inner: Lock::new(Inner {
                hi,
                nodes: BTreeMap::new(),
                defs: FxHashMap::default(),
                readers: FxHashMap::default(),
                contribs: FxHashMap::default(),
                sums: FxHashMap::default(),
                range_readers: FxHashMap::default(),
                by_op: FxHashMap::default(),
            }),
        }
    }

    /// The last definition of `f` before `p` (at `p` too when `inclusive`).
    pub(crate) fn reaching(inner: &Inner, f: Field, p: Pos, inclusive: bool) -> Option<(Pos, &Def)> {
        let m = inner.defs.get(&f)?;
        let r = if inclusive { m.range(..=p).next_back() } else { m.range(..p).next_back() };
        r.map(|(p, d)| (*p, d))
    }

    /// The first definition of `f` after `p`.
    pub(crate) fn next_def(inner: &Inner, f: Field, p: Pos) -> Option<(Pos, &Def)> {
        inner.defs.get(&f)?.range((Excluded(p), Unbounded)).next().map(|(p, d)| (*p, d))
    }

    /// A label strictly after `after` and before the next node (or `hi`);
    /// None when that gap is closed and the segment must relabel or split.
    pub(crate) fn gap_after(inner: &Inner, after: Pos) -> Option<Pos> {
        let next = inner
            .nodes
            .range((Excluded(after), Unbounded))
            .next()
            .map_or(inner.hi, |(p, _)| *p);
        (next.0 - after.0 >= 2).then(|| Pos(after.0 + (next.0 - after.0) / 2))
    }

    /// A label strictly before `before` and after the previous node (or
    /// `lo`); None when that gap is closed.
    pub(crate) fn gap_before(&self, inner: &Inner, before: Pos) -> Option<Pos> {
        let prev = inner.nodes.range(..before).next_back().map_or(self.lo.0.saturating_sub(1), |(p, _)| p.0);
        (before.0 - prev >= 2).then(|| Pos(prev + (before.0 - prev) / 2))
    }

    /// Spreads its nodes' labels evenly over `[lo, hi)`, moving each node's
    /// `pos` and re-keying `nodes`, `defs` and `contribs`. Under the write
    /// lock; nothing outside holds a label.
    pub(crate) fn relabel(&self, inner: &mut Inner, eng: &Engine) {
        let n = inner.nodes.len() as u64;
        let width = inner.hi.0 - self.lo.0;
        assert!(width > n + 1, "segment {:?} out of labels: {n} nodes in {width}", self.id);
        let step = width / (n + 1);
        let old: Vec<(Pos, NodeId)> = inner.nodes.iter().map(|(p, n)| (*p, *n)).collect();
        let mut map: FxHashMap<Pos, Pos> = FxHashMap::default();
        let mut nodes = BTreeMap::new();
        for (i, (p, id)) in old.iter().enumerate() {
            let np = Pos(self.lo.0 + (i as u64 + 1) * step);
            map.insert(*p, np);
            nodes.insert(np, *id);
            eng.node(*id).pos.store(np.0, Ordering::Release);
        }
        inner.nodes = nodes;
        for m in inner.defs.values_mut() {
            rekey(m, &map);
        }
        for m in inner.contribs.values_mut() {
            rekey(m, &map);
        }
    }

    pub(crate) fn needs_split(inner: &Inner) -> bool {
        inner.nodes.len() > SEG_CAP
    }

    /// Moves the nodes at and after the median label into a new segment
    /// (their labels unchanged, their `seg` updated), with their
    /// definitions, readers, contributions, sums, range readers and by-op
    /// entries; `hi` shrinks to the median. The new segment is registered
    /// in the order and the field indexes before this returns.
    pub(crate) fn split(&self, inner: &mut Inner, eng: &Engine, new_id: SegId) -> Arc<Segment> {
        let mid = *inner.nodes.keys().nth(inner.nodes.len() / 2).expect("a segment to split has nodes");
        self.split_at(inner, eng, mid, new_id)
    }

    /// `split`, at label `at`: the new segment is `[at, hi)`.
    pub(crate) fn split_at(&self, inner: &mut Inner, eng: &Engine, at: Pos, new_id: SegId) -> Arc<Segment> {
        let hi = inner.hi;
        inner.hi = at;
        let nodes = inner.nodes.split_off(&at);
        let mut defs = FxHashMap::default();
        let mut gone_defs = Vec::new();
        for (f, m) in &mut inner.defs {
            let up = m.split_off(&at);
            if !up.is_empty() {
                defs.insert(*f, up);
            }
            if m.is_empty() {
                gone_defs.push(*f);
            }
        }
        inner.defs.retain(|_, m| !m.is_empty());
        let mut contribs = FxHashMap::default();
        let mut gone_contribs = Vec::new();
        let mut resum = Vec::new();
        for (f, m) in &mut inner.contribs {
            let up = m.split_off(&at);
            if !up.is_empty() {
                contribs.insert(*f, up);
                resum.push(*f);
            }
            if m.is_empty() {
                gone_contribs.push(*f);
            }
        }
        for f in &resum {
            inner.resum(eng, *f);
        }
        let mut readers: FxHashMap<Field, Vec<Reader>> = FxHashMap::default();
        let mut gone_readers = Vec::new();
        for (f, v) in &mut inner.readers {
            let (up, down): (Vec<Reader>, Vec<Reader>) =
                v.drain(..).partition(|r| eng.node(r.anchor).pos_hint() >= at);
            *v = down;
            if !up.is_empty() {
                readers.insert(*f, up);
            }
            if v.is_empty() {
                gone_readers.push(*f);
            }
        }
        inner.readers.retain(|_, v| !v.is_empty());
        let range_readers = inner.range_readers.clone();
        let mut by_op: FxHashMap<OpId, Vec<NodeId>> = FxHashMap::default();
        for (op, v) in &mut inner.by_op {
            let (up, down): (Vec<NodeId>, Vec<NodeId>) = v.drain(..).partition(|n| eng.node(*n).pos_hint() >= at);
            *v = down;
            if !up.is_empty() {
                by_op.insert(*op, up);
            }
        }
        inner.by_op.retain(|_, v| !v.is_empty());
        let new = Arc::new(Segment {
            id: new_id,
            unit: self.unit,
            lo: at,
            inner: Lock::new(Inner {
                hi,
                nodes,
                defs,
                readers,
                contribs,
                sums: FxHashMap::default(),
                range_readers,
                by_op,
            }),
        });
        {
            let mut ni = new.inner.write();
            for f in &resum {
                ni.resum(eng, *f);
            }
            eng.segs.set(new_id.0 as usize, new.clone());
            for id in ni.nodes.values() {
                eng.node(*id).seg.store(new_id.0, Ordering::Release);
            }
            for f in ni.defs.keys() {
                eng.defs.add(*f, at, new_id);
            }
            for f in ni.contribs.keys() {
                eng.contribs.add(*f, at, new_id);
            }
            for f in ni.readers.keys() {
                eng.readers.add(*f, at, new_id);
            }
        }
        for f in gone_defs {
            eng.defs.remove(f, self.lo, self.id);
        }
        for f in gone_contribs {
            eng.contribs.remove(f, self.lo, self.id);
        }
        for f in gone_readers {
            eng.readers.remove(f, self.lo, self.id);
        }
        eng.order.insert(at, new_id);
        new
    }
}

fn rekey<V>(m: &mut BTreeMap<Pos, V>, map: &FxHashMap<Pos, Pos>) {
    let old = std::mem::take(m);
    for (p, v) in old {
        m.insert(map.get(&p).copied().unwrap_or(p), v);
    }
}
