//! A run's commit: into the node's own segment, then the walk forward.
//!
//! 0. Before the lock: the record, the placements' nodes and predictions,
//!    the scratch lists. Under it, room the filing would grow (a map, a run
//!    of definitions, a list of readers) is found; it is made outside and
//!    the lock taken again (`segment::Grow`): nothing is allocated under it.
//! 1. Under the node's segment's write lock: diff the product's definitions
//!    and contributions against the node's last record, file the changes,
//!    match placements by (op, args) to keep unchanged children, place new
//!    ones in the gap (relabel or split if closed), record the reads
//!    anchored here, walk forward as far as the segment goes (`walk_here`),
//!    answer the run's own reads that the segment answers, bump the serial,
//!    swap the record. Release.
//! 2. Reads anchored in other segments: each under that segment's lock, one
//!    at a time, never nested.
//! 3. The readers step 1 found changed are marked; each walk goes on past
//!    the segment (`reask`). For each changed contribution: its range
//!    readers. A first commit walks on the walks that stopped at it
//!    (`first_hole`).
//! 4. Removed placements: `unfile`, recursively, each under its own segment.
//! 5. Its relations from their input again (`settle_rel`); a relation
//!    defined for the first time by this operation: `learn_shape`.
//! 6. The node's own reads step 1 could not answer, answered: a walk that
//!    passed before they were filed is caught here, and the node runs again;
//!    one that passes after finds the node `committing` and marks it.
//!
//! No other segment is written in step 1. No lock is held in step 3 except
//! one segment's read lock at a time, and a short write lock to recompute a
//! relation met on the way.

use std::collections::VecDeque;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::api::{Brackets, Cause, Effect, Locality, NodeId, OpId, PathId, UnitId};
use crate::engine::Engine;
use crate::index::RangeKey;
use crate::lock::ns;
use crate::node::{
    Anchor, DefRec, Node, Placement, Product, ReadKind, RunRecord, State, Wake, pack_effect, unpack_effect,
};
use crate::order::{Pos, SegId};
use crate::read::{self, Reaching, next_contribution, reaching_at, reader_answer, through};
use crate::sched::{Worker, allow_final_rerun, enqueue, mark_dirty, mark_stale};
use crate::segment::{Def, GLOBAL, Inner, Made, PosMap, RangeReader, Reader, Segment, live, push_pruned};
use crate::values::{Field, FxHashMap, FxHashSet, ValueId, Values};

/// A range read's field, ends, and whether a `next` or with `to`'s own.
type RangeOf = (Field, Option<NodeId>, Option<NodeId>, bool, bool);

/// Counts for the report.
#[derive(Default, Clone, Copy)]
pub(crate) struct CommitStats {
    pub(crate) woken: u64,
    /// The reader records the definitions' walks looked at (`walk_here`,
    /// `reask_from`: each segment's readers of the field, those it passed
    /// by included).
    pub(crate) walked: u64,
    /// The readers a change of brackets asked again (`ScopeReach`).
    pub(crate) scope_asked: u64,
    pub(crate) relative_resolved: u64,
    pub(crate) predictions_confirmed: u64,
    pub(crate) shaped: u64,
    pub(crate) commit_ns: u64,
    /// (`profile`) `commit_ns` on the thread's CPU (`lock::thread_cpu_ns`).
    pub(crate) commit_cpu_ns: u64,
    /// (`profile`, `obs::CountingAlloc`) What the commit allocated and
    /// freed under its segment's lock.
    pub(crate) commit_allocs: u64,
    pub(crate) commit_frees: u64,
    pub(crate) reask_ns: u64,
}

impl CommitStats {
    pub(crate) fn add(&mut self, o: &CommitStats) {
        self.woken += o.woken;
        self.walked += o.walked;
        self.scope_asked += o.scope_asked;
        self.relative_resolved += o.relative_resolved;
        self.predictions_confirmed += o.predictions_confirmed;
        self.shaped += o.shaped;
        self.commit_ns += o.commit_ns;
        self.reask_ns += o.reask_ns;
    }
}

/// The node's segment, write-locked, with the node still in it; None when
/// the node is gone.
pub(crate) fn own_write<'a>(
    eng: &'a Engine,
    node: &Node,
) -> Option<(&'a Arc<Segment>, crate::lock::WriteGuard<'a, Inner>)> {
    loop {
        if node.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(node.seg_hint());
        let inner = seg.inner.write();
        if node.seg_hint() == seg.id && inner.nodes.get(&node.pos_hint()) == Some(&node.id) {
            return Some((seg, inner));
        }
        if node.state() == State::Gone {
            return None;
        }
    }
}

/// The last node of `n`'s subtree: `n`, or the furthest of its placements'.
/// A list of the nodes still to visit, not a frame each: a continuation
/// chain is a subtree as deep as it is long.
pub(crate) fn subtree_end(eng: &Engine, n: NodeId) -> NodeId {
    subtree_end_skip(eng, n, None)
}

thread_local! {
    /// `subtree_end`'s list of nodes to visit, kept by the thread: walked
    /// under a commit's lock, where nothing is allocated (room for
    /// `TODO_ROOM` made before it, `todo_room`; grown past that only by a
    /// subtree wider than any the thread met).
    static TODO: std::cell::Cell<Vec<NodeId>> = const { std::cell::Cell::new(Vec::new()) };
}

const TODO_ROOM: usize = 256;

/// `TODO`'s room, before a commit's lock.
fn todo_room() {
    let mut v = TODO.take();
    v.reserve(TODO_ROOM.saturating_sub(v.len()));
    TODO.set(v);
}

/// `subtree_end` passing over the nodes of `skip` and theirs: a node
/// being relocated and its subtree (`relocate`).
fn subtree_end_skip(eng: &Engine, n: NodeId, skip: Option<&FxHashSet<NodeId>>) -> NodeId {
    let (mut best, mut best_pos) = (n, eng.node(n).pos_hint());
    let mut todo = TODO.take();
    todo.clear();
    todo.push(n);
    while let Some(x) = todo.pop() {
        let Some(r) = eng.node(x).run_record() else {
            continue;
        };
        for &c in &r.placed {
            // (one not on the line yet goes after this end)
            let Some(cn) = eng.placed_node(c) else {
                continue;
            };
            if cn.state() == State::Gone || skip.is_some_and(|k| k.contains(&c)) {
                continue;
            }
            if cn.pos_hint() > best_pos {
                (best, best_pos) = (c, cn.pos_hint());
            }
            todo.push(c);
        }
    }
    TODO.set(todo);
    best
}

/// Where a node placed after `n` and its children goes (`follow_after`):
/// after the last node of its children's subtrees (or `n`), and before
/// its first continuation (`Node::follows`), if it has one: the
/// position it must stand before.
/// The slot `y` stands in under `a`: the slot of the node of `y`'s
/// anchor chain whose anchor is `a` (`y` itself, or what it was placed
/// after, and so on); none where the chain ends before `a` (a pushed
/// node, or one anchored elsewhere).
fn slot_under(eng: &Engine, y: NodeId, a: NodeId) -> Option<u8> {
    sib_under(eng, y, a).map(|z| eng.node(z).slot())
}

/// The node of `y`'s anchor chain whose anchor is `a` (`slot_under`'s).
fn sib_under(eng: &Engine, y: NodeId, a: NodeId) -> Option<NodeId> {
    let mut z = y;
    for _ in 0..100_000 {
        let zn = eng.placed_node(z)?;
        let an = zn.anchor()?;
        if an == a {
            return Some(z);
        }
        z = an;
    }
    None
}

/// Where a node of rank `rank` placed after `a` and its children goes,
/// from `from` (the children's end) in this segment: past the nodes
/// placed after `a` (slot 1) by a lower rank, and what stands under
/// them (`Ctx::rank_placement`); `from` for an unranked one. The walk
/// stops at a node of `skip` (the node relocated and its subtree, which
/// it must stand before).
fn ranked_end(
    eng: &Engine,
    inner: &Inner,
    a: NodeId,
    from: NodeId,
    rank: u64,
    skip: Option<&FxHashSet<NodeId>>,
) -> NodeId {
    let mut last = from;
    if rank == 0 {
        return last;
    }
    for (_, &x) in inner.nodes.range((Excluded(eng.node(from).pos_hint()), Unbounded)) {
        if skip.is_some_and(|k| k.contains(&x)) {
            break;
        }
        let Some(xn) = eng.placed_node(x) else {
            continue;
        };
        if xn.state() == State::Gone {
            continue;
        }
        let Some(d) = sib_under(eng, x, a) else {
            break;
        };
        let dn = eng.node(d);
        if dn.slot() != 1 || dn.rank() == 0 || dn.rank() >= rank {
            break;
        }
        last = x;
    }
    last
}

/// Whether `x` is a placement of `by`'s, or of one of its placements.
fn own_of(eng: &Engine, x: NodeId, by: NodeId) -> bool {
    let mut q = Some(x);
    while let Some(y) = q {
        if y == by {
            return true;
        }
        q = eng.placed_node(y).and_then(|n| n.parent);
    }
    false
}

/// Whether `x` standing between a placement's floor and it strands it:
/// a node of a later slot under `a` than `s`, or of none (pushed, or
/// anchored elsewhere), not `by`'s own.
fn strands(eng: &Engine, x: NodeId, by: NodeId, a: NodeId, s: u8, rank: u64) -> bool {
    // (one not in the table yet: made by this commit, its own)
    let Some(xn) = eng.placed_node(x) else {
        return false;
    };
    if xn.state() == State::Gone || x == a {
        return false;
    }
    // (a placement of `by`'s own, kept or replaced: where the replaced
    // one stands, the one made anew goes)
    if own_of(eng, x, by) {
        return false;
    }
    let t = slot_under(eng, x, a);
    // (and, of a ranked one after `a`, one placed after `a` by a higher
    // rank: `Ctx::rank_placement`)
    let bad = !matches!(t, Some(t) if t <= s)
        || (s == 1 && rank > 0 && t == Some(1) && sib_under(eng, x, a).is_some_and(|d| eng.node(d).rank() > rank));
    if bad && crate::obs::enabled() && crate::sched::wake_log() {
        eprintln!(
            "stranded: anchor {} slot {s}: node {} (op {:?}, anchor {:?} slot {}, under {t:?}) between",
            a.raw(),
            x.raw(),
            xn.op(),
            xn.anchor().map(NodeId::raw),
            xn.slot()
        );
    }
    bad
}

/// The nodes not `by`'s own standing between `lo` and `hi`, in every
/// segment the range crosses, each read-locked in turn before the
/// commit's lock: what the check of a kept placement (`stranded`) cannot
/// see under its lock, where the placement or its floor stands in
/// another segment than its placer.
fn foreign_between(eng: &Engine, by: NodeId, lo: Pos, hi: Pos) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut cur = eng.seg_for_pos(lo).map(|s| s.id);
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        if seg.lo >= hi {
            break;
        }
        for (_, &x) in inner.nodes.range((Excluded(lo), Excluded(hi))) {
            if !own_of(eng, x, by) {
                out.push(x);
            }
        }
        let end = inner.hi;
        drop(inner);
        if end >= hi {
            break;
        }
        cur = eng.order.next(sid, seg.lo);
    }
    out
}

/// Whether a placement of `by`'s kept at `kpos` in slot `s` of anchor
/// `a` is stranded: a node of a later slot under `a`, or of none
/// (pushed, or anchored elsewhere), stands between `floor` (the slot's
/// start) and it: in this segment, under its lock, or among `others`
/// (`foreign_between`, found before the lock), wherever they stand now.
/// A kept placement stands where its slot is; when its anchor's earlier
/// slots moved back and nodes of later slots came between, it is made
/// anew where it belongs.
#[allow(clippy::too_many_arguments)]
fn stranded(
    eng: &Engine,
    inner: &Inner,
    others: &[NodeId],
    by: NodeId,
    a: NodeId,
    s: u8,
    floor: Pos,
    kpos: Pos,
    rank: u64,
) -> bool {
    if kpos <= floor {
        return false;
    }
    inner
        .nodes
        .range((Excluded(floor), Excluded(kpos)))
        .any(|(_, &x)| strands(eng, x, by, a, s, rank))
        || others.iter().any(|&x| {
            let p = eng.node(x).pos_hint();
            p > floor && p < kpos && strands(eng, x, by, a, s, rank)
        })
}

/// The last node in this segment after `from` that stands under `a`
/// (its anchor chain reaches `a`): a continuation of `a`'s goes after
/// them all, as what others placed after `a` and its children, and what
/// follows those, comes before it. `from` where none does.
fn attached_end(eng: &Engine, inner: &Inner, a: NodeId, from: NodeId) -> NodeId {
    attached_end_skip(eng, inner, a, from, None)
}

/// `attached_end` passing over the nodes of `skip` (`relocate`).
fn attached_end_skip(eng: &Engine, inner: &Inner, a: NodeId, from: NodeId, skip: Option<&FxHashSet<NodeId>>) -> NodeId {
    let mut last = from;
    for (_, &x) in inner.nodes.range((Excluded(eng.node(from).pos_hint()), Unbounded)) {
        let Some(xn) = eng.placed_node(x) else {
            continue;
        };
        if xn.state() == State::Gone || skip.is_some_and(|k| k.contains(&x)) {
            continue;
        }
        if slot_under(eng, x, a).is_none() {
            break;
        }
        last = x;
    }
    last
}

/// `xs` in the order of their labels as each was read once: another
/// segment's rebalance (`Order::rebalance`) may move a label while the
/// sort runs, and a key read again under it, changed, broke the total
/// order the sort relies on (a panic at the page loop's commit in Grail
/// at sixteen threads, its rests relocated). A snapshot is what every
/// caller wants: an order to place or run nodes in, each of which
/// re-reads its own place under its segment's lock.
pub(crate) fn sort_by_label(eng: &Engine, xs: &mut [NodeId]) {
    xs.sort_by_cached_key(|&n| eng.node(n).pos_hint());
}

pub(crate) fn children_end(eng: &Engine, n: NodeId) -> (NodeId, Option<Pos>) {
    children_end_skip(eng, n, None)
}

/// `children_end` passing over the nodes of `skip` (`relocate`).
fn children_end_skip(eng: &Engine, n: NodeId, skip: Option<&FxHashSet<NodeId>>) -> (NodeId, Option<Pos>) {
    let (mut best, mut best_pos) = (n, eng.node(n).pos_hint());
    let mut first_follow: Option<Pos> = None;
    let Some(r) = eng.node(n).run_record() else {
        return (best, None);
    };
    for &c in &r.placed {
        let Some(cn) = eng.placed_node(c) else {
            continue;
        };
        if cn.state() == State::Gone || skip.is_some_and(|k| k.contains(&c)) {
            continue;
        }
        if cn.follows.load(Ordering::Acquire) {
            first_follow = Some(first_follow.map_or(cn.pos_hint(), |p| p.min(cn.pos_hint())));
            continue;
        }
        let e = subtree_end_skip(eng, c, skip);
        let ep = eng.node(e).pos_hint();
        if ep > best_pos {
            (best, best_pos) = (e, ep);
        }
    }
    (best, first_follow)
}

/// One contribution per field: several to the same field combined.
pub(crate) fn combine_dups(eng: &Engine, xs: Vec<(Field, ValueId)>) -> Vec<(Field, ValueId)> {
    let mut out: Vec<(Field, ValueId)> = Vec::with_capacity(xs.len());
    for (f, x) in xs {
        match out.iter_mut().find(|(g, _)| *g == f) {
            Some((_, acc)) => {
                let m = eng.monoid(f);
                *acc = m.combine(eng.values(), *acc, x);
            }
            None => out.push((f, x)),
        }
    }
    out
}

/// The last definition of each field at each locality wins.
pub(crate) fn dedup_defs(defs: Vec<DefRec>) -> Vec<DefRec> {
    let mut seen = FxHashSet::default();
    let mut out: Vec<DefRec> = Vec::with_capacity(defs.len());
    for d in defs.into_iter().rev() {
        if seen.insert((d.f, d.loc)) {
            out.push(d);
        }
    }
    out.reverse();
    out
}

/// A run's definition as filed: its depth (`def_depth`), and for a field
/// it defines at several depths, the one multi definition (`Def::multi`).
struct Filed {
    f: Field,
    depth: u16,
    value: ValueId,
    rel: Option<i64>,
    multi: bool,
}

/// `defs` (deduplicated) at their depths under `b`, one per field: the
/// last at each depth, outermost first in a multi value (a global one
/// outermost of all); a relation is never one of several.
fn filed_defs(v: &Values, defs: &[DefRec], b: &Brackets) -> Vec<Filed> {
    let mut at: FxHashMap<Field, usize> = FxHashMap::default();
    type Parts = (Field, Option<i64>, Vec<(u16, ValueId)>);
    let mut ps: Vec<Parts> = Vec::with_capacity(defs.len());
    for d in defs {
        let depth = def_depth(d.loc, b);
        let i = *at.entry(d.f).or_insert_with(|| {
            ps.push((d.f, None, Vec::new()));
            ps.len() - 1
        });
        let e = &mut ps[i];
        if e.1.is_some() || d.rel.is_some() {
            e.2.clear();
        }
        e.1 = d.rel;
        e.2.retain(|x| x.0 != depth);
        e.2.push((depth, d.value));
    }
    let outer = |d: u16| if d == GLOBAL { 0 } else { u32::from(d) + 1 };
    ps.into_iter()
        .map(|(f, rel, mut xs)| {
            if let [(depth, value)] = xs[..] {
                return Filed {
                    f,
                    depth,
                    value,
                    rel,
                    multi: false,
                };
            }
            xs.sort_by_key(|x| outer(x.0));
            Filed {
                f,
                depth: xs[0].0,
                value: v.list(xs.iter().flat_map(|&(d, x)| [v.int(i64::from(d)), x]).collect()),
                rel,
                multi: true,
            }
        })
        .collect()
}

/// Files a reader record in a segment it is anchored in; true for the
/// segment's first reader of `f` (the caller adds it to the index once the
/// lock is released). `spare` is a list made before the lock, used for a
/// field's first.
fn file_reader(eng: &Engine, inner: &mut Inner, f: Field, r: Reader, spare: &mut Vec<Reader>) -> bool {
    if r.anchor != r.node {
        let mut a = eng.node(r.anchor).anchored.lock();
        if !a.contains(&f) {
            a.push(f);
        }
    }
    let v = inner.readers.entry(f).or_insert_with(|| std::mem::take(spare));
    let first = v.is_empty();
    push_pruned(v, r, |x| live(eng, x.node, x.serial));
    first
}

/// Index changes made under a segment's lock, applied before it is let go
/// (a field-index shard under it, as a split takes one): applied after
/// it, one commit's removal of a segment's last definition of a field
/// could land after another's addition of a first one, and the index lose
/// a segment that holds the field, every read passing over it for good
/// (amsldoc at -j8: a section's counter read past the section before it).
#[derive(Default)]
struct IndexOps {
    defs_add: Vec<Field>,
    defs_remove: Vec<Field>,
    contribs_add: Vec<Field>,
    contribs_remove: Vec<Field>,
    readers_add: Vec<Field>,
}

impl IndexOps {
    fn with_capacity(defs: usize, contribs: usize, reads: usize) -> IndexOps {
        IndexOps {
            defs_add: Vec::with_capacity(defs),
            defs_remove: Vec::with_capacity(defs),
            contribs_add: Vec::with_capacity(contribs),
            contribs_remove: Vec::with_capacity(contribs),
            readers_add: Vec::with_capacity(reads),
        }
    }

    /// Applied under the segment's lock, its removals checked against
    /// what the segment holds now (a placement this commit made may have
    /// filed a prediction of a field the commit's own run stopped
    /// defining).
    /// Its lists are emptied, not freed: that is after the lock.
    fn apply(&mut self, eng: &Engine, seg: &Segment, inner: &Inner) {
        for f in self.defs_add.drain(..) {
            eng.defs.add(f, seg.lo, seg.id);
            eng.bump(f);
        }
        for f in self.contribs_add.drain(..) {
            eng.contribs.add(f, seg.lo, seg.id);
            eng.bump(f);
        }
        for f in self.readers_add.drain(..) {
            eng.readers.add(f, seg.lo, seg.id);
        }
        for f in self.defs_remove.drain(..) {
            if inner.defs.get(&f).is_none_or(PosMap::is_empty) {
                eng.defs.remove(f, seg.lo, seg.id);
            }
            eng.bump(f);
        }
        for f in self.contribs_remove.drain(..) {
            if inner.contribs.get(&f).is_none_or(PosMap::is_empty) {
                eng.contribs.remove(f, seg.lo, seg.id);
            }
            eng.bump(f);
        }
    }
}

/// The labels a node's label stays within for its life: its segment's
/// range now (a relabel stays in it, a split narrows it); None when gone.
fn node_span(eng: &Engine, n: NodeId) -> Option<(Pos, Pos)> {
    let node = eng.node(n);
    loop {
        if node.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(node.seg_hint());
        let inner = seg.inner.read();
        if inner.nodes.get(&node.pos_hint()) == Some(&n) {
            return Some((seg.lo, inner.hi));
        }
        if node.state() == State::Gone {
            return None;
        }
    }
}

/// The labels a range reader's range stays within (`index::RangeIndex`).
fn range_span(eng: &Engine, r: &RangeReader) -> (Pos, Pos) {
    let me = eng.node(r.node);
    let lo = match r.from {
        Some(n) => node_span(eng, n).map_or(Pos(0), |s| s.0),
        None => read::unit_start(eng, me),
    };
    let hi = match r.to {
        Some(n) => node_span(eng, n).map_or(Pos(u64::MAX), |s| s.1),
        None if r.next => Pos(u64::MAX),
        None => node_span(eng, r.node).map_or(Pos(u64::MAX), |s| s.1),
    };
    (lo, hi)
}

/// Files range readers in their fields' interval lists (one lock a field),
/// and each `next` stopped at a node not yet committed at that node; one
/// whose stop has committed meanwhile is re-asked at once.
fn file_ranges(eng: &Engine, w: Option<&Worker>, mut rs: Vec<(Field, RangeReader)>, by: NodeId, st: &mut CommitStats) {
    rs.sort_by_key(|x| x.0);
    let mut layout = eng.layout_gen.load(Ordering::Acquire);
    let mut again: Vec<(Field, RangeKey, RangeReader)> = Vec::new();
    let mut i = 0;
    while i < rs.len() {
        let f = rs[i].0;
        let j = i + rs[i..].iter().take_while(|x| x.0 == f).count();
        let items: Vec<(Pos, Pos, RangeReader)> = rs[i..j]
            .iter()
            .map(|(_, r)| {
                let (lo, hi) = range_span(eng, r);
                (lo, hi, r.clone())
            })
            .collect();
        let keys = eng.ranges.insert(f, items, |r| live(eng, r.node, r.serial));
        // (a rebalance since the spans were found may have moved a node
        // out of them: filed again over their spans now)
        while eng.layout_gen.load(Ordering::Acquire) != layout {
            layout = eng.layout_gen.load(Ordering::Acquire);
            for (k, (_, r)) in keys.iter().zip(&rs[i..j]) {
                let (lo, hi) = range_span(eng, r);
                eng.ranges.refile(f, *k, lo, hi);
            }
        }
        for (k, (_, r)) in keys.into_iter().zip(&rs[i..j]) {
            if !file_stop(eng, f, k, r) || !file_bounds(eng, f, k, r) {
                again.push((f, k, r.clone()));
            }
        }
        i = j;
    }
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in again {
        reask_range(eng, w, f, k, &r, by, false, st, &mut memo);
    }
}

/// Files a `next` reader whose search stopped at a node not yet committed
/// at that node (`reask_holes`); false when the node has committed or gone
/// since: the stop is stale.
fn file_stop(eng: &Engine, f: Field, k: RangeKey, r: &RangeReader) -> bool {
    let (true, Some(stop)) = (r.hole, r.to) else {
        return true;
    };
    let n = eng.node(stop);
    // (a running node may be past its commit's take of its stops and still
    // open, a hole while it commits (`Node::is_open`): marked before it is
    // looked at, so either the end of its run (`reask_late_stops`) sees the
    // mark or this sees the node no longer open)
    if matches!(n.state(), State::Running | State::RunningDirty | State::RunningInert) {
        n.late_stops.store(true, Ordering::SeqCst);
        std::sync::atomic::fence(Ordering::SeqCst);
    }
    let Some((_, mut inner)) = own_write(eng, n) else {
        return false;
    };
    if !n.is_open() {
        return false;
    }
    push_pruned(&mut inner.stops, (f, k, r.clone()), |x| live(eng, x.2.node, x.2.serial));
    true
}

/// Files a run that met `holes` before it (`Ctx::hole_before`) with each,
/// to be run again when it commits, is taken, is removed or moves
/// ([`wake_waiters`]); one that is no hole any more by then runs it again
/// at once. Filed before it is looked at, as `stop_at`: a node that stops
/// being a hole meanwhile has its waiters taken after.
fn file_waits(
    eng: &Engine,
    w: Option<&Worker>,
    node: &Arc<Node>,
    serial: u32,
    holes: Vec<NodeId>,
    st: &mut CommitStats,
) {
    for h in holes {
        let hn = eng.node(h);
        push_pruned(&mut hn.waiters.lock(), (node.id, serial), |x| live(eng, x.0, x.1));
        if !hn.is_hole()
            && crate::sched::mark_stale_now(
                eng,
                w,
                node,
                Wake {
                    by: Some(h),
                    cause: Cause::Structure,
                    at_ns: eng.now_ns(),
                },
            )
        {
            st.woken += 1;
        }
    }
}

/// Runs the runs filed with `node` as the hole before them
/// (`file_waits`) again: it has committed, been taken, removed or moved.
/// One with another node before it not in yet is filed with that one
/// instead, not run: its read-on waits for the whole prefix, and run at
/// each hole that fills it did its work up to the read-on again for each
/// (floats2col -j8: 85 nodes woken up to 46 times each, 793 ms of runs
/// thrown away).
fn wake_waiters(eng: &Engine, w: Option<&Worker>, node: &Node, st: &mut CommitStats) {
    let ws = std::mem::take(&mut *node.waiters.lock());
    for (id, serial) in ws {
        if !live(eng, id, serial) {
            continue;
        }
        if let Some(h) = read::on_one_layout(eng, || read::hole_before(eng, eng.node(id))).filter(|&h| h != node.id) {
            let hn = eng.node(h);
            push_pruned(&mut hn.waiters.lock(), (id, serial), |x| live(eng, x.0, x.1));
            // (filed before it is looked at: one in since takes its
            // waiters after, or is seen here)
            if hn.is_hole() {
                continue;
            }
        }
        if crate::sched::mark_stale_now(
            eng,
            w,
            eng.node(id),
            Wake {
                by: Some(node.id),
                cause: Cause::Structure,
                at_ns: eng.now_ns(),
            },
        ) {
            st.woken += 1;
        }
    }
}

/// Files a range reader with the nodes its range starts after and ends
/// at (`Node::bounded`), to be asked again when one is removed; false when
/// one is gone already.
fn file_bounds(eng: &Engine, f: Field, k: RangeKey, r: &RangeReader) -> bool {
    let mut ok = true;
    for b in [r.from, if r.next { None } else { r.to }].into_iter().flatten() {
        if b == r.node {
            continue;
        }
        let n = eng.node(b);
        push_pruned(&mut n.bounded.lock(), (f, k, r.clone()), |x| {
            live(eng, x.2.node, x.2.serial)
        });
        if n.state() == State::Gone {
            ok = false;
        }
    }
    ok
}

/// Whether a range reader's range covers `by`'s label (`at` where `by`
/// is off the line, or where `fixed`: its old label, `relocate`), on one
/// layout (`read::on_one_layout`): the labels of
/// `by` and of the range's ends taken under their segments' locks, and
/// taken again if one of those segments moved meanwhile. (Labels taken
/// one by one without a lock, a relabel between them compared an old
/// label with a new one: a contribution before a reader's end taken for
/// one after it, the reader never re-asked; linkif's \pdflastlink read
/// with no page's objects before it, issue #5.)
fn covers(eng: &Engine, r: &RangeReader, by: NodeId, at: Pos, fixed: bool) -> bool {
    let label = |n: NodeId| read::pos_of(eng, n).unwrap_or_else(|| eng.node(n).pos_hint());
    read::on_one_layout(eng, || {
        let at = if fixed { at } else { read::pos_of(eng, by).unwrap_or(at) };
        let me = eng.node(r.node);
        let from = match r.from {
            Some(n) => label(n),
            None => read::unit_start(eng, me),
        };
        if at <= from {
            return false;
        }
        match r.to {
            Some(n) => {
                let t = label(n);
                if r.next || r.incl { at <= t } else { at < t }
            }
            None => r.next || at < label(r.node),
        }
    })
}

/// The node's label under `inner` if it is there, else its last known.
fn pos_in(inner: &Inner, node: &Node) -> Pos {
    let p = node.pos_hint();
    let _ = inner.nodes.get(&p);
    p
}

/// Commits `p` as `node`'s run. Woken readers go to `w`'s deque.
#[allow(clippy::too_many_lines)]
pub(crate) fn commit(eng: &Engine, w: &Worker, node: &Arc<Node>, mut p: Product) -> CommitStats {
    let mut st = CommitStats::default();
    let t0 = Instant::now();
    let ww = Some(w);
    // (a run that published nothing new: the last record's products, its
    // placements kept as they are)
    // (one relocated since its last commit checks its placements where
    // it stands now: they are the last run's again, matched and kept or
    // moved as a run's are, `relocate`)
    let moved = node.moved.swap(false, Ordering::AcqRel);
    let keep_placed = p.unchanged && node.record().is_some() && !moved;
    // (whether its definitions reach readers outside its settling layer
    // at once: before any walk of this commit asks, `sched::settles_edge`)
    if !p.unchanged {
        node.settle_now.store(p.settle_now, Ordering::Release);
        crate::sched::clear_move(eng, node);
    }
    if let Some(r) = node.record().filter(|_| p.unchanged) {
        p.defs.clone_from(&r.defs);
        p.contribs.clone_from(&r.contribs);
        p.output = r.output;
        p.effect = Some(r.effect);
        p.claim = r.claim;
        p.claim_all = r.claim_all;
        p.placements.clear();
        if moved {
            p.ranks.clear();
            p.placements = replay_placements(eng, node.id, &r.placed, &mut p.ranks);
        }
        if p.state.is_none() && !p.at_hole {
            p.state = r.state;
        }
    }
    // (the fields a settling node defines: a reader of one past the
    // layer's frontier waits for it, `Ctx::set_frontier`)
    let settling = eng.op(node.op()).settles_at_quiet();
    if settling {
        for d in &p.defs {
            eng.mark_settled(d.f);
        }
    }
    let frontier = p.frontier;
    // (a run that went on from its last run's state rests on that run's
    // reads: kept, but those it read again)
    let mut own_reads = p.reads.len();
    // (the kept reads' indices in the record: after the run's own, or,
    // for a run that went on after its last run's first reads, first)
    let mut kept_first: Option<usize> = None;
    let mut gone_kept: Vec<(Field, NodeId)> = Vec::new();
    // (the kept reads this run read again with another answer: the state
    // it resumed from rested on the old one, `replaced_moved` below)
    let mut replaced_moved: Vec<ReadKind> = Vec::new();
    // (a run that went on from the whole of its last run's reads, and a
    // filed one of those it no longer rests on, `ReadRec::filed`)
    let mut went_on = false;
    let mut lost_filed = false;
    // (a run that went on after its last run's first k reads: those
    // first, as they were, then its own, so the record's reads stay in
    // the order they were made, `Ctx::goes_on_after_reads`)
    if let Some(k) = p.kept_reads.get()
        && let Some(r) = node.run_record()
    {
        let mut merged: Vec<crate::node::ReadRec> = r.reads[..k.min(r.reads.len())].to_vec();
        kept_first = Some(merged.len());
        merged.append(&mut p.reads);
        p.reads = merged;
    } else if p.resumed.get()
        && let Some(r) = node.run_record()
    {
        went_on = true;
        let rreads: &[crate::node::ReadRec] = &r.reads;
        let again: FxHashMap<(ReadKind, PathId), Option<ValueId>> =
            p.reads.iter().map(|x| ((x.kind.clone(), x.path), x.seen)).collect();
        // (a kept read of the line this run read again and found moved: a
        // range or a read at a node, which is what a state folded along
        // the line rests on; the state it resumed from was built on the
        // old answer, and the new read, which replaces the kept one in
        // the record, holds, so no check would see it. Not a `next` that
        // met a hole: nothing of the state rests on an answer there. A
        // plain read of a field at the node, a placement's output, are
        // the run's own to act on as it reads them each run)
        for x in rreads {
            if let Some(&seen) = again.get(&(x.kind.clone(), x.path))
                && seen != x.seen
                && matches!(
                    x.kind,
                    ReadKind::Fold { .. }
                        | ReadKind::Next { .. }
                        | ReadKind::Field {
                            at: Anchor::Before(_) | Anchor::AfterOwn(_),
                            ..
                        }
                )
                && !(matches!(x.kind, ReadKind::Next { .. }) && x.seen.is_none())
            {
                replaced_moved.push(x.kind.clone());
            }
        }
        // (not the next contribution after a node gone: no run reads from
        // it again, and its going moved the read from the node before it,
        // which carries the change; kept, it would move at every commit)
        // (nor a read at a node gone: it asks the run again where that
        // node stood, once, below, and is not kept)
        let is_gone = |n: NodeId| eng.placed_node(n).is_none_or(|n| n.state() == State::Gone);
        let gone = |x: &crate::node::ReadRec| match x.kind {
            ReadKind::Next { after: Some(n), .. } => is_gone(n),
            ReadKind::Field {
                at: Anchor::Before(n) | Anchor::AfterOwn(n),
                ..
            } => is_gone(n),
            _ => false,
        };
        for x in rreads {
            if let ReadKind::Field {
                f,
                at: Anchor::Before(n) | Anchor::AfterOwn(n),
            } = x.kind
                && x.seen.is_some()
                && is_gone(n)
                && !again.contains_key(&(x.kind.clone(), x.path))
            {
                gone_kept.push((f, n));
            }
        }
        // (nor a placement's output: a run that parks reads the outputs
        // its state rests on again each run, as it places them; one it
        // did not reach this run, past where it stopped, its state does
        // not rest on, and kept it would refute the state when that
        // placement runs)
        // (a filed read this run read again with the same answer stays
        // filed, and this run's own read of it is dropped: filed again it
        // would be one more live reader of the same read, one a commit,
        // and every walk would ask the node once per commit since the
        // floor (xassoccnt's manual 8% slower at -j1))
        let filed_again: FxHashSet<(ReadKind, PathId)> = rreads
            .iter()
            .filter(|x| x.filed && again.get(&(x.kind.clone(), x.path)) == Some(&x.seen))
            .map(|x| (x.kind.clone(), x.path))
            .collect();
        if !filed_again.is_empty() {
            p.reads.retain(|r| !filed_again.contains(&(r.kind.clone(), r.path)));
            own_reads = p.reads.len();
        }
        p.reads.extend(
            rreads
                .iter()
                .filter(|x| {
                    let read_again = again.get(&(x.kind.clone(), x.path));
                    if x.filed {
                        let keep = read_again.is_none_or(|&seen| seen == x.seen) && !gone(x);
                        lost_filed |= !keep;
                        return keep;
                    }
                    read_again.is_none() && !gone(x) && !matches!(x.kind, ReadKind::Placed { .. })
                })
                .map(|x| crate::node::ReadRec {
                    filed: r.state.is_some() && stays_filed(x),
                    ..x.clone()
                }),
        );
    }
    for d in &mut p.defs {
        if let Some(c) = d.rel {
            d.value = rel_value_at(eng, d.f, node.id, c);
        }
    }
    let defs = dedup_defs(std::mem::take(&mut p.defs));
    let brackets = effective_brackets(node, p.effect);
    let filed = filed_defs(&eng.values, &defs, &brackets);
    // (a global definition by a node standing outside every scope is seen
    // as a local one at its depth is: one confirms a prediction, filed
    // at depth 0, of its value)
    let at_top = filed.iter().any(|d| d.depth == GLOBAL) && read::depth_of(eng, node.id) == Some(0);
    let bracket_change: Option<Brackets>;
    let contribs = combine_dups(eng, std::mem::take(&mut p.contribs));
    let mut last = node.record();
    let serial = node.serial.load(Ordering::Acquire) + 1;
    let old_placed: Vec<NodeId> = last.as_ref().map(|r| r.placed.clone()).unwrap_or_default();
    let kept = match_placements(eng, &old_placed, &p.placements);
    // (the nodes of other segments between a kept placement and the
    // lowest floor its check can have, the placer or a node it places
    // after: its check under the lock sees its own segment only)
    // (a kept placement's floor is the placer's, or one of the nodes it
    // or a placement before it places after: each its own span, from the
    // lowest of those to it. One span from the lowest anchor of all to
    // the farthest kept placement took in the whole text between a page
    // loop at the line's end, whose kept rest is next to it, and the
    // text node its new finisher goes after: every loop commit read
    // every node after that page's text, para4000 -j8's page chain 0.5 s
    // of its 1.1 s)
    let others: Vec<NodeId> = {
        let mut lo = node.pos_hint();
        let mut spans: Vec<(Pos, Pos)> = Vec::new();
        for (what, k) in p.placements.iter().zip(&kept) {
            if let Placement::FollowAfter(at, ..) = what
                && let Some(an) = eng.placed_node(*at)
            {
                lo = lo.min(an.pos_hint());
            }
            if let Some(k) = k {
                let kp = eng.node(*k).pos_hint();
                if kp > lo {
                    spans.push((lo, kp));
                }
            }
        }
        spans.sort_unstable();
        let mut merged: Vec<(Pos, Pos)> = Vec::with_capacity(spans.len());
        for (a, b) in spans {
            match merged.last_mut() {
                Some(last) if a <= last.1 => last.1 = last.1.max(b),
                _ => merged.push((a, b)),
            }
        }
        merged
            .into_iter()
            .flat_map(|(a, b)| foreign_between(eng, node.id, a, b))
            .collect()
    };
    // (the definitions its last run made that this run does not, for the
    // first text node it places anew: a run that now ends earlier, its
    // rest made after it (a recut), left those to the rest, whose first
    // run makes them again; filed as the rest's prediction, a reader after
    // it whose answer stays is not run again while the rest has not run,
    // and the rest's commit asks again those whose answer moved,
    // `settle_predictions`)
    let dropped: Vec<DefRec> = last
        .as_ref()
        .filter(|r| !r.provisional)
        .map(|r| {
            r.defs
                .iter()
                .filter(|d| d.rel.is_none() && !defs.iter().any(|x| x.f == d.f))
                .copied()
                .collect()
        })
        .unwrap_or_default();
    let mut rest_taken = dropped.is_empty();
    // everything a placement needs but its label, made before the lock
    let mut fresh: Vec<Option<(Arc<Node>, Predicted)>> = p
        .placements
        .iter()
        .zip(&kept)
        .enumerate()
        .map(|(i, (what, k))| {
            if k.is_some() && !matches!(what, Placement::FollowAfter(..) | Placement::FollowAfterPlaced(..)) {
                return None;
            }
            let (op, args) = what.key();
            let id = NodeId::from_raw(eng.nodes.reserve() as u64);
            let (n, mut pred) = built(eng, node.unit, Some(node.id), op, args.to_vec(), id);
            placed_effect(&p.place_effects, i, &n, &mut pred);
            // (made anew in place of the one at its index, of its
            // operation, which goes: that one's definitions are its
            // prediction, so that a reader after it whose answer stays
            // the same is not run again)
            if k.is_none()
                && let Some(&o) = old_placed.get(i)
                && !kept.contains(&Some(o))
                && let Some(on) = eng.placed_node(o)
                && on.op() == op
                && let Some(r) = on.record()
            {
                pred.inherit(&r.defs);
            }
            if !rest_taken && k.is_none() && eng.op(op).text() {
                rest_taken = true;
                let more: Vec<DefRec> = dropped
                    .iter()
                    .filter(|d| !pred.fields().any(|f| f == d.f))
                    .copied()
                    .collect();
                pred.inherit(&more);
            }
            Some((n, pred))
        })
        .collect();
    // (which reads this commit files. A run that goes on from its last
    // run keeps that run's reads, filed by the commits before: their
    // readers stand (`FILED`, from the node's `filed_floor` on), and this
    // commit files only the reads since, so that a loop parking at every
    // contributor of a long stretch does work for what it read now, not
    // for all it has read. All are filed anew when one of the kept ones
    // drops out, whose reader only a floor raised past it retires, and
    // after a run that did not go on. A read a next run makes again in
    // any case, a search that met a hole, stands for this commit alone)
    let file_all = !went_on || lost_filed;
    if file_all {
        for r in &mut p.reads {
            r.filed = false;
        }
    }
    // the record, made before the lock: no allocation under it
    let mut rec = Arc::new(RunRecord {
        serial,
        output: p.output,
        defs,
        contribs,
        // (kept for as long as the node stands: no growth's slack)
        reads: {
            let mut r = std::mem::take(&mut p.reads);
            r.shrink_to_fit();
            r
        },
        sources: {
            let mut s = std::mem::take(&mut p.sources);
            if (p.unchanged || p.resumed.get())
                && let Some(r) = node.record()
            {
                s.extend_from_slice(&r.sources);
            }
            s.sort_unstable();
            s.dedup();
            s.retain(|&x| x != node.id);
            s.into_boxed_slice()
        },
        // (`placed`, swapped in under the lock)
        placed: Vec::new(),
        claim: p.claim,
        claim_all: p.claim_all,
        effect: if brackets == Brackets::default() {
            Effect::None
        } else {
            Effect::Scope(brackets)
        },
        provisional: false,
        state: p.state,
        waits: std::mem::take(&mut p.waits).into_boxed_slice(),
    });
    let mut spare_readers: Vec<Reader> = Vec::with_capacity(4);
    let fields_of = |r: &Option<Arc<RunRecord>>| -> (Vec<Field>, Vec<Field>) {
        r.as_ref().map_or_else(Default::default, |r| {
            (
                r.defs.iter().map(|d| d.f).collect(),
                r.contribs.iter().map(|c| c.0).collect(),
            )
        })
    };
    let (mut old_fields, mut old_contribs) = fields_of(&last);
    let nd = rec.defs.len() + old_fields.len();
    let nc = rec.contribs.len() + old_contribs.len();
    let mut changed_defs: Vec<Field> = Vec::with_capacity(nd);
    let mut changed_contribs: Vec<Field> = Vec::with_capacity(nc);
    let mut rels: Vec<(Field, i64)> = Vec::with_capacity(rec.defs.len());
    let mut placed: Vec<NodeId> = Vec::with_capacity(p.placements.len().max(old_placed.len()));
    let mut made: Vec<Arc<Node>> = Vec::with_capacity(p.placements.len());
    // (the scratch of the filing under the lock, each as long as it can
    // grow there: nothing is allocated under it)
    let np = p.placements.len();
    let mut deferred: Vec<Deferred> = Vec::with_capacity(np);
    let mut replaced: Vec<(NodeId, Field, ValueId, u16)> = Vec::with_capacity(filed.len());
    let mut removed_kept: Vec<NodeId> = Vec::with_capacity(np);
    // (with their labels as each was read once, and their order before:
    // `sort_by_label`'s, sorted in place)
    let mut old_follows: Vec<(Pos, usize, NodeId)> = Vec::with_capacity(old_placed.len());
    let mut waiting: Vec<usize> = Vec::with_capacity(np);
    let mut made_now: Vec<bool> = Vec::with_capacity(np);
    let mut moving: Vec<NodeId> = Vec::with_capacity(np);
    let mut removed: Vec<NodeId> = Vec::with_capacity(old_placed.len() + np);
    // (the runs of the fields the filing empties, freed after the lock)
    let mut gone_runs: (
        Vec<crate::segment::PosMap<crate::segment::Def>>,
        Vec<crate::segment::PosMap<ValueId>>,
    ) = (
        Vec::with_capacity(old_fields.len()),
        Vec::with_capacity(old_contribs.len() + rec.contribs.len()),
    );
    let mut here_reads: Vec<(Field, Reader)> = Vec::with_capacity(rec.reads.len());
    let mut other_reads: Vec<(Field, Reader, Anchor)> = Vec::with_capacity(rec.reads.len());
    let mut final_reads: Vec<(Field, Reader)> = Vec::new();
    let mut range_reads: Vec<(Field, RangeReader)> = Vec::new();
    let mut range_seen: Vec<Vec<(PathId, Option<ValueId>)>> = Vec::new();
    let mut range_at: FxHashMap<RangeOf, usize> = FxHashMap::default();
    let mut env_reads: Vec<(Field, crate::engine::EnvReader)> = Vec::new();
    // what the lock decides of the walks and of the run's own reads
    let mut walks: Vec<(Field, Here)> = Vec::with_capacity(nd);
    let mut wake: Vec<(NodeId, NodeId, Field)> = Vec::with_capacity(8);
    let mut later: Vec<(Field, Reader, NodeId)> = Vec::new();
    let mut holes: Vec<(Field, RangeKey, RangeReader)> = Vec::new();
    let mut waits: Vec<NodeId> = Vec::new();
    let mut unchecked: Vec<usize> = Vec::with_capacity(rec.reads.len());
    let mut refuted: Option<usize> = None;
    let mut ix = IndexOps::with_capacity(nd, nc, rec.reads.len());
    let mut room: Option<Made> = None;
    let mut next_answers: FxHashMap<usize, Option<ValueId>> = FxHashMap::default();
    let parks = rec.state.is_some();
    for (ri, r) in rec.reads.iter().enumerate() {
        if r.filed && !file_all {
            continue;
        }
        // (only a run that parks: a next run going on from it keeps its
        // reads; any other's stand for its commit, and a walk asks no more
        // of them than their serial)
        let serial = if parks && stays_filed(r) {
            serial | FILED
        } else {
            serial
        };
        match &r.kind {
            ReadKind::Field { f, at } => {
                let (anchor, inclusive) = match *at {
                    Anchor::Me => (node.id, false),
                    Anchor::Before(n) => (n, false),
                    Anchor::AfterOwn(n) => (n, true),
                    Anchor::End(_) | Anchor::Final => (node.id, true),
                };
                let rec = Reader {
                    node: node.id,
                    serial,
                    anchor,
                    inclusive,
                    path: r.path,
                    seen: r.seen,
                };
                // (a derived read's witness, kept from an earlier run (one
                // that stood, a state resumed from): filed on the field its
                // name gives now as well, before the lock, so a change of
                // that field asks it again)
                if matches!(at, Anchor::Me | Anchor::Before(_) | Anchor::AfterOwn(_))
                    && let Some(spec) = read::derived_spec(eng, r.path)
                    && read::derived_base(eng.values(), spec) != Some(*f)
                    && let Some(g) = read::derived_field_at(eng, spec, anchor, inclusive)
                    && g != *f
                {
                    here_reads.push((g, rec.clone()));
                }
                match at {
                    // (a loop-back read: filed under the field, re-asked when
                    // the last definition of it lands, changes or goes)
                    Anchor::Final => final_reads.push((*f, rec)),
                    Anchor::End(_) => other_reads.push((*f, rec, *at)),
                    Anchor::Me | Anchor::Before(_) | Anchor::AfterOwn(_) => here_reads.push((*f, rec)),
                }
            }
            ReadKind::Env { f } => env_reads.push((
                *f,
                crate::engine::EnvReader {
                    node: node.id,
                    serial,
                    path: r.path,
                    seen: r.seen,
                },
            )),
            ReadKind::Fold { f, from, to, incl } => {
                let i = *range_at.entry((*f, *from, *to, false, *incl)).or_insert_with(|| {
                    range_reads.push((
                        *f,
                        RangeReader {
                            node: node.id,
                            serial,
                            from: *from,
                            to: *to,
                            next: false,
                            incl: *incl,
                            hole: false,
                            seen: Arc::from([]),
                        },
                    ));
                    range_seen.push(Vec::new());
                    range_reads.len() - 1
                });
                range_seen[i].push((r.path, r.seen));
            }
            ReadKind::Next { f, after } => {
                let i = *range_at.entry((*f, *after, None, true, false)).or_insert_with(|| {
                    // (the bound's label and the search from it on one
                    // layout, `read::on_one_layout`)
                    let n = read::on_one_layout(eng, || {
                        // (the bound's label taken under its segment's
                        // lock, the segment met; a bound gone: its last)
                        let from = match after {
                            Some(n) => Some(read::pos_of(eng, *n).unwrap_or_else(|| eng.node(*n).pos_hint())),
                            None => Some(read::unit_start(eng, node)),
                        };
                        next_contribution(eng, *f, from, node.id)
                    });
                    next_answers.insert(range_reads.len(), read::next_value(eng, &n));
                    range_reads.push((
                        *f,
                        RangeReader {
                            node: node.id,
                            serial,
                            from: *after,
                            to: n.stop,
                            next: true,
                            incl: false,
                            hole: n.item.is_none() && n.stop.is_some(),
                            seen: Arc::from([]),
                        },
                    ));
                    range_seen.push(Vec::new());
                    range_reads.len() - 1
                });
                range_seen[i].push((r.path, r.seen));
                // (the range is filed as this search found it, its stop or
                // the contribution it answers: a run that saw another
                // answer is refuted here, not later by a search of its
                // own, which may find what this one did not (a node that
                // had contributed, re-asked meanwhile: its contribution
                // passed over, `Node::is_open`) and let the run stand on a
                // range filed past what it read; amsldoc -j16 ended with 0
                // pages, the loop's rest on "none after 72" with node 75's
                // contribution filed, 10 builds in 600 on one loaded node)
                if refuted.is_none() && read::apply_path(eng, next_answers.get(&i).copied().flatten(), r.path) != r.seen
                {
                    refuted = Some(ri);
                }
            }
            ReadKind::Placed { .. } | ReadKind::Following { .. } => {}
            ReadKind::Hole => {
                if let Some(h) = r.seen.and_then(|x| eng.values().as_int(x)) {
                    waits.push(NodeId::from_raw(u64::from_ne_bytes(h.to_ne_bytes())));
                }
            }
        }
    }
    for ((_, r), seen) in range_reads.iter_mut().zip(range_seen) {
        r.seen = Arc::from(seen);
    }
    let mut read_fields: Vec<(Field, usize)> = here_reads.iter().map(|(f, _)| (*f, 1)).collect();
    read_fields.sort_unstable_by_key(|x| x.0);
    read_fields.dedup_by(|a, b| {
        let same = a.0 == b.0;
        if same {
            b.1 += a.1;
        }
        same
    });
    // (what the run defines and contributes, by field: the last run's
    // others are taken back under the lock, each looked up here, not
    // searched for)
    let new_fields: FxHashSet<Field> = rec.defs.iter().map(|d| d.f).collect();
    let new_contribs: FxHashSet<Field> = rec.contribs.iter().map(|c| c.0).collect();
    let mut tries = 0;
    todo_room();
    // (what the lock finds the filing would grow: its lists' room made here)
    let mut g = crate::segment::Grow::with_capacity(
        rec.defs.len(),
        rec.contribs.len(),
        read_fields.len(),
        fresh.iter().flatten().map(|(_, p)| p.fields().count()).sum(),
        fresh.len(),
        here_reads.len(),
    );
    let (seg_id, seg_lo, old_output, rec, seg_pos, removed) = loop {
        let Some((seg, mut inner)) = own_write(eng, node) else {
            return st;
        };
        if node.taken() {
            return st;
        }
        // (a node that has not committed may be given a relation ahead of
        // its run meanwhile, `learn_shape`: what this run does not define of
        // it is withdrawn, so the diff is against that record)
        {
            let now = node.last.lock();
            let same = match (&*now, &last) {
                (None, None) => true,
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                _ => false,
            };
            if !same {
                last = now.clone();
                drop(now);
                drop(inner);
                (old_fields, old_contribs) = fields_of(&last);
                gone_runs.0.reserve(old_fields.len());
                gone_runs.1.reserve(old_contribs.len() + rec.contribs.len());
                continue;
            }
        }
        if Segment::needs_split(&inner) {
            let id = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, id);
            continue;
        }
        // (labels for its new placements: a segment used up takes room
        // from a neighbour first, once)
        if tries < crate::order::ROOM_TRIES && !seg.has_room(&inner, fresh.iter().filter(|x| x.is_some()).count()) {
            drop(inner);
            tries += 1;
            eng.order
                .rebalance(eng, seg.id, fresh.iter().filter(|x| x.is_some()).count());
            continue;
        }
        let t_lock = Instant::now();
        let cpu_lock = crate::lock::thread_cpu_ns();
        let section = crate::obs::Section::enter();
        let pos = pos_in(&inner, node);
        // room for what is filed, made outside the lock
        if let Some(m) = room.as_mut() {
            m.install(eng, &mut inner);
        }
        inner.grow_for(
            &mut g,
            eng,
            pos,
            rec.defs.iter().map(|d| d.f),
            rec.contribs.iter().map(|c| c.0),
            &read_fields,
            fresh.iter().flatten().flat_map(|(_, p)| p.fields()),
            usize::from(brackets.closes > 0 || brackets.opens > 0)
                + fresh.iter().flatten().filter(|(_, p)| p.brackets()).count(),
            old_fields.iter().copied(),
            [holes.capacity(), wake.capacity().min(later.capacity())],
            fresh.iter().flatten().map(|(n, _)| n.op()),
            here_reads
                .iter()
                .filter(|(_, r)| r.anchor != node.id)
                .map(|(f, r)| (r.anchor, *f)),
        );
        if !g.is_empty() {
            drop(section);
            drop(inner);
            room = Some(g.make());
            let [h, w] = g.scratch();
            holes.reserve(h.saturating_sub(holes.len()));
            wake.reserve(w.saturating_sub(wake.len()));
            later.reserve(w.saturating_sub(later.len()));
            continue;
        }
        let mut lap = crate::obs::Laps::new(t_lock);
        // brackets, then definitions at their depths
        bracket_change = file_brackets(eng, &mut inner, node, pos, brackets);
        let contribs = &rec.contribs;
        // (predictions this commit replaced, handed on once the segment's
        // lock is let go)
        for d in &filed {
            let depth = d.depth;
            let m = inner.defs.entry(d.f).or_default();
            let first = m.is_empty();
            match m.get_mut(&pos) {
                Some(e)
                    if e.value == d.value
                        && (e.depth == depth || (e.provisional && at_top && e.depth == 0 && depth == GLOBAL)) =>
                {
                    if e.provisional {
                        st.predictions_confirmed += 1;
                    }
                    e.depth = depth;
                    e.provisional = false;
                    e.rel = d.rel;
                    e.by = node.id;
                    e.multi = d.multi;
                }
                Some(e) => {
                    // (judged when the evaluation ends, against what the
                    // node then defines here: a run on stale inputs run
                    // again is a rerun, its prediction right if its last
                    // run agrees, `Engine::judge_predictions`)
                    if e.provisional {
                        replaced.push((node.id, d.f, e.value, e.depth));
                    }
                    *e = Def {
                        depth,
                        value: d.value,
                        by: node.id,
                        rel: d.rel,
                        provisional: false,
                        multi: d.multi,
                    };
                    changed_defs.push(d.f);
                }
                None => {
                    m.insert(
                        pos,
                        Def {
                            depth,
                            value: d.value,
                            by: node.id,
                            rel: d.rel,
                            provisional: false,
                            multi: d.multi,
                        },
                    );
                    changed_defs.push(d.f);
                    if first {
                        ix.defs_add.push(d.f);
                    }
                }
            }
            if let Some(c) = d.rel {
                rels.push((d.f, c));
            }
        }
        for &f in &old_fields {
            if new_fields.contains(&f) {
                continue;
            }
            if let Some(m) = inner.defs.get_mut(&f)
                && m.get(&pos).is_some_and(|e| e.by == node.id)
            {
                m.remove(&pos);
                changed_defs.push(f);
                if m.is_empty() {
                    // (its run freed after the lock)
                    gone_runs.0.extend(inner.defs.remove(&f));
                    ix.defs_remove.push(f);
                }
            }
        }
        lap.lap("defs");
        // contributions
        for &(f, x) in contribs {
            let m = inner.contribs.entry(f).or_default();
            let first = m.is_empty();
            match m.insert(pos, x) {
                Some(old) if old == x => {}
                _ => {
                    changed_contribs.push(f);
                    if first {
                        ix.contribs_add.push(f);
                    }
                }
            }
        }
        for &f in &old_contribs {
            if new_contribs.contains(&f) {
                continue;
            }
            if let Some(m) = inner.contribs.get_mut(&f)
                && m.remove(&pos).is_some()
            {
                changed_contribs.push(f);
                if m.is_empty() {
                    ix.contribs_remove.push(f);
                }
            }
        }
        changed_contribs.sort_unstable();
        changed_contribs.dedup();
        for &f in &changed_contribs {
            gone_runs.1.extend(inner.resum_keep(eng, f));
            eng.bump(f);
        }
        for &f in &changed_defs {
            eng.bump(f);
        }
        lap.lap("contribs");
        // placements
        let mut after = node.id;
        let mut in_seg = true;
        // (a continuation made anew takes the place of the one it
        // replaces: what others placed after this node's children since,
        // before that one, stays before it, as a page fired in this node
        // does; in the order of their labels, taken from the front)
        old_follows.extend(
            old_placed
                .iter()
                .copied()
                .filter(|n| !kept.contains(&Some(*n)))
                .filter(|&n| {
                    eng.placed_node(n)
                        .is_some_and(|x| x.state() != State::Gone && x.slot() == 2)
                })
                .enumerate()
                .map(|(i, n)| (eng.node(n).pos_hint(), i, n)),
        );
        old_follows.sort_unstable();
        let mut follows_at = 0;
        // (a kept placement's subtree end found only where a placement
        // follows it: the last one's subtree may be a whole chain)
        let mut kept_end: Option<NodeId> = None;
        // (the last placement with a label: one made in this commit and put
        // off to after the lock has none yet, and goes before the next
        // placement kept, `waiting`, so that one still stands after it)
        let mut labelled = node.id;
        // (whether the last placement went after another node than this
        // one: one following it goes after it there, not before a
        // continuation of this node's that it replaces)
        let mut elsewhere = false;
        // (the placement before, where it stands: a kept one follows it)
        let mut prev: Option<NodeId> = None;
        // (`made_now`: whether each placement is a node made by this
        // commit; `moving`: the placements relocated by this commit, put
        // off to after the lock as one made anew is, standing nowhere yet
        // for the checks of those after them)
        for (i, what) in p.placements.iter().enumerate() {
            // (a placement after another node goes after that one's
            // children, not after this subtree's end: not walked for it,
            // a loop rest's whole chain of rests after it at each rest's
            // commit)
            if let Some(k) = kept_end.take() {
                after = if matches!(what, Placement::FollowAfter(..) | Placement::FollowAfterPlaced(..)) {
                    k
                } else {
                    subtree_end(eng, k)
                };
                labelled = after;
            }
            let after_pos = if moving.contains(&labelled) {
                Pos(0)
            } else {
                eng.node(labelled).pos_hint()
            };
            // (after a placement of this run: after the node it stands
            // for; one made by this commit is placed after it, not kept
            // where it stood before that node was)
            let resolved;
            let mut kept_i = kept[i];
            // (a matched placement not standing where it is placed now:
            // relocated, `relocate`)
            let mut move_k: Option<NodeId> = None;
            let what = match what {
                Placement::FollowAfterPlaced(j, op, a) => {
                    if made_now[*j]
                        && let Some(k) = kept_i.take()
                    {
                        move_k = Some(k);
                    }
                    resolved = Placement::FollowAfter(placed[*j], *op, a.clone());
                    &resolved
                }
                w => w,
            };
            made_now.push(true);
            let rank_i = match what {
                Placement::FollowAfter(..) => p.ranks.get(i).copied().unwrap_or(0),
                _ => 0,
            };
            if let Some(k) = kept_i {
                let kn = eng.node(k);
                // (one placed after another node stands while it is still
                // after that node and its children, wherever the placer is:
                // a node may place after a node before itself)
                let (floor, ceiling) = match what {
                    Placement::FollowAfter(at, ..) => {
                        let (e, before) = children_end(eng, *at);
                        (eng.node(e).pos_hint(), before)
                    }
                    Placement::Child(..) | Placement::Follow(..) => (after_pos, None),
                    Placement::FollowAfterPlaced(..) => unreachable!("resolved above"),
                };
                // (and nothing of a later slot under its anchor stands
                // between: one left where it was when the slots before
                // it moved back is made anew where it belongs)
                let (anchor, slot, sfloor) = match what {
                    Placement::FollowAfter(at, ..) => (*at, 1, floor),
                    Placement::Child(..) => (node.id, 0, floor),
                    Placement::Follow(..) if elsewhere => {
                        // (the placement before not in the table yet, made
                        // by this commit and put off: the floor stands)
                        let q = prev.expect("a placement before, placed elsewhere");
                        let f = eng
                            .placed_node(q)
                            .filter(|qn| {
                                qn.state() != State::Gone && qn.pos_hint() != Pos(0) && !moving.contains(&qn.id)
                            })
                            .map_or(floor, |qn| eng.node(subtree_end(eng, qn.id)).pos_hint());
                        (q, 1, f)
                    }
                    Placement::Follow(..) => (node.id, 2, floor),
                    Placement::FollowAfterPlaced(..) => unreachable!("resolved above"),
                };
                let kpos = kn.pos_hint();
                // (a ranked one after another node: of its rank still, no
                // node of a higher rank before it, `Ctx::rank_placement`)
                let rank = if slot == 1 { rank_i } else { 0 };
                if kn.state() != State::Gone
                    && kpos > floor
                    && ceiling.is_none_or(|c| kpos < c)
                    && kn.rank() == rank
                    && !stranded(eng, &inner, &others, node.id, anchor, slot, sfloor, kpos, rank)
                {
                    placed.push(k);
                    kept_end = Some(k);
                    prev = Some(k);
                    elsewhere = matches!(what, Placement::FollowAfter(..));
                    for d in waiting.drain(..) {
                        if deferred[d].2.is_none() {
                            deferred[d].2 = Some(k);
                        }
                    }
                    made_now[i] = false;
                    continue;
                }
                move_k = Some(k);
            }
            // (a relocation keeps the node, its record and its own
            // placements (docs/spec-continuations.md §2.3); one across
            // units, or of a node a claim holds or takes, is a remake: its
            // place is its claimer's, among the claimer's siblings, and a
            // node's unit is fixed at its making)
            let movable = move_k.filter(|&k| {
                let kn = eng.node(k);
                let unit = match what {
                    // (one made by this commit, not on the line yet: of the placer's unit)
                    Placement::FollowAfter(at, ..) => Some(eng.placed_node(*at).map_or(node.unit, |a| a.unit)),
                    _ => Some(node.unit),
                };
                !matches!(kn.state(), State::Gone | State::Inert | State::RunningInert)
                    && kn.claimer().is_none()
                    && unit == Some(kn.unit)
                    && kn.pos_hint() != Pos(0)
            });
            if let Some(k) = move_k
                && movable.is_none()
            {
                removed_kept.push(k);
            }
            let (n, pred) = match (movable, fresh[i].take()) {
                (Some(k), _) => {
                    moving.push(k);
                    (eng.node(k).clone(), None)
                }
                (None, Some((n, pred))) => (n, Some(pred)),
                (None, None) => {
                    let (op, args) = what.key();
                    let id = NodeId::from_raw(eng.nodes.reserve() as u64);
                    let (n, mut pred) = built(eng, node.unit, Some(node.id), op, args.to_vec(), id);
                    placed_effect(&p.place_effects, i, &n, &mut pred);
                    (n, Some(pred))
                }
            };
            let id = n.id;
            n.follows
                .store(!matches!(what, Placement::Child(..)), Ordering::Release);
            placed.push(id);
            let at = match what {
                Placement::FollowAfter(at, _, _) => Some(*at),
                Placement::Child(..) | Placement::Follow(..) => None,
                Placement::FollowAfterPlaced(..) => unreachable!("resolved above"),
            };
            let (anchor, slot) = match what {
                Placement::FollowAfter(at, ..) => (*at, 1),
                Placement::Child(..) => (node.id, 0),
                Placement::Follow(..) if elsewhere => (prev.expect("a placement before, placed elsewhere"), 1),
                Placement::Follow(..) => (node.id, 2),
                Placement::FollowAfterPlaced(..) => unreachable!("resolved above"),
            };
            n.anchor.store(anchor.raw() + 1, Ordering::Release);
            n.slot.store(slot, Ordering::Release);
            n.rank.store(if slot == 1 { rank_i } else { 0 }, Ordering::Release);
            prev = Some(id);
            let before = if matches!(what, Placement::Follow(..)) && !elsewhere {
                while old_follows
                    .get(follows_at)
                    .is_some_and(|&(_, _, b)| eng.node(b).pos_hint() <= after_pos)
                {
                    follows_at += 1;
                }
                follows_at += 1;
                old_follows.get(follows_at - 1).map(|x| x.2)
            } else {
                None
            };
            // (one replacing a continuation left behind, with nodes of a
            // later slot between, or standing in another segment: not
            // there, where the continuation belongs)
            let before = before.filter(|&b| {
                let bn = eng.node(b);
                let bp = bn.pos_hint();
                bn.seg_hint() == seg.id
                    && inner.nodes.get(&bp) == Some(&b)
                    && !stranded(eng, &inner, &others, node.id, node.id, 2, after_pos, bp, 0)
            });
            let in_here = at.is_none()
                && pred.is_some()
                && in_seg
                && seg.can_place(&inner)
                && !Segment::needs_split(&inner)
                && eng.node(after).seg_hint() == seg.id
                && inner.nodes.get(&eng.node(after).pos_hint()) == Some(&after)
                && before.is_none_or(|b| {
                    eng.node(b).seg_hint() == seg.id && inner.nodes.get(&eng.node(b).pos_hint()) == Some(&b)
                });
            if in_here {
                // (a continuation goes after what others placed after this
                // node and its children, and what follows those)
                let after = if slot == 2 && before.is_none() {
                    attached_end(eng, &inner, node.id, after)
                } else {
                    after
                };
                let pred = pred.expect("a node made anew");
                made.push(match before {
                    Some(b) => place_before(eng, seg, &mut inner, b, n, pred),
                    None => place(eng, seg, &mut inner, after, n, pred),
                });
                labelled = id;
            } else {
                in_seg = false;
                if at.is_none() {
                    waiting.push(deferred.len());
                }
                deferred.push((i, at.unwrap_or(after), before, n, pred));
            }
            // (one after another node goes after its children there: what
            // follows it is checked against that)
            // (a node this commit made after another not on the line yet
            // goes after it later, in order: nothing is placed after it here)
            if let Some(a) = at
                && eng.placed_node(a).is_some()
            {
                labelled = children_end(eng, a).0;
            }
            elsewhere = at.is_some();
            after = id;
        }
        if keep_placed {
            placed.clone_from(&old_placed);
        }
        removed.extend(old_placed.iter().copied().filter(|n| !placed.contains(n)));
        removed.append(&mut removed_kept);
        // (a placement that relabelled the segment moved this node's label)
        let pos = pos_in(&inner, node);
        lap.lap("placements");
        // reads anchored here
        node.committing.store(true, Ordering::SeqCst);
        for (f, r) in here_reads.drain(..) {
            let a = eng.node(r.anchor);
            if walk_trace(eng, f) {
                eprintln!(
                    "walk-trace: commit of node {} serial {} files a read anchored at {} (pos {} seg {}) in seg {} lo {} hi {}: {}",
                    node.id.raw(),
                    r.serial,
                    r.anchor.raw(),
                    a.pos_hint().0,
                    a.seg_hint().0,
                    seg.id.0,
                    seg.lo.0,
                    inner.hi.0,
                    if r.anchor == node.id
                        || (a.seg_hint() == seg.id && inner.nodes.get(&a.pos_hint()) == Some(&r.anchor))
                    {
                        "here"
                    } else {
                        "elsewhere"
                    }
                );
            }
            if r.anchor == node.id || (a.seg_hint() == seg.id && inner.nodes.get(&a.pos_hint()) == Some(&r.anchor)) {
                if file_reader(eng, &mut inner, f, r, &mut spare_readers) {
                    ix.readers_add.push(f);
                }
            } else {
                let at = Anchor::Before(r.anchor);
                other_reads.push((f, r, at));
            }
        }
        lap.lap("readers");
        // the walks, as far as this segment goes
        changed_defs.sort_unstable();
        changed_defs.dedup();
        for &f in &changed_defs {
            walks.push((
                f,
                walk_here(eng, &mut inner, f, pos, node.id, &mut wake, &mut later, &mut st),
            ));
        }
        // (a `next` reader stops at a node to run again as at a hole,
        // `Node::is_open`: taken at every commit)
        // (into room made before the lock, `Grow::scratch`)
        holes.extend(inner.stops.extract_if(.., |s| s.2.to == Some(node.id)));
        lap.lap("walks");
        // the run's own reads at its position, answered again (under
        // scopes after the lock: an answer may lie past a close)
        let plain = !read::scoped(eng);
        for (i, r) in rec.reads.iter().enumerate() {
            if r.filed && !file_all {
                continue;
            }
            if let ReadKind::Field { f, at: Anchor::Me } = r.kind
                && plain
                && !read::client_path(eng, r.path)
                && let Some((_, d)) = Segment::reaching(&inner, f, pos, false)
            {
                if refuted.is_none() && through(eng, Reaching::of(&d.plain(&eng.values)), r.path) != r.seen {
                    note_refute(
                        eng,
                        f,
                        node.id,
                        Some(d.by),
                        r.seen,
                        through(eng, Reaching::of(&d.plain(&eng.values)), r.path),
                    );
                    refuted = Some(i);
                }
            } else {
                unchecked.push(i);
            }
        }
        if serial == 1 {
            inner.holes = inner.holes.saturating_sub(1);
        }
        std::mem::swap(
            &mut Arc::get_mut(&mut rec).expect("the record is not shared yet").placed,
            &mut placed,
        );
        let old = node.last.lock().replace(rec.clone());
        for r in rec.reads.iter().filter(|_| crate::obs::enabled()) {
            if let Some(f) = r.kind.field()
                && ftrace(eng, f)
            {
                eprintln!(
                    "ftrace: read {} by {} ({}) serial {serial} seen {} filed {} {:?} t {}",
                    eng.values().as_str(f.0).unwrap_or_default(),
                    node.id.raw(),
                    eng.op(node.op()).name(),
                    r.seen.map_or(0, ValueId::raw),
                    r.filed,
                    r.kind,
                    eng.now_ns()
                );
            }
        }
        for d in rec.defs.iter().filter(|_| crate::obs::enabled()) {
            if ftrace(eng, d.f) {
                eprintln!(
                    "ftrace: def {} by {} ({}) serial {serial} value {} changed {} t {}",
                    eng.values().as_str(d.f.0).unwrap_or_default(),
                    node.id.raw(),
                    eng.op(node.op()).name(),
                    d.value.raw(),
                    changed_defs.contains(&d.f),
                    eng.now_ns()
                );
            }
        }
        if file_all {
            node.filed_floor.store(serial, Ordering::Release);
        }
        node.serial.store(serial, Ordering::Release);
        // (`PHITEX_REFUTE_LOG`: a run again whose products are its last
        // run's, defined, contributed and output alike: a cutoff's)
        if refute_log()
            && let Some(o) = old.as_ref().filter(|o| !o.provisional)
        {
            let same = o.output == rec.output
                && o.contribs == rec.contribs
                && o.defs.len() == rec.defs.len()
                && o.defs
                    .iter()
                    .zip(&rec.defs)
                    .all(|(a, b)| a.f == b.f && a.value == b.value && a.rel == b.rel);
            eprintln!(
                "redo:\t{}\t{}\t{}",
                node.id.raw(),
                eng.op(node.op()).name(),
                u8::from(same)
            );
        }
        // (`PHITEX_KREAD_KEYS=a|b|…` or `*`, with `PHITEX_REFUTE_LOG`: each
        // committed text run's reads of those fields, the value's id, for
        // the scaling report's predictor test: what a paragraph start would
        // have had to guess)
        if refute_log() && eng.op(node.op()).text() {
            let keys = kread_keys();
            for r in &rec.reads {
                if let ReadKind::Field { f, at: Anchor::Me } = r.kind
                    && let Some(name) = eng.values().as_str(f.0)
                    && (keys.is_empty() || keys.contains(&name))
                {
                    eprintln!(
                        "kread:\t{}\t{}\t{name}\t{}",
                        node.id.raw(),
                        pos.0,
                        r.seen.map_or(0, ValueId::raw)
                    );
                }
            }
        }
        if let Some(o) = &old {
            eng.stale_reads.fetch_add(o.reads.len() as u64, Ordering::Relaxed);
        }
        let old_output = last.as_ref().filter(|r| !r.provisional).map(|r| r.output);
        lap.lap("checks");
        ix.apply(eng, seg, &inner);
        // (the room reserved in the field indexes and not added into)
        eng.defs.release();
        eng.contribs.release();
        eng.readers.release();
        lap.lap("index");
        st.commit_ns = ns(t_lock);
        st.commit_cpu_ns = crate::lock::thread_cpu_ns().saturating_sub(cpu_lock);
        drop(section);
        drop(inner);
        (st.commit_allocs, st.commit_frees) = crate::obs::section_take();
        lap.report(node.id.0, st.commit_ns, rec.defs.len(), rec.reads.len());
        if !replaced.is_empty() {
            eng.pred_pending.lock().append(&mut replaced);
        }
        drop(old);
        break (seg.id, seg.lo, old_output, rec, pos, removed);
    };
    // 2. reads anchored elsewhere
    // (a node read at that is gone by now: a read that found a value there
    // finds none, and no removal will ask it again)
    let mut anchor_gone: Vec<(Field, NodeId)> = gone_kept;
    for (f, r, at) in other_reads {
        // (a read at a unit's end: filed with the field, not at the node
        // that was last when it committed. A node placed after that one,
        // or a split of the last segment under way while the last node
        // was looked up (the lookup's order of segments taken before the
        // split's new one is in it), left the reader before a definition
        // reaching the end, whose walk never came back to it: finishers
        // at -j8 kept a page their loop had moved on from, marginpars28
        // wrong in 10 of 120 builds. The walks that pass the unit's end
        // re-ask it, `reask_ends`)
        if let Anchor::End(u) = at {
            eng.ranges.add_end(f, u, r, |x| live(eng, x.node, x.serial));
            continue;
        }
        let a = eng.node(r.anchor);
        let mut spare: Vec<Reader> = Vec::with_capacity(4);
        if let Some((seg, mut inner)) = own_write(eng, a) {
            let first = file_reader(eng, &mut inner, f, r, &mut spare);
            drop(inner);
            if first {
                eng.readers.add(f, seg.lo, seg.id);
            }
        } else if r.seen.is_some() {
            anchor_gone.push((f, r.anchor));
        }
    }
    file_ranges(eng, ww, range_reads, node.id, &mut st);
    file_waits(eng, ww, node, serial, waits, &mut st);
    if eng.op(node.op()).read_back() && !changed_defs.is_empty() {
        eng.has_finals.store(true, Ordering::Release);
        for &f in &changed_defs {
            eng.ranges.mark_read_back(f);
        }
    }
    for (f, r) in final_reads {
        if !eng.has_finals.load(Ordering::Acquire) {
            eng.has_finals.store(true, Ordering::Release);
        }
        eng.ranges.add_final(f, r, |x| live(eng, x.node, x.serial));
    }
    for (f, r) in env_reads {
        eng.ranges.add_envs(f, vec![r], |x| live(eng, x.node, x.serial));
    }
    // deferred placements: after a node elsewhere (one placed after this
    // node was removed or taken, which withdrew this record, goes with it)
    let mut moves = 0u64;
    // (those still to move: the place of one is not after another that
    // stands where it stood, about to come after it)
    let mut to_move: FxHashSet<NodeId> = deferred.iter().filter(|d| d.4.is_none()).map(|d| d.3.id).collect();
    for (i, at, before, n, pred) in deferred {
        // (a relocation: after what it is placed after now, if this run
        // still stands)
        let Some(pred) = pred else {
            if node.state() == State::Gone || !node.record().is_some_and(|r| Arc::ptr_eq(&r, &rec)) {
                continue;
            }
            let to = if matches!(
                p.placements[i],
                Placement::FollowAfter(..) | Placement::FollowAfterPlaced(..)
            ) {
                Target::AfterChildren(at)
            } else if let Some(b) = before {
                Target::Before(b, at)
            } else {
                Target::After(at)
            };
            to_move.remove(&n.id);
            if relocate(eng, ww, node.id, &n, to, &to_move, &mut st) {
                moves += 1;
                *w.log.lock().moved_by_op.entry(n.op()).or_default() += 1;
            }
            continue;
        };
        let n = if matches!(
            p.placements[i],
            Placement::FollowAfter(..) | Placement::FollowAfterPlaced(..)
        ) {
            place_after_children(eng, at, n, pred)
        } else if let Some(b) = before {
            place_before_node(eng, b, at, n, pred)
        } else {
            place_after(eng, at, n, pred)
        };
        if node.state() == State::Gone || !node.record().is_some_and(|r| Arc::ptr_eq(&r, &rec)) {
            unfile(eng, ww, &n);
            continue;
        }
        made.push(n);
    }
    if moves > 0 {
        w.log.lock().placements_moved += moves;
    }
    // (the ledger: a run after a run of the same node that left every
    // output as it was, its definitions and relations, its contributions,
    // its output, its brackets and its placements, is a wasted run:
    // nothing after it is asked again for it, `Report::runs_wasted`)
    if last.as_ref().is_some_and(|r| !r.provisional)
        && changed_defs.is_empty()
        && changed_contribs.is_empty()
        && old_output == Some(rec.output)
        && bracket_change.is_none()
        && made.is_empty()
        && removed.is_empty()
        && moves == 0
    {
        *w.log.lock().wasted_by_op.entry(node.op()).or_default() += 1;
    }
    // what this run no longer places goes once what replaces it stands: a
    // node made anew in place of a removed one has filed its definitions
    // as predicted, and the readers after it whose answer stays are not
    // asked again
    for id in removed {
        // (one a commit cut short never placed is not on the line)
        if let Some(n) = eng.placed_node(id) {
            // (one a sibling's claim took: that claimer read its text,
            // `Ctx::following`, and now reads another's there, as for a
            // client's removal, `Engine::remove`. A paragraph whose line
            // a rerun of its placer remade kept the old line, and the new
            // one ran as a paragraph of its own after it)
            let claimer = n.claimer().filter(|&c| n.taken() && c != node.id);
            unfile(eng, ww, n);
            if let Some(c) = claimer.and_then(|c| eng.placed_node(c)) {
                mark_dirty(
                    eng,
                    ww,
                    c,
                    Wake {
                        by: Some(id),
                        cause: Cause::Structure,
                        at_ns: eng.now_ns(),
                    },
                );
            }
        }
    }
    if p.claim > 0 || last.as_ref().is_some_and(|r| r.claim > 0) {
        // (the claim the record keeps, of the rest or of siblings, as its
        // count says: a run that published nothing new (`Ctx::unchanged`)
        // keeps its last run's claim, and the flag its own run's walk
        // left (a claim of none: siblings) read that last run's claim of
        // the rest, usize::MAX, as one of every sibling. A page's
        // finisher whose routine ended in a fatal error on values not
        // settled, run again with its page stale, took 43 finishers of
        // the next loop that way, inert ever after: thumbs at -j8, a
        // timeout in 48, accl 25360)
        node.claim_all.store(p.claim_all, Ordering::Release);
        apply_claim(eng, ww, node, p.claim);
    }
    // (the frontier this run moved, before any reader is asked again: one
    // woken by this commit past it reads what the layer defines and waits
    // there (`Ctx::hold_at_frontier`). Set after the re-asks, the woken
    // reader began on the old frontier and read on past the new one, and
    // ran again on what a run held before it then defined)
    if let Some(at) = frontier {
        eng.frontier_by.store(node.id.raw() + 1, Ordering::Release);
        let new = at.map_or(0, |n| n.raw() + 1);
        let old = eng.frontier.swap(new, Ordering::AcqRel);
        crate::sched::log_frontier(eng, old, new, Some(node.id), "set");
    }
    // a changed bracket: the readers after it whose answer moved
    if let Some(old) = bracket_change {
        reask_after_own_effect(eng, ww, node, seg_pos, old, brackets, &old_fields, &mut st);
    }
    // 3. the walks: readers the lock found changed, then on past the segment
    for (n, by, f) in wake {
        let rn = eng.node(n);
        if mark_stale(
            eng,
            ww,
            rn,
            Wake {
                by: Some(by),
                cause: Cause::Changed {
                    field: f,
                    site: Some(by),
                },
                at_ns: eng.now_ns(),
            },
        ) {
            st.woken += 1;
        }
    }
    for (f, r, by) in later {
        reask_reader(eng, ww, f, &r, Some(by), &mut st);
    }
    for (f, h) in walks {
        match h {
            Here::Done => {}
            Here::On(by) => reask_later(eng, ww, f, seg_id, seg_lo, by, &mut st),
            Here::From(by) => reask(eng, ww, f, eng.node(by).pos_hint(), seg_id, by, &mut st),
            Here::Stopped(h) => stop_at(eng, ww, h, f, 0, &mut st),
        }
    }
    if serial == 1 {
        let defined: Vec<(Field, u16)> = rec.defs.iter().map(|d| (d.f, def_level(eng, d.f, node.id))).collect();
        walk_stopped(eng, ww, node, node.pos_hint(), seg_id, &defined, &mut st);
    }
    for &f in &changed_contribs {
        reask_ranges(eng, ww, f, seg_pos, node.id, &holes, &mut st);
    }
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in holes {
        reask_range(eng, ww, f, k, &r, node.id, false, &mut st, &mut memo);
    }
    if !made.is_empty() {
        let mut log = w.log.lock();
        for n in &made {
            *log.placed_by_op.entry(n.op()).or_default() += 1;
        }
    }
    // (queued in their order: the ready heap takes the first queued first
    // among equal keys (`sched::Ready`), so its placements run in their
    // order, a child before the node after it, where `sched::Ready`
    // ties them (the settling layer's, keyed at the frontier, which this
    // run moved above))
    for n in &made {
        enqueue(
            eng,
            ww,
            n,
            Wake {
                by: Some(node.id),
                cause: Cause::Created { by: Some(node.id) },
                at_ns: eng.now_ns(),
            },
        );
        settle_predictions(eng, ww, n, &mut st);
    }
    // (the frontier moved: its placements stand, so a reader let go meets
    // them)
    if frontier.is_some() {
        crate::sched::release_frontier(eng, ww);
    }
    if let Some(parent) = node.parent {
        let parent = eng.node(parent);
        if old_output != Some(p.output) {
            reask_parent(eng, ww, parent, node.id, &mut st);
        }
        wake_waiting(eng, ww, parent, node.id, &mut st);
    }
    // (the runs that stopped for this node's commit, `Ctx::read`)
    crate::sched::release_waiting(eng, ww, node.id);
    // its relations from their input now: one that changed before they were
    // filed was computed from the older
    for &(f, c) in &rels {
        if settle_rel(eng, f, node.id, c) {
            st.relative_resolved += 1;
            reask(eng, ww, f, node.pos_hint(), node.seg_hint(), node.id, &mut st);
        }
    }
    // 5. a first relation by this operation
    for (f, c) in rels.into_iter().filter(|_| eng.op(node.op()).alike()) {
        learn_shape(eng, ww, node.op(), f, c, &mut st);
    }
    // 6. the node's own reads the lock could not answer
    // (one whose field no change has counted since it read stands)
    // (a bracket filed since the run began may have moved any read)
    let scope_moved = read::scoped(eng) && eng.scope_gen.load(Ordering::Acquire) != p.scope_gen;
    // (a filed read's reader stood through the run: one that moved re-asked
    // the run, `Node::asked`, and they are checked with the rest; so too
    // for a run not woken by the answer it parked on, which a change
    // before it ran may have merged into its wake)
    let asked = node.asked.swap(false, Ordering::SeqCst);
    if !file_all && (asked || scope_moved || !p.on_park) {
        unchecked.extend(rec.reads.iter().enumerate().filter(|(_, r)| r.filed).map(|(i, _)| i));
    }
    let refuted = refuted.or_else(|| {
        let mut memo = read::FoldMemo::new();
        // (siblings read in turn, checked in one pass)
        let mut follow_at = None;
        unchecked.into_iter().find(|&i| {
            let r = &rec.reads[i];
            // (a `next`'s answer moves when a node it stopped at commits
            // with nothing, which counts no change of its field)
            if !scope_moved
                && !matches!(r.kind, ReadKind::Next { .. })
                && r.kind.field().is_some_and(|f| eng.field_epoch(f) == r.epoch)
            {
                return false;
            }
            if let ReadKind::Following { i } = r.kind {
                let n = read::following_on(eng, node, i, &mut follow_at);
                return read::apply_path(eng, n.map(|n| read::text_value(eng, &n)), r.path) != r.seen;
            }
            !read::holds(eng, node, r, &mut memo)
        })
    });
    // (where a read that moved sends the next run: a field's change, the
    // environment's, a range's at the range's start (where the change
    // lies the check does not know: a loop resuming from its state goes
    // back before it), a placement's output)
    // (a read at a node, by a run that parks a state: the change reaches
    // that node, which the next run goes back before; else no site)
    let parks = p.state.is_some();
    let cause_of = |kind: &ReadKind| match kind {
        ReadKind::Field {
            f,
            at: Anchor::Before(n) | Anchor::AfterOwn(n),
        } if parks => Cause::Changed {
            field: *f,
            site: Some(*n),
        },
        ReadKind::Field { f, .. } => Cause::Changed { field: *f, site: None },
        ReadKind::Env { f } => Cause::Environment { field: *f },
        ReadKind::Fold { f, from: start, .. } | ReadKind::Next { f, after: start } => Cause::Range {
            field: *f,
            by: start.or_else(|| first_of_unit(eng, node)).unwrap_or(node.id),
        },
        ReadKind::Placed { .. } => Cause::Output { of: node.id },
        ReadKind::Following { .. } | ReadKind::Hole => Cause::Structure,
    };
    // (a `next` after `n` that answered none, stopped at a node not
    // committed then: no node between `n` and that stop contributed, so
    // the change lies at the contribution or stop it answers now, which
    // is where the next run goes back before, not `n`: a loop parked at
    // a hole the node after `n` filled goes on from where it parked, as
    // when that node's own commit wakes it)
    let cause_of_read = |r: &crate::node::ReadRec| match r.kind {
        ReadKind::Next { f, after: Some(n) } if r.seen.is_none() => {
            let now = read::on_one_layout(eng, || {
                read::pos_of(eng, n).map(|at| read::next_contribution(eng, f, Some(at), node.id))
            });
            let by = now.and_then(|x| x.item.map(|(id, _)| id).or(x.stop)).unwrap_or(n);
            Cause::Range { field: f, by }
        }
        _ => cause_of(&r.kind),
    };
    // (a read kept from the run its state came from no longer holds: the
    // run that resumed from that state rests on it. A range read names
    // where the change may lie, and the next run, offered the state, goes
    // back before it as for any range that moved; any other kept read
    // that moved makes the state stale, and the next run starts from
    // nothing. So every kept read is checked once a kept one moved: a
    // kept field read behind a range read's start would otherwise stand
    // unseen. One that took none clears that.)
    let mut causes: Vec<Cause> = Vec::new();
    let mut stale = false;
    if let Some(i) = refuted
        && refute_log()
        && let Some(f) = rec.reads[i].kind.field()
    {
        // (the commit's own check: the read's old answer and its new one)
        let r = &rec.reads[i];
        let now = read::answer_in(eng, node, &r.kind, r.path, &mut read::FoldMemo::new());
        note_refute(eng, f, node.id, None, r.seen, now);
    }
    node.refuted
        .store(refuted.is_some() || !replaced_moved.is_empty(), Ordering::Release);
    if let Some(i) = refuted {
        // (a range from a node, or a read at a node: the change lies
        // there; one from the unit's start names no node a run could go
        // back before, nor one from the site the run was told to go back
        // before, which it did not read again. That site is the one the
        // read's cause names (`cause_of_read`: for a `next` that answered
        // none, the contribution or stop it answers now, not its start):
        // a run woken there that rests on the kept read again would be
        // sent back there at every commit, never from nothing)
        let range = |k: &ReadKind, c: &Cause| match (k, c) {
            (ReadKind::Next { after: Some(_), .. }, Cause::Range { by, .. }) => p.woken_at != Some(*by),
            (
                ReadKind::Fold { from: Some(n), .. }
                | ReadKind::Next { after: Some(n), .. }
                | ReadKind::Field {
                    at: Anchor::Before(n) | Anchor::AfterOwn(n),
                    ..
                },
                _,
            ) => p.woken_at != Some(*n),
            _ => false,
        };
        let c = cause_of_read(&rec.reads[i]);
        let kept = |j: usize| kept_first.map_or(j >= own_reads, |n| j < n);
        // (a run that parks a state: the state rests on its own reads as
        // on those it kept, and its next run, offered it, is told only
        // the causes found here; each read that moved is one of them. Told
        // the first alone, a loop rest whose fold moved and whose start
        // page's give-back moved too went on from that state, its read of
        // the give-back kept, unchecked as one woken by the answer it
        // parked on, and stood on the routine's earlier run: a float and
        // its links on argumentation-doc's page 9, 2 to 7 builds in 60 at
        // -j4, accl 24790, 25034)
        let rests = |j: usize| kept(j) || parks;
        if kept(i) {
            stale = !range(&rec.reads[i].kind, &c);
        }
        causes.push(c);
        if rests(i) {
            let mut memo = read::FoldMemo::new();
            for (j, r) in rec.reads.iter().enumerate() {
                if !rests(j) || j == i || stale {
                    continue;
                }
                // (as the check above: a field no change has counted
                // since it was read stands; a sibling's read, answered
                // only in turn with the ones before it, is taken as
                // moved)
                let moved = match r.kind {
                    ReadKind::Following { .. } => kept(j),
                    _ if !scope_moved
                        && !matches!(r.kind, ReadKind::Next { .. })
                        && r.kind.field().is_some_and(|f| eng.field_epoch(f) == r.epoch) =>
                    {
                        false
                    }
                    _ => !read::holds(eng, node, r, &mut memo),
                };
                if !moved {
                    continue;
                }
                let c = cause_of_read(r);
                if !kept(j) || range(&r.kind, &c) {
                    causes.push(c);
                } else {
                    stale = true;
                }
            }
        }
    }
    // (a kept read the run read again with another answer, `replaced_moved`:
    // the state it resumed from rested on the old answer, as for a kept
    // read that moved: a range from a node or a read at a node names
    // where, and the next run, offered the state, goes back before it,
    // unless the run was told to go back before that node, or (a `next`)
    // before the contribution it answers, from where it read it afresh;
    // from the unit's start, the state is stale)
    for k in &replaced_moved {
        match k {
            ReadKind::Next { f, after: Some(n) } => {
                let answer = read::on_one_layout(eng, || {
                    read::pos_of(eng, *n).and_then(|at| read::next_contribution(eng, *f, Some(at), node.id).stop)
                });
                if p.woken_at != Some(*n) && (answer.is_none() || p.woken_at != answer) {
                    causes.push(cause_of(k));
                }
            }
            ReadKind::Fold { from: Some(n), .. }
            | ReadKind::Field {
                at: Anchor::Before(n) | Anchor::AfterOwn(n),
                ..
            } => {
                if p.woken_at != Some(*n) {
                    causes.push(cause_of(k));
                }
            }
            ReadKind::Fold { from: None, .. } | ReadKind::Next { after: None, .. } => {
                stale = true;
                causes.push(cause_of(k));
            }
            _ => {}
        }
    }
    node.state_stale.store(stale, Ordering::Release);
    // (a read at a node gone: for a run that parks a state, the change
    // lies where that node stood, so the next run, resuming from it, goes
    // back before it; the read, which nothing asks again, is not kept by
    // a run after. For any other, the structure moved)
    if p.state.is_some() {
        for (f, n) in anchor_gone {
            causes.push(Cause::Changed {
                field: f,
                site: Some(n),
            });
        }
    } else if causes.is_empty() && !anchor_gone.is_empty() {
        causes.push(Cause::Structure);
    }
    // (a read of a field whose definition reaching here is a settling
    // operation's, by a node of another: held to the quiet point as a
    // walk's wake of it is, `sched::hold_wake`; the rest re-ask the run
    // now)
    for cause in causes {
        let by = match &cause {
            Cause::Changed { field, .. } => read::reaching_at(eng, *field, node.id, false).and_then(|r| r.by),
            _ => None,
        };
        // (its definer's walk came before this run filed the read: it
        // moved the read all the same)
        crate::sched::note_move(eng, by, node, &cause);
        let at_once = |by: NodeId| match &cause {
            Cause::Changed { field, .. } => eng
                .placed_node(by)
                .is_some_and(|b| eng.op(b.op()).settles_now(*field, eng.values())),
            _ => false,
        };
        if let Some(by) = by
            && !at_once(by)
            && crate::sched::settles_edge(eng, by, node)
        {
            let cause = match cause {
                Cause::Changed { field, .. } => Cause::Changed { field, site: Some(by) },
                c => c,
            };
            crate::sched::hold_wake(
                eng,
                ww,
                node.id,
                Wake {
                    by: Some(by),
                    cause,
                    at_ns: eng.now_ns(),
                },
            );
            continue;
        }
        mark_dirty(
            eng,
            ww,
            node,
            Wake {
                by: None,
                cause,
                at_ns: eng.now_ns(),
            },
        );
    }
    st.reask_ns = ns(t0).saturating_sub(st.commit_ns);
    st
}

/// A filed reader's serial: it stands from its node's `filed_floor` to
/// the node's serial ([`crate::segment::live`]), not at its own alone.
pub(crate) const FILED: u32 = 1 << 31;

/// Whether a read's reader stays filed across the commits of runs going
/// on from it ([`crate::node::ReadRec::filed`]): not a search that met a
/// hole or the end, which the next run makes again, nor the reads that
/// are not filed as readers.
fn stays_filed(r: &crate::node::ReadRec) -> bool {
    match r.kind {
        ReadKind::Field { .. } | ReadKind::Env { .. } | ReadKind::Fold { .. } => true,
        ReadKind::Next { .. } => r.seen.is_some(),
        ReadKind::Placed { .. } | ReadKind::Following { .. } | ReadKind::Hole => false,
    }
}

/// The first node of `n`'s unit.
fn first_of_unit(eng: &Engine, n: &Node) -> Option<NodeId> {
    let mut cur = eng.seg_for_pos(read::unit_start(eng, n)).map(|s| s.id);
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != n.unit && seg.lo > read::unit_start(eng, n) {
            return None;
        }
        if seg.unit == n.unit
            && let Some((_, &x)) = seg.inner.read().nodes.iter().next()
        {
            return Some(x);
        }
        cur = eng.order.next(sid, seg.lo);
    }
    None
}

/// What the lock decided of a walk from a changed definition.
enum Here {
    /// It stopped in the segment.
    Done,
    /// The segment has no definition of the field after `by`'s: on in later
    /// segments.
    On(NodeId),
    /// The next relation's input is in an earlier segment: the walk goes on
    /// from `by`'s definition after the lock.
    From(NodeId),
    /// It stopped at a node that has not committed (`first_hole`).
    Stopped(NodeId),
}

/// The walk forward (`reask_from`) within the committing node's segment,
/// under its write lock: readers whose answer changed go to `wake` (as
/// (reader, definer, field)), those whose answer is in an earlier segment
/// to `later`, and a relation met is recomputed in place.
#[allow(clippy::too_many_arguments)]
fn walk_here(
    eng: &Engine,
    inner: &mut Inner,
    f: Field,
    from: Pos,
    by: NodeId,
    wake: &mut Vec<(NodeId, NodeId, Field)>,
    later: &mut Vec<(Field, Reader, NodeId)>,
    st: &mut CommitStats,
) -> Here {
    // (under scopes the next definition may not hide this one, and an
    // answer may lie past a close: walked after the lock)
    if read::scoped(eng) {
        return Here::From(by);
    }
    let v = eng.values();
    let op = eng.node(by).op();
    let (mut from, mut by) = (from, by);
    loop {
        let mut next = Segment::next_def(inner, f, from).map(|(p, d)| (p, *d));
        let hole = first_hole(eng, inner, op, from, next.map(|x| x.0));
        if hole.is_some() {
            next = None;
        }
        let stop = hole.map(|h| h.0).or(next.map(|x| x.0));
        if let Some(rs) = inner.readers.get(&f) {
            st.walked += rs.len() as u64;
            for r in rs {
                let ap = eng.node(r.anchor).pos_hint();
                let inside = (ap > from || (ap == from && r.inclusive))
                    && stop.is_none_or(|s| ap < s || (ap == s && !r.inclusive));
                if !inside || !live(eng, r.node, r.serial) {
                    continue;
                }
                let here = (inner.nodes.get(&ap) == Some(&r.anchor))
                    .then(|| Segment::reaching(inner, f, ap, r.inclusive))
                    .flatten();
                // (one read through the client's projection: asked again
                // after the lock, `read::client_path`)
                match here {
                    Some((_, d)) if !read::client_path(eng, r.path) => {
                        let now = through(eng, Reaching::of(&d.plain(&eng.values)), r.path);
                        if now != r.seen {
                            note_refute(eng, f, r.node, Some(by), r.seen, now);
                            wake.push((r.node, by, f));
                        }
                    }
                    _ => later.push((f, r.clone(), by)),
                }
            }
        }
        if let Some((_, h)) = hole {
            return Here::Stopped(h);
        }
        let Some((p, d)) = next else {
            return Here::On(by);
        };
        let Some(c) = d.rel else {
            return Here::Done;
        };
        let Some((_, i)) = Segment::reaching(inner, f, p, false) else {
            return Here::From(by);
        };
        // (an alias's input, `read::through_aliases`, is not followed under
        // the lock: the relation's own settle puts it right, `settle_rel`)
        let nv = v.int(v.as_int(i.value).unwrap_or(0).wrapping_add(c));
        if nv == d.value {
            return Here::Done;
        }
        if let Some(e) = inner.defs.get_mut(&f).and_then(|m| m.get_mut(&p)) {
            e.value = nv;
        }
        eng.bump(f);
        st.relative_resolved += 1;
        by = d.by;
        from = p;
    }
}

/// A new node's provisional relations put right from their true input
/// (the one earlier segment `file_predicted` could not look at), and their
/// readers re-asked.
pub(crate) fn settle_predictions(eng: &Engine, w: Option<&Worker>, n: &Arc<Node>, st: &mut CommitStats) {
    if n.has_brackets.load(Ordering::Acquire) {
        let (b, _) = unpack_effect(n.effect.load(Ordering::Acquire));
        reask_after_effect(eng, w, n, n.pos_hint(), Brackets::default(), b, st);
    }
    let Some(rec) = n.record() else {
        return;
    };
    if !rec.provisional {
        return;
    }
    for d in &rec.defs {
        if let Some(c) = d.rel {
            settle_rel(eng, d.f, n.id, c);
        }
        reask(eng, w, d.f, n.pos_hint(), n.seg_hint(), n.id, st);
    }
}

/// Recomputes `by`'s relation `f := input + c` until a read of the input
/// after the write still gives the value written: a walk that read an
/// older input and wrote after one that read a newer is put right by its
/// own read after. Whether the filed value changed.
pub(crate) fn settle_rel(eng: &Engine, f: Field, by: NodeId, c: i64) -> bool {
    let n = eng.node(by);
    let mut changed = false;
    loop {
        let nv = rel_value_at(eng, f, by, c);
        let Some((_, mut inner)) = own_write(eng, n) else {
            return changed;
        };
        let p = n.pos_hint();
        match inner.defs.get_mut(&f).and_then(|m| m.get_mut(&p)) {
            Some(e) if e.by == by && e.rel == Some(c) => {
                if e.value != nv {
                    e.value = nv;
                    changed = true;
                    eng.bump(f);
                }
            }
            _ => return changed,
        }
        drop(inner);
        if rel_value_at(eng, f, by, c) == nv {
            return changed;
        }
    }
}

/// Re-asks a parent that waits for this placement's commit
/// (`Ctx::placed_output_wait`), whatever its output.
fn wake_waiting(eng: &Engine, w: Option<&Worker>, parent: &Arc<Node>, child: NodeId, st: &mut CommitStats) {
    let Some(r) = parent.run_record() else {
        return;
    };
    if r.waits.iter().any(|&j| r.placed.get(j) == Some(&child))
        && mark_stale(
            eng,
            w,
            parent,
            Wake {
                by: Some(child),
                cause: Cause::Output { of: child },
                at_ns: eng.now_ns(),
            },
        )
    {
        st.woken += 1;
    }
}

/// Re-asks the parent's read of a placement's output.
fn reask_parent(eng: &Engine, w: Option<&Worker>, parent: &Arc<Node>, child: NodeId, st: &mut CommitStats) {
    let Some(r) = parent.run_record() else {
        return;
    };
    // (its reads of placements, not its placements: a placer of thousands
    // reads few of them)
    let mut out = None;
    for rd in &r.reads {
        if let ReadKind::Placed { i: j, guess } = rd.kind
            && r.placed.get(j) == Some(&child)
            && let o = *out.get_or_insert_with(|| eng.node(child).run_record().map(|x| x.output))
            && !(guess && o.is_none())
            && read::apply_path(eng, o, rd.path) != rd.seen
        {
            if mark_stale(
                eng,
                w,
                parent,
                Wake {
                    by: Some(child),
                    cause: Cause::Output { of: child },
                    at_ns: eng.now_ns(),
                },
            ) {
                st.woken += 1;
            }
            return;
        }
    }
}

/// The walk forward from a changed definition of `f` at `from` in `seg`:
/// readers of `f` anchored after `from`, in `seg` and then in each later
/// segment the field index does not let it skip, up to the next definition
/// of `f` that hides it. Each reader whose answer through its path differs
/// from what it saw is marked dirty (`sched::mark_dirty`). A relation met
/// as the next definition is recomputed in place (one short write lock)
/// and, if its value changed, the walk goes on past it: the cascade of a
/// counter chain.
pub(crate) fn reask(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    from: Pos,
    seg: SegId,
    by: NodeId,
    st: &mut CommitStats,
) {
    let level = def_level(eng, f, by);
    reask_from(eng, w, f, from, seg, Some(by), Some(eng.node(by).op()), level, st);
}

/// The scope depth of `by`'s definition of `f` where it stands now (0
/// where it has none, or nothing on the line has brackets).
pub(crate) fn def_level(eng: &Engine, f: Field, by: NodeId) -> u16 {
    if !read::scoped(eng) {
        return 0;
    }
    let Some(n) = eng.placed_node(by) else {
        return 0;
    };
    loop {
        if n.state() == State::Gone {
            return 0;
        }
        let seg = eng.seg(n.seg_hint());
        let start = read::chain_start(eng, seg);
        let inner = seg.inner.read();
        let p = n.pos_hint();
        if inner.nodes.get(&p) != Some(&by) {
            continue;
        }
        // (a multi definition's widest part: the walk goes as far as it)
        return read::levels(&eng.values, &inner, f, &start, Some(p), true)
            .iter()
            .filter(|x| x.0 == p && x.1.by == by)
            .map(|x| u16::try_from(x.2.0).unwrap_or(u16::MAX))
            .min()
            .unwrap_or(0);
    }
}

/// The first definition of `f` in the segment after `from` that hides one
/// at scope depth `level` from every later reader (one at that depth or
/// shallower: whatever sees the walked one sees it), with its depth, and
/// the deeper ones before it, which the walk passes: a reader past their
/// scope's close sees the walked one again.
#[allow(clippy::type_complexity)]
fn next_hiding(
    v: &Values,
    inner: &Inner,
    f: Field,
    start: &[u64],
    from: Pos,
    level: u16,
) -> (Option<(Pos, Def, u16)>, Vec<(Pos, Def)>) {
    let mut deeper = Vec::new();
    for (q, d, lv) in read::levels(v, inner, f, start, None, false) {
        if q <= from {
            continue;
        }
        let k = u16::try_from(lv.0).unwrap_or(u16::MAX);
        if k <= level {
            return (Some((q, d, k)), deeper);
        }
        deeper.push((q, d));
    }
    (None, deeper)
}

/// `reask` from a label with no defining node (the environment: `by`
/// None), or from a node's definition at scope depth `level`. With
/// `stop_op`, the walk also stops at a node of that operation that has
/// not committed (`first_hole`). A walk that reaches the end of the line
/// without passing a definition of `f` was from the last one: the
/// loop-back readers of `f` are asked again (`reask_final`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn reask_from(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    from: Pos,
    seg: SegId,
    by: Option<NodeId>,
    stop_op: Option<OpId>,
    level: u16,
    st: &mut CommitStats,
) {
    let scoped = read::scoped(eng);
    let (mut cur, mut from, mut by, mut level) = (seg, from, by, level);
    let mut passed = false;
    let trace = walk_trace(eng, f);
    if trace {
        eprintln!(
            "walk-trace: from {} seg {} by {:?} stop_op {:?} level {level}",
            from.0,
            seg.0,
            by.map(NodeId::raw),
            stop_op
        );
    }
    loop {
        let s = eng.seg(cur);
        let start = scoped.then(|| read::chain_start(eng, s));
        let (readers, next, hole, deeper) = {
            let inner = s.inner.read();
            if let Some(b) = by {
                let byn = eng.node(b);
                if byn.seg_hint() == cur && inner.nodes.get(&byn.pos_hint()) == Some(&b) {
                    from = byn.pos_hint();
                }
            }
            let (next, mut deeper) = match &start {
                None => (Segment::next_def(&inner, f, from).map(|(p, d)| (p, *d, 0)), Vec::new()),
                Some(c) => next_hiding(&eng.values, &inner, f, c, from, level),
            };
            let hole = stop_op.and_then(|op| first_hole(eng, &inner, op, from, next.map(|x| x.0)));
            let stop = hole.map(|h| h.0).or(next.map(|x| x.0));
            deeper.retain(|x| stop.is_none_or(|s| x.0 < s));
            if trace {
                eprintln!(
                    "walk-trace:   seg {cur:?} holds readers of the field: [{}]",
                    inner.readers.get(&f).map_or(String::new(), |rs| rs
                        .iter()
                        .map(|r| format!(
                            "{}@{}{}",
                            r.node.raw(),
                            eng.node(r.anchor).pos_hint().0,
                            if live(eng, r.node, r.serial) { "" } else { "x" }
                        ))
                        .collect::<Vec<_>>()
                        .join(" "))
                );
            }
            let readers: Vec<Reader> = match inner.readers.get(&f) {
                Some(rs) => {
                    st.walked += rs.len() as u64;
                    rs.iter()
                        .filter(|r| {
                            let ap = eng.node(r.anchor).pos_hint();
                            (ap > from || (ap == from && r.inclusive))
                                && stop.is_none_or(|s| ap < s || (ap == s && !r.inclusive))
                        })
                        .cloned()
                        .collect()
                }
                None => Vec::new(),
            };
            (readers, next, hole, deeper)
        };
        if trace {
            eprintln!(
                "walk-trace:   seg {cur:?} lo {} from {} readers [{}] next {:?} hole {:?} deeper {}",
                s.lo.0,
                from.0,
                readers
                    .iter()
                    .map(|r| r.node.raw().to_string())
                    .collect::<Vec<_>>()
                    .join(" "),
                next.map(|x| (x.0.0, x.1.by.raw(), x.2)),
                hole.map(|h| (h.0.0, h.1.raw())),
                deeper.len()
            );
        }
        for r in &readers {
            reask_reader(eng, w, f, r, by, st);
        }
        if !deeper.is_empty() {
            passed = true;
            // (a deeper relation counts on from this one: its input moved)
            for (_, d) in &deeper {
                if let Some(c) = d.rel
                    && settle_rel(eng, f, d.by, c)
                {
                    st.relative_resolved += 1;
                    let dn = eng.node(d.by);
                    reask(eng, w, f, dn.pos_hint(), dn.seg_hint(), d.by, st);
                }
            }
        }
        if let Some((_, h)) = hole {
            stop_at(eng, w, h, f, level, st);
            return;
        }
        match next {
            Some((_, d, k)) => {
                let Some(c) = d.rel else {
                    return;
                };
                if !settle_rel(eng, f, d.by, c) {
                    return;
                }
                st.relative_resolved += 1;
                by = Some(d.by);
                level = k;
                passed = false;
            }
            None => {
                let next = next_with(eng, f, s.lo);
                // (the walk passes the end of this segment's unit: its
                // readers at that end)
                if next.is_none_or(|x| eng.seg(x).unit != s.unit) {
                    reask_ends(eng, w, f, s.unit, by, st);
                }
                let Some(next) = next else {
                    if !passed {
                        reask_final(eng, f);
                    }
                    return;
                };
                cur = next;
                from = Pos(eng.seg(cur).lo.0.saturating_sub(1));
            }
        }
    }
}

/// (`PHITEX_WALK_TRACE=<field>`, profile) Whether walks of `f` are
/// printed: where each starts and, segment by segment, the readers it
/// asks and where it stops.
fn walk_trace(eng: &Engine, f: Field) -> bool {
    static W: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    crate::obs::enabled()
        && W.get_or_init(|| std::env::var("PHITEX_WALK_TRACE").ok())
            .as_deref()
            .is_some_and(|x| eng.values().render(f.0) == format!("{x:?}") || eng.values().render(f.0) == x)
}

/// The first segment after the one starting at `lo` with a definition or
/// a reader of `f`.
fn next_with(eng: &Engine, f: Field, lo: Pos) -> Option<SegId> {
    let after = Pos(lo.0 + 1);
    let nd = eng.defs.first_at_or_after(f, after);
    let nr = eng.readers.first_at_or_after(f, after);
    match (nd, nr) {
        (None, None) => None,
        (Some(a), None) => Some(a.1),
        (None, Some(b)) => Some(b.1),
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a.1 } else { b.1 }),
    }
}

/// The walk forward of a definition of `f` by `by`, the last of its
/// segment (starting at `lo`), on from the next segment; at the end of
/// the line, the loop-back readers.
fn reask_later(eng: &Engine, w: Option<&Worker>, f: Field, seg: SegId, lo: Pos, by: NodeId, st: &mut CommitStats) {
    let next = next_with(eng, f, lo);
    let unit = eng.seg(seg).unit;
    if next.is_none_or(|x| eng.seg(x).unit != unit) {
        reask_ends(eng, w, f, unit, Some(by), st);
    }
    if let Some(cur) = next {
        let op = eng.node(by).op();
        reask_from(
            eng,
            w,
            f,
            Pos(eng.seg(cur).lo.0.saturating_sub(1)),
            cur,
            Some(by),
            Some(op),
            0,
            st,
        );
    } else {
        reask_final(eng, f);
    }
}

/// The last definition of `f` landed, changed or went: its published
/// value is checked at the next quiet point (`publish_finals`).
pub(crate) fn reask_final(eng: &Engine, f: Field) {
    if eng.has_finals.load(Ordering::Acquire) {
        eng.ranges.mark_dirty(f);
    }
}

/// [`reask_final`] for every field published (a walk of every reader).
pub(crate) fn reask_final_all(eng: &Engine) {
    if eng.has_finals.load(Ordering::Acquire) {
        eng.ranges.mark_all_dirty();
    }
}

/// At a quiet point (nothing queued or running), on one thread: each
/// loop-back value marked since the last (`reask_final`) published again
/// as the field's last definition now, then the readers whose answer
/// moved re-asked (`sched::allow_final_rerun`: past the limit one is left
/// as it is). The readers of their own definition first, alone: a file's
/// node keeping what it wrote and running the tools on it, as latexmk runs
/// makeindex between two runs, its new definitions published at the next
/// quiet point; then, with none of those left, the readers of others'
/// definitions, the next pass of the loop. The readers re-asked, and
/// whether they were a pass.
pub(crate) fn publish_finals(eng: &Engine, w: &Worker) -> (usize, bool) {
    if !eng.has_finals.load(Ordering::Acquire) {
        return (0, false);
    }
    for f in eng.ranges.take_dirty() {
        eng.ranges.republish(f, read::last_definition(eng, f));
    }
    let mut asks: Vec<(Field, read::Reaching, Reader)> = Vec::new();
    for f in eng.ranges.pending(false) {
        let last = read::final_reaching(eng, f);
        for r in eng.ranges.finals(f, |r| live(eng, r.node, r.serial)) {
            if through(eng, last, r.path) != r.seen && eng.node(r.node).state() == State::Clean {
                asks.push((f, last, r));
            }
        }
    }
    let ask = |own: bool| {
        let mut k = 0;
        for (f, last, r) in &asks {
            if (last.by == Some(r.node)) != own {
                continue;
            }
            let n = eng.node(r.node);
            if n.state() != State::Clean || !allow_final_rerun(eng, w, n) {
                continue;
            }
            let wake = Wake {
                by: last.by,
                cause: Cause::Changed {
                    field: *f,
                    site: last.by,
                },
                at_ns: eng.now_ns(),
            };
            // (a quiet point's own wake, never held: holding it would leave
            // the reader for a quiet point that never comes)
            if crate::sched::mark_stale_now(eng, Some(w), n, wake) {
                k += 1;
            }
        }
        k
    };
    let k = ask(true);
    if k > 0 {
        return (k, false);
    }
    eng.ranges.pending(true);
    let k = ask(false);
    (k, k > 0)
}

/// The first node in `(from, to)` of operation `op` that has not
/// committed: a walk of `f` from a definition by `op` stops there, as at a
/// definition, and `stop_at` files `f` with it to be walked on when it
/// commits, is taken or is removed. A node of the same operation likely
/// defines the same fields, and its readers would be re-asked once more by
/// its own commit: a reader far ahead of a chain of definers is re-asked
/// once, not once per definer. Under the segment's lock.
fn first_hole(eng: &Engine, inner: &Inner, op: OpId, from: Pos, to: Option<Pos>) -> Option<(Pos, NodeId)> {
    if inner.holes == 0 {
        return None;
    }
    let hi = to.map_or(Unbounded, Excluded);
    inner
        .nodes
        .range((Excluded(from), hi))
        .find(|(_, id)| {
            let n = eng.node(**id);
            n.op() == op && n.is_hole()
        })
        .map(|(p, id)| (*p, *id))
}

/// Files `f` with the node a walk stopped at, with the depth of the
/// definition walked from, after the walk's lock. A node that has
/// committed (or gone) meanwhile has taken its list already: the walk
/// goes on from it here.
fn stop_at(eng: &Engine, w: Option<&Worker>, h: NodeId, f: Field, level: u16, st: &mut CommitStats) {
    let n = eng.node(h);
    {
        let mut s = n.stopped.lock();
        match s.iter_mut().find(|x| x.0 == f) {
            Some(x) => x.1 = x.1.min(level),
            None => s.push((f, level)),
        }
    }
    if !n.is_hole() {
        walk_stopped(eng, w, n, n.pos_hint(), n.seg_hint(), &[], st);
    }
}

/// The walks that stopped at `node` (`first_hole`), on from it, but for
/// the fields it defines at the depth walked from or shallower (its own
/// walks hide those).
fn walk_stopped(
    eng: &Engine,
    w: Option<&Worker>,
    node: &Node,
    pos: Pos,
    seg: SegId,
    defined: &[(Field, u16)],
    st: &mut CommitStats,
) {
    wake_waiters(eng, w, node, st);
    let fs = std::mem::take(&mut *node.stopped.lock());
    for (f, level) in fs {
        if walk_trace(eng, f) {
            eprintln!(
                "walk-trace: stopped walks at node {} go on from {} (level {level}; its own: {})",
                node.id.raw(),
                pos.0,
                defined.iter().any(|&(g, k)| g == f && k <= level)
            );
        }
        if !defined.iter().any(|&(g, k)| g == f && k <= level) {
            reask_from(eng, w, f, pos, seg, Some(node.id), None, level, st);
        }
    }
}

/// Re-asks the `At::End(u)` readers of `f` whose answer moved: a walk
/// of `f` passed unit `u`'s end (no definition hid the one walked from
/// before it, or one went).
fn reask_ends(eng: &Engine, w: Option<&Worker>, f: Field, u: UnitId, by: Option<NodeId>, st: &mut CommitStats) {
    let rs = eng.ranges.ends(f, u, |x| live(eng, x.node, x.serial));
    if rs.is_empty() {
        if ftrace(eng, f) {
            eprintln!(
                "ftrace: end-walk {} by {} no reader t {}",
                eng.values().as_str(f.0).unwrap_or_default(),
                by.map_or(0, NodeId::raw),
                eng.now_ns()
            );
        }
        return;
    }
    let now = read::on_one_layout(eng, || read::reaching_visible(eng, f, Pos(eng.unit(u).end.0 - 1), true));
    let cause = match by {
        Some(b) => Cause::Changed {
            field: f,
            site: Some(b),
        },
        None => Cause::Environment { field: f },
    };
    let tr = ftrace(eng, f);
    for r in rs {
        if tr {
            let n = eng.node(r.node);
            eprintln!(
                "ftrace: end-walk {} by {} reader {} serial {} (now {}) state {:?} seen {} now {} t {}",
                eng.values().as_str(f.0).unwrap_or_default(),
                by.map_or(0, NodeId::raw),
                r.node.raw(),
                r.serial,
                n.serial.load(Ordering::Acquire),
                n.state(),
                r.seen.map_or(0, ValueId::raw),
                through(eng, now, r.path).map_or(0, ValueId::raw),
                eng.now_ns()
            );
        }
        if through(eng, now, r.path) != r.seen
            && mark_stale(
                eng,
                w,
                eng.node(r.node),
                Wake {
                    by,
                    cause: cause.clone(),
                    at_ns: eng.now_ns(),
                },
            )
        {
            st.woken += 1;
        }
    }
}

/// (`PHITEX_FIELD_TRACE=<prefix>`, observe) Whether `f`'s name begins with
/// the prefix: its definitions, committed reads, end walks and removals
/// are printed (`ftrace:` lines).
pub(crate) fn ftrace(eng: &Engine, f: Field) -> bool {
    static P: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    crate::obs::enabled()
        && P.get_or_init(|| std::env::var("PHITEX_FIELD_TRACE").ok())
            .as_deref()
            .is_some_and(|p| eng.values().as_str(f.0).is_some_and(|s| s.starts_with(p)))
}

/// (`PHITEX_REFUTE_LOG`, observe) A read refuted: the field, the reader,
/// the definer whose value moved it, the value the reader saw and the one
/// it would see now; one tab-separated line, for the scaling report's
/// table of what speculation guessed and what came.
fn refute_log() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| crate::obs::enabled() && std::env::var_os("PHITEX_REFUTE_LOG").is_some())
}

/// (`PHITEX_KREAD_KEYS`) The fields whose committed reads are printed;
/// empty for `*`.
fn kread_keys() -> &'static [String] {
    static K: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    K.get_or_init(|| {
        std::env::var("PHITEX_KREAD_KEYS").map_or_else(
            |_| vec!["\u{0}".to_owned()],
            |v| {
                if v == "*" {
                    Vec::new()
                } else {
                    v.split('|').map(str::to_owned).collect()
                }
            },
        )
    })
}

fn note_refute(
    eng: &Engine,
    f: Field,
    reader: NodeId,
    by: Option<NodeId>,
    seen: Option<ValueId>,
    now: Option<ValueId>,
) {
    if !refute_log() {
        return;
    }
    // (where in the reader's last run the field was first read: the share
    // of its reads before it, what a resumption from there would keep)
    let at = eng.node(reader).record().map_or(String::from("-"), |r| {
        let k = r
            .reads
            .iter()
            .position(|x| matches!(&x.kind, ReadKind::Field { f: g, .. } if *g == f))
            .unwrap_or(r.reads.len());
        format!("{k}/{}", r.reads.len())
    });
    let v = eng.values();
    let show = |x: Option<ValueId>| {
        x.map_or_else(
            || "-".to_owned(),
            |x| {
                v.render(x)
                    .chars()
                    .take(160)
                    .collect::<String>()
                    .replace(['\t', '\n'], " ")
            },
        )
    };
    let op = |n: NodeId| eng.op(eng.node(n).op()).name();
    eprintln!(
        "refute:\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{at}\t{}\t{}",
        v.as_str(f.0).unwrap_or_default(),
        reader.raw(),
        op(reader),
        by.map_or(0, NodeId::raw),
        by.map_or("-", op),
        by.map_or(0, |b| eng.node(b).runs.load(std::sync::atomic::Ordering::Relaxed)),
        u8::from(seen == now),
        show(seen),
        show(now),
        seen.map_or(0, ValueId::raw),
        now.map_or(0, ValueId::raw)
    );
}

fn reask_reader(eng: &Engine, w: Option<&Worker>, f: Field, r: &Reader, by: Option<NodeId>, st: &mut CommitStats) {
    if walk_trace(eng, f) {
        let alive = live(eng, r.node, r.serial);
        let now = if alive { reader_answer(eng, f, r) } else { None };
        eprintln!(
            "walk-trace:     reader {} serial {} anchor {} live {alive} seen {} now {}",
            r.node.raw(),
            r.serial,
            r.anchor.raw(),
            r.seen.map_or("-".to_owned(), |x| eng.values().render(x)),
            now.map_or("-".to_owned(), |x| eng.values().render(x))
        );
    }
    if !live(eng, r.node, r.serial) {
        return;
    }
    let cause = match by {
        Some(b) => Cause::Changed {
            field: f,
            site: Some(b),
        },
        None => Cause::Environment { field: f },
    };
    let now = reader_answer(eng, f, r);
    if now != r.seen {
        note_refute(eng, f, r.node, by, r.seen, now);
    }
    // (a derived read that stands where its name moved: the field named
    // now gets a witness, so its own change re-asks the reader; then it is
    // asked again, for a definition of that field landing meanwhile, whose
    // walk passed before the witness was there)
    let now = if now == r.seen
        && let Some(spec) = read::derived_spec(eng, r.path)
        && let Some(g) = read::derived_field_at(eng, spec, r.anchor, r.inclusive)
        && g != f
        && file_witness(eng, g, r)
    {
        reader_answer(eng, g, r)
    } else {
        now
    };
    if now != r.seen
        && mark_stale(
            eng,
            w,
            eng.node(r.node),
            Wake {
                by,
                cause,
                at_ns: eng.now_ns(),
            },
        )
    {
        st.woken += 1;
    }
}

/// Files reader `r` again on field `g` (a derived read's witness,
/// `Step::Derived`), in the segment its anchor stands in, unless the node's
/// record is filed there already; under that segment's lock, the reader
/// index with it. False where the anchor is gone or it was there.
fn file_witness(eng: &Engine, g: Field, r: &Reader) -> bool {
    let a = eng.node(r.anchor);
    loop {
        if a.state() == State::Gone {
            return false;
        }
        let seg = eng.seg(a.seg_hint());
        let mut inner = seg.inner.write();
        if inner.nodes.get(&a.pos_hint()) != Some(&r.anchor) {
            continue;
        }
        if inner.readers.get(&g).is_some_and(|rs| {
            rs.iter()
                .any(|x| x.node == r.node && x.serial == r.serial && x.path == r.path)
        }) {
            return false;
        }
        let mut spare = Vec::new();
        if file_reader(eng, &mut inner, g, r.clone(), &mut spare) {
            eng.readers.add(g, seg.lo, seg.id);
        }
        return true;
    }
}

/// Re-asks the readers of folds over `f` whose range covers `at`, `by`'s
/// label (its last where it is gone), but the `next` readers stopped at
/// `by` (`stops`, taken from it): the change re-asks those after, with
/// its stops.
pub(crate) fn reask_ranges(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    at: Pos,
    by: NodeId,
    stops: &[(Field, RangeKey, RangeReader)],
    st: &mut CommitStats,
) {
    let at = read::pos_of(eng, by).unwrap_or(at);
    reask_ranges_in(eng, w, f, (at, by), false, stops, st);
}

/// `reask_ranges` at label `at` whatever `by`'s label is now (its old
/// one, `relocate`).
fn reask_ranges_at(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    at: Pos,
    by: NodeId,
    stops: &[(Field, RangeKey, RangeReader)],
    st: &mut CommitStats,
) {
    reask_ranges_in(eng, w, f, (at, by), true, stops, st);
}

/// The range readers holding `at` (the index's spans are segments',
/// which a relabel keeps) whose range covers `by` ([`covers`]),
/// re-asked; a `next` stopped at `by` (one of `stops`) is not: the
/// change re-asks it once, with the stops after. Re-asked here too, the
/// run the first re-ask woke could end and commit before the second,
/// which, asking the record that run replaces, ran it again for the same
/// change from before `by` (a page loop parked at a hole folded `by`,
/// then started its pages again from nothing: pages2's park test, a few
/// builds in a hundred with the CPUs loaded).
fn reask_ranges_in(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    (at, by): (Pos, NodeId),
    fixed: bool,
    stops: &[(Field, RangeKey, RangeReader)],
    st: &mut CommitStats,
) {
    let mut readers = eng.ranges.holding(f, at);
    // (in order of their ends: the folds of readers from one start go on
    // from one another's, `read::fold_in`)
    readers.sort_by_cached_key(|(_, r)| read::pos_of(eng, r.to.unwrap_or(r.node)).map_or(u64::MAX, |p| p.0));
    let mut memo = read::FoldMemo::new();
    let t0 = std::time::Instant::now();
    let held = readers.len();
    let mut asked = 0usize;
    let mut starts: Vec<Option<NodeId>> = Vec::new();
    if walk_trace(eng, f) {
        eprintln!(
            "walk-trace: ranges at {} by {} fixed {fixed}: [{}]",
            at.0,
            by.raw(),
            readers
                .iter()
                .map(|(k, r)| format!(
                    "{}(from {:?} to {:?} next {} incl {} live {} stop {} covers {})",
                    r.node.raw(),
                    r.from.map(NodeId::raw),
                    r.to.map(NodeId::raw),
                    r.next,
                    r.incl,
                    live(eng, r.node, r.serial),
                    stops.iter().any(|s| s.0 == f && s.1 == *k),
                    covers(eng, r, by, at, fixed)
                ))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
    for (k, r) in readers {
        if stops.iter().any(|s| s.0 == f && s.1 == k) {
            continue;
        }
        if covers(eng, &r, by, at, fixed) {
            asked += 1;
            if crate::obs::enabled() && !starts.contains(&r.from) {
                starts.push(r.from);
            }
            reask_range(eng, w, f, k, &r, by, true, st, &mut memo);
        }
    }
    // (`PHITEX_REASK_TRACE`, profile: a re-ask of 10 ms or more, its
    // field, the readers held, those asked and their starts)
    if crate::obs::enabled() && t0.elapsed().as_millis() >= 10 && std::env::var_os("PHITEX_REASK_TRACE").is_some() {
        eprintln!(
            "reask-trace: {} by {} at {}: {held} held, {asked} asked from {} starts, {:?}",
            eng.values().render(f.0),
            by.raw(),
            at.0,
            starts.len(),
            t0.elapsed()
        );
    }
}

/// Re-asks one range reader after `by`'s change. `filed`: a `next`'s
/// filing at the node it stopped at stands (a reader the range index
/// holds); not where that filing was taken (its stop committed, was
/// taken or went) or never made (`file_stop` found the stop not open).
#[allow(clippy::too_many_arguments)]
fn reask_range(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    k: RangeKey,
    r: &RangeReader,
    by: NodeId,
    filed: bool,
    st: &mut CommitStats,
    memo: &mut read::FoldMemo,
) {
    if !live(eng, r.node, r.serial) {
        return;
    }
    // (a reader queued to run, gone back before this change already: its
    // run reads it, and asking again changes nothing; the fold that would
    // say whether its answer moved is not made. openmoji: a run claiming
    // a sibling asked every `lines` reader after it, 4292 folds of every
    // file's counts at each of 7129 claims, accl 23273)
    if !r.next && crate::sched::wake_redundant(eng, eng.node(r.node), &Cause::Range { field: f, by }) {
        return;
    }
    // a `next` whose answer stands but whose search now stops further on
    // (the node it stopped at committed with nothing for it): filed again
    // up to its new stop, which re-asks it in turn
    if r.next {
        if !eng.ranges.has(f, k) {
            // (filed again since: that record answers)
            return;
        }
        let me = eng.node(r.node);
        // (the labels and the searches from them on one layout,
        // `read::on_one_layout`)
        let found = read::on_one_layout(eng, || {
            let from = match r.from {
                Some(n) => read::pos_of(eng, n),
                None => Some(read::unit_start(eng, me)),
            }?;
            // (asked because the node its search stopped at committed or
            // went: the search goes on from that node, not from its start;
            // a contribution made before it since asks again in full)
            let resume = (r.hole && r.to == Some(by))
                .then(|| read::pos_of(eng, by).map_or(eng.node(by).pos_hint(), |p| Pos(p.0 - 1)))
                .filter(|p| *p > from);
            let mut n = read::next_contribution(eng, f, Some(resume.unwrap_or(from)), r.node);
            let mut v = read::next_value(eng, &n);
            // (a node placed since before the one it stopped at, a
            // continuation made anew in place of a removed one, is found
            // only from the start: the answer asked again in full)
            if resume.is_some() && !r.seen.iter().all(|&(p, s)| read::apply_path(eng, v, p) == s) {
                n = read::next_contribution(eng, f, Some(from), r.node);
                v = read::next_value(eng, &n);
            }
            Some((n, v))
        });
        if let Some((n, v)) = found {
            if r.seen.iter().all(|&(p, s)| read::apply_path(eng, v, p) == s) {
                // (a search that stops at the same node again, its filing
                // there taken by that node's commit or never made because
                // the node was committing when it was filed, while the node
                // is open again: filed there again. Left unfiled, the
                // node's next commit re-asked nobody and the reader kept
                // its answer of none before a contribution: review's next
                // readers across splits, 52 of 4000 readers wrong at 16
                // threads, accl 22384)
                let refile = n.stop != r.to || (!filed && n.item.is_none() && n.stop.is_some());
                if refile && eng.ranges.remove(f, k) {
                    let mut again = r.clone();
                    again.to = n.stop;
                    again.hole = n.item.is_none() && n.stop.is_some();
                    file_ranges(eng, w, vec![(f, again)], by, st);
                    // (the frontier its reader set at that stop moves on with
                    // it: the reader is not re-asked, its answer stands, and
                    // a reader held at the frontier waited for a run of it
                    // that never came)
                    if let Some(to) = r.to
                        && n.stop != r.to
                        && eng.frontier_by.load(Ordering::Acquire) == r.node.raw() + 1
                        && eng
                            .frontier
                            .compare_exchange(
                                to.raw() + 1,
                                n.stop.map_or(0, |s| s.raw() + 1),
                                Ordering::AcqRel,
                                Ordering::Acquire,
                            )
                            .is_ok()
                    {
                        crate::sched::log_frontier(
                            eng,
                            to.raw() + 1,
                            n.stop.map_or(0, |s| s.raw() + 1),
                            Some(r.node),
                            "stop",
                        );
                        crate::sched::release_frontier(eng, w);
                    }
                }
                return;
            }
            // (the answer this search gave, not another search's: one made
            // after a node placed meanwhile may find the old answer again
            // with a stop the record does not hold)
            if mark_stale(
                eng,
                w,
                eng.node(r.node),
                Wake {
                    by: Some(by),
                    cause: Cause::Range { field: f, by },
                    at_ns: eng.now_ns(),
                },
            ) {
                st.woken += 1;
            }
            return;
        }
    }
    if !read::range_stands(eng, f, r, memo)
        && mark_stale(
            eng,
            w,
            eng.node(r.node),
            Wake {
                by: Some(by),
                cause: Cause::Range { field: f, by },
                at_ns: eng.now_ns(),
            },
        )
    {
        st.woken += 1;
    }
}

/// The `next` readers whose search stopped at `node` (`file_stop`), taken
/// out of its segment's list, under that segment's lock.
fn take_stops(inner: &mut Inner, node: NodeId) -> Vec<(Field, RangeKey, RangeReader)> {
    if inner.stops.is_empty() {
        return Vec::new();
    }
    let (mine, rest) = std::mem::take(&mut inner.stops)
        .into_iter()
        .partition(|s| s.2.to == Some(node));
    inner.stops = rest;
    mine
}

/// The `next` readers filed at `node` while it ran (`Node::late_stops`),
/// taken and re-asked once its run has ended and it is no longer open.
/// A hole stays open while it commits (`Node::is_open`), so a reader
/// whose search stopped at it then was filed after the commit had taken
/// the node's stops, and no commit of the node took that filing: the
/// reader kept its answer of none before the field's contribution
/// (review's next_readers_follow_their_stops_across_splits).
pub(crate) fn reask_late_stops(eng: &Engine, w: Option<&Worker>, node: &Node) {
    std::sync::atomic::fence(Ordering::SeqCst);
    if !node.late_stops.swap(false, Ordering::SeqCst) {
        return;
    }
    let stops = match own_write(eng, node) {
        Some((_, mut inner)) => take_stops(&mut inner, node.id),
        None => return,
    };
    if !stops.is_empty() {
        let mut st = CommitStats::default();
        reask_stops(eng, w, stops, node.id, &mut st);
    }
}

/// Re-asks the `next` readers whose search stopped at `by`: it has
/// committed, been taken or removed, so the first contribution after their
/// start may now be known.
fn reask_stops(
    eng: &Engine,
    w: Option<&Worker>,
    stops: Vec<(Field, RangeKey, RangeReader)>,
    by: NodeId,
    st: &mut CommitStats,
) {
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in stops {
        reask_range(eng, w, f, k, &r, by, false, st, &mut memo);
    }
}

/// The first relation `f := input + c` made by operation `op`: every live
/// node of `op` without a committed definition of `f` is given the same
/// relation as a provisional definition at its position, in order, each
/// computed from the one before. Readers of those positions are re-asked
/// and run on the predicted values; the nodes' own runs then confirm them
/// (same value: nobody re-asked) or replace them.
pub(crate) fn learn_shape(eng: &Engine, w: Option<&Worker>, op: OpId, f: Field, c: i64, st: &mut CommitStats) {
    {
        let mut sh = eng.shapes.write();
        if sh.contains_key(&(op, f)) {
            return;
        }
        sh.insert((op, f), c);
        eng.shapes_n.fetch_add(1, Ordering::Release);
    }
    let v = eng.values();
    let plus = |carried: Option<ValueId>, c: i64| v.int(carried.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c));
    let mut carried: Option<ValueId> = read::env_value(eng, f);
    let mut to_walk: Vec<(SegId, NodeId)> = Vec::new();
    for sid in eng.order.all() {
        let s = eng.seg(sid);
        let mut inner = s.inner.write();
        let mut pend: Vec<(Pos, NodeId)> = inner
            .by_op
            .get(&op)
            .map(|ns| {
                ns.iter()
                    .filter_map(|&n| {
                        let nd = eng.node(n);
                        nd.is_hole().then(|| (nd.pos_hint(), n))
                    })
                    .collect()
            })
            .unwrap_or_default();
        if pend.is_empty() {
            if let Some((_, d)) = inner.defs.get(&f).and_then(|m| m.iter().next_back()) {
                carried = Some(d.value);
            }
            continue;
        }
        pend.sort_unstable();
        let existing: Vec<(Pos, Def)> = inner
            .defs
            .get(&f)
            .map(|m| m.iter().map(|(p, d)| (*p, *d)).collect())
            .unwrap_or_default();
        let first = existing.is_empty();
        let (mut i, mut j) = (0, 0);
        let mut filed: Vec<(NodeId, ValueId)> = Vec::new();
        let m = inner.defs.entry(f).or_default();
        loop {
            let take_pending = match (pend.get(i), existing.get(j)) {
                (None, None) => break,
                (Some(&(pp, _)), Some(&(dp, _))) => pp < dp,
                (Some(_), None) => true,
                (None, Some(_)) => false,
            };
            if take_pending {
                let (pp, n) = pend[i];
                let nv = plus(carried, c);
                m.insert(
                    pp,
                    Def {
                        depth: 0,
                        value: nv,
                        by: n,
                        rel: Some(c),
                        provisional: true,
                        multi: false,
                    },
                );
                filed.push((n, nv));
                carried = Some(nv);
                eng.bump(f);
                i += 1;
            } else {
                let (dp, d) = existing[j];
                if pend.get(i).is_some_and(|&(pp, _)| pp == dp) {
                    i += 1;
                }
                if let Some(cc) = d.rel {
                    let nv = plus(carried, cc);
                    if nv != d.value {
                        if let Some(e) = m.get_mut(&dp) {
                            e.value = nv;
                        }
                        eng.bump(f);
                        st.relative_resolved += 1;
                        to_walk.push((sid, d.by));
                    }
                    carried = Some(nv);
                } else {
                    carried = Some(d.value);
                }
                j += 1;
            }
        }
        if first && !m.is_empty() {
            eng.defs.add(f, s.lo, sid);
        }
        for (n, nv) in filed {
            let nd = eng.node(n);
            let mut last = nd.last.lock();
            let mut defs = last.as_ref().map(|r| r.defs.clone()).unwrap_or_default();
            defs.retain(|d| d.f != f);
            defs.push(DefRec {
                f,
                loc: Locality::Local,
                value: nv,
                rel: Some(c),
            });
            *last = Some(Arc::new(RunRecord::provisional(defs)));
            st.shaped += 1;
            to_walk.push((sid, n));
        }
    }
    for (sid, n) in to_walk {
        reask(eng, w, f, eng.node(n).pos_hint(), sid, n, st);
    }
}

/// A new node's predictions, made before its segment's lock: its
/// operation's `predicted_defs`, and the relations its operation is known
/// to make (valued under the lock, `file_predicted`).
pub(crate) struct Predicted {
    /// The node's provisional record, made with its prediction (none
    /// without one): its definitions, those predicted a value first, then
    /// from `shaped` on the shapes, whose values are found when it is filed
    /// (`file_predicted`): nothing is made under the segment's lock.
    rec: Option<Arc<RunRecord>>,
    shaped: usize,
    /// Its operation's static effect, filed with the node.
    effect: Effect,
}

impl Predicted {
    fn defs(&self) -> &[DefRec] {
        self.rec.as_ref().map_or(&[], |r| &r.defs)
    }

    /// The fields it is predicted to define.
    pub(crate) fn fields(&self) -> impl Iterator<Item = Field> + '_ {
        self.defs().iter().map(|d| d.f)
    }

    /// Whether filing it files brackets.
    pub(crate) fn brackets(&self) -> bool {
        let b = self.effect.brackets();
        self.effect != Effect::Pending && (b.closes > 0 || b.opens > 0)
    }

    /// `defs`, those of a node this one replaces, for the fields its
    /// operation predicts no value for: a relation with the value it gave
    /// there (a shape's value is computed from the input in the node's
    /// segment only, and the commit that places it settles it from the
    /// true input, `settle_predictions`).
    fn inherit(&mut self, defs: &[DefRec]) {
        for d in defs {
            if self.defs()[..self.shaped].iter().any(|x| x.f == d.f) {
                continue;
            }
            let rec = Arc::get_mut(
                self.rec
                    .get_or_insert_with(|| Arc::new(RunRecord::provisional(Vec::new()))),
            )
            .expect("a prediction not filed yet");
            let shaped = self.shaped;
            let mut i = 0;
            rec.defs.retain(|x| {
                i += 1;
                i <= shaped || x.f != d.f
            });
            rec.defs.insert(shaped, *d);
            self.shaped += 1;
        }
    }
}

pub(crate) fn predict(eng: &Engine, op: OpId, args: &[ValueId]) -> Predicted {
    let mut defs: Vec<DefRec> = Vec::new();
    if let Some(pd) = eng.op(op).predicted_defs(args, eng.values()) {
        let settling = eng.op(op).settles_at_quiet();
        for (f, x) in pd {
            if settling {
                eng.mark_settled(f);
            }
            defs.push(DefRec {
                f,
                loc: Locality::Local,
                value: x,
                rel: None,
            });
        }
    }
    let mut shapes: Vec<(Field, i64)> = eng.op(op).predicted_rels(args, eng.values()).unwrap_or_default();
    shapes.retain(|s| !defs.iter().any(|d| d.f == s.0));
    let learned = if eng.shapes_n.load(Ordering::Acquire) == 0 {
        Vec::new()
    } else {
        eng.shapes
            .read()
            .iter()
            .filter(|((o, f), _)| *o == op && !defs.iter().any(|d| d.f == *f) && !shapes.iter().any(|s| s.0 == *f))
            .map(|((_, f), c)| (*f, *c))
            .collect::<Vec<_>>()
    };
    shapes.extend(learned);
    let effect = eng.op(op).effect(args, eng.values());
    let shaped = defs.len();
    defs.extend(shapes.into_iter().map(|(f, c)| DefRec {
        f,
        loc: Locality::Local,
        value: ValueId::UNIT,
        rel: Some(c),
    }));
    Predicted {
        rec: (!defs.is_empty()).then(|| Arc::new(RunRecord::provisional(defs))),
        shaped,
        effect,
    }
}

/// A node not yet on the line, and its predictions: all a placement needs
/// but a label.
pub(crate) fn built(
    eng: &Engine,
    unit: UnitId,
    parent: Option<NodeId>,
    op: OpId,
    args: Vec<ValueId>,
    id: NodeId,
) -> (Arc<Node>, Predicted) {
    let pred = predict(eng, op, &args);
    let n = Node::new(id, unit, op, args, parent, SegId(0), Pos(0));
    n.effect.store(pack_effect(pred.effect), Ordering::Release);
    (Arc::new(n), pred)
}

/// A new placement's brackets from its placer's run where its operation
/// leaves them to its own (`Ctx::predict_placed_effect`).
fn placed_effect(effects: &[(usize, Effect)], i: usize, n: &Node, pred: &mut Predicted) {
    if pred.effect != Effect::Pending {
        return;
    }
    if let Some(&(_, e)) = effects.iter().find(|x| x.0 == i) {
        pred.effect = e;
        n.effect.store(pack_effect(e), Ordering::Release);
    }
}

/// Files a new node's predictions at its label: the relations from the
/// input in this segment or the environment (the commit that places it
/// puts the value right from the true input, `settle_predictions`).
pub(crate) fn file_predicted(eng: &Engine, seg: &Segment, inner: &mut Inner, node: &Arc<Node>, pred: Predicted) {
    let v = eng.values();
    let pos = node.pos_hint();
    let Predicted { rec, shaped, effect } = pred;
    // (its brackets before it runs: readers after it see the depth it
    // makes; `settle_predictions` re-asks them)
    if effect != Effect::Pending {
        file_brackets(eng, inner, node, pos, effect.brackets());
    }
    let Some(mut rec) = rec else {
        return;
    };
    let r = Arc::get_mut(&mut rec).expect("a prediction not filed yet");
    for d in &mut r.defs[shaped..] {
        let c = d.rel.unwrap_or(0);
        let input = Segment::reaching(inner, d.f, pos, false)
            .map(|(_, d)| d.value)
            .or_else(|| read::env_value(eng, d.f));
        d.value = v.int(input.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c));
    }
    for d in &rec.defs {
        let m = inner.defs.entry(d.f).or_default();
        let first = m.is_empty();
        m.insert(
            pos,
            Def {
                depth: 0,
                value: d.value,
                by: node.id,
                rel: d.rel,
                provisional: true,
                multi: false,
            },
        );
        if first {
            eng.defs.add(d.f, seg.lo, seg.id);
        }
        eng.bump(d.f);
    }
    *node.last.lock() = Some(rec);
}

/// Takes a node's definitions and contributions out of its segment (under
/// its write lock) and re-asks their readers, and, when it is removed, the
/// reads made at its label. Its placements, for the caller to unfile.
fn retract_products(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>, remove: bool) -> Vec<NodeId> {
    // (a node removed gives back what it holds while it is still on the
    // line: its walk starts where it stands, and a node off the line keeps
    // the label it had, which a rebalance since may have given a sibling
    // it holds, so the walk would start past it; a run of it still going
    // claims nothing after this: `release_leaving`)
    if remove {
        release_leaving(eng, w, node);
    }
    let Some((seg, mut inner)) = own_write(eng, node) else {
        return Vec::new();
    };
    // (a sibling a claim took and gave back before its products were
    // withdrawn may have run and committed since: its products stand)
    if !remove && !node.taken() {
        return Vec::new();
    }
    // (under the lock its commit installs a record under: a record read
    // before it misses the placements a commit made meanwhile, left on
    // the line with no placer)
    let rec = node.record();
    let pos = node.pos_hint();
    let seg_id = seg.id;
    let was_hole = node.is_hole();
    let t_held = std::time::Instant::now();
    let mut lap = crate::obs::Laps::new(t_held);
    // (others' reads at its label, `At`: gone with it)
    let mut anchored: Vec<(Field, Reader)> = Vec::new();
    if remove {
        // (under the fields others read at it, not every field the
        // segment's readers read)
        let fs = std::mem::take(&mut *node.anchored.lock());
        for f in fs {
            for r in inner.readers.get(&f).into_iter().flatten() {
                if r.anchor == node.id && r.node != node.id {
                    anchored.push((f, r.clone()));
                }
            }
        }
        node.set_state(State::Gone);
        if let Some(o) = node.last.lock().as_ref() {
            eng.stale_reads.fetch_add(o.reads.len() as u64, Ordering::Relaxed);
        }
        inner.nodes.remove(&pos);
        if was_hole {
            inner.holes = inner.holes.saturating_sub(1);
        }
        if let Some(v) = inner.by_op.get_mut(&node.op()) {
            v.retain(|&n| n != node.id);
        }
    }
    lap.lap("scan");
    // (its brackets go with its products; the `next`s stopped at it are
    // taken here, where it is known to be in this segment)
    let bracket_gone = if node.has_brackets.load(Ordering::Acquire) {
        file_brackets(eng, &mut inner, node, pos, Brackets::default())
    } else {
        None
    };
    lap.lap("brackets");
    let mut stops = take_stops(&mut inner, node.id);
    lap.lap("stops");
    if remove {
        stops.append(&mut std::mem::take(&mut *node.bounded.lock()));
    }
    let mut changed_defs = Vec::new();
    let mut changed_contribs = Vec::new();
    let mut ix = IndexOps::default();
    if let Some(r) = &rec {
        for d in &r.defs {
            if let Some(m) = inner.defs.get_mut(&d.f)
                && m.get(&pos).is_some_and(|e| e.by == node.id)
            {
                m.remove(&pos);
                changed_defs.push(d.f);
                if m.is_empty() {
                    inner.defs.remove(&d.f);
                    ix.defs_remove.push(d.f);
                }
            }
        }
        for (f, _) in &r.contribs {
            if let Some(m) = inner.contribs.get_mut(f)
                && m.remove(&pos).is_some()
            {
                changed_contribs.push(*f);
                if m.is_empty() {
                    ix.contribs_remove.push(*f);
                }
                inner.resum(eng, *f);
            }
        }
    }
    lap.lap("defs");
    lap.report(
        node.id.0,
        u64::try_from(t_held.elapsed().as_nanos()).unwrap_or(0),
        inner.stops.len(),
        inner.readers.len(),
    );
    *node.last.lock() = None;
    for &f in changed_defs.iter().chain(&changed_contribs) {
        eng.bump(f);
    }
    ix.apply(eng, seg, &inner);
    drop(inner);
    crate::obs::long_hold("retract", changed_defs.len(), t_held);
    if eng.op(node.op()).read_back() {
        for &f in &changed_defs {
            eng.ranges.mark_read_back(f);
        }
    }
    let mut st = CommitStats::default();
    if let Some(old) = bracket_gone {
        reask_after_effect(eng, w, node, pos, old, Brackets::default(), &mut st);
    }
    for f in changed_defs {
        if ftrace(eng, f) {
            eprintln!(
                "ftrace: retract {} by {} ({}) t {}",
                eng.values().as_str(f.0).unwrap_or_default(),
                node.id.raw(),
                eng.op(node.op()).name(),
                eng.now_ns()
            );
        }
        reask(eng, w, f, pos, seg_id, node.id, &mut st);
    }
    for f in changed_contribs {
        reask_ranges(eng, w, f, pos, node.id, &stops, &mut st);
    }
    for (f, r) in &anchored {
        reask_reader(eng, w, *f, r, Some(node.id), &mut st);
    }
    reask_stops(eng, w, stops, node.id, &mut st);
    if was_hole && remove {
        walk_stopped(eng, w, node, pos, seg_id, &[], &mut st);
    }
    // (whatever it holds, committed or claimed by a run that never
    // committed: a node removed or taken claims nothing)
    release_claimed(eng, w, node);
    eng.note_stats(w, &st);
    rec.map(|r| r.placed.clone()).unwrap_or_default()
}

/// Removes a node: its definitions, contributions, reads, by-op entry and
/// its placements', all the way down; readers of what they defined are
/// re-asked. Their state becomes `Gone`. A list of the placements still to
/// remove, not a frame each: a continuation chain is as deep as it is long.
pub(crate) fn unfile(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>) {
    let mut todo = retract_products(eng, w, node, true);
    todo.reverse();
    while let Some(c) = todo.pop() {
        // (one not on the line yet: its placer's commit removes it once
        // placed, finding its placer's record gone)
        let Some(cn) = eng.placed_node(c) else {
            continue;
        };
        let mut more = retract_products(eng, w, cn, true);
        more.reverse();
        todo.append(&mut more);
    }
}

/// A sibling taken by a claim after it ran, or before with predictions
/// filed: its products are withdrawn, its placements removed, the node
/// stays, inert.
fn retract(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>) {
    for c in retract_products(eng, w, node, false) {
        if let Some(cn) = eng.placed_node(c) {
            unfile(eng, w, cn);
        }
    }
}

/// The claimer's inert siblings made dirty again (its claim is gone).
fn release_claimed(eng: &Engine, w: Option<&Worker>, me: &Node) {
    apply_claim(eng, w, me, 0);
}

/// Makes the next `want` pushed siblings of `me` inert, and dirty again
/// those beyond that an earlier claim took. With `Node::claim_all` set
/// (`Ctx::claim_rest`), every text node (`Operation::text`) after `me` in
/// its unit, whatever placed it.
pub(crate) fn apply_claim(eng: &Engine, w: Option<&Worker>, me: &Node, want: usize) {
    claim_walk(eng, w, me, want, false);
}

/// A node about to leave the line gives back what it holds, from where it
/// still stands, and claims nothing after: a run of it still going claims
/// nothing more, so nothing is left for a walk from the label it keeps
/// off the line, which a rebalance may since have given a sibling it
/// would hold (the walk would begin past it).
fn release_leaving(eng: &Engine, w: Option<&Worker>, me: &Node) {
    claim_walk(eng, w, me, 0, true);
}

fn claim_walk(eng: &Engine, w: Option<&Worker>, me: &Node, want: usize, leave: bool) {
    // (one walk of this claimer's at a time, from its `claim_end` to the
    // one it stores: `Node::claim_mx`)
    let mut walk = me.claim_mx.lock();
    *walk |= leave;
    // (a node removed or taken claims nothing: a run of it still going
    // when it was gives back what it held instead of taking siblings)
    let want = if *walk || matches!(me.state(), State::Gone | State::Inert | State::RunningInert) {
        0
    } else {
        want
    };
    let all = me.claim_all.load(Ordering::Acquire) && want > 0;
    let mut cur = Some(me.seg_hint());
    let mut lower = Some(me.pos_hint());
    let mut k = 0usize;
    let mut retracts: Vec<Arc<Node>> = Vec::new();
    let mut releases: Vec<Arc<Node>> = Vec::new();
    // (those of `releases` made `Dirty`, for the queue)
    let mut freed: Vec<Arc<Node>> = Vec::new();
    let mut taken_holes: Vec<Arc<Node>> = Vec::new();
    let mut stops: Vec<(Field, RangeKey, RangeReader)> = Vec::new();
    // (what it held before, up to there; what it holds now, to here)
    let old_end = me
        .claim_end
        .load(Ordering::Acquire)
        .checked_sub(1)
        .map(NodeId::from_raw);
    #[cfg(test)]
    tests::paused(me.id, want);
    if want == 0 && old_end.is_none() {
        return;
    }
    let mut ps = read::SiblingParents::of(me);
    let mut new_end: Option<NodeId> = None;
    let mut first = true;
    'segs: while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != me.unit {
            break;
        }
        {
            let mut inner = seg.inner.write();
            // (where the claimer is, read under its segment's lock: a
            // label read before a relabel would start the walk before
            // it, and take siblings it does not follow)
            if first {
                first = false;
                let p = me.pos_hint();
                if me.state() != State::Gone && (me.seg_hint() != sid || inner.nodes.get(&p) != Some(&me.id)) {
                    drop(inner);
                    first = true;
                    cur = Some(me.seg_hint());
                    lower = Some(me.pos_hint());
                    continue 'segs;
                }
                lower = Some(p);
                ps = read::SiblingParents::of(me);
            }
            let before = taken_holes.len();
            let lo = lower.map_or(Unbounded, Excluded);
            let mut done = k >= want && old_end.is_none();
            for (_, id) in inner.nodes.range((lo, Unbounded)) {
                if done {
                    break;
                }
                let s = eng.node(*id);
                let takes = if all {
                    let op = eng.op(s.op());
                    s.state() != State::Gone && op.sibling() && op.text() && !s.held_before(eng, me.id, me.pos_hint())
                } else {
                    ps.takes(eng, s)
                };
                if !takes {
                    // (one it holds that this walk's rule passes over: a
                    // placed text node a claim of the rest took,
                    // `Ctx::claim_rest`, which a claim of siblings never
                    // takes: given back, whatever it claims now)
                    if s.claimer() == Some(me.id) {
                        s.claimer.store(0, Ordering::Release);
                        // (free at once, under the segment's lock: a node
                        // taken with no holder is neither held nor free,
                        // and another claim's walk meeting it counted it
                        // as its own without taking it)
                        if s.release() {
                            freed.push(s.clone());
                        }
                        releases.push(s.clone());
                    }
                    if k >= want && Some(s.id) == old_end {
                        done = true;
                    }
                    continue;
                }
                if k < want {
                    let hole = s.is_hole();
                    new_end = Some(s.id);
                    if Some(s.id) == old_end && k + 1 >= want {
                        done = true;
                    }
                    // (one a later sibling's claim holds is taken with
                    // that sibling: its holder is me now)
                    if s.claimer().is_some_and(|c| c != me.id) && s.taken() {
                        s.claimer.store(me.id.raw() + 1, Ordering::Release);
                    } else if s.take() {
                        s.claimer.store(me.id.raw() + 1, Ordering::Release);
                        // (one that has not run yet: what it was predicted
                        // to define is withdrawn as a run's would be)
                        if s.committed() || s.record().is_some() {
                            retracts.push(s.clone());
                        }
                        if !s.committed() && hole {
                            taken_holes.push(s.clone());
                        }
                    }
                } else {
                    if s.claimer() == Some(me.id) {
                        s.claimer.store(0, Ordering::Release);
                        // (free at once, under the segment's lock: a node
                        // taken with no holder is neither held nor free,
                        // and another claim's walk meeting it counted it
                        // as its own without taking it)
                        if s.release() {
                            freed.push(s.clone());
                        }
                        releases.push(s.clone());
                    }
                    if Some(s.id) == old_end || old_end.is_none() {
                        done = true;
                    }
                }
                k += 1;
                if k >= want && old_end.is_none() {
                    done = true;
                }
            }
            inner.holes = inner.holes.saturating_sub(taken_holes.len() - before);
            for h in &taken_holes[before..] {
                stops.append(&mut take_stops(&mut inner, h.id));
            }
            if done {
                break 'segs;
            }
        }
        lower = None;
        cur = eng.order.next(sid, seg.lo);
    }
    me.claim_end
        .store(new_end.map_or(0, |n| n.raw() + 1), Ordering::Release);
    drop(walk);
    #[cfg(test)]
    tests::paused_after(me.id);
    for s in retracts {
        retract(eng, w, &s);
    }
    // (a claimer removed or taken while its run went on claims nothing:
    // what it took after its removal gave back what it held goes back
    // here; either way the siblings end released)
    if want > 0 && matches!(me.state(), State::Gone | State::Inert | State::RunningInert) {
        apply_claim(eng, w, me, 0);
    }
    let mut st = CommitStats::default();
    // (a taken node never contributes: the `next`s that stopped at it
    // search on past it)
    for s in &taken_holes {
        // (one taken while its first run goes on may have claimed already,
        // in that run: a node taken claims nothing, and no commit of its
        // will give back what it took)
        release_claimed(eng, w, s);
        walk_stopped(eng, w, s, s.pos_hint(), s.seg_hint(), &[], &mut st);
        // (one that never ran keeps its operation's brackets: taken, it
        // has none)
        file_effect(eng, w, s, Some(Brackets::default()), &mut st);
    }
    reask_stops(eng, w, stops, me.id, &mut st);
    for s in &releases {
        file_effect(eng, w, s, None, &mut st);
    }
    eng.note_stats(w, &st);
    for s in &releases {
        wake_puller(eng, w, s, me.id);
    }
    for s in freed {
        enqueue(
            eng,
            w,
            &s,
            Wake {
                by: Some(me.id),
                cause: Cause::Structure,
                at_ns: eng.now_ns(),
            },
        );
    }
}

/// The nearest node before `s` that would take it as a sibling, not
/// inert and not `except`, re-asked where its last run read its
/// siblings (`Ctx::following`): `s` held by another's claim was passed
/// over when it read them, and now it is free that answer moved; its
/// commit checked the answer while `s` was still held.
fn wake_puller(eng: &Engine, w: Option<&Worker>, s: &Node, except: NodeId) {
    let mut cur = Some(s.seg_hint());
    let mut upper = Some(s.pos_hint());
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != s.unit {
            return;
        }
        let found = {
            let inner = seg.inner.read();
            let hi = upper.map_or(Unbounded, Excluded);
            let mut found = None;
            for (_, id) in inner.nodes.range((Unbounded, hi)).rev() {
                let x = eng.node(*id);
                if x.id == except || x.state() == State::Gone || x.taken() {
                    continue;
                }
                if read::SiblingParents::of(x).takes(eng, s) || s.parent == Some(x.id) {
                    found = Some(x.clone());
                    break;
                }
            }
            found
        };
        if let Some(x) = found {
            let pulled = x
                .record()
                .is_some_and(|r| r.reads.iter().any(|rd| matches!(rd.kind, ReadKind::Following { .. })));
            if pulled {
                mark_stale(
                    eng,
                    w,
                    &x,
                    Wake {
                        by: Some(s.id),
                        cause: Cause::Structure,
                        at_ns: eng.now_ns(),
                    },
                );
            }
            return;
        }
        upper = None;
        cur = eng.order.prev(sid, seg.lo);
    }
}

/// Puts a built node on the line after `after` (a label in the gap, the
/// segment relabelled if the gap is closed), registers it in the segment
/// and files its predictions. Under the segment's write lock, which the
/// caller holds; `after` is in the segment.
pub(crate) fn place(
    eng: &Engine,
    seg: &Segment,
    inner: &mut Inner,
    after: NodeId,
    node: Arc<Node>,
    pred: Predicted,
) -> Arc<Node> {
    let p = match Segment::gap_after(inner, eng.node(after).pos_hint()) {
        Some(p) => p,
        None => {
            seg.relabel(inner, eng);
            Segment::gap_after(inner, eng.node(after).pos_hint()).expect("a gap after a relabel")
        }
    };
    node.seg.store(seg.id.0, Ordering::Release);
    node.pos.store(p.0, Ordering::Release);
    eng.nodes
        .set(usize::try_from(node.id.raw()).unwrap_or(usize::MAX), node.clone());
    inner.nodes.insert(p, node.id);
    inner.holes += 1;
    inner.by_op.entry(node.op()).or_default().push(node.id);
    file_predicted(eng, seg, inner, &node, pred);
    log_placed(eng, &node, "after", after);
    node
}

/// (`PHITEX_WAKE_LOG`, profile) A placement: the node, its placer, anchor
/// and slot, its label, and the node it went after or before.
fn log_placed(eng: &Engine, node: &Node, how: &str, rel: NodeId) {
    if crate::obs::enabled() && crate::sched::wake_log() {
        eprintln!(
            "placed: node {} op {:?} parent {:?} anchor {:?} slot {} at {:?} {how} {} (at {:?})",
            node.id.raw(),
            node.op(),
            node.parent.map(NodeId::raw),
            node.anchor().map(NodeId::raw),
            node.slot(),
            node.pos_hint().0,
            rel.raw(),
            eng.placed_node(rel).map(|r| r.pos_hint().0)
        );
    }
}

/// A placement made outside its placer's segment, after the lock: its
/// index, the node it goes after, the node it goes before if any (a
/// continuation replacing another), the node.
/// (No prediction: a relocation of a node kept, `relocate`.)
type Deferred = (usize, NodeId, Option<NodeId>, Arc<Node>, Option<Predicted>);

/// `Engine::insert_placed`'s placement: right after node `beside`, its
/// placements and what stands under it (`attached_end`: the nodes others
/// placed after it), before the next of its placer's; or (`after`
/// false) right before it. None when `beside` is gone.
pub(crate) fn place_beside(
    eng: &Engine,
    beside: NodeId,
    after: bool,
    node: Arc<Node>,
    pred: Predicted,
) -> Option<Arc<Node>> {
    let start = if after { subtree_end(eng, beside) } else { beside };
    let sn = eng.node(start);
    let mut tries = 0;
    loop {
        let (seg, mut inner) = own_write(eng, sn)?;
        if Segment::needs_split(&inner) {
            let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, sid);
            continue;
        }
        if !seg.has_room(&inner, 1) && (tries < crate::order::ROOM_TRIES || !seg.can_place(&inner)) {
            drop(inner);
            tries += 1;
            assert!(tries <= 2 * crate::order::ROOM_TRIES, "no room after rebalances");
            eng.order.rebalance(eng, seg.id, 1);
            continue;
        }
        if after {
            let end = attached_end(eng, &inner, beside, start);
            return Some(place(eng, seg, &mut inner, end, node, pred));
        }
        return Some(place_before(eng, seg, &mut inner, beside, node, pred));
    }
}

/// Edits the placements its placer's last run made (`Engine::insert_placed`,
/// `remove_placed`): the record is the same run's with `f` applied to its
/// list, so the placer's next run matches what a client placed beside its
/// placements as its own (`match_placements`).
pub(crate) fn edit_placed(eng: &Engine, placer: NodeId, f: impl FnOnce(&mut Vec<NodeId>)) {
    let p = eng.node(placer);
    let mut last = p.last.lock();
    let Some(r) = last.as_ref() else {
        return;
    };
    let mut placed = r.placed.clone();
    f(&mut placed);
    *last = Some(Arc::new(RunRecord {
        serial: r.serial,
        output: r.output,
        defs: r.defs.clone(),
        contribs: r.contribs.clone(),
        reads: r.reads.clone(),
        sources: r.sources.clone(),
        placed,
        claim: r.claim,
        claim_all: r.claim_all,
        effect: r.effect,
        provisional: r.provisional,
        state: r.state,
        waits: r.waits.clone(),
    }));
}

/// `place`, before `before` (a node of the segment) instead of after one.
fn place_before(
    eng: &Engine,
    seg: &Segment,
    inner: &mut Inner,
    before: NodeId,
    node: Arc<Node>,
    pred: Predicted,
) -> Arc<Node> {
    let p = match seg.gap_before(inner, eng.node(before).pos_hint()) {
        Some(p) => p,
        None => {
            seg.relabel(inner, eng);
            seg.gap_before(inner, eng.node(before).pos_hint())
                .expect("a gap after a relabel")
        }
    };
    node.seg.store(seg.id.0, Ordering::Release);
    node.pos.store(p.0, Ordering::Release);
    eng.nodes
        .set(usize::try_from(node.id.raw()).unwrap_or(usize::MAX), node.clone());
    inner.nodes.insert(p, node.id);
    inner.holes += 1;
    inner.by_op.entry(node.op()).or_default().push(node.id);
    file_predicted(eng, seg, inner, &node, pred);
    log_placed(eng, &node, "before", before);
    node
}

/// `place_before` under `before`'s segment's lock; after `at` and its
/// placements where `before` is gone.
fn place_before_node(eng: &Engine, before: NodeId, at: NodeId, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
    let bn = eng.node(before);
    let mut tries = 0;
    loop {
        let Some((seg, mut inner)) = own_write(eng, bn) else {
            return place_after(eng, at, node, pred);
        };
        if Segment::needs_split(&inner) {
            let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, sid);
            continue;
        }
        if !seg.has_room(&inner, 1) && (tries < crate::order::ROOM_TRIES || !seg.can_place(&inner)) {
            drop(inner);
            tries += 1;
            assert!(tries <= 2 * crate::order::ROOM_TRIES, "no room after rebalances");
            eng.order.rebalance(eng, seg.id, 1);
            continue;
        }
        return place_before(eng, seg, &mut inner, before, node, pred);
    }
}

/// `place` after node `at` and its placements, under their segment's lock.
pub(crate) fn place_after(eng: &Engine, at: NodeId, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
    place_at_end(eng, at, subtree_end(eng, at), node, pred)
}

/// `place` after node `at` and its children, before its continuation.
fn place_after_children(eng: &Engine, at: NodeId, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
    place_at_end(eng, at, children_end(eng, at).0, node, pred)
}

fn place_at_end(eng: &Engine, at: NodeId, end: NodeId, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
    let endn = eng.node(end);
    let mut tries = 0;
    loop {
        if let Some((seg, mut inner)) = own_write(eng, endn) {
            if Segment::needs_split(&inner) {
                let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
                seg.split(&mut inner, eng, sid);
                continue;
            }
            if !seg.has_room(&inner, 1) && (tries < crate::order::ROOM_TRIES || !seg.can_place(&inner)) {
                drop(inner);
                tries += 1;
                assert!(tries <= 2 * crate::order::ROOM_TRIES, "no room after rebalances");
                eng.order.rebalance(eng, seg.id, 1);
                continue;
            }
            let end = match node.parent {
                Some(p) if node.slot() == 2 => attached_end(eng, &inner, p, end),
                _ if node.slot() == 1 => ranked_end(eng, &inner, at, end, node.rank(), None),
                _ => end,
            };
            return place(eng, seg, &mut inner, end, node, pred);
        }
        // the node is gone: after its parent, or at the unit's end
        return match endn.parent.or(node.parent) {
            Some(p) if p != at => place_after(eng, p, node, pred),
            _ => eng.push_node(node, pred),
        };
    }
}

/// A run's placements as its last run made them (`placed`, by `me`):
/// what a run of a relocated node that published nothing new
/// (`Ctx::unchanged`) places, so its commit checks them where it stands
/// now. Those gone or never placed are left out, as they were.
fn replay_placements(eng: &Engine, me: NodeId, placed: &[NodeId], ranks: &mut Vec<u64>) -> Vec<Placement> {
    let mut out: Vec<Placement> = Vec::with_capacity(placed.len());
    let mut at: Vec<(NodeId, usize)> = Vec::with_capacity(placed.len());
    for &c in placed {
        let Some(cn) = eng.placed_node(c) else {
            continue;
        };
        if cn.state() == State::Gone {
            continue;
        }
        let (op, args) = (cn.op(), cn.args().to_vec());
        let x = match (cn.slot(), cn.anchor()) {
            (0, _) => Placement::Child(op, args),
            (2, _) => Placement::Follow(op, args),
            (_, Some(a)) => match at.iter().find(|x| x.0 == a) {
                Some(&(_, j)) => Placement::FollowAfterPlaced(j, op, args),
                None => Placement::FollowAfter(a, op, args),
            },
            (_, None) => Placement::FollowAfter(me, op, args),
        };
        at.push((c, out.len()));
        ranks.push(cn.rank());
        out.push(x);
    }
    out
}

/// Where a relocated node goes (`relocate`), as a node made anew there
/// would: after a node and its children (`Ctx::follow_after`), after a
/// node and its placements, or before a node (a continuation's place it
/// takes; after the second where that one is gone).
#[derive(Clone, Copy)]
pub(crate) enum Target {
    AfterChildren(NodeId),
    After(NodeId),
    Before(NodeId, NodeId),
}

/// What a relocated node left at its old label (`lift`): the label and
/// segment, the fields it defined and contributed to there, its brackets,
/// others' reads anchored at it (filed again where it lands), the `next`
/// and range readers bounded by it, and whether it had not committed.
struct Left {
    pos: Pos,
    seg: SegId,
    /// The nodes it stood right after and right before in its segment
    /// (none: it was the segment's first, or last), and the segment's
    /// count of label moves then (`Segment::moved`): its old label names
    /// its place only while the segment's labels have not moved
    /// (`old_place`).
    before: Option<NodeId>,
    after: Option<NodeId>,
    moves: u64,
    defs: Vec<Field>,
    contribs: Vec<Field>,
    brackets: Option<Brackets>,
    readers: Vec<(Field, Reader)>,
    stops: Vec<(Field, RangeKey, RangeReader)>,
    hole: bool,
}

/// Relocates `k`, a placement of `placer`'s kept by its (op, args) whose
/// place moved (docs/spec-continuations.md §2.3): its id, record and own
/// placements are kept. The nodes that go: `k` and every node that stands
/// under it (placed by it or by its placements, or placed after it or
/// after one of those by another node: `follow_after`), after it in their
/// order, wherever the move goes: a subtree left behind where `k` moved
/// back stood after the nodes between its old and its new place, and a
/// node placed after `k` meanwhile went after `k`'s children there
/// (`children_end`), past those nodes (numerica at -j4: a page's
/// finisher after its piece's next pages). Each is lifted from its old
/// label (`lift`: its definitions, contributions, brackets taken out
/// there), put at the new one with its record's definitions filed as
/// predictions and its brackets (`land`), then the readers at the new
/// place whose answer moved are re-asked, then those at the old place
/// (whose walk stops at the new definitions where it reaches them: a
/// reader past the new place sees the same), and it is marked to run
/// again, its reads checked where it stands. A node whose claim holds a
/// sibling that would stand before it gives its claim back first: its run
/// takes it again from the new place. False when it stayed (its place
/// gone with its placer, or the place inside its own subtree).
pub(crate) fn relocate(
    eng: &Engine,
    w: Option<&Worker>,
    placer: NodeId,
    k: &Arc<Node>,
    to: Target,
    moving: &FxHashSet<NodeId>,
    st: &mut CommitStats,
) -> bool {
    // its subtree, as it stands
    let mut sub: Vec<NodeId> = Vec::new();
    let mut skip: FxHashSet<NodeId> = FxHashSet::default();
    skip.insert(k.id);
    let mut todo = vec![k.id];
    while let Some(x) = todo.pop() {
        let Some(r) = eng.node(x).run_record() else {
            continue;
        };
        for &c in &r.placed {
            let Some(cn) = eng.placed_node(c) else {
                continue;
            };
            if cn.state() == State::Gone || cn.pos_hint() == Pos(0) || !skip.insert(c) {
                continue;
            }
            sub.push(c);
            todo.push(c);
        }
    }
    // (and its placer's other placements about to move: they are no part
    // of what it goes after)
    skip.extend(moving.iter().copied());
    // (and what others placed after it or under it: the nodes after it
    // whose anchor chain reaches it, as they stand in its segment, but
    // for those about to move)
    if let Some(seg) = eng.seg_for_pos(k.pos_hint()) {
        let inner = seg.inner.read();
        for (_, &x) in inner.nodes.range((Excluded(k.pos_hint()), Unbounded)) {
            let Some(xn) = eng.placed_node(x) else {
                continue;
            };
            if xn.state() == State::Gone || xn.pos_hint() == Pos(0) {
                continue;
            }
            if slot_under(eng, x, k.id).is_none() {
                break;
            }
            if skip.insert(x) {
                sub.push(x);
            }
        }
    }
    // where it goes: after `end`, or before `before`
    let (end, before) = match to {
        Target::AfterChildren(at) => (children_end_skip(eng, at, Some(&skip)).0, None),
        Target::After(at) => (subtree_end_skip(eng, at, Some(&skip)), None),
        Target::Before(b, at) => match eng.placed_node(b) {
            Some(bn) if bn.state() != State::Gone && bn.pos_hint() != Pos(0) && !skip.contains(&b) => (at, Some(b)),
            _ => (subtree_end_skip(eng, at, Some(&skip)), None),
        },
    };
    if skip.contains(&end) || eng.placed_node(end).is_none() {
        return false;
    }
    let lim = before.map_or_else(|| eng.node(end).pos_hint(), |b| eng.node(b).pos_hint());
    let mut movers: Vec<Arc<Node>> = vec![k.clone()];
    let mut rest: Vec<Arc<Node>> = sub.iter().map(|&c| eng.node(c).clone()).collect();
    // (each label read once, as above: a rebalance elsewhere may move one
    // during the sort)
    rest.sort_by_cached_key(|c| c.pos_hint());
    movers.extend(rest);
    // (a claim holding a sibling it would stand after: given back)
    for m in &movers {
        let held_end = m.claim_end.load(Ordering::Acquire).checked_sub(1).map(NodeId::from_raw);
        if m.run_record().is_some_and(|r| r.claim > 0)
            && held_end.is_some_and(|h| eng.placed_node(h).is_some_and(|hn| hn.pos_hint() < lim))
        {
            apply_claim(eng, w, m, 0);
        }
    }
    let (lo, mut hi) = (k.pos_hint().min(lim), k.pos_hint().max(lim));
    let mut prev = end;
    let mut first = true;
    let mut landed: Vec<Arc<Node>> = Vec::with_capacity(movers.len());
    for m in movers {
        let Some(left) = lift(eng, &m) else {
            continue;
        };
        hi = hi.max(left.pos);
        let parent = if first && m.slot() == 2 && before.is_none() {
            m.parent
        } else {
            None
        };
        // (a ranked one after a node: past the lower ranks there)
        let ranked = match to {
            Target::AfterChildren(at) if first && m.slot() == 1 && m.rank() > 0 => Some((at, m.rank())),
            _ => None,
        };
        let place = land(
            eng,
            &m,
            prev,
            if first { before } else { None },
            (parent, ranked),
            &skip,
            left.hole,
        );
        let Some((npos, nseg, defs, b)) = place else {
            // (its place gone with its placer: it goes with it, its old
            // place's readers asked again, and its placements with it,
            // from the record it had: one cleared first left them on the
            // line with no placer, a finisher's page writers at -j16,
            // note-intermittents §2)
            m.set_state(State::Gone);
            let rec = m.last.lock().take();
            lifted_walks(eng, w, &m, left, st);
            unfile_placed(eng, w, rec.as_deref());
            // (a node removed claims nothing: what it holds goes back)
            release_leaving(eng, w, &m);
            continue;
        };
        first = false;
        prev = m.id;
        // the readers anchored at it, filed where it stands now
        let mut first_of: Vec<Field> = Vec::new();
        {
            let mut spare: Vec<Reader> = Vec::new();
            if let Some((seg, mut inner)) = own_write(eng, &m) {
                for (f, r) in &left.readers {
                    if file_reader(eng, &mut inner, *f, r.clone(), &mut spare) {
                        first_of.push(*f);
                    }
                }
                drop(inner);
                for f in first_of {
                    eng.readers.add(f, seg.lo, seg.id);
                }
            }
        }
        // the new place first: a walk from the old label stops at it
        for d in &defs {
            if let Some(c) = d.rel {
                settle_rel(eng, d.f, m.id, c);
            }
        }
        for d in &defs {
            reask(eng, w, d.f, npos, nseg, m.id, st);
        }
        if b.closes > 0 || b.opens > 0 {
            reask_after_effect(eng, w, &m, npos, Brackets::default(), b, st);
        }
        // then the old place
        lifted_walks(eng, w, &m, left, st);
        hi = hi.max(npos);
        landed.push(m);
    }
    if landed.is_empty() {
        return false;
    }
    // (range readers bounded by a node that moved now reach anywhere
    // between its two labels)
    eng.ranges
        .widen_over(Pos(lo.0.saturating_sub(1)), Pos(hi.0.saturating_add(1)));
    // each runs again where it stands, in their order (the ready heap
    // takes the first queued first among equal keys, `sched::Ready`)
    for m in &landed {
        m.moved.store(true, Ordering::Release);
        // (a parked state rests on reads at the old place)
        if m.record().is_some_and(|r| r.state.is_some()) {
            m.state_stale.store(true, Ordering::Release);
        }
        let wake = Wake {
            by: Some(placer),
            cause: Cause::Created { by: Some(placer) },
            at_ns: eng.now_ns(),
        };
        if m.state() == State::Clean && m.cas_state(State::Clean, State::Dirty) {
            enqueue(eng, w, m, wake);
        } else {
            // (one queued already is queued again at its new label: its
            // entry keyed by the old label ran it after the nodes between
            // the two, each reading its definitions before it ran there;
            // the stale entry finds it run, `sched::work`. Grail's cold
            // build at -j1: a page's routine moved to the piece it now
            // fires in ran ~170 ms after the pieces past that one, which
            // held at the frontier on its definitions, 331 holds in a
            // pass, mc 79156)
            let queued = m.state() == State::Dirty;
            mark_dirty(eng, w, m, wake);
            if queued {
                crate::sched::requeue(eng, w, m);
            }
        }
    }
    if crate::obs::enabled() && crate::sched::wake_log() {
        eprintln!(
            "relocated: node {} op {:?} by {} with {} of its subtree, after {} (at {:?})",
            k.id.raw(),
            k.op(),
            placer.raw(),
            landed.len() - 1,
            end.raw(),
            k.pos_hint().0
        );
    }
    true
}

/// The walks from a relocated node's old label (`relocate`): the readers
/// anchored at it there (filed again where it landed), those after its
/// brackets, definitions and contributions there, the `next`s and ranges
/// it bounded, the walks that stopped at it while it had not committed.
fn lifted_walks(eng: &Engine, w: Option<&Worker>, m: &Arc<Node>, mut left: Left, st: &mut CommitStats) {
    let (pos, seg) = old_place(eng, &left);
    for &f in &left.defs {
        if walk_trace(eng, f) {
            eprintln!(
                "walk-trace: lifted node {} from {} seg {} (lifted at {} seg {}) hole {}",
                m.id.raw(),
                pos.0,
                seg.0,
                left.pos.0,
                left.seg.0,
                left.hole
            );
        }
    }
    (left.pos, left.seg) = (pos, seg);
    for (f, r) in &left.readers {
        reask_reader(eng, w, *f, r, Some(m.id), st);
    }
    if let Some(old) = left.brackets {
        reask_after_effect_at(eng, w, m, left.pos, old, Brackets::default(), st);
    }
    for &f in &left.defs {
        reask_from(eng, w, f, left.pos, left.seg, None, Some(m.op()), 0, st);
    }
    for &f in &left.contribs {
        reask_ranges_at(eng, w, f, left.pos, m.id, &left.stops, st);
    }
    reask_stops(eng, w, left.stops, m.id, st);
    if left.hole {
        wake_waiters(eng, w, m, st);
        let fs = std::mem::take(&mut *m.stopped.lock());
        for (f, level) in fs {
            reask_from(eng, w, f, left.pos, left.seg, None, Some(m.op()), level, st);
        }
    }
}

/// Where a lifted node's place is now, as the walks from it start: its
/// old label while its segment's labels have not moved since the lift;
/// else just after the node it stood after there, at that node's label
/// now (a relabel renumbers every node but leaves a free label as it
/// was: the moved node's landing in its own segment relabelled it, its
/// old label fell after nodes that had stood after it, and the walk from
/// there passed over their readers: cleanthesis at -j4, a page's
/// finisher kept the page number a later page's finisher, landed ahead
/// of it before, had given it); else just before the node it stood
/// before; else (both gone, or it stood alone) the segment's start.
fn old_place(eng: &Engine, left: &Left) -> (Pos, SegId) {
    let seg = eng.seg(left.seg);
    if seg.moved.load(Ordering::Acquire) == left.moves {
        return (left.pos, left.seg);
    }
    if let Some(b) = left.before
        && let Some(p) = read::pos_of(eng, b)
    {
        return (p, eng.node(b).seg_hint());
    }
    // (the node after it, gone or moved: just before the one after it)
    if let Some(a) = left.after
        && let Some(p) = read::pos_of(eng, a)
    {
        return (Pos(p.0.saturating_sub(1)), eng.node(a).seg_hint());
    }
    (Pos(seg.lo.0.saturating_sub(1)), left.seg)
}

/// Takes a node off its label for `relocate`, under its segment's lock:
/// out of the segment's nodes and its operation's list, its definitions,
/// contributions and brackets taken out, others' readers anchored at it
/// taken (its own go: it runs again), the `next` readers stopped at it
/// and the range readers bounded by it taken. Its label is 0 until it
/// lands (`land`): no other placement goes after it meanwhile. Its record
/// stays.
fn lift(eng: &Engine, m: &Arc<Node>) -> Option<Left> {
    let (seg, mut inner) = own_write(eng, m)?;
    let pos = m.pos_hint();
    let hole = m.is_hole();
    let rec = m.record();
    let mut fs: Vec<Field> = m.anchored.lock().clone();
    if let Some(r) = &rec {
        fs.extend(r.reads.iter().filter_map(|x| match x.kind {
            ReadKind::Field { f, at: Anchor::Me } => Some(f),
            _ => None,
        }));
    }
    fs.sort_unstable();
    fs.dedup();
    let mut readers: Vec<(Field, Reader)> = Vec::new();
    for f in fs {
        if let Some(rs) = inner.readers.get_mut(&f) {
            rs.retain(|r| {
                if r.anchor != m.id {
                    return true;
                }
                if r.node != m.id && live(eng, r.node, r.serial) {
                    readers.push((f, r.clone()));
                }
                false
            });
        }
    }
    let before = inner.nodes.range(..pos).next_back().map(|(_, &n)| n);
    let after = inner.nodes.range((Excluded(pos), Unbounded)).next().map(|(_, &n)| n);
    let moves = seg.moved.load(Ordering::Acquire);
    inner.nodes.remove(&pos);
    if hole {
        inner.holes = inner.holes.saturating_sub(1);
    }
    if let Some(v) = inner.by_op.get_mut(&m.op()) {
        v.retain(|&n| n != m.id);
    }
    let brackets = if m.has_brackets.load(Ordering::Acquire) {
        file_brackets(eng, &mut inner, m, pos, Brackets::default())
    } else {
        None
    };
    let mut stops = take_stops(&mut inner, m.id);
    stops.append(&mut std::mem::take(&mut *m.bounded.lock()));
    let mut defs = Vec::new();
    let mut contribs = Vec::new();
    let mut ix = IndexOps::default();
    if let Some(r) = &rec {
        for d in &r.defs {
            if let Some(mp) = inner.defs.get_mut(&d.f)
                && mp.get(&pos).is_some_and(|e| e.by == m.id)
            {
                mp.remove(&pos);
                defs.push(d.f);
                if mp.is_empty() {
                    inner.defs.remove(&d.f);
                    ix.defs_remove.push(d.f);
                }
            }
        }
        for (f, _) in &r.contribs {
            if let Some(mp) = inner.contribs.get_mut(f)
                && mp.remove(&pos).is_some()
            {
                contribs.push(*f);
                if mp.is_empty() {
                    ix.contribs_remove.push(*f);
                }
                inner.resum(eng, *f);
            }
        }
    }
    m.pos.store(0, Ordering::Release);
    for &f in defs.iter().chain(&contribs) {
        eng.bump(f);
    }
    ix.apply(eng, seg, &inner);
    let seg_id = seg.id;
    drop(inner);
    if eng.op(m.op()).read_back() {
        for &f in &defs {
            eng.ranges.mark_read_back(f);
        }
    }
    Some(Left {
        pos,
        seg: seg_id,
        before,
        after,
        moves,
        defs,
        contribs,
        brackets,
        readers,
        stops,
        hole,
    })
}

/// Puts a lifted node back on the line for `relocate`: before `before`,
/// or after `after` (a continuation of `attach`'s after what stands under
/// it there, `attached_end`, passing over `skip`), under that node's
/// segment's lock; its record's definitions filed as predictions there
/// (its record stays: its run diffs against it) and its brackets, its
/// last run's. Its label and segment, the definitions filed, the
/// brackets; None when the node it goes by is gone.
#[allow(clippy::too_many_arguments)]
fn land(
    eng: &Engine,
    m: &Arc<Node>,
    after: NodeId,
    before: Option<NodeId>,
    (attach, ranked): (Option<NodeId>, Option<(NodeId, u64)>),
    skip: &FxHashSet<NodeId>,
    hole: bool,
) -> Option<(Pos, SegId, Vec<DefRec>, Brackets)> {
    let by = eng.node(before.unwrap_or(after));
    let rec = m.record();
    let defs: Vec<DefRec> = rec.as_ref().map(|r| r.defs.clone()).unwrap_or_default();
    let b = match rec.as_ref().filter(|r| !r.provisional) {
        Some(r) => r.effect.brackets(),
        None => {
            let (b, pending) = unpack_effect(m.effect.load(Ordering::Acquire));
            if pending { Brackets::default() } else { b }
        }
    };
    let mut tries = 0;
    loop {
        let (seg, mut inner) = own_write(eng, by)?;
        if Segment::needs_split(&inner) {
            let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, sid);
            continue;
        }
        if !seg.has_room(&inner, 1) && (tries < crate::order::ROOM_TRIES || !seg.can_place(&inner)) {
            drop(inner);
            tries += 1;
            assert!(tries <= 2 * crate::order::ROOM_TRIES, "no room after rebalances");
            eng.order.rebalance(eng, seg.id, 1);
            continue;
        }
        let p = match before {
            Some(bf) => match seg.gap_before(&inner, eng.node(bf).pos_hint()) {
                Some(p) => p,
                None => {
                    seg.relabel(&mut inner, eng);
                    seg.gap_before(&inner, eng.node(bf).pos_hint())
                        .expect("a gap after a relabel")
                }
            },
            None => {
                let end = attach.map_or(after, |a| attached_end_skip(eng, &inner, a, after, Some(skip)));
                let end = ranked.map_or(end, |(a, r)| ranked_end(eng, &inner, a, end, r, Some(skip)));
                match Segment::gap_after(&inner, eng.node(end).pos_hint()) {
                    Some(p) => p,
                    None => {
                        seg.relabel(&mut inner, eng);
                        Segment::gap_after(&inner, eng.node(end).pos_hint()).expect("a gap after a relabel")
                    }
                }
            }
        };
        m.seg.store(seg.id.0, Ordering::Release);
        m.pos.store(p.0, Ordering::Release);
        inner.nodes.insert(p, m.id);
        if hole {
            inner.holes += 1;
        }
        inner.by_op.entry(m.op()).or_default().push(m.id);
        if b.closes > 0 || b.opens > 0 {
            file_brackets(eng, &mut inner, m, p, b);
        }
        // (at the depths its commit filed them at, `filed_defs`: filed at
        // depth 0, a global definition by a node inside a scope was seen
        // inside that scope alone, and a reader past the scope's close met
        // the definition before it, ran, and ran again once the moved node
        // had: Grail's float box `\bx@M`, voided by the routine that
        // placed the float, read again by every later routine as the
        // float's box when that routine moved, mc 79156)
        for d in filed_defs(&eng.values, &defs, &b) {
            let mp = inner.defs.entry(d.f).or_default();
            let first = mp.is_empty();
            mp.insert(
                p,
                Def {
                    depth: d.depth,
                    value: d.value,
                    by: m.id,
                    rel: d.rel,
                    provisional: true,
                    multi: d.multi,
                },
            );
            if first {
                eng.defs.add(d.f, seg.lo, seg.id);
            }
            eng.bump(d.f);
        }
        log_placed(eng, m, "moved after", after);
        return Some((p, seg.id, defs, b));
    }
}

/// The placements of a node that goes (`relocate` with nowhere to go),
/// removed: those of record `r`, its last, taken off it first.
fn unfile_placed(eng: &Engine, w: Option<&Worker>, r: Option<&RunRecord>) {
    if let Some(r) = r {
        for &c in &r.placed {
            if let Some(cn) = eng.placed_node(c) {
                unfile(eng, w, cn);
            }
        }
    }
}

/// Matches a run's placements to the last run's by (op, args): kept nodes
/// in order, each `Some(id)`, and `None` where a new one is to be made.
pub(crate) fn match_placements(eng: &Engine, last: &[NodeId], new: &[Placement]) -> Vec<Option<NodeId>> {
    if last.is_empty() {
        return vec![None; new.len()];
    }
    let mut pool: FxHashMap<(OpId, Vec<ValueId>), VecDeque<NodeId>> = FxHashMap::default();
    for &n in last {
        // (one a commit cut short never placed: not there to keep)
        let Some(nd) = eng.placed_node(n) else {
            continue;
        };
        if nd.state() == State::Gone {
            continue;
        }
        pool.entry((nd.op(), nd.args().to_vec())).or_default().push_back(n);
    }
    new.iter()
        .map(|p| {
            let (op, a) = p.key();
            pool.get_mut(&(op, a.to_vec())).and_then(VecDeque::pop_front)
        })
        .collect()
}

/// `rel_value` at a node's label, read under its segment's lock.
pub(crate) fn rel_value_at(eng: &Engine, f: Field, at: NodeId, c: i64) -> ValueId {
    let v = eng.values();
    let input = read::through_aliases(eng, reaching_at(eng, f, at, false).and_then(|r| r.value));
    v.int(input.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c))
}

/// The brackets a commit files for its node: what its run set, else its
/// operation's static effect (none while that is pending).
fn effective_brackets(node: &Node, e: Option<Effect>) -> Brackets {
    match e {
        Some(e) => e.brackets(),
        None => {
            let (b, pending) = unpack_effect(node.effect.load(Ordering::Acquire));
            if pending { Brackets::default() } else { b }
        }
    }
}

/// Files `b` as the brackets of `node` at `pos` (none: takes them out),
/// under its segment's lock; the brackets it had when they changed. A
/// change counts a new scope epoch (`Engine::scope_gen`: every chain
/// cached is stale) and a change of the depth field.
pub(crate) fn file_brackets(eng: &Engine, inner: &mut Inner, node: &Node, pos: Pos, b: Brackets) -> Option<Brackets> {
    let some = b.closes > 0 || b.opens > 0;
    let old = inner.brackets.get(&pos).filter(|x| x.0 == node.id).map(|x| x.1);
    let same = match old {
        Some(o) => some && (o.closes, o.opens, o.before) == (b.closes, b.opens, b.before),
        None => !some,
    };
    if same {
        if some {
            inner.brackets.insert(pos, (node.id, b));
        }
        return None;
    }
    if some {
        if old.is_none() {
            eng.bracket_nodes.fetch_add(1, Ordering::AcqRel);
            node.has_brackets.store(true, Ordering::Release);
        }
        inner.brackets.insert(pos, (node.id, b));
    } else {
        inner.brackets.remove(&pos);
        eng.bracket_nodes.fetch_sub(1, Ordering::AcqRel);
        node.has_brackets.store(false, Ordering::Release);
    }
    inner.sum_depth();
    let g = eng.scope_changed();
    inner.brk_gen = g;
    node.bracket_gen.store(g, Ordering::Release);
    eng.bump(eng.depth_field);
    Some(old.unwrap_or_default())
}

/// Files a node's brackets at its position from its operation's static
/// effect (a node made, its text replaced, a sibling given back by a
/// claim; a pending one keeps what it has until it runs) or none (taken
/// by a claim) and, when they changed, re-asks every reader after it
/// whose answer moved: a bracket decides which definitions reach
/// everything after it.
pub(crate) fn file_effect(
    eng: &Engine,
    w: Option<&Worker>,
    node: &Arc<Node>,
    b: Option<Brackets>,
    st: &mut CommitStats,
) {
    // (`Some`: a node taken, its brackets none; `None`: its own, free)
    let taken = b.is_some();
    let b = match b {
        Some(b) => b,
        None => {
            let (b, pending) = unpack_effect(node.effect.load(Ordering::Acquire));
            if pending {
                return;
            }
            b
        }
    };
    if !node.has_brackets.load(Ordering::Acquire) && b.closes == 0 && b.opens == 0 {
        return;
    }
    let Some((_, mut inner)) = own_write(eng, node) else {
        return;
    };
    // (under the lock a claim takes and gives back under: brackets of a
    // node given back, of one taken none, whichever walk came last)
    if node.taken() != taken {
        return;
    }
    let pos = node.pos_hint();
    let old = file_brackets(eng, &mut inner, node, pos, b);
    drop(inner);
    if let Some(old) = old {
        reask_after_effect(eng, w, node, pos, old, b, st);
    }
}

/// The clamped walk of a depth through a segment of summary `(net, min)`:
/// the lowest depth reached, and the depth at its end.
/// A bracket walk through one segment from depths `a` (the old side), `b`
/// (the new one) and `p` (the fewest both have shared), from `at` in the
/// first segment, else whole by the segment's summary: the depths after
/// it (or at the stop), and where the two sides agree again, if they do.
fn scopes_through(inner: &Inner, first: bool, at: Pos, a: i64, b: i64, p: i64) -> (i64, i64, i64, Option<Pos>) {
    let (mut a, mut b, mut p) = (a, b, p);
    if a == b && b == p {
        return (a, b, p, Some(at));
    }
    if first {
        for (q, (_, x)) in inner.brackets.range((Excluded(at), Unbounded)) {
            a = (a - i64::from(x.closes)).max(0);
            b = (b - i64::from(x.closes)).max(0);
            p = p.min(a).min(b);
            a += i64::from(x.opens);
            b += i64::from(x.opens);
            if a == b && b == p {
                return (a, b, p, Some(*q));
            }
        }
        return (a, b, p, None);
    }
    let (net, min) = (i64::from(inner.depth_net), i64::from(inner.depth_min));
    let (la, ea) = through_segment(a, net, min);
    let (lb, eb) = through_segment(b, net, min);
    p = p.min(la).min(lb);
    (a, b) = (ea, eb);
    let stop = (a == b && b == p).then(|| Pos(inner.hi.0.saturating_sub(1)));
    (a, b, p, stop)
}

fn through_segment(d: i64, net: i64, min: i64) -> (i64, i64) {
    ((d + min).max(0), (d + net).max(net - min))
}

/// Whether a bracket changed, or moved, after scope count `g` in the
/// segments from the one at `at` to `last`: the region of a walk that
/// stopped in `last`. A change elsewhere has its own walk: one before
/// `at` walks on past it, one after the stop starts where this walk's
/// changes no longer reach. (Any change anywhere since counted as such a
/// race: at -j4 nearly every walk, whose region was a node wide, asked
/// every reader to the line's end, 4 to 5 M, biblatex, mc 80647.)
fn brackets_changed_since(eng: &Engine, at: Pos, last: SegId, g: u64) -> bool {
    if eng.scope_gen.load(Ordering::Acquire) == g {
        return false;
    }
    let mut cur = eng.seg_for_pos(at).map(|s| s.id);
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        if inner.brk_gen > g {
            return true;
        }
        let lo = seg.lo;
        drop(inner);
        if sid == last {
            return false;
        }
        cur = eng.order.next(sid, lo);
    }
    // (the stop's segment no longer after `at`: moved meanwhile)
    true
}

/// Whether a definition's reach can move with the brackets around it: a
/// local one (its scope may close earlier or later), one in several
/// scopes, or a relation (its input's may).
fn scoped_def(d: &Def) -> bool {
    d.depth != GLOBAL || d.multi || d.rel.is_some()
}

/// The fields whose readers a change of brackets at `at` can give another
/// answer, as a walk from `at` goes on. A reader whose answer moved reads
/// a field one of whose definitions its scopes now hide and did not, or
/// the reverse: a definition reaching every later position (`GLOBAL`)
/// is seen alike on both sides; a local one is seen alike where the two
/// sides share the scope it lives in. The sides share every scope open
/// at `at` up to the `p`-th, `p` the fewest scopes both sides have had
/// open between `at` and the reader. So: the fields with a local
/// definition between `at` and the reader (`fields`, added as the walk
/// meets them), and those with one before `at` in a scope open at `at`
/// above the `p`-th (`below`: since that scope's opener). Besides, a
/// reader of the depth itself and one by a derived path (whose field is
/// found from other fields at its place) are always asked.
struct ScopeReach {
    /// The scopes open at `at`, its own closes not applied: each one's
    /// opener and which of its opens (`read::opener`).
    open: Vec<u64>,
    /// The scopes open at `at` from the `covered`-th up have had their
    /// definitions before `at` taken into `fields`.
    covered: usize,
    /// Where those definitions start (the `covered`-th scope's opener),
    /// `at` while none is covered.
    from: Pos,
    fields: FxHashSet<Field>,
    /// An opener not found on the line: every reader is asked.
    all: bool,
}

impl ScopeReach {
    fn new(eng: &Engine, at: Pos) -> ScopeReach {
        let open = loop {
            let Some(seg) = eng.seg_for_pos(at) else {
                break Vec::new();
            };
            let start = read::chain_start(eng, seg);
            let inner = seg.inner.read();
            if at < seg.lo || (at >= inner.hi && eng.order.at_or_after(at) != Some(seg.id)) {
                continue;
            }
            break read::chain_at(&inner, &start, at, false, false);
        };
        ScopeReach {
            covered: open.len(),
            open,
            from: at,
            fields: FxHashSet::default(),
            all: false,
        }
    }

    /// The definitions before `at` in the scopes open there from the
    /// `p`-th up (0: the outermost), taken into `fields`.
    fn below(&mut self, eng: &Engine, p: i64) {
        let p = usize::try_from(p.max(0)).unwrap_or(0);
        if self.all || p >= self.covered {
            return;
        }
        let opener = NodeId::from_raw(self.open[p] >> 8);
        let Some(lo) = read::pos_of(eng, opener) else {
            self.all = true;
            return;
        };
        let hi = self.from;
        let mut cur = eng.seg_for_pos(lo).map(|s| s.id);
        while let Some(sid) = cur {
            let seg = eng.seg(sid);
            if seg.lo > hi {
                break;
            }
            let inner = seg.inner.read();
            for (f, m) in &inner.defs {
                if !self.fields.contains(f) && m.range(lo..hi).any(|(_, d)| scoped_def(d)) {
                    self.fields.insert(*f);
                }
            }
            let next_lo = seg.lo;
            drop(inner);
            cur = eng.order.next(sid, next_lo);
        }
        self.covered = p;
        self.from = lo;
    }
}

/// Re-asks the readers anchored at and after `at` (all fields) whose
/// answer moved, after `by`'s brackets changed from `old` to `new`: up to
/// where the scopes open on both sides are the same again (whole segments
/// by their summaries), and past it the readers of what is defined in
/// between. A reader whose answer still stands is left as it is.
pub(crate) fn reask_after_effect(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    at: Pos,
    old: Brackets,
    new: Brackets,
    st: &mut CommitStats,
) {
    let at = read::pos_of(eng, by.id).unwrap_or(at);
    reask_after_effect_at(eng, w, by, at, old, new, st);
}

/// [`reask_after_effect`] for a commit's own change of `by`'s brackets,
/// `own_old` the fields its record before defined
/// ([`reask_after_effect_with`]).
#[allow(clippy::too_many_arguments)]
fn reask_after_own_effect(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    at: Pos,
    old: Brackets,
    new: Brackets,
    own_old: &[Field],
    st: &mut CommitStats,
) {
    let at = read::pos_of(eng, by.id).unwrap_or(at);
    reask_after_effect_with(eng, w, by, at, old, new, Some(own_old), st);
}

/// `reask_after_effect` from label `at` whatever `by`'s label is now
/// (its old one, `relocate`).
fn reask_after_effect_at(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    at: Pos,
    old: Brackets,
    new: Brackets,
    st: &mut CommitStats,
) {
    reask_after_effect_with(eng, w, by, at, old, new, None, st);
}

/// `reask_after_effect_at`; with `own_old`, the fields `by`'s record
/// before this change defined: a definition of `by`'s in one of the
/// scopes it opens now and did not before is in a scope closed where the
/// walk stops, seen by no reader after the stop, and where `by` did not
/// define its field before, a reader there sees what it saw. Its field
/// is not walked past the stop for it (an opener's scope kind, defined
/// inside the scope it opens: every reader of the scope kind to the end
/// of the line was asked again at each opener's first commit, ~1.5 s
/// each, 1 040 s of biblatex's -j4 build, mc 80580).
#[allow(clippy::too_many_arguments)]
fn reask_after_effect_with(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    at: Pos,
    old: Brackets,
    new: Brackets,
    own_old: Option<&[Field]>,
    st: &mut CommitStats,
) {
    // (a definition of `by`'s, of a field it did not define before, in a
    // scope it opens now and did not before)
    let inside_new_open = |f: &Field, d: &Def| {
        own_old.is_some_and(|o| {
            d.by == by.id
                && d.depth != GLOBAL
                && d.depth > u16::from(old.opens)
                && d.depth <= u16::from(new.opens)
                && !o.contains(f)
        })
    };
    let d0 = i64::from(read::depth_at(eng, at));
    let (co, cn) = ((d0 - i64::from(old.closes)).max(0), (d0 - i64::from(new.closes)).max(0));
    let (mut a, mut b) = (co + i64::from(old.opens), cn + i64::from(new.opens));
    // (the scopes both sides share: the common ones under the node's
    // closes, and its opens as well where those agree)
    let mut p = if co == cn {
        co + i64::from(old.opens.min(new.opens))
    } else {
        co.min(cn)
    };
    let mut readers: Vec<(Field, Reader)> = Vec::new();
    let mut fields: Vec<Field> = Vec::new();
    let mut end: Option<(SegId, Pos)> = None;
    let mut cur = eng.seg_for_pos(at).map(|s| s.id);
    let mut first = true;
    // (which readers in the region can see another answer: those of a
    // field with a definition whose scope the change can end or extend,
    // `ScopeReach`; the others' answers are those of definitions both
    // sides see. Every reader there was asked, 3 to 4 M per walk to the
    // line's end in biblatex's -j4 build, ~15 s each, of which 2 to 5 %
    // read such a field, accl 25435)
    let mut reach = ScopeReach::new(eng, at);
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        let mut inner = seg.inner.read();
        // (where the walk stops in this segment, if it does, and the
        // fewest scopes both sides share up to there)
        let (na, nb, np, stop) = scopes_through(&inner, first, at, a, b, p);
        // (the definitions before `at` in the scopes open there above
        // those, looked up under no segment's lock: then this segment is
        // read again, and any bracket filed in it meanwhile counted)
        let (na, nb, np, stop) = if usize::try_from(np.max(0)).unwrap_or(0) < reach.covered && !reach.all {
            drop(inner);
            let mut q = np;
            loop {
                reach.below(eng, q);
                inner = seg.inner.read();
                let x = scopes_through(&inner, first, at, a, b, p);
                if reach.all || usize::try_from(x.2.max(0)).unwrap_or(0) >= reach.covered {
                    break x;
                }
                q = x.2;
                drop(inner);
            }
        } else {
            (na, nb, np, stop)
        };
        (a, b, p) = (na, nb, np);
        let inside = |q: Pos| (!first || q >= at) && stop.is_none_or(|s| q <= s);
        // (the definitions here first: a reader after one in this segment
        // is asked whatever its place in it, as is one before, needlessly)
        for (f, m) in &inner.defs {
            if !reach.fields.contains(f) && m.iter().any(|(q, d)| inside(*q) && scoped_def(d)) {
                reach.fields.insert(*f);
            }
        }
        for (f, rs) in &inner.readers {
            let all = reach.all || *f == eng.depth_field || reach.fields.contains(f);
            for r in rs {
                if (all || read::derived_spec(eng, r.path).is_some())
                    && inside(eng.node(r.anchor).pos_hint())
                    && live(eng, r.node, r.serial)
                {
                    readers.push((*f, r.clone()));
                }
            }
        }
        for (f, m) in &inner.defs {
            if m.iter().any(|(q, d)| inside(*q) && !inside_new_open(f, d)) {
                fields.push(*f);
            }
        }
        let lo = seg.lo;
        drop(inner);
        if let Some(q) = stop {
            end = Some((sid, q));
            break;
        }
        first = false;
        cur = eng.order.next(sid, lo);
    }
    // (where the depths agree again holds for the brackets as they are
    // now; a reader's answer rests on them as they were when it read. A
    // change filed since this one (any node's) may be one of those the
    // walk passed or stopped at, and its own walk, from where the depths
    // already reflect this change, may find nothing moved: a close filed
    // just after another close's walk began made each walk stop at the
    // other, and the readers after both kept a definition both closes
    // hide (note-intermittents §1: \begin's scope kind at -j16). Then
    // every reader past the stop is re-asked too, each by its answer)
    // (what is not filed at positions: the end readers, the relations and,
    // to the end of the line, the finals: in the region, or from the
    // change on after a race)
    let mut rest = (at, end.map(|(_, q)| q));
    // (none where the region is empty, the two sides alike from `at` on:
    // a close at the bottom level, which closes nothing, whatever filed
    // around it. A change that moves the depth at `at` walks on past it
    // itself, and found here the depth it made)
    if let Some((sid, q)) = end
        && q != at
        && brackets_changed_since(eng, at, sid, by.bracket_gen.load(Ordering::Acquire))
    {
        end = None;
        rest = (at, None);
        // (of the fields whose scopes this change can move, as on the
        // walk: past the stop the two sides agree, and what the other
        // change and this one move together is a definition in one of
        // their regions, or before it in a scope open there, whose scope
        // ends elsewhere now; the other's walk asks those of its own)
        let mut cur = Some(sid);
        let mut first = true;
        while let Some(sid) = cur {
            let seg = eng.seg(sid);
            let inner = seg.inner.read();
            for (f, rs) in &inner.readers {
                let all = reach.all || *f == eng.depth_field || reach.fields.contains(f);
                for r in rs {
                    if (all || read::derived_spec(eng, r.path).is_some())
                        && (!first || eng.node(r.anchor).pos_hint() > q)
                        && live(eng, r.node, r.serial)
                    {
                        readers.push((*f, r.clone()));
                    }
                }
            }
            let lo = seg.lo;
            drop(inner);
            first = false;
            cur = eng.order.next(sid, lo);
        }
    }
    st.scope_asked += readers.len() as u64;
    {
        let _memo = read::WalkMemo::new();
        for (f, r) in &readers {
            reask_reader(eng, w, *f, r, Some(by.id), st);
        }
    }
    // (the relations, end readers and finals likewise: of those fields;
    // all of them were put right or asked to the line's end at each walk
    // that found no stop, every definition on the way looked at)
    let only = (!reach.all).then(|| {
        let mut o = reach.fields.clone();
        o.insert(eng.depth_field);
        o
    });
    reask_rest(eng, w, by, rest.0, rest.1, only.as_ref(), st);
    // (past the region only the readers of what is defined in it can see
    // another answer)
    if let Some((sid, q)) = end {
        fields.sort_unstable();
        fields.dedup();
        for f in fields {
            reask_from(eng, w, f, q, sid, None, None, 0, st);
        }
    }
}

/// What a walk of every reader from `from` (to `to`, inclusive, where
/// given; of the fields `only` where given) asks besides the readers
/// filed at positions: each relation defined in that span put right from its input (and the readers of one
/// whose value moved, by its own walk), the end readers of each unit
/// that ends in it and, with no bound, every published final. (After the
/// research version, `xwalk::reask_rest` of the parallelism branch.)
fn reask_rest(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    from: Pos,
    to: Option<Pos>,
    only: Option<&FxHashSet<Field>>,
    st: &mut CommitStats,
) {
    let wanted = |f: &Field| only.is_none_or(|o| o.contains(f));
    let inside = |q: Pos| q >= from && to.is_none_or(|t| q <= t);
    // (relations in line order: one put right feeds the next one's input)
    let mut rels: Vec<(Field, NodeId, i64)> = Vec::new();
    let mut cur = eng.seg_for_pos(from).map(|s| s.id);
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        let inner = seg.inner.read();
        let mut here: Vec<(Pos, Field, NodeId, i64)> = Vec::new();
        for (f, m) in inner.defs.iter().filter(|(f, _)| wanted(f)) {
            for (q, d) in m.iter() {
                if let Some(c) = d.rel
                    && inside(*q)
                {
                    here.push((*q, *f, d.by, c));
                }
            }
        }
        let past = to.is_some_and(|t| inner.hi > t);
        let lo = seg.lo;
        drop(inner);
        here.sort_unstable_by_key(|x| x.0);
        rels.extend(here.into_iter().map(|(_, f, n, c)| (f, n, c)));
        if past {
            break;
        }
        cur = eng.order.next(sid, lo);
    }
    for (f, n, c) in rels {
        if settle_rel(eng, f, n, c) {
            st.relative_resolved += 1;
            let dn = eng.node(n);
            reask(eng, w, f, dn.pos_hint(), dn.seg_hint(), n, st);
        }
    }
    reask_ends_after(eng, w, by, from, to, only, st);
    if to.is_none() {
        match only {
            None => reask_final_all(eng),
            Some(o) => o.iter().for_each(|&f| reask_final(eng, f)),
        }
    }
}

/// The end readers of each unit ending from `from` (to `to`, inclusive,
/// where given), of the fields `only` where given.
fn reask_ends_after(
    eng: &Engine,
    w: Option<&Worker>,
    by: &Node,
    from: Pos,
    to: Option<Pos>,
    only: Option<&FxHashSet<Field>>,
    st: &mut CommitStats,
) {
    for (f, u) in eng.ranges.end_fields() {
        let q = Pos(eng.unit(u).end.0.saturating_sub(1));
        if only.is_none_or(|o| o.contains(&f)) && q >= from && to.is_none_or(|t| q <= t) {
            reask_ends(eng, w, f, u, Some(by.id), st);
        }
    }
}

/// The scope depth a definition is filed at (`segment::Def::depth`),
/// relative to its node's depth after its closes: `Local` 0, `Scope` its
/// opens, `ScopeAt(k)` `k + 1`, `Global` everywhere (`GLOBAL`).
pub(crate) fn def_depth(loc: Locality, b: &Brackets) -> u16 {
    match loc {
        Locality::Local => 0,
        Locality::Global => GLOBAL,
        Locality::Scope => u16::from(b.opens),
        Locality::ScopeAt(k) => u16::from(k) + 1,
    }
}

#[cfg(test)]
mod tests {
    //! A claimer's two walks forced to interleave (`apply_claim`): its
    //! give-back paused between reading `claim_end` and storing its own,
    //! while its commit's walk takes a sibling past it. Found on amsldoc
    //! at sixteen threads: the give-back's store left `claim_end` short of
    //! the sibling, the next give-back stopped there, and the sibling
    //! stayed taken by a node removed, its text never run.

    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Barrier, Mutex};
    use std::time::Duration;

    use crate::api::{NodeId, NodeState, Operation, Stop};
    use crate::ctx::Ctx;
    use crate::engine::Engine;
    use crate::values::ValueId;

    /// The node whose walk with `want` 0 pauses after reading
    /// `claim_end` (raw id + 1; 0: none), and the barrier it waits at
    /// twice: once to say it paused, once to go on.
    static PAUSE_AT: AtomicU64 = AtomicU64::new(0);
    static PAUSE: Mutex<Option<Arc<Barrier>>> = Mutex::new(None);
    /// Held by each test here for its length: node ids are per engine, so
    /// another test's node could stand for the one paused at.
    static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

    /// The node whose walk pauses after it stored its `claim_end` (raw
    /// id + 1), before what it gave back is filed and queued; the same
    /// barrier.
    static PAUSE_AFTER: AtomicU64 = AtomicU64::new(0);

    pub(super) fn paused_after(me: NodeId) {
        if PAUSE_AFTER.load(Ordering::Acquire) != me.raw() + 1 {
            return;
        }
        PAUSE_AFTER.store(0, Ordering::Release);
        let b = PAUSE.lock().unwrap().clone();
        if let Some(b) = b {
            b.wait();
            b.wait();
        }
    }

    pub(super) fn paused(me: NodeId, want: usize) {
        if want != 0 || PAUSE_AT.load(Ordering::Acquire) != me.raw() + 1 {
            return;
        }
        PAUSE_AT.store(0, Ordering::Release);
        let b = PAUSE.lock().unwrap().clone();
        if let Some(b) = b {
            b.wait();
            b.wait();
        }
    }

    /// `[n]`: claims its next n siblings.
    struct Claimer;

    impl Operation for Claimer {
        fn name(&self) -> &'static str {
            "claimer"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let n = usize::try_from(cx.values().as_int(cx.args()[0]).unwrap()).unwrap();
            for i in 0..n {
                let _ = cx.following(i);
            }
            cx.claim(n);
            Ok(ValueId::UNIT)
        }
    }

    /// `[x]`: outputs x.
    struct Text;

    impl Operation for Text {
        fn name(&self) -> &'static str {
            "text"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(cx.args()[0])
        }
    }

    /// `[f]`: claims the rest of the text (`Ctx::claim_rest`) where f is
    /// 1, nothing otherwise.
    struct RestIf;

    impl Operation for RestIf {
        fn name(&self) -> &'static str {
            "rest_if"
        }

        fn text(&self) -> bool {
            true
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            if cx.read(crate::values::Field(cx.args()[0]))?.and_then(|x| v.as_int(x)) == Some(1) {
                cx.claim_rest();
            } else {
                cx.claim(0);
            }
            Ok(ValueId::UNIT)
        }
    }

    /// `[x]`: a text node.
    struct Piece;

    impl Operation for Piece {
        fn name(&self) -> &'static str {
            "piece"
        }

        fn text(&self) -> bool {
            true
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(cx.args()[0])
        }
    }

    /// `[]`: not text; places a `Piece` after itself.
    struct Placer(crate::api::OpId);

    impl Operation for Placer {
        fn name(&self) -> &'static str {
            "placer"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            cx.follow(self.0, vec![cx.values().int(9)]);
            Ok(ValueId::UNIT)
        }
    }

    /// A sort of nodes by label while another thread rebalances their
    /// segments, relabelling them again and again: each label is read
    /// once (`sort_by_label`), so the sort never meets a key that moved
    /// under it, and every sort gives an order (a key read again in the
    /// comparator broke the sort's total order: a panic at the page loop's
    /// commit in Grail at -j16).
    #[test]
    fn a_sort_by_label_holds_while_rebalances_move_the_labels() {
        let e = Arc::new(Engine::new());
        let piece = e.register(Arc::new(Piece));
        let u = e.new_unit();
        let ids: Vec<NodeId> = (0..4000).map(|i| e.push(u, piece, vec![e.values().int(i)])).collect();
        let stop = Arc::new(AtomicBool::new(false));
        let mover = {
            let (e, stop, ids) = (Arc::clone(&e), Arc::clone(&stop), ids.clone());
            std::thread::spawn(move || {
                let mut k = 0usize;
                while !stop.load(Ordering::Acquire) {
                    let seg = e.node(ids[k.wrapping_mul(7919) % ids.len()]).seg_hint();
                    e.order.rebalance(&e, seg, 1 + k % 97);
                    k += 1;
                }
                k
            })
        };
        let t0 = std::time::Instant::now();
        let mut sorts = 0;
        while t0.elapsed() < Duration::from_secs(2) {
            let mut xs = ids.clone();
            xs.reverse();
            super::sort_by_label(&e, &mut xs);
            sorts += 1;
        }
        stop.store(true, Ordering::Release);
        let moves = mover.join().expect("the rebalancing thread");
        assert!(moves > 0 && sorts > 0, "{moves} rebalances during {sorts} sorts");
    }

    #[test]
    fn a_claim_of_the_rest_made_smaller_gives_back_the_placed_text_it_took() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Built with f = 0, then f = 1: C claims the rest of the text, the
        // pushed piece T and the piece R that Q (not text) placed. f set to 0 again, C claims
        // nothing and both run again: R, which no claim of C's siblings
        // takes, is given back too (amsldoc at sixteen threads: an
        // `\item` run on a guess that ended the job took the rests of the
        // pieces after it, and eight stayed taken, their text lost).
        let e = Engine::new();
        let piece = e.register(Arc::new(Piece));
        let rest_if = e.register(Arc::new(RestIf));
        let placer = e.register(Arc::new(Placer(piece)));
        let def = e.register(Arc::new(Def));
        let v = e.values();
        let f = v.field("f");
        let u = e.new_unit();
        let d = e.push(u, def, vec![f.0, v.int(0)]);
        let c = e.push(u, rest_if, vec![f.0]);
        e.push(u, placer, vec![]);
        let t = e.push(u, piece, vec![v.int(1)]);
        e.evaluate(1).unwrap();
        let r = e
            .order(u)
            .into_iter()
            .find(|&n| e.node_info(n).is_some_and(|i| i.op == "piece" && i.parent.is_some()))
            .unwrap();
        e.replace(d, def, vec![f.0, v.int(1)]);
        e.evaluate(1).unwrap();
        for n in [t, r] {
            assert_eq!(
                e.node_info(n).unwrap().claimer,
                Some(c),
                "taken by the claim of the rest"
            );
        }
        e.replace(d, def, vec![f.0, v.int(0)]);
        e.evaluate(1).unwrap();
        for n in [t, r] {
            let i = e.node_info(n).unwrap();
            assert_eq!(
                (i.state, i.claimer.map(|x| x.raw())),
                (NodeState::Done, None),
                "node {}",
                n.raw()
            );
        }
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    }

    /// `[f]`: claims the rest of the text where f is 1; where f is 2 claims
    /// nothing and keeps what it published last (`Ctx::unchanged`: a
    /// finisher whose page is stale); nothing otherwise. Not text.
    struct RestOrKeep;

    impl Operation for RestOrKeep {
        fn name(&self) -> &'static str {
            "rest_or_keep"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let v = cx.values();
            let f = cx.read(crate::values::Field(cx.args()[0]))?.and_then(|x| v.as_int(x));
            if f == Some(1) {
                cx.claim_rest();
            } else {
                if f == Some(2) {
                    cx.unchanged();
                }
                cx.claim(0);
            }
            Ok(ValueId::UNIT)
        }
    }

    /// `[x]`: not text, a sibling to a claim.
    struct Mark;

    impl Operation for Mark {
        fn name(&self) -> &'static str {
            "mark"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(cx.args()[0])
        }
    }

    #[test]
    fn a_kept_claim_of_the_rest_stays_one_of_the_rest() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // C claims the rest of the text (f = 1): the piece T, not the mark
        // M, which is no text. Run again with f = 2, C publishes nothing
        // new (`Ctx::unchanged`): its last claim stands, of the rest, so M
        // stays free. The claim kept was read as one of usize::MAX
        // siblings, and took M (a page's finisher, its routine ended by a
        // fatal error on values not settled, run again on a stale page:
        // it took the next loop's finishers, inert for good; thumbs at
        // -j8, accl 25360).
        let e = Engine::new();
        let piece = e.register(Arc::new(Piece));
        let keep = e.register(Arc::new(RestOrKeep));
        let mark = e.register(Arc::new(Mark));
        let def = e.register(Arc::new(Def));
        let v = e.values();
        let f = v.field("f");
        let u = e.new_unit();
        let d = e.push(u, def, vec![f.0, v.int(1)]);
        let c = e.push(u, keep, vec![f.0]);
        let m = e.push(u, mark, vec![v.int(0)]);
        let t = e.push(u, piece, vec![v.int(1)]);
        for threads in [1, 4] {
            e.evaluate(threads).unwrap();
            assert_eq!(e.node_info(t).unwrap().claimer, Some(c), "-j{threads}");
            assert_eq!(e.node_info(m).unwrap().claimer, None, "-j{threads}");
            e.replace(d, def, vec![f.0, v.int(2)]);
            e.evaluate(threads).unwrap();
            assert_eq!(e.node_info(t).unwrap().claimer, Some(c), "kept: -j{threads}");
            let i = e.node_info(m).unwrap();
            assert_eq!((i.state, i.claimer), (NodeState::Done, None), "-j{threads}");
            assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
            e.replace(d, def, vec![f.0, v.int(1)]);
        }
    }

    /// `[f, x]`: defines f = x.
    struct Def;

    impl Operation for Def {
        fn name(&self) -> &'static str {
            "def"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            cx.define(crate::values::Field(cx.args()[0]), cx.args()[1]);
            Ok(cx.args()[1])
        }
    }

    #[test]
    fn a_node_removed_gives_back_what_it_held_whatever_the_labels_became() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // C holds T0 and T1. C is removed; its give-back pauses after
        // reading claim_end while the segment is relabelled evenly (a
        // rebalance, as a placement that finds the labels used up makes
        // one). T0 and T1 go back however their labels moved.
        let e = Engine::new();
        let claimer = e.register(Arc::new(Claimer));
        let text = e.register(Arc::new(Text));
        let v = e.values();
        let u = e.new_unit();
        let c = e.push(u, claimer, vec![v.int(2)]);
        let ts: Vec<NodeId> = (0..3).map(|k| e.push(u, text, vec![v.int(k)])).collect();
        e.evaluate(1).unwrap();
        assert_eq!(e.claimed(c), vec![ts[0], ts[1]]);
        // (the four spread over the segment, then thirty after them in the
        // gap after T2: the relabel below spreads them all, and T0 and T1
        // land below the label C had)
        let seg = e.node(ts[0]).seg_hint();
        assert!(e.order.rebalance(&e, seg, 0));
        for k in 0..30 {
            e.push(u, text, vec![v.int(10 + k)]);
        }
        e.evaluate(1).unwrap();
        let c_at = e.node(c).pos_hint();
        let b = Arc::new(Barrier::new(2));
        *PAUSE.lock().unwrap() = Some(b.clone());
        PAUSE_AT.store(c.raw() + 1, Ordering::Release);
        let moved = std::thread::scope(|s| {
            let er = &e;
            let remover = s.spawn(move || er.remove(c));
            b.wait();
            let seg = e.node(ts[0]).seg_hint();
            e.order.rebalance(&e, seg, 0);
            let moved = e.node(ts[0]).pos_hint() < c_at;
            b.wait();
            remover.join().unwrap();
            moved
        });
        *PAUSE.lock().unwrap() = None;
        assert!(moved, "the relabel moved T0 below the label C had ({c_at:?})");
        for (k, &t) in ts.iter().enumerate() {
            let i = e.node_info(t).unwrap();
            assert!(
                i.state != NodeState::Inert && i.claimer.is_none(),
                "T{k} still held by {:?}",
                i.claimer.map(|n| n.raw())
            );
        }
    }

    /// `[]`: places a `Kid` after itself.
    struct PlaceKid(crate::api::OpId);

    impl Operation for PlaceKid {
        fn name(&self) -> &'static str {
            "place_kid"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            cx.follow(self.0, vec![]);
            Ok(ValueId::UNIT)
        }
    }

    /// `[]`: places a `Text` as its child.
    struct Kid(crate::api::OpId);

    impl Operation for Kid {
        fn name(&self) -> &'static str {
            "kid"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            cx.child(self.0, vec![cx.values().int(5)]);
            Ok(ValueId::UNIT)
        }
    }

    #[test]
    fn a_node_relocated_to_a_place_gone_takes_its_placements_with_it() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // K, placed by P, with its child L, is moved back to after A, which
        // is removed meanwhile: K has nowhere to go and is removed, and L,
        // past where K was going, is no mover. L goes with K. Its record
        // was cleared before its placements were looked up, which found
        // none: amsldoc at sixteen threads left a finisher's page writer
        // on the line, its placer gone (note-intermittents §2).
        let e = Engine::new();
        let text = e.register(Arc::new(Text));
        let kid = e.register(Arc::new(Kid(text)));
        let place_kid = e.register(Arc::new(PlaceKid(kid)));
        let v = e.values();
        let u = e.new_unit();
        let a = e.push(u, text, vec![v.int(1)]);
        e.push(u, text, vec![v.int(2)]);
        let p = e.push(u, place_kid, vec![]);
        e.evaluate(1).unwrap();
        let of = |name: &str| {
            e.order(u)
                .into_iter()
                .find(|&n| e.node_info(n).is_some_and(|i| i.op == name))
                .unwrap()
        };
        let k = of("kid");
        let l = e
            .order(u)
            .into_iter()
            .find(|&n| e.node_info(n).is_some_and(|i| i.parent == Some(k)))
            .unwrap();
        e.remove(a);
        let mut st = super::CommitStats::default();
        let moved = super::relocate(
            &e,
            None,
            p,
            e.node(k),
            super::Target::After(a),
            &Default::default(),
            &mut st,
        );
        assert!(!moved, "K had nowhere to go");
        assert_eq!(e.node(k).state(), crate::node::State::Gone);
        assert_eq!(e.node(l).state(), crate::node::State::Gone, "K's child L");
        let bad: Vec<String> = e
            .check_invariants()
            .into_iter()
            .filter(|m| m.contains("holding it no more"))
            .collect();
        assert!(bad.is_empty(), "{bad:?}");
    }

    /// `[f, k]`: opens a scope after itself and defines f = k inside it.
    struct Opens;

    impl Operation for Opens {
        fn name(&self) -> &'static str {
            "opens"
        }

        fn effect(&self, _: &[ValueId], _: &crate::values::Values) -> crate::api::Effect {
            crate::api::Effect::Scope(crate::api::Brackets {
                closes: 0,
                opens: 1,
                before: false,
            })
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            let (f, k) = (crate::values::Field(cx.args()[0]), cx.args()[1]);
            cx.define_with(f, k, crate::api::Locality::Scope);
            Ok(k)
        }
    }

    /// `[f]`: outputs the definition of f reaching it.
    struct Reads;

    impl Operation for Reads {
        fn name(&self) -> &'static str {
            "reads"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(cx.read(crate::values::Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
        }
    }

    #[test]
    fn two_closes_filed_before_either_walk_reask_the_readers_after_both() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // B opens a scope and defines f in it; X, Y and R read f, R last.
        // X's and Y's runs each find a close, filed one after the other
        // before either walk: X's walk, from depths that already have Y's
        // close, stops at Y (past it the depths agree), and Y's, from a
        // depth X's close already brought to 0, stops at itself. R read f
        // inside the scope and now finds none: it has to run again
        // (amsldoc at sixteen threads: the readers after an environment's
        // \begin kept its scope kind, note-intermittents §1).
        let e = Engine::new();
        let opens = e.register(Arc::new(Opens));
        let reads = e.register(Arc::new(Reads));
        let v = e.values();
        let f = v.field("f").0;
        let u = e.new_unit();
        e.push(u, opens, vec![f, v.int(7)]);
        let x = e.push(u, reads, vec![f]);
        let y = e.push(u, reads, vec![f]);
        let r = e.push(u, reads, vec![f]);
        e.evaluate(1).unwrap();
        assert_eq!(e.output(r).and_then(|o| v.as_int(o)), Some(7));
        let close = crate::api::Brackets {
            closes: 1,
            opens: 0,
            before: false,
        };
        let file = |n: NodeId| {
            let node = e.node(n);
            let (_, mut inner) = super::own_write(&e, node).expect("on the line");
            super::file_brackets(&e, &mut inner, node, node.pos_hint(), close).expect("a change")
        };
        let (ox, oy) = (file(x), file(y));
        let mut st = super::CommitStats::default();
        super::reask_after_effect(&e, None, e.node(x), e.node(x).pos_hint(), ox, close, &mut st);
        super::reask_after_effect(&e, None, e.node(y), e.node(y).pos_hint(), oy, close, &mut st);
        assert_ne!(
            e.node(r).state(),
            crate::node::State::Clean,
            "R kept f, which both closes hide from it"
        );
    }

    #[test]
    fn a_node_removed_claims_nothing_after() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // C, claiming nothing, is removed; a run of it still going then
        // claims two (amsldoc at sixteen threads: an exec removed with its
        // placer claimed ten pieces after its removal gave back what it
        // held). The give-back of a claimer gone after its walk pauses
        // while the segment is relabelled evenly, T0 landing below the
        // label C kept. Nothing is held: a removed node claims nothing.
        let e = Engine::new();
        let claimer = e.register(Arc::new(Claimer));
        let text = e.register(Arc::new(Text));
        let v = e.values();
        let u = e.new_unit();
        let c = e.push(u, claimer, vec![v.int(0)]);
        let ts: Vec<NodeId> = (0..3).map(|k| e.push(u, text, vec![v.int(k)])).collect();
        e.evaluate(1).unwrap();
        let seg = e.node(ts[0]).seg_hint();
        assert!(e.order.rebalance(&e, seg, 0));
        for k in 0..30 {
            e.push(u, text, vec![v.int(10 + k)]);
        }
        e.evaluate(1).unwrap();
        e.remove(c);
        let c_at = e.node(c).pos_hint();
        let b = Arc::new(Barrier::new(2));
        *PAUSE.lock().unwrap() = Some(b.clone());
        PAUSE_AT.store(c.raw() + 1, Ordering::Release);
        let moved = std::thread::scope(|s| {
            let er = &e;
            let run = s.spawn(move || super::apply_claim(er, None, er.node(c), 2));
            b.wait();
            let seg = e.node(ts[0]).seg_hint();
            e.order.rebalance(&e, seg, 0);
            let moved = e.node(ts[0]).pos_hint() < c_at;
            b.wait();
            run.join().unwrap();
            moved
        });
        *PAUSE.lock().unwrap() = None;
        assert!(moved, "the relabel moved T0 below the label C had ({c_at:?})");
        for (k, &t) in ts.iter().enumerate() {
            let i = e.node_info(t).unwrap();
            assert!(
                i.state != NodeState::Inert && i.claimer.is_none(),
                "T{k} still held by {:?}",
                i.claimer.map(|n| n.raw())
            );
        }
        assert!(e.check_invariants().is_empty(), "{:?}", e.check_invariants());
    }

    #[test]
    fn a_sibling_given_back_while_another_claim_walks_is_held_by_that_claim() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // P claims nothing, Q holds T. Q's claim goes to none: its walk
        // gives T back and pauses before T is queued; P's claim of two
        // then takes Q and T. T is P's: held by it, not run on its own
        // (amsldoc at sixteen threads: a `\begin{minipage}` claiming 21
        // counted its `\end{center}` while a resume's give-back of it was
        // under way, and the `\end{center}` then ran as a node too: 'Extra
        // \endgroup', md5 f5ec0654).
        let e = Engine::new();
        let claimer = e.register(Arc::new(Claimer));
        let text = e.register(Arc::new(Text));
        let v = e.values();
        let u = e.new_unit();
        let p = e.push(u, claimer, vec![v.int(0)]);
        let q = e.push(u, claimer, vec![v.int(1)]);
        let t = e.push(u, text, vec![v.int(7)]);
        e.evaluate(1).unwrap();
        assert_eq!(e.node_info(t).unwrap().claimer, Some(q));
        let b = Arc::new(Barrier::new(2));
        *PAUSE.lock().unwrap() = Some(b.clone());
        PAUSE_AFTER.store(q.raw() + 1, Ordering::Release);
        std::thread::scope(|s| {
            let er = &e;
            let give_back = s.spawn(move || super::apply_claim(er, None, er.node(q), 0));
            b.wait();
            super::apply_claim(&e, None, e.node(p), 2);
            b.wait();
            give_back.join().unwrap();
        });
        *PAUSE.lock().unwrap() = None;
        let i = e.node_info(t).unwrap();
        assert_eq!((i.state, i.claimer), (NodeState::Inert, Some(p)), "T held by P");
    }

    #[test]
    fn a_give_back_interleaved_with_a_claim_leaves_nothing_held() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // C holds T0. Its give-back (want 0) reads claim_end (T0) and
        // pauses; its commit's walk then takes T0 and T1; the give-back
        // goes on. Whatever order the lock lets them run in, a last
        // give-back leaves T0 and T1 held by no one.
        let e = Engine::new();
        let claimer = e.register(Arc::new(Claimer));
        let text = e.register(Arc::new(Text));
        let v = e.values();
        let u = e.new_unit();
        let c = e.push(u, claimer, vec![v.int(1)]);
        let ts: Vec<NodeId> = (0..3).map(|k| e.push(u, text, vec![v.int(k)])).collect();
        e.evaluate(1).unwrap();
        assert_eq!(e.claimed(c), vec![ts[0]]);
        let b = Arc::new(Barrier::new(2));
        *PAUSE.lock().unwrap() = Some(b.clone());
        PAUSE_AT.store(c.raw() + 1, Ordering::Release);
        let a_done = Arc::new(AtomicBool::new(false));
        std::thread::scope(|s| {
            let give_back = s.spawn(|| super::apply_claim(&e, None, e.node(c), 0));
            b.wait();
            let done = a_done.clone();
            let er = &e;
            let walk = s.spawn(move || {
                super::apply_claim(er, None, er.node(c), 2);
                done.store(true, Ordering::Release);
            });
            // (the walk runs while the give-back is paused, unless a lock
            // holds it back: then the give-back goes on first)
            let t = std::time::Instant::now();
            while !a_done.load(Ordering::Acquire) && t.elapsed() < Duration::from_millis(200) {
                std::thread::sleep(Duration::from_millis(1));
            }
            b.wait();
            give_back.join().unwrap();
            walk.join().unwrap();
        });
        *PAUSE.lock().unwrap() = None;
        // (C taken or removed: what it holds goes back)
        super::release_claimed(&e, None, e.node(c));
        for (k, &t) in ts.iter().enumerate() {
            let i = e.node_info(t).unwrap();
            assert!(
                i.state != NodeState::Inert && i.claimer.is_none(),
                "T{k} still held by {:?}",
                i.claimer.map(|n| n.raw())
            );
        }
    }

    /// A contribution's walk asks each range reader filed at its label
    /// whether the reader's range covers it (`covers`), comparing its
    /// label with the range's ends: while another thread fills a gap of
    /// their segment and relabels it again and again, a contribution
    /// before a reader's end is always found inside its range (labels
    /// compared across a relabel took one before a reader for one after
    /// it, and the reader was never re-asked: linkif's \pdflastlink read
    /// with no page's objects before it, linkobj, read_time_numbers in
    /// 1 of 10 suite runs on 8 CPUs, issue #5).
    #[test]
    fn a_contribution_before_a_readers_end_is_covered_while_labels_move() {
        use crate::node::ReadKind;
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = Arc::new(Engine::new());
        let text = e.register(Arc::new(Piece));
        let u = e.new_unit();
        let ids: Vec<NodeId> = (0..200).map(|i| e.push(u, text, vec![e.values().int(i)])).collect();
        e.evaluate(1).expect("evaluated");
        // (readers of the whole range up to themselves, each just after a
        // contributor, as the pieces after a page's finisher)
        let readers: Vec<super::RangeReader> = (0..10)
            .map(|m| ids[141 + 2 * m])
            .map(|n| super::RangeReader {
                node: n,
                serial: e.node(n).serial.load(Ordering::Acquire),
                from: None,
                to: None,
                next: false,
                incl: false,
                hole: false,
                seen: Arc::from([]),
            })
            .collect();
        let stop = Arc::new(AtomicBool::new(false));
        let mover = {
            let (e, stop, ids) = (Arc::clone(&e), Arc::clone(&stop), ids.clone());
            std::thread::spawn(move || {
                let mut k = 0usize;
                while !stop.load(Ordering::Acquire) {
                    // (before the readers: their labels and the
                    // contributors' move with each relabel)
                    let at = ids[k.wrapping_mul(7919) % 150];
                    let put: Vec<NodeId> = (0..24)
                        .map(|_| e.insert_after(at, text, vec![e.values().int(-1)]))
                        .collect();
                    for n in put {
                        e.remove(n);
                    }
                    k += 1;
                }
                k
            })
        };
        let t0 = std::time::Instant::now();
        let (mut asks, mut missed) = (0u64, 0u64);
        let mut k = 0usize;
        while t0.elapsed() < Duration::from_secs(2) {
            // (as `reask_ranges_in` asks: the contributor's label, then for
            // each reader filed there whether its range covers it, a
            // re-ask's work between one and the next)
            let m = k % 10;
            k += 1;
            let by = ids[140 + 2 * m];
            let at = crate::read::pos_of(&e, by).expect("on the line");
            for r in &readers[m..] {
                asks += 1;
                if !super::covers(&e, r, by, at, false) {
                    missed += 1;
                }
                let _ = crate::read::answer(&e, e.node(r.node), &ReadKind::Hole, crate::api::PathId::WHOLE);
            }
        }
        stop.store(true, Ordering::Release);
        let moves = mover.join().expect("the moving thread");
        assert!(moves > 0, "no move during {asks} asks");
        assert_eq!(
            missed, 0,
            "{missed} of {asks} contributions before a reader missed during {moves} gap fills"
        );
    }

    /// `[name, x]`: defines `name` := x.
    struct SetField;

    impl Operation for SetField {
        fn name(&self) -> &'static str {
            "set"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            cx.define(crate::values::Field(cx.args()[0]), cx.args()[1]);
            Ok(cx.args()[1])
        }
    }

    /// `[name]`: reads `name` and outputs it.
    struct GetField;

    impl Operation for GetField {
        fn name(&self) -> &'static str {
            "get"
        }

        fn run(&self, cx: &mut Ctx<'_>) -> Result<ValueId, Stop> {
            Ok(cx.read(crate::values::Field(cx.args()[0]))?.unwrap_or(ValueId::UNIT))
        }
    }

    /// A node lifted off its label (`relocate`) and its segment's labels
    /// moved before the walks from its old place: a relabel leaves the
    /// free label as it was and renumbers the nodes, so the reader that
    /// stood after the node now stands at or before that label. The walk
    /// from the old place still asks it (`old_place`: from the node the
    /// lifted one stood after). The walk from the old label passed over
    /// it: cleanthesis at -j4, a page's finisher kept the page number of
    /// a later page's finisher that had stood before it and moved past it.
    #[test]
    fn the_walk_from_a_lifted_nodes_place_reaches_its_readers_after_a_relabel() {
        let _one = ONE_AT_A_TIME.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let e = Engine::new();
        let set = e.register(Arc::new(SetField));
        let get = e.register(Arc::new(GetField));
        let v = e.values();
        let (m, o) = (v.field("m"), v.field("other"));
        let u = e.new_unit();
        let _d = e.push(u, set, vec![m.0, v.int(1)]);
        let x = e.push(u, set, vec![m.0, v.int(7)]);
        let r = e.push(u, get, vec![m.0]);
        for i in 0..8 {
            e.push(u, set, vec![o.0, v.int(i)]);
        }
        e.evaluate(1).unwrap();
        assert_eq!(e.output(r).and_then(|y| v.as_int(y)), Some(7));
        let xn = Arc::clone(e.node(x));
        let left = super::lift(&e, &xn).expect("x lifted");
        // (its segment's labels moved: relabelled as a landing's relabel
        // does, packed near the segment's start, as a narrow segment's are)
        {
            let seg = e.seg(left.seg);
            let mut inner = seg.inner.write();
            let hi = inner.hi;
            inner.hi = crate::order::Pos(seg.lo.0 + 64);
            seg.relabel(&mut inner, &e);
            inner.hi = hi;
        }
        assert!(
            e.node(r).pos_hint() <= left.pos,
            "the reader now at or before the free label: {} vs {}",
            e.node(r).pos_hint().0,
            left.pos.0
        );
        let pos = super::old_place(&e, &left).0;
        let how = format!(
            "moves {} -> {}, before {:?} at {:?}, after {:?}, lifted at {} seg {}",
            left.moves,
            e.seg(left.seg).moved.load(Ordering::Acquire),
            left.before.map(NodeId::raw),
            left.before.and_then(|b| crate::read::pos_of(&e, b)).map(|p| p.0),
            left.after.map(NodeId::raw),
            left.pos.0,
            left.seg.0
        );
        super::lifted_walks(&e, None, &xn, left, &mut super::CommitStats::default());
        assert_eq!(
            e.node(r).state(),
            crate::node::State::Dirty,
            "the reader that read x's definition is asked again (reader at {}, the old place's walk from {}; {how})",
            e.node(r).pos_hint().0,
            pos.0
        );
    }
}
