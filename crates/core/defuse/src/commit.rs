//! A run's commit: into the node's own segment, then the walk forward.
//!
//! 1. Under the node's segment's write lock: diff the product's definitions
//!    and contributions against the node's last record, file the changes,
//!    match placements by (op, args) to keep unchanged children, place new
//!    ones in the gap (relabel or split if closed), record the reads
//!    anchored here, bump the serial, swap the record. Release.
//! 2. Reads anchored in other segments: each under that segment's lock, one
//!    at a time, never nested.
//! 3. For each changed (field, position): the walk forward (`reask`). For
//!    each changed contribution: its range readers.
//! 4. Removed placements: `unfile`, recursively, each under its own segment.
//! 5. A relation defined for the first time by this operation: `learn_shape`.
//! 6. The node's own reads, answered again: a walk that passed before they
//!    were filed is caught here, and the node runs again.
//!
//! No other segment is written in step 1. No lock is held in step 3 except
//! one segment's read lock at a time, and a short write lock to recompute a
//! relation met on the way.

use std::collections::VecDeque;
use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use std::sync::RwLockWriteGuard;
use std::sync::atomic::Ordering;
use std::time::Instant;

use crate::api::{Cause, Locality, NodeId, OpId, UnitId};
use crate::engine::Engine;
use crate::lock::ns;
use crate::node::{Anchor, DefRec, Node, Placement, Product, ReadKind, RunRecord, State, Wake};
use crate::order::{Pos, SegId};
use crate::read::{self, answer, next_contribution, range_answer, reader_answer, reaching_at};
use crate::sched::{Worker, enqueue, mark_dirty, mark_stale};
use crate::segment::{Def, Inner, RangeReader, Reader, Segment, live, push_pruned};
use crate::values::{Field, FxHashMap, FxHashSet, ValueId};

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
pub(crate) fn own_write<'a>(eng: &'a Engine, node: &Node) -> Option<(&'a Arc<Segment>, RwLockWriteGuard<'a, Inner>)> {
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
/// or by its own commit's check of its reads.
#[derive(Default)]
struct IndexOps {
    defs_add: Vec<Field>,
    defs_remove: Vec<Field>,
    contribs_add: Vec<Field>,
    contribs_remove: Vec<Field>,
    readers_add: Vec<Field>,
}

impl IndexOps {
    fn apply(self, eng: &Engine, seg: &Segment) {
        for f in self.defs_add {
            eng.defs.add(f, seg.lo, seg.id);
        }
        for f in self.contribs_add {
            eng.contribs.add(f, seg.lo, seg.id);
        }
        for f in self.readers_add {
            eng.readers.add(f, seg.lo, seg.id);
        }
        for f in self.defs_remove {
            eng.defs.remove(f, seg.lo, seg.id);
        }
        for f in self.contribs_remove {
            eng.contribs.remove(f, seg.lo, seg.id);
        }
    }
}

/// Files a range reader in every segment its range covers.
fn file_range_reader(eng: &Engine, me: &Node, f: Field, r: RangeReader) {
    let from = match r.from {
        Some(n) => eng.node(n).pos_hint(),
        None => read::unit_start(eng, me),
    };
    let to = match r.to {
        Some(n) => Some(eng.node(n).pos_hint()),
        None if r.next => None,
        None => Some(me.pos_hint()),
    };
    let mut cur = eng.seg_for_pos(from).map(|s| s.id).or_else(|| eng.order.first());
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if to.is_some_and(|t| seg.lo > t) {
            break;
        }
        {
            let mut inner = seg.inner.write();
            let v = inner.range_readers.entry(f).or_default();
            push_pruned(v, r.clone(), |x| live(eng, x.node, x.serial));
        }
        cur = eng.order.next(sid, seg.lo);
    }
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
            if r.next { at <= t } else { at < t }
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
    // the record, made before the lock: no allocation under it
    let mut rec = Arc::new(RunRecord {
        serial,
        output: p.output,
        defs,
        contribs,
        reads: std::mem::take(&mut p.reads),
        placed: Vec::with_capacity(p.placements.len()),
        claim: p.claim,
        effect: p.effect,
        provisional: false,
    });
    let mut spare_readers: Vec<Reader> = Vec::with_capacity(4);
    let old_fields: Vec<Field> = last.as_ref().map(|r| r.defs.iter().map(|d| d.f).collect()).unwrap_or_default();
    let old_contribs: Vec<Field> = last.as_ref().map(|r| r.contribs.iter().map(|c| c.0).collect()).unwrap_or_default();
    let mut changed_defs: Vec<Field> = Vec::new();
    let mut changed_contribs: Vec<Field> = Vec::new();
    let mut rels: Vec<(Field, i64)> = Vec::new();
    let mut placed: Vec<NodeId> = Vec::with_capacity(p.placements.len());
    let mut made: Vec<Arc<Node>> = Vec::new();
    let mut deferred: Vec<(usize, NodeId, OpId, Vec<ValueId>)> = Vec::new();
    let mut here_reads: Vec<(Field, Reader)> = Vec::new();
    let mut other_reads: Vec<(Field, Reader, Anchor)> = Vec::new();
    let mut range_reads: Vec<(Field, RangeReader)> = Vec::new();
    let mut env_reads: Vec<(Field, crate::engine::EnvReader)> = Vec::new();
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
            ReadKind::Fold { f, from, to } => range_reads.push((
                *f,
                RangeReader {
                    node: node.id,
                    serial,
                    from: *from,
                    to: *to,
                    next: false,
                    path: r.path,
                    seen: r.seen,
                },
            )),
            ReadKind::Next { f, after } => {
                let from = match after {
                    Some(n) => Some(eng.node(*n).pos_hint()),
                    None => Some(read::unit_start(eng, node)),
                };
                let stop = next_contribution(eng, *f, from).stop;
                range_reads.push((
                    *f,
                    RangeReader {
                        node: node.id,
                        serial,
                        from: *after,
                        to: stop,
                        next: true,
                        path: r.path,
                        seen: r.seen,
                    },
                ));
            }
            ReadKind::Placed { .. } | ReadKind::Following { .. } => {}
        }
    }
    let (seg_id, pos, old_output, rec) = loop {
        let Some((seg, mut inner)) = own_write(eng, node) else {
            return st;
        };
        if node.state() == State::Inert {
            return st;
        }
        if Segment::needs_split(&inner) {
            let id = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
            seg.split(&mut inner, eng, id);
            continue;
        }
        let t_lock = Instant::now();
        let pos = pos_in(&inner, node);
        let mut ix = IndexOps::default();
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
        }
        // placements
        let mut after = node.id;
        let mut in_seg = true;
        let mut removed_kept: Vec<NodeId> = Vec::new();
        for (i, what) in p.placements.iter().enumerate() {
            let after_pos = eng.node(after).pos_hint();
            if let Some(k) = kept[i]
                && !matches!(what, Placement::FollowAfter(..))
            {
                let kn = eng.node(k);
                if kn.state() != State::Gone && kn.pos_hint() > after_pos {
                    placed.push(k);
                    after = subtree_end(eng, k);
                    continue;
                }
                removed_kept.push(k);
            }
            let (op, args) = what.key();
            let id = NodeId::from_raw(eng.nodes.reserve() as u64);
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
                let n = place(eng, seg, &mut inner, node.unit, Some(node.id), after, op, args.to_vec(), id);
                made.push(n);
            } else {
                in_seg = false;
                deferred.push((i, at.unwrap_or(after), op, args.to_vec()));
            }
            after = id;
        }
        let mut removed: Vec<NodeId> = old_placed.iter().copied().filter(|n| !placed.contains(n)).collect();
        removed.extend(removed_kept);
        // reads anchored here
        let mut later: Vec<(Field, Reader, Anchor)> = Vec::new();
        for (f, r) in here_reads.drain(..) {
            let a = eng.node(r.anchor);
            if r.anchor == node.id || (a.seg_hint() == seg.id && inner.nodes.get(&a.pos_hint()) == Some(&r.anchor)) {
                if file_reader(eng, &mut inner, f, r, &mut spare_readers) {
                    ix.readers_add.push(f);
                }
            } else {
                let at = Anchor::Before(r.anchor);
                later.push((f, r, at));
            }
        }
        other_reads.extend(later);
        Arc::get_mut(&mut rec).expect("the record is not shared yet").placed = placed;
        *node.last.lock() = Some(rec.clone());
        node.serial.store(serial, Ordering::Release);
        let old_output = last.as_ref().filter(|r| !r.provisional).map(|r| r.output);
        st.commit_ns = ns(t_lock);
        drop(inner);
        ix.apply(eng, seg);
        for id in removed {
            unfile(eng, ww, eng.node(id));
        }
        break (seg.id, pos, old_output, rec);
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
    for (f, r) in range_reads {
        file_range_reader(eng, node, f, r);
    }
    if !env_reads.is_empty() {
        let mut er = eng.env.readers.write();
        for (f, r) in env_reads {
            er.entry(f).or_default().push(r);
        }
    }
    // deferred placements: after a node elsewhere
    let mut by_index: Vec<(usize, Arc<Node>)> = Vec::new();
    for (i, at, op, args) in deferred {
        let n = place_after(eng, node.unit, Some(node.id), at, op, args, rec.placed[i]);
        by_index.push((i, n));
    }
    made.extend(by_index.into_iter().map(|x| x.1));
    if p.claim > 0 || last.as_ref().is_some_and(|r| r.claim > 0) {
        apply_claim(eng, ww, node, p.claim);
    }
    // 3. the walks
    changed_defs.sort_unstable();
    changed_defs.dedup();
    for &f in &changed_defs {
        reask(eng, ww, f, pos, seg_id, node.id, &mut st);
    }
    for &f in &changed_contribs {
        reask_ranges(eng, ww, f, pos, node.id, &mut st);
    }
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
    if serial == 1 {
        reask_holes(eng, ww, node, &mut st);
    }
    if old_output != Some(p.output)
        && let Some(parent) = node.parent
    {
        reask_parent(eng, ww, eng.node(parent), node.id, &mut st);
    }
    // 5. a first relation by this operation
    for (f, c) in rels {
        learn_shape(eng, ww, node.op(), f, c, &mut st);
    }
    // 6. the node's own reads, answered again
    for r in &rec.reads {
        if answer(eng, node, &r.kind, r.path) != r.seen {
            let cause = match &r.kind {
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
            break;
        }
    }
    st.reask_ns = ns(t0).saturating_sub(st.commit_ns);
    st
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
            let nv = rel_value_at(eng, d.f, n.id, c);
            if nv != d.value
                && let Some((_, mut inner)) = own_write(eng, n)
            {
                let p = n.pos_hint();
                if let Some(e) = inner.defs.get_mut(&d.f).and_then(|m| m.get_mut(&p))
                    && e.provisional
                    && e.by == n.id
                {
                    e.value = nv;
                }
            }
        }
        reask(eng, w, d.f, n.pos_hint(), n.seg_hint(), n.id, st);
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
pub(crate) fn reask(eng: &Engine, w: Option<&Worker>, f: Field, from: Pos, seg: SegId, by: NodeId, st: &mut CommitStats) {
    reask_from(eng, w, f, from, seg, Some(by), st);
}

/// `reask` from a label with no defining node (the environment: `by`
/// None), or from a node's definition.
pub(crate) fn reask_from(
    eng: &Engine,
    w: Option<&Worker>,
    f: Field,
    from: Pos,
    seg: SegId,
    by: Option<NodeId>,
    st: &mut CommitStats,
) {
    let v = eng.values();
    let (mut cur, mut from, mut by) = (seg, from, by);
    loop {
        let s = eng.seg(cur);
        let (readers, next) = {
            let inner = s.inner.read();
            if let Some(b) = by {
                let byn = eng.node(b);
                if byn.seg_hint() == cur && inner.nodes.get(&byn.pos_hint()) == Some(&b) {
                    from = byn.pos_hint();
                }
            }
            let next = Segment::next_def(&inner, f, from).map(|(p, d)| (p, *d));
            let stop = next.map(|x| x.0);
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
            (readers, next)
        };
        for r in &readers {
            reask_reader(eng, w, f, r, by, st);
        }
        match next {
            Some((_, d)) => {
                let Some(c) = d.rel else {
                    return;
                };
                let input = reaching_at(eng, f, d.by, false).and_then(|r| r.value);
                let nv = v.int(input.and_then(|x| v.as_int(x)).unwrap_or(0).wrapping_add(c));
                if nv == d.value {
                    return;
                }
                {
                    let mut inner = s.inner.write();
                    let p = eng.node(d.by).pos_hint();
                    if let Some(e) = inner.defs.get_mut(&f).and_then(|m| m.get_mut(&p))
                        && e.by == d.by
                    {
                        e.value = nv;
                    }
                }
                st.relative_resolved += 1;
                by = Some(d.by);
            }
            None => {
                let after = Pos(s.lo.0 + 1);
                let nd = eng.defs.first_at_or_after(f, after);
                let nr = eng.readers.first_at_or_after(f, after);
                cur = match (nd, nr) {
                    (None, None) => return,
                    (Some(a), None) => a.1,
                    (None, Some(b)) => b.1,
                    (Some(a), Some(b)) => {
                        if a.0 <= b.0 {
                            a.1
                        } else {
                            b.1
                        }
                    }
                };
                from = Pos(eng.seg(cur).lo.0.saturating_sub(1));
            }
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

/// Re-asks the readers of folds over `f` whose range covers `at`.
pub(crate) fn reask_ranges(eng: &Engine, w: Option<&Worker>, f: Field, at: Pos, by: NodeId, st: &mut CommitStats) {
    let Some(seg) = eng.seg_for_pos(at) else {
        return;
    };
    let readers: Vec<RangeReader> = {
        let inner = seg.inner.read();
        let byn = eng.node(by);
        let at = if inner.nodes.get(&byn.pos_hint()) == Some(&by) { byn.pos_hint() } else { at };
        match inner.range_readers.get(&f) {
            Some(rs) => rs.iter().filter(|r| covers(eng, r, at)).cloned().collect(),
            None => Vec::new(),
        }
    };
    for r in readers {
        if !live(eng, r.node, r.serial) {
            continue;
        }
        if range_answer(eng, f, &r) != r.seen
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
}

/// Re-asks the `next` readers whose search stopped at `node`: it has
/// committed, been taken or removed, so the first contribution after their
/// start may now be known.
pub(crate) fn reask_holes(eng: &Engine, w: Option<&Worker>, node: &Node, st: &mut CommitStats) {
    let seg = eng.seg(node.seg_hint());
    let readers: Vec<(Field, RangeReader)> = {
        let inner = seg.inner.read();
        inner
            .range_readers
            .iter()
            .flat_map(|(f, rs)| rs.iter().filter(|r| r.next && r.to == Some(node.id)).map(move |r| (*f, r.clone())))
            .collect()
    };
    for (f, r) in readers {
        if !live(eng, r.node, r.serial) {
            continue;
        }
        if range_answer(eng, f, &r) != r.seen
            && mark_stale(
                eng,
                w,
                eng.node(r.node),
                Wake {
                    by: Some(node.id),
                    cause: Cause::Range { field: f, by: node.id },
                    at_ns: eng.now_ns(),
                },
            )
        {
            st.woken += 1;
        }
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

/// Files a new node's provisional definitions: its operation's
/// `predicted_defs`, and the relations its operation is known to make
/// (from the input in this segment or the environment; the commit that
/// places it puts the value right from the true input, `settle_predictions`).
pub(crate) fn file_predicted(eng: &Engine, seg: &Segment, inner: &mut Inner, node: &Arc<Node>) {
    let v = eng.values();
    let op = eng.op(node.op());
    let args = node.args();
    let pos = node.pos_hint();
    let mut defs: Vec<DefRec> = Vec::new();
    if let Some(pd) = op.predicted_defs(&args, v) {
        for (f, x) in pd {
            defs.push(DefRec {
                f,
                loc: Locality::Local,
                value: x,
                rel: None,
            });
        }
    }
    let shapes: Vec<(Field, i64)> = eng
        .shapes
        .read()
        .iter()
        .filter(|((o, _), _)| *o == node.op())
        .map(|((_, f), c)| (*f, *c))
        .collect();
    for (f, c) in shapes {
        if defs.iter().any(|d| d.f == f) {
            continue;
        }
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
    if remove {
        node.set_state(State::Gone);
        inner.nodes.remove(&pos);
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

/// A sibling taken by a claim after it ran: its products are withdrawn,
/// the node stays, inert.
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
    'segs: while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != me.unit {
            break;
        }
        {
            let inner = seg.inner.write();
            let lo = lower.map_or(Unbounded, Excluded);
            for (_, id) in inner.nodes.range((lo, Unbounded)) {
                let s = eng.node(*id);
                if s.parent.is_some() || s.state() == State::Gone {
                    continue;
                }
                if k < want {
                    if s.state() != State::Inert {
                        s.set_state(State::Inert);
                        if s.committed() {
                            retracts.push(s.clone());
                        }
                    }
                } else if s.state() == State::Inert {
                    releases.push(s.clone());
                } else {
                    break 'segs;
                }
                k += 1;
            }
        }
        lower = None;
        cur = eng.order.next(sid, seg.lo);
    }
    for s in retracts {
        retract(eng, w, &s);
    }
    for s in releases {
        if s.cas_state(State::Inert, State::Dirty) {
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

/// Makes a node for a placement after `after` (a label in the gap, the
/// segment relabelled if the gap is closed), registers it in the
/// segment and files its predictions. Under the segment's write lock,
/// which the caller holds; `after` is in the segment.
pub(crate) fn place(
    eng: &Engine,
    seg: &Segment,
    inner: &mut Inner,
    unit: UnitId,
    parent: Option<NodeId>,
    after: NodeId,
    op: OpId,
    args: Vec<ValueId>,
    id: NodeId,
) -> Arc<Node> {
    let p = match Segment::gap_after(inner, eng.node(after).pos_hint()) {
        Some(p) => p,
        None => {
            seg.relabel(inner, eng);
            Segment::gap_after(inner, eng.node(after).pos_hint()).expect("a gap after a relabel")
        }
    };
    let node = Arc::new(Node::new(id, unit, op, args, parent, seg.id, p));
    eng.nodes.set(usize::try_from(id.raw()).unwrap_or(usize::MAX), node.clone());
    inner.nodes.insert(p, id);
    inner.by_op.entry(op).or_default().push(id);
    file_predicted(eng, seg, inner, &node);
    node
}

/// `place` after node `at` and its placements, under their segment's lock.
pub(crate) fn place_after(
    eng: &Engine,
    unit: UnitId,
    parent: Option<NodeId>,
    at: NodeId,
    op: OpId,
    args: Vec<ValueId>,
    id: NodeId,
) -> Arc<Node> {
    let end = subtree_end(eng, at);
    let endn = eng.node(end);
    loop {
        if let Some((seg, mut inner)) = own_write(eng, endn) {
            if Segment::needs_split(&inner) {
                let sid = SegId(u32::try_from(eng.segs.reserve()).expect("segment ids"));
                seg.split(&mut inner, eng, sid);
                continue;
            }
            return place(eng, seg, &mut inner, unit, parent, end, op, args, id);
        }
        // the node is gone: after its parent, or at the unit's end
        return match endn.parent.or(parent) {
            Some(p) if p != at => place_after(eng, unit, parent, p, op, args, id),
            _ => eng.push_node(unit, parent, op, args, id),
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
