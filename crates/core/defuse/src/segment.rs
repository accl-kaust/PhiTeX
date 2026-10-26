//! A segment: a bounded range of the line and everything about it.
//!
//! The segment is the unit of locking. A read of field `f` at `p` locks
//! `p`'s segment for one map lookup (and, through the field index, the one
//! earlier segment that last defines `f`). A commit locks the committing
//! node's segment for one diff. Nothing ever holds two segments.

use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Included, Unbounded};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::api::{NodeId, OpId, PathId, UnitId};
use crate::engine::Engine;
use crate::index::RangeKey;
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
    pub(crate) defs: FxHashMap<Field, PosMap<Def>>,
    /// Readers of each field anchored in this segment. Stale records (an
    /// older serial than the reader's) are dropped when met.
    pub(crate) readers: FxHashMap<Field, Vec<Reader>>,
    /// Contributions by field, in position order, and their fold.
    pub(crate) contribs: FxHashMap<Field, PosMap<ValueId>>,
    pub(crate) sums: FxHashMap<Field, ValueId>,
    /// Counts the sums dropped (`resum`): a sum computed outside the lock
    /// is kept only if none was dropped meanwhile (`read::segment_part`).
    pub(crate) sums_gen: u64,
    /// `next` readers whose search stopped at one of its nodes, not yet
    /// committed (`commit::reask_holes`); their ranges are in
    /// `Engine::ranges`.
    pub(crate) stops: Vec<(Field, RangeKey, RangeReader)>,
    /// Its nodes by operation, for `commit::learn_shape`.
    pub(crate) by_op: FxHashMap<OpId, Vec<NodeId>>,
    /// Each node's scope brackets, by position; absent means none.
    pub(crate) brackets: PosMap<crate::api::Brackets>,
    /// The segment's scope summary: the net change of depth over it, and
    /// the lowest depth reached inside it relative to its start (≤ 0),
    /// kept current with `brackets` (`Inner::sum_depth`). A visibility
    /// walk skips a whole segment by these.
    pub(crate) depth_net: i32,
    pub(crate) depth_min: i32,
    /// Its nodes that have not committed (about: a walk skips the search
    /// for one when there are none, `commit::first_hole`).
    pub(crate) holes: usize,
}

/// A field's definitions or contributions in one segment, by label. Most
/// fields have one per segment, kept without an allocation; more are a
/// sorted run whose room a commit makes before its lock (`Grow`).
#[derive(Clone, Debug, Default)]
pub(crate) enum PosMap<V> {
    #[default]
    Zero,
    One(Pos, V),
    Many(Vec<(Pos, V)>),
}

pub(crate) enum PosIter<'a, V> {
    One(Option<(&'a Pos, &'a V)>),
    Run(std::slice::Iter<'a, (Pos, V)>),
}

impl<'a, V> Iterator for PosIter<'a, V> {
    type Item = (&'a Pos, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        match self {
            PosIter::One(x) => x.take(),
            PosIter::Run(i) => i.next().map(|(p, v)| (p, v)),
        }
    }
}

impl<V> DoubleEndedIterator for PosIter<'_, V> {
    fn next_back(&mut self) -> Option<Self::Item> {
        match self {
            PosIter::One(x) => x.take(),
            PosIter::Run(i) => i.next_back().map(|(p, v)| (p, v)),
        }
    }
}

impl<V> PosMap<V> {
    pub(crate) fn is_empty(&self) -> bool {
        match self {
            PosMap::Zero => true,
            PosMap::One(..) => false,
            PosMap::Many(m) => m.is_empty(),
        }
    }

    fn find(m: &[(Pos, V)], p: Pos) -> Result<usize, usize> {
        m.binary_search_by(|x| x.0.cmp(&p))
    }

    pub(crate) fn get(&self, p: &Pos) -> Option<&V> {
        match self {
            PosMap::Zero => None,
            PosMap::One(q, v) => (q == p).then_some(v),
            PosMap::Many(m) => Self::find(m, *p).ok().map(|i| &m[i].1),
        }
    }

    pub(crate) fn get_mut(&mut self, p: &Pos) -> Option<&mut V> {
        match self {
            PosMap::Zero => None,
            PosMap::One(q, v) => (q == p).then_some(v),
            PosMap::Many(m) => Self::find(m, *p).ok().map(|i| &mut m[i].1),
        }
    }

    /// Whether inserting at `p` allocates.
    pub(crate) fn full_for(&self, p: Pos) -> bool {
        match self {
            PosMap::Zero => false,
            PosMap::One(q, _) => *q != p,
            PosMap::Many(m) => m.len() == m.capacity() && Self::find(m, p).is_err(),
        }
    }

    pub(crate) fn len(&self) -> usize {
        match self {
            PosMap::Zero => 0,
            PosMap::One(..) => 1,
            PosMap::Many(m) => m.len(),
        }
    }

    pub(crate) fn insert(&mut self, p: Pos, v: V) -> Option<V> {
        match self {
            PosMap::Zero => {
                *self = PosMap::One(p, v);
                None
            }
            PosMap::One(q, w) if *q == p => Some(std::mem::replace(w, v)),
            PosMap::One(..) => {
                let PosMap::One(q, w) = std::mem::take(self) else {
                    unreachable!()
                };
                let mut m = Vec::with_capacity(4);
                m.push((q, w));
                *self = PosMap::Many(m);
                self.insert(p, v)
            }
            PosMap::Many(m) => match Self::find(m, p) {
                Ok(i) => Some(std::mem::replace(&mut m[i].1, v)),
                Err(i) => {
                    m.insert(i, (p, v));
                    None
                }
            },
        }
    }

    pub(crate) fn remove(&mut self, p: &Pos) -> Option<V> {
        match self {
            PosMap::Zero => None,
            PosMap::One(q, _) if q != p => None,
            PosMap::One(..) => match std::mem::take(self) {
                PosMap::One(_, v) => Some(v),
                _ => unreachable!(),
            },
            PosMap::Many(m) => Self::find(m, *p).ok().map(|i| m.remove(i).1),
        }
    }

    pub(crate) fn iter(&self) -> PosIter<'_, V> {
        match self {
            PosMap::Zero => PosIter::One(None),
            PosMap::One(q, v) => PosIter::One(Some((q, v))),
            PosMap::Many(m) => PosIter::Run(m.iter()),
        }
    }

    pub(crate) fn range<R: std::ops::RangeBounds<Pos>>(&self, r: R) -> PosIter<'_, V> {
        match self {
            PosMap::Zero => PosIter::One(None),
            PosMap::One(q, v) => PosIter::One(r.contains(q).then_some((q, v))),
            PosMap::Many(m) => {
                let lo = match r.start_bound() {
                    Included(p) => m.partition_point(|x| x.0 < *p),
                    Excluded(p) => m.partition_point(|x| x.0 <= *p),
                    Unbounded => 0,
                };
                let hi = match r.end_bound() {
                    Included(p) => m.partition_point(|x| x.0 <= *p),
                    Excluded(p) => m.partition_point(|x| x.0 < *p),
                    Unbounded => m.len(),
                };
                PosIter::Run(m[lo..hi.max(lo)].iter())
            }
        }
    }

    pub(crate) fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    /// The entries at and after `at`, taken out.
    pub(crate) fn split_off(&mut self, at: &Pos) -> PosMap<V> {
        match self {
            PosMap::Zero => PosMap::Zero,
            PosMap::One(q, _) if *q < *at => PosMap::Zero,
            PosMap::One(..) => std::mem::take(self),
            PosMap::Many(m) => {
                let i = m.partition_point(|x| x.0 < *at);
                PosMap::Many(m.split_off(i))
            }
        }
    }

    /// Labels moved in order (a relabel).
    fn rekey(&mut self, map: &FxHashMap<Pos, Pos>) {
        match self {
            PosMap::Zero => {}
            PosMap::One(q, _) => *q = map.get(q).copied().unwrap_or(*q),
            PosMap::Many(m) => {
                for x in m.iter_mut() {
                    x.0 = map.get(&x.0).copied().unwrap_or(x.0);
                }
            }
        }
    }

    /// Moves its entries into `run`, which has room, and becomes it; the
    /// old run is given back to be freed after the lock.
    fn regrow(&mut self, mut run: Vec<(Pos, V)>) -> Vec<(Pos, V)> {
        match std::mem::take(self) {
            PosMap::Zero => {
                *self = PosMap::Zero;
                run
            }
            PosMap::One(q, v) => {
                run.push((q, v));
                *self = PosMap::Many(run);
                Vec::new()
            }
            PosMap::Many(mut old) => {
                run.append(&mut old);
                *self = PosMap::Many(run);
                old
            }
        }
    }
}

/// A vector of `n` slots' room, written once so that its pages are
/// touched: filling it under a lock faults nothing in.
pub(crate) fn touched<T: Clone>(n: usize, x: T) -> Vec<T> {
    let mut v = vec![x; n];
    v.clear();
    v
}

/// A map with room for `n` entries, its slots touched as `touched`'s.
fn touched_map<V: Default>(n: usize) -> FxHashMap<Field, V> {
    let mut m = FxHashMap::default();
    m.reserve(n);
    for i in 0..n as u64 {
        m.insert(Field(ValueId::from_raw(i)), V::default());
    }
    m.clear();
    m
}

/// What a commit would grow under its segment's lock, found under it and
/// made outside it (`Grow::make`); `Made::install` then only moves.
#[derive(Default)]
pub(crate) struct Grow {
    defs: usize,
    contribs: usize,
    readers: usize,
    def_runs: Vec<(Field, usize)>,
    contrib_runs: Vec<(Field, usize)>,
    reader_lists: Vec<(Field, usize)>,
}

#[derive(Default)]
pub(crate) struct Made {
    defs: Option<FxHashMap<Field, PosMap<Def>>>,
    contribs: Option<FxHashMap<Field, PosMap<ValueId>>>,
    readers: Option<FxHashMap<Field, Vec<Reader>>>,
    def_runs: Vec<(Field, Vec<(Pos, Def)>)>,
    contrib_runs: Vec<(Field, Vec<(Pos, ValueId)>)>,
    reader_lists: Vec<(Field, Vec<Reader>)>,
}

/// Room a map needs for `k` new keys, as its new size; 0 when it has it.
fn map_room<V>(m: &FxHashMap<Field, V>, k: usize) -> usize {
    if k == 0 || m.capacity() - m.len() >= k {
        0
    } else {
        ((m.len() + k) * 2).max(16)
    }
}

const DUMMY_DEF: Def = Def {

    depth: 0, // (scopes: `commit::def_depth`, the reader's depth)
    value: ValueId::UNIT,
    by: NodeId(0),
    rel: None,
    provisional: false,
};

const DUMMY_READER: Reader = Reader {

    depth: 0, // (scopes: `commit::def_depth`, the reader's depth)
    node: NodeId(0),
    serial: 0,
    anchor: NodeId(0),
    inclusive: false,
    path: PathId::WHOLE,
    seen: None,
};

impl Grow {
    pub(crate) fn is_empty(&self) -> bool {
        self.defs == 0
            && self.contribs == 0
            && self.readers == 0
            && self.def_runs.is_empty()
            && self.contrib_runs.is_empty()
            && self.reader_lists.is_empty()
    }

    /// Makes what it names, outside any lock.
    pub(crate) fn make(&self) -> Made {
        Made {
            defs: (self.defs > 0).then(|| touched_map(self.defs)),
            contribs: (self.contribs > 0).then(|| touched_map(self.contribs)),
            readers: (self.readers > 0).then(|| touched_map(self.readers)),
            def_runs: self
                .def_runs
                .iter()
                .map(|&(f, n)| (f, touched(n, (Pos(0), DUMMY_DEF))))
                .collect(),
            contrib_runs: self
                .contrib_runs
                .iter()
                .map(|&(f, n)| (f, touched(n, (Pos(0), ValueId::UNIT))))
                .collect(),
            reader_lists: self
                .reader_lists
                .iter()
                .map(|&(f, n)| (f, touched(n, DUMMY_READER)))
                .collect(),
        }
    }
}

impl Made {
    /// Moves the segment's containers into the bigger ones made; what they
    /// replaced is kept here, to be freed after the lock.
    pub(crate) fn install(&mut self, inner: &mut Inner) {
        if let Some(mut m) = self.defs.take()
            && m.capacity() > inner.defs.capacity()
        {
            m.extend(inner.defs.drain());
            std::mem::swap(&mut inner.defs, &mut m);
            self.defs = Some(m);
        }
        if let Some(mut m) = self.contribs.take()
            && m.capacity() > inner.contribs.capacity()
        {
            m.extend(inner.contribs.drain());
            std::mem::swap(&mut inner.contribs, &mut m);
            self.contribs = Some(m);
        }
        if let Some(mut m) = self.readers.take()
            && m.capacity() > inner.readers.capacity()
        {
            m.extend(inner.readers.drain());
            std::mem::swap(&mut inner.readers, &mut m);
            self.readers = Some(m);
        }
        for (f, run) in &mut self.def_runs {
            if let Some(pm) = inner.defs.get_mut(f)
                && run.capacity() > pm.len()
            {
                *run = pm.regrow(std::mem::take(run));
            }
        }
        for (f, run) in &mut self.contrib_runs {
            if let Some(pm) = inner.contribs.get_mut(f)
                && run.capacity() > pm.len()
            {
                *run = pm.regrow(std::mem::take(run));
            }
        }
        for (f, list) in &mut self.reader_lists {
            match inner.readers.get_mut(f) {
                Some(v) => {
                    if list.capacity() > v.len() {
                        list.append(v);
                        std::mem::swap(v, list);
                    }
                }
                None => {
                    inner.readers.insert(*f, std::mem::take(list));
                }
            }
        }
    }
}

impl Inner {
    /// What filing these definitions and contributions at `pos`, and
    /// readers of these fields (each so many times), would grow.
    pub(crate) fn grow_for(
        &self,
        pos: Pos,
        defs: impl Iterator<Item = Field>,
        contribs: impl Iterator<Item = Field>,
        reads: &[(Field, usize)],
    ) -> Grow {
        let mut g = Grow::default();
        let mut new = 0;
        for f in defs {
            match self.defs.get(&f) {
                None => new += 1,
                Some(pm) => {
                    if pm.full_for(pos) {
                        g.def_runs.push((f, (pm.len() * 2).max(4)));
                    }
                }
            }
        }
        g.defs = map_room(&self.defs, new);
        let mut new = 0;
        for f in contribs {
            match self.contribs.get(&f) {
                None => new += 1,
                Some(pm) => {
                    if pm.full_for(pos) {
                        g.contrib_runs.push((f, (pm.len() * 2).max(4)));
                    }
                }
            }
        }
        g.contribs = map_room(&self.contribs, new);
        let mut new = 0;
        for &(f, k) in reads {
            match self.readers.get(&f) {
                None => {
                    new += 1;
                    g.reader_lists.push((f, k.max(4)));
                }
                Some(v) => {
                    if v.capacity() - v.len() < k {
                        g.reader_lists.push((f, (v.len() * 2).max(v.len() + k)));
                    }
                }
            }
        }
        g.readers = map_room(&self.readers, new);
        g
    }
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
    /// Its scope depth (`Locality`); 0 for a global. It reaches a later
    /// position only while the depth between stays at or above it
    /// (`read::visible`).
    pub(crate) depth: u16,
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
    /// The depth the read was answered at: the anchor's, after the
    /// closes that come before it. A change of brackets between a
    /// definition and this reader re-asks it (`commit::reask_after_effect`).
    pub(crate) depth: u16,
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
    /// A fold up to and with `to`'s own.
    pub(crate) incl: bool,
    /// A `next` stopped at a node not yet committed (`to`).
    pub(crate) hole: bool,
    /// Each path read through the range and what it gave.
    pub(crate) seen: Arc<[(PathId, Option<ValueId>)]>,
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
    /// (`read::segment_part`); an empty field is dropped.
    pub(crate) fn resum(&mut self, _eng: &Engine, f: Field) {
        self.sums.remove(&f);
        self.sums_gen += 1;
        if self.contribs.get(&f).is_some_and(PosMap::is_empty) {
            self.contribs.remove(&f);
        }
    }
}

impl Segment {
    pub(crate) fn new(id: SegId, unit: UnitId, lo: Pos, hi: Pos) -> Segment {
        Segment {
            id,
            unit,
            lo,
            inner: Lock::new(Inner {
                brackets: PosMap::Zero,
                depth_net: 0,
                depth_min: 0,
                hi,
                nodes: BTreeMap::new(),
                defs: FxHashMap::default(),
                readers: FxHashMap::default(),
                contribs: FxHashMap::default(),
                sums: FxHashMap::default(),
                sums_gen: 0,
                stops: Vec::new(),
                by_op: FxHashMap::default(),
                holes: 0,
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
        let next = inner.nodes.range((Excluded(after), Unbounded)).next().map(|(p, _)| *p);
        // (after the last node a push's step, not half the rest: nodes
        // placed at a segment's end one after another, as a chain of
        // steps is, would halve its labels at each, and each split of it
        // halve them again)
        let (next, step) = match next {
            Some(q) => (q, (q.0 - after.0) / 2),
            None => (inner.hi, ((inner.hi.0 - after.0) / 2).min(crate::order::GAP)),
        };
        (next.0 - after.0 >= 2 && step >= 1).then(|| Pos(after.0 + step))
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
            m.rekey(&map);
        }
        for m in inner.contribs.values_mut() {
            m.rekey(&map);
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
    ///
    /// The lower half is relabelled first a push's step apart from `lo`
    /// (or closer, to fit before the median) and the new segment starts
    /// right after it: nodes placed at one spot, a chain of steps before
    /// the unit's last nodes, fill the upper half, which keeps the rest
    /// of the labels instead of half of them at every split.
    pub(crate) fn split(&self, inner: &mut Inner, eng: &Engine, new_id: SegId) -> Arc<Segment> {
        let k = inner.nodes.len() / 2;
        let mid = *inner.nodes.keys().nth(k).expect("a segment to split has nodes");
        let step = ((mid.0 - self.lo.0) / (k as u64 + 1)).min(crate::order::GAP);
        if step < 2 {
            return self.split_at(inner, eng, mid, new_id);
        }
        let lower: Vec<(Pos, NodeId)> = inner.nodes.range(..mid).map(|(p, n)| (*p, *n)).collect();
        let mut map: FxHashMap<Pos, Pos> = FxHashMap::default();
        for (i, (p, id)) in lower.iter().enumerate() {
            let np = Pos(self.lo.0 + (i as u64 + 1) * step);
            if np != *p {
                map.insert(*p, np);
                inner.nodes.remove(p);
                eng.node(*id).pos.store(np.0, Ordering::Release);
            }
        }
        for (p, id) in &lower {
            if let Some(&np) = map.get(p) {
                inner.nodes.insert(np, *id);
            }
        }
        if !map.is_empty() {
            for m in inner.defs.values_mut() {
                m.rekey(&map);
            }
            for m in inner.contribs.values_mut() {
                m.rekey(&map);
            }
        }
        self.split_at(inner, eng, Pos(self.lo.0 + (k as u64 + 1) * step), new_id)
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
        let (stops, down): (Vec<_>, Vec<_>) = std::mem::take(&mut inner.stops)
            .into_iter()
            .partition(|s: &(Field, RangeKey, RangeReader)| s.2.to.is_some_and(|n| eng.node(n).pos_hint() >= at));
        inner.stops = down;
        let mut by_op: FxHashMap<OpId, Vec<NodeId>> = FxHashMap::default();
        for (op, v) in &mut inner.by_op {
            let (up, down): (Vec<NodeId>, Vec<NodeId>) = v.drain(..).partition(|n| eng.node(*n).pos_hint() >= at);
            *v = down;
            if !up.is_empty() {
                by_op.insert(*op, up);
            }
        }
        inner.by_op.retain(|_, v| !v.is_empty());
        let up_holes = nodes.values().filter(|n| eng.node(**n).is_hole()).count();
        inner.holes = inner.nodes.values().filter(|n| eng.node(**n).is_hole()).count();
        let new = Arc::new(Segment {
            id: new_id,
            unit: self.unit,
            lo: at,
            inner: Lock::new(Inner {
                brackets: PosMap::Zero,
                depth_net: 0,
                depth_min: 0,
                hi,
                nodes,
                defs,
                readers,
                contribs,
                sums: FxHashMap::default(),
                sums_gen: 0,
                stops,
                by_op,
                holes: up_holes,
            }),
        });
        let moved: Vec<Field> = {
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
            ni.defs.keys().chain(ni.contribs.keys()).copied().collect()
        };
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
        // (counted again once indexed and ordered: a read that took this
        // segment for them before the split checks again)
        for f in moved {
            eng.bump(f);
        }
        new
    }
}

impl Inner {
    /// Recomputes `depth_net` and `depth_min` from `brackets`, in position
    /// order: for each node, its closes (before or after its definitions
    /// alike for the summary) then its opens.
    pub(crate) fn sum_depth(&mut self) {
        todo!()
    }
}
