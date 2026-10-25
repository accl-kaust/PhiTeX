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

use crate::api::{Cause, Locality, NodeId, OpId, PathId, UnitId};
use crate::engine::Engine;
use crate::lock::ns;
use crate::node::{Anchor, DefRec, Node, Placement, Product, ReadKind, RunRecord, State, Wake};
use crate::order::{Pos, SegId};
use crate::read::{self, Reaching, next_contribution, reaching_at, reader_answer, through};
use crate::sched::{Worker, enqueue, mark_dirty, mark_stale};
use crate::index::RangeKey;
use crate::segment::{Def, Inner, Made, RangeReader, Reader, Segment, live, push_pruned};
use crate::values::{Field, FxHashMap, FxHashSet, ValueId};

/// A range read's field, ends, and whether a `next` or with `to`'s own.
type RangeOf = (Field, Option<NodeId>, Option<NodeId>, bool, bool);

/// Counts for the report.
#[derive(Default, Clone, Copy)]
pub(crate) struct CommitStats {
    pub(crate) woken: u64,
    pub(crate) relative_resolved: u64,
    pub(crate) predictions_confirmed: u64,
    pub(crate) predictions_refuted: u64,
    pub(crate) shaped: u64,
    pub(crate) commit_ns: u64,
    pub(crate) reask_ns: u64,
}

impl CommitStats {
    pub(crate) fn add(&mut self, o: &CommitStats) {
        self.woken += o.woken;
        self.relative_resolved += o.relative_resolved;
        self.predictions_confirmed += o.predictions_confirmed;
        self.predictions_refuted += o.predictions_refuted;
        self.shaped += o.shaped;
        self.commit_ns += o.commit_ns;
        self.reask_ns += o.reask_ns;
    }
}

/// The node's segment, write-locked, with the node still in it; None when
/// the node is gone.
pub(crate) fn own_write<'a>(eng: &'a Engine, node: &Node) -> Option<(&'a Arc<Segment>, crate::lock::WriteGuard<'a, Inner>)> {
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
pub(crate) fn subtree_end(eng: &Engine, n: NodeId) -> NodeId {
    let mut best = n;
    if let Some(r) = eng.node(n).run_record() {
        for &c in &r.placed {
            if eng.node(c).state() == State::Gone {
                continue;
            }
            let e = subtree_end(eng, c);
            if eng.node(e).pos_hint() > eng.node(best).pos_hint() {
                best = e;
            }
        }
    }
    best
}

/// One contribution per field: several to the same field combined.
fn combine_dups(eng: &Engine, xs: Vec<(Field, ValueId)>) -> Vec<(Field, ValueId)> {
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

/// The last definition of each field wins.
fn dedup_defs(defs: Vec<DefRec>) -> Vec<DefRec> {
    let mut seen = FxHashSet::default();
    let mut out: Vec<DefRec> = Vec::with_capacity(defs.len());
    for d in defs.into_iter().rev() {
        if seen.insert(d.f) {
            out.push(d);
        }
    }
    out.reverse();
    out
}

/// Files a reader record in a segment it is anchored in; true for the
/// segment's first reader of `f` (the caller adds it to the index once the
/// lock is released). `spare` is a list made before the lock, used for a
/// field's first.
fn file_reader(eng: &Engine, inner: &mut Inner, f: Field, r: Reader, spare: &mut Vec<Reader>) -> bool {
    let v = inner.readers.entry(f).or_insert_with(|| std::mem::take(spare));
    let first = v.is_empty();
    push_pruned(v, r, |x| live(eng, x.node, x.serial));
    first
}

/// Index changes made under a segment's lock, applied after it: a reader
/// that misses the entry meanwhile is put right by the walk that follows,
/// or by its own commit's check of its reads: each entry's field is
/// counted again once the entry is in.
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

    fn apply(self, eng: &Engine, seg: &Segment) {
        for f in self.defs_add {
            eng.defs.add(f, seg.lo, seg.id);
            eng.bump(f);
        }
        for f in self.contribs_add {
            eng.contribs.add(f, seg.lo, seg.id);
            eng.bump(f);
        }
        for f in self.readers_add {
            eng.readers.add(f, seg.lo, seg.id);
        }
        for f in self.defs_remove {
            eng.defs.remove(f, seg.lo, seg.id);
            eng.bump(f);
        }
        for f in self.contribs_remove {
            eng.contribs.remove(f, seg.lo, seg.id);
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
        for (k, (_, r)) in keys.into_iter().zip(&rs[i..j]) {
            if !file_stop(eng, f, k, r) {
                again.push((f, k, r.clone()));
            }
        }
        i = j;
    }
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in again {
        reask_range(eng, w, f, k, &r, by, st, &mut memo);
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
    let Some((_, mut inner)) = own_write(eng, n) else {
        return false;
    };
    if !n.is_hole() {
        return false;
    }
    push_pruned(&mut inner.stops, (f, k, r.clone()), |x| live(eng, x.2.node, x.2.serial));
    true
}

/// Whether a range reader's range covers `at`.
fn covers(eng: &Engine, r: &RangeReader, at: Pos) -> bool {
    let me = eng.node(r.node);
    let from = match r.from {
        Some(n) => eng.node(n).pos_hint(),
        None => read::unit_start(eng, me),
    };
    if at <= from {
        return false;
    }
    match r.to {
        Some(n) => {
            let t = eng.node(n).pos_hint();
            if r.next || r.incl { at <= t } else { at < t }
        }
        None => r.next || at < me.pos_hint(),
    }
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
    for d in &mut p.defs {
        if let Some(c) = d.rel {
            d.value = rel_value_at(eng, d.f, node.id, c);
        }
    }
    let defs = dedup_defs(std::mem::take(&mut p.defs));
    let contribs = combine_dups(eng, std::mem::take(&mut p.contribs));
    let last = node.record();
    let serial = node.serial.load(Ordering::Acquire) + 1;
    let old_placed: Vec<NodeId> = last.as_ref().map(|r| r.placed.clone()).unwrap_or_default();
    let kept = match_placements(eng, &old_placed, &p.placements);
    // everything a placement needs but its label, made before the lock
    let mut fresh: Vec<Option<(Arc<Node>, Predicted)>> = p
        .placements
        .iter()
        .zip(&kept)
        .map(|(what, k)| {
            if k.is_some() && !matches!(what, Placement::FollowAfter(..)) {
                return None;
            }
            let (op, args) = what.key();
            let id = NodeId::from_raw(eng.nodes.reserve() as u64);
            Some(built(eng, node.unit, Some(node.id), op, args.to_vec(), id))
        })
        .collect();
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
        placed: Vec::with_capacity(p.placements.len()),
        claim: p.claim,
        effect: p.effect,
        provisional: false,
    });
    let mut spare_readers: Vec<Reader> = Vec::with_capacity(4);
    let old_fields: Vec<Field> = last.as_ref().map(|r| r.defs.iter().map(|d| d.f).collect()).unwrap_or_default();
    let old_contribs: Vec<Field> = last.as_ref().map(|r| r.contribs.iter().map(|c| c.0).collect()).unwrap_or_default();
    let nd = rec.defs.len() + old_fields.len();
    let nc = rec.contribs.len() + old_contribs.len();
    let mut changed_defs: Vec<Field> = Vec::with_capacity(nd);
    let mut changed_contribs: Vec<Field> = Vec::with_capacity(nc);
    let mut rels: Vec<(Field, i64)> = Vec::with_capacity(rec.defs.len());
    let mut placed: Vec<NodeId> = Vec::with_capacity(p.placements.len());
    let mut made: Vec<Arc<Node>> = Vec::with_capacity(p.placements.len());
    let mut deferred: Vec<(usize, NodeId, Arc<Node>, Predicted)> = Vec::new();
    let mut here_reads: Vec<(Field, Reader)> = Vec::with_capacity(rec.reads.len());
    let mut other_reads: Vec<(Field, Reader, Anchor)> = Vec::new();
    let mut range_reads: Vec<(Field, RangeReader)> = Vec::new();
    let mut range_seen: Vec<Vec<(PathId, Option<ValueId>)>> = Vec::new();
    let mut range_at: FxHashMap<RangeOf, usize> = FxHashMap::default();
    let mut env_reads: Vec<(Field, crate::engine::EnvReader)> = Vec::new();
    // what the lock decides of the walks and of the run's own reads
    let mut walks: Vec<(Field, Here)> = Vec::with_capacity(nd);
    let mut wake: Vec<(NodeId, NodeId, Field)> = Vec::with_capacity(8);
    let mut later: Vec<(Field, Reader, NodeId)> = Vec::new();
    let mut holes: Vec<(Field, RangeKey, RangeReader)> = Vec::new();
    let mut unchecked: Vec<usize> = Vec::with_capacity(rec.reads.len());
    let mut refuted: Option<usize> = None;
    let mut ix = IndexOps::with_capacity(nd, nc, rec.reads.len());
    let mut room: Option<Made> = None;
    for r in &rec.reads {
        match &r.kind {
            ReadKind::Field { f, at } => {
                let (anchor, inclusive) = match *at {
                    Anchor::Me => (node.id, false),
                    Anchor::Before(n) => (n, false),
                    Anchor::AfterOwn(n) => (n, true),
                    Anchor::End(_) => (node.id, true),
                };
                let rec = Reader {
                    node: node.id,
                    serial,
                    anchor,
                    inclusive,
                    path: r.path,
                    seen: r.seen,
                };
                if matches!(at, Anchor::End(_)) {
                    other_reads.push((*f, rec, *at));
                } else {
                    here_reads.push((*f, rec));
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
                let from = match after {
                    Some(n) => Some(eng.node(*n).pos_hint()),
                    None => Some(read::unit_start(eng, node)),
                };
                let i = *range_at.entry((*f, *after, None, true, false)).or_insert_with(|| {
                    let n = next_contribution(eng, *f, from);
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
            }
            ReadKind::Placed { .. } | ReadKind::Following { .. } => {}
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
    let (seg_id, seg_lo, old_output, rec, seg_pos) = loop {
        let Some((seg, mut inner)) = own_write(eng, node) else {
            return st;
        };
        if node.taken() {
            return st;
        }
        if Segment::needs_split(&inner) {
            let id = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, id);
            continue;
        }
        let t_lock = Instant::now();
        let pos = pos_in(&inner, node);
        // room for what is filed, made outside the lock
        if let Some(m) = room.as_mut() {
            m.install(&mut inner);
        }
        let g = inner.grow_for(
            pos,
            rec.defs.iter().map(|d| d.f),
            rec.contribs.iter().map(|c| c.0),
            &read_fields,
        );
        if !g.is_empty() {
            drop(inner);
            room = Some(g.make());
            continue;
        }
        // definitions
        let defs = &rec.defs;
        let contribs = &rec.contribs;
        for d in defs {
            let m = inner.defs.entry(d.f).or_default();
            let first = m.is_empty();
            match m.get_mut(&pos) {
                Some(e) if e.value == d.value => {
                    if e.provisional {
                        st.predictions_confirmed += 1;
                    }
                    e.provisional = false;
                    e.rel = d.rel;
                    e.by = node.id;
                }
                Some(e) => {
                    if e.provisional {
                        st.predictions_refuted += 1;
                    }
                    *e = Def {
                        value: d.value,
                        by: node.id,
                        rel: d.rel,
                        provisional: false,
                    };
                    changed_defs.push(d.f);
                }
                None => {
                    m.insert(
                        pos,
                        Def {
                            value: d.value,
                            by: node.id,
                            rel: d.rel,
                            provisional: false,
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
            if defs.iter().any(|d| d.f == f) {
                continue;
            }
            if let Some(m) = inner.defs.get_mut(&f)
                && m.get(&pos).is_some_and(|e| e.by == node.id)
            {
                m.remove(&pos);
                changed_defs.push(f);
                if m.is_empty() {
                    inner.defs.remove(&f);
                    ix.defs_remove.push(f);
                }
            }
        }
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
            if contribs.iter().any(|c| c.0 == f) {
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
            inner.resum(eng, f);
            eng.bump(f);
        }
        for &f in &changed_defs {
            eng.bump(f);
        }
        // placements
        let mut after = node.id;
        let mut in_seg = true;
        let mut removed_kept: Vec<NodeId> = Vec::new();
        // (a kept placement's subtree end found only where a placement
        // follows it: the last one's subtree may be a whole chain)
        let mut kept_end: Option<NodeId> = None;
        for (i, what) in p.placements.iter().enumerate() {
            if let Some(k) = kept_end.take() {
                after = subtree_end(eng, k);
            }
            // (after one made in this commit and not placed yet: no kept
            // node stands after it)
            let after_pos = eng
                .nodes
                .get(usize::try_from(after.raw()).unwrap_or(usize::MAX))
                .map_or(Pos(u64::MAX), |n| n.pos_hint());
            if let Some(k) = kept[i] {
                let kn = eng.node(k);
                // (one placed after another node stands while it is still
                // after that node)
                let floor = match what {
                    Placement::FollowAfter(at, ..) => after_pos.max(eng.node(*at).pos_hint()),
                    Placement::Child(..) | Placement::Follow(..) => after_pos,
                };
                if kn.state() != State::Gone && kn.pos_hint() > floor {
                    placed.push(k);
                    kept_end = Some(k);
                    continue;
                }
                removed_kept.push(k);
            }
            let (n, pred) = match fresh[i].take() {
                Some(x) => x,
                None => {
                    let (op, args) = what.key();
                    let id = NodeId::from_raw(eng.nodes.reserve() as u64);
                    built(eng, node.unit, Some(node.id), op, args.to_vec(), id)
                }
            };
            let id = n.id;
            placed.push(id);
            let at = match what {
                Placement::FollowAfter(at, _, _) => Some(*at),
                Placement::Child(..) | Placement::Follow(..) => None,
            };
            let in_here = at.is_none()
                && in_seg
                && eng.node(after).seg_hint() == seg.id
                && inner.nodes.get(&eng.node(after).pos_hint()) == Some(&after);
            if in_here {
                made.push(place(eng, seg, &mut inner, after, n, pred));
            } else {
                in_seg = false;
                deferred.push((i, at.unwrap_or(after), n, pred));
            }
            after = id;
        }
        let mut removed: Vec<NodeId> = old_placed.iter().copied().filter(|n| !placed.contains(n)).collect();
        removed.extend(removed_kept);
        // (a placement that relabelled the segment moved this node's label)
        let pos = pos_in(&inner, node);
        // reads anchored here
        node.committing.store(true, Ordering::Release);
        for (f, r) in here_reads.drain(..) {
            let a = eng.node(r.anchor);
            if r.anchor == node.id || (a.seg_hint() == seg.id && inner.nodes.get(&a.pos_hint()) == Some(&r.anchor)) {
                if file_reader(eng, &mut inner, f, r, &mut spare_readers) {
                    ix.readers_add.push(f);
                }
            } else {
                let at = Anchor::Before(r.anchor);
                other_reads.push((f, r, at));
            }
        }
        // the walks, as far as this segment goes
        changed_defs.sort_unstable();
        changed_defs.dedup();
        for &f in &changed_defs {
            walks.push((
                f,
                walk_here(eng, &mut inner, f, pos, node.id, &mut wake, &mut later, &mut st),
            ));
        }
        if serial == 1 && !inner.stops.is_empty() {
            let (mine, rest) = std::mem::take(&mut inner.stops).into_iter().partition(|s| s.2.to == Some(node.id));
            inner.stops = rest;
            holes = mine;
        }
        // the run's own reads at its position, answered again
        for (i, r) in rec.reads.iter().enumerate() {
            if let ReadKind::Field { f, at: Anchor::Me } = r.kind
                && let Some((_, d)) = Segment::reaching(&inner, f, pos, false)
            {
                if refuted.is_none() && through(eng, Reaching::of(d), r.path) != r.seen {
                    refuted = Some(i);
                }
            } else {
                unchecked.push(i);
            }
        }
        if serial == 1 {
            inner.holes = inner.holes.saturating_sub(1);
        }
        Arc::get_mut(&mut rec).expect("the record is not shared yet").placed = std::mem::take(&mut placed);
        let old = node.last.lock().replace(rec.clone());
        node.serial.store(serial, Ordering::Release);
        let old_output = last.as_ref().filter(|r| !r.provisional).map(|r| r.output);
        st.commit_ns = ns(t_lock);
        drop(inner);
        drop(old);
        std::mem::take(&mut ix).apply(eng, seg);
        for id in removed {
            unfile(eng, ww, eng.node(id));
        }
        break (seg.id, seg.lo, old_output, rec, pos);
    };
    // 2. reads anchored elsewhere
    for (f, mut r, at) in other_reads {
        if let Anchor::End(u) = at {
            let Some(last_node) = eng.unit_last_node(u) else {
                continue;
            };
            r.anchor = last_node;
            r.inclusive = true;
        }
        let a = eng.node(r.anchor);
        let mut spare: Vec<Reader> = Vec::with_capacity(4);
        if let Some((seg, mut inner)) = own_write(eng, a) {
            let first = file_reader(eng, &mut inner, f, r, &mut spare);
            drop(inner);
            if first {
                eng.readers.add(f, seg.lo, seg.id);
            }
        }
    }
    file_ranges(eng, ww, range_reads, node.id, &mut st);
    if !env_reads.is_empty() {
        let mut er = eng.env.readers.write();
        for (f, r) in env_reads {
            er.entry(f).or_default().push(r);
        }
    }
    // deferred placements: after a node elsewhere
    for (_, at, n, pred) in deferred {
        made.push(place_after(eng, at, n, pred));
    }
    if p.claim > 0 || last.as_ref().is_some_and(|r| r.claim > 0) {
        apply_claim(eng, ww, node, p.claim);
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
            Here::On(by) => reask_later(eng, ww, f, seg_lo, by, &mut st),
            Here::From(by) => reask(eng, ww, f, eng.node(by).pos_hint(), seg_id, by, &mut st),
            Here::Stopped(h) => stop_at(eng, ww, h, f, &mut st),
        }
    }
    if serial == 1 {
        let defined: Vec<Field> = rec.defs.iter().map(|d| d.f).collect();
        walk_stopped(eng, ww, node, node.pos_hint(), seg_id, &defined, &mut st);
    }
    for &f in &changed_contribs {
        reask_ranges(eng, ww, f, seg_pos, node.id, &mut st);
    }
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in holes {
        reask_range(eng, ww, f, k, &r, node.id, &mut st, &mut memo);
    }
    // (the last first: a worker's deque gives the newest first, so its
    // placements run in their order, a child before the node after it)
    for n in made.iter().rev() {
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
    if old_output != Some(p.output)
        && let Some(parent) = node.parent
    {
        reask_parent(eng, ww, eng.node(parent), node.id, &mut st);
    }
    // its relations from their input now: one that changed before they were
    // filed was computed from the older
    for &(f, c) in &rels {
        if settle_rel(eng, f, node.id, c) {
            st.relative_resolved += 1;
            reask(eng, ww, f, node.pos_hint(), node.seg_hint(), node.id, &mut st);
        }
    }
    // 5. a first relation by this operation
    for (f, c) in rels {
        learn_shape(eng, ww, node.op(), f, c, &mut st);
    }
    // 6. the node's own reads the lock could not answer
    // (one whose field no change has counted since it read stands)
    let refuted = refuted.or_else(|| {
        let mut memo = read::FoldMemo::new();
        unchecked.into_iter().find(|&i| {
            let r = &rec.reads[i];
            if r.kind.field().is_some_and(|f| eng.field_epoch(f) == r.epoch) {
                return false;
            }
            read::answer_in(eng, node, &r.kind, r.path, &mut memo) != r.seen
        })
    });
    if let Some(i) = refuted {
        let cause = match &rec.reads[i].kind {
            ReadKind::Field { f, .. } => Cause::Changed { field: *f, site: None },
            ReadKind::Env { f } => Cause::Environment { field: *f },
            ReadKind::Fold { f, .. } | ReadKind::Next { f, .. } => Cause::Range { field: *f, by: node.id },
            ReadKind::Placed { .. } => Cause::Output { of: node.id },
            ReadKind::Following { .. } => Cause::Structure,
        };
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
                match here {
                    Some((_, d)) => {
                        if through(eng, Reaching::of(d), r.path) != r.seen {
                            wake.push((r.node, by, f));
                        }
                    }
                    None => later.push((f, r.clone(), by)),
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

/// Re-asks the parent's read of a placement's output.
fn reask_parent(eng: &Engine, w: Option<&Worker>, parent: &Arc<Node>, child: NodeId, st: &mut CommitStats) {
    let Some(r) = parent.run_record() else {
        return;
    };
    let Some(i) = r.placed.iter().position(|&c| c == child) else {
        return;
    };
    let out = eng.node(child).run_record().map(|x| x.output);
    for rd in &r.reads {
        if let ReadKind::Placed { i: j } = rd.kind
            && j == i
            && read::apply_path(eng, out, rd.path) != rd.seen
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
/// of `f`. Each reader whose answer through its path differs from what it
/// saw is marked dirty (`sched::mark_dirty`). A relation met as the next
/// definition is recomputed in place (one short write lock) and, if its
/// value changed, the walk goes on past it: the cascade of a counter chain.
pub(crate) fn reask(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    from: Pos,
    seg: SegId,
    by: NodeId,
    st: &mut CommitStats,
) {
    reask_from(eng, w, f, from, seg, Some(by), Some(eng.node(by).op()), st);
}

/// `reask` from a label with no defining node (the environment: `by`
/// None), or from a node's definition. With `stop_op`, the walk also stops
/// at a node of that operation that has not committed (`first_hole`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn reask_from(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    from: Pos,
    seg: SegId,
    by: Option<NodeId>,
    stop_op: Option<OpId>,
    st: &mut CommitStats,
) {
    let (mut cur, mut from, mut by) = (seg, from, by);
    loop {
        let s = eng.seg(cur);
        let (readers, next, hole) = {
            let inner = s.inner.read();
            if let Some(b) = by {
                let byn = eng.node(b);
                if byn.seg_hint() == cur && inner.nodes.get(&byn.pos_hint()) == Some(&b) {
                    from = byn.pos_hint();
                }
            }
            let next = Segment::next_def(&inner, f, from).map(|(p, d)| (p, *d));
            let hole = stop_op.and_then(|op| first_hole(eng, &inner, op, from, next.map(|x| x.0)));
            let stop = hole.map(|h| h.0).or(next.map(|x| x.0));
            let readers: Vec<Reader> = match inner.readers.get(&f) {
                Some(rs) => rs
                    .iter()
                    .filter(|r| {
                        let ap = eng.node(r.anchor).pos_hint();
                        (ap > from || (ap == from && r.inclusive))
                            && stop.is_none_or(|s| ap < s || (ap == s && !r.inclusive))
                    })
                    .cloned()
                    .collect(),
                None => Vec::new(),
            };
            (readers, next, hole)
        };
        for r in &readers {
            reask_reader(eng, w, f, r, by, st);
        }
        if let Some((_, h)) = hole {
            stop_at(eng, w, h, f, st);
            return;
        }
        match next {
            Some((_, d)) => {
                let Some(c) = d.rel else {
                    return;
                };
                if !settle_rel(eng, f, d.by, c) {
                    return;
                }
                st.relative_resolved += 1;
                by = Some(d.by);
            }
            None => {
                let Some(next) = next_with(eng, f, s.lo) else {
                    return;
                };
                cur = next;
                from = Pos(eng.seg(cur).lo.0.saturating_sub(1));
            }
        }
    }
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
/// segment (starting at `lo`), on from the next segment.
fn reask_later(eng: &Engine, w: Option<&Worker>, f: Field, lo: Pos, by: NodeId, st: &mut CommitStats) {
    if let Some(cur) = next_with(eng, f, lo) {
        let op = eng.node(by).op();
        reask_from(
            eng,
            w,
            f,
            Pos(eng.seg(cur).lo.0.saturating_sub(1)),
            cur,
            Some(by),
            Some(op),
            st,
        );
    }
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

/// Files `f` with the node a walk stopped at, after the walk's lock. A
/// node that has committed (or gone) meanwhile has taken its list already:
/// the walk goes on from it here.
fn stop_at(eng: &Engine, w: Option<&Worker>, h: NodeId, f: Field, st: &mut CommitStats) {
    let n = eng.node(h);
    {
        let mut s = n.stopped.lock();
        if !s.contains(&f) {
            s.push(f);
        }
    }
    if !n.is_hole() {
        walk_stopped(eng, w, n, n.pos_hint(), n.seg_hint(), &[], st);
    }
}

/// The walks that stopped at `node` (`first_hole`), on from it, but for
/// the fields it defines (its own walks).
fn walk_stopped(
    eng: &Engine,
    w: Option<&Worker>,
    node: &Node,
    pos: Pos,
    seg: SegId,
    defined: &[Field],
    st: &mut CommitStats,
) {
    let fs = std::mem::take(&mut *node.stopped.lock());
    for f in fs {
        if !defined.contains(&f) {
            reask_from(eng, w, f, pos, seg, Some(node.id), None, st);
        }
    }
}

fn reask_reader(eng: &Engine, w: Option<&Worker>, f: Field, r: &Reader, by: Option<NodeId>, st: &mut CommitStats) {
    if !live(eng, r.node, r.serial) {
        return;
    }
    let cause = match by {
        Some(b) => Cause::Changed { field: f, site: Some(b) },
        None => Cause::Environment { field: f },
    };
    if reader_answer(eng, f, r) != r.seen
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

/// Re-asks the readers of folds over `f` whose range covers `at`, `by`'s
/// label (its last where it is gone).
pub(crate) fn reask_ranges(eng: &Engine, w: Option<&Worker>, f: Field, at: Pos, by: NodeId, st: &mut CommitStats) {
    let at = read::pos_of(eng, by).unwrap_or(at);
    let readers = eng.ranges.holding(f, at);
    let mut memo = read::FoldMemo::new();
    for (k, r) in readers {
        if covers(eng, &r, at) {
            reask_range(eng, w, f, k, &r, by, st, &mut memo);
        }
    }
}

/// Re-asks one range reader after `by`'s change.
#[allow(clippy::too_many_arguments)]
fn reask_range(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    k: RangeKey,
    r: &RangeReader,
    by: NodeId,
    st: &mut CommitStats,
    memo: &mut read::FoldMemo,
) {
    if !live(eng, r.node, r.serial) {
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
        let from = match r.from {
            Some(n) => read::pos_of(eng, n),
            None => Some(read::unit_start(eng, me)),
        };
        if let Some(from) = from {
            let n = read::next_contribution(eng, f, Some(from));
            let v = read::next_value(eng, &n);
            if r.seen.iter().all(|&(p, s)| read::apply_path(eng, v, p) == s) {
                if n.stop != r.to && eng.ranges.remove(f, k) {
                    let mut again = r.clone();
                    again.to = n.stop;
                    again.hole = n.item.is_none() && n.stop.is_some();
                    file_ranges(eng, w, vec![(f, again)], by, st);
                }
                return;
            }
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

/// Re-asks the `next` readers whose search stopped at `node`: it has
/// committed, been taken or removed, so the first contribution after their
/// start may now be known.
pub(crate) fn reask_holes(eng: &Engine, w: Option<&Worker>, node: &Node, st: &mut CommitStats) {
    let seg = eng.seg(node.seg_hint());
    let readers: Vec<(Field, RangeKey, RangeReader)> = {
        let mut inner = seg.inner.write();
        if inner.stops.is_empty() {
            return;
        }
        let (mine, rest) = std::mem::take(&mut inner.stops).into_iter().partition(|s| s.2.to == Some(node.id));
        inner.stops = rest;
        mine
    };
    let mut memo = read::FoldMemo::new();
    for (f, k, r) in readers {
        reask_range(eng, w, f, k, &r, node.id, st, &mut memo);
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
                        value: nv,
                        by: n,
                        rel: Some(c),
                        provisional: true,
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
    defs: Vec<DefRec>,
    shapes: Vec<(Field, i64)>,
}

pub(crate) fn predict(eng: &Engine, op: OpId, args: &[ValueId]) -> Predicted {
    let mut defs: Vec<DefRec> = Vec::new();
    if let Some(pd) = eng.op(op).predicted_defs(args, eng.values()) {
        for (f, x) in pd {
            defs.push(DefRec {
                f,
                loc: Locality::Local,
                value: x,
                rel: None,
            });
        }
    }
    let shapes = if eng.shapes_n.load(Ordering::Acquire) == 0 {
        Vec::new()
    } else {
        eng.shapes
            .read()
            .iter()
            .filter(|((o, f), _)| *o == op && !defs.iter().any(|d| d.f == *f))
            .map(|((_, f), c)| (*f, *c))
            .collect()
    };
    Predicted { defs, shapes }
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
    (Arc::new(Node::new(id, unit, op, args, parent, SegId(0), Pos(0))), pred)
}

/// Files a new node's predictions at its label: the relations from the
/// input in this segment or the environment (the commit that places it
/// puts the value right from the true input, `settle_predictions`).
pub(crate) fn file_predicted(eng: &Engine, seg: &Segment, inner: &mut Inner, node: &Arc<Node>, pred: Predicted) {
    let v = eng.values();
    let pos = node.pos_hint();
    let Predicted { mut defs, shapes } = pred;
    for (f, c) in shapes {
        let input = Segment::reaching(inner, f, pos, false)
            .map(|(_, d)| d.value)
            .or_else(|| read::env_value(eng, f));
        let val = v.int(input.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c));
        defs.push(DefRec {
            f,
            loc: Locality::Local,
            value: val,
            rel: Some(c),
        });
    }
    if defs.is_empty() {
        return;
    }
    for d in &defs {
        let m = inner.defs.entry(d.f).or_default();
        let first = m.is_empty();
        m.insert(
            pos,
            Def {
                value: d.value,
                by: node.id,
                rel: d.rel,
                provisional: true,
            },
        );
        if first {
            eng.defs.add(d.f, seg.lo, seg.id);
        }
        eng.bump(d.f);
    }
    *node.last.lock() = Some(Arc::new(RunRecord::provisional(defs)));
}

/// Takes a node's definitions and contributions out of its segment (under
/// its write lock) and re-asks their readers; its placements are unfiled.
fn retract_products(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>, remove: bool) {
    let rec = node.record();
    let Some((seg, mut inner)) = own_write(eng, node) else {
        return;
    };
    let pos = node.pos_hint();
    let seg_id = seg.id;
    let was_hole = node.is_hole();
    if remove {
        node.set_state(State::Gone);
        inner.nodes.remove(&pos);
        if was_hole {
            inner.holes = inner.holes.saturating_sub(1);
        }
        if let Some(v) = inner.by_op.get_mut(&node.op()) {
            v.retain(|&n| n != node.id);
        }
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
    *node.last.lock() = None;
    for &f in changed_defs.iter().chain(&changed_contribs) {
        eng.bump(f);
    }
    drop(inner);
    ix.apply(eng, seg);
    let mut st = CommitStats::default();
    for f in changed_defs {
        reask(eng, w, f, pos, seg_id, node.id, &mut st);
    }
    for f in changed_contribs {
        reask_ranges(eng, w, f, pos, node.id, &mut st);
    }
    reask_holes(eng, w, node, &mut st);
    if was_hole && remove {
        walk_stopped(eng, w, node, pos, seg_id, &[], &mut st);
    }
    if let Some(r) = &rec {
        for &c in &r.placed {
            unfile(eng, w, eng.node(c));
        }
        if r.claim > 0 {
            release_claimed(eng, w, node);
        }
    }
    eng.note_stats(w, &st);
}

/// Removes a node: its definitions, contributions, reads, by-op entry and,
/// recursively, its placements; readers of what it defined are re-asked.
/// Its state becomes `Gone`.
pub(crate) fn unfile(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>) {
    retract_products(eng, w, node, true);
}

/// A sibling taken by a claim after it ran, or before with predictions
/// filed: its products are withdrawn, the node stays, inert.
fn retract(eng: &Engine, w: Option<&Worker>, node: &Arc<Node>) {
    retract_products(eng, w, node, false);
}

/// The claimer's inert siblings made dirty again (its claim is gone).
fn release_claimed(eng: &Engine, w: Option<&Worker>, me: &Node) {
    apply_claim(eng, w, me, 0);
}

/// Makes the next `want` pushed siblings of `me` inert, and dirty again
/// those beyond that an earlier claim took.
pub(crate) fn apply_claim(eng: &Engine, w: Option<&Worker>, me: &Node, want: usize) {
    let mut cur = Some(me.seg_hint());
    let mut lower = Some(me.pos_hint());
    let mut k = 0usize;
    let mut retracts: Vec<Arc<Node>> = Vec::new();
    let mut releases: Vec<Arc<Node>> = Vec::new();
    let mut taken_holes: Vec<Arc<Node>> = Vec::new();
    'segs: while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != me.unit {
            break;
        }
        {
            let mut inner = seg.inner.write();
            let before = taken_holes.len();
            let lo = lower.map_or(Unbounded, Excluded);
            for (_, id) in inner.nodes.range((lo, Unbounded)) {
                let s = eng.node(*id);
                if s.parent.is_some() || s.state() == State::Gone {
                    continue;
                }
                if k < want {
                    let hole = s.is_hole();
                    if s.take() {
                        // (one that has not run yet: what it was predicted
                        // to define is withdrawn as a run's would be)
                        if s.committed() || s.record().is_some() {
                            retracts.push(s.clone());
                        }
                        if !s.committed() && hole {
                            taken_holes.push(s.clone());
                        }
                    }
                } else if s.taken() {
                    releases.push(s.clone());
                } else {
                    inner.holes = inner.holes.saturating_sub(taken_holes.len() - before);
                    break 'segs;
                }
                k += 1;
            }
            inner.holes = inner.holes.saturating_sub(taken_holes.len() - before);
        }
        lower = None;
        cur = eng.order.next(sid, seg.lo);
    }
    for s in retracts {
        retract(eng, w, &s);
    }
    let mut st = CommitStats::default();
    // (a taken node never contributes: the `next`s that stopped at it
    // search on past it)
    for s in taken_holes {
        walk_stopped(eng, w, &s, s.pos_hint(), s.seg_hint(), &[], &mut st);
        reask_holes(eng, w, &s, &mut st);
    }
    eng.note_stats(w, &st);
    for s in releases {
        if s.release() {
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
    node
}

/// `place` after node `at` and its placements, under their segment's lock.
pub(crate) fn place_after(eng: &Engine, at: NodeId, node: Arc<Node>, pred: Predicted) -> Arc<Node> {
    let end = subtree_end(eng, at);
    let endn = eng.node(end);
    loop {
        if let Some((seg, mut inner)) = own_write(eng, endn) {
            if Segment::needs_split(&inner) {
                let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
                seg.split(&mut inner, eng, sid);
                continue;
            }
            return place(eng, seg, &mut inner, end, node, pred);
        }
        // the node is gone: after its parent, or at the unit's end
        return match endn.parent.or(node.parent) {
            Some(p) if p != at => place_after(eng, p, node, pred),
            _ => eng.push_node(node, pred),
        };
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
        let nd = eng.node(n);
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
    let input = reaching_at(eng, f, at, false).and_then(|r| r.value);
    v.int(input.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c))
}
