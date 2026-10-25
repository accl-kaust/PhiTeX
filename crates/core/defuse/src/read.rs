//! Answering reads: the reaching definition, a fold, a path.

use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;

use crate::api::{NodeId, PathId, ProjId, Step};
use crate::engine::Engine;
use crate::node::{Anchor, Node, ReadKind, State};
use crate::order::{Pos, SegId};
use crate::segment::{RangeReader, Reader, Segment, reduce};
use crate::values::{Field, Value, ValueId};

/// A path that is no path: the defining node's id as an integer
/// (`Ctx::def_site`).
pub(crate) const SITE: PathId = PathId(u32::MAX);

/// What reaches a position.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Reaching {
    /// None: no definition and no environment value.
    pub(crate) value: Option<ValueId>,
    /// The defining node; None for the environment.
    pub(crate) by: Option<NodeId>,
    pub(crate) provisional: bool,
}

impl Reaching {
    pub(crate) fn of(d: &crate::segment::Def) -> Reaching {
        Reaching {
            value: Some(d.value),
            by: Some(d.by),
            provisional: d.provisional,
        }
    }
}

/// The first contribution after a position, or where the search stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Next {
    pub(crate) item: Option<(NodeId, ValueId)>,
    /// The contribution's node, or the uncommitted node the search stopped
    /// at; None at the end of the line.
    pub(crate) stop: Option<NodeId>,
}

fn env_reaching(eng: &Engine, f: Field) -> Reaching {
    Reaching {
        value: env_value(eng, f),
        by: None,
        provisional: false,
    }
}

/// The environment's value of `f`: the client's settings, then its
/// provider once, cached.
pub(crate) fn env_value(eng: &Engine, f: Field) -> Option<ValueId> {
    if let Some(x) = eng.env.values.read().get(&f) {
        return *x;
    }
    let x = eng.env.provider.read().as_ref().and_then(|p| p(eng.values(), f)).flatten();
    eng.env.values.write().entry(f).or_insert(x);
    x
}

/// The last definition of `f` in the last indexed segment before the one
/// starting at `lo`, else the environment.
fn before_segment(eng: &Engine, f: Field, lo: Pos) -> Reaching {
    let mut lo = lo;
    while let Some(s) = eng.defs.last_before(f, lo) {
        let seg = eng.seg(s);
        {
            let inner = seg.inner.read();
            if let Some((_, d)) = inner.defs.get(&f).and_then(|m| m.iter().next_back()) {
                return Reaching {
                    value: Some(d.value),
                    by: Some(d.by),
                    provisional: d.provisional,
                };
            }
        }
        lo = seg.lo;
    }
    env_reaching(eng, f)
}

/// The definition of `f` reaching `at` (at `at` too when `inclusive`):
/// `at`'s segment first, then the last earlier segment the field index
/// names, then the environment. Two short read locks at most.
pub(crate) fn reaching(eng: &Engine, f: Field, at: Pos, inclusive: bool) -> Reaching {
    let Some(seg) = eng.seg_for_pos(at) else {
        return env_reaching(eng, f);
    };
    {
        let inner = seg.inner.read();
        if let Some((_, d)) = Segment::reaching(&inner, f, at, inclusive) {
            return Reaching {
                value: Some(d.value),
                by: Some(d.by),
                provisional: d.provisional,
            };
        }
    }
    before_segment(eng, f, seg.lo)
}

/// `reaching` at a node's label, read under its segment's lock; None when
/// the node is gone.
pub(crate) fn reaching_at(eng: &Engine, f: Field, anchor: NodeId, inclusive: bool) -> Option<Reaching> {
    let n = eng.node(anchor);
    loop {
        if n.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(n.seg_hint());
        let lo = {
            let inner = seg.inner.read();
            let p = n.pos_hint();
            if inner.nodes.get(&p) != Some(&anchor) {
                if n.state() == State::Gone {
                    return None;
                }
                continue;
            }
            if let Some((_, d)) = Segment::reaching(&inner, f, p, inclusive) {
                return Some(Reaching {
                    value: Some(d.value),
                    by: Some(d.by),
                    provisional: d.provisional,
                });
            }
            seg.lo
        };
        return Some(before_segment(eng, f, lo));
    }
}

/// The contributions of a segment in `(from, to)`, reduced; the cached
/// sum when the range covers the whole segment.
fn segment_part(eng: &Engine, mono: &dyn crate::api::Monoid, seg: &Segment, f: Field, from: Option<Pos>, to: Option<Pos>) -> ValueId {
    let whole = {
        let inner = seg.inner.read();
        let covers = from.is_none_or(|p| p < seg.lo) && to.is_none_or(|p| p >= inner.hi);
        if covers {
            match inner.sums.get(&f) {
                Some(&x) => return x,
                None => true,
            }
        } else {
            match inner.contribs.get(&f) {
                Some(m) => {
                    let lo = from.map_or(Unbounded, Excluded);
                    let hi = to.map_or(Unbounded, Excluded);
                    let xs: Vec<ValueId> = m.range((lo, hi)).map(|(_, &x)| x).collect();
                    return reduce(eng, mono, xs);
                }
                None => return mono.empty(eng.values()),
            }
        }
    };
    debug_assert!(whole);
    seg.inner.write().sum(eng, f)
}

/// The declared monoid's fold over the contributions to `f` after `from`
/// (None: the start) and before `to` (None: the end): partial sums at the
/// two ends, the segment sums between, combined in order.
pub(crate) fn fold(eng: &Engine, f: Field, from: Option<Pos>, to: Option<Pos>) -> ValueId {
    let mono = eng.monoid(f);
    let v = eng.values();
    let fs = from.and_then(|p| eng.seg_for_pos(p));
    let ts = to.and_then(|p| eng.seg_for_pos(p));
    if let (Some(a), Some(b)) = (&fs, &ts)
        && a.id == b.id
    {
        return segment_part(eng, &*mono, a, f, from, to);
    }
    let mut acc = mono.empty(v);
    let start = match &fs {
        Some(s) => {
            acc = segment_part(eng, &*mono, s, f, from, None);
            Pos(s.lo.0 + 1)
        }
        None => Pos(0),
    };
    let end = ts.as_ref().map_or(Pos(u64::MAX), |s| s.lo);
    for sid in eng.contribs.between(f, start, end) {
        let s = eng.seg(sid);
        let x = segment_part(eng, &*mono, s, f, None, None);
        acc = mono.combine(v, acc, x);
    }
    if let Some(s) = &ts {
        let x = segment_part(eng, &*mono, s, f, None, to);
        acc = mono.combine(v, acc, x);
    }
    acc
}

/// The first contribution to `f` after `after` (None: the start), or the
/// first node after it that has not committed yet: a contribution that may
/// still come is not the first.
pub(crate) fn next_contribution(eng: &Engine, f: Field, after: Option<Pos>) -> Next {
    let mut cur: Option<SegId> = match after {
        Some(p) => eng.seg_for_pos(p).map(|s| s.id).or_else(|| eng.order.first()),
        None => eng.order.first(),
    };
    let mut lower = after;
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        let found = {
            let inner = seg.inner.read();
            let lo = lower.map_or(Unbounded, Excluded);
            let mut hit = None;
            for (p, id) in inner.nodes.range((lo, Unbounded)) {
                if let Some(&x) = inner.contribs.get(&f).and_then(|m| m.get(p)) {
                    hit = Some(Next {
                        item: Some((*id, x)),
                        stop: Some(*id),
                    });
                    break;
                }
                if eng.node(*id).is_hole() {
                    hit = Some(Next {
                        item: None,
                        stop: Some(*id),
                    });
                    break;
                }
            }
            hit
        };
        if let Some(n) = found {
            return n;
        }
        lower = None;
        cur = eng.order.next(sid, seg.lo);
    }
    Next { item: None, stop: None }
}

/// A `next` as a value: `[node, value]`.
pub(crate) fn next_value(eng: &Engine, n: &Next) -> Option<ValueId> {
    let v = eng.values();
    n.item.map(|(id, x)| v.list(vec![v.int(i64::try_from(id.raw()).unwrap_or(0)), x]))
}

pub(crate) fn step(eng: &Engine, v: Option<ValueId>, s: Step) -> Option<ValueId> {
    let vals = eng.values();
    match s {
        Step::Present => Some(vals.bool(v.is_some())),
        Step::Index(i) => {
            let x = v?;
            vals.with_list(x, |xs| xs.get(i as usize).copied())
                .unwrap_or_else(|| vals.seq_nth(x, u64::from(i)))
        }
        Step::Last => {
            let x = v?;
            vals.with_list(x, |xs| xs.last().copied()).unwrap_or_else(|| {
                let n = vals.seq_len(x);
                (n > 0).then(|| vals.seq_nth(x, n - 1)).flatten()
            })
        }
        Step::Len => {
            let x = v?;
            vals.with(x, |val| match val {
                Value::List(xs) => Some(vals.int(xs.len() as i64)),
                Value::Str(s) => Some(vals.int(s.len() as i64)),
                Value::Nil => Some(vals.int(0)),
                Value::Seq(n) => Some(vals.int(i64::try_from(n.len).unwrap_or(i64::MAX))),
                _ => None,
            })
        }
        Step::Key(k) => {
            let x = v?;
            vals.with_list(x, |xs| {
                xs.iter()
                    .find_map(|&e| vals.with_list(e, |kv| (kv.first() == Some(&k)).then(|| kv.get(1).copied())).flatten())
                    .flatten()
            })
            .flatten()
        }
        Step::Has(k) => {
            let x = v?;
            vals.with_list(x, |xs| {
                xs.iter()
                    .any(|&e| vals.with_list(e, |kv| kv.first() == Some(&k)).unwrap_or(false))
            })
            .map(|b| vals.bool(b))
        }
        Step::Proj(p) => {
            let x = v?;
            let proj = eng.projection(p)?;
            Some(proj(x, vals))
        }
        Step::Eq(k) => Some(vals.bool(v == Some(k))),
        Step::Cmp(k) => {
            let a = vals.as_int(v?)?;
            let b = vals.as_int(k)?;
            Some(vals.int(match a.cmp(&b) {
                std::cmp::Ordering::Less => -1,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
            }))
        }
        Step::Each(_) | Step::Indices(_) | Step::Keys(_) | Step::Rank(_) | Step::Ranks(_) | Step::Distinct => {
            unimplemented!("path step: next run")
        }
    }
}

/// `true` for an empty, zero, nil or unit value, `false` otherwise
/// (`ProjId::IS_EMPTY`).
pub(crate) fn is_empty_proj(x: ValueId, v: &crate::values::Values) -> ValueId {
    let e = v.with(x, |val| match val {
        Value::Unit | Value::Nil => true,
        Value::Bool(b) => !*b,
        Value::Int(i) => *i == 0,
        Value::Str(s) => s.is_empty(),
        Value::List(xs) => xs.is_empty(),
        Value::Seq(_) => false,
    });
    v.bool(e)
}

/// `p` applied to `v` (`PathId::WHOLE`: `v`).
pub(crate) fn apply_path(eng: &Engine, v: Option<ValueId>, p: PathId) -> Option<ValueId> {
    if p == PathId::WHOLE {
        return v;
    }
    if p == SITE {
        return v;
    }
    let steps = eng.path_steps(p)?;
    let mut cur = v;
    for s in steps {
        cur = step(eng, cur, *s);
        if cur.is_none() && !matches!(s, Step::Present | Step::Eq(_)) {
            return None;
        }
    }
    cur
}

/// `Reaching` through a path, `SITE` giving the defining node.
pub(crate) fn through(eng: &Engine, r: Reaching, p: PathId) -> Option<ValueId> {
    if p == SITE {
        return r.by.map(|n| eng.values().int(i64::try_from(n.raw()).unwrap_or(0)));
    }
    apply_path(eng, r.value, p)
}

/// A node's label now, under its segment's read lock; None when gone.
pub(crate) fn pos_of(eng: &Engine, n: NodeId) -> Option<Pos> {
    let node = eng.node(n);
    loop {
        if node.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(node.seg_hint());
        let inner = seg.inner.read();
        let p = node.pos_hint();
        if inner.nodes.get(&p) == Some(&n) {
            return Some(p);
        }
        if node.state() == State::Gone {
            return None;
        }
    }
}

/// The unit's start as an exclusive lower bound.
pub(crate) fn unit_start(eng: &Engine, me: &Node) -> Pos {
    Pos(eng.unit(me.unit).start.0 - 1)
}

/// The `i`-th pushed sibling after `me` (placements skipped), as a value
/// naming its text: `[op, node, args]`; None past the unit's end.
pub(crate) fn following(eng: &Engine, me: &Node, i: usize) -> Option<Arc<Node>> {
    let mut cur = Some(me.seg_hint());
    let mut lower = Some(me.pos_hint());
    let mut k = 0;
    while let Some(sid) = cur {
        let seg = eng.seg(sid);
        if seg.unit != me.unit {
            return None;
        }
        let hit = {
            let inner = seg.inner.read();
            let lo = lower.map_or(Unbounded, Excluded);
            let mut hit = None;
            for (_, id) in inner.nodes.range((lo, Unbounded)) {
                let n = eng.node(*id);
                if n.parent.is_some() || n.state() == State::Gone {
                    continue;
                }
                if k == i {
                    hit = Some(n.clone());
                    break;
                }
                k += 1;
            }
            hit
        };
        if hit.is_some() {
            return hit;
        }
        lower = None;
        cur = eng.order.next(sid, seg.lo);
    }
    None
}

pub(crate) fn text_value(eng: &Engine, n: &Node) -> ValueId {
    let v = eng.values();
    let args = n.args();
    v.list(vec![
        v.int(i64::from(n.op().0)),
        v.int(i64::try_from(n.id.raw()).unwrap_or(0)),
        v.list_from(&args),
    ])
}

/// The current answer to a recorded read of `me`, through its path: what a
/// re-ask compares with `seen`. A re-ask that finds the same answer leaves
/// the reader standing; that is the whole of validation.
pub(crate) fn answer(eng: &Engine, me: &Node, kind: &ReadKind, path: PathId) -> Option<ValueId> {
    answer_in(eng, me, kind, path, &mut FoldMemo::new())
}

/// The folds one pass of answers computed, by range: a node's reads
/// through many paths of one range fold it once.
pub(crate) type FoldMemo = Vec<((Field, Pos, Pos), ValueId)>;

/// `fold`, from `memo` where this pass computed it.
pub(crate) fn fold_in(eng: &Engine, f: Field, from: Pos, to: Pos, memo: &mut FoldMemo) -> ValueId {
    if let Some(e) = memo.iter().find(|e| e.0 == (f, from, to)) {
        return e.1;
    }
    let x = fold(eng, f, Some(from), Some(to));
    memo.push(((f, from, to), x));
    x
}

/// [`answer`], its folds from `memo`.
pub(crate) fn answer_in(eng: &Engine, me: &Node, kind: &ReadKind, path: PathId, memo: &mut FoldMemo) -> Option<ValueId> {
    match kind {
        ReadKind::Field { f, at } => {
            let r = match *at {
                Anchor::Me => reaching_at(eng, *f, me.id, false)?,
                Anchor::Before(n) => reaching_at(eng, *f, n, false)?,
                Anchor::AfterOwn(n) => reaching_at(eng, *f, n, true)?,
                Anchor::End(u) => reaching(eng, *f, Pos(eng.unit(u).end.0 - 1), true),
            };
            through(eng, r, path)
        }
        ReadKind::Env { f } => apply_path(eng, env_value(eng, *f), path),
        ReadKind::Fold { f, from, to, incl } => {
            let from = match from {
                Some(n) => Some(pos_of(eng, *n)?),
                None => Some(unit_start(eng, me)),
            };
            let to = match to {
                Some(n) => pos_of(eng, *n)?,
                None => pos_of(eng, me.id)?,
            };
            apply_path(eng, Some(fold_in(eng, *f, from?, end_of(to, *incl), memo)), path)
        }
        ReadKind::Next { f, after } => {
            let after = match after {
                Some(n) => Some(pos_of(eng, *n)?),
                None => Some(unit_start(eng, me)),
            };
            let n = next_contribution(eng, *f, after);
            apply_path(eng, next_value(eng, &n), path)
        }
        ReadKind::Placed { i } => {
            let r = me.run_record()?;
            let c = *r.placed.get(*i)?;
            let child = eng.node(c);
            if child.state() == State::Gone {
                return None;
            }
            apply_path(eng, child.run_record().map(|r| r.output), path)
        }
        ReadKind::Following { i } => {
            let n = following(eng, me, *i)?;
            apply_path(eng, Some(text_value(eng, &n)), path)
        }
    }
}

/// A segment-side reader record's current answer.
pub(crate) fn reader_answer(eng: &Engine, f: Field, r: &Reader) -> Option<ValueId> {
    let rc = reaching_at(eng, f, r.anchor, r.inclusive)?;
    through(eng, rc, r.path)
}

/// Whether a range reader record's every path still gives what it saw, its
/// folds from `memo`.
pub(crate) fn range_stands(eng: &Engine, f: Field, r: &RangeReader, memo: &mut FoldMemo) -> bool {
    let v = range_value_in(eng, f, r, memo);
    r.seen.iter().all(|&(p, s)| v.and_then(|v| apply_path(eng, v, p)) == s)
}

/// A range reader's value before its paths (None where an end is gone:
/// every path gives none).
fn range_value_in(eng: &Engine, f: Field, r: &RangeReader, memo: &mut FoldMemo) -> Option<Option<ValueId>> {
    let me = eng.node(r.node);
    let from = match r.from {
        Some(n) => pos_of(eng, n)?,
        None => unit_start(eng, me),
    };
    if r.next {
        let n = next_contribution(eng, f, Some(from));
        return Some(next_value(eng, &n));
    }
    let to = match r.to {
        Some(n) => pos_of(eng, n)?,
        None => pos_of(eng, me.id)?,
    };
    Some(Some(fold_in(eng, f, from, end_of(to, r.incl), memo)))
}

/// A fold's exclusive end at `to`, or just past it where `to`'s own is in
/// (labels are integers: nothing lies between `to` and the next).
pub(crate) fn end_of(to: Pos, incl: bool) -> Pos {
    if incl { Pos(to.0 + 1) } else { to }
}

/// The built-in projections.
pub(crate) fn builtin_projection(p: ProjId) -> Option<crate::api::Projection> {
    (p == ProjId::IS_EMPTY).then(|| Arc::new(is_empty_proj) as crate::api::Projection)
}
