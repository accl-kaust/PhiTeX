//! Answering reads: the reaching definition, a fold, a path.

use std::ops::Bound::{Excluded, Unbounded};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::api::{Brackets, NodeId, PathId, ProjId, Step};
use crate::engine::Engine;
use crate::node::{Anchor, Node, ReadKind, State};
use crate::order::{Pos, SegId};
use crate::segment::{Def, GLOBAL, Inner, RangeReader, Reader, Segment, reduce};
use crate::values::{Field, Value, ValueId, Values};

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
                let d = d.plain(&eng.values);
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
            let d = d.plain(&eng.values);
            return Reaching {
                value: Some(d.value),
                by: Some(d.by),
                provisional: d.provisional,
            };
        }
    }
    before_segment(eng, f, seg.lo)
}

/// `reaching` at a node's label, read under its segment's lock, under
/// scopes ([`reaching_visible`]); None when the node is gone.
pub(crate) fn reaching_at(eng: &Engine, f: Field, anchor: NodeId, inclusive: bool) -> Option<Reaching> {
    if scoped(eng) {
        return reaching_at_visible(eng, f, anchor, inclusive);
    }
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
/// sum when the range covers the whole segment. The monoid runs under no
/// lock: the contributions are copied out under the read lock, and a whole
/// segment's sum is kept after (a short write lock) unless one of its sums
/// was dropped meanwhile.
fn segment_part(eng: &Engine, mono: &dyn crate::api::Monoid, seg: &Segment, f: Field, from: Option<Pos>, to: Option<Pos>) -> ValueId {
    let (xs, whole) = {
        let inner = seg.inner.read();
        let covers = from.is_none_or(|p| p < seg.lo) && to.is_none_or(|p| p >= inner.hi);
        if covers && let Some(&x) = inner.sums.get(&f) {
            return x;
        }
        let Some(m) = inner.contribs.get(&f) else {
            return mono.empty(eng.values());
        };
        let xs: Vec<ValueId> = if covers {
            m.values().copied().collect()
        } else {
            let lo = from.map_or(Unbounded, Excluded);
            let hi = to.map_or(Unbounded, Excluded);
            m.range((lo, hi)).map(|(_, &x)| x).collect()
        };
        (xs, covers.then_some(inner.sums_gen))
    };
    let x = reduce(eng, mono, xs);
    if let Some(seen) = whole {
        let mut inner = seg.inner.write();
        if inner.sums_gen == seen {
            inner.sums.insert(f, x);
        }
    }
    x
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
/// first node after it that has not committed yet and whose operation may
/// contribute `f` (`Operation::contributes`): a contribution that may still
/// come is not the first.
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
                    hit = Some((
                        Next {
                            item: Some((*id, x)),
                            stop: Some(*id),
                        },
                        false,
                    ));
                    break;
                }
                if eng.node(*id).is_hole() {
                    hit = Some((
                        Next {
                            item: None,
                            stop: Some(*id),
                        },
                        true,
                    ));
                    break;
                }
            }
            hit
        };
        match found {
            Some((n, false)) => return n,
            Some((n, true)) => {
                // (asked outside the segment's lock: the node's text has
                // its own)
                let id = n.stop.expect("a hole");
                let h = eng.node(id);
                if eng.op(h.op()).contributes(&h.args(), eng.values(), f) {
                    return n;
                }
                let Some(p) = pos_of(eng, id) else {
                    // (gone meanwhile: searched again from where it was)
                    lower = Some(h.pos_hint());
                    cur = eng.seg_for_pos(h.pos_hint()).map(|s| s.id);
                    continue;
                };
                lower = Some(p);
                cur = eng.seg_for_pos(p).map(|s| s.id);
            }
            None => {
                lower = None;
                cur = eng.order.next(sid, seg.lo);
            }
        }
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
    following_on(eng, me, i, &mut None)
}

/// `following`, going on from `at`, the `j`-th sibling an earlier call
/// found, where `j < i` (siblings read one after another cost one pass,
/// not one from `me` each); `at` becomes this answer.
pub(crate) fn following_on(eng: &Engine, me: &Node, i: usize, at: &mut Option<(usize, NodeId)>) -> Option<Arc<Node>> {
    let (mut k, from): (usize, &Node) = match *at {
        Some((j, n)) if j < i => (j + 1, eng.node(n)),
        _ => (0, me),
    };
    let mut cur = Some(from.seg_hint());
    let mut lower = Some(from.pos_hint());
    let hit = following_from(eng, me, i, &mut k, &mut cur, &mut lower);
    *at = hit.as_ref().map(|n| (i, n.id));
    hit
}

fn following_from(
    eng: &Engine,
    me: &Node,
    i: usize,
    k: &mut usize,
    cur: &mut Option<SegId>,
    lower: &mut Option<Pos>,
) -> Option<Arc<Node>> {
    while let Some(sid) = *cur {
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
                if n.parent.is_some() || n.state() == State::Gone || !eng.op(n.op()).sibling() || n.held_by_other(me.id) {
                    continue;
                }
                if *k == i {
                    hit = Some(n.clone());
                    break;
                }
                *k += 1;
            }
            hit
        };
        if hit.is_some() {
            return hit;
        }
        *lower = None;
        *cur = eng.order.next(sid, seg.lo);
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
            if *f == eng.depth_field {
                let d = depth_of(eng, me.id)?;
                return apply_path(eng, Some(eng.values().int(i64::from(d))), path);
            }
            let r = match *at {
                Anchor::Me => reaching_at(eng, *f, me.id, false)?,
                Anchor::Before(n) => reaching_at(eng, *f, n, false)?,
                Anchor::AfterOwn(n) => reaching_at(eng, *f, n, true)?,
                Anchor::End(u) => reaching_visible(eng, *f, Pos(eng.unit(u).end.0 - 1), true),
                Anchor::Final => last_definition(eng, *f),
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
    if f == eng.depth_field {
        let d = depth_of(eng, r.anchor)?;
        return apply_path(eng, Some(eng.values().int(i64::from(d))), r.path);
    }
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

/// The last definition of `f` on the whole line (`At::Final`), else the
/// environment: the field index's last segment, its last entry. Globals
/// and locals alike: whatever is last, since a loop-back reader asks what
/// the back of the document left.
pub(crate) fn last_definition(eng: &Engine, f: Field) -> Reaching {
    let mut lo = Pos(u64::MAX);
    while let Some(s) = eng.defs.last_before(f, lo) {
        let seg = eng.seg(s);
        if let Some((_, d)) = seg.inner.read().defs.get(&f).and_then(|m| m.iter().next_back()) {
            return Reaching::of(&d.plain(&eng.values));
        }
        lo = seg.lo;
    }
    env_reaching(eng, f)
}

/// A scope's identity: the node that opened it and which of its opens.
pub(crate) fn opener(n: NodeId, j: u8) -> u64 {
    (n.raw() << 8) | u64::from(j)
}

/// A node's closes applied to the scopes open: one at the root does
/// nothing (TeX's extra `}`, an error it ignores).
pub(crate) fn close(chain: &mut Vec<u64>, b: &Brackets) {
    chain.truncate(chain.len().saturating_sub(usize::from(b.closes)));
}

pub(crate) fn open(chain: &mut Vec<u64>, n: NodeId, b: &Brackets) {
    chain.extend((0..b.opens).map(|j| opener(n, j)));
}

/// Whether any node on the line has brackets: without, every definition
/// reaches every later position and no scope is walked.
pub(crate) fn scoped(eng: &Engine) -> bool {
    eng.bracket_nodes.load(Ordering::Acquire) > 0
}

/// The scopes open at the start of `s`, outermost first: its cached
/// chain if no bracket changed since, else the last segment before it
/// whose chain is known, and on from there through each segment's
/// brackets, each chain found kept. One segment's lock at a time.
pub(crate) fn chain_start(eng: &Engine, s: &Arc<Segment>) -> Arc<[u64]> {
    let g = eng.scope_gen.load(Ordering::Acquire);
    let cached = |t: &Segment| t.inner.read().chain.as_ref().filter(|c| c.0 == g).map(|c| c.1.clone());
    if let Some(c) = cached(s) {
        return c;
    }
    let mut todo: Vec<&Arc<Segment>> = vec![s];
    let mut known: Option<(&Arc<Segment>, Arc<[u64]>)> = None;
    loop {
        let t = *todo.last().expect("a segment");
        let Some(p) = eng.order.prev(t.id, t.lo) else {
            break;
        };
        let ps = eng.seg(p);
        if let Some(c) = cached(ps) {
            known = Some((ps, c));
            break;
        }
        todo.push(ps);
    }
    let store = |t: &Segment, c: &Arc<[u64]>| {
        if eng.scope_gen.load(Ordering::Acquire) == g {
            t.inner.write().chain = Some((g, c.clone()));
        }
    };
    let (mut prev, mut c) = match known {
        Some(k) => k,
        None => {
            let first = todo.pop().expect("a segment");
            let c: Arc<[u64]> = Arc::from(Vec::new());
            store(first, &c);
            (first, c)
        }
    };
    while let Some(t) = todo.pop() {
        c = chain_end(prev, &c);
        store(t, &c);
        prev = t;
    }
    c
}

/// The scopes open at the end of `s`, from those at its start.
fn chain_end(s: &Segment, start: &Arc<[u64]>) -> Arc<[u64]> {
    let inner = s.inner.read();
    if inner.brackets.is_empty() {
        return start.clone();
    }
    let mut c = start.to_vec();
    for (_, (n, b)) in inner.brackets.iter() {
        close(&mut c, b);
        open(&mut c, *n, b);
    }
    Arc::from(c)
}

/// The scopes open at `at` in a segment whose start's are `start`: those
/// before it, with its closes where `closes` says, and its opens too
/// where `opens` does.
pub(crate) fn chain_at(inner: &Inner, start: &[u64], at: Pos, closes: bool, opens: bool) -> Vec<u64> {
    let mut c = start.to_vec();
    for (_, (n, b)) in inner.brackets.range(..at) {
        close(&mut c, b);
        open(&mut c, *n, b);
    }
    if let Some((n, b)) = inner.brackets.get(&at) {
        if closes || opens {
            close(&mut c, b);
        }
        if opens {
            open(&mut c, *n, b);
        }
    }
    c
}

/// Where a reader at `at` looks from: after the closes its node makes
/// before its reads, or, `inclusive`, after the node altogether.
pub(crate) fn reader_chain(inner: &Inner, start: &[u64], at: Pos, inclusive: bool) -> Vec<u64> {
    let before = inner.brackets.get(&at).is_some_and(|(_, b)| b.before);
    chain_at(inner, start, at, inclusive || before, inclusive)
}

/// A definition's scope: its depth (0: it reaches every later position)
/// and the scope it lives in. `closed`: the scopes open at its node after
/// the node's closes; `own`: the node's brackets.
pub(crate) fn level(d: &Def, closed: &[u64], own: Option<&(NodeId, Brackets)>) -> (usize, u64) {
    if d.depth == GLOBAL {
        return (0, 0);
    }
    let k = closed.len();
    if d.depth > 0
        && let Some((n, b)) = own
        && b.opens > 0
    {
        let j = u8::try_from(d.depth - 1).unwrap_or(u8::MAX).min(b.opens - 1);
        return (k + usize::from(j) + 1, opener(*n, j));
    }
    (k, if k == 0 { 0 } else { closed[k - 1] })
}

/// Whether a definition in scope `lv` reaches a reader whose open scopes
/// are `target`: its scope is still open there.
pub(crate) fn visible(target: &[u64], lv: (usize, u64)) -> bool {
    lv.0 == 0 || (target.len() >= lv.0 && target[lv.0 - 1] == lv.1)
}

/// Each definition of `f` in the segment before `limit` (at it too when
/// `incl`; None: all of them), in order, with its scope; `start`: the
/// scopes open at the segment's start.
/// A multi definition gives each of its parts, outermost first.
pub(crate) fn levels(v: &Values, inner: &Inner, f: Field, start: &[u64], limit: Option<Pos>, incl: bool) -> Vec<(Pos, Def, (usize, u64))> {
    let Some(m) = inner.defs.get(&f) else {
        return Vec::new();
    };
    let defs = match limit {
        None => m.iter(),
        Some(p) if incl => m.range(..=p),
        Some(p) => m.range(..p),
    };
    let mut chain = start.to_vec();
    let mut brs = inner.brackets.iter().peekable();
    let mut out = Vec::new();
    for (q, d) in defs {
        while let Some((bp, (n, b))) = brs.peek() {
            if **bp >= *q {
                break;
            }
            close(&mut chain, b);
            open(&mut chain, *n, b);
            brs.next();
        }
        let own = brs.peek().filter(|(bp, _)| **bp == *q).map(|(_, x)| *x);
        let closed = match own {
            Some(x) if x.1.closes > 0 => {
                let mut c = chain.clone();
                close(&mut c, &x.1);
                Some(c)
            }
            _ => None,
        };
        let c = closed.as_deref().unwrap_or(&chain);
        if d.multi {
            for p in d.parts(v) {
                out.push((*q, p, level(&p, c, own)));
            }
        } else {
            out.push((*q, *d, level(d, c, own)));
        }
    }
    out
}

/// The last definition in `levels` a reader with open scopes `target` sees.
fn last_visible(ls: &[(Pos, Def, (usize, u64))], target: &[u64]) -> Option<Def> {
    ls.iter().rev().find(|x| visible(target, x.2)).map(|x| x.1)
}

/// The definitions of `f` before the segment starting at `lo`, by the
/// field index, the last one visible to `target`; else the environment.
fn visible_before(eng: &Engine, f: Field, target: &[u64], lo: Pos) -> Reaching {
    let mut lo = lo;
    while let Some(s) = eng.defs.last_before(f, lo) {
        let seg = eng.seg(s);
        let start = chain_start(eng, seg);
        {
            let inner = seg.inner.read();
            if let Some(d) = last_visible(&levels(&eng.values, &inner, f, &start, None, false), target) {
                return Reaching::of(&d);
            }
        }
        lo = seg.lo;
    }
    env_reaching(eng, f)
}

/// The scope depth at `p`: scopes open just before the node there, its
/// own closes not applied.
pub(crate) fn depth_at(eng: &Engine, p: Pos) -> u32 {
    if !scoped(eng) {
        return 0;
    }
    loop {
        let Some(seg) = eng.seg_for_pos(p) else {
            return 0;
        };
        let start = chain_start(eng, seg);
        let inner = seg.inner.read();
        if p < seg.lo || (p >= inner.hi && eng.order.at_or_after(p) != Some(seg.id)) {
            continue;
        }
        return u32::try_from(chain_at(&inner, &start, p, false, false).len()).unwrap_or(u32::MAX);
    }
}

/// The scope depth at a node (its closes not applied); None when gone.
pub(crate) fn depth_of(eng: &Engine, n: NodeId) -> Option<u32> {
    let node = eng.node(n);
    if !scoped(eng) {
        return (node.state() != State::Gone).then_some(0);
    }
    loop {
        if node.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(node.seg_hint());
        let start = chain_start(eng, seg);
        let inner = seg.inner.read();
        let p = node.pos_hint();
        if inner.nodes.get(&p) != Some(&n) {
            continue;
        }
        return Some(u32::try_from(chain_at(&inner, &start, p, false, false).len()).unwrap_or(u32::MAX));
    }
}

/// [`reaching`] under scopes: the last definition of `f` before `at` that
/// is visible there (`visible`); an invisible one is passed over and the
/// walk goes on to the one before it, across segments by the field index.
pub(crate) fn reaching_visible(eng: &Engine, f: Field, at: Pos, inclusive: bool) -> Reaching {
    if !scoped(eng) {
        return reaching(eng, f, at, inclusive);
    }
    loop {
        let Some(seg) = eng.seg_for_pos(at) else {
            return env_reaching(eng, f);
        };
        let start = chain_start(eng, seg);
        let target = {
            let inner = seg.inner.read();
            if at < seg.lo || (at >= inner.hi && eng.order.at_or_after(at) != Some(seg.id)) {
                continue;
            }
            let target = reader_chain(&inner, &start, at, inclusive);
            if let Some(d) = last_visible(&levels(&eng.values, &inner, f, &start, Some(at), inclusive), &target) {
                return Reaching::of(&d);
            }
            target
        };
        return visible_before(eng, f, &target, seg.lo);
    }
}

/// [`reaching_at`] under scopes.
fn reaching_at_visible(eng: &Engine, f: Field, anchor: NodeId, inclusive: bool) -> Option<Reaching> {
    let n = eng.node(anchor);
    loop {
        if n.state() == State::Gone {
            return None;
        }
        let seg = eng.seg(n.seg_hint());
        let start = chain_start(eng, seg);
        let target = {
            let inner = seg.inner.read();
            let p = n.pos_hint();
            if inner.nodes.get(&p) != Some(&anchor) {
                continue;
            }
            let target = reader_chain(&inner, &start, p, inclusive);
            if let Some(d) = last_visible(&levels(&eng.values, &inner, f, &start, Some(p), inclusive), &target) {
                return Some(Reaching::of(&d));
            }
            target
        };
        return Some(visible_before(eng, f, &target, seg.lo));
    }
}
